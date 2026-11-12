# Docker

The image `zingodevops/zingod` runs `zingod start` as user `zingod` (UID and GID 1000), listens
for gRPC on `0.0.0.0:8137`, and has a built-in health check. It includes `aria2c` for
[index snapshots](first-sync.md#index-snapshot). To start a container, follow the
[quick start](quick-start.md#run-with-docker).

Other subcommands use the same config and data. Run them in the running container
(`docker exec zingod zingod hold`) or in a one-off container with the same mounts
(`zingod verify`).

## Paths

| Path                      | Contents                                              | Mount                            |
| ------------------------- | ----------------------------------------------------- | -------------------------------- |
| `/etc/zingod/zingod.toml` | the config file                                       | file, read-only                  |
| `/etc/zingod/tls/`        | TLS certificates and keys, by convention              | directory, read-only             |
| `/var/lib/zingod/`        | indexes, state (including the hold marker), peer cache | volume, writable by UID 1000     |

A named volume gets the right ownership automatically. A bind mount must be writable by UID 1000:
`mkdir -p ./data && chown 1000:1000 ./data`. Under rootless Podman, `--userns=keep-id` maps UID
1000 to your user.

## Configuration

The image reads `zingod.toml` like any install, and `ZINGOD_CONFIG_*` environment variables
override it ([configuration](configuration.md)). The file is required, because
`[[trusted_validators]]` cannot be expressed in environment variables.

```sh
docker run ... -e ZINGOD_CONFIG_SYNC__CONCURRENCY=64 -e RUST_LOG=info,zingod_grpc=debug zingodevops/zingod
```

The image differs from a host install in one default: it listens on `0.0.0.0:8137`. Because that
is not a loopback address, `[serve] tls` must be set ([TLS](network.md#tls)).

To expose metrics, set `ZINGOD_CONFIG_METRICS__LISTEN_ADDRESS=0.0.0.0:9999` and publish that port
only to your scraper.

## TLS certificates

Mount the certificate directory, not the individual files. Zingod reloads renewed certificates,
but a single-file bind mount keeps pointing at the old file after renewal.

```sh
docker run ... \
  -v /etc/letsencrypt/live/zingod.example.com:/etc/zingod/tls:ro \
  -e ZINGOD_CONFIG_SERVE__TLS__CERT_PATH=/etc/zingod/tls/fullchain.pem \
  -e ZINGOD_CONFIG_SERVE__TLS__KEY_PATH=/etc/zingod/tls/privkey.pem \
  zingodevops/zingod
```

The key must be readable by UID 1000.

## Health check

The built-in check requests `/readyz` on `127.0.0.1:8137`, over HTTPS without certificate
verification, or over plaintext when `tls = "off"`. It passes in the `serving` and `degraded`
[states](monitoring.md#states), so a new node is unhealthy until it reaches the tip.

If the gRPC listener uses another port, override the check:

```yaml
healthcheck:
  test: ["CMD", "wget", "-q", "-O", "/dev/null", "http://127.0.0.1:9137/readyz"]
```

## Compose

```yaml
services:
  zingod:
    image: zingodevops/zingod:latest
    ports:
      - "127.0.0.1:8137:8137"
    environment:
      ZINGOD_CONFIG_SERVE__TLS: "off"
    volumes:
      - ./zingod.toml:/etc/zingod/zingod.toml:ro   # jsonrpc_address = "zebra:8232"
      - zingod-data:/var/lib/zingod
    depends_on:
      - zebra

  zebra:
    image: zfnd/zebra:latest
    volumes:
      - zebra-data:/home/zebra/.cache/zebra
    # set [rpc] listen_addr so zingod can reach it

volumes:
  zingod-data:
  zebra-data:
```

`jsonrpc_address` accepts a hostname, so a Compose service name works.

## Building the image

```sh
docker build -t zingod .
docker build -t zingod --build-arg CARGO_PROFILE=profiling .   # with symbols and frame pointers
```

The build produces a static musl binary on Alpine. Base images are pinned by digest.
