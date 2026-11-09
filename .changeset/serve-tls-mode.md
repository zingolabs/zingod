---
default: minor
---

# zingod: `[serve] tls` names its mode; TLS off by default on loopback only

`tls` takes `"off"`, `"self_signed"` or `{ cert_path, key_path }` (a `[serve.tls]` table of the
two paths still works). Unset, a loopback `grpc_listen_address` serves plaintext and any other
address refuses to start, naming the three choices: the self-signed default it replaces is a
certificate no wallet verifying against public roots accepts. TLS off away from loopback warns
once at startup, no longer every 15 minutes.

- Was `cert_path = "plaintext"` + `key_path = "plaintext"`: now `tls = "off"` (the old pair is
  refused with that hint)
- Was no `[serve.tls]` off loopback (container image included): set `tls`
