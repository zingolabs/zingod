//! [`GrpcLimits`]: serving caps (zingod's `[grpc]`)
//!
//! - every cap refuses, never queues

use std::num::{NonZeroU32, NonZeroUsize};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zingod_primitives::serde_secs;

use crate::proxy_protocol::TrustedProxies;

/// What one zingod serves at once + which peers name their client (zingod's `[grpc]`)
///
/// - each field's contract: usage.md "Serve stack"
/// - `max_streams_per_connection` = every h2 stream, `max_work_streams` = admitted work only
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct GrpcLimits {
    pub max_connections: NonZeroUsize,
    pub max_connections_per_ip: NonZeroUsize,
    pub max_streams_per_connection: NonZeroU32,
    pub max_work_streams: NonZeroUsize,
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
            max_connections: const { non_zero(4096) },
            // CGNAT: many wallets behind one carrier address
            max_connections_per_ip: const { non_zero(128) },
            // lightwalletd's `grpc-max-concurrent-streams` (clients tuned against it)
            max_streams_per_connection: const { NonZeroU32::new(100).expect("100 is non-zero") },
            max_work_streams: const { non_zero(2048) },
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
