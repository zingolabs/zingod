//! What this host holds: gRPC's [`HostCapacity`], derived (no operator sets it)
//!
//! - open files: one per connection, past [`RESERVED`]; at the limit `accept()` and index opens fail
//! - memory (the cgroup's limit, else the host's): [`PER_CONNECTION`] each within
//!   1 / [`MEMORY_SHARE`]; streams waiting on their clients within 1 / [`PARKED_SHARE`]

use std::num::NonZeroUsize;
use std::path::Path;

use procfs::process::Process;
use procfs::{Current as _, Meminfo};
use rlimit::Resource;
use tracing::info;
use zingod_grpc::HostCapacity;

use crate::error::IndexerError;

/// fds besides client sockets: index files (~290 measured on a syncing mainnet node, bounded by
/// tree levels + LSM segment caps), validator RPC pools, metrics, logs
const RESERVED: u64 = 1024;

/// Heap one busy wallet connection costs, ~1.7× (bench: 60-75 KB each at 6-13k connections)
const PER_CONNECTION: u64 = 128 * 1024;

/// 1 / this of memory for connections
const MEMORY_SHARE: u64 = 2;

/// 1 / this of memory for messages held for clients not reading (over it: their connections close,
/// heaviest client first); the last quarter = index caches, read threads, page cache
const PARKED_SHARE: u64 = 4;

/// Fewer = a host too small to serve (refused at boot, not discovered under load)
const MIN_CONNECTIONS: u64 = 256;

/// Raises the soft open-file limit to the hard one (indexes and connections share it)
pub(crate) fn raise_fd_limit() -> Result<(), IndexerError> {
    let limit = rlimit::increase_nofile_limit(u64::MAX).map_err(IndexerError::OpenFileLimit)?;
    info!(limit, "Open-file limit");
    Ok(())
}

/// Connections = the lower of what the open files and the memory hold; parked = a memory share
pub(crate) fn host_capacity() -> Result<HostCapacity, IndexerError> {
    let (open_files, _) =
        rlimit::getrlimit(Resource::NOFILE).map_err(IndexerError::OpenFileLimit)?;
    let host = Meminfo::current().map_err(|error| IndexerError::Memory(error.to_string()))?;
    let memory = cgroup_memory_limit().map_or(host.mem_total, |limit| limit.min(host.mem_total));
    let capacity = derive(open_files, memory)?;
    let (connections, parked_mib) = (capacity.connections, capacity.parked_bytes >> 20);
    info!(connections, parked_mib, open_files, memory_mib = memory >> 20, "Connection cap");
    Ok(capacity)
}

fn derive(open_files: u64, memory: u64) -> Result<HostCapacity, IndexerError> {
    let by_files = open_files.saturating_sub(RESERVED);
    let by_memory = memory / MEMORY_SHARE / PER_CONNECTION;
    let connections = by_files.min(by_memory);
    match connections >= MIN_CONNECTIONS {
        true => Ok(HostCapacity {
            connections: NonZeroUsize::new(usize::try_from(connections).unwrap_or(usize::MAX))
                .expect(">= MIN_CONNECTIONS"),
            parked_bytes: memory / PARKED_SHARE,
        }),
        false => Err(IndexerError::TooSmall { open_files, memory_mib: memory >> 20, connections }),
    }
}

/// Lowest `memory.max` from this process's cgroup v2 up to the root (`None` = unlimited / v1)
fn cgroup_memory_limit() -> Option<u64> {
    let cgroups = Process::myself().ok()?.cgroups().ok()?;
    let own = cgroups.0.into_iter().find(|cgroup| cgroup.hierarchy == 0)?;
    let own = Path::new("/sys/fs/cgroup").join(own.pathname.trim_start_matches('/'));
    own.ancestors()
        .take_while(|dir| dir.starts_with("/sys/fs/cgroup"))
        .filter_map(|dir| std::fs::read_to_string(dir.join("memory.max")).ok())
        .filter_map(|max| max.trim().parse::<u64>().ok())
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connections = the scarcer of open files (past the reserve) and memory (half, 128 KiB each);
    /// parked = a quarter of memory; under 256 connections refused at boot, naming both
    #[test]
    fn the_cap_is_the_scarcer_of_open_files_and_memory_and_a_tiny_host_is_refused() {
        const GIB: u64 = 1 << 30;
        // case, open-file limit, memory, (connections, parked) (None = refused)
        #[rustfmt::skip]
        let cases = [
            ("systemd hard limit, 8 GiB VM",  524_288,   8 * GIB,   Some((32_768, 2 * GIB))),
            ("docker hard limit, 64 GiB box", 1_048_576, 64 * GIB,  Some((262_144, 16 * GIB))),
            ("8 GiB VM, files scarce",        5_120,     8 * GIB,   Some((4_096, 2 * GIB))),
            ("soft limit never raised",       1_024,     8 * GIB,   None),
            ("64 MiB container: 256, enough", 524_288,   GIB / 16,  Some((256, GIB / 64))),
            ("32 MiB container",              524_288,   GIB / 32,  None),
        ];
        for (case, open_files, memory, want) in cases {
            let cap = derive(open_files, memory);

            let got = cap.as_ref().ok().map(|cap| (cap.connections.get(), cap.parked_bytes));
            assert_eq!(got, want, "{case}: {cap:?}");
        }

        let refused = derive(1_024, 8 * GIB).expect_err("no fds past the reserve").to_string();
        for named in ["1024 open files", "8192 MiB", "LimitNOFILE"] {
            assert!(refused.contains(named), "{named}: {refused}");
        }
    }
}
