//! Serve-path metrics, one call site per measurement

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use metrics::Unit;
use tonic::Code;
use zingod_primitives::metric::Metric;

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
const RETIRED_CONNECTIONS_TOTAL: &str = "zingod.grpc.retired_connections_total";
const ADAPTIVE_STREAM_LIMIT: &str = "zingod.grpc.adaptive_stream_limit";
const OVERLOADED: &str = "zingod.grpc.overloaded";
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
    Metric::gauge(ACTIVE_STREAMS, "Work streams holding an admission slot"),
    Metric::counter(
        ADMISSION_REJECTED_TOTAL,
        "Work streams refused UNAVAILABLE at the adaptive work-stream limit",
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
    Metric::counter(CONNECTIONS_REJECTED_TOTAL, "Connections refused by the accept-time caps"),
    Metric::counter(
        RETIRED_CONNECTIONS_TOTAL,
        "Connections sent GOAWAY, by reason: age, idle (gRPC A9) or retired (an operator hold)",
    ),
    Metric::gauge(
        ADAPTIVE_STREAM_LIMIT,
        "Work streams admitted at once now (latency-adaptive, max_work_streams the ceiling)",
    ),
    Metric::gauge(
        OVERLOADED,
        "1 while admission has refused work persistently (= /readyz down, reason overloaded)",
    ),
    Metric::counter(
        ORIGIN_REQUESTS_TOTAL,
        "gRPC-web requests carrying an Origin, by gate verdict (blocked, allowed, default)",
    ),
];

pub(crate) fn connection_retired(reason: RetireReason) {
    metrics::counter!(RETIRED_CONNECTIONS_TOTAL, "reason" => reason.label()).increment(1);
}

/// Set at each serving summary
pub(crate) fn adaptive_stream_limit(limit: usize) {
    metrics::gauge!(ADAPTIVE_STREAM_LIMIT).set(limit as f64);
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

pub(crate) fn stream_rejected() {
    metrics::counter!(ADMISSION_REJECTED_TOTAL).increment(1);
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

/// Set at each overload window
pub(crate) fn overloaded(latched: bool) {
    metrics::gauge!(OVERLOADED).set(f64::from(u8::from(latched)));
}

/// A stream's first message (a one-reply RPC's = its whole answer: [`stream_finished`] times it)
pub(crate) fn first_message(method: Option<Method>, elapsed: Duration) {
    if reply(method) == Reply::Stream {
        let method = method_label(method);
        metrics::histogram!(FIRST_MESSAGE_SECONDS, "method" => method).record(elapsed);
    }
}

/// One stream's close-out (recorded where the body drops)
///
/// - `latency` = time to its first message, for an answered work request
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
