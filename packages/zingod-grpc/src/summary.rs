//! `Serving` log line every [`INTERVAL`]: rate, connections, refused, failed, evicted (rest = metrics)
//!
//! - WARN once any request was refused or failed or a connection evicted, else INFO; silent idle
//! - first server fault of a window logged as it happens (ERROR `Request failed`), the rest counted
//! - then one `Origin requests` line per top origin (gRPC-web, `Origin`-bearing), the rest pooled

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tonic::Code;
use tracing::{error, info, warn};

use crate::metrics::{code_label, method_label};
use crate::origin::OriginVerdict;
use crate::service::Method;

pub(crate) const INTERVAL: Duration = Duration::from_secs(60);

/// Per window: origins logged by name, hosts tallied by name (`Origin` = client-set), longest host
const TOP_ORIGINS: usize = 10;
const MAX_ORIGINS: usize = 1024;
const MAX_HOST: usize = 253;

static ORIGINS: Mutex<Origins> = Mutex::new(Origins { hosts: BTreeMap::new(), pooled: 0 });

static REQUESTS: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static EVICTED: AtomicU64 = AtomicU64::new(0);
static FAILURE_LOGGED: AtomicBool = AtomicBool::new(false);

/// This server's fault (any other non-`Ok` = the client's or a refusal)
fn is_failure(code: Code) -> bool {
    matches!(code, Code::Unknown | Code::Internal | Code::DataLoss)
}

/// Stream close-out; a window's first failure logged with the message its client got
pub(crate) fn finished(method: Option<Method>, code: Code, message: &str) {
    REQUESTS.fetch_add(1, Ordering::Relaxed);
    if matches!(code, Code::Unavailable | Code::ResourceExhausted) {
        REFUSED.fetch_add(1, Ordering::Relaxed);
    }
    if is_failure(code) {
        FAILED.fetch_add(1, Ordering::Relaxed);
        if !FAILURE_LOGGED.swap(true, Ordering::Relaxed) {
            let (method, code) = (method_label(method), code_label(code));
            error!(method, code, error = message, "Request failed");
        }
    }
}

/// A connection closed for holding the most unread responses past their budget (`stall.rs`)
pub(crate) fn evicted() {
    EVICTED.fetch_add(1, Ordering::Relaxed);
}

/// One `Origin`-bearing request, under its host (verdict = the latest it got)
pub(crate) fn origin(host: &str, verdict: OriginVerdict) {
    ORIGINS.lock().expect("origins mutex poisoned").count(host, verdict);
}

/// One window's origins: host → (requests, latest verdict); `pooled` = requests past the bounds
#[derive(Debug, Default)]
struct Origins {
    hosts: BTreeMap<String, (u64, OriginVerdict)>,
    pooled: u64,
}

impl Origins {
    fn count(&mut self, host: &str, verdict: OriginVerdict) {
        let full = self.hosts.len() >= MAX_ORIGINS || host.len() > MAX_HOST;
        match self.hosts.get_mut(host) {
            Some(seen) => *seen = (seen.0 + 1, verdict),
            None if full => self.pooled += 1,
            None => _ = self.hosts.insert(host.to_owned(), (1, verdict)),
        }
    }
}

/// The window's [`TOP_ORIGINS`] busiest by name, the rest as `(other)`
fn log_origins() {
    let Origins { hosts, mut pooled } = std::mem::take(&mut *ORIGINS.lock().expect("origins"));
    let mut hosts: Vec<_> = hosts.into_iter().collect();
    hosts.sort_by_key(|(_, (requests, _))| std::cmp::Reverse(*requests));
    for (rank, (origin, (requests, verdict))) in hosts.into_iter().enumerate() {
        match rank < TOP_ORIGINS {
            true => info!(origin, requests, verdict = verdict.label(), "Origin requests"),
            false => pooled += requests,
        }
    }
    if pooled > 0 {
        info!(origin = "(other)", requests = pooled, "Origin requests");
    }
}

/// (requests, refused, failed, evicted) since the last call, zeroed
fn take() -> (u64, u64, u64, u64) {
    let take = |counter: &AtomicU64| counter.swap(0, Ordering::Relaxed);
    FAILURE_LOGGED.store(false, Ordering::Relaxed);
    (take(&REQUESTS), take(&REFUSED), take(&FAILED), take(&EVICTED))
}

/// Logs the window since the last call (`elapsed` long, `connections` held now)
pub(crate) fn summarise(elapsed: Duration, connections: usize) {
    let (requests, refused, failed, evicted) = take();
    if requests == 0 && connections == 0 {
        return;
    }
    let per_s =
        Duration::from_secs(requests).div_duration_f64(elapsed.max(Duration::from_nanos(1)));
    let rps = (per_s * 100.0).round() / 100.0;
    match refused + failed + evicted {
        0 => info!(rps, conns = connections, "Serving"),
        _ => warn!(rps, conns = connections, refused, failed, evicted, "Serving"),
    }
    log_origins();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Refusals + server faults counted apart; client errors only as requests; taken = zeroed
    #[test]
    fn a_window_counts_refusals_and_failures_apart_from_client_errors() {
        let codes = [Code::Ok, Code::NotFound, Code::Cancelled, Code::Unavailable, Code::Internal];
        for code in codes {
            finished(None, code, "");
        }

        evicted();

        assert_eq!(take(), (5, 1, 1, 1), "(requests, refused, failed, evicted)");
        assert_eq!(take(), (0, 0, 0, 0), "taken = zeroed");
    }

    /// 1030 sites + an oversized host: 1024 held by name (counting on, latest verdict), the rest
    /// pooled
    #[test]
    fn origins_hold_1024_hosts_by_name_and_pool_the_rest() {
        let mut origins = Origins::default();
        for site in 0..1030 {
            origins.count(&format!("site{site}.example"), OriginVerdict::Default);
        }
        origins.count("site0.example", OriginVerdict::Blocked);
        origins.count(&"a".repeat(254), OriginVerdict::Default);

        assert_eq!(origins.hosts.len(), MAX_ORIGINS, "held by name up to the bound");
        assert_eq!(origins.hosts["site0.example"], (2, OriginVerdict::Blocked), "held counts on");
        assert_eq!(origins.pooled, 6 + 1, "6 sites past the bound + the oversized host");
    }
}
