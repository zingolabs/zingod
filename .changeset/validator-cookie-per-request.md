---
default: minor
---

# zingod-source, zingod: validator cookie read per request; a local validator's default cookie found

- A cookie (`cookie_path`) is read on every request, not once at boot: a validator restart's new
  cookie is used at once (was 401 until zingod restarted), and a cookie not yet written at boot is
  a retried request failure, never a failed start.
- A `[[trusted_validators]]` entry with no credentials and a loopback `jsonrpc_address` sends the
  validator's default cookie, `<cache dir>/zebra/.cookie` or `<cache dir>/zakura/.cookie`,
  whichever exists (neither = no auth; both = set `cookie_path`). A validator elsewhere is never
  sent a local cookie.
