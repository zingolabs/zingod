# Zingod

Zingod is a Zcash indexer written in Rust. `zingod` follows the chain from one or more trusted
validators ([Zebra](https://github.com/ZcashFoundation/zebra) or
[Zakura](https://github.com/zakura-core/zakura)) and, optionally, Zcash p2p peers,
builds its own indexes, and serves light wallets over the lightwalletd-compatible
`CompactTxStreamer` gRPC service.

Validation stays in the validator and indexing moves to Zingod: the two run as separate
processes, on separate hosts if you like, and a new index needs no change to the validator
([who owns what](./docs/architecture.md#1-who-owns-what)).

[![latest release](https://img.shields.io/github/v/tag/zingolabs/zingod?filter=v*&label=latest%20release)](https://github.com/zingolabs/zingod/tags)
[![Docker Hub](https://img.shields.io/docker/v/zingodevops/zingod?sort=semver&label=docker)](https://hub.docker.com/r/zingodevops/zingod)

## Quick start

You need a Zebra or Zakura node with JSON-RPC enabled (`[rpc] listen_addr` in `zebrad.toml` or
`zakurad.toml`). Zingod trusts it for validation and reads every block from it, through the
Zcash JSON-RPC interface that <https://zcash.github.io/rpc/> specifies
([the methods Zingod calls](./docs/running.md#requirements)).

**1. Write a config.** Every key at its default:

```sh
docker run --rm zingodevops/zingod generate-config -o /dev/stdout > zingod.toml
```

Set `network` (`mainnet`, `testnet` or `regtest`) and point `[[trusted_validators]]`
`jsonrpc_address` at your validator. Everything else can stay as it is; the
[annotated config](./docs/example_configs/zingod.toml) explains each key.

**2. Run it.** The image runs as UID 1000, keeps its indexes under `/var/lib/zingod` and serves
gRPC on 8137:

```sh
docker run -d --name zingod -p 8137:8137 \
  -v ./zingod.toml:/etc/zingod/zingod.toml:ro \
  -v zingod-data:/var/lib/zingod \
  zingodevops/zingod
```

**3. Wait for the indexes.** The first start builds every index from the validator: about an hour
on NVMe with the validator on the same host, longer over a network. Or skip the sync and
[bootstrap from a snapshot](#migrating-to-zingod). The gRPC port also answers plain HTTPS
(self-signed by default): `/status` reports the [state](./docs/running.md#states) (`syncing`,
then `serving`) and the tip, and `/readyz` is 200 once wallets can be served:

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

## Migrating to Zingod

Zingod reads nothing from a previous indexer's install: its config, data directory and metrics
are its own. Coming from Zaino: [docs/migrating-from-zaino.md](./docs/migrating-from-zaino.md)
maps every config key and metric family.

1. Write a config with `zingod generate-config` and set your validator's address and
   credentials. An unknown key refuses to start.
2. Give it an empty data directory: `/var/lib/zingod` in the image, `$XDG_DATA_HOME/zingod` on a
   host. Zingod keeps one directory per index under `indexes/`.
3. Either let the first start sync from your validator, or download the indexes: with a fast
   connection, a published snapshot bootstraps every index before boot. Add to `zingod.toml`:

   ```toml
   [snapshot]
   manifest = "https://snapshots.zingolabs.dev/zingod-snapshot-1.0.0/manifest.json"
   ```

   The image ships the downloader (`aria2c`); on a host install it must be on `PATH`. Only empty
   index directories are filled, each archive's hash is checked, and a restart resumes
   ([index snapshot](./docs/running.md#index-snapshot)).
4. Point dashboards at the `zingod_*` families. Zingod serves gRPC only; node JSON-RPC goes to the
   validator.

## Documentation

- [Architecture](./docs/architecture.md): **start here.** How the pieces fit and why.
- [Running zingod](./docs/running.md): configuration, states, hold, systemd, network exposure.
- [RPC API](./docs/rpc_api.md): the methods served and the status codes a client must tell apart.
- [What the clients actually call](./docs/client-requirements.md): the two wallet sync engines.
- [Docker](./docs/docker.md), [Testing](./docs/testing.md),
  [Updating Zebra](./docs/updating_zebra_crates.md), [Glossary](./CONTEXT.md).
- [CONTRIBUTING.md](./CONTRIBUTING.md), [RELEASING.md](./RELEASING.md),
  [live-test guidelines](./live-tests/CLAUDE.md).

## Security disclosure

Report security issues privately, never in a public issue: see
[CONTRIBUTING.md](./CONTRIBUTING.md#security-issues).

## License

[Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0); see [LICENSE](./LICENSE).
