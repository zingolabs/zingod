//! Per-stream guard: admission slot, stall ticket, connection activity + close-out metrics, all
//! owned by the response body
//!
//! - request body whole before admission (a bodiless or trickled request holds no slot; its
//!   arriving bytes charged to the unread budget: `stall::Arriving`)
//! - slot held for the stream's life, reclaimable while its client has not read for a grace
//!   (`admission.rs`, `stall.rs`)
//! - body dropped = stream over, however it ended (stream end, client gone: hyper drops it either
//!   way) → slot back, close-out measured
//! - refusal measured as the request it was (counted, never logged one by one)

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::{Buf as _, Bytes};
use futures::future::BoxFuture;
use http::{HeaderMap, Request, Response};
use http_body::{Body, Frame, SizeHint};
use http_body_util::{BodyExt as _, Full};
use tonic::{Code, Status};
use tower::ServiceExt as _;

use crate::admission::{AdmissionState, Slot};
use crate::lifetime::{Activity, Open};
use crate::origin::{OriginGate, OriginVerdict};
use crate::overload::Wallet;
use crate::service::Method;
use crate::stall::{self, Ticket};
use crate::{metrics, summary, wire};

/// One per connection (cloned per request): `wallet` = this connection's overload mark
#[derive(Clone, Debug)]
pub(crate) struct Guard<Inner> {
    inner: Inner,
    admission: Arc<AdmissionState>,
    stalls: Arc<stall::Watch>,
    activity: Arc<Activity>,
    gate: Arc<OriginGate>,
    wallet: Arc<Wallet>,
}

impl<Inner> Guard<Inner> {
    pub(crate) fn new(
        inner: Inner,
        admission: Arc<AdmissionState>,
        stalls: Arc<stall::Watch>,
        activity: Arc<Activity>,
        gate: Arc<OriginGate>,
    ) -> Self {
        Self { inner, admission, stalls, activity, gate, wallet: Arc::default() }
    }
}

/// Wire protocol a request arrived in (request extension; absent = native gRPC)
#[derive(Clone, Copy, Debug)]
pub(crate) enum Protocol {
    Grpc,
    GrpcWeb,
}

impl Protocol {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Grpc => "grpc",
            Self::GrpcWeb => "grpc_web",
        }
    }

    /// Tags a gRPC-web request (before `tonic_web` rewrites its content-type to native gRPC)
    pub(crate) fn tag<B>(mut request: Request<B>) -> Request<B> {
        let content_type = request.headers().get(http::header::CONTENT_TYPE);
        if content_type.is_some_and(|value| value.as_bytes().starts_with(b"application/grpc-web")) {
            request.extensions_mut().insert(Self::GrpcWeb);
        }
        request
    }
}

