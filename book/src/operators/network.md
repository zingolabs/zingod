# Network exposure

Zingod has two listeners:

- **gRPC** (`[serve] grpc_listen_address`) serves wallets, plus `/readyz` and `/status`, which are
  safe to expose publicly.
- **Admin** (`[metrics] listen_address`, off by default) serves metrics and probes, with no
  authentication or encryption. Metrics include validator addresses. Keep it on loopback or a
  private network; Zingod warns at startup if it is bound to a public address.

Connections to validators are plain HTTP JSON-RPC.

Limits on connections, streams and reads are set under `[grpc]`; see the
[reference](configuration.md#reference). When a limit is reached, the request is refused, not
queued.

## TLS

`[serve] tls` controls TLS on the gRPC listener:

| Value                     | Behaviour                                                                       |
| ------------------------- | ------------------------------------------------------------------------------- |
| unset                     | plaintext on a loopback address; any other address refuses to start            |
| `{ cert_path, key_path }` | TLS with the given PEM files                                                    |
| `"off"`                   | plaintext on any address, for use behind a TLS-terminating proxy (warns at startup) |
| `"self_signed"`           | TLS with a generated certificate, for clients that pin it                      |

```toml
[serve.tls]
cert_path = "/etc/zingod/fullchain.pem"   # chain, leaf first
key_path = "/etc/zingod/key.pem"          # PKCS#8, PKCS#1 or SEC1
```

An invalid pair stops startup. Zingod checks the files every minute and picks up a renewed
certificate without a restart; if the new pair fails to load, it keeps serving the old one.

With `"self_signed"`, Zingod generates an ECDSA P-256 certificate with no expiry, stores it in
`<state_dir>/tls`, and logs its SHA-256 fingerprint at each start. It covers `localhost`, the
loopback addresses and the listen address. Wallets that verify against system roots, including
zingolib, reject it, so public servers need a CA-issued certificate. To reissue it, for example
after changing the listen address, delete `cert.pem`.

## Browser clients (gRPC-web)

`[serve] grpc_web = true` serves browser wallets over gRPC-web on the gRPC listener, with CORS
allowing any origin. Native gRPC is unaffected.

Enabling it makes the server usable from any website. A popular site embedding it can send load
proportional to its own traffic.

`zingod origin` manages per-origin policy. Changes apply within a second and persist until
changed:

| Command                         | Effect                                                         |
| ------------------------------- | -------------------------------------------------------------- |
| `zingod origin block <domain>`  | refuse requests from the origin with `PERMISSION_DENIED`       |
| `zingod origin allow <domain>`  | serve the origin even when a parent domain is blocked          |
| `zingod origin reset <domain>`  | remove the origin's policy                                     |
| `zingod origin`                 | list the policies in effect                                    |

- Policies apply to the registrable domain and its subdomains. The most specific policy wins, so
  `allow app.example.com` can override `block example.com`.
- Every origin that is not blocked meets the same limits as any other client. Clients can set any
  `Origin` header, so an origin never lifts a limit.
- `zingod_grpc_origin_requests_total{verdict}` counts browser requests by policy. To find an
  abusive origin, check your proxy or edge logs.

Over HTTP/1.1, browsers open one connection per in-flight stream, each counting against
`max_connections_per_ip`. Serve over TLS so browsers use HTTP/2.

## Stream timeouts

Streams have no total time limit. Two settings close streams that stop making progress:

- `unread_timeout_secs`: the client stopped reading. The connection is closed.
- `silent_timeout_secs`: the server has had nothing to send. The stream ends with `UNAVAILABLE`.
  `GetMempoolStream` is exempt.

## Behind a proxy

Behind a proxy, every connection comes from the proxy's address, so `max_connections_per_ip` would
limit all clients together. To limit per client, list the proxy in `trusted_proxies` and have it
send a [PROXY protocol](https://www.haproxy.org/download/2.9/doc/proxy-protocol.txt) header (v1
or v2):

```toml
[serve]
tls = "off"   # the proxy terminates TLS

[grpc]
trusted_proxies = ["10.0.0.5/32"]
```

Connections from a trusted proxy must start with the header, or they are closed. Other addresses
connect without one.

nginx must use the `stream` module, since `grpc_pass` in the `http` module cannot send the
header:

```nginx
stream {
    server {
        listen 443 ssl;
        ssl_certificate     /etc/ssl/zingod.pem;
        ssl_certificate_key /etc/ssl/zingod.key;
        ssl_alpn            h2;
        proxy_pass          127.0.0.1:8137;
        proxy_protocol      on;
    }
}
```

For HAProxy, use `server zingod 127.0.0.1:8137 send-proxy-v2` in a `mode tcp` backend.

If your proxy cannot send the header, leave `trusted_proxies` empty and raise
`max_connections_per_ip` to `max_connections`.

## Open files

Each connection uses a file descriptor. At startup, Zingod raises its soft `RLIMIT_NOFILE` to the
hard limit and refuses to start if `max_connections` plus 1024 does not fit. Raise the hard limit
(`LimitNOFILE=` in systemd, `--ulimit nofile=` in Docker) or lower `max_connections`.
