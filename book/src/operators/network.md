# Network exposure

Zingod has two listeners:

- **gRPC** (`[serve] grpc_listen_address`) serves wallets, plus `/readyz` and `/status`, which are
  safe to expose publicly.
- **Admin** (`[metrics] listen_address`, off by default) serves metrics and probes, with no
  authentication or encryption. Metrics include validator addresses. Keep it on loopback or a
  private network; Zingod warns at startup if it is bound to a public address.

Connections to validators are plain HTTP JSON-RPC.

Limits on connections, requests and reads are set under `[grpc]`; see
[DoS protection](dos-protection.md).

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
proportional to its own traffic. To refuse a site, see [Browser
origins](dos-protection.md#browser-origins).

Over HTTP/1.1, browsers open one connection per in-flight stream, each counting against
`max_connections_per_ip`. Serve over TLS so browsers use HTTP/2.

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
