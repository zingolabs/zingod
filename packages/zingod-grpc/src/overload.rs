//! Overload latch: share of wallets hurt, smoothed, over 10 % → readiness `overloaded`, back under 2 %
//!
//! - hurt = refused work (each connection once a tick) or held back by a congested uplink
//! - each [`TICK`]'s share of its connections, smoothed over [`TIME_CONSTANT`] (an EWMA, as the
//!   kernel's PSI averages): a trip takes hurt that lasts, never one bad tick
//! - no minimum count: each input needs a crowd already (refusal = limit ≥ 128 full; network =
//!   ≥ 10 senders congested 30 s)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tracing::{info, warn};

use crate::metrics;

pub(crate) const TICK: Duration = Duration::from_secs(10);

/// 60 s: one tick trips only at ≥ 65 % hurt; 37 % hurt trips in 20 s; release ≈ 2-3 min
const TIME_CONSTANT: Duration = Duration::from_secs(60);

/// Smoothed share: >= ENTER latches, < EXIT clears (the gap = no flapping)
const ENTER: f64 = 0.10;
const EXIT: f64 = 0.02;

/// One connection's mark: the tick it was last counted refused in (+1; 0 = never)
#[derive(Debug, Default)]
pub(crate) struct Wallet {
    refused_in: AtomicU64,
}

/// - `network_milli` = connections held back by a congested uplink now (× 1000; the uplink
///   sampler's latest dump, a level: its dumps and these ticks never align)
/// - share of the tick's connections: open at its start (`open_before`) + `opened` in it
#[derive(Debug, Default)]
pub(crate) struct Overload {
    tick: AtomicU64,
    refused: AtomicU64,
    network_milli: AtomicU64,
    open_before: AtomicU64,
    opened: AtomicU64,
    smoothed: Mutex<Shares>,
    latched: AtomicBool,
}

/// Smoothed share hurt, by cause (their sum = the latch's input)
#[derive(Debug, Default, Clone, Copy)]
struct Shares {
    refused: f64,
    network: f64,
}

