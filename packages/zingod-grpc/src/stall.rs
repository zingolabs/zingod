//! Streams whose client stopped reading: per connection past `unread_timeout`, node-wide past the
//! [`Parked`] memory budget
//!
//! - flow control stalls hyper on `poll_capacity`: it stops polling the body, so a stream cannot
//!   notice from inside, only its connection can close
//! - owing = a body handed hyper a data frame and has not been polled since (a `Pending` body =
//!   idle or waiting on a read, never owing)
//! - owing past [`GRACE`] with the work pool nearly full: its work slot reclaimed
//!   ([`reclaim_idle`]; a client reading on keeps its slot, as one never reading cannot hold it)
//! - the connection's watchdog closes it once any stream has owed for `unread_timeout`, or once
//!   [`Parked`] picks it to bring the node's owed bytes back under budget

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::admission::{AdmissionState, Claim};
use crate::{metrics, summary};

/// Owing this long = its client is not reading (a syncing wallet takes the next message well
/// within it): the work slot is reclaimable
pub(crate) const GRACE: Duration = Duration::from_secs(1);

/// Reclaim sweep cadence while the pool is pressed (a refused newcomer waits at most this)
const SWEEP: Duration = Duration::from_millis(250);

/// Per owing stream beyond its messages: h2 stream state + the route's cursor
const OVERHEAD: u64 = 4 * 1024;

/// One owing stream holds ≈ 2 messages: one in h2's send buffer, one hyper waits to send
const MESSAGES_HELD: u64 = 2;

/// Over budget, an eviction pass at most this often (each scans every connection; the evicted
/// take a moment to close, so passes per frame would rescan a flood at its peak)
const EVICT_EVERY: Duration = Duration::from_millis(100);

/// Bytes the node holds for owing streams, by client, against one budget (the host's: zingod
/// derives it from memory)
///
/// - over budget → the heaviest client's heaviest connection closes, until the rest fit (one
///   hoarding address evicts itself; a wallet with one waiting stream goes last)
/// - `clients` = each client's live connections (IPv4 address, IPv6 /64: one subscriber's prefix)
#[derive(Debug)]
pub(crate) struct Parked {
    budget: u64,
    total: AtomicU64,
    clients: Mutex<HashMap<Client, Vec<Weak<Watch>>>>,
    evicted_at: Mutex<Option<Instant>>,
}

impl Default for Parked {
    fn default() -> Self {
        Self::new(u64::MAX)
    }
}

impl Parked {
    pub(crate) fn new(budget: u64) -> Self {
        let (total, clients) = (AtomicU64::new(0), Mutex::new(HashMap::new()));
        Self { budget, total, clients, evicted_at: Mutex::new(None) }
    }

    /// A connection's watch, counted under its client until dropped
    pub(crate) fn watch(self: &Arc<Self>, client: IpAddr) -> Arc<Watch> {
        let client = Client::of(client);
        let watch = Arc::new(Watch::under(Arc::clone(self), client));
        let mut clients = self.clients.lock().expect("parked clients mutex poisoned");
        clients.entry(client).or_default().push(Arc::downgrade(&watch));
        watch
    }

    fn charge(&self, bytes: u64) {
        let total = self.total.fetch_add(bytes, Ordering::Relaxed).saturating_add(bytes);
        metrics::parked_bytes(total);
        if total > self.budget && self.eviction_due() {
            self.evict();
        }
    }

    fn eviction_due(&self) -> bool {
        let mut evicted_at = self.evicted_at.lock().expect("eviction time mutex poisoned");
        if evicted_at.is_some_and(|at| at.elapsed() < EVICT_EVERY) {
            return false;
        }
        *evicted_at = Some(Instant::now());
        true
    }

    fn release(&self, bytes: u64) {
        let total = self.total.fetch_sub(bytes, Ordering::Relaxed).saturating_sub(bytes);
        metrics::parked_bytes(total);
    }

