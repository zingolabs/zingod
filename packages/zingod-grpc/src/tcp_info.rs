//! Kernel TCP state of the listener's connections → wallets waiting on this node's uplink
//!
//! - one dump per [`INTERVAL`] (ESTABLISHED, local port = listener's), keyed by socket cookie
//! - waiting = sending held by the network (cwnd), not the wallet's window nor our buffer
//! - queue delay = smoothed RTT above the connection's own lifetime lowest (its queue-free path)
//! - waiting counts toward overload only once most connections queue or lose for 30 s ([`Congestion`])

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use netlink_packet_core::{
    NetlinkHeader, NetlinkMessage, NetlinkPayload, NLM_F_DUMP, NLM_F_REQUEST,
};
use netlink_packet_sock_diag::constants::{AF_INET, AF_INET6, IPPROTO_TCP};
use netlink_packet_sock_diag::inet::nlas::{Nla, TcpInfo};
use netlink_packet_sock_diag::inet::{
    ExtensionFlags, InetRequest, InetResponse, SocketId, StateFlags,
};
use netlink_packet_sock_diag::SockDiagMessage;
use netlink_sys::protocols::NETLINK_SOCK_DIAG;
use netlink_sys::{Socket, SocketAddr};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::admission::AdmissionState;
use crate::metrics;
use crate::proxy_protocol::TrustedProxies;

const INTERVAL: Duration = Duration::from_secs(10);

/// Shared congestion = median queue delay over sending connections, or resent share
/// - bench: full uplink 200-470 ms, fading to 51-85 ms past ~5 min (new connections' floors hold
///   the queue); healthy 12-20 ms; drops 26 %
/// - 500 wallets at 1 Mbit each reached 88-97 ms once for ~106 s: the two overlap (kept at 50 ms:
///   a long saturation stays seen)
const CONGESTED_QUEUE_DELAY: Duration = Duration::from_millis(50);
const CONGESTED_RETRANSMITTED_PER_MILLE: u64 = 20;
/// Fewer senders = no median worth the name (one slow phone on a quiet node ≠ a full uplink)
const MIN_SENDERS: usize = 10;
/// Congested this long without a break before waiting counts
/// - bench: full uplink 8-12 min unbroken; slow wallets' ~1 s spikes last seconds
const SUSTAINED: Duration = Duration::from_secs(30);

/// Large enough for one kernel dump chunk (it fills at most the reader's buffer)
const RECEIVE_BUFFER: usize = 64 * 1024;

/// One socket's cumulative counters (`waiting` = µs held by the network; bytes)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Totals {
    waiting: u64,
    sent: u64,
    retransmitted: u64,
}

/// One socket at dump time (`rtt` = smoothed, `min_rtt` = kernel's over the last 300 s; µs)
/// - `cookie` = the kernel's id for the socket, never reused (a 4-tuple is: NAT ports recycle)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sample {
    cookie: u64,
    totals: Totals,
    rtt: u32,
    min_rtt: u32,
}

/// Since the previous dump, summed over sockets (`queue_delays` = one per socket that sent)
#[derive(Debug, Default, PartialEq)]
struct Window {
    elapsed: Duration,
    waiting: Duration,
    sent: u64,
    retransmitted: u64,
    queue_delays: Vec<Duration>,
}

impl Window {
    /// (median queue delay, retransmitted ‰): the uplink's own symptoms, shared by most connections
    fn symptoms(&self) -> (Duration, u64) {
        let mut delays = self.queue_delays.clone();
        delays.sort_unstable();
        let median = delays.get(delays.len() / 2).copied().unwrap_or_default();
        let retransmitted = self.retransmitted.saturating_mul(1000) / self.sent.max(1);
        (median, retransmitted)
    }

    /// This window alone ([`Congestion`] = whether it has lasted)
    fn congested(&self) -> bool {
        let (median, retransmitted) = self.symptoms();
        self.queue_delays.len() >= MIN_SENDERS
            && (median >= CONGESTED_QUEUE_DELAY
                || retransmitted >= CONGESTED_RETRANSMITTED_PER_MILLE)
    }
}

/// Start of the current unbroken congested run (`None` = clear at the last dump)
#[derive(Debug, Default)]
struct Congestion {
    since: Option<Instant>,
}

impl Congestion {
    /// Sustained = congested for [`SUSTAINED`] without a clear dump between
    fn step(&mut self, at: Instant, congested: bool) -> bool {
        self.since = congested.then(|| self.since.unwrap_or(at));
        self.since.is_some_and(|since| at.saturating_duration_since(since) >= SUSTAINED)
    }
}

