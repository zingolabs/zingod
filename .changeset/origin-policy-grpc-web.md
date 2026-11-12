---
default: minor
---

# Serve browser (gRPC-web) clients with an operator origin policy

`[serve] grpc_web = true` serves browser wallets over gRPC-web on the gRPC listener: permissive
CORS answers their preflight, native gRPC is untouched. Off by default, so existing deployments
are unchanged.

`zingod origin block|allow|reset <domain>` (and bare `zingod origin` to list) manage per-origin
policy from markers under `<state_dir>/origins`, picked up by a running zingod within a second,
no restart. `block` refuses an origin (`PermissionDenied`), `allow` exempts one from the
work-stream limit, keyed by registrable domain (eTLD+1) with most-specific-marker-wins carve-outs.
Browser load by verdict is the `zingod_grpc_origin_requests_total` metric.
