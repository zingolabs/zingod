# Changelog

Every Zingod crate ships at one shared version, so each section below covers the whole
workspace. [Knope](https://knope.tech) writes each section at release from the `.changeset/`
files merged since the last one ([RELEASING.md](./RELEASING.md)).

Before 1.0.0 the project was Zaino, versioned per crate at
[zingolabs/zaino](https://github.com/zingolabs/zaino): that history is in each crate's
`CHANGELOG.md` as of its tag `zainod-0.10.2`.
## 1.1.0 (2026-11-09)

### Features

#### zingod: default index directory moves from the cache home to the data home

An unset `index.<name>.path` now defaults to `$XDG_DATA_HOME/zingod/indexes/<name>` (was
`$XDG_CACHE_HOME/...`), where cache cleaners do not delete hours of sync. Upgrading with indexes at
the old default: move `$XDG_CACHE_HOME/zingod/indexes` to `$XDG_DATA_HOME/zingod/indexes` before
starting, or zingod syncs a new set from scratch. Paths set in the config, and the container image
(both homes = `/var/lib`), are unaffected.

#### zingod: `[serve] tls` names its mode; TLS off by default on loopback only

`tls` takes `"off"`, `"self_signed"` or `{ cert_path, key_path }` (a `[serve.tls]` table of the
two paths still works). Unset, a loopback `grpc_listen_address` serves plaintext and any other
address refuses to start, naming the three choices: the self-signed default it replaces is a
certificate no wallet verifying against public roots accepts. TLS off away from loopback warns
once at startup, no longer every 15 minutes.

- Was `cert_path = "plaintext"` + `key_path = "plaintext"`: replace the table with `tls = "off"`
- Was no `[serve.tls]` off loopback (container image included): set `tls`

#### zingod-source, zingod: validator cookie read again when refused; a local validator's default cookie found

- A cookie (`cookie_path`) is read on the first request and again after the validator answers
  `401` or `403`: a validator restart's new cookie is used from the request after the refusal
  (was 401 until zingod restarted). A cookie not yet written at boot is a retried request
  failure, never a failed start; a cookie that exists and cannot be read fails the start.
- A `[[trusted_validators]]` entry with no credentials and a loopback `jsonrpc_address` sends no
  credentials until the validator answers `401` or `403`, then the validator's default cookie,
  `<cache dir>/zebra/.cookie` or `<cache dir>/zakura/.cookie`, whichever was written last
  (neither = no auth; to pin one, set `cookie_path`). A validator at any other address is never
  sent a local cookie.

### Fixes

#### zingod-grpc: `[grpc]` defaults admit concurrent wallets on one connection or one address

- `max_streams_per_connection` 8 → 100, lightwalletd's `grpc-max-concurrent-streams`. One
  zingolib wallet holds up to 7 streams at once, so 8 left a client that pipelines more
  requests queueing them client-side until its connection closed.
- `max_connections_per_ip` 32 → 128: wallets behind one carrier-grade NAT address share it.

### Internal

#### CI compiles workspace test binaries unoptimized

Dependencies stay at `opt-level = 3`; local test builds unchanged.

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
