---
default: minor
---

# zingod: default index directory moves from the cache home to the data home

An unset `index.<name>.path` now defaults to `$XDG_DATA_HOME/zingod/indexes/<name>` (was
`$XDG_CACHE_HOME/...`), where cache cleaners do not delete hours of sync. An install whose indexes
sit at the old default refuses to load its config, naming both directories: move the old
`zingod/indexes` directory under the data home, or set each `index.<name>.path` to it. Paths set in
the config, and the container image (both homes = `/var/lib`), are unaffected.
