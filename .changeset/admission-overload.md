---
default: minor
---

# zingod-grpc, zingod: one adaptive work limit, a load-balancer overload signal, no hardware knobs

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
