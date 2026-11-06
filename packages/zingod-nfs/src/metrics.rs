//! NFS event counters (names = ztest's `zingod` families: a rename breaks its sync probes)
//!
//! - State gauges (`zingod_best_tip`, `zingod_fetch_height`) = `zingod-grpc-snapshot`'s, at scrape
//! - Fetch counters = the finalized path's (`zingod-sync`)

use zingod_primitives::metric::Metric;

const REORGS_TOTAL: &str = "zingod.reorgs_total";

/// Every metric this crate emits
pub const METRICS: &[Metric] = &[Metric::counter_from_zero(
    REORGS_TOTAL,
    "Served tips replaced by another branch rather than extended",
)];

pub(crate) fn reorg() {
    metrics::counter!(REORGS_TOTAL).increment(1);
}