impl Overload {
    /// Refused work; a wallet counts once per tick however often it is refused
    pub(crate) fn refused(&self, wallet: &Wallet) {
        let mark = self.tick.load(Ordering::Relaxed) + 1;
        if wallet.refused_in.swap(mark, Ordering::Relaxed) != mark {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Mean connections the network holds back now (0 = uplink clear; `tcp_info.rs`)
    pub(crate) fn network_waiting(&self, connections: f64) {
        let milli = (connections.max(0.0) * 1000.0) as u64;
        self.network_milli.store(milli, Ordering::Relaxed);
    }

    /// A connection accepted (counted into the tick it opens in)
    pub(crate) fn connected(&self) {
        self.opened.fetch_add(1, Ordering::Relaxed);
    }

    /// Folds the tick (`elapsed` long, `connections` open now) into the smoothed share
    pub(crate) fn tick(&self, elapsed: Duration, connections: usize) {
        self.tick.fetch_add(1, Ordering::Relaxed);
        let refused = self.refused.swap(0, Ordering::Relaxed);
        let network = self.network_milli.load(Ordering::Relaxed) as f64 / 1000.0;
        // every connection the tick had (not just those open at its end: refused wallets reconnect)
        let open_now = u64::try_from(connections).unwrap_or(u64::MAX);
        let had = self.open_before.swap(open_now, Ordering::Relaxed);
        let opened = self.opened.swap(0, Ordering::Relaxed);
        let had = had.saturating_add(opened).max(open_now).max(1) as f64;
        let instant =
            Shares { refused: (refused as f64).min(had) / had, network: network.min(had) / had };
        // elapsed-weighted: a late tick weighs as long as it ran
        let weight = 1.0 - (-elapsed.as_secs_f64() / TIME_CONSTANT.as_secs_f64()).exp();
        let shares = {
            let mut smoothed = self.smoothed.lock().expect("overload shares mutex poisoned");
            smoothed.refused += weight * (instant.refused - smoothed.refused);
            smoothed.network += weight * (instant.network - smoothed.network);
            *smoothed
        };
        let hurt = shares.refused + shares.network;
        let was = self.latched.load(Ordering::Relaxed);
        let now = match was {
            false => hurt >= ENTER,
            true => hurt >= EXIT,
        };
        self.latched.store(now, Ordering::Relaxed);
        let percent = |share: f64| (share * 1000.0).round() / 10.0;
        let (refused_percent, network_percent) = (percent(shares.refused), percent(shares.network));
        match (was, now) {
            (false, true) => {
                warn!(refused_percent, network_percent, connections, "Overload latched")
            }
            (true, false) => {
                info!(refused_percent, network_percent, connections, "Overload cleared")
            }
            _ => {}
        }
        metrics::overload_shares(shares.refused, shares.network);
    }

    pub(crate) fn latched(&self) -> bool {
        self.latched.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoothed over 60 s: hurt that lasts trips it, one bad tick (< 65 %) never; under 2 % = back
    #[test]
    fn the_latch_trips_on_lasting_hurt_over_ten_percent_and_releases_under_two() {
        // case, runs of ticks: (refused, held by the network, ticks) of 1000 connections,
        // ticks (1-based) where the latch flips
        #[rustfmt::skip]
        let cases = [
            ("9 % refused, 5 min",             vec![(90, 0, 30)],                  vec![]),
            ("15 % refused",                   vec![(150, 0, 12)],                 vec![7]),
            ("one tick 60 %",                  vec![(600, 0, 1), (0, 0, 30)],      vec![]),
            ("one tick 70 %, then healthy",    vec![(700, 0, 1), (0, 0, 30)],      vec![1, 12]),
            ("37 % network 1 min, then clear", vec![(0, 370, 6), (0, 0, 20)],      vec![2, 21]),
            ("6 % refused + 6 % network",      vec![(60, 60, 30)],                 vec![11]),
            ("tripped, then 5 % holds it",     vec![(1000, 0, 1), (50, 0, 40)],    vec![1]),
        ];
        for (case, runs, want) in cases {
            let overload = Overload::default();

            let mut flips = Vec::new();
            let ticks = runs.iter().flat_map(|&(refused, network, ticks)| {
                std::iter::repeat_n((refused, network), ticks)
            });
            for (tick, (refused, network)) in (1..).zip(ticks) {
                let was = overload.latched();
                for _ in 0..refused {
                    overload.refused(&Wallet::default());
                }
                overload.network_waiting(f64::from(network));
                overload.tick(TICK, 1_000);
                if overload.latched() != was {
                    flips.push(tick);
                }
            }

            assert_eq!(flips, want, "{case}: ticks where the latch flipped");
        }
    }

    /// Wallets refused then reconnecting: the share is of every connection the tick had, ≤ 100 %
    #[test]
    fn the_share_counts_every_connection_of_the_tick_not_just_those_left_open() {
        let overload = Overload::default();
        overload.tick(TICK, 1_000);

        // 1000 open at the start, 2000 more opened, 3000 refused, 900 left open at the end
        for _ in 0..2_000 {
            overload.connected();
        }
        for _ in 0..3_000 {
            overload.refused(&Wallet::default());
        }
        overload.tick(TICK, 900);

        let smoothed = overload.smoothed.lock().expect("shares").refused;
        let one_tick = 1.0 - (-1.0f64 / 6.0).exp();
        assert!((smoothed - one_tick).abs() < 1e-9, "3000 of 3000 hurt = 100 %: {smoothed}");
        let open_before = overload.open_before.load(Ordering::Relaxed);
        assert_eq!(open_before, 900, "the next tick starts from those still open");
    }

    /// One wallet refused all tick = one wallet hurt; counted again in the next tick
    #[test]
    fn a_wallet_refused_many_times_counts_once_a_tick() {
        let overload = Overload::default();
        let wallet = Wallet::default();
        let count = |overload: &Overload| overload.refused.load(Ordering::Relaxed);

        for _ in 0..500 {
            overload.refused(&wallet);
        }
        assert_eq!(count(&overload), 1, "500 refusals, one wallet");

        overload.tick(TICK, 20);
        overload.refused(&wallet);
        overload.refused(&Wallet::default());
        assert_eq!(count(&overload), 2, "next tick: the same wallet again + another");
    }
}
