//! Client's `grpc-timeout` = a request's only time bound (never a total cap)
//!
//! - past it: pending handler = `DEADLINE_EXCEEDED`, a stream ends in `DEADLINE_EXCEEDED` trailers
//! - a stream that keeps delivering lives as long as the client wants (Z#1537)
//! - client stopped reading = `stall.rs` (`unread_timeout` → connection closed)

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, Response};
use http_body::Frame;
use tokio::time::{Instant, Sleep};
use tonic::{body::Body, Status};

use crate::wire::trailers;

/// `grpc-timeout` (gRPC over HTTP/2: <= 8 digits + `H M S m u n`)
///
/// - malformed = none (as tonic reads it)
fn requested(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get("grpc-timeout")?.to_str().ok()?;
    let (digits, unit) = value.split_at_checked(value.len().checked_sub(1)?)?;
    if digits.is_empty() || digits.len() > 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let amount: u64 = digits.parse().ok()?;
    match unit {
        "H" => Some(Duration::from_secs(amount * 3_600)),
        "M" => Some(Duration::from_secs(amount * 60)),
        "S" => Some(Duration::from_secs(amount)),
        "m" => Some(Duration::from_millis(amount)),
        "u" => Some(Duration::from_micros(amount)),
        "n" => Some(Duration::from_nanos(amount)),
        _ => None,
    }
}

/// `headers`' deadline, fixed at arrival
pub(crate) fn of(headers: &HeaderMap) -> Option<Instant> {
    requested(headers).map(|timeout| Instant::now() + timeout)
}

/// `answer` cut at `deadline`, its stream body too (`None` = never cut)
pub(crate) async fn apply(
    deadline: Option<Instant>,
    answer: impl Future<Output = Response<Body>>,
) -> Response<Body> {
    let Some(deadline) = deadline else { return answer.await };
    let Ok(response) = tokio::time::timeout_at(deadline, answer).await else {
        return expired().into_http();
    };
    // grpc-status in the headers = complete (unary or a refusal): nothing left to bound
    if response.headers().contains_key("grpc-status") {
        return response;
    }
    let deadline = Box::pin(tokio::time::sleep_until(deadline));
    response.map(|inner| Body::new(Bounded { inner: Some(inner), deadline }))
}

fn expired() -> Status {
    Status::deadline_exceeded("grpc-timeout passed")
}

/// Stream body under its deadline (`inner` dropped at the cut: its slot goes with it)
struct Bounded {
    inner: Option<Body>,
    deadline: Pin<Box<Sleep>>,
}

