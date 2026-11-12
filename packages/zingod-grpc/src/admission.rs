//! Work-stream admission: a [`Slot`] under the adaptive limit, or `UNAVAILABLE` + pushback, never a
//! queue
//!
//! - Slot owned by the response *body* (`guard.rs`), not the handler (released at handler return
//!   = unbounded streams)
//! - `GetMempoolStream` takes none (idle until the next block: bounded by the connection caps)
//! - each slot feeds its messages' production time to the limiter (`adaptive.rs`)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderValue, Response};
use tokio::time::Instant;
use tonic::Status;

use crate::adaptive::Limiter;
use crate::metrics;
use crate::overload::Overload;

/// Refusal retry hint, ms (held streams = wallet syncs: a slot frees well under a second)
const PUSHBACK_MS: &str = "250";

/// gRPC's own "come back later" header (grpc-core)
const PUSHBACK_HEADER: &str = "grpc-retry-pushback-ms";

/// Work streams held, the adaptive limit and its outcomes, shared by every connection
/// ([`GrpcService::admission`](crate::GrpcService::admission) = its readiness view)
#[derive(Debug)]
pub struct AdmissionState {
    held: AtomicUsize,
    limiter: Limiter,
    overload: Overload,
}

impl AdmissionState {
    pub(crate) fn new(limits: &crate::GrpcLimits) -> Self {
        Self {
            held: AtomicUsize::new(0),
            limiter: Limiter::new(limits.max_work_streams.get()),
            overload: Overload::default(),
        }
    }

    /// Refusing work persistently, as of the last serving summary (rules: `overload.rs`)
    pub fn overloaded(&self) -> bool {
        self.overload.latched()
    }

    #[cfg(test)]
    pub(crate) fn work_limit(&self) -> usize {
        self.limiter.limit()
    }

    /// Closes a serving summary's window (`elapsed` long)
    pub(crate) fn tick(&self, elapsed: Duration) {
        metrics::adaptive_stream_limit(self.limiter.limit());
        self.overload.tick(elapsed);
    }

    pub(crate) fn held(&self) -> usize {
        self.held.load(Ordering::Relaxed)
    }

    /// A slot, or `None` = refused at once at the limit
    pub(crate) fn admit(self: &Arc<Self>) -> Option<Slot> {
        let limit = self.limiter.limit();
        let taken = |held: usize| (held < limit).then_some(held + 1);
        if self.held.fetch_update(Ordering::Relaxed, Ordering::Relaxed, taken).is_err() {
            self.overload.refused();
            metrics::stream_rejected();
            return None;
        }
        self.overload.admitted();
        metrics::stream_admitted();
        Some(Slot { admission: Arc::clone(self), busy_since: Some(Instant::now()) })
    }
}

/// One admitted stream's place, back on drop
///
/// - `busy_since` = producing its next message since (`None` = handed one, not polled since)
#[derive(Debug)]
pub(crate) struct Slot {
    admission: Arc<AdmissionState>,
    busy_since: Option<Instant>,
}

impl Slot {
    pub(crate) fn polled(&mut self) {
        self.busy_since.get_or_insert_with(Instant::now);
    }

    /// A data frame out: its production time to the limiter
    pub(crate) fn handed(&mut self) {
        if let Some(since) = self.busy_since.take() {
            self.admission.limiter.sample(since.elapsed(), self.admission.held());
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.admission.held.fetch_sub(1, Ordering::Relaxed);
        metrics::stream_released();
    }
}

/// `UNAVAILABLE` + pushback hint, before any handler runs
pub(crate) fn refused() -> Response<tonic::body::Body> {
    let mut response = Status::unavailable("work stream limit reached").into_http();
    response.headers_mut().insert(PUSHBACK_HEADER, HeaderValue::from_static(PUSHBACK_MS));
    response
}
