//! Index reads on their own threads: `read_threads` in flight at most, never tokio's blocking pool
//!
//! - address-history scans apart (`scan_threads`): a flood of them never queues a block read
//! - tokio's pool = sync's index writes + `/metrics`: a wallet read never queues ahead of them
//! - wait for a thread = disk pressure (`read_wait_seconds`)
//! - a read whose request is gone before a thread takes it = skipped (a cancel never leaks a read)
//! - a request awaiting its read is not runnable: the adaptive limit (CPU queueing) never sees a
//!   slow disk

use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Instant;

use tonic::Status;

use crate::metrics;

/// Process-wide (clone = share)
#[derive(Clone, Debug)]
pub(crate) struct ReadPool {
    reads: Arc<rayon::ThreadPool>,
    scans: Arc<rayon::ThreadPool>,
}

impl ReadPool {
    pub(crate) fn new(
        reads: NonZeroUsize,
        scans: NonZeroUsize,
    ) -> Result<Self, rayon::ThreadPoolBuildError> {
        let pool = |threads: NonZeroUsize, name: &'static str| {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads.get())
                .thread_name(move |index| format!("zingod-{name}-{index}"))
                .build()?;
            Ok(Arc::new(pool))
        };
        Ok(Self { reads: pool(reads, "read")?, scans: pool(scans, "scan")? })
    }

    /// `read` on a read thread, oldest first
    pub(crate) async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, Status> {
        run(&self.reads, "read", read).await
    }

    /// An address-history scan on a scan thread, oldest first
    pub(crate) async fn scan<T: Send + 'static>(
        &self,
        scan: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, Status> {
        run(&self.scans, "scan", scan).await
    }
}

/// `read` on one of `pool`'s threads (`label` = its `read_wait_seconds` pool)
///
/// - panic in `read` resumes here (broken invariant: zingod aborts)
/// - pool gone (shutdown) → `UNAVAILABLE`
async fn run<T: Send + 'static>(
    pool: &rayon::ThreadPool,
    label: &'static str,
    read: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Status> {
    let (done, outcome) = tokio::sync::oneshot::channel();
    let queued = Instant::now();
    pool.spawn_fifo(move || {
        if done.is_closed() {
            return;
        }
        metrics::read_waited(label, queued.elapsed());
        // receiver gone mid-read: nothing left to deliver to
        let _ = done.send(panic::catch_unwind(AssertUnwindSafe(read)));
    });
    match outcome.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(payload)) => panic::resume_unwind(payload),
        Err(_) => Err(Status::unavailable("shutting down")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::time::Duration;

    use super::*;

    /// One thread held by a slow read: the next waits its turn, a dropped one never runs
    #[tokio::test]
    async fn reads_queue_for_a_thread_and_a_dropped_one_never_runs() {
        let pool = ReadPool::new(NonZeroUsize::MIN, NonZeroUsize::MIN).expect("one of each");
        let ran = Arc::new(AtomicUsize::new(0));
        let (release, hold) = std::sync::mpsc::channel::<()>();

        let slow = tokio::spawn({
            let pool = pool.clone();
            async move { pool.read(move || hold.recv().is_ok()).await }
        });
        let dropped = {
            let (pool, ran) = (pool.clone(), Arc::clone(&ran));
            tokio::spawn(async move { pool.read(move || ran.fetch_add(1, SeqCst)).await })
        };
        let queued = tokio::spawn({
            let (pool, ran) = (pool.clone(), Arc::clone(&ran));
            async move { pool.read(move || ran.fetch_add(1, SeqCst)).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        dropped.abort();
        // its future gone before the thread frees (else the abort races the released read)
        let aborted = dropped.await.expect_err("aborted while queued");
        assert!(aborted.is_cancelled(), "dropped read's task cancelled: {aborted:?}");
        assert!(!queued.is_finished(), "one thread, held: the next read waits");
        release.send(()).expect("slow read listening");

        let slow = slow.await.expect("slow task").expect("slow read");
        let queued = queued.await.expect("queued task").expect("queued read");
        assert!(slow, "held read answered once released");
        assert_eq!((queued, ran.load(SeqCst)), (0, 1), "only the read still wanted ran");
    }

    /// Scan threads apart: with the one scan thread held, a read completes at once and the next
    /// scan waits its turn
    #[tokio::test]
    async fn a_held_scan_thread_never_delays_a_read_and_scans_queue_behind_it() {
        let pool = ReadPool::new(NonZeroUsize::MIN, NonZeroUsize::MIN).expect("one of each");
        let (release, hold) = std::sync::mpsc::channel::<()>();
        let scan = |pool: &ReadPool, scan: Box<dyn FnOnce() -> bool + Send>| {
            let pool = pool.clone();
            tokio::spawn(async move { pool.scan(scan).await })
        };

        let held = scan(&pool, Box::new(move || hold.recv().is_ok()));
        let next = scan(&pool, Box::new(|| true));
        tokio::time::sleep(Duration::from_millis(50)).await;
        let read = tokio::time::timeout(Duration::from_secs(1), pool.read(|| 7)).await;
        let next_waited = !next.is_finished();
        release.send(()).expect("held scan listening");

        assert_eq!(read.ok().map(|read| read.ok()), Some(Some(7)), "read beside a held scan");
        assert!(next_waited, "one scan thread, held: the next scan waits");
        let scans = (held.await.expect("held task"), next.await.expect("next task"));
        assert_eq!((scans.0.ok(), scans.1.ok()), (Some(true), Some(true)), "both scans answered");
    }

    #[tokio::test]
    #[should_panic(expected = "index invariant")]
    async fn a_panicking_read_resumes_in_its_caller() {
        let pool = ReadPool::new(NonZeroUsize::MIN, NonZeroUsize::MIN).expect("one of each");
        let _ = pool.read(|| panic!("index invariant")).await;
    }
}
