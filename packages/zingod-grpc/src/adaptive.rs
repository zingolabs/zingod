//! Adaptive work-stream limit: Netflix concurrency-limits' Gradient2 over sustained CPU queueing
//!
//! - [`WORK_CEILING`] until the CPU falls behind; the limit = a shed, never a guess at capacity
//! - sample = the gRPC tasks' scheduling delay: woken (ready) → polled, i.e. waiting for a core
//!   (`tokio_metrics::TaskMonitor`); a task waiting on a disk read, a client or the network is
//!   not ready, so none of those count (shedding cannot speed them up)
//! - decision = median of 10 windows, so a block rush (≤ 2 s) moves ≤ 4 of them
//! - rules + their measured basis: `usage.md` "Serve stack", adaptive limit

use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tokio_metrics::TaskMonitor;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::metrics;

/// Fixed-point scale of the estimated limit and the gradient (integer Gradient2)
const MILLI: u64 = 1000;

/// Median scheduling delay over this = the CPU is falling behind
/// - bench (2 cores, 12k wallets): delay 7 → 158 ms with load; never shedding failed 6k sessions
///   (tip p99 1.2 s), a 20 ms budget 21k (refusals end syncs): shed only where waits hurt
const BUDGET: Duration = Duration::from_millis(100);

/// Decisions over budget in a row before the first shrink (15 s: a rush never lasts it)
const SUSTAIN: u32 = 3;

/// Largest single shrink (‰ of the limit), then smoothed: weight of a step = 1 / SMOOTHING_PARTS
const MIN_GRADIENT_MILLI: u64 = 500;
const SMOOTHING_PARTS: u64 = 5;

/// Healthy decision → limit × this (‰): floor to ceiling in ~2.5 min
const GROWTH_MILLI: u64 = 1100;

/// One window = this long, counted once ≥ [`WINDOW_WAKES`] (an idle node decides nothing); a
/// decision = [`DECISION_WINDOWS`] of them
const WINDOW: Duration = Duration::from_millis(500);
const WINDOW_WAKES: u64 = 10;
const DECISION_WINDOWS: usize = 10;

/// Lowest limit while shedding (clamped to the ceiling)
const FLOOR: usize = 128;

/// Limit while the CPU keeps up (a full disk fills it → refusals → overload: the disk's signal)
pub(crate) const WORK_CEILING: usize = 2048;

/// The limit, moved only by [`Limiter::window`]'s decisions
#[derive(Debug)]
pub(crate) struct Limiter {
    limit: AtomicUsize,
    floor: usize,
    ceiling: usize,
    gradient: Mutex<Gradient>,
}

/// `windows` = this decision's window delays; `over` = decisions over budget in a row
#[derive(Debug)]
struct Gradient {
    estimated_milli: u64,
    windows: Vec<u64>,
    over: u32,
}

/// One decision's outcome (its limit + the median delay it weighed)
struct Decision {
    limit: u64,
    delay_ns: u64,
}

impl Limiter {
    pub(crate) fn new(ceiling: usize) -> Self {
        Self {
            limit: AtomicUsize::new(ceiling),
            floor: FLOOR.min(ceiling),
            ceiling,
            gradient: Mutex::new(Gradient {
                estimated_milli: wide(ceiling).saturating_mul(MILLI),
                windows: Vec::with_capacity(DECISION_WINDOWS),
                over: 0,
            }),
        }
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit.load(Relaxed)
    }

    /// Until `cancel`: every [`WINDOW`], `tasks`' mean scheduling delay (idle windows skipped)
    pub(crate) async fn follow(&self, tasks: &TaskMonitor, cancel: &CancellationToken) {
        let mut intervals = tasks.intervals();
        let mut ticks = tokio::time::interval(WINDOW);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = ticks.tick() => {}
            }
            let Some(window) = intervals.next() else { return };
            if window.total_scheduled_count >= WINDOW_WAKES {
                self.window(window.mean_scheduled_duration());
            }
        }
    }

    /// One window's mean scheduling delay; every [`DECISION_WINDOWS`]th decides
    fn window(&self, delay: Duration) {
        let delay_ns = u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX);
        let mut gradient = self.gradient.lock().expect("adaptive limiter poisoned");
        let range =
            wide(self.floor).saturating_mul(MILLI)..=wide(self.ceiling).saturating_mul(MILLI);
        if let Some(decided) = gradient.window(delay_ns, range) {
            let limit = usize::try_from(decided.limit).expect("limit <= ceiling, a usize");
            let was = self.limit.swap(limit, Relaxed);
            let delay = Duration::from_nanos(decided.delay_ns);
            let cpu_wait_ms = delay.as_millis();
            match (was == self.ceiling, limit == self.ceiling) {
                (true, false) => warn!(limit, cpu_wait_ms, "Work limit lowered"),
                (false, true) => info!(limit, cpu_wait_ms, "Work limit restored"),
                _ => {}
            }
            metrics::adaptive_decided(limit, self.ceiling, delay);
        }
    }
}

