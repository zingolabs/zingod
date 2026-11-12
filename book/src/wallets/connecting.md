# Connecting

Zingod serves the `CompactTxStreamer` gRPC service from the
[LightWallet Protocol](protocol.md), the same service as lightwalletd. A wallet that works with
lightwalletd works with Zingod without changes. Zingod does not serve JSON-RPC.

## Transport

gRPC over HTTP/2 on a single port, 8137 by default.

Public servers use TLS with a CA-issued certificate. Plaintext is for a wallet on the same host,
or for a server behind the operator's own TLS-terminating proxy. An operator can also run a
self-signed certificate; only clients that pin its fingerprint can connect to it.

## Browser clients

If the operator enables gRPC-web, browsers can connect on the same port. CORS allows any origin.
With gRPC-web off, browser requests fail CORS. Native gRPC clients work either way.

- Over HTTP/1.1, each in-flight stream uses its own connection, and connections are limited per
  client address. Connect over TLS so the browser can use HTTP/2.
- Operators can block an origin. Requests from a blocked origin fail with `PERMISSION_DENIED`.

## Server info

`GetLightdInfo` is answered from Zingod's own state, so it works even when the server's
validators are unreachable.

| Field                           | Value                                                                  |
| ------------------------------- | ---------------------------------------------------------------------- |
| `vendor`                        | `zingod`                                                               |
| `chainName`                     | `main`, `test` or `regtest`                                            |
| `blockHeight`                   | the served tip (see [heights](methods.md#heights))                    |
| `estimatedHeight`               | the best block Zingod has verified; higher than `blockHeight` while syncing |
| `consensusBranchId`             | the consensus branch at `blockHeight`                                  |
| `upgradeName`, `upgradeHeight`  | the next network upgrade after `blockHeight`, or empty and 0           |
| `lightwalletProtocolVersion`    | the [protocol release](protocol.md) Zingod implements                  |

## Connection limits

Defaults are shown; operators can change them.

| Limit                                | Default                                    |
| ------------------------------------ | ------------------------------------------ |
| concurrent streams per connection    | 100                                        |
| connections per client address       | 128                                        |
| connection lifetime                  | 30 minutes (±10%), then HTTP/2 GOAWAY      |
| idle connection                      | closed after 30 minutes                    |
| in-flight streams after a GOAWAY     | 10 minutes to finish                       |
| client not reading a stream          | connection closed after 5 minutes          |
| stream with nothing to send          | ends `UNAVAILABLE` after 5 minutes (except `GetMempoolStream`) |

Streams have no total time limit, so a full-chain `GetBlockRange` runs for as long as the client
keeps reading. gRPC libraries reconnect after a GOAWAY on their own. A server being taken out of
service also sends GOAWAY, so reconnecting is how a wallet moves to another server.

## Health check

The gRPC port also answers two HTTP paths, useful for picking a server:

- `GET /readyz`: `200` when the server is ready for wallets, otherwise `503`.
- `GET /status`: JSON with the server's state, tip and served methods
  ([format](../operators/monitoring.md#endpoints)).
