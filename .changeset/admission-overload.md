---
default: minor
---

# zingod-grpc, zingod: one adaptive work limit, a load-balancer overload signal, no hardware knobs

The adaptive work limit (2048) now sheds only when the CPU falls behind: requests ready to run
wait more than 100 ms for a core (median over 5 seconds, for 15 seconds). Waiting on the disk, a
wallet or the network never counts, and a new block's brief rush never sheds. The Linux PSI
signals are gone. A stream whose wallet stops reading for a second gives its slot up when slots
run short, so streams opened and never read cannot lock other wallets out. A request takes a slot
only once its whole body has arrived. One address (an IPv6 /64 counts as one) holds at most
`[grpc] work_share_per_ip` of the limit (default `0.0625`, `1.0` = none, for load tests), so
filling it takes 16 addresses; `zingod_grpc_admission_rejected_total` gains `reason` (`limit`,
`client_share`).

`/readyz` reports `down` with reason `overloaded` while too many wallets are hurt: refused at the
limit, or held back by a congested uplink (read from the kernel's TCP state every 10 seconds:
most connections queueing 50 ms or more, or 2% of bytes resent, for 30 seconds).
The share of connections hurt is smoothed over 60 seconds; over 10% latches it, under 2%
releases it. See the operators' Load and DoS protection page.

Connection and memory limits come from the host: connections = the open-file limit minus 1024 or
half of memory at 128 KiB each, whichever is lower (logged as `Connection cap`; under 256, zingod
refuses to start). Unread responses get a quarter of memory; past it, the address holding the
most loses its fullest connection.

Index reads run on zingod's own threads (`[grpc] read_threads`, default 32: SATA SSD or cloud
volume 16, HDD 4), so index writes and `/metrics` never wait behind wallet reads.

- Remove from your config, or zingod refuses to start: `[grpc.overload]`, and under `[grpc]`
  `max_connections`, `max_streams_per_connection`, `max_work_streams`, `adaptive_work_streams`,
  `max_subscriptions`, `max_point_reads`, `max_range_reads`, `max_scan_reads`,
  `silent_timeout_secs`
- `zingod overload` is removed; delete `<state_dir>/overload/` if it exists
- Metrics:
  - gone: `zingod_grpc_overload_latched` (use `zingod_state_reason{reason="overloaded"}`),
    `zingod_grpc_active_subscriptions`, `zingod_grpc_subscriptions_rejected_total`
  - renamed: `zingod_grpc_disk_read_wait_seconds{lane}` → `zingod_grpc_read_wait_seconds`
  - new: `zingod_grpc_overload_share{cause}`, `zingod_grpc_adaptive_stream_ceiling`,
    `zingod_grpc_adaptive_scheduling_delay_seconds`, `zingod_grpc_parked_bytes`,
    `zingod_grpc_parked_evicted_total`, `zingod_grpc_uplink_*`
- Logs: the `Serving` line carries `rps`, `conns`, `refused`, `failed` and `evicted` only; new
  `Work limit lowered`/`restored`, `Uplink congested`/`clear`, `Overload latched`/`cleared`
