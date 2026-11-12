# Migrating from Zaino

Zingod 1.0 replaces Zaino 0.10. It is a new daemon, not an upgrade: it reads nothing from a Zaino
install, so set it up fresh and delete Zaino's data once Zingod is serving.

| Zaino 0.10                                                     | Zingod 1.0                                                          |
| -------------------------------------------------------------- | ------------------------------------------------------------------- |
| repo `zingolabs/zaino`, tags `zainod-X.Y.Z`                    | repo `zingolabs/zingod`, tags `vX.Y.Z`                              |
| binary `zainod`                                                | binary `zingod`                                                     |
| image `zingodevops/zainod`, `/app/config`, `/app/data`         | image `zingodevops/zingod`, `/etc/zingod/zingod.toml`, `/var/lib/zingod` |
| `$XDG_CONFIG_HOME/zaino/zainod.toml`, `ZAINO_*` variables      | `$XDG_CONFIG_HOME/zingod/zingod.toml`, `ZINGOD_CONFIG_*` variables  |
| one LMDB database                                              | one directory per index                                             |
| cargo features (`prometheus`, `no_tls_*`, …)                   | none; every feature is always built                                 |

## Config

Start from `zingod generate-config`; unknown keys are rejected, so a Zaino config will not load.
Carry these values over:

| Zaino 0.10                                                                   | Zingod 1.0                                                   |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------ |
| `network = "Mainnet"`                                                        | `network = "mainnet"` (lowercase)                            |
| `[validator_settings]` `validator_jsonrpc_listen_address`, `validator_cookie_path`, `validator_user`, `validator_password` | one `[[trusted_validators]]` table per validator: `jsonrpc_address`, `cookie_path`, `user`, `password` |
| `grpc_settings.listen_address`, `[grpc_settings.tls]`                        | `serve.grpc_listen_address`, `serve.tls` ([TLS](network.md#tls)) |
| `metrics_endpoint`                                                           | `metrics.listen_address`                                     |
| `backend`, `zebra_db_path`, `ephemeral_finalised_state`, `json_server_settings`, `storage`, `mempool`, `donation_address` | removed                                                      |

## Data

Zingod builds its indexes on first start, or installs them from an
[index snapshot](first-sync.md#index-snapshot). Plan for the [first sync](first-sync.md).

## APIs

- Zingod serves no JSON-RPC. Zaino's JSON-RPC server and port 8237 are gone; send node RPC to the
  validator.
- `zaino.index.v1.IndexedTipService`, `GetBlockNullifiers` and `Ping` return `UNIMPLEMENTED`.
  Poll `GetLatestBlock` instead, and use `GetBlockRange` with `poolTypes`.
- `GetBlockRangeNullifiers` and `GetTaddressTxids` still work but are deprecated.
- See [methods](../wallets/methods.md) and [status codes](../wallets/status-codes.md) for other
  behaviour wallets may notice.

## Metrics

All `zaino_*` and `zainod_*` metrics are gone; `process_*` remains. Closest replacements:

| Zaino                                 | Zingod                                         |
| ------------------------------------- | ---------------------------------------------- |
| `zaino_chain_tip_height`              | `zingod_best_tip`                              |
| `zaino_sync_fetched_height`           | `zingod_fetch_height`                          |
| `zaino_sync_finalized_height`         | `zingod_index_finalized_height{index}`         |
| `zaino_grpc_request_duration_seconds` | `zingod_grpc_duration_seconds{method}`         |
| `zaino_rpc_outbound_*`                | `zingod_validator_rpc_*`                       |
| `zaino_mempool_transactions`          | `zingod_chainview_mempool_transactions{state}` |

Histograms are native histograms ([metrics](monitoring.md#metrics)).

## Logs

Logs are logfmt on stdout, filtered by `RUST_LOG` (default `zingod=info`). `ZAINOLOG_*` is
ignored.
