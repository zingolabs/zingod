# Zingod

Zingod is a Zcash indexer for light wallets. It follows the chain from one or more trusted
validators ([Zebra](https://github.com/ZcashFoundation/zebra) or
[Zakura](https://github.com/zakura-core/zakura)), optionally also from Zcash p2p peers, builds its
own indexes, and serves the lightwalletd-compatible `CompactTxStreamer` gRPC service.

The validator validates. Zingod indexes and serves.

- **[Wallet developers](wallets/connecting.md)**: connecting, how each method behaves, and the
  status codes to handle.
- **[Operators](operators/quick-start.md)**: installing, configuring, monitoring and exposing a
  Zingod server.
- **[Contributors](contributors/architecture.md)**: how Zingod is built, and how it is tested.

Source and releases: [github.com/zingolabs/zingod](https://github.com/zingolabs/zingod).