/// One open socket between dumps: its counters + lifetime lowest RTT (µs)
/// - kernel `min_rtt` windows over `tcp_min_rtt_wlen` (300 s): a standing queue would lift it
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Seen {
    totals: Totals,
    lowest: u32,
}

/// Previous dump's time + each open socket by cookie (closed ones drop out; `None` = no dump yet)
#[derive(Debug, Default)]
struct Tracker {
    previous: Option<(Instant, HashMap<u64, Seen>)>,
}

impl Tracker {
    /// `None` on the first dump (totals since each socket opened ≠ this window)
    fn step(&mut self, at: Instant, samples: &[Sample]) -> Option<Window> {
        let previous = self.previous.take();
        let mut window = previous.as_ref().map(|(then, _)| Window {
            elapsed: at.saturating_duration_since(*then),
            ..Window::default()
        });
        let mut current = HashMap::with_capacity(samples.len());
        for sample in samples {
            let before = previous.as_ref().and_then(|(_, seen)| seen.get(&sample.cookie)).copied();
            let floor = before.map_or(sample.min_rtt, |seen| seen.lowest.min(sample.min_rtt));
            current.insert(sample.cookie, Seen { totals: sample.totals, lowest: floor });
            let Some(window) = window.as_mut() else { continue };
            // unseen = opened since the last dump: all of its totals fall in this window
            let before = before.map(|seen| seen.totals).unwrap_or_default();
            let (now, delta) = (sample.totals, |now: u64, then: u64| now.saturating_sub(then));
            let sent = delta(now.sent, before.sent);
            window.waiting += micros(delta(now.waiting, before.waiting));
            window.sent += sent;
            window.retransmitted += delta(now.retransmitted, before.retransmitted);
            // idle socket's srtt = stale (last measured whenever it last sent)
            if sent > 0 {
                window.queue_delays.push(micros(u64::from(sample.rtt.saturating_sub(floor))));
            }
        }
        self.previous = Some((at, current));
        window
    }
}

fn micros(us: u64) -> Duration {
    Duration::from_micros(us)
}

/// Until `cancel`: one dump per [`INTERVAL`] → the overload latch's network part
///
/// - stops (warned) once the kernel refuses a dump: network part absent, serving unaffected
pub(crate) async fn sample(
    port: u16,
    proxies: TrustedProxies,
    admission: Arc<AdmissionState>,
    cancel: CancellationToken,
) {
    let (mut tracker, mut congestion, mut congested) =
        (Tracker::default(), Congestion::default(), false);
    let mut ticks = tokio::time::interval(INTERVAL);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = ticks.tick() => {}
        }
        let proxies = proxies.clone();
        let samples = match zingod_sync::blocking(move || dump(port, &proxies)).await {
            Ok(samples) => samples,
            Err(error) => {
                admission.network_waiting(0.0);
                return warn!(%error, "Kernel TCP stats unavailable, uplink not watched");
            }
        };
        let now = Instant::now();
        let Some(window) = tracker.step(now, &samples) else { continue };
        let sustained = congestion.step(now, window.congested());
        let (median, retransmitted) = window.symptoms();
        let waiting = window.waiting.div_duration_f64(window.elapsed.max(Duration::from_nanos(1)));
        admission.network_waiting(if sustained { waiting } else { 0.0 });
        metrics::uplink_window(waiting, median, retransmitted, sustained);
        if sustained != congested {
            congested = sustained;
            let (queue_delay_ms, retransmitted_percent) =
                (median.as_millis(), retransmitted as f64 / 10.0);
            match congested {
                true => warn!(queue_delay_ms, retransmitted_percent, "Uplink congested"),
                false => info!(queue_delay_ms, retransmitted_percent, "Uplink clear"),
            }
        }
    }
}

/// Every ESTABLISHED socket on local `port`, both families (a trusted proxy's = its hop, skipped)
fn dump(port: u16, proxies: &TrustedProxies) -> io::Result<Vec<Sample>> {
    let mut socket = Socket::new(NETLINK_SOCK_DIAG)?;
    socket.bind_auto()?;
    socket.connect(&SocketAddr::new(0, 0))?;
    let mut samples = Vec::new();
    for (family, id) in [(AF_INET, SocketId::new_v4()), (AF_INET6, SocketId::new_v6())] {
        request(&socket, family, id)?;
        receive(&socket, |response| {
            let id = &response.header.socket_id;
            // canonical = as transport.rs judges the peer (dual-stack v4 = ::ffff:a.b.c.d)
            if id.source_port == port && !proxies.trusts(id.destination_address.to_canonical()) {
                samples.extend(decode_sample(id.cookie, &response.nlas));
            }
        })?;
    }
    Ok(samples)
}

