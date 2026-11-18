//! zingod daemon errors

use std::path::PathBuf;

/// Configuring, booting or running the daemon (`TaskEnded` = a task ended cleanly before any
/// shutdown signal: never expected, a fault)
#[derive(Debug, thiserror::Error)]
pub(crate) enum IndexerError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("writing {}: {source}", path.display())]
    ConfigFile { path: PathBuf, source: std::io::Error },
    #[error("reading or raising the open-file limit: {0}")]
    OpenFileLimit(std::io::Error),
    #[error("reading the memory size: {0}")]
    Memory(String),
    #[error(
        "this host holds {connections} wallet connections ({open_files} open files, {memory_mib} \
         MiB memory), under 256: raise the hard open-file limit (systemd LimitNOFILE=, docker \
         --ulimit nofile=) or give zingod more memory"
    )]
    TooSmall { open_files: u64, memory_mib: u64, connections: u64 },
    #[error(transparent)]
    ValidatorEndpoint(#[from] zingod_source::EndpointError),
    #[error(transparent)]
    OpenIndex(#[from] zingod_persistence::StoreError),
    #[error(transparent)]
    Nfs(#[from] zingod_nfs::NfsError),
    #[error(transparent)]
    Follow(#[from] zingod_sync::FollowError),
    #[error(transparent)]
    GrpcSnapshot(#[from] zingod_grpc_snapshot::GrpcSnapshotError),
    #[error(transparent)]
    Grpc(#[from] zingod_grpc::GrpcServeError),
    #[error(transparent)]
    Tls(#[from] zingod_grpc::TlsError),
    #[error("metrics error: {0}")]
    Metrics(String),
    #[error("index snapshot: {0}")]
    Bootstrap(String),
    #[error("{task} task ended before shutdown")]
    TaskEnded { task: &'static str },
}
