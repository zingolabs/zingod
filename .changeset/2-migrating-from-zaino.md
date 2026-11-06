---
default: major
---

# Migrating from Zaino 0.10

Zingod 1.0 replaces Zaino: a new daemon, not an upgrade. Nothing from a 0.10 install is read.

| Zaino 0.10 | Zingod 1.0 |
| --- | --- |
| `github.com/zingolabs/zaino`, tags `zainod-X.Y.Z`, per-crate versions | `github.com/zingolabs/zingod`, tag `vX.Y.Z`, one version for every crate |
| binary `zainod`, crates `zaino-*` (on crates.io) | `zingod`, crates `zingod-*` (not published) |
| image `zingodevops/zainod`, user `zaino`, `/app/config`, `/app/data` | `zingodevops/zingod`, user `zingod`, `/etc/zingod/zingod.toml`, `/var/lib/zingod` |
| `$XDG_CONFIG_HOME/zaino/zainod.toml`, `ZAINO_*` overrides | `$XDG_CONFIG_HOME/zingod/zingod.toml`, `ZINGOD_CONFIG_*` (`__` nests) |
| LMDB finalised state under `storage.database` | one directory per index under `<cache dir>/zingod/indexes`; delete the old one |
| cargo features (`prometheus`, `no_tls_*`, …) | none: metrics, admin listener and every index always built |

**Config.** The file is new and an unknown key refuses to start, so begin from
`zingod generate-config` and carry over the values:

| Zaino 0.10 key | Zingod 1.0 key |
| --- | --- |
| `network = "Mainnet" \| "Testnet" \| "Regtest"` | `network = "mainnet" \| "testnet" \| "regtest"` |
| `[validator_settings]` `validator_jsonrpc_listen_address`, `validator_cookie_path`, `validator_user` / `validator_password` | one `[[trusted_validators]]` per validator: `jsonrpc_address`, `cookie_path`, `user` / `password` |
| `grpc_settings.listen_address`, `[grpc_settings.tls]` | `serve.grpc_listen_address`, `[serve.tls]` (default: self-signed; set `tls = "plaintext"` for a plaintext client or proxy) |
| `metrics_endpoint` | `metrics.listen_address` |
| `backend`, `zebra_db_path`, `ephemeral_finalised_state`, `json_server_settings`, `storage`, `mempool`, `donation_address` | removed |

**Data.** The first start syncs every enabled index from the validators, or fills empty ones from
`[snapshot]`, before `/readyz` passes. Plan for the initial sync.

**JSON-RPC.** Zingod serves none: the server, cookie auth and port 8237 are gone. Node RPC goes
to the validator; wallets keep using gRPC.

**gRPC.** `zaino.index.v1.IndexedTipService`, `GetBlockNullifiers` and `Ping` answer
`UNIMPLEMENTED` (poll `GetLatestBlock`; use `GetBlock` or `GetBlockRange` with `poolTypes`).
`GetBlockRangeNullifiers` and `GetTaddressTxids` are served as deprecated aliases. Status codes
and the answers that changed: [RPC API](../docs/rpc_api.md).

**Metrics.** Every `zaino_*` / `zainod_*` family is gone; only `process_*` carries over. The new
families are `zingod_*`, with native histograms
([metrics](../docs/running.md#metrics)). Nearest equivalents: `zaino_chain_tip_height` →
`zingod_best_tip`, `zaino_sync_fetched_height` → `zingod_fetch_height`,
`zaino_sync_finalized_height` → `zingod_index_finalized_height{index}`,
`zaino_grpc_request_duration_seconds` → `zingod_grpc_duration_seconds{method}`,
`zaino_rpc_outbound_*` → `zingod_validator_rpc_*`, `zaino_mempool_transactions` →
`zingod_chainview_mempool_transactions{state}`.

**CLI and logs.** Commands are `start`, `generate-config`, `verify`, `hold`, `resume` and
`overload`. Logs are logfmt on stdout, filtered by `RUST_LOG` (default `zingod=info`);
`ZAINOLOG_*` is not read. Run zingod under a service manager that restarts it: any panic ends the
process.
