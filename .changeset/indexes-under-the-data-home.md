---
default: minor
---

# zingod: default index directory moves from the cache home to the data home

An unset `index.<name>.path` now defaults to `$XDG_DATA_HOME/zingod/indexes/<name>` (was
`$XDG_CACHE_HOME/...`), where cache cleaners do not delete hours of sync. Upgrading with indexes at
the old default: move `$XDG_CACHE_HOME/zingod/indexes` to `$XDG_DATA_HOME/zingod/indexes` before
starting, or zingod syncs a new set from scratch. Paths set in the config, and the container image
(both homes = `/var/lib`), are unaffected.
