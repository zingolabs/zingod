---
default: patch
---

# zingod-grpc: `[grpc]` defaults admit concurrent wallets on one connection or one address

- `max_streams_per_connection` 8 → 100, lightwalletd's `grpc-max-concurrent-streams`. One
  zingolib wallet holds up to 7 streams at once, so 8 left a client that pipelines more
  requests queueing them client-side until its connection closed.
- `max_connections_per_ip` 32 → 128: wallets behind one carrier-grade NAT address share it.