    /// Marks connections to close, the heaviest client's heaviest first, until the bytes not
    /// already leaving fit the budget
    fn evict(&self) {
        let clients = self.clients.lock().expect("parked clients mutex poisoned");
        let live: Vec<Vec<Arc<Watch>>> = clients
            .values()
            .map(|watches| watches.iter().filter_map(Weak::upgrade).collect())
            .collect();
        drop(clients);
        let staying = |watch: &&Arc<Watch>| !watch.evicted.load(Ordering::Relaxed);
        let owed = |watch: &Arc<Watch>| watch.owed.load(Ordering::Relaxed);
        let leaving: u64 = live.iter().flatten().filter(|watch| !staying(watch)).map(owed).sum();
        let mut held = self.total.load(Ordering::Relaxed).saturating_sub(leaving);
        while held > self.budget {
            let heaviest = live
                .iter()
                .max_by_key(|watches| watches.iter().filter(staying).map(owed).sum::<u64>());
            let Some(watch) = heaviest
                .and_then(|watches| watches.iter().filter(staying).max_by_key(|watch| owed(watch)))
            else {
                return;
            };
            if owed(watch) == 0 {
                return;
            }
            held = held.saturating_sub(owed(watch));
            watch.evicted.store(true, Ordering::Relaxed);
            watch.evict.notify_one();
            metrics::parked_evicted();
            summary::evicted();
        }
    }

    /// Work slots of streams owing for `grace` or longer, back to the pool (count reclaimed)
    pub(crate) fn reclaim(&self, grace: Duration) -> usize {
        let watches: Vec<Arc<Watch>> = {
            let clients = self.clients.lock().expect("parked clients mutex poisoned");
            clients.values().flatten().filter_map(Weak::upgrade).collect()
        };
        let mut reclaimed = 0;
        for watch in watches {
            let owing = watch.owing.lock().expect("stall watch poisoned");
            let idle = owing.values().filter(|owing| owing.since.elapsed() >= grace);
            reclaimed +=
                idle.filter_map(|owing| owing.claim.as_ref()).filter(|c| c.reclaim()).count();
        }
        reclaimed
    }
}

/// Until `cancel`: every [`SWEEP`] while the work pool is pressed, idle streams' slots reclaimed
pub(crate) async fn reclaim_idle(
    parked: Arc<Parked>,
    admission: Arc<AdmissionState>,
    cancel: CancellationToken,
) {
    let mut sweeps = tokio::time::interval(SWEEP);
    sweeps.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = sweeps.tick() => {}
        }
        if admission.pressed() {
            parked.reclaim(GRACE);
        }
    }
}

/// Unit of fairness (eviction, work share): an IPv4 address, or an IPv6 /64 (one subscriber's
/// prefix)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Client(IpAddr);

impl Client {
    fn of(ip: IpAddr) -> Self {
        match ip.to_canonical() {
            IpAddr::V6(v6) => {
                let [a, b, c, d, ..] = v6.segments();
                Self(IpAddr::V6(Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0)))
            }
            v4 => Self(v4),
        }
    }
}

/// One connection's owing streams, by ticket id (`owed` = their charged bytes)
#[derive(Debug)]
pub(crate) struct Watch {
    owing: Mutex<HashMap<u64, Owing>>,
    issued: AtomicU64,
    owed: AtomicU64,
    evicted: AtomicBool,
    evict: Notify,
    parked: Arc<Parked>,
    client: Client,
}

/// One owing stream: since when, and its work slot (`None` = a subscription, holding none)
#[derive(Debug)]
struct Owing {
    since: Instant,
    claim: Option<Arc<Claim>>,
}

