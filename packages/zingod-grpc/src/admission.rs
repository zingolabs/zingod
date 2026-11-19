//! Work-stream admission: a [`Slot`] under the adaptive limit, or `UNAVAILABLE` + pushback
//!
//! - slot held for the stream's life (bounds work in progress), except one whose client has not
//!   read for [`GRACE`](crate::stall::GRACE) while slots run short: reclaimed (`stall.rs`)
//! - reclaimed, then read on: takes a slot back even past the limit (never refused mid-stream;
//!   newcomers refused until it drains, and the limit sheds if the overshoot queues the CPU)
//! - `GetMempoolStream` takes none (idle until the next block: bounded by the connection caps)
//! - the limit follows the gRPC tasks' CPU queueing (`tasks`, `adaptive.rs`)
//! - one client (IPv4 address, IPv6 /64) holds at most its [`WorkShare`] of it (default 1/16):
//!   filling it takes a crowd; a client refused over its share = its own doing, no overload

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::{HeaderName, HeaderValue, Response};
use tokio_metrics::TaskMonitor;
use tokio_util::sync::CancellationToken;
use tonic::Status;

use crate::adaptive::Limiter;
use crate::limits::WorkShare;
use crate::metrics;
use crate::overload::{Overload, Wallet};
use crate::stall::Client;

/// Refusal retry hint, ms (held streams = wallet syncs: a slot frees well under a second)
const PUSHBACK_MS: &str = "250";

/// gRPC headers a refusal carries (status by tonic, pushback = grpc-core's "come back later")
pub(crate) const STATUS_HEADER: HeaderName = HeaderName::from_static("grpc-status");
pub(crate) const MESSAGE_HEADER: HeaderName = HeaderName::from_static("grpc-message");
pub(crate) const DETAILS_HEADER: HeaderName = HeaderName::from_static("grpc-status-details-bin");
pub(crate) const PUSHBACK_HEADER: HeaderName = HeaderName::from_static("grpc-retry-pushback-ms");

/// Work streams holding a slot, the adaptive limit and its outcomes, shared by every connection
/// ([`GrpcService::admission`](crate::GrpcService::admission) = its readiness view)
///
/// - `refused` = a newcomer refused since the last reclaim sweep looked
/// - `tasks` = every connection's and stream's task, timed woken → polled (the limiter's input)
/// - `clients` = live work streams per client, reclaimed or not (entry gone at 0)
#[derive(Debug)]
pub struct AdmissionState {
    held: AtomicUsize,
    clients: Mutex<HashMap<Client, usize>>,
    share: WorkShare,
    limiter: Limiter,
    overload: Overload,
    refused: AtomicBool,
    tasks: TaskMonitor,
}

impl AdmissionState {
    /// `ceiling` = [`WORK_CEILING`](crate::adaptive::WORK_CEILING) (tests: smaller, to refuse)
    pub(crate) fn new(ceiling: usize, share: WorkShare) -> Self {
        Self {
            held: AtomicUsize::new(0),
            clients: Mutex::new(HashMap::new()),
            share,
            limiter: Limiter::new(ceiling),
            overload: Overload::default(),
            refused: AtomicBool::new(false),
            tasks: TaskMonitor::new(),
        }
    }

    /// Where gRPC's tasks are spawned instrumented (`transport.rs`: connections, hyper's streams)
    pub(crate) fn tasks(&self) -> &TaskMonitor {
        &self.tasks
    }

    /// Until `cancel`: the limit follows the tasks' CPU queueing (`adaptive.rs`)
    pub(crate) async fn adapt(&self, cancel: &CancellationToken) {
        self.limiter.follow(&self.tasks, cancel).await;
    }

    /// Worth a reclaim sweep: a newcomer refused since the last, or within 1/16 of the limit
    pub(crate) fn pressed(&self) -> bool {
        let limit = self.limiter.limit();
        let held = self.held.load(Ordering::Relaxed);
        self.refused.swap(false, Ordering::Relaxed) || held.saturating_add(limit / 16) >= limit
    }

    fn release(&self) {
        self.held.fetch_sub(1, Ordering::Relaxed);
        metrics::stream_released();
    }