/// Request `Origin` → host (scheme / port stripped, lowercased); absent = native client (default)
fn origin_host(headers: &HeaderMap) -> Option<String> {
    let origin = headers.get(http::header::ORIGIN)?.to_str().ok()?;
    let host = origin.rsplit("://").next()?.split([':', '/']).next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// `grpc-status`, plus its decoded `grpc-message` when it is not `Ok` (never read on success)
fn status_of(headers: &HeaderMap) -> Option<(Code, Option<String>)> {
    let code = Code::from_i32(headers.get("grpc-status")?.to_str().ok()?.parse().ok()?);
    let message = (code != Code::Ok)
        .then(|| Status::from_header_map(headers).map(|status| status.message().to_owned()))
        .flatten();
    Some((code, message))
}

/// Never pushes back itself: `inner` readied inside the call (`oneshot`)
impl<Inner, ReqBody> tower::Service<Request<ReqBody>> for Guard<Inner>
where
    Inner: tower::Service<Request<Full<Bytes>>, Response = Response<tonic::body::Body>>
        + Clone
        + Send
        + 'static,
    Inner::Error: Send + 'static,
    Inner::Future: Send + 'static,
    ReqBody: Body + Send + 'static,
    ReqBody::Data: Send,
    ReqBody::Error: std::fmt::Display,
{
    type Response = Response<Guarded<tonic::body::Body>>;
    type Error = Inner::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let method = Method::of_request(&request);
        let mut close = CloseOut {
            method,
            protocol: request.extensions().get().copied().unwrap_or(Protocol::Grpc),
            started: Instant::now(),
            answered: false,
            sent: 0,
            code: None,
            message: None,
            ended: false,
            _open: self.activity.opened(),
        };
        let ticket = self.stalls.ticket();
        // native requests carry no Origin (verdict Default, uncounted); gRPC-web ones are metered
        let verdict = match origin_host(request.headers()) {
            Some(host) => {
                let verdict = self.gate.verdict(&host);
                metrics::origin_request(verdict);
                summary::origin(&host, verdict);
                verdict
            }
            None => OriginVerdict::Default,
        };
        if verdict == OriginVerdict::Blocked {
            let refusal = Status::permission_denied("origin blocked").into_http();
            return Box::pin(std::future::ready(Ok(refused(refusal, ticket, close))));
        }
        let (inner, admission, wallet) =
            (self.inner.clone(), Arc::clone(&self.admission), Arc::clone(&self.wallet));
        let mut arriving = stall::Arriving::of(&self.stalls);
        let client = self.stalls.client();
        Box::pin(async move {
            let (parts, body) = request.into_parts();
            let body = body.map_frame(|frame| {
                if let Some(data) = frame.data_ref() {
                    arriving.add(data.remaining());
                }
                frame
            });
            let body = match wire::collect_request(body, method).await {
                Ok(body) => body,
                Err(status) => return Ok(refused(status.into_http(), ticket, close)),
            };
            // every unblocked origin = one admission path (Origin client-settable, never lifts a cap)
            let slot = if method.is_some_and(Method::is_subscription) {
                None
            } else {
                match admission.admit(&wallet, client) {
                    Ok(slot) => Some(slot),
                    Err(refusal) => return Ok(refused(refusal.response(), ticket, close)),
                }
            };
            drop(arriving);

            let response = inner.oneshot(Request::from_parts(parts, Full::new(body))).await?;
            // header-carried `grpc-status` = unary answer or error, already complete
            let (code, message) = status_of(response.headers()).unzip();
            (close.code, close.message) = (code, message.flatten());
            Ok(response.map(|inner| Guarded { inner, slot, ticket, close }))
        })
    }
}

/// A refusal before any handler ran, measured as the request it was (`code` = its `grpc-status`)
fn refused(
    refusal: Response<tonic::body::Body>,
    ticket: Ticket,
    mut close: CloseOut,
) -> Response<Guarded<tonic::body::Body>> {
    close.code = status_of(refusal.headers()).map(|(code, _)| code);
    refusal.map(|inner| Guarded { inner, slot: None, ticket, close })
}

/// Response body owning its stream's guards (`slot` = `None`: a subscription, a refusal, or ended)
#[derive(Debug)]
pub(crate) struct Guarded<B> {
    inner: B,
    slot: Option<Slot>,
    ticket: Ticket,
    close: CloseOut,
}

/// One stream's measurement to its end, recorded on drop
///
/// - `answered` = first DATA frame sent (its time = `first_message_seconds`)
/// - `message` = non-`Ok` status message
#[derive(Debug)]
struct CloseOut {
    method: Option<Method>,
    protocol: Protocol,
    started: Instant,
    answered: bool,
    sent: u64,
    code: Option<Code>,
    message: Option<String>,
    ended: bool,
    _open: Open,
}

