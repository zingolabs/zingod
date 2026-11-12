# Configuration

Zingod reads one TOML file: `$XDG_CONFIG_HOME/zingod/zingod.toml`, or the path given with `-c`.
`zingod generate-config` writes a file with every key at its default.

Settings are applied in this order, later ones winning:

1. Built-in defaults.
1. The config file.
1. Environment variables prefixed `ZINGOD_CONFIG_`, with `__` between nested keys. For example,
   `ZINGOD_CONFIG_SYNC__CONCURRENCY=64` sets `concurrency` under `[sync]`.

Every subcommand validates the whole config and refuses to run on an unknown key or invalid value.

Default paths are under the XDG base directories. If there is no home directory (for example,
`HOME` unset under a service manager), set each path explicitly: `serve.state_dir`, the `path` of
every enabled index, and `p2p.cache_dir` if `[p2p]` is enabled.

## Overview

| Key or table             | Purpose                                                                                  |
| ------------------------ | ---------------------------------------------------------------------------------------- |
| `network`                | `mainnet` (default), `testnet` or `regtest`                                              |
| `validator_client`       | how Zingod talks to validators ([below](#validator-client))                              |
| `[[trusted_validators]]` | one table per validator: address, credentials, timeouts, connection budget, priority     |
| `[submission]`           | how `SendTransaction` retries relaying                                                   |
| `[p2p]`                  | Zingod's own Zcash p2p peers (off by default)                                            |
| `[serve]`                | gRPC listen address, TLS, gRPC-web, state directory, drain delay                         |
| `[grpc]`                 | [DoS protection](dos-protection.md) limits and timeouts, trusted proxies, shutdown grace |
| `[metrics]`              | admin listener for metrics and probes (off by default)                                   |
| `[sync]`                 | block fetching and finality depth                                                        |
| `[index.<name>]`         | per index: `enabled` and `path`                                                          |
| `[snapshot]`             | bootstrap empty indexes from an [index snapshot](first-sync.md#index-snapshot)           |

Trusted validators do not vote. Each one adds failover, so two validators survive one going down.

At least one index must be enabled. `utxo_set` is enabled together with `compact_block`.

## Validator client

`validator_client` sets how Zingod talks to every trusted validator:

- `legacy_rpc` (default): JSON-RPC for everything. Works with zebrad, zakurad and zcashd.
- `grpc`: blocks over zebrad's indexer gRPC at each validator's `indexer_address`, everything else
  over JSON-RPC. Requires a zebrad build whose indexer serves `GetBlockRange` (Zingod's Zebra
  fork; stock zebrad does not). Do not use it with zakurad, whose `GetBlockRange` returns a
  different message type.

The indexer gRPC connection is plaintext unless `[trusted_validators.indexer_tls]` configures
mutual TLS.

## Reference

Every key, with its default and a description. This is the
[Public Testnet](quick-start.md#try-it-on-the-public-testnet) `zingod.toml`
([download](zingod.toml)).

```toml
{{#include zingod.toml}}
```