fn request(socket: &Socket, family: u8, socket_id: SocketId) -> io::Result<()> {
    let mut header = NetlinkHeader::default();
    header.flags = NLM_F_REQUEST | NLM_F_DUMP;
    let mut message = NetlinkMessage::new(
        header,
        SockDiagMessage::InetRequest(InetRequest {
            family,
            protocol: IPPROTO_TCP,
            extensions: ExtensionFlags::INFO,
            states: StateFlags::ESTABLISHED,
            socket_id,
        })
        .into(),
    );
    message.finalize();
    let mut bytes = vec![0; message.buffer_len()];
    message.serialize(&mut bytes);
    socket.send(&bytes, 0)?;
    Ok(())
}

/// Each response of one dump to `each`, until the kernel's `Done`
fn receive(socket: &Socket, mut each: impl FnMut(&InetResponse)) -> io::Result<()> {
    let mut buffer = vec![0; RECEIVE_BUFFER];
    loop {
        let received = socket.recv(&mut &mut buffer[..], 0)?;
        let mut chunk = buffer.get(..received).unwrap_or_default();
        while !chunk.is_empty() {
            let message = NetlinkMessage::<SockDiagMessage>::deserialize(chunk)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            match message.payload {
                NetlinkPayload::Done(_) => return Ok(()),
                NetlinkPayload::Error(error) if error.code.is_some() => return Err(error.to_io()),
                NetlinkPayload::InnerMessage(SockDiagMessage::InetResponse(response)) => {
                    each(&response);
                }
                _ => {}
            }
            let length = usize::try_from(message.header.length).unwrap_or(usize::MAX);
            chunk = chunk.get(length.max(1)..).unwrap_or_default();
        }
    }
}