impl<B: Body<Data = Bytes> + Unpin> Body for Guarded<B> {
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let this = &mut *self;
        // hyper polling = the client took the last frame; off the owing list first (the reclaim
        // sweep reclaims only what is on it, under its lock)
        this.ticket.pulled();
        if let Some(slot) = &this.slot {
            slot.resume();
        }
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        let close = &mut this.close;
        match &polled {
            Poll::Ready(Some(Ok(frame))) => match frame.data_ref() {
                Some(chunk) => {
                    this.ticket.handed(chunk.len(), this.slot.as_ref().map(Slot::claim));
                    if !close.answered {
                        close.answered = true;
                        metrics::first_message(close.method, close.started.elapsed());
                    }
                    close.sent += u64::try_from(chunk.len()).expect("usize fits u64");
                }
                // trailers = a streaming answer's real status
                None => {
                    if let Some((code, message)) = frame.trailers_ref().and_then(status_of) {
                        (close.code, close.message) = (Some(code), message);
                    }
                }
            },
            Poll::Ready(Some(Err(_)) | None) => {
                this.slot = None;
                close.ended = true;
            }
            Poll::Pending => {}
        }

        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for CloseOut {
    fn drop(&mut self) {
        // no status, no clean end = client gone mid-stream
        let code = self.code.unwrap_or(match self.ended {
            true => Code::Ok,
            false => Code::Cancelled,
        });
        let elapsed = self.started.elapsed();
        metrics::stream_finished(self.method, self.protocol, code, elapsed, self.sent);
        summary::finished(self.method, code, self.message.as_deref().unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use http::HeaderValue;
    use http_body_util::{BodyExt as _, Full, StreamBody};
    use tower::Service as _;

    use super::*;
    use crate::adaptive::WORK_CEILING;
    use crate::limits::WorkShare;

    /// - `/open` + `GetMempoolStream` never end
    /// - `/data` = one frame then waits
    /// - `/many` = a frame at every poll, never done (a range sync)
    /// - `/short` = one frame, done
    #[derive(Clone)]
    struct Answers;

    impl tower::Service<Request<Full<Bytes>>> for Answers {
        type Response = Response<tonic::body::Body>;
        type Error = Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, request: Request<Full<Bytes>>) -> Self::Future {
            let path = request.uri().path();
            let body = match path {
                _ if path == "/open" || path == Method::GetMempoolStream.path() => {
                    tonic::body::Body::new(StreamBody::new(futures::stream::pending::<
                        Result<Frame<Bytes>, Status>,
                    >()))
                }
                "/data" => {
                    let first = Ok::<_, Status>(Frame::data(Bytes::from_static(b"\0\0\0\0\0")));
                    let then = futures::stream::pending();
                    tonic::body::Body::new(StreamBody::new(futures::StreamExt::chain(
                        futures::stream::iter([first]),
                        then,
                    )))
                }
                "/many" => {
                    let frame = || Ok::<_, Status>(Frame::data(Bytes::from_static(b"\0\0\0\0\0")));
                    tonic::body::Body::new(StreamBody::new(futures::stream::repeat_with(frame)))
                }
                _ => tonic::body::Body::new(Full::new(Bytes::from_static(b"\0\0\0\0\0"))),
            };

            std::future::ready(Ok(Response::new(body)))
        }
    }

    fn request(path: &str) -> Request<Full<Bytes>> {
        let mut request = Request::builder()
            .uri(format!("http://localhost{path}"))
            .body(Full::new(Bytes::new()))
            .expect("request");
        Method::resolve(&mut request);
        request
    }

    fn request_origin(path: &str, origin: &str) -> Request<Full<Bytes>> {
        let mut request = request(path);
        request
            .headers_mut()
            .insert(http::header::ORIGIN, HeaderValue::from_str(origin).expect("origin"));
        request
    }

    /// Body whole before a slot: headers + part of a body, the rest withheld, hold none (pool of
    /// 1); its arriving bytes are charged, so past the budget its connection is the one evicted
    #[tokio::test]
    async fn a_request_takes_a_slot_only_once_its_body_has_arrived() {
        use futures::{FutureExt as _, StreamExt as _};

        let state = Arc::new(AdmissionState::new(1, WorkShare::default()));
        let parked = Arc::new(stall::Parked::new(16 * 1024));
        let trickler_watch = parked.watch([203, 0, 113, 1].into());
        let guard = |watch| {
            Guard::new(Answers, Arc::clone(&state), watch, Arc::default(), OriginGate::new())
        };
        let (mut trickler, mut wallet) =
            (guard(Arc::clone(&trickler_watch)), guard(parked.watch([203, 0, 113, 2].into())));
        let part = Ok::<_, Infallible>(Frame::data(Bytes::from(vec![0; 20 * 1024])));
        let withheld =
            StreamBody::new(futures::stream::iter([part]).chain(futures::stream::pending()));
        let mut trickled =
            Request::builder().uri("http://localhost/open").body(withheld).expect("request");
        Method::resolve(&mut trickled);

        let mut trickling = trickler.call(trickled);
        let still_collecting = (&mut trickling).now_or_never().is_none();
        let held_while_trickling = state.held();
        let evicted = trickler_watch.evicted().now_or_never().is_some();
        let admitted = wallet.call(request("/open")).await.expect("infallible");

        assert!(still_collecting, "body incomplete: no response yet");
        assert_eq!(held_while_trickling, 0, "an incomplete body holds no work slot");
        assert!(evicted, "20 KiB arriving past a 16 KiB budget: the trickler's connection picked");
        assert_eq!(status_of(admitted.headers()), None, "another client's whole request admitted");
        assert_eq!(state.held(), 1, "the one slot = the complete request's");
    }

    /// With one work slot full: blocked = `PermissionDenied` before admission; a carve-out under
    /// the block or an unmarked origin = refused at the limit like any client
    #[tokio::test]
    async fn the_origin_gate_blocks_and_otherwise_defers_to_the_limit() {
        let state = Arc::new(AdmissionState::new(1, WorkShare::default()));
        let gate = OriginGate::new();
        gate.set(["evil.com".to_owned()].into(), ["good.evil.com".to_owned()].into());
        let mut guard =
            Guard::new(Answers, Arc::clone(&state), Arc::default(), Arc::default(), gate);

        // fill the one work slot (held open, never ends)
        let held = guard.call(request("/open")).await.expect("infallible");
        let _held = std::pin::pin!(held.into_body());
        assert_eq!(state.held(), 1, "slot taken");

        let verdict = |response: &Response<Guarded<tonic::body::Body>>| {
            status_of(response.headers()).map(|(code, _)| code)
        };
        let blocked = guard.call(request_origin("/short", "https://evil.com")).await.expect("i");
        let allowed =
            guard.call(request_origin("/short", "https://good.evil.com")).await.expect("i");
        let other =
            guard.call(request_origin("/short", "https://unknown.example")).await.expect("i");

        assert_eq!(verdict(&blocked), Some(Code::PermissionDenied), "blocked origin refused");
        assert_eq!(verdict(&allowed), Some(Code::Unavailable), "carve-out refused at the limit");
        assert_eq!(verdict(&other), Some(Code::Unavailable), "default origin refused at the limit");
        assert_eq!(state.held(), 1, "no refusal took a slot");
    }

    /// Guard → ticket wiring (stall timing itself: `stall.rs`)
    #[tokio::test(start_paused = true)]
    async fn a_stream_owes_from_a_handed_data_frame_until_its_next_poll() {
        let stalls = Arc::new(stall::Watch::default());
        let admission = Arc::new(AdmissionState::new(WORK_CEILING, WorkShare::default()));
        let mut guard =
            Guard::new(Answers, admission, Arc::clone(&stalls), Arc::default(), OriginGate::new());

        let idle = guard.call(request("/open")).await.expect("infallible");
        let mut idle = std::pin::pin!(idle.into_body());
        let waiting = futures::poll!(idle.frame());
        assert!(waiting.is_pending(), "/open never sends: {waiting:?}");
        assert_eq!(stalls.oldest(), None, "a pending body owes nothing");

        let data = guard.call(request("/data")).await.expect("infallible");
        let mut data = std::pin::pin!(data.into_body());
        let first = data.frame().await.expect("a frame").expect("in-band");
        assert!(first.is_data(), "/data's first frame: {first:?}");
        let now = tokio::time::Instant::now();
        assert_eq!(
            stalls.oldest(),
            Some(now),
            "handed a data frame (paused clock): owing from now"
        );

        let pulled = futures::poll!(data.frame());
        assert!(pulled.is_pending(), "/data waits after its one frame: {pulled:?}");
        assert_eq!(stalls.oldest(), None, "polled again: pulled");
    }

    /// The body's drop = the only signal a client walked away mid-stream
    #[tokio::test]
    async fn a_stream_slot_lives_as_long_as_its_body() {
        let state = Arc::new(AdmissionState::new(WORK_CEILING, WorkShare::default()));
        let mut guard = Guard::new(
            Answers,
            Arc::clone(&state),
            Arc::default(),
            Arc::default(),
            OriginGate::new(),
        );
        let in_flight = || state.held();

        let open = guard.call(request("/open")).await.expect("infallible");
        assert_eq!(in_flight(), 1, "handler returned, stream still open");
        drop(open);
        assert_eq!(in_flight(), 0, "body dropped: returned");

        let short = guard.call(request("/short")).await.expect("infallible");
        let mut body = std::pin::pin!(short.into_body());
        assert_eq!(in_flight(), 1, "unread");
        while body.frame().await.is_some() {}
        assert_eq!(in_flight(), 0, "read to its end: returned before the drop");
    }

    /// Konclave report: never-read streams held every slot
    /// - a client reading keeps its slot; not reading past the grace, the sweep reclaims it
    /// - reclaimed, it still counts toward its client's share (limit 1 → share 1): reclaim
    ///   then reopen never grows a client past its share
    /// - reading on after a reclaim: back past the limit (never refused mid-stream), newcomers
    ///   refused until it drains; a reclaimed then dropped stream releases nothing twice
    #[tokio::test(start_paused = true)]
    async fn a_stream_whose_client_stops_reading_loses_its_slot_and_reading_on_takes_one_back() {
        use crate::stall::{Parked, GRACE};

        let state = Arc::new(AdmissionState::new(1, WorkShare::default()));
        let parked = Arc::new(Parked::default());
        let client = |last: u8| {
            let watch = parked.watch([203, 0, 113, last].into());
            Guard::new(Answers, Arc::clone(&state), watch, Arc::default(), OriginGate::new())
        };
        let (mut a_client, mut b_client, mut c_client) = (client(1), client(2), client(3));
        let refusal = |response: &Response<_>| status_of(response.headers());
        let at_limit = Some((Code::Unavailable, Some("work stream limit reached".to_owned())));
        let at_share =
            Some((Code::Unavailable, Some("per-client work stream share reached".to_owned())));

        // A: one message sent, its client not reading yet: within the grace it keeps the slot
        let a = a_client.call(request("/many")).await.expect("infallible");
        let mut a = Box::pin(a.into_body());
        a.frame().await.expect("a frame").expect("in-band");
        let b = b_client.call(request("/short")).await.expect("infallible");
        assert_eq!(refusal(&b), at_limit, "A within the grace: pool of 1 full");

        // A still not reading past the grace: reclaimed; A's client still at its share, B admitted
        tokio::time::advance(GRACE).await;
        assert_eq!(parked.reclaim(GRACE), 1, "A's slot reclaimed");
        let again = a_client.call(request("/many")).await.expect("infallible");
        assert_eq!(refusal(&again), at_share, "reclaimed A still counts for its client");
        let b = b_client.call(request("/many")).await.expect("infallible");
        assert_eq!(refusal(&b), None, "B admitted in A's place");
        let mut b = Box::pin(b.into_body());
        b.frame().await.expect("a frame").expect("in-band");

        // A's client reads on with B holding the slot: A goes on, past the limit; C refused
        a.frame().await.expect("A reads on").expect("in-band");
        assert_eq!(state.held(), 2, "A took a slot back past the limit of 1");
        let c = c_client.call(request("/short")).await.expect("infallible");
        assert_eq!(refusal(&c), at_limit, "C refused until held drains");

        // B reclaimed then gone (one release, by the sweep); A ends: every slot + share back
        tokio::time::advance(GRACE).await;
        assert_eq!(parked.reclaim(GRACE), 2, "A and B both idle past the grace");
        drop((b, a));
        assert_eq!(state.held(), 0, "nothing released twice, nothing leaked");
        let d = a_client.call(request("/short")).await.expect("infallible");
        assert_eq!(refusal(&d), None, "D admitted: A's client's share back too");
    }

    /// `GetMempoolStream` idles until the next block: never takes or waits on a work slot
    #[tokio::test]
    async fn subscriptions_open_with_every_work_slot_taken() {
        let state = Arc::new(AdmissionState::new(1, WorkShare::default()));
        let mut guard = Guard::new(
            Answers,
            Arc::clone(&state),
            Arc::default(),
            Arc::default(),
            OriginGate::new(),
        );
        let mempool = &Method::GetMempoolStream.path();
        let code = |response: &Response<_>| status_of(response.headers()).map(|(code, _)| code);

        let _work = guard.call(request("/open")).await.expect("infallible");
        let refused = guard.call(request("/open")).await.expect("infallible");
        let subscribed: Vec<_> =
            futures::future::join_all((0..3).map(|_| guard.call(request(mempool))))
                .await
                .into_iter()
                .map(|response| code(&response.expect("infallible")))
                .collect();

        assert_eq!(code(&refused), Some(Code::Unavailable), "work pool of 1 full");
        assert_eq!(subscribed, vec![None; 3], "every subscription opened");
        assert_eq!(state.held(), 1, "subscriptions hold no work slot");
    }

    /// The limit = [`WORK_CEILING`] (2048) until zingod falls behind; one client (IPv6 /64 here)
    /// holds at most 1/16 of it
    ///
    /// - a client past its 128: refused, its own doing (no pool pressure → no overload)
    /// - 16 clients fill it; the next refused at once (a queue would hang a wallet behind the
    ///   open streams), `UNAVAILABLE` + pushback, taking no slot
    #[tokio::test]
    async fn one_client_stops_at_its_share_and_only_a_crowd_fills_the_limit() {
        use futures::FutureExt as _;

        let state = Arc::new(AdmissionState::new(WORK_CEILING, WorkShare::default()));
        let parked = Arc::new(stall::Parked::default());
        let guard = |client: &str| {
            let watch = parked.watch(client.parse().expect("address"));
            Guard::new(Answers, Arc::clone(&state), watch, Arc::default(), OriginGate::new())
        };
        let refusal = |response: Response<Guarded<tonic::body::Body>>| {
            let status = Status::from_header_map(response.headers()).expect("Trailers-Only");
            let pushback = response.headers().get("grpc-retry-pushback-ms").cloned();
            (status.code(), status.message().to_owned(), pushback)
        };
        let retry_after = Some(HeaderValue::from_static("250"));
        let refused = |message: &str| (Code::Unavailable, message.to_owned(), retry_after.clone());
        let (at_limit, at_share) =
            (refused("work stream limit reached"), refused("per-client work stream share reached"));
        let mut open = Vec::new();

        let mut greedy = guard("2001:db8::1");
        for _ in 0..128 {
            open.push(greedy.call(request("/open")).await.expect("infallible"));
        }
        let over_share = greedy.call(request("/open")).await.expect("infallible");
        let sibling = guard("2001:db8::ffff").call(request("/open")).await.expect("infallible");
        let pressed_by_one = state.pressed();
        for crowd in 1..16 {
            let mut guard = guard(&format!("2001:db8:0:{crowd}::1"));
            for _ in 0..128 {
                open.push(guard.call(request("/open")).await.expect("infallible"));
            }
        }
        let newcomer = guard("2001:db8:0:16::1").call(request("/open")).now_or_never();

        assert_eq!(refusal(over_share), at_share, "129th from one client refused");
        assert_eq!(refusal(sibling), at_share, "same /64 = same client");
        assert!(!pressed_by_one, "one client at its share: no pressure, no overload");
        let newcomer = newcomer.expect("refused at once, never queued").expect("infallible");
        assert_eq!(refusal(newcomer), at_limit, "16 clients × 128 = the limit");
        let work = (state.work_limit(), state.held());
        assert_eq!(work, (2048, 2048), "at the ceiling, refusals took no slot");
    }
}