    /// A stream of `client`'s ended (its share counts live streams, reclaimed or not)
    fn ended(&self, client: Client) {
        let mut clients = self.clients.lock().expect("client shares mutex poisoned");
        if let Some(live) = clients.get_mut(&client) {
            *live -= 1;
            if *live == 0 {
                clients.remove(&client);
            }
        }
    }

    /// Hurting wallets persistently, as of the last overload window (rules: `overload.rs`)
    pub fn overloaded(&self) -> bool {
        self.overload.latched()
    }

    #[cfg(test)]
    pub(crate) fn work_limit(&self) -> usize {
        self.limiter.limit()
    }

    /// One overload tick (`elapsed` long, `connections` open now)
    pub(crate) fn tick(&self, elapsed: Duration, connections: usize) {
        self.overload.tick(elapsed, connections);
    }

    /// A connection accepted (the overload share's denominator)
    pub(crate) fn connected(&self) {
        self.overload.connected();
    }

    /// Mean connections a congested uplink holds back now, 0 = clear (uplink sampler)
    pub(crate) fn network_waiting(&self, connections: f64) {
        self.overload.network_waiting(connections);
    }

    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.held.load(Ordering::Relaxed)
    }

    /// A slot, or refused at once: `client` at its share (live streams), or the limit full
    pub(crate) fn admit(
        self: &Arc<Self>,
        wallet: &Wallet,
        client: Client,
    ) -> Result<Slot, Refusal> {
        let limit = self.limiter.limit();
        let mut clients = self.clients.lock().expect("client shares mutex poisoned");
        if clients.get(&client).is_some_and(|&live| live >= self.share.of(limit)) {
            metrics::stream_rejected(Refusal::ClientShare);
            return Err(Refusal::ClientShare);
        }
        let taken = |held: usize| (held < limit).then_some(held + 1);
        if self.held.fetch_update(Ordering::Relaxed, Ordering::Relaxed, taken).is_err() {
            self.refused.store(true, Ordering::Relaxed);
            self.overload.refused(wallet);
            metrics::stream_rejected(Refusal::Limit);
            return Err(Refusal::Limit);
        }
        *clients.entry(client).or_insert(0) += 1;
        metrics::stream_admitted();
        let claim = Claim { held: AtomicBool::new(true), admission: Arc::clone(self) };
        Ok(Slot { claim: Arc::new(claim), client })
    }
}

/// Why a work stream was refused
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Limit,
    ClientShare,
}

impl Refusal {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Limit => "limit",
            Self::ClientShare => "client_share",
        }
    }

    /// `UNAVAILABLE` + pushback hint, before any handler runs
    pub(crate) fn response(self) -> Response<tonic::body::Body> {
        let message = match self {
            Self::Limit => "work stream limit reached",
            Self::ClientShare => "per-client work stream share reached",
        };
        let mut response = Status::unavailable(message).into_http();
        response.headers_mut().insert(PUSHBACK_HEADER, HeaderValue::from_static(PUSHBACK_MS));
        response
    }
}

/// A stream's slot, shared with the reclaim sweep (`held` = false: reclaimed)
///
/// - each release is a `held` swap: the sweep and the stream's end never both release
#[derive(Debug)]
pub(crate) struct Claim {
    held: AtomicBool,
    admission: Arc<AdmissionState>,
}

impl Claim {
    /// Its slot back to the pool (`false` = already)
    pub(crate) fn reclaim(&self) -> bool {
        let held = self.held.swap(false, Ordering::AcqRel);
        if held {
            self.admission.release();
        }
        held
    }
}

/// One admitted stream's hold on work, for its whole life (its client's share: until it ends,
/// reclaimed or not, so reclaim-then-reopen never grows a share)
#[derive(Debug)]
pub(crate) struct Slot {
    claim: Arc<Claim>,
    client: Client,
}

impl Slot {
    /// What the reclaim sweep may take back while its client is not reading
    pub(crate) fn claim(&self) -> Arc<Claim> {
        Arc::clone(&self.claim)
    }

    /// Polled again: a reclaimed slot taken back, past the limit if need be
    pub(crate) fn resume(&self) {
        if !self.claim.held.swap(true, Ordering::AcqRel) {
            self.claim.admission.held.fetch_add(1, Ordering::Relaxed);
            metrics::stream_admitted();
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.claim.reclaim();
        self.claim.admission.ended(self.client);
    }
}
