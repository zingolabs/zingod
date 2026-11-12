# `zingod` — usage

The daemon binary: config → boot → one task per stage. Operating it (configuration, states,
hold, systemd, index snapshots, `verify`, network exposure):
[the operators chapter](../../book/src/operators/quick-start.md).

## Layout

- `config/`: the file's `[…]` tables (`DaemonConfigFile`) resolved once, at load, into
  `DaemonConfig` (paths set, enabled indexes, validator endpoints, every check passed): the one
  shape `start` and every subcommand run on. Tables land in each crate's own config types; the
  only place daemon config crosses into the stack crates.
- `daemon/`: boot and pipeline composition. The pipeline's shape is drawn once, in
  `daemon.rs`. Every stage is one task in a `JoinSet`; the first to end ends the daemon.
- `health/`: the state rules (`readiness`, pure), `/readyz`, `/status`, the admin listener,
  systemd notify and the state log.
- `telemetry/`: logging, the metrics recorder, disk sampling, sync progress, gRPC egress.
- `command/`: `verify`, `hold` / `resume`, `origin`.

## The admin listener

Runs on its own thread and current-thread runtime: a probe answered from a saturated serving
runtime would measure that runtime's queue, and a timed-out liveness probe gets the pod killed.

- A supervised task on the serving runtime beats the heartbeat every 100 ms; `/livez` fails after
  30 s without one.
- It binds before the metrics recorder installs, so a bind failure fails startup.
- At most 32 admin connections are served at once; the rest wait in the accept backlog.
- Each scrape sets every state gauge into a recorder of its own, from one `GrpcSnapshot` load
  (G9), so a label set the state dropped leaves the scrape.

## Failure policy

zingod dies rather than serve from a state it cannot vouch for
([zingod-persistence: the commit and crash contract](../zingod-persistence/usage.md#the-commit-and-crash-contract)):

- Any panic aborts the process (`main`'s hook prints it to stderr first), including a page
  checksum mismatch on the read that first touches a corrupt page.
- Index writers are infallible and panic on any failure, naming the index
  ([messages](../zingod-sync/usage.md#failure-panic-never-err)).
- Any other task that ends first, cleanly or not, cancels the rest and `zingod start` exits 1
  (`IndexerError::TaskEnded` or the task's own error). `FollowError::Diverged` = the next
  finalized block does not extend an index's stored tip, as for a directory from another chain:
  delete that index's directory to resync it.
- An index directory built for another network or format, shorter than its seals, or whose tail
  page or `.crc` file fails its checksum refuses to open: resync it.
- A trusted validator going away never stops zingod.

## Logging

What zingod logs and what to watch: [operators: logging](../../book/src/operators/monitoring.md#logging).

## Tests

`daemon.rs` tests the task policy (a task ending first cancels the rest and fails the daemon,
a signal stops it clean, during boot too) and the whole pipeline over a `MockValidator` and gRPC
on localhost (`a_reorg_serves_the_heavier_chain_and_every_rpc_agrees_on_the_served_tip`). The state
rules, `/status`, systemd notify, hold, origin markers and `verify` test beside their code.
