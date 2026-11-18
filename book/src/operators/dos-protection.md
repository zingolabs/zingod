# Load and DoS protection

Zingod refuses work it cannot take on at once instead of queueing it, so a wallet can try another
server. The defaults suit most nodes.

## Settings

| Setting                         | Default  | What it does                                                       |
| ------------------------------- | -------- | ------------------------------------------------------------------ |
| `[grpc] max_connections_per_ip` | `128`    | Connections one address may open; more are closed. High because mobile carriers put many wallets behind one address. Behind a proxy, set [`trusted_proxies`](network.md#behind-a-proxy) so this counts wallets, not the proxy. |
| `[grpc] work_share_per_ip`      | `0.0625` | Share of the request limit one address may use (1/16: it takes 16 addresses to fill a node). `1.0` turns it off, for load tests from a few addresses. |
| `[grpc] read_threads`           | `32`     | Index reads at once (blocks, tree states, transactions).           |
| `[grpc] scan_threads`           | `4`      | Transparent-address history reads at once. Kept apart so a flood of them never slows block reads. |
| `[grpc] unread_timeout_secs`    | `300`    | A connection whose wallet stops reading for this long is closed.   |
| `[serve] max_address_rows`      | `100000` | Transparent receives one address request may walk; more is refused with `RESOURCE_EXHAUSTED`. |

An IPv6 /64 counts as one address. The thread defaults suit NVMe; for other disks:

| Disk                   | `read_threads` | `scan_threads` |
| ---------------------- | -------------- | -------------- |
| NVMe (default)         | 32             | 4              |
| SATA SSD, cloud volume | 16             | 2              |
| HDD                    | 4              | 1              |

## Limits

Fixed, or sized from the host at startup:

| Limit                                  | Value                                                 | Over it                          |
| -------------------------------------- | ----------------------------------------------------- | -------------------------------- |
| connections                            | open-file limit − 1024, or half of memory at 128 KiB each, whichever is lower | closed |
| streams per connection                 | 100                                                   | refused                          |
| requests in progress                   | 2048, lowered while the CPU falls behind (down to 128) | `UNAVAILABLE`                   |
| request size                           | 64 KiB, `SendTransaction` 2 MB                        | `RESOURCE_EXHAUSTED`             |
| request upload time                    | 30 s                                                  | `DEADLINE_EXCEEDED`              |
| responses waiting to be read, requests still uploading | a quarter of memory                   | the heaviest address's fullest connection closed |
| `/readyz`, `/status`                   | 10 at once, then 1 per second, per connection         | `429`                            |

- Startup logs the connection limit as `Connection cap`. Under 256, Zingod refuses to start: raise
  the hard open-file limit (systemd `LimitNOFILE=`, docker `--ulimit nofile=`) or give it more
  memory.
- A refusal ends a wallet's sync: pepper-sync and librustzcash do not retry.

## Browser origins

With [gRPC-web](network.md#browser-clients-grpc-web) on, any website can have browsers call your
server. Policies take effect within a second and survive restarts; the most specific one wins:

| Command                        | Effect                                                    |
| ------------------------------ | --------------------------------------------------------- |
| `zingod origin block <domain>` | refuse it and its subdomains with `PERMISSION_DENIED`     |
| `zingod origin allow <domain>` | serve it even when a parent domain is blocked             |
| `zingod origin reset <domain>` | remove its policy                                         |
| `zingod origin`                | list the policies                                         |

An allowed origin still gets the same limits as everyone else.

## Overload

`/readyz` reports `overloaded` (`503`) once 10% or more of connections, averaged over a minute,
are refused at the request limit or held back by a congested uplink. It clears under 2%. Zingod
keeps serving the whole time; only `/readyz` changes. Refusals past `work_share_per_ip` never
count. `zingod_grpc_overload_share` shows the share by `cause`.

The uplink counts as congested when, for 30 seconds, the median of at least 10 sending
connections queues 50 ms or more, or 2% or more of bytes are resent. Connections through a trusted
proxy are not checked.

## Troubleshooting

| You see                                                   | Cause                          | Do this                       |
| --------------------------------------------------------- | ------------------------------ | ----------------------------- |
| refusals, `adaptive_stream_limit` under 2048               | CPU                            | more cores or nodes           |
| refusals, limit at 2048, `read_wait_seconds{pool="read"}` high | disk                      | faster disk, more RAM         |
| refusals, limit at 2048, `read_wait_seconds{pool="read"}` low  | too many wallets          | more nodes                    |
| `overload_share{cause="network"}`                         | uplink                         | more bandwidth or nodes       |
| `connections_rejected_total` rising                       | connection limits              | check `trusted_proxies`       |
| `admission_rejected_total{reason="client_share"}` rising  | one address sending too much   | nothing: refused for you      |
| `parked_evicted_total` rising                             | clients not reading or uploading slowly | nothing: closed for you |

Metric names are prefixed `zingod_grpc_`.