fn wide(value: usize) -> u64 {
    u64::try_from(value).expect("usize fits u64")
}

impl Gradient {
    /// One window's delay; every [`DECISION_WINDOWS`]th decides (`range` = floor..=ceiling, ‰)
    fn window(&mut self, delay_ns: u64, range: std::ops::RangeInclusive<u64>) -> Option<Decision> {
        self.windows.push(delay_ns);
        if self.windows.len() < DECISION_WINDOWS {
            return None;
        }
        // upper median: ≥ 6 of 10 windows slow before a decision is
        let (_, &mut median, _) = self.windows.select_nth_unstable(DECISION_WINDOWS / 2);
        self.windows.clear();
        let delay = median.max(1);
        let budget = u64::try_from(BUDGET.as_nanos()).expect("budget fits u64");
        let over = delay > budget;
        self.over = if over { self.over.saturating_add(1) } else { 0 };
        let estimated = self.estimated_milli;
        let next = match over {
            false => estimated.saturating_mul(GROWTH_MILLI) / MILLI,
            true if self.over < SUSTAIN => estimated,
            true => {
                // how far over the budget = how hard to shrink (at most halved, smoothed 1/5)
                let gradient =
                    (budget.saturating_mul(MILLI) / delay).clamp(MIN_GRADIENT_MILLI, MILLI);
                let shrunk = estimated.saturating_mul(gradient) / MILLI;
                (estimated.saturating_mul(SMOOTHING_PARTS - 1) + shrunk) / SMOOTHING_PARTS
            }
        };
        self.estimated_milli = next.clamp(*range.start(), *range.end());
        Some(Decision { limit: self.estimated_milli / MILLI, delay_ns: delay })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CEILING: usize = 2048;
    const QUICK: Duration = Duration::from_micros(200);
    const SLOW: Duration = Duration::from_millis(300);

    /// `count` windows, each at `delay`: the limit after each
    fn windows(limiter: &Limiter, count: usize, delay: Duration) -> Vec<usize> {
        (0..count)
            .map(|_| {
                limiter.window(delay);
                limiter.limit()
            })
            .collect()
    }

    /// A node's day of block rushes: 2 s at 300 ms (3× the budget), every 75 s, for 30 min
    #[test]
    fn block_rushes_never_move_the_limit_off_its_ceiling() {
        let limiter = Limiter::new(CEILING);
        let mut seen = Vec::new();

        for _ in 0..24 {
            seen.extend(windows(&limiter, 4, SLOW));
            seen.extend(windows(&limiter, 146, QUICK));
        }

        assert!(seen.iter().all(|&limit| limit == CEILING), "a rush shed work: {seen:?}");
    }

    /// 200 µs, then 300 ms sustained (3× the 100 ms budget), then back
    #[test]
    fn sustained_cpu_queueing_sheds_after_fifteen_seconds_to_the_floor_and_recovery_regrows() {
        let limiter = Limiter::new(CEILING);
        windows(&limiter, 60, QUICK);

        let first_two_decisions = windows(&limiter, 20, SLOW);
        let third = windows(&limiter, 10, SLOW);
        let sustained = windows(&limiter, 400, SLOW);
        let recovered = windows(&limiter, 400, QUICK);

        let (shed, settled) = (third[9], *sustained.last().expect("400 windows"));
        assert!(first_two_decisions.iter().all(|&limit| limit == CEILING), "10 s: no shed yet");
        // halving (gradient floor 0.5) smoothed 1/5: × 0.9
        assert_eq!(shed, CEILING * 9 / 10, "15 s: one smoothed step");
        assert_eq!(settled, FLOOR, "sustained 3× the budget: down to the floor");
        assert!(sustained.iter().all(|&limit| limit >= FLOOR), "never under the floor");
        assert_eq!(recovered.last(), Some(&CEILING), "200 s back at 200 µs: the ceiling again");
    }

    /// Any delay up to the budget, sustained: never a shed
    #[test]
    fn queueing_up_to_the_budget_never_sheds() {
        let limiter = Limiter::new(CEILING);

        let busy = windows(&limiter, 600, Duration::from_millis(50));
        let at_budget = windows(&limiter, 600, BUDGET);

        assert!(busy.iter().chain(&at_budget).all(|&limit| limit == CEILING), "{busy:?}");
    }
}
