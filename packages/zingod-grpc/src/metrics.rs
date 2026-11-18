//! Serve-path metrics, one call site per measurement

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use metrics::Unit;
use tonic::Code;
use zingod_primitives::metric::Metric;

use crate::admission::Refusal;
use crate::lifetime::RetireReason;
use crate::origin::OriginVerdict;
use crate::service::{Method, Reply};

const REQUESTS_TOTAL: &str = "zingod.grpc.requests_total";
const FIRST_MESSAGE_SECONDS: &str = "zingod.grpc.first_message_seconds";
const DURATION_SECONDS: &str = "zingod.grpc.duration_seconds";
const SENT_BYTES_TOTAL: &str = "zingod.grpc.sent_bytes_total";
const ACTIVE_STREAMS: &str = "zingod.grpc.active_streams";
const ADMISSION_REJECTED_TOTAL: &str = "zingod.grpc.admission_rejected_total";
const ACCEPT_ERRORS_TOTAL: &str = "zingod.grpc.accept_errors_total";
const CONNECTIONS_ACTIVE: &str = "zingod.grpc.connections_active";
const CONNECTIONS_REJECTED_TOTAL: &str = "zingod.grpc.connections_rejected_total";
const STALLED_CONNECTIONS_TOTAL: &str = "zingod.grpc.stalled_connections_total";
const PARKED_BYTES: &str = "zingod.grpc.parked_bytes";
const PARKED_EVICTED_TOTAL: &str = "zingod.grpc.parked_evicted_total";
const RETIRED_CONNECTIONS_TOTAL: &str = "zingod.grpc.retired_connections_total";
const ADAPTIVE_STREAM_LIMIT: &str = "zingod.grpc.adaptive_stream_limit";
const ADAPTIVE_STREAM_CEILING: &str = "zingod.grpc.adaptive_stream_ceiling";
const ADAPTIVE_SCHEDULING_DELAY_SECONDS: &str = "zingod.grpc.adaptive_scheduling_delay_seconds";
const READ_WAIT_SECONDS: &str = "zingod.grpc.read_wait_seconds";
const OVERLOAD_SHARE: &str = "zingod.grpc.overload_share";
const UPLINK_WAITING_CONNECTIONS: &str = "zingod.grpc.uplink_waiting_connections";
const UPLINK_QUEUE_DELAY_MEDIAN_SECONDS: &str = "zingod.grpc.uplink_queue_delay_median_seconds";
const UPLINK_RETRANSMITTED_RATIO: &str = "zingod.grpc.uplink_retransmitted_ratio";
const UPLINK_CONGESTED: &str = "zingod.grpc.uplink_congested";
const ORIGIN_REQUESTS_TOTAL: &str = "zingod.grpc.origin_requests_total";

/// Σ `SENT_BYTES_TOTAL` over methods, readable without the metrics recorder (zingod's egress)
static SENT_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn sent_bytes_total() -> u64 {
    SENT_BYTES.load(Ordering::Relaxed)
}

/// `method` label (`None` = `unknown`: any other path, so the set stays fixed)
pub(crate) fn method_label(method: Option<Method>) -> &'static str {
    method.map_or("unknown", Method::name)
}

/// `code` label = `Code`'s `Debug` spelling (never formatted per request)
pub(crate) fn code_label(code: Code) -> &'static str {
    match code {
        Code::Ok => "Ok",
        Code::Cancelled => "Cancelled",
        Code::Unknown => "Unknown",
        Code::InvalidArgument => "InvalidArgument",
        Code::DeadlineExceeded => "DeadlineExceeded",
        Code::NotFound => "NotFound",
        Code::AlreadyExists => "AlreadyExists",
        Code::PermissionDenied => "PermissionDenied",
        Code::ResourceExhausted => "ResourceExhausted",
        Code::FailedPrecondition => "FailedPrecondition",
        Code::Aborted => "Aborted",
        Code::OutOfRange => "OutOfRange",
        Code::Unimplemented => "Unimplemented",
        Code::Internal => "Internal",
        Code::Unavailable => "Unavailable",
        Code::DataLoss => "DataLoss",
        Code::Unauthenticated => "Unauthenticated",
    }
}

/// Unknown path = one reply (answered `UNIMPLEMENTED`)
fn reply(method: Option<Method>) -> Reply {
    method.map_or(Reply::One, |method| method.route().reply)
}

