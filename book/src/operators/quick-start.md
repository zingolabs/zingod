# Quick start

## Requirements

- A Zcash validator with JSON-RPC enabled: [Zebra](https://github.com/ZcashFoundation/zebra)
  (`zebrad`, tested with 6.2.3) or [Zakura](https://zakura.com/) (`zakurad`). Set
  `[rpc] listen_addr` in its config.
- Local storage for the indexes, preferably NVMe ([first sync](first-sync.md)).

Zingod uses these [Zcash JSON-RPC](https://zcash.github.io/rpc/) methods: `getblock`,
`getblockchaininfo`, `getblockhash`, `getblockheader`, `getdeprecationinfo`, `getinfo`,
`getpeerinfo`, `getrawmempool`, `getrawtransaction` and `sendrawtransaction`. If the validator
restricts its RPC methods, allow these. Zingod also calls `checkadmission` when the validator
supports it.

## Run with Docker

1. Generate a config with every key at its default:

   ```sh
   docker run --rm zingodevops/zingod generate-config -o /dev/stdout > zingod.toml
   ```

1. Edit `zingod.toml`:
   - Set `network` to `mainnet`, `testnet` or `regtest`.
   - Set `jsonrpc_address` under `[[trusted_validators]]` to your validator.
   - Set `tls` under `[serve]`. The container listens on all interfaces, so Zingod requires it:
     `tls = "off"` if wallets connect only from this host, or a certificate pair
     ([TLS](network.md#tls)).

1. Start it:

   ```sh
   docker run -d --name zingod -p 127.0.0.1:8137:8137 \
     -v ./zingod.toml:/etc/zingod/zingod.toml:ro \
     -v zingod-data:/var/lib/zingod \
     zingodevops/zingod
   ```

1. Wait for the first sync. It takes about an hour on NVMe with the validator on the same host,
   and longer over a network. An [index snapshot](first-sync.md#index-snapshot) skips most of it.
   Check progress over HTTP on the gRPC port:

   ```sh
   curl -s http://localhost:8137/status                     # state and tip
   curl -sf http://localhost:8137/readyz && echo serving    # 200 once ready
   ```

Wallets can then connect to `http://localhost:8137`. See [Docker](docker.md) for volumes, TLS
mounts and Compose.

## Run on a host

Building requires `protoc` on `PATH`.

```sh
cargo install --locked --git https://github.com/zingolabs/zingod zingod
zingod generate-config      # writes $XDG_CONFIG_HOME/zingod/zingod.toml
zingod start                # or: zingod start -c /path/to/zingod.toml
```

Run it under a service manager such as [systemd](maintenance.md#systemd). `zingod --help` lists
every subcommand.

## Try it on the Public Testnet

[`zebrad.toml`](zebrad.toml) and [`zingod.toml`](zingod.toml) are a matching pair of configs for
the Public Testnet, with zebrad's JSON-RPC on `127.0.0.1:18232`.

1. Install zebrad: `cargo install zebrad --locked`.
1. In `zebrad.toml`, replace `<PATH_TO_ZEBRA>` with a writable directory. In `zingod.toml`,
   replace `<ZINGOD_DATA>`.
1. Run `zebrad -c zebrad.toml start`.
1. In another shell, run `zingod start -c zingod.toml`.

Zingod serves gRPC on `127.0.0.1:8137`.
