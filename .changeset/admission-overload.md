---
default: minor
---

# zingod-grpc, zingod: overload = admission refusing work; no pressure thresholds, no overrides

The adaptive work-stream limit is now the single authority on load. `/readyz` reports `down` with
reason `overloaded` once at least 10% of work requests are refused (at one or more a second) for
5 minutes, and clears after 5 minutes under 2%. CPU and IO saturation reach it as latency, which
lowers the limit, so the separate Linux PSI signals are gone. The limit always adapts, with
`max_work_streams` as its ceiling.

- Remove `[grpc.overload]` (`cpu_pressure_percent`, `io_pressure_percent`) and
  `[grpc] adaptive_work_streams` from your config: zingod refuses to start with them
- `zingod overload` is removed; delete `<state_dir>/overload/` if it exists
- Metric `zingod_grpc_overload_latched{cause}` is now `zingod_grpc_overloaded` (no labels); the
  `/readyz` message is `Overloaded: refusing work`