pub const METRICS: &[Metric] = &[
    Metric::counter(REQUESTS_TOTAL, "Finished requests, by method and gRPC status code"),
    Metric::histogram(
        DURATION_SECONDS,
        "One-reply RPC latency: admission through trailers, OK answers only, by method",
    )
    .unit(Unit::Seconds),
    Metric::histogram(
        FIRST_MESSAGE_SECONDS,
        "Server-streaming RPC latency: admission to first message, by method",
    )
    .unit(Unit::Seconds),
    Metric::counter(SENT_BYTES_TOTAL, "Response body bytes, by method").unit(Unit::Bytes),
    Metric::gauge(
        ACTIVE_STREAMS,
        "Work streams holding an admission slot (one whose client stopped reading loses it when \
         slots run short)",
    ),
    Metric::counter(
        ADMISSION_REJECTED_TOTAL,
        "Work streams refused UNAVAILABLE, by reason: limit (the adaptive work-stream limit full), \
         client_share (one address or /64 holding 1/16 of it)",
    ),
    Metric::counter(
        ACCEPT_ERRORS_TOTAL,
        "accept() failures the listener backed off from (EMFILE, ENFILE, ENOBUFS, ...)",
    ),
    Metric::gauge(CONNECTIONS_ACTIVE, "Open client connections"),
    Metric::counter(
        STALLED_CONNECTIONS_TOTAL,
        "Connections closed for holding data their client did not read within unread_timeout",
    ),
    Metric::gauge(
        PARKED_BYTES,
        "Bytes held for streams waiting on their clients to read (estimate: 4 KiB + 2 messages \
         each) and request bodies still arriving; over the host's budget the heaviest client's \
         heaviest connection closes",
    )
    .unit(Unit::Bytes),
    Metric::counter(
        PARKED_EVICTED_TOTAL,
        "Connections closed to bring parked_bytes back under the host's budget",
    ),
    Metric::counter(CONNECTIONS_REJECTED_TOTAL, "Connections refused by the accept-time caps"),
    Metric::counter(
        RETIRED_CONNECTIONS_TOTAL,
        "Connections sent GOAWAY, by reason: age, idle (gRPC A9) or retired (an operator hold)",
    ),
    Metric::gauge(
        ADAPTIVE_STREAM_LIMIT,
        "Work streams admitted at once now (the ceiling until the CPU falls behind)",
    ),
    Metric::gauge(ADAPTIVE_STREAM_CEILING, "The work limit's ceiling (limit below it = shedding)"),
    Metric::gauge(
        ADAPTIVE_SCHEDULING_DELAY_SECONDS,
        "gRPC tasks' wait for a CPU core once ready (woken to polled), median of the adaptive \
         limit's last 5 s decision; disk, client and network waits never count (sheds over 0.1 \
         for 15 s)",
    )
    .unit(Unit::Seconds),
    Metric::histogram(
        READ_WAIT_SECONDS,
        "Index reads' wait for a thread, by pool: read (read_threads), scan (scan_threads)",
    )
    .unit(Unit::Seconds),
    Metric::gauge(
        OVERLOAD_SHARE,
        "Share of connections hurt, smoothed over 60 s (updated every 10 s), by cause: refused \
         (work refused at the limit) or network (waiting on a congested uplink). Overloaded from \
         >= 0.10 (causes summed) until under 0.02",
    ),
    Metric::gauge(
        UPLINK_WAITING_CONNECTIONS,
        "Debug: connections whose sending the network held back, mean over the last 10 s (counts \
         toward overload_share only while uplink_congested)",
    ),
    Metric::gauge(
        UPLINK_QUEUE_DELAY_MEDIAN_SECONDS,
        "Debug: median over sending connections of smoothed RTT above the connection's own \
         lifetime lowest (congested at >= 0.05)",
    )
    .unit(Unit::Seconds),
    Metric::gauge(
        UPLINK_RETRANSMITTED_RATIO,
        "Debug: share of TCP bytes resent over the last 10 s (congested at >= 0.02)",
    ),
    Metric::gauge(
        UPLINK_CONGESTED,
        "Debug: 1 once queue delay or resends stay over their line for 30 s (>= 10 senders)",
    ),
    Metric::counter(
        ORIGIN_REQUESTS_TOTAL,
        "gRPC-web requests carrying an Origin, by gate verdict (blocked, allowed, default)",
    ),
];

pub(crate) fn connection_retired(reason: RetireReason) {
    metrics::counter!(RETIRED_CONNECTIONS_TOTAL, "reason" => reason.label()).increment(1);
}

