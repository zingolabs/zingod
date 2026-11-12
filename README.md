# Zingod

Zingod is a Zcash indexer written in Rust. `zingod` follows the chain from one or more trusted
validators ([Zebra](https://github.com/ZcashFoundation/zebra) or
[Zakura](https://github.com/zakura-core/zakura)) and, optionally, Zcash p2p peers,
builds its own indexes, and serves light wallets over the lightwalletd-compatible
`CompactTxStreamer` gRPC service.

Validation stays in the validator and indexing moves to Zingod: the two run as separate
processes, on separate hosts if you like, and a new index needs no change to the validator
([who owns what](./book/src/contributors/architecture.md#1-who-owns-what)).

[![latest release](https://img.shields.io/github/v/tag/zingolabs/zingod?filter=v*&label=latest%20release)](https://github.com/zingolabs/zingod/tags)
[![Docker Hub](https://img.shields.io/docker/v/zingodevops/zingod?sort=semver&label=docker)](https://hub.docker.com/r/zingodevops/zingod)

## Documentation

**[zingolabs.github.io/zingod](https://zingolabs.github.io/zingod/)** ([source](./book/src/SUMMARY.md)):

- **Wallet developers**: [connecting](./book/src/wallets/connecting.md),
  [methods](./book/src/wallets/methods.md), [status codes](./book/src/wallets/status-codes.md).
- **Operators**: [quick start](./book/src/operators/quick-start.md),
  [configuration](./book/src/operators/configuration.md),
  [network exposure](./book/src/operators/network.md),
  [migrating from Zaino](./book/src/operators/migrating-from-zaino.md).
- **Contributors**: [architecture](./book/src/contributors/architecture.md) (start here),
  [testing](./book/src/contributors/testing.md), [glossary](./CONTEXT.md),
  [CONTRIBUTING.md](./CONTRIBUTING.md), [RELEASING.md](./RELEASING.md).

`cargo nextest run --workspace` runs the unit tests with no validator; `mdbook serve book`
previews the book.

## Security disclosure

Report security issues privately, never in a public issue: see
[CONTRIBUTING.md](./CONTRIBUTING.md#security-issues).

## License

[Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0); see [LICENSE](./LICENSE).
