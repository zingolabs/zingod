# Maintenance

## Operator hold

`zingod hold` takes a node out of rotation; `zingod resume` puts it back. A hold persists across
restarts, deploys and reboots.

Use a hold before any planned stop. Stopping without one closes the gRPC listener immediately.

```sh
zingod hold            # returns once connections are drained, printing "safe to stop"
systemctl stop zingod
# ... maintenance ...
systemctl start zingod
zingod resume
```

1. `hold` writes a marker file, `held`, in `[serve] state_dir`. It needs write access to that
   directory; there is no network endpoint.
1. Within a second, the running node reports `held` and `/readyz` returns `503`.
1. After `[serve] hold_goaway_delay_secs`, every gRPC connection receives an HTTP/2 GOAWAY, and
   in-flight streams get `max_connection_age_grace_secs` to finish. Set the delay to at least your
   load balancer's failure-detection time plus its DNS TTL.
1. `hold` returns once the GOAWAYs are sent.

A held node keeps running and answering health checks and metrics. After `resume`, a node that
fell behind reports `syncing` until it catches up. A stopped node is never reported as `held`,
so dashboards can tell planned maintenance from a crash.

Both commands are idempotent.

## systemd

```ini
[Service]
Type=notify
ExecStart=/usr/bin/zingod start --config /etc/zingod/zingod.toml
Restart=always
TimeoutStartSec=15min
```

- With `Type=notify`, `systemctl start` returns once Zingod is serving at the tip, or held. A
  deploy that cannot catch up shows as a failed unit.
- While startup makes progress, Zingod extends the start timeout by 5 minutes at a time, so
  `TimeoutStartSec` only bounds a stalled start.
- `TimeoutStopSec` must cover `[grpc] shutdown_grace_secs` plus the time to flush the indexes.
- `NOTIFY_SOCKET` must be a filesystem path, not an abstract socket.

## Stopping and restarting

On SIGINT or SIGTERM, Zingod closes the gRPC listener, sends GOAWAY to every connection, waits up
to `[grpc] shutdown_grace_secs` for in-flight streams, flushes the indexes, and exits 0.

Zingod exits rather than serve data it cannot trust, so run it under a service manager that
restarts it. It exits with code 1 when:

- a validator entry is invalid, the gRPC address cannot be bound, or an index cannot be opened;
- an index directory belongs to another network or format, or is truncated or corrupt;
- an index no longer matches the chain (`Diverged`);
- any internal task stops.

For a corrupt or mismatched index, delete its directory and Zingod resyncs it. An unreachable
validator never stops Zingod.

Restarts are fast. Opening an index checks only its file lengths and last pages, then Zingod
refetches the recent blocks above what was flushed (`Fetching non-finalized blocks` in the log).

## `zingod verify`

`zingod verify -c /etc/zingod/zingod.toml` checks every page checksum of every enabled index. It
only reads, so it is safe to run next to a live node.

It prints a JSON report to stdout and a summary per index to stderr; `--help` lists the exit
codes. Per file, it reports `lost`, `bad_sums` and `bad_pages`. Bytes written by a live node after
its last commit are reported as `orphaned_bytes`, not as corruption.

If any file is corrupt or lost, delete that index's directory and let Zingod resync it.
