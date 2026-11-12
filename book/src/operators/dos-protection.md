# DoS protection

Zingod protects itself with a fixed set of limits, checked in the order a request meets them. A
request over a limit is refused immediately, never queued, so an overloaded node stays responsive
and clients know at once to back off or try another server.

| Layer         | Bounds                                     | Settings (`[grpc]`)                                      | When exceeded                       |
| ------------- | ------------------------------------------ | -------------------------------------------------------- | ----------------------------------- |
| Connections   | open sockets, in total and per client      | `max_connections`, `max_connections_per_ip`              | connection closed at accept         |
| Streams       | concurrent requests on one connection      | `max_streams_per_connection`                             | HTTP/2 refuses the stream           |
| Work          | requests being served across all clients   | `max_work_streams` (ceiling of the adaptive limit)       | `UNAVAILABLE` with a retry hint     |
| Request size  | request body and address rows              | `[serve] max_address_rows`                               | `RESOURCE_EXHAUSTED`                |
| Progress      | clients that stop reading                  | `unread_timeout_secs`                                    | connection closed                   |
| Origins       | browser sites you block                    | `zingod origin block`                                    | `PERMISSION_DENIED`                 |

The defaults suit a public server on one machine. Every value is in the
[configuration reference](configuration.md#reference).

## Connections

- `max_connections` (4096) caps open connections in total. Each one uses a file descriptor:
  Zingod raises its soft `RLIMIT_NOFILE` to the hard limit at startup and refuses to start if
  `max_connections` plus 1024 does not fit. Raise the hard limit (`LimitNOFILE=` in systemd,
  `--ulimit nofile=` in Docker) or lower `max_connections`.
- `max_connections_per_ip` (128) caps connections from one client address. It is high because
  mobile carriers put many wallets behind one address (CGNAT). Behind a proxy, every connection
  comes from the proxy's address unless it sends a PROXY header; see
  [Behind a proxy](network.md#behind-a-proxy).
- `max_streams_per_connection` (100) caps concurrent requests on one connection, matching
  lightwalletd. A zingolib wallet uses up to 7.

Idle and long-lived connections are recycled: a connection with no open stream for
`max_connection_idle_secs` (30 minutes) gets a GOAWAY, which frees its per-client slot. Every
connection also gets one after `max_connection_age_secs` (30 minutes, ±10%), and its in-flight
streams get `max_connection_age_grace_secs` (10 minutes) to finish.

## Work streams

Every method except `GetMempoolStream` takes a work slot for as long as it runs, including a long
`GetBlockRange`. `GetMempoolStream` stays open until the next block and is idle most of that time,
so it takes none; the connection limits bound it.

The number of slots is not fixed. Zingod measures how long it takes to produce each response
message and adjusts the limit from that, using Netflix's Gradient2 algorithm:

- While the node is busy and response times stay steady, the limit grows slowly.
- When response times rise past 1.5 times their 5-minute average, the limit shrinks within
  seconds: to under half in about 5 seconds at 4 times the usual response time.
- While the node is quiet (fewer than half the slots in use) the limit does not move.
- It starts at 256, never drops below 32, and never exceeds `max_work_streams` (2048).
- Time spent waiting on a slow client does not count. Only time Zingod itself spends counts.

CPU or disk saturation shows up as slower responses, so the limit tracks what the machine can
actually serve. Disk reads have no separate limit: each runs inside a work stream, so a slow disk
lowers the work limit. `max_work_streams` is a safety ceiling for costs that do not show up as
latency, such as memory. You rarely need to change it.

A request over the limit gets `UNAVAILABLE` (`work stream limit reached`) with a
`grpc-retry-pushback-ms: 250` header. Wallets retry after that delay, or move to another server.

The work limit is shared by all clients and filled first come, first served. A single address can
hold up to `max_connections_per_ip × max_streams_per_connection` streams (12,800 by default),
more than the whole work limit, so one aggressive client can take every slot until its streams
finish. On a public server, rate-limit per client address at your proxy or edge, or lower those
two settings if your clients do not share addresses.

## Request size

- A request body over 64 KiB is refused with `RESOURCE_EXHAUSTED`. `SendTransaction` allows
  2 MB + 1 KiB, the largest valid transaction.
- An address request that would walk more than `[serve] max_address_rows` (100,000) received
  transparent outputs, across all its addresses, is refused with `RESOURCE_EXHAUSTED`.

## Slow clients

Streams have no total time limit; a full sync takes as long as it takes. A client's own
`grpc-timeout` is honoured as its deadline. A client that stops reading data Zingod has sent for
`unread_timeout_secs` (300) has its whole connection closed.

`/readyz` and `/status` on the gRPC listener never take a work slot. Each connection may send 10
at once, refilling at one per second; beyond that it gets `429`.

## Browser origins

With [gRPC-web](network.md#browser-clients-grpc-web) enabled, any website can make its visitors'
browsers call your server. `zingod origin` refuses traffic from sites you choose. Changes apply
within a second and persist until changed:

| Command                         | Effect                                                         |
| ------------------------------- | -------------------------------------------------------------- |
| `zingod origin block <domain>`  | refuse requests from the origin with `PERMISSION_DENIED`       |
| `zingod origin allow <domain>`  | serve the origin even when a parent domain is blocked          |
| `zingod origin reset <domain>`  | remove the origin's policy                                     |
| `zingod origin`                 | list the policies in effect                                    |

- Policies apply to the registrable domain and its subdomains. The most specific policy wins, so
  `allow app.example.com` can override `block example.com`.
- Every origin that is not blocked meets the same limits as any other client. Clients can set any
  `Origin` header, so an origin never lifts a limit.
- `zingod_grpc_origin_requests_total{verdict}` counts browser requests by policy. To find an
  abusive origin, check your proxy or edge logs.

## Overload

The work limit decides when a node is overloaded. If at least 10% of work requests are refused,
at one or more refusals a second, for 5 minutes, `/readyz` reports `down` with reason
`overloaded` and the log shows `Overload latched` (WARN). After 5 minutes with fewer than 2%
refused, it clears (`Overload cleared`, INFO).

An overloaded node keeps serving up to its limit. The `down` state tells a load balancer to send
new clients elsewhere until the node recovers. There is nothing to tune: if a node is often
overloaded, add capacity or spread clients across more nodes.

## What to watch

| Metric                                         | Meaning                                                 |
| ---------------------------------------------- | ------------------------------------------------------- |
| `zingod_grpc_adaptive_stream_limit`            | the work limit now                                      |
| `zingod_grpc_active_streams`                   | work streams being served                               |
| `zingod_grpc_admission_rejected_total`         | work requests refused at the limit                      |
| `zingod_grpc_connections_rejected_total`       | connections refused at a connection limit               |
| `zingod_grpc_stalled_connections_total`        | connections closed by `unread_timeout_secs`             |
| `zingod_grpc_overloaded`                       | 1 while the node reports `overloaded`                   |

A work limit that sits well below `max_work_streams` while refusals climb means the machine is the
bottleneck. Refusals with the limit at its ceiling mean `max_work_streams` itself is the
bottleneck. The once-a-minute [`Serving` line](monitoring.md#the-serving-line) summarises
refusals in the log.