/// `INET_DIAG_INFO` → one sample (`None` = the kernel attached no `tcp_info`)
fn decode_sample(cookie: [u8; 8], nlas: &[Nla]) -> Option<Sample> {
    let info: &TcpInfo = nlas.iter().find_map(|nla| match nla {
        Nla::TcpInfo(info) => Some(info),
        _ => None,
    })?;
    Some(Sample {
        cookie: u64::from_ne_bytes(cookie),
        totals: Totals {
            // busy_time = Σ every chrono state (kernel tcp_get_info_chrono_stats): rest = cwnd's
            waiting: info
                .busy_time
                .saturating_sub(info.rwnd_limited)
                .saturating_sub(info.sndbuf_limited),
            sent: info.bytes_sent,
            retransmitted: info.bytes_retrans,
        },
        rtt: info.rtt,
        min_rtt: info.min_rtt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Window = per-socket deltas; a new socket counts whole, a closed one drops out
    #[test]
    fn windows_difference_each_socket_and_count_a_new_one_from_its_open() {
        let totals = |waiting, sent, retransmitted| Totals { waiting, sent, retransmitted };
        let sample = |cookie, totals, rtt, min_rtt| Sample { cookie, totals, rtt, min_rtt };
        let start = Instant::now();
        let mut tracker = Tracker::default();

        let first = [
            sample(1, totals(4_000, 9_000, 0), 40_000, 20_000),
            sample(2, totals(0, 100, 0), 30_000, 30_000),
        ];
        assert_eq!(tracker.step(start, &first), None, "first dump = baselines only");

        let second = [
            // 1: +5 ms waiting, +4000 B (+400 retransmitted)
            sample(1, totals(9_000, 13_000, 400), 45_000, 20_000),
            // 2: sent nothing → no queue delay sample (stale srtt)
            sample(2, totals(0, 100, 0), 90_000, 30_000),
            // 3: new since the last dump → its totals whole
            sample(3, totals(2_000, 600, 0), 12_000, 10_000),
        ];
        assert_eq!(
            tracker.step(start + INTERVAL, &second),
            Some(Window {
                elapsed: INTERVAL,
                waiting: Duration::from_millis(7),
                sent: 4_600,
                retransmitted: 400,
                queue_delays: vec![Duration::from_millis(25), Duration::from_millis(2)],
            }),
        );

        // 1 closed (absent); 3 reopened under a new cookie never inherits 1's totals
        let third = [sample(4, totals(1_000, 50, 0), 10_000, 10_000)];
        let window = tracker.step(start + 2 * INTERVAL, &third).expect("after a dump");
        assert_eq!((window.waiting, window.sent), (Duration::from_millis(1), 50));
    }

    /// One window: most of ≥ 10 senders queueing, or losing; slow wallets or a quiet node never
    #[test]
    fn only_a_queue_or_loss_shared_by_most_of_ten_senders_is_congestion() {
        let ms = Duration::from_millis;
        // case, queue delays of sending connections, bytes sent, retransmitted, congested
        #[rustfmt::skip]
        let cases = [
            ("healthy",                      vec![ms(15); 10],                               1_000, 2,   false),
            ("slow wallets: a high tail",    [vec![ms(15); 8], vec![ms(900); 2]].concat(),  1_000, 1,   false),
            ("uplink full, provider queues", [vec![ms(40); 4], vec![ms(216); 6]].concat(),  1_000, 4,   true),
            ("uplink full, provider drops",  vec![ms(11); 10],                               1_000, 260, true),
            ("quiet node, one slow phone",   vec![ms(900)],                                  1_000, 1,   false),
            ("quiet node, three losing",     vec![ms(11); 3],                                1_000, 260, false),
            ("idle: nothing sent",           vec![],                                         0,     0,   false),
        ];
        for (case, queue_delays, sent, retransmitted, want) in cases {
            let window = Window { queue_delays, sent, retransmitted, ..Window::default() };

            assert_eq!(window.congested(), want, "{case}: {:?}", window.symptoms());
        }
    }

    /// Waiting counts from 30 s of unbroken congestion; one clear dump restarts the clock
    #[test]
    fn congestion_counts_once_unbroken_for_thirty_seconds() {
        let start = Instant::now();
        let mut congestion = Congestion::default();
        // dump every 10 s: congested?
        let dumps = [true, true, true, true, true, false, true, true, true, true];

        let sustained: Vec<bool> = (0u32..)
            .zip(dumps)
            .map(|(n, congested)| congestion.step(start + INTERVAL * n, congested))
            .collect();

        #[rustfmt::skip]
        let want = [false, false, false, true, true, false, false, false, false, true];
        assert_eq!(sustained, want, "0-40 s congested, 50 s clear, 60-90 s congested");
    }

    /// A standing queue outlasting the kernel's 300 s window stays measured: the floor = lifetime
    #[test]
    fn queue_delay_is_measured_against_the_connections_own_lifetime_lowest() {
        let (start, ms) = (Instant::now(), |ms: u32| ms * 1_000);
        let socket = |cookie, rtt, min_rtt, sent| Sample {
            cookie,
            totals: Totals { waiting: 0, sent, retransmitted: 0 },
            rtt: ms(rtt),
            min_rtt: ms(min_rtt),
        };
        let mut tracker = Tracker::default();

        tracker.step(start, &[socket(1, 20, 20, 0)]);
        // 10 min into a 200 ms standing queue: the kernel's min (300 s window) now includes it
        let later = start + Duration::from_secs(600);
        let window = tracker.step(later, &[socket(1, 220, 210, 1_000), socket(2, 220, 210, 1_000)]);
        assert_eq!(
            window.expect("2nd dump").queue_delays,
            [Duration::from_millis(200), Duration::from_millis(10)],
            "open before the queue: 220 - its own 20; opened during it: no clean floor of its own",
        );
    }

    /// Real kernel dump: a loopback connection on the listener's port, its bytes counted
    #[tokio::test]
    async fn a_dump_finds_the_listeners_connection_with_its_sent_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("bound").port();
        let mut client =
            tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("connect");
        let (mut served, _) = listener.accept().await.expect("accept");
        served.write_all(&[7; 10_000]).await.expect("write");
        client.read_exact(&mut [0; 10_000]).await.expect("read");

        let none_trusted = TrustedProxies::default();
        let samples = dump(port, &none_trusted).expect("sock_diag dump");
        // client's own socket (local port = ephemeral) excluded; the served one = sender
        assert_eq!(samples.len(), 1, "{samples:?}");
        assert!(samples.iter().all(|sample| sample.totals.sent >= 10_000), "{samples:?}");

        let loopback_trusted = TrustedProxies::new(vec!["127.0.0.0/8".parse().expect("net")]);
        assert_eq!(dump(port, &loopback_trusted).expect("dump"), [], "proxy hop skipped");
    }
}