impl http_body::Body for Bounded {
    type Data = Bytes;
    type Error = Status;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Status>>> {
        let this = self.get_mut();
        let Some(inner) = this.inner.as_mut() else { return Poll::Ready(None) };
        let polled = match this.deadline.is_elapsed() {
            true => None,
            false => Some(Pin::new(inner).poll_frame(cx)),
        };
        match polled {
            Some(Poll::Ready(Some(Ok(frame)))) if !frame.is_trailers() => {
                Poll::Ready(Some(Ok(frame)))
            }
            Some(Poll::Ready(done)) => {
                this.inner = None;
                Poll::Ready(done)
            }
            Some(Poll::Pending) if this.deadline.as_mut().poll(cx).is_pending() => Poll::Pending,
            None | Some(Poll::Pending) => {
                this.inner = None;
                Poll::Ready(Some(Ok(Frame::trailers(trailers(&expired())))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use http::{HeaderMap, HeaderValue, Response};
    use http_body::Frame;
    use http_body_util::{BodyExt as _, StreamBody};
    use tokio::time::Instant;
    use tonic::{body::Body, Status};

    use super::requested;
    use crate::wire::trailers;

    fn timeout(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("grpc-timeout", HeaderValue::from_static(value));
        headers
    }

    /// gRPC over HTTP/2's `TimeoutValue TimeoutUnit`, anything else = no deadline (tonic's)
    #[test]
    fn grpc_timeout_reads_up_to_eight_digits_and_a_unit() {
        let secs = Duration::from_secs;
        let cases = [
            ("1H", Some(secs(3_600))),
            ("2M", Some(secs(120))),
            ("30S", Some(secs(30))),
            ("100m", Some(Duration::from_millis(100))),
            ("30000000u", Some(secs(30))),
            ("7n", Some(Duration::from_nanos(7))),
            ("99999999S", Some(secs(99_999_999))),
            ("123456789S", None),
            ("S", None),
            ("5", None),
            ("5x", None),
            ("+5S", None),
            ("-5S", None),
            (" 5S", None),
        ];
        for (value, expected) in cases {
            assert_eq!(requested(&timeout(value)), expected, "{value:?}");
        }
        assert_eq!(requested(&HeaderMap::new()), None, "absent");
    }

    /// Every frame of `response` with the second it arrived (paused clock from `start`)
    async fn drained(start: Instant, response: Response<Body>) -> Vec<(u64, String)> {
        if let Some(status) = Status::from_header_map(response.headers()) {
            return vec![(start.elapsed().as_secs(), format!("{:?}", status.code()))];
        }
        let mut body = std::pin::pin!(response.into_body());
        let mut frames = Vec::new();
        while let Some(frame) = body.frame().await {
            let at = start.elapsed().as_secs();
            match frame.expect("in-band").into_trailers() {
                Ok(trailers) => {
                    let status = Status::from_header_map(&trailers).expect("a status");
                    frames.push((at, format!("{:?}", status.code())));
                }
                Err(_) => frames.push((at, "data".to_owned())),
            }
        }
        frames
    }

    /// `Stream`: `count` records, one every `period`, then `OK` trailers
    #[derive(Clone, Copy, Debug)]
    enum Handler {
        Pending,
        Stream { count: u64, period: Duration },
    }

    async fn handle(handler: Handler) -> Response<Body> {
        use std::cmp::Ordering::{Equal, Greater, Less};

        let Handler::Stream { count, period } = handler else {
            return std::future::pending().await;
        };
        let frames = futures::stream::unfold(0, move |sent: u64| async move {
            let frame = match sent.cmp(&count) {
                Less => {
                    tokio::time::sleep(period).await;
                    Frame::data(Bytes::from_static(b"\0\0\0\0\0"))
                }
                Equal => Frame::trailers(trailers(&Status::ok(""))),
                Greater => return None,
            };
            Some((Ok::<_, Status>(frame), sent + 1))
        });
        Response::new(Body::new(StreamBody::new(Box::pin(frames))))
    }

    /// Z#1537 (paused clock): no total cap, no silence cap, only the client's deadline
    #[tokio::test(start_paused = true)]
    async fn a_stream_lives_while_it_delivers_until_the_clients_deadline() {
        let s100 = Duration::from_secs(100);
        let data = |at: u64| (at, "data".to_owned());
        let ended = |at: u64, code: &str| (at, code.to_owned());
        let every_100s_for_1000s: Vec<_> = (1..=10).map(|n| data(n * 100)).collect();
        let delivering = Handler::Stream { count: 10, period: s100 };

        // case, grpc-timeout, handler, frames as (second, data / status)
        #[rustfmt::skip]
        let cases = [
            ("delivering 1,000 s",            HeaderMap::new(), delivering,
                [every_100s_for_1000s, vec![ended(1_000, "Ok")]].concat()),
            ("client deadline 250 s",         timeout("250S"),  delivering,
                vec![data(100), data(200), ended(250, "DeadlineExceeded")]),
            ("1,000 s gaps, no deadline",     HeaderMap::new(), Handler::Stream { count: 2, period: s100 * 10 },
                vec![data(1_000), data(2_000), ended(2_000, "Ok")]),
            ("handler pending, 2 s deadline", timeout("2S"),    Handler::Pending,
                vec![ended(2, "DeadlineExceeded")]),
        ];
        for (case, headers, handler, expected) in cases {
            let start = Instant::now();

            let response = super::apply(super::of(&headers), handle(handler)).await;

            assert_eq!(drained(start, response).await, expected, "{case}: {handler:?}");
        }
    }

    /// The router reads each request's own `grpc-timeout`: a `GetTaddressBalanceStream` whose
    /// client never ends its request stream = `DEADLINE_EXCEEDED` at the header's 500 ms (else
    /// at the 30 s request-body deadline)
    #[tokio::test(start_paused = true)]
    async fn the_router_cuts_each_request_at_its_own_grpc_timeout() {
        use prost::Message as _;
        use tower::Service as _;
        use zingod_proto::proto::service;
        use zingod_sync::IndexKind;

        use crate::service::Method;
        use crate::testing::{dispatch, fixed_snapshots, indexed, routes_serving, MAINNET};

        let chain = zingod_source::testing::MockChain::regtest().network(MAINNET);
        let genesis = chain.genesis();
        let views = vec![indexed(IndexKind::TransparentAddress, &chain, genesis)];
        let held_by = zingod_chainview::TrustedValidatorSet::default();
        let snapshots = fixed_snapshots(&chain, genesis, views, held_by, &[]);
        let indexes = [IndexKind::TransparentAddress];
        let mut router = dispatch(routes_serving(MAINNET, &indexes, snapshots));

        let alice = service::Address { address: "t1Hsc1LR8yKnbbe3twRp88p6vFfC5t7DLbs".to_owned() };
        let mut framed = Vec::new();
        zingod_proto::frame::frame_into(&mut framed, |out| alice.encode_raw(out));
        // grpc-timeout, cut after (ms), its message
        for (header, cut_ms, message) in [
            (Some("500m"), 500, "grpc-timeout passed"),
            (None, 30_000, "request body not complete within 30s"),
        ] {
            let first = Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from(framed.clone())));
            let unending = futures::StreamExt::chain(
                futures::stream::iter([first]),
                futures::stream::pending(),
            );
            let mut request = http::Request::builder()
                .uri(format!("http://localhost{}", Method::GetTaddressBalanceStream.path()));
            if let Some(header) = header {
                request = request.header("grpc-timeout", header);
            }
            let mut request = request.body(StreamBody::new(unending)).expect("request");
            Method::resolve(&mut request);
            let start = Instant::now();
            let response = router.call(request).await.expect("router answers");
            let status = Status::from_header_map(response.headers()).expect("Trailers-Only status");
            let cut = (status.code(), status.message(), start.elapsed().as_millis());
            let expected = (tonic::Code::DeadlineExceeded, message, cut_ms);
            assert_eq!(cut, expected, "grpc-timeout {header:?}: (code, message, cut after ms)");
        }
    }
}
