//! Peer-set request metrics (`request` = zebra's `Request::command`: a bounded label set, the one
//! zebra's own `zebra_net_out_requests` counts by)

use metrics::Unit;
use zingod_primitives::metric::{CallMetrics, Metric};

pub(crate) const CALLS: CallMetrics = CallMetrics {
    in_flight: "zingod.peers.in_flight",
    duration: "zingod.peers.request_duration_seconds",
    failures: "zingod.peers.request_failures_total",
};

pub const METRICS: &[Metric] = &[
    Metric::gauge(CALLS.in_flight, "Peer-set requests in flight (ready wait included), by request"),
    Metric::histogram(
        CALLS.duration,
        "Peer round trip (sent to a ready peer → its answer), answered requests only, by request",
    )
    .unit(Unit::Seconds),
    Metric::counter(
        CALLS.failures,
        "Peer-set requests that settled on an error, by request and failure (timeout; peer = \
         zebra's error: no ready peer, an inventory `notfound`)",
    ),
];
