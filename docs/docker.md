# Container image

The repo-root `Dockerfile` builds `zingod` as a static musl binary (jemalloc allocator) on Alpine.
It runs as `zingod` (UID and GID 1000), listens for gRPC on `0.0.0.0:8137`, and has a built-in
health check. Entrypoint `zingod`, default command `start`.

```sh
docker build -t zingod .
docker build -t zingod --build-arg CARGO_PROFILE=profiling .   # line tables and frame pointers
```

Both base images are pinned by digest and the Alpine packages on them are not, so a rebuild of
the same commit installs the same packages and a digest bump picks up security fixes. The image
ships `aria2c` (the `[snapshot]` bootstrap's downloader), CA certificates and busybox.

## Run

```sh
docker run --rm zingod generate-config -o /dev/stdout > zingod.toml   # every key at its default
# edit zingod.toml: network, [[trusted_validators]]
docker run -d --name zingod -p 8137:8137 \
  -v ./zingod.toml:/etc/zingod/zingod.toml:ro \
  -v zingod-data:/var/lib/zingod \
  zingod
```

Every other subcommand reads the same config and data: `docker exec zingod zingod hold` beside
the running node, or `zingod verify` in a one-off container with the same two mounts.

## Paths

| Path                      | Holds                                                        | Mount                     |
| ------------------------- | ------------------------------------------------------------ | ------------------------- |
| `/etc/zingod/zingod.toml`  | the config (zingod's default path)                           | a file, read-only         |
| `/etc/zingod/tls/`         | the `[serve.tls]` and `indexer_tls` PEM files, by convention | a directory, read-only    |
| `/var/lib/zingod/`         | `indexes/`, `state/` (the `zingod hold` marker), `peers/`    | a volume, writable by 1000 |

The image sets `XDG_CONFIG_HOME=/etc` and `XDG_CACHE_HOME=XDG_DATA_HOME=/var/lib`, so a config
that names no path keeps everything zingod writes under `/var/lib/zingod`. A named volume takes the
image's ownership on first use. A bind mount must be writable by UID 1000:
`mkdir -p ./data && chown 1000:1000 ./data`. Under rootless Podman, `--userns=keep-id` maps UID
1000 to your host user.

## Configure

The image reads the same `zingod.toml` as any other install, layered under `ZINGOD_CONFIG_*`
environment variables ([running.md: Configuration](./running.md#configuration)). The file is
required: `[[trusted_validators]]` is an array of tables, which environment variables cannot
express. Everything else can come from either place:

```sh
docker run ... -e ZINGOD_CONFIG_SYNC__CONCURRENCY=64 -e RUST_LOG=info,zingod_grpc=debug zingod
```

The image's one default that differs from a host install is
`ZINGOD_CONFIG_SERVE__GRPC_LISTEN_ADDRESS=0.0.0.0:8137` (zingod's own default, `127.0.0.1`, is
unreachable through a published port). To scrape `/metrics`, set
`ZINGOD_CONFIG_METRICS__LISTEN_ADDRESS=0.0.0.0:9999`: zingod warns at boot that it binds a
non-private address, so publish that port only to the scraper
([running.md: Network exposure](./running.md#network-exposure)).

## TLS

Mount the certificate directory, not the files. zingod re-reads the pair every 60 s, and a
single-file bind mount keeps the old inode when a renewal replaces the file, so the new
certificate would never arrive.

```sh
docker run ... \
  -v /etc/letsencrypt/live/zingod.example.com:/etc/zingod/tls:ro \
  -e ZINGOD_CONFIG_SERVE__TLS__CERT_PATH=/etc/zingod/tls/fullchain.pem \
  -e ZINGOD_CONFIG_SERVE__TLS__KEY_PATH=/etc/zingod/tls/privkey.pem \
  zingod
```

The key must be readable by UID 1000. The same directory holds the client certificates for a
validator's `[trusted_validators.indexer_tls]`, named in `zingod.toml`.

## Health check

The built-in check GETs the public `/readyz` on `127.0.0.1:8137`, over plaintext or, with
`[serve.tls]`, over HTTPS. It passes while zingod is `serving` or `degraded`, so a fresh node
reports unhealthy until its indexes reach the tip
([states](../packages/zingod/usage.md#the-admin-listener)). With the gRPC listener on another
port, override it:

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
      - "8137:8137"
    volumes:
      - ./zingod.toml:/etc/zingod/zingod.toml:ro   # [[trusted_validators]] jsonrpc_address = "zebra:8232"
      - zingod-data:/var/lib/zingod
    depends_on:
      - zebra

  zebra:
    image: zfnd/zebra:latest
    volumes:
      - zebra-data:/home/zebra/.cache/zebra
    # [rpc] listen_addr must be reachable from zingod

volumes:
  zingod-data:
  zebra-data:
```

`jsonrpc_address` accepts a hostname, resolved on each connection, so a Compose service name
works.
