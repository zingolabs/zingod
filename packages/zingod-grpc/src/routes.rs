//! One module per route group, its handlers (registry + dispatch: [`crate::service`])

mod args;
pub(super) mod blocks;
pub(super) mod deprecated_nullifiers;
pub(super) mod info;
pub(super) mod mempool;
pub(super) mod transactions;
pub(super) mod transparent_address;
pub(super) mod tree_state;

use tonic::Status;

/// `read` on the blocking pool (faults mmapped pages: never on a runtime worker)
///
/// - panic in `read` resumes here (broken invariant: zingod aborts)
/// - runtime shutting down → `UNAVAILABLE`
async fn blocking<T: Send + 'static>(
    read: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Status> {
    tokio::task::spawn_blocking(read).await.map_err(|join| match join.try_into_panic() {
        Ok(panic) => std::panic::resume_unwind(panic),
        Err(_) => Status::unavailable("shutting down"),
    })
}
