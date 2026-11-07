# Changelog

Every Zingod crate ships at one shared version, so each section below covers the whole
workspace. [Knope](https://knope.tech) writes each section at release from the `.changeset/`
files merged since the last one ([RELEASING.md](./RELEASING.md)).

Before 1.0.0 the project was Zaino, versioned per crate at
[zingolabs/zaino](https://github.com/zingolabs/zaino): that history is in each crate's
`CHANGELOG.md` as of its tag `zainod-0.10.2`.
## 1.0.0 (2026-11-07)

### Breaking Changes

#### Zingod 1.0.0: a lightwalletd-compatible indexer over trusted validators

`zingod` reads the chain from one or more Zcash validators it trusts, builds its own indexes and
serves light wallets over the lightwalletd `CompactTxStreamer` gRPC. Docs:
[architecture](https://github.com/zingolabs/zingod/blob/v1.0.0/docs/architecture.md),
[running zingod](https://github.com/zingolabs/zingod/blob/v1.0.0/docs/running.md),
[container image](https://github.com/zingolabs/zingod/blob/v1.0.0/docs/docker.md),
[RPC API](https://github.com/zingolabs/zingod/blob/v1.0.0/docs/rpc_api.md).

- **Validators:** 1 to 64 `[[trusted_validators]]`, all equal, scheduled by one traffic balancer;
  a misanswering validator is benched, a down one retried, and two survive one outage.
  `validator_client = "grpc"` fetches blocks over the validator's indexer gRPC as raw bytes.
- **Peers:** with `[p2p]` on, Zcash peers keep the tip and mempool served through a validator
  outage (state `degraded`), and `SendTransaction` relays through them.
- **Indexes:** compact blocks (each `CompactTx.fee` filled), block hash → height, commitment-tree
  state (Sapling, Orchard, Ironwood) and transparent addresses, each `enabled` and `path`
  separately, each in its own directory with atomic commits and a CRC on every page. Blocks above
  `sync.finalised_depth` stay in memory, so reorgs never touch disk. `zingod verify` checks every
  page offline.
- **Bootstrap:** `[snapshot]` fills empty indexes from a published index snapshot before boot.
- **Submission:** `SendTransaction` asks a validator for admission first, then relays until the
  transaction spreads; rejections carry the validator's own JSON-RPC code, as lightwalletd does.
- **Serving:** TLS on by default (a self-signed pair under `state_dir/tls`; `[serve.tls]` names
  your own PEM pair, or `"plaintext"` for both paths serves in the clear); `[grpc]` connection,
  stream and read caps that refuse rather than queue; overload detection from CPU and I/O
  pressure; PROXY protocol from `trusted_proxies`.
- **Operations:** one state (`provisioning`, `syncing`, `serving`, `degraded`, `held`, `down`) on
  `/readyz`, the public `/status`, the `zingod_state` gauge and the log; `zingod hold` / `resume`
  take a node out of rotation across restarts; systemd `Type=notify`; logfmt logs; Prometheus
  native histograms on the admin `/metrics`.
- **Protocol:** lightwallet-protocol v0.6.0, `GetStandardFee` (ZIP 317), NU7 on Mainnet
  (validators and peers must run Zebra 7.0.0 or later).

Coming from Zaino 0.10: the
[migration guide](https://github.com/zingolabs/zingod/blob/v1.0.0/docs/migrating-from-zaino.md).
