# Changelog

Every Zingod crate ships at one shared version, so each section below covers the whole
workspace. [Knope](https://knope.tech) writes each section at release from the `.changeset/`
files merged since the last one ([RELEASING.md](./RELEASING.md)).

Before 1.0.0 the project was Zaino, versioned per crate at
[zingolabs/zaino](https://github.com/zingolabs/zaino): that history is in each crate's
`CHANGELOG.md` as of its tag `zainod-0.10.2`.
## 1.2.0 (2026-11-19)

### Features

#### zingod-grpc, zingod: one adaptive work limit, a load-balancer overload signal, no hardware knobs

- Work limit (2048) sheds only when the CPU falls behind (ready requests wait > 100 ms for a core,
  15 s); disk, wallet and network waits never count. Linux PSI signals gone
- `/readyz` `down`, reason `overloaded`, while > 10% of wallets are refused or held back by a
  congested uplink (60 s average; clears under 2%). Serving continues
- Connections and memory limits come from the host (`Connection cap` at startup; under 256,
  zingod refuses to start)
- Per address (an IPv6 /64 = one): `[grpc] work_share_per_ip` of the work limit, default
  `0.0625` (`1.0` = none, for load tests)
- Requests take a work slot only once their body has arrived; streams whose wallet stops reading
  give theirs up when slots run short
- Index reads on zingod's own threads: `[grpc] read_threads` (32), address-history scans apart in
  `[grpc] scan_threads` (4)
- Remove from your config, or zingod refuses to start: `[grpc.overload]`, and under `[grpc]`
  `max_connections`, `max_streams_per_connection`, `max_work_streams`, `adaptive_work_streams`,
  `max_subscriptions`, `max_point_reads`, `max_range_reads`, `max_scan_reads`,
  `silent_timeout_secs`
- `zingod overload` removed; delete `<state_dir>/overload/` if it exists
- Metrics:
  - gone: `zingod_grpc_overload_latched` (use `zingod_state_reason{reason="overloaded"}`),
    `zingod_grpc_active_subscriptions`, `zingod_grpc_subscriptions_rejected_total`
  - renamed: `zingod_grpc_disk_read_wait_seconds{lane}` → `zingod_grpc_read_wait_seconds{pool}`
  - new: `zingod_grpc_overload_share{cause}`, `zingod_grpc_adaptive_stream_ceiling`,
    `zingod_grpc_adaptive_scheduling_delay_seconds`, `zingod_grpc_parked_bytes`,
    `zingod_grpc_parked_evicted_total`, `zingod_grpc_uplink_*`;
    `zingod_grpc_admission_rejected_total` gains `reason`
- Logs: `Serving` = `rps`, `conns`, `refused`, `failed`, `evicted`; new `Work limit lowered` /
  `restored`, `Uplink congested` / `clear`, `Overload latched` / `cleared`

#### zingod-grpc: gRPC-web traffic by protocol and origin

`zingod_grpc_requests_total` gains a `protocol` label (`grpc`, `grpc_web`). Once a minute, an
`Origin requests` log line names each of the 10 busiest browser origins with its request count
and verdict; one with `origin="(other)"` sums the rest.

#### Serve browser (gRPC-web) clients with an operator origin policy

`[serve] grpc_web = true` serves browser wallets over gRPC-web on the gRPC listener: permissive
CORS answers their preflight, native gRPC is untouched. Off by default, so existing deployments
are unchanged.

`zingod origin block|allow|reset <domain>` (and bare `zingod origin` to list) manage per-origin
policy from markers under `<state_dir>/origins`, picked up by a running zingod within a second,
no restart. `block` refuses an origin and its subdomains (`PermissionDenied`), `allow` carves a
subdomain back out (most-specific marker wins); every unblocked origin meets the same limits as
any client.
Browser load by verdict is the `zingod_grpc_origin_requests_total` metric.

### Fixes

#### Documentation moves to a book at zingolabs.github.io/zingod

All documentation outside the crates now lives in one book, with a part each for wallet
developers (connecting, each method's answers, status codes, the protocol files), operators
(quick start, configuration, first sync, monitoring, maintenance, network exposure, Docker,
migrating from Zaino) and contributors (architecture, testing). The `docs/` directory is gone;
the example configs are in `book/src/operators/`.

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
