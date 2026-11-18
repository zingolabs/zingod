# zingod-grpc

The wallet-facing `CompactTxStreamer` endpoint: one `GrpcService`, every method dispatched by
path over `RouteDeps`. The wallet-facing contract (each method's answers, status codes):
[the book's wallet chapter](../../book/src/wallets/methods.md).

## What answers each method

Derived answers come from a Zingod index, never the validator; consensus objects a validator
already stores are asked of it ([architecture §1](../../book/src/contributors/architecture.md#1-who-owns-what)).

| Method                                                  | Answered by                                                                                                                                    |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `GetLatestBlock`                                        | the served tip (no index read)                                                                                                                 |
| `GetBlock`, `GetBlockRange`                             | compact-block index                                                                                                                            |
| `GetTreeState`, `GetLatestTreeState`, `GetSubtreeRoots` | tree-state index                                                                                                                               |
| `GetAddressUtxos`, `GetAddressUtxosStream`              | transparent-address index                                                                                                                      |
| `GetTaddressBalance`, `GetTaddressBalanceStream`        | transparent-address index                                                                                                                      |
| `GetTaddressTransactions`                               | transparent-address index for the `{height, txid}` set, the bytes as `GetTransaction`'s                                                        |
| `GetTransaction`                                        | validator; none answering: the served mempool's bytes, else the block the txid-location index names, from p2p peers, checked and the transaction cut out |
| `GetStandardFee`                                        | computed (no validator call): `zingod_primitives::protocol::standard_fee` (zebrad's rule), estimator version 0                                |
| `SendTransaction`                                       | chain view [submission](../../book/src/contributors/architecture.md#6-submission): a trusted `checkadmission` answers, then relayed through the peers           |
| `GetMempoolTx`, `GetMempoolStream`                      | chain view [mempool](../../book/src/contributors/architecture.md#5-the-mempool-view): listed by a trusted validator, admitted by a peer quorum, or our own accepted send |
| `GetLightdInfo`                                         | the served `GrpcSnapshot`: its tip, the verified best, the upgrade schedule (no validator call)                                                |
| `GetBlockRangeNullifiers` *(deprecated, TODO: REMOVE)*  | compact-block index, re-projected to nullifiers                                                                                                |
| `GetTaddressTxids` *(deprecated, TODO: REMOVE)*         | as `GetTaddressTransactions`                                                                                                                   |

- "The validator" = the traffic balancer over every `[[trusted_validators]]` entry. An absent
  answer is asked of the next validator (a lagging one may lack a just-mined transaction):
  `NOT_FOUND` only when every one asked said absent.
- Every index can be disabled (at least one stays on); utxo-set, which fills every
  `CompactTx.fee`, runs with compact-block. A disabled index's methods are `UNIMPLEMENTED`; for
  block-hash that is only the by-hash form of `GetBlock` and `GetTreeState`.
- `lightwalletProtocolVersion` = the vendored protos' release (`zingod_proto::LIGHTWALLET_PROTOCOL_VERSION`).
- `SendTransaction` rejections: zebrad's `-1` answers that judge the transaction count as
  rejections; its other `-1`s (queue full, mempool disabled) do not.
- Why the deprecated rows exist:
  [client-requirements.md](../../book/src/contributors/client-requirements.md#two-deprecated-methods-served-until-pepper-sync-moves-off-them).

## RouteDeps

```rust
let routes = RouteDeps {
    grpc_snapshots: publisher.handle(),  // zingod_grpc_snapshot::GrpcSnapshots<V>: one load per request
    chain_view: Arc::clone(&view),       // Arc<ChainView<S>>: SendTransaction
    validators,                          // zingod_traffic::TrafficBalancer<S>: GetTransaction
    network,                             // declared (GetLightdInfo.chainName)
    version: env!("CARGO_PKG_VERSION"),  // the daemon's (GetLightdInfo.version)
    max_address_rows,                    // receives one t-address request may walk
    indexes,                             // Vec<IndexKind>: the configured ones
};
let server = GrpcService::new(grpc_listen_address, limits)  // limits.trusted_proxies included
    .with_retire(retire)                 // the owner's operator hold (zingod_grpc::Retire)
    .with_health(health)
    .with_tls(tls);
let routing = server.routing();
tokio::spawn(server.bind().await?.run(cancel.child_token()));
// … boot …
routing.route(routes);
```

A server binds and runs before its routes exist (an owner still booting or downloading an index
snapshot): it answers health from the start and every method `UNAVAILABLE` (`starting: not routed
yet`) until `route(routes)`. Routing twice panics.

- **One `GrpcSnapshot` per request** (G1): each handler loads one and reads only it for the
  request or the whole stream (`served()?`, `indexed()` for tree states, `mempool()?`,
  `lightd()?`). Exceptions: `SendTransaction` (`chain_view.validate_transaction` →
  `broadcast_transaction`) and `GetTransaction` (`validators.lookup`, then the served mempool's
  bytes, else the peers' block the txid-location index names).
- No height named = the served tip. A height named = up to `IndexesAtBlock::answers_through(kind)`:
  a point read past it is `NOT_FOUND`, a range streams to it, then `OUT_OF_RANGE`.
- By `BlockID.hash`: `height_of(hash)` in the `block_hash` view names the height, and the
  answering index must hold the same hash there (else `NOT_FOUND`). Block-hash disabled = a hash
  request is `UNIMPLEMENTED`.
- An index missing from `RouteDeps::indexes` = disabled: its methods are `UNIMPLEMENTED` naming
  it, served or not. An unknown path is `UNIMPLEMENTED`.
- `zingod_grpc_snapshot::Unavailable` → `UNAVAILABLE` with its message.
- `S` = the validators' `ChainDataSource`; `V` = the indexes' `zingod_persistence::PinnedView`
  (zingod: `DiskView`).
- `limits: GrpcLimits` is zingod's `[grpc]` table as parsed (unknown keys refused).
- `bind().await` takes the socket (a failure = `GrpcServeError::Bind`, so a boot failure). Once
  `cancel` fires, `run`
  closes the listener, sends every connection a GOAWAY, and returns when they have all closed or
  `drain_timeout` has passed. `run` has no failure of its own: accept errors back off and retry.
- `chainName`, the tree state's `network` and the t-address mismatch message spell the network
  as lightwalletd does (`main` / `test` / `regtest`), from `routes::info::chain_name`.

### Adding a method

Every method is one `Method` variant in `service.rs`; its name is its wire name and its `method`
metric label. A new RPC is:

1. a `Method` variant,
2. its arm in `Method::route`: its `Reply` and the index whose view its handler reads (`None` =
   reads none, so never disabled); every method but `GetMempoolStream`
   (`Method::is_subscription`: idle between arrivals) takes a work slot,
3. its arm in `Wired::handle`, which hands the handler its decoded request (`wire::decode`, or
   `wire::decode_each` for a client stream); the handler returns `Result<Response<Body>, Status>`
   built with `wire::unary_response`, `wire::streamed_response` or `wire::streaming`, never by
   hand (`streaming` joins ready records into full DATA frames).

Both matches are exhaustive, so the compiler names the arms a new variant still needs. An `Err`
becomes its status in one place (`Wired::answer`).

### Why `GetTransaction` forwards

Raw transaction bytes are consensus data the validator already stores. Indexing them would be a
second copy of the chain, far larger than the compact store (compact blocks drop proofs,
signatures and 528 of each 580-byte note ciphertext), to serve a read that happens only for
transactions a wallet already trial-decrypted. The address methods are the opposite case: they
are *derived*, so forwarding them would mean a second implementation that can disagree, or a
dependency on a validator index Zebra need not have. The path with no validator answering:
[architecture §1](../../book/src/contributors/architecture.md#1-who-owns-what).

## Serve stack

hyper-util's HTTP/2 server around the path dispatch, not `tonic::transport::Server`, so the
accept loop is zingod's. Outermost first, per connection after the first three:

```text
accept ─ total cap ─ PROXY header (trusted peers) ─ per-client cap ─ TLS ─ h2 / HTTP/1.1
  ─ health (resolves the RPC) ─ CORS + gRPC-web (with_grpc_web) ─ guard (metrics, admission,
    origin gate, stall watch) ─ dispatch (grpc-timeout, request body) ─ handler
```

The guard (`guard.rs`) wraps each response body once: the body owns its admission slot, its
stall-watch ticket and its close-out metrics, so all three end when hyper drops it.

- **Slot** held for the stream's life (bounds work in progress; freed per message, the 2-core
  bench admitted past its CPU: 7× the failed sessions).
- **Client share**: one `stall::Client` (IPv4 address / IPv6 /64) has ≤
  `GrpcLimits::work_share_per_ip` of the limit (`WorkShare::of`, ≥ 1) as live work streams
  (`AdmissionState::clients`, counted until the `Slot` drops, reclaimed or not: no
  reclaim-then-reopen growth; entry gone at 0); refused past it = `Refusal::ClientShare`, never
  the overload's (one client cannot take a node out of rotation).
- **Owing** = handed a frame, not polled since. hyper 1.10 (`PipeToSendStream`) polls a body only
  once its last frame fits the client's window, so owing = waiting on the client (≈ 2 messages
  buffered).
- **Reclaim** (`stall::reclaim_idle`, every 250 ms while `AdmissionState::pressed`): a stream
  owing past `stall::GRACE` (1 s) loses its slot (`Claim::held` swapped, so the sweep and the
  drop never both release). A stream leaves the owing list under the watch's lock before it
  resumes; resuming takes a slot back even past the limit (never refused mid-stream).
- **Parked memory** (`stall::Parked`): each owing ticket charges 4 KiB + 2 × its frame against
  `HostCapacity::parked_bytes`; over it, the client (IPv4 address / IPv6 /64) owing most loses
  its connection owing most (only the connection can go: hyper never polls a stalled body).
- **Body before admission**: the guard collects the request body whole (`wire::collect_request`:
  method cap, 30 s) before `admit`, its bytes charged meanwhile (`stall::Arriving`), so a
  headers-only or trickled request holds no work slot and its buffer stays in the parked budget.
- **Hardware-sized caps** are no config: `HostCapacity` (zingod derives it from the open-file
  limit and memory, `daemon/capacity.rs`), `adaptive::WORK_CEILING`, 100 streams per connection.
  The rest: [the example config](../../book/src/operators/zingod.toml).

- **Health** (`with_health(Arc<dyn Health>)`): `GET /readyz` (`200` / `503`) and `GET /status`
  (always `200`), answered before admission over HTTP/1.1 or h2, so they never take a work slot.
  Each connection may send 10 at once and regains one per second; past that `429`. A non-`GET`
  is `405`, any other path over HTTP/1.x `404` — forwarded to the gRPC-web stack instead when
  `with_grpc_web(true)` (browser gRPC-web and its CORS preflight ride HTTP/1.1).
- **Browser gRPC-web** (`with_grpc_web(true)`): `tower-http` CORS (`*`, grpc-status / grpc-message
  exposed) answers the preflight, `tonic-web` translates gRPC-web framing; native gRPC is
  untouched (CORS short-circuits only an `OPTIONS` preflight). Off = no CORS, so a browser is
  refused. Per-origin policy is the gate below.
- **Origin gate** (`with_origin_gate(Arc<OriginGate>)`, read in `guard.rs`): a request's `Origin`
  → `block` (`PermissionDenied` before admission), else the normal admission path; `allow` only
  carves a host out of a blocked parent, never lifts a cap (`Origin` is client-settable). A
  missing `Origin` (native clients) is default and uncounted. Markers are set operator-side
  (`zingod origin`) and swapped whole each second; `verdict` folds an incoming host to the
  most-specific marker covering it.
- **TLS** (`with_tls(Tls::load(files)?)` or `with_tls(Tls::self_signed(dir, listen)?)`): rustls
  with the aws-lc-rs provider, ALPN `h2` then `http/1.1` (load-balancer monitors connect over
  HTTPS/1.1). A missing or mismatched pair fails `Tls::load`, never plaintext. The files are
  checked every 60 s; a changed pair serves new connections, one that fails to load is logged
  while the previous pair keeps serving. `Tls::self_signed` writes `dir/cert.pem` + `dir/key.pem`
  on first use (SANs: `localhost`, the loopbacks, `listen`'s address) and then loads them the same
  way, so an operator may replace them in place; `leaf_sha256()` is the served certificate's
  fingerprint for the owner's startup log.
- **Connection age and idleness** (gRPC A9). Wallets hold one connection for hours, so without
  an age limit they never re-resolve DNS. At its age (drawn per connection within ±10%, so
  connections opened together do not cycle together) or once idle, a connection gets a GOAWAY;
  its in-flight streams then have `max_connection_age_grace`.
- **Operator hold** (`Retire`, the owner's, handed in with `with_retire`, so a hold may start
  before the server exists). `Retire::hold(delay)`: once `delay` has passed, every connection
  gets the same GOAWAY + grace, those open then and each one accepted while held. A repeated
  `hold` keeps the first deadline; `resume()` lifts it. `Retire::state()` = `Hold::Off`,
  `Draining` or `Drained`.
- **Adaptive limit** (`adaptive.rs`, Netflix concurrency-limits' Gradient2). The input is CPU
  queueing: every connection and stream task is spawned through one `tokio_metrics::TaskMonitor`
  (`transport.rs`'s `Timed` executor), and each 500 ms window's mean woken → polled delay is a
  sample (≥ 10 wakes, else skipped). A task waiting on a read, a client or the network is not
  ready, so none of those count. Every 10 windows (5 s) a decision takes their upper median (a
  block rush moves ≤ 4); over the 100 ms budget three decisions running (15 s) shrinks the limit
  by `clamp(budget / median, 0.5, 1)`, smoothed at 0.2, never under 128; each decision not over
  grows it 10 % toward `WORK_CEILING` (≈ 2.5 min from the floor). Gradient2 proper times whole
  requests (queueing included); zingod's earlier input, its own production time, stayed ~0 ms
  with the bench's CPU saturated. Each decision sets `adaptive_stream_limit`,
  `adaptive_stream_ceiling` and `adaptive_scheduling_delay_seconds`.
- **Overload report.** `GrpcService::admission()` → `AdmissionState::overloaded()`, the
  [overload](#overload) latch, stepped every 10 s (`overload::TICK`).
- **Trusted proxies.** A peer inside `trusted_proxies` must open with a PROXY header (v1 or v2,
  read under 5 s, ≤ 4 KiB), and the per-client cap counts the address it names; `LOCAL` /
  `UNKNOWN` count the proxy itself.
- Each accepted socket gets `TCP_NODELAY` and, on Linux, `TCP_NOTSENT_LOWAT` = 128 KiB: unsent
  bytes wait in h2, not the kernel, so a unary reply is not stuck behind a range already queued
  on the same connection.
- Request bodies are capped before decode ([sizes](../../book/src/wallets/status-codes.md)).
- **Deadline.** `grpc-timeout` is read at arrival (a malformed value is ignored, as tonic
  ignores it). Past it a pending handler answers `DEADLINE_EXCEEDED` and a stream ends with
  `DEADLINE_EXCEEDED` trailers; its slot goes with it, a queued read is skipped, and a read
  already on a thread finishes there (bounded by `read_threads`).
- **Unread.** A body that handed hyper a frame and is not polled again (flow control stalled on
  a client that stopped reading) owes that frame. Once any stream on a connection has owed for
  `unread_timeout`, the connection is closed. An idle stream (a quiet mempool subscription, a
  pending read) never owes.
- **Frames.** Records ready back to back are joined into DATA chunks of up to 16 KiB, and a
  record of 16 KiB or more passes through uncopied. One small frame per record makes an h2
  0.4.16+ client that reads late answer `GOAWAY too_many_data_frames` (lightwalletd #593).
- `method` and `code` metric labels come from fixed sets, so a misbehaving client cannot grow
  the series count.

### Index reads

Every read that touches index files runs on the read pool (`read_pool.rs`): `read_threads`
threads of its own, oldest read first, never a runtime worker and never tokio's blocking pool.
That pool belongs to sync's index writes and the `/metrics` scrape, so a wallet surge cannot delay
the tip or blind monitoring. Address-history scans (`ReadPool::scan`, the three
`transparent_address` index reads) have `scan_threads` threads apart, so they never queue a block
read. At most `read_threads` reads are in flight: a read whose request was
cancelled before a thread took it is skipped, so cancellations cannot stack reads. A read's wait
for a thread is `read_wait_seconds{pool}` (`read` / `scan`); the adaptive limit never sees it (a slow disk fills held
slots instead, and refusals reach the overload latch). Sizing per device:
[operators: settings](../../book/src/operators/dos-protection.md#settings).

### Answered without a read

The answers every synced wallet asks for right after each block never wait for a read thread:

| Answer | Why it needs no read |
|---|---|
| `GetLatestBlock` | the served tip |
| `GetBlock` by height, in the NFS layer | RAM record |
| `GetBlockRange`, in the layer | one RAM record per chunk, projected on read |
| `GetMempoolStream` opening | framed once per served-tip epoch |
| `GetTreeState` (layer heights), `GetLatestTreeState` | framed once per NFS publish |
| `GetSubtreeRoots` | each pool's list framed once per NFS publish; a request = one slice |

The tree-state memos (`memo::PerView`) are keyed on the NFS publish (the `NfsPublication` `Arc`),
not the `GrpcSnapshot`, which moves on every chain-view fold. A new publish starts empty, so
there is no invalidation rule. The first ask per publish computes on a read thread; concurrent
asks await that one computation without holding a thread (`tokio::sync::OnceCell`), and every
later ask is answered inline.

### Overload

`overload.rs`: each 10 s tick's share of connections hurt, smoothed by an EWMA (60 s time
constant, elapsed-weighted, per cause). Hurt = refused at the adaptive limit (`Wallet` mark: once
a tick) or held back by a congested uplink (the sampler's latest mean waiting connections, a
level, as its dumps and the ticks never align). No minimum count: a refusal needs ≥ 128 in
flight, the network part ≥ 10 senders congested 30 s. Rules:
[operators: overload](../../book/src/operators/dos-protection.md#overload).

### Uplink sampler

`tcp_info.rs`, one task per listener (spawned beside the accept loop, ends with it): every 10 s a
`sock_diag` dump of ESTABLISHED sockets on the listener's port, both families, keyed by socket
cookie (never reused; a 4-tuple is, as NAT ports recycle). A socket first seen counts whole (opened
since the last dump); the first dump only primes; a closed one drops out.

- waiting = `tcpi_busy_time` minus its receive-window and send-buffer parts
- floor = the socket's lifetime lowest `min_rtt` (`Seen`: the kernel's windows over
  `tcp_min_rtt_wlen`, 300 s, and a standing queue lifts it); queue delay = srtt above it
- waiting reaches the overload latch only once `Window::congested` (≥ 10 sending sockets, median
  queue delay or retransmitted share over its line) holds unbroken for 30 s (`Congestion`): a full
  uplink stays congested, slow wallets' own queues flap (bench: ≤ 18 s unbroken)

A trusted proxy's sockets are skipped (their state = the proxy's hop). A refused dump stops the task
with one warning; serving never depends on it. Rules:
[operators: overload](../../book/src/operators/dos-protection.md#overload).

### Serving log

`run` logs one `Serving` summary a minute under the caller's span, from the same close-outs the
metrics count; its fields and levels:
[operators: the `Serving` line](../../book/src/operators/monitoring.md#the-serving-line).

- Latency metrics by each `Method`'s `Reply` (a test holds every method to `service.proto`): a
  unary reply records `duration_seconds` (`OK` answers only), a server stream
  `first_message_seconds`.

## Stored bytes on the wire

Compact-block records are stored gRPC-framed (`[0x00][len:be32][message]`), so `GetBlock` and
`GetBlockRange` write the index's bytes (pool-pruned by its `RangeCursor`) straight into the
response body. Only `GetLatestBlock` and the deprecated `GetBlockRangeNullifiers` re-projection
encode messages. Tree-state, transparent-address and chain-view answers are domain values,
framed by their dispatch. `GetMempoolStream`'s opening is one DATA chunk, framed once per
served-tip epoch and shared by refcount, so the per-block reconnect of every wallet costs one
render; each later arrival is its own record.
