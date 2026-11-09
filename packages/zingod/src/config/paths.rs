//! Default on-disk locations (`dirs` = XDG base dirs on Linux, platform dirs elsewhere)
//!
//! - `None` = no home dir (e.g. unset `HOME` under a service manager): the key must be set

use std::path::{Path, PathBuf};

use zingod_sync::IndexKind;

/// `<config dir>/zingod/zingod.toml`
pub(crate) fn default_config() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("zingod/zingod.toml"))
}

/// `<local data dir>/zingod/indexes/<kind.name()>` (one directory per index: each its own store)
///
/// - data, not cache (a resync = hours); local = never a roaming profile
pub(crate) fn default_index(kind: IndexKind) -> Option<PathBuf> {
    Some(dirs::data_local_dir()?.join("zingod/indexes").join(kind.name()))
}

/// `Some(old root)` = `path` = its default, absent, while 1.0.0's default (`<cache dir>`) holds it
pub(crate) fn unmoved_index(kind: IndexKind, path: &Path) -> Option<PathBuf> {
    let old_root = dirs::cache_dir()?.join("zingod/indexes");
    let unmoved = default_index(kind).is_some_and(|default| default == path)
        && !path.exists()
        && old_root.join(kind.name()).exists();
    unmoved.then_some(old_root)
}

/// `<data dir>/zingod/state`: zingod's own state (the operator hold marker), never a cache
pub(crate) fn default_state() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("zingod/state"))
}

/// `<cache dir>/zingod/peers`: zebra-network's peer address cache
pub(crate) fn default_peer_cache() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("zingod/peers"))
}
