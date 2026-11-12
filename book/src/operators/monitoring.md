# Monitoring

## States

Zingod reports a single state. The same value appears in `/readyz`, `/status`, the
`zingod_state{state}` metric and the `State changed` log line. The first matching row applies:

| State          | Meaning                                                                                   | `/readyz` |
| -------------- | ----------------------------------------------------------------------------------------- | --------- |
| `held`         | an operator ran [`zingod hold`](maintenance.md#operator-hold)                             | `503`     |
| `down`         | cannot serve: the server is stalled or [overloaded](dos-protection.md#overload), startup is waiting for a validator, or the tip is unverified and stale | `503`     |
| `provisioning` | starting up, or installing an [index snapshot](first-sync.md#index-snapshot)              | `503`     |
| `degraded`     | serving a recent tip that no trusted validator has confirmed (p2p peers supplied it)      | `200`     |
| `syncing`      | serving, but behind the best block; or no validator's mempool is available yet            | `503`     |
| `serving`      | serving at the best block                                                                 | `200`     |

- **Confirmed** means a trusted validator has had the block on its best chain. One block past the
  last confirmation still counts, to cover the validator's polling delay.
- **Recent** means the block's timestamp is within 24 block intervals of the current time.
- A `degraded` node stays in load-balancer rotation so wallets keep being served while every
  trusted validator is down. Alert on the state, not on `/readyz`.
- `syncing` never refuses requests. During a reorg, a node briefly reports `syncing` with the
  `tip unvouched` [alarm](#alarms) raised.

`detail` adds context to the state (`null` until measured):

| State                        | `detail`                                                                           |
| ---------------------------- | ---------------------------------------------------------------------------------- |
| `provisioning`               | `{phase, bytes, total_bytes, bytes_per_sec}` for a snapshot install; else `null`   |
| `syncing`                    | `{height, target, blocks_per_sec}`: served height, best height, rate over ~60 s    |
| `serving`                    | `{start, mbps}`: gRPC egress over the last full 10-minute UTC window               |
| `degraded`, `down`, `held`   | `{reason, message}`                                                                |

`reason` is one of `no_validator_reachable`, `tip_unvouched`, `tip_stale`, `runtime_stalled`,
`overloaded`, `goaway_pending` or `goaway_sent`.

## Endpoints

| Listener | Path       | Response                                                       |
| -------- | ---------- | -------------------------------------------------------------- |
| gRPC     | `/readyz`  | `{"state", "detail"}`, `200` or `503`                          |
| gRPC     | `/status`  | public status (below), always `200`                            |
| admin    | `/readyz`  | same as on the gRPC listener                                   |
| admin    | `/livez`   | `503` after 30 seconds without a heartbeat from the server     |
| admin    | `/metrics` | Prometheus metrics                                             |

The gRPC listener answers these paths over HTTP/1.1 or HTTP/2 from the moment the process starts,
before any gRPC method is available. Each connection may send 10 requests at once, refilling at
one per second; beyond that it gets `429`.

`/status` is safe to expose publicly. It contains no validator addresses, peers, disk details or
mempool contents:

```json
{"state": "serving", "detail": {"start": 1790000400, "mbps": 12.5},
 "chain": {"network": "mainnet", "tip": {"height": 3515406, "hash": "0000…"}, "served": 3515400,
           "validators": {"trusted": 2, "answering": 1}, "alarms": []},
 "build": {"version": "1.0.0"},
 "indexes": {"compact_block": ["GetBlock", "GetBlockRange", "GetBlockRangeNullifiers"], "utxo_set": []}}
```

| Field                   | Meaning                                                              |
| ----------------------- | -------------------------------------------------------------------- |
| `chain.tip`             | the best block Zingod has verified                                   |
| `chain.served`          | the height wallets are served at (`null` before startup completes)   |
| `chain.validators`      | trusted validators configured, and how many are answering            |
| `chain.alarms`          | one message per raised [alarm](#alarms)                              |
| `indexes`               | each enabled index and the methods it serves                         |

## Alarms

Alarms appear in `/status`, and are logged at WARN when raised and INFO when cleared. They do not
change what Zingod serves.

| Alarm           | Raised when                                                                           |
| --------------- | ------------------------------------------------------------------------------------- |
| stale           | a validator's tip is 24 or more blocks behind the height expected from the clock      |
| end of service  | a validator's release reaches its end-of-support height within a week                 |
| finality paused | more than `finalised_depth` non-finalized blocks for 60 s, and finality not advancing |
| tip unvouched   | the best block is more than one block past the last one a trusted validator confirmed |

## Metrics

Every metric has HELP text. Histograms are Prometheus native histograms, available only in the
protobuf format: enable `scrape_native_histograms` in Prometheus or Alloy. The text format carries
every counter and gauge, but no histograms.

Disk space and disk I/O are not exported; use node exporter or cAdvisor.

## Logging

`zingod start` logs to stdout in [logfmt](https://brandur.org/logfmt). `RUST_LOG` sets the filter;
the default is `zingod=info`. An invalid filter stops startup.

```text
ts=… level=info target=zingod::telemetry::progress message="Syncing blocks" height=3503023 target=3506659 bps=10 eta=5m49s component=ZingodNFS
```

- Alert on WARN and ERROR. INFO is progress and lifecycle.
- `component` names the subsystem: `Zingod`, `Admin`, `IndexSnapshot`, `Traffic`, `ChainView`,
  `ZingodSync`, `ZingodNFS`, `GrpcSnapshot`, `Grpc`, or an index name.
- `State changed` is logged within a second of each state change, at ERROR for `down`, WARN for
  `held` and `degraded`, otherwise INFO. `Not ready` repeats every 60 seconds while the state is
  `down`, `degraded` or `held`.
- While syncing, progress lines appear every 30 seconds. `Block fetch stalled` warns when syncing
  stops advancing. `Serving the verified tip` marks the end of the sync.

### The `Serving` line

Once a minute, the gRPC server logs a `Serving` summary: `rps` (requests completed per second)
and `conns` (open connections). If any request was refused (`UNAVAILABLE`, `RESOURCE_EXHAUSTED`)
or failed (`INTERNAL`, `UNKNOWN`, `DATA_LOSS`) that minute, it is logged at WARN with `refused`
and `failed` counts. The first failure of each minute is also logged as `Request failed` (ERROR)
with its message. Everything else is in the [metrics](#metrics).