/// Unbudgeted, unregistered (tests)
impl Default for Watch {
    fn default() -> Self {
        Self::under(Arc::default(), Client(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
    }
}

impl Watch {
    fn under(parked: Arc<Parked>, client: Client) -> Self {
        Self {
            owing: Mutex::new(HashMap::new()),
            issued: AtomicU64::new(0),
            owed: AtomicU64::new(0),
            evicted: AtomicBool::new(false),
            evict: Notify::new(),
            parked,
            client,
        }
    }

    pub(crate) fn client(&self) -> Client {
        self.client
    }

    pub(crate) fn ticket(self: &Arc<Self>) -> Ticket {
        let id = self.issued.fetch_add(1, Ordering::Relaxed);
        Ticket { watch: Arc::clone(self), id, charged: None }
    }

    pub(crate) fn oldest(&self) -> Option<Instant> {
        self.owing.lock().expect("stall watch poisoned").values().map(|owing| owing.since).min()
    }

    /// Resolves once some stream has owed for `timeout` (sleeps to the oldest deadline)
    ///
    /// - nothing owed → recheck every `timeout` (detection within 2 × `timeout` at worst)
    pub(crate) async fn stalled(&self, timeout: Duration) {
        loop {
            let deadline = match self.oldest() {
                Some(since) if since.elapsed() >= timeout => return,
                Some(since) => since + timeout,
                None => Instant::now() + timeout,
            };
            tokio::time::sleep_until(deadline).await;
        }
    }

    /// Resolves once [`Parked`] picks this connection to close
    pub(crate) async fn evicted(&self) {
        // notify_one keeps its permit for a waiter that arrives later
        if !self.evicted.load(Ordering::Relaxed) {
            self.evict.notified().await;
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut clients = self.parked.clients.lock().expect("parked clients mutex poisoned");
        if let Some(watches) = clients.get_mut(&self.client) {
            watches.retain(|watch| watch.strong_count() > 0);
            if watches.is_empty() {
                clients.remove(&self.client);
            }
        }
    }
}

/// One stream's standing with its connection's [`Watch`] (`charged` = its bytes while owing)
#[derive(Debug)]
pub(crate) struct Ticket {
    watch: Arc<Watch>,
    id: u64,
    charged: Option<u64>,
}

impl Ticket {
    /// Handed hyper a `len`-byte data frame: owing until the next poll (`claim` = its work slot)
    pub(crate) fn handed(&mut self, len: usize, claim: Option<Arc<Claim>>) {
        if self.charged.is_none() {
            let len = u64::try_from(len).unwrap_or(u64::MAX);
            let bytes = OVERHEAD.saturating_add(len.saturating_mul(MESSAGES_HELD));
            let owing = Owing { since: Instant::now(), claim };
            self.watch.owing.lock().expect("stall watch poisoned").insert(self.id, owing);
            self.watch.owed.fetch_add(bytes, Ordering::Relaxed);
            self.charged = Some(bytes);
            self.watch.parked.charge(bytes);
        }
    }

    /// Polled again: hyper had room for more
    pub(crate) fn pulled(&mut self) {
        if let Some(bytes) = self.charged.take() {
            self.watch.owing.lock().expect("stall watch poisoned").remove(&self.id);
            self.watch.owed.fetch_sub(bytes, Ordering::Relaxed);
            self.watch.parked.release(bytes);
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.pulled();
    }
}

/// A request body arriving before admission: its bytes charged as the connection's until dropped
/// (a client trickling big bodies = the heaviest, evicted as for unread responses)
#[derive(Debug)]
pub(crate) struct Arriving {
    watch: Arc<Watch>,
    bytes: u64,
}

impl Arriving {
    pub(crate) fn of(watch: &Arc<Watch>) -> Self {
        Self { watch: Arc::clone(watch), bytes: 0 }
    }

    pub(crate) fn add(&mut self, len: usize) {
        let len = u64::try_from(len).unwrap_or(u64::MAX);
        self.bytes = self.bytes.saturating_add(len);
        self.watch.owed.fetch_add(len, Ordering::Relaxed);
        self.watch.parked.charge(len);
    }
}

impl Drop for Arriving {
    fn drop(&mut self) {
        self.watch.owed.fetch_sub(self.bytes, Ordering::Relaxed);
        self.watch.parked.release(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(300);

    /// Paused clock: the stuck stream, not the one still reading
    #[tokio::test(start_paused = true)]
    async fn a_frame_left_unpulled_for_the_timeout_trips_the_watch() {
        let watch = Arc::new(Watch::default());
        let (mut reading, mut stuck) = (watch.ticket(), watch.ticket());
        let start = Instant::now();
        reading.handed(5, None);
        stuck.handed(5, None);
        let stalled = tokio::spawn({
            let watch = Arc::clone(&watch);
            async move { watch.stalled(TIMEOUT).await }
        });

        tokio::time::sleep(TIMEOUT / 2).await;
        reading.pulled();
        reading.handed(5, None);
        stalled.await.expect("stalled");

        assert_eq!(start.elapsed(), TIMEOUT, "the stuck stream's frame, unpulled for TIMEOUT");
        drop(stuck);
        reading.pulled();
        assert_eq!(watch.oldest(), None, "a dropped or pulled stream owes nothing");
    }

    /// Paused clock: tripped only after a frame handed at 3 × TIMEOUT (rechecks every TIMEOUT)
    #[tokio::test(start_paused = true)]
    async fn a_watch_with_nothing_owed_never_trips() {
        let watch = Arc::new(Watch::default());
        let start = Instant::now();
        let stalled = tokio::spawn({
            let watch = Arc::clone(&watch);
            async move { watch.stalled(TIMEOUT).await }
        });

        tokio::time::sleep(TIMEOUT * 3).await;
        let mut late = watch.ticket();
        late.handed(5, None);
        stalled.await.expect("stalled");

        let tripped_at = start.elapsed();
        let window = TIMEOUT * 4..=TIMEOUT * 5;
        assert!(window.contains(&tripped_at), "quiet 3 × TIMEOUT, tripped at {tripped_at:?}");
    }

    /// Over budget: the heaviest client's heaviest connection closes, once, and only as many as
    /// the budget needs; a wallet with one waiting stream stays; an IPv6 /64 = one client; one
    /// eviction pass per [`EVICT_EVERY`] however many charges land meanwhile
    #[tokio::test(start_paused = true)]
    async fn over_budget_the_heaviest_clients_heaviest_connection_closes() {
        const KIB: usize = 1024;
        // each owing stream: 4 KiB + 2 × its frame (16 KiB frame = 36 KiB, 40 KiB frame = 84 KiB)
        let parked = Arc::new(Parked::new(400 * 1024));
        let hoarder = |n: u16| IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, n));
        let (big, small) = (parked.watch(hoarder(1)), parked.watch(hoarder(2)));
        let wallet = parked.watch(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)));

        // wallet 36 KiB; one /64 over two addresses: big 3 × 84, small 1 × 84 → 372 KiB, fits
        let mut tickets = vec![wallet.ticket()];
        tickets[0].handed(16 * KIB, None);
        for watch in [&big, &big, &big, &small] {
            let mut ticket = watch.ticket();
            ticket.handed(40 * KIB, None);
            tickets.push(ticket);
        }
        let evicted = |watch: &Watch| watch.evicted.load(Ordering::Relaxed);
        assert_eq!(parked.total.load(Ordering::Relaxed), 372 * 1024);
        assert!(![&wallet, &big, &small].iter().any(|watch| evicted(watch)), "under budget");

        // small waits once more (456 KiB): the /64 (420) outweighs the wallet; big (252) > small
        let mut over = small.ticket();
        over.handed(40 * KIB, None);
        let flags: Vec<bool> = [&wallet, &big, &small].iter().map(|watch| evicted(watch)).collect();
        assert_eq!(
            flags,
            [false, true, false],
            "the /64 = heaviest client; its heaviest connection"
        );
        tokio::time::timeout(Duration::from_secs(1), big.evicted()).await.expect("notified");

        // big's bytes count as leaving: another small charge evicts nothing more
        let mut more = wallet.ticket();
        more.handed(KIB, None);
        assert!(!evicted(&small) && !evicted(&wallet), "big leaving brings the rest under budget");

        // wallet floods 204 KiB: 409 KiB not leaving, over again, but within EVICT_EVERY of the
        // last pass: no scan; the first charge after it evicts the wallet (246 KiB > /64's 172)
        let mut flood = wallet.ticket();
        flood.handed(100 * KIB, None);
        assert!(!evicted(&wallet), "a pass ran under EVICT_EVERY ago: this charge skips it");
        tokio::time::advance(EVICT_EVERY).await;
        let mut next = small.ticket();
        next.handed(0, None);
        assert!(evicted(&wallet) && !evicted(&small), "next pass: the now-heaviest client");

        // closing returns every byte; the client table forgets dropped connections
        drop((tickets, over, more, flood, next, big, small, wallet));
        assert_eq!(parked.total.load(Ordering::Relaxed), 0, "every charge released");
        assert!(parked.clients.lock().expect("clients").is_empty(), "no dropped watch kept");
    }
}
