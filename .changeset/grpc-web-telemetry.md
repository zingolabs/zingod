---
default: minor
---

# zingod-grpc: gRPC-web traffic by protocol and origin

`zingod_grpc_requests_total` gains a `protocol` label (`grpc`, `grpc_web`). Once a minute, an
`Origin requests` log line names each of the 10 busiest browser origins with its request count
and verdict; one with `origin="(other)"` sums the rest.
