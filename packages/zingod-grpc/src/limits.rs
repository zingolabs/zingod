//! [`GrpcLimits`]: serving caps (zingod's `[grpc]`)
//!
//! - every cap refuses, never queues

use std::num::NonZeroUsize;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zingod_primitives::serde_secs;

use crate::proxy_protocol::TrustedProxies;

/// What the host holds, never configured (zingod derives both from its open-file limit + memory)
///
/// - `parked_bytes` = budget for streams waiting on their clients (`stall::Parked`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostCapacity {
    pub connections: NonZeroUsize,
    pub parked_bytes: u64,
}

/// Share of the work limit one client (IPv4 address / IPv6 /64) may hold: in (0, 1], 1 = none
/// (a load test's few generator addresses)
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct WorkShare(f64);

/// Parsed in (0, 1]: never NaN
impl Eq for WorkShare {}

impl Default for WorkShare {
    /// 1/16: 128 slots at the 2048 ceiling, filling a node takes 16 clients
    fn default() -> Self {
        Self(1.0 / 16.0)
    }
}

impl TryFrom<f64> for WorkShare {
    type Error = String;

    fn try_from(share: f64) -> Result<Self, Self::Error> {
        match share > 0.0 && share <= 1.0 {
            true => Ok(Self(share)),
            false => Err(format!("work_share_per_ip = {share}: must be over 0 and at most 1")),
        }
    }
}

impl From<WorkShare> for f64 {
    fn from(share: WorkShare) -> Self {
        share.0
    }
}

impl WorkShare {
    /// Streams one client may hold at `limit` (≥ 1)
    pub(crate) fn of(self, limit: usize) -> usize {
        ((limit as f64 * self.0) as usize).max(1)
    }
}

/// What one zingod serves at once + which peers name their client (zingod's `[grpc]`)
///
/// - each field's contract: usage.md "Serve stack"
/// - hardware-sized caps = [`HostCapacity`], never set here; work =
///   [`WORK_CEILING`](crate::adaptive::WORK_CEILING), lowered by Gradient2
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct GrpcLimits {
    pub max_connections_per_ip: NonZeroUsize,
    pub work_share_per_ip: WorkShare,
    pub read_threads: NonZeroUsize,
    pub scan_threads: NonZeroUsize,
    #[serde(rename = "unread_timeout_secs", with = "serde_secs::nonzero")]
    pub unread_timeout: Duration,
    #[serde(rename = "max_connection_age_secs", with = "serde_secs::zero_is_never")]
    pub max_connection_age: Option<Duration>,
    #[serde(rename = "max_connection_age_grace_secs", with = "serde_secs::any")]
    pub max_connection_age_grace: Duration,
    #[serde(rename = "max_connection_idle_secs", with = "serde_secs::zero_is_never")]
    pub max_connection_idle: Option<Duration>,
    pub trusted_proxies: TrustedProxies,
    #[serde(rename = "shutdown_grace_secs", with = "serde_secs::any")]
    pub drain_timeout: Duration,
}

impl Default for GrpcLimits {
    fn default() -> Self {
        const fn non_zero(value: usize) -> NonZeroUsize {
            NonZeroUsize::new(value).expect("a literal default is non-zero")
        }
        Self {
            // CGNAT: many wallets behind one carrier address
            max_connections_per_ip: const { non_zero(128) },
            work_share_per_ip: WorkShare::default(),
            // an NVMe's useful parallelism (SATA SSD 16, HDD 4: operators' DoS protection page)
            read_threads: const { non_zero(32) },
            // address-history scans apart: a flood of them never queues a block read
            scan_threads: const { non_zero(4) },
            // past pepper-sync's longest legitimate backpressure (scanning a fetched batch)
            unread_timeout: Duration::from_secs(300),
            // long-held wallet connections re-resolve DNS (gRPC A9)
            max_connection_age: Some(Duration::from_secs(30 * 60)),
            // a whole `GetBlockRange` sync
            max_connection_age_grace: Duration::from_secs(600),
            max_connection_idle: Some(Duration::from_secs(30 * 60)),
            trusted_proxies: TrustedProxies::default(),
            drain_timeout: Duration::from_secs(10),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In (0, 1] or refused; a client's streams = its share of the limit now, at least 1
    #[test]
    fn a_work_share_parses_in_zero_to_one_and_sizes_a_client_by_the_limit() {
        // case: share, streams at the 2048 ceiling, at the 128 floor (None = refused)
        #[rustfmt::skip]
        let cases = [
            ("default 1/16",  WorkShare::default().0, Some((128, 8))),
            ("load test",     1.0,                    Some((2048, 128))),
            ("tiny",          0.0001,                 Some((1, 1))),
            ("none",          0.0,                    None),
            ("over all",      1.5,                    None),
            ("not a number",  f64::NAN,               None),
        ];
        for (case, share, want) in cases {
            let sized =
                WorkShare::try_from(share).ok().map(|share| (share.of(2048), share.of(128)));

            assert_eq!(sized, want, "{case}");
        }
    }
}
