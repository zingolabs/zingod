//! `Serving` log line every [`INTERVAL`]: rate, connections, refused + failed (the rest = metrics)
//!
//! - WARN once any request was refused or failed, else INFO; silent while idle
//! - first server fault of a window logged as it happens (ERROR `Request failed`), the rest counted

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tonic::Code;
use tracing::{error, info, warn};

use crate::metrics::{code_label, method_label};
use crate::service::Method;

pub(crate) const INTERVAL: Duration = Duration::from_secs(60);

static REQUESTS: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
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

/// (requests, refused, failed) since the last call, zeroed
fn take() -> (u64, u64, u64) {
    let take = |counter: &AtomicU64| counter.swap(0, Ordering::Relaxed);
    FAILURE_LOGGED.store(false, Ordering::Relaxed);
    (take(&REQUESTS), take(&REFUSED), take(&FAILED))
}

/// Logs the window since the last call (`elapsed` long, `connections` held now)
pub(crate) fn summarise(elapsed: Duration, connections: usize) {
    let (requests, refused, failed) = take();
    if requests == 0 && connections == 0 {
        return;
    }
    let per_s =
        Duration::from_secs(requests).div_duration_f64(elapsed.max(Duration::from_nanos(1)));
    let rps = (per_s * 100.0).round() / 100.0;
    match refused + failed {
        0 => info!(rps, conns = connections, "Serving"),
        _ => warn!(rps, conns = connections, refused, failed, "Serving"),
    }
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

        assert_eq!(take(), (5, 1, 1), "(requests, refused, failed)");
        assert_eq!(take(), (0, 0, 0), "taken = zeroed");
    }
}
