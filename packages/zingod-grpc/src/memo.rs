//! Wire answers computed once per published view
//!
//! - every synced wallet asks the same tip questions right after each block (tree states, subtree
//!   roots): one computation per publication, then refcount clones
//! - keyed on the view's `Arc` identity: a new publication = a fresh memo, never an invalidation
//!   rule to get wrong (a view is immutable)
//! - one publication held: a request pinned on an older one replaces it (brief, at publication)

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use tokio::sync::OnceCell;
use tonic::Status;

pub(crate) struct PerView<V, K, T> {
    last: Mutex<Option<Memo<V, K, T>>>,
}

struct Memo<V, K, T> {
    view: Arc<V>,
    answers: HashMap<K, Arc<OnceCell<T>>>,
}

impl<V, K, T> Default for PerView<V, K, T> {
    fn default() -> Self {
        Self { last: Mutex::new(None) }
    }
}

impl<V, K: Eq + Hash, T: Clone> PerView<V, K, T> {
    /// Already computed for `view`: the inline path (no hop, no read thread)
    pub(crate) fn cached(&self, view: &Arc<V>, key: &K) -> Option<T> {
        let last = self.last.lock().expect("per-view memo poisoned");
        let memo = last.as_ref().filter(|memo| Arc::ptr_eq(&memo.view, view))?;
        memo.answers.get(key)?.get().cloned()
    }

    /// `compute` at most once per `(view, key)`
    ///
    /// - concurrent askers of one key await the first (the herd after a block = one computation,
    ///   one read thread; the rest hold no thread)
    /// - `Err` (shutdown) not kept: the next asker computes
    pub(crate) async fn get_or_compute(
        &self,
        view: &Arc<V>,
        key: K,
        compute: impl Future<Output = Result<T, Status>>,
    ) -> Result<T, Status> {
        let cell = {
            let mut last = self.last.lock().expect("per-view memo poisoned");
            let memo = match last.as_mut().filter(|memo| Arc::ptr_eq(&memo.view, view)) {
                Some(memo) => memo,
                None => last.insert(Memo { view: Arc::clone(view), answers: HashMap::new() }),
            };
            Arc::clone(memo.answers.entry(key).or_default())
        };
        cell.get_or_try_init(|| compute).await.cloned()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    use super::*;

    /// Eight askers while the first computes: one computation, every asker answered, then inline
    #[tokio::test]
    async fn concurrent_askers_of_one_key_share_one_computation_and_a_failure_is_not_kept() {
        let memo: PerView<u32, &str, usize> = PerView::default();
        let view = Arc::new(1u32);
        let computed = AtomicUsize::new(0);
        let (release, held) = tokio::sync::watch::channel(false);
        let compute = || async {
            computed.fetch_add(1, SeqCst);
            // held open until every asker is waiting
            held.clone().wait_for(|&go| go).await.expect("release sender alive");
            Ok(10)
        };
        let failed = memo.get_or_compute(&view, "tip", async { Err(Status::unavailable("x")) });
        assert_eq!(failed.await.map_err(|status| status.code()), Err(tonic::Code::Unavailable));
        assert_eq!(memo.cached(&view, &"tip"), None, "a failure is not an answer");

        let askers =
            futures::future::join_all((0..8).map(|_| memo.get_or_compute(&view, "tip", compute())));
        let herd = futures::future::join(askers, async { release.send(true) }).await.0;

        let herd: Vec<usize> = herd.into_iter().map(|answer| answer.expect("answered")).collect();
        assert_eq!(herd, [10; 8], "every asker answered");
        assert_eq!(computed.load(SeqCst), 1, "eight concurrent askers, one computation");
        assert_eq!(memo.cached(&view, &"tip"), Some(10), "then served inline");
        assert_eq!(memo.cached(&view, &"roots"), None, "another key = its own computation");
    }

    #[tokio::test]
    async fn a_new_view_starts_empty_and_drops_the_older_one() {
        let memo: PerView<u32, &str, usize> = PerView::default();
        let (first, second) = (Arc::new(1u32), Arc::new(2u32));
        memo.get_or_compute(&first, "tip", async { Ok(10) }).await.expect("computed");

        let fresh = memo.cached(&second, &"tip");
        let computed = memo.get_or_compute(&second, "tip", async { Ok(20) }).await;

        assert_eq!(fresh, None, "a new publication knows nothing");
        assert_eq!(computed.expect("computed"), 20, "computed for the new publication");
        assert_eq!(memo.cached(&first, &"tip"), None, "the older publication dropped");
    }
}
