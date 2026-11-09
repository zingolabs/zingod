---
default: minor
---

# zingod-source, zingod: validator cookie read again when refused; a local validator's default cookie found

- A cookie (`cookie_path`) is read on the first request and again after the validator answers
  `401` or `403`: a validator restart's new cookie is used from the request after the refusal
  (was 401 until zingod restarted). A cookie not yet written at boot is a retried request
  failure, never a failed start; a cookie that exists and cannot be read fails the start.
- A `[[trusted_validators]]` entry with no credentials and a loopback `jsonrpc_address` sends no
  credentials until the validator answers `401` or `403`, then the validator's default cookie,
  `<cache dir>/zebra/.cookie` or `<cache dir>/zakura/.cookie`, whichever was written last
  (neither = no auth; to pin one, set `cookie_path`). A validator at any other address is never
  sent a local cookie.