/// One adaptive-limit decision: its limit + ceiling, and the median scheduling delay it weighed
pub(crate) fn adaptive_decided(limit: usize, ceiling: usize, delay: Duration) {
    metrics::gauge!(ADAPTIVE_STREAM_LIMIT).set(limit as f64);
    metrics::gauge!(ADAPTIVE_STREAM_CEILING).set(ceiling as f64);
    metrics::gauge!(ADAPTIVE_SCHEDULING_DELAY_SECONDS).set(delay.as_secs_f64());
}

/// One uplink window: the congestion gate's inputs (waiting, median delay, resent ‰) + verdict
pub(crate) fn uplink_window(
    waiting: f64,
    median: Duration,
    resent_per_mille: u64,
    congested: bool,
) {
    metrics::gauge!(UPLINK_WAITING_CONNECTIONS).set(waiting);
    metrics::gauge!(UPLINK_QUEUE_DELAY_MEDIAN_SECONDS).set(median.as_secs_f64());
    metrics::gauge!(UPLINK_RETRANSMITTED_RATIO).set(resent_per_mille as f64 / 1000.0);
    metrics::gauge!(UPLINK_CONGESTED).set(f64::from(u8::from(congested)));
}

pub(crate) fn read_waited(pool: &'static str, waited: Duration) {
    metrics::histogram!(READ_WAIT_SECONDS, "pool" => pool).record(waited);
}

pub(crate) fn connection_opened() {
    metrics::gauge!(CONNECTIONS_ACTIVE).increment(1.0);
}

pub(crate) fn connection_closed() {
    metrics::gauge!(CONNECTIONS_ACTIVE).decrement(1.0);
}

/// A connection closed for holding data unread past `unread_timeout`
pub(crate) fn connection_stalled() {
    metrics::counter!(STALLED_CONNECTIONS_TOTAL).increment(1);
}

pub(crate) fn parked_bytes(total: u64) {
    metrics::gauge!(PARKED_BYTES).set(total as f64);
}

pub(crate) fn parked_evicted() {
    metrics::counter!(PARKED_EVICTED_TOTAL).increment(1);
}

pub(crate) fn accept_failed() {
    metrics::counter!(ACCEPT_ERRORS_TOTAL).increment(1);
}

pub(crate) fn connection_rejected() {
    metrics::counter!(CONNECTIONS_REJECTED_TOTAL).increment(1);
}

pub(crate) fn stream_admitted() {
    metrics::gauge!(ACTIVE_STREAMS).increment(1.0);
}

pub(crate) fn stream_released() {
    metrics::gauge!(ACTIVE_STREAMS).decrement(1.0);
}

pub(crate) fn stream_rejected(refusal: Refusal) {
    metrics::counter!(ADMISSION_REJECTED_TOTAL, "reason" => refusal.label()).increment(1);
}

fn verdict_label(verdict: OriginVerdict) -> &'static str {
    match verdict {
        OriginVerdict::Blocked => "blocked",
        OriginVerdict::Allowed => "allowed",
        OriginVerdict::Default => "default",
    }
}

/// One `Origin`-bearing (gRPC-web) request, by gate verdict (native requests never counted here)
pub(crate) fn origin_request(verdict: OriginVerdict) {
    metrics::counter!(ORIGIN_REQUESTS_TOTAL, "verdict" => verdict_label(verdict)).increment(1);
}

/// One overload window: wallets hurt by cause, over `open` connections (the latch = readiness)
pub(crate) fn overload_shares(refused: f64, network: f64) {
    metrics::gauge!(OVERLOAD_SHARE, "cause" => "refused").set(refused);
    metrics::gauge!(OVERLOAD_SHARE, "cause" => "network").set(network);
}

/// A stream's first message (a one-reply RPC's = its whole answer: [`stream_finished`] times it)
pub(crate) fn first_message(method: Option<Method>, elapsed: Duration) {
    if reply(method) == Reply::Stream {
        let method = method_label(method);
        metrics::histogram!(FIRST_MESSAGE_SECONDS, "method" => method).record(elapsed);
    }
}

/// One stream's close-out (recorded where the body drops)
pub(crate) fn stream_finished(method: Option<Method>, code: Code, elapsed: Duration, sent: u64) {
    let name = method_label(method);
    metrics::counter!(REQUESTS_TOTAL, "method" => name, "code" => code_label(code)).increment(1);
    // answers only (refusal = µs of no work)
    if reply(method) == Reply::One && code == Code::Ok {
        metrics::histogram!(DURATION_SECONDS, "method" => name).record(elapsed);
    }
    metrics::counter!(SENT_BYTES_TOTAL, "method" => name).increment(sent);
    SENT_BYTES.fetch_add(sent, Ordering::Relaxed);
}
