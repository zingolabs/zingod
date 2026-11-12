# Protocol files

The wallet API is defined by the
[LightWallet Protocol](https://github.com/zcash/lightwallet-protocol) `.proto` files and their
CHANGELOG. There is no ZIP for it. Zingod implements one protocol release and reports it in
`GetLightdInfo.lightwalletProtocolVersion`.

The files below are the ones Zingod builds from. They differ from upstream in `service.proto`:

- `Ping` and `GetBlockNullifiers` are removed, and return `UNIMPLEMENTED`.
- `GetBlockRangeNullifiers` and `GetTaddressTxids` are marked deprecated.

## `service.proto`

```protobuf
{{#include ../../../packages/zingod-proto/proto/service.proto}}
```

## `compact_formats.proto`

```protobuf
{{#include ../../../packages/zingod-proto/proto/compact_formats.proto}}
```
