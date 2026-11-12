//! Overload latch: admission refusing work, persistently → readiness `overloaded`
//!
//! - one signal = the adaptive limiter's refusals (CPU / IO saturation → latency → lower limit)
//! - stepped once per serving summary (`summary::INTERVAL`); no operator thresholds

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tracing::{info, warn};

use crate::admission::Class;
use crate::metrics;

/// Refusals: >= ENTER_PERCENT of work (at >= 1 refusal/s) latches, < EXIT_PERCENT clears
const ENTER_PERCENT: u64 = 10;
const EXIT_PERCENT: u64 = 2;

/// Time either condition must hold before the latch flips (admission already sheds in seconds)
const FLIP_AFTER: Duration = Duration::from_secs(5 * 60);

/// `flipping_for` = how long the opposite condition has held
#[derive(Debug, Default)]
struct Latch {
    held: bool,
    flipping_for: Duration,
}

#[derive(Debug, Default)]
pub(crate) struct Overload {
    admitted_work: AtomicU64,
    refused_work: AtomicU64,
    refused_subscriptions: AtomicU64,
    latch: Mutex<Latch>,
}

impl Overload {
    pub(crate) fn admitted(&self, class: Class) {
        if class == Class::Work {
            self.admitted_work.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn refused(&self, class: Class) {
        metrics::stream_rejected(class);
        let refused = match class {
            Class::Work => &self.refused_work,
            Class::Subscription => &self.refused_subscriptions,
        };
        refused.fetch_add(1, Ordering::Relaxed);
    }

    /// Closes the window (`elapsed` long): its refusals, every class
    pub(crate) fn tick(&self, elapsed: Duration) -> u64 {
        let take = |counter: &AtomicU64| counter.swap(0, Ordering::Relaxed);
        let (admitted, refused) = (take(&self.admitted_work), take(&self.refused_work));
        let attempts = admitted.saturating_add(refused);
        let percent_of_attempts = |percent: u64| attempts.saturating_mul(percent);
        let refused_percent = refused.saturating_mul(100);
        let refusing = refused_percent >= percent_of_attempts(ENTER_PERCENT)
            && Duration::from_secs(refused) >= elapsed;
        let quiet = refused == 0 || refused_percent < percent_of_attempts(EXIT_PERCENT);

        let mut latch = self.latch.lock().expect("overload latch poisoned");
        let flipping = if latch.held { quiet } else { refusing };
        latch.flipping_for = if flipping { latch.flipping_for + elapsed } else { Duration::ZERO };
        if latch.flipping_for >= FLIP_AFTER {
            (latch.held, latch.flipping_for) = (!latch.held, Duration::ZERO);
            match latch.held {
                true => warn!(refused, attempts, "Overload latched"),
                false => info!(refused, attempts, "Overload cleared"),
            }
        }
        metrics::overloaded(latch.held);

        refused.saturating_add(take(&self.refused_subscriptions))
    }

    pub(crate) fn latched(&self) -> bool {
        self.latch.lock().expect("overload latch poisoned").held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    /// Enter: >= 10 % refused at >= 1 refusal/s for 5 min; exit: < 2 % for 5 min
    #[test]
    fn refusals_latch_after_five_sustained_minutes_and_clear_after_five_quiet() {
        let latched_5 = [vec![false; 4], vec![true]].concat();
        // case, work admissions/s, % refused each minute, latched after each minute
        #[rustfmt::skip]
        let cases = [
            ("9 % for 10 min",                100, vec![9; 10],                       vec![false; 10]),
            ("10 % for 5 min",                100, vec![10; 5],                       latched_5.clone()),
            ("30 %, one minute at 25 %",      100, vec![30, 30, 25, 30, 30],          latched_5.clone()),
            ("30 %, one minute at 5 %",       100, vec![30, 30, 5, 30, 30, 30],       vec![false; 6]),
            ("50 % of 1/s = 0.5 refusals/s",  1,   vec![50; 10],                      vec![false; 10]),
            ("latched, then 2 % for 5 min",   100, [vec![10; 5], vec![2; 5]].concat(), [latched_5.clone(), vec![true; 5]].concat()),
            ("latched, then 1 % for 5 min",   100, [vec![10; 5], vec![1; 5]].concat(), [latched_5.clone(), vec![true; 4], vec![false]].concat()),
        ];
        for (case, per_second, refused_percent, want) in cases {
            let overload = Overload::default();

            let latched: Vec<bool> = refused_percent
                .iter()
                .map(|&percent| {
                    let total = per_second * 60;
                    for _ in 0..total * (100 - percent) / 100 {
                        overload.admitted(Class::Work);
                    }
                    for _ in 0..total * percent / 100 {
                        overload.refused(Class::Work);
                    }
                    overload.tick(MINUTE);
                    overload.latched()
                })
                .collect();

            assert_eq!(latched, want, "{case}: latched after each minute");
        }
    }

    /// Subscription refusals count toward the summary, never toward the latch
    #[test]
    fn a_window_reports_every_refusal_and_latches_on_work_only() {
        let overload = Overload::default();
        for _ in 0..10 {
            for _ in 0..120 {
                overload.refused(Class::Subscription);
            }
            assert_eq!(overload.tick(MINUTE), 120, "every subscription refusal reported");
        }
        assert!(!overload.latched(), "10 min of subscription refusals: not overloaded");

        for _ in 0..120 {
            overload.refused(Class::Work);
        }
        overload.refused(Class::Subscription);
        assert_eq!(overload.tick(MINUTE), 121, "120 work + 1 subscription refused");
        assert_eq!(overload.tick(MINUTE), 0, "next window starts at zero");
    }
}
