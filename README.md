# Zingod

Zingod is a Zcash indexer written in Rust. `zingod` follows the chain from one or more trusted
[Zebra](https://github.com/ZcashFoundation/zebra) validators and, optionally, Zcash p2p peers,
builds its own indexes, and serves light wallets over the lightwalletd-compatible
`CompactTxStreamer` gRPC service.

Validation stays in Zebra and indexing moves to Zingod: the two run as separate processes, on
separate hosts if you like, and a new index needs no change to the validator
([who owns what](./docs/architecture.md#1-who-owns-what)).

[![latest release](https://img.shields.io/github/v/tag/zingolabs/zingod?filter=v*&label=latest%20release)](https://github.com/zingolabs/zingod/tags)
[![Docker Hub](https://img.shields.io/docker/v/zingodevops/zingod?sort=semver&label=docker)](https://hub.docker.com/r/zingodevops/zingod)

## Quick start

You need a Zebra node with JSON-RPC enabled (`[rpc] listen_addr` in `zebrad.toml`). Zingod
trusts it for validation and reads every block from it.

**1. Write a config.** Every key at its default:

```sh
docker run --rm zingodevops/zingod generate-config -o /dev/stdout > zingod.toml
```

Set `network` (`mainnet`, `testnet` or `regtest`) and point `[[trusted_validators]]`
`jsonrpc_address` at your zebrad. Everything else can stay as it is; the
[annotated config](./docs/example_configs/zingod.toml) explains each key.

**2. Run it.** The image runs as UID 1000, keeps its indexes under `/var/lib/zingod` and serves
gRPC on 8137:

```sh
docker run -d --name zingod -p 8137:8137 \
  -v ./zingod.toml:/etc/zingod/zingod.toml:ro \
  -v zingod-data:/var/lib/zingod \
  zingodevops/zingod
```

**3. Wait for the indexes.** The first start builds every index from the validator, or fills
them from a published [index snapshot](./docs/running.md#index-snapshot). The gRPC port also
answers plain HTTPS (self-signed by default): `/status` reports the [state](./docs/running.md#states) (`syncing`, then
`serving`) and the tip, and `/readyz` is 200 once wallets can be served:

```sh
curl -sk https://localhost:8137/status
curl -skf https://localhost:8137/readyz && echo serving
```

Point a light wallet at `localhost:8137`. [Running zingod](./docs/running.md) covers the rest:
TLS, the admin listener and metrics, p2p peers, `zingod hold` for maintenance, systemd, and what
to expose to the internet. [Docker](./docs/docker.md) covers volumes, TLS mounts and Compose.

Without a container (`protoc` on `PATH`):

```sh
cargo install --locked --git https://github.com/zingolabs/zingod zingod
zingod generate-config             # writes $XDG_CONFIG_HOME/zingod/zingod.toml; edit it
zingod start
```

Developing: `cargo nextest run --workspace` runs the unit tests with no validator; see
[CONTRIBUTING.md](./CONTRIBUTING.md) and [Testing](./docs/testing.md).

## Documentation

- [Architecture](./docs/architecture.md): **start here.** How the pieces fit and why.
- [Running zingod](./docs/running.md): configuration, states, hold, systemd, network exposure.
- [RPC API](./docs/rpc_api.md): the methods served and the status codes a client must tell apart.
- [What the clients actually call](./docs/client-requirements.md): the two wallet sync engines.
- [Docker](./docs/docker.md), [Testing](./docs/testing.md),
  [Updating Zebra](./docs/updating_zebra_crates.md), [Glossary](./CONTEXT.md).
- [CONTRIBUTING.md](./CONTRIBUTING.md), [RELEASING.md](./RELEASING.md),
  [live-test guidelines](./live-tests/CLAUDE.md).

### Crate usage guides

All crates live under `packages/`; each guide covers that crate's API, invariants and tests.

- [`zingod`](./packages/zingod/usage.md): the daemon binary.
- [`zingod-grpc`](./packages/zingod-grpc/usage.md): the gRPC routes and serve stack.
- [`zingod-grpc-snapshot`](./packages/zingod-grpc-snapshot/usage.md): the one published `GrpcSnapshot` of everything served.
- [`zingod-chainview`](./packages/zingod-chainview/usage.md): header sync, the mempool view and submission.
- [`zingod-header-chain`](./packages/zingod-header-chain/usage.md): the Validated Header tree, finality, `validate`.
- [`zingod-nfs`](./packages/zingod-nfs/usage.md): the non-finalized state.
- [`zingod-sync`](./packages/zingod-sync/usage.md): the finalized stream and every index writer's loop.
- [`zingod-persistence`](./packages/zingod-persistence/usage.md): the persistence port, `DiskEngine`, the on-disk layout and crash contract.
- [`zingod-index-compact-block`](./packages/zingod-index-compact-block/usage.md): height → gRPC-framed compact block.
- [`zingod-index-utxo-set`](./packages/zingod-index-utxo-set/usage.md): unspent outputs' values (compact-block fees).
- [`zingod-index-tree-state`](./packages/zingod-index-tree-state/usage.md): commitment-tree states and subtree roots.
- [`zingod-index-transparent-address`](./packages/zingod-index-transparent-address/usage.md): the t-address RPCs.
- [`zingod-internal-block-hash-to-height`](./packages/zingod-internal-block-hash-to-height/usage.md): the hash → height locator.
- [`zingod-internal-txid-location`](./packages/zingod-internal-txid-location/usage.md): where each mined transaction sits.
- [`zingod-source`](./packages/zingod-source/usage.md): the `ChainDataSource` port, its JSON-RPC and indexer-gRPC clients, `MockChain`.
- [`zingod-traffic`](./packages/zingod-traffic/usage.md): one scheduler for every request to validators and peers.
- [`zingod-peers`](./packages/zingod-peers/usage.md): zebra-network embedded as Zingod's p2p layer.
- [`zingod-primitives`](./packages/zingod-primitives/usage.md): domain types and protocol constants.
- [`zingod-proto`](./packages/zingod-proto/README.md): vendored lightwallet and Zebra indexer protocols.

## Security disclosure

Report security issues privately, never in a public issue: see
[CONTRIBUTING.md](./CONTRIBUTING.md#security-issues).

## License

[Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0); see [LICENSE](./LICENSE).
