//! Default on-disk locations (`dirs` = XDG base dirs on Linux, platform dirs elsewhere)
//!
//! - `None` = no home dir (e.g. unset `HOME` under a service manager): the key must be set

use std::path::PathBuf;

use zingod_sync::IndexKind;

/// `<config dir>/zingod/zingod.toml`
pub(crate) fn default_config() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("zingod/zingod.toml"))
}

/// `<cache dir>/zingod/indexes/<kind.name()>` (one directory per index: each its own store)
pub(crate) fn default_index(kind: IndexKind) -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("zingod/indexes").join(kind.name()))
}

/// `<data dir>/zingod/state`: zingod's own state (the operator hold marker), never a cache
pub(crate) fn default_state() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("zingod/state"))
}

/// `<cache dir>/zingod/peers`: zebra-network's peer address cache
pub(crate) fn default_peer_cache() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("zingod/peers"))
}
