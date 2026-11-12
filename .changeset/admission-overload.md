---
default: minor
---

# zingod-grpc, zingod: one adaptive work limit governs load; fixed caps, pressure signals and overrides removed

The adaptive work-stream limit is now the single authority on load. It always adapts, with
`max_work_streams` as its ceiling; disk reads run inside it with no cap of their own, and
`GetMempoolStream` takes no slot (the connection caps bound it). `/readyz` reports `down` with
reason `overloaded` once at least 10% of work requests are refused (at one or more a second) for
5 minutes, and clears after 5 minutes under 2%. CPU and IO saturation reach it as latency, which
lowers the limit, so the Linux PSI signals are gone. A stream's only time bound is its client's
`grpc-timeout` (and `unread_timeout_secs` for a client that stops reading).

- Remove from your config, or zingod refuses to start: `[grpc.overload]`, and under `[grpc]`
  `adaptive_work_streams`, `max_subscriptions`, `max_point_reads`, `max_range_reads`,
  `max_scan_reads`, `silent_timeout_secs`
- `zingod overload` is removed; delete `<state_dir>/overload/` if it exists
- Metrics: `zingod_grpc_overload_latched{cause}` is now `zingod_grpc_overloaded` (no labels);
  `zingod_grpc_active_subscriptions`, `zingod_grpc_subscriptions_rejected_total` and
  `zingod_grpc_disk_read_wait_seconds` are gone
- `/readyz` message is `Overloaded: refusing work`
- The `Serving` log line carries `rps`, `conns`, `refused` and `failed` only; `Request
  unavailable` is no longer logged
