//! Transport-neutral validator link config: [`Endpoint`] (address, [`Auth`], [`Timeouts`],
//! [`LinkLimits`]) → URL + credentials, errors
//!
//! - no probe: reachability = the caller's retry loop (validator down at boot ≠ fatal)

use std::io::Read as _;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::ResponseSizeCap;

#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    #[error("validator address {address} is not host:port: {reason}")]
    Address { address: String, reason: String },

    #[error("cannot read validator cookie {path}: {source}")]
    Cookie { path: String, source: std::io::Error },

    #[error("validator cookie {path} is empty")]
    EmptyCookie { path: String },

    #[error("cannot read indexer TLS {role} {path}: {source}")]
    TlsFile { role: &'static str, path: String, source: std::io::Error },
}

/// Validator JSON-RPC endpoint: `address` = `host:port`
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub address: String,
    pub auth: Auth,
    pub timeouts: Timeouts,
    pub limits: LinkLimits,
}

/// Credentials a validator expects
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    None,
    Cookie(PathBuf),
    DefaultCookie(Vec<PathBuf>),
    Basic { user: String, password: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Credentials {
    NoAuth,
    Basic { user: String, password: String },
}

impl Credentials {
    pub(crate) fn basic(user: &str, password: &str) -> Self {
        Self::Basic { user: user.to_owned(), password: password.to_owned() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Held {
    Unread,
    Known(Credentials),
}

struct Cookie {
    modified: Option<SystemTime>,
    contents: String,
}

type Reading = (PathBuf, std::io::Result<Cookie>);

impl Auth {
    /// - reads each cookie file on the blocking pool
    pub(crate) async fn credentials(&self) -> Result<Credentials, EndpointError> {
        let paths = self.cookie_paths().to_vec();
        if paths.is_empty() {
            return self.sent(Vec::new());
        }
        let read = tokio::task::spawn_blocking({
            let paths = paths.clone();
            move || read_cookies(&paths)
        });
        let unread = |stopped: tokio::task::JoinError| {
            let reading = |path| (path, Err(std::io::Error::other(stopped.to_string())));
            paths.into_iter().map(reading).collect()
        };
        self.sent(read.await.unwrap_or_else(unread))
    }

    /// - reads each cookie file on the calling thread
    pub(crate) fn startable(&self) -> Result<(), EndpointError> {
        match self.before_challenge() {
            Held::Known(_) => Ok(()),
            Held::Unread => start_verdict(self.sent(read_cookies(self.cookie_paths()))),
        }
    }

    pub(crate) fn before_challenge(&self) -> Held {
        match self {
            Self::DefaultCookie(_) => Held::Known(Credentials::NoAuth),
            fixed_or_explicit => fixed_or_explicit.fixed().map_or(Held::Unread, Held::Known),
        }
    }

    fn fixed(&self) -> Option<Credentials> {
        match self {
            Self::None => Some(Credentials::NoAuth),
            Self::Basic { user, password } => Some(Credentials::basic(user, password)),
            Self::Cookie(_) | Self::DefaultCookie(_) => None,
        }
    }

    fn cookie_paths(&self) -> &[PathBuf] {
        match self {
            Self::None | Self::Basic { .. } => &[],
            Self::Cookie(path) => std::slice::from_ref(path),
            Self::DefaultCookie(candidates) => candidates,
        }
    }

    fn sent(&self, readings: Vec<Reading>) -> Result<Credentials, EndpointError> {
        if let Some(fixed) = self.fixed() {
            return Ok(fixed);
        }
        let absent_is_no_auth = matches!(self, Self::DefaultCookie(_));
        let present = readings
            .into_iter()
            .filter(|(_, cookie)| !(absent_is_no_auth && cookie.as_ref().is_err_and(absent)))
            .map(|(path, cookie)| match cookie {
                Ok(cookie) => Ok((path, cookie)),
                Err(source) => {
                    Err(EndpointError::Cookie { path: path.display().to_string(), source })
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let newest = present.into_iter().rev().max_by_key(|(_, cookie)| cookie.modified);
        newest.map_or(Ok(Credentials::NoAuth), |(path, cookie)| basic_auth(&path, &cookie))
    }
}

fn absent(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

fn basic_auth(path: &Path, cookie: &Cookie) -> Result<Credentials, EndpointError> {
    let token = cookie.contents.trim();
    let token = token.strip_prefix("__cookie__:").unwrap_or(token);
    if token.is_empty() {
        return Err(EndpointError::EmptyCookie { path: path.display().to_string() });
    }
    Ok(Credentials::basic("__cookie__", token))
}

fn start_verdict(read: Result<Credentials, EndpointError>) -> Result<(), EndpointError> {
    match read {
        Err(EndpointError::Cookie { source, .. }) if absent(&source) => Ok(()),
        Err(EndpointError::EmptyCookie { .. }) => Ok(()),
        settled => settled.map(drop),
    }
}

/// - reads each of `paths`
fn read_cookies(paths: &[PathBuf]) -> Vec<Reading> {
    paths.iter().map(|path| (path.clone(), read_cookie(path))).collect()
}

/// - reads `path`
fn read_cookie(path: &Path) -> std::io::Result<Cookie> {
    let mut file = std::fs::File::open(path)?;
    let modified = file.metadata()?.modified().ok();
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(Cookie { modified, contents })
}

/// - `read` = silence, never total duration (multi-MB block over a slow link must finish)
/// - Total deadline below a block's transfer time = retry livelock
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    pub read: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { connect: Duration::from_secs(2), read: Duration::from_secs(30) }
    }
}

/// One validator's link: idle connections kept, largest reply buffered
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkLimits {
    pub max_connections: NonZeroU32,
    pub response_size_cap: ResponseSizeCap,
}

/// 32 connections (zebrad admits 100 across every client), full reply cap
impl Default for LinkLimits {
    fn default() -> Self {
        Self {
            max_connections: const { NonZeroU32::new(32).expect("32 is non-zero") },
            response_size_cap: ResponseSizeCap::MAX,
        }
    }
}

/// `http://{address}`, hostname kept, not pre-resolved (reqwest resolves per connection →
/// follows a validator whose IP changes, e.g. a restarted pod)
pub(crate) fn validator_url(address: &str) -> Result<String, EndpointError> {
    Ok(parsed(address)?.as_str().trim_end_matches('/').to_string())
}

fn parsed(address: &str) -> Result<reqwest::Url, EndpointError> {
    let invalid = |reason: &str| EndpointError::Address {
        address: address.to_string(),
        reason: reason.to_string(),
    };
    let port_given = address.rsplit_once(':').is_some_and(|(_, port)| port.parse::<u16>().is_ok());
    if !port_given {
        return Err(invalid("missing or non-numeric port"));
    }
    let url =
        reqwest::Url::parse(&format!("http://{address}")).map_err(|e| invalid(&e.to_string()))?;
    if url.path() != "/" || url.query().is_some() {
        return Err(invalid("unexpected path or query"));
    }
    Ok(url)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Loopback,
    Private,
    Public,
    Named,
}

impl Reach {
    pub fn of_ip(ip: IpAddr) -> Self {
        match ip.to_canonical() {
            ip if ip.is_loopback() => Self::Loopback,
            IpAddr::V4(ip) if ip.is_private() => Self::Private,
            IpAddr::V6(ip) if ip.is_unique_local() => Self::Private,
            _ => Self::Public,
        }
    }

    pub fn of_validator(address: &str) -> Result<Self, EndpointError> {
        let url = parsed(address)?;
        let host = url.host_str().unwrap_or_default();
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let named = || match host.trim_end_matches('.') {
            "localhost" => Self::Loopback,
            _ => Self::Named,
        };
        Ok(host.parse().map_or_else(|_| named(), Self::of_ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(contents: &str, age_secs: u64) -> std::io::Result<Cookie> {
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 - age_secs);
        Ok(Cookie { modified: Some(modified), contents: contents.to_owned() })
    }

    fn failed(kind: std::io::ErrorKind) -> std::io::Result<Cookie> {
        Err(std::io::Error::from(kind))
    }

    fn basic() -> Auth {
        Auth::Basic { user: "user".to_owned(), password: "pass".to_owned() }
    }

    fn outcome(sent: Result<Credentials, EndpointError>) -> String {
        match sent {
            Ok(Credentials::NoAuth) => "no auth".to_owned(),
            Ok(Credentials::Basic { user, password }) => format!("{user}:{password}"),
            Err(EndpointError::Cookie { path, .. }) => format!("unreadable {path}"),
            Err(EndpointError::EmptyCookie { path }) => format!("empty {path}"),
            Err(other) => format!("{other}"),
        }
    }

    #[test]
    fn credentials_sent_follow_the_cookies_read() {
        use std::io::ErrorKind::{NotFound, PermissionDenied};
        let explicit = Auth::Cookie("zebra".into());
        let default = Auth::DefaultCookie(vec!["zebra".into(), "zakura".into()]);
        #[rustfmt::skip]
        let cases = [
            ("explicit, prefixed",           &explicit, vec![written("__cookie__:one", 0)],                "__cookie__:one"),
            ("explicit, bare and untrimmed", &explicit, vec![written("  one  \n", 0)],                     "__cookie__:one"),
            ("explicit, absent",             &explicit, vec![failed(NotFound)],                            "unreadable zebra"),
            ("explicit, empty",              &explicit, vec![written("", 0)],                              "empty zebra"),
            ("default, neither",             &default,  vec![failed(NotFound), failed(NotFound)],          "no auth"),
            ("default, zebra alone",         &default,  vec![written("one", 0), failed(NotFound)],         "__cookie__:one"),
            ("default, zakura newer",        &default,  vec![written("one", 9), written("two", 0)],        "__cookie__:two"),
            ("default, same age: the first", &default,  vec![written("one", 5), written("two", 5)],        "__cookie__:one"),
            ("default, one unreadable",      &default,  vec![written("one", 0), failed(PermissionDenied)], "unreadable zakura"),
            ("default, the newest empty",    &default,  vec![written("one", 9), written("", 0)],           "empty zakura"),
        ];
        for (case, auth, cookies, expected) in cases {
            let readings = auth.cookie_paths().iter().cloned().zip(cookies).collect();
            assert_eq!(outcome(auth.sent(readings)), expected, "{case}");
        }
    }

    #[test]
    fn only_an_explicit_cookie_is_unknown_before_the_first_request() {
        let cases = [
            ("none: no auth", Auth::None, Held::Known(Credentials::NoAuth)),
            ("basic: the pair", basic(), Held::Known(Credentials::basic("user", "pass"))),
            ("explicit cookie: to be read", Auth::Cookie("zebra".into()), Held::Unread),
            (
                "default cookie: no auth until refused",
                Auth::DefaultCookie(vec![]),
                Held::Known(Credentials::NoAuth),
            ),
        ];
        for (case, auth, expected) in cases {
            assert_eq!(auth.before_challenge(), expected, "{case}");
        }
    }

    #[test]
    fn a_validator_address_reaches_what_its_parsed_host_names() {
        #[rustfmt::skip]
        let cases = [
            ("127.0.0.1:8232",            Reach::Loopback),
            ("127.1:8232",                Reach::Loopback),
            ("user@127.0.0.1:8232",       Reach::Loopback),
            ("[::1]:8232",                Reach::Loopback),
            ("[::ffff:127.0.0.1]:8232",   Reach::Loopback),
            ("localhost:8232",            Reach::Loopback),
            ("LOCALHOST.:8232",           Reach::Loopback),
            ("10.0.0.5:8232",             Reach::Private),
            ("192.168.1.9:8232",          Reach::Private),
            ("[fd00::1]:8232",            Reach::Private),
            ("8.8.8.8:8232",              Reach::Public),
            ("[2001:4860::8888]:8232",    Reach::Public),
            ("zebra-eu:8232",             Reach::Named),
            ("localhost.example:8232",    Reach::Named),
        ];
        for (address, expected) in cases {
            let reach = Reach::of_validator(address).map_err(|error| error.to_string());
            assert_eq!(reach, Ok(expected), "{address}");
        }
        let unparsed = Reach::of_validator("zebrad");
        assert!(matches!(unparsed, Err(EndpointError::Address { .. })), "no port: {unparsed:?}");
    }

    #[test]
    fn only_a_cookie_that_can_never_be_read_refuses_the_start() {
        use std::io::ErrorKind::{IsADirectory, NotFound, PermissionDenied};
        let unreadable = |kind| {
            let source = std::io::Error::from(kind);
            Err(EndpointError::Cookie { path: "zebra".to_owned(), source })
        };
        let cases = [
            ("no auth", Ok(Credentials::NoAuth), true),
            ("a cookie", Ok(Credentials::basic("__cookie__", "one")), true),
            ("not yet written", unreadable(NotFound), true),
            (
                "caught mid-write",
                Err(EndpointError::EmptyCookie { path: "zebra".to_owned() }),
                true,
            ),
            ("another user's", unreadable(PermissionDenied), false),
            ("a directory", unreadable(IsADirectory), false),
        ];
        for (case, read, starts) in cases {
            assert_eq!(start_verdict(read).is_ok(), starts, "{case}");
        }
    }

    #[tokio::test]
    async fn the_newer_default_cookie_on_disk_is_the_one_sent() {
        let dir = tempfile::tempdir().expect("the OS temp directory is writable");
        let [zebra, zakura] = ["zebra.cookie", "zakura.cookie"].map(|name| dir.path().join(name));
        let default = Auth::DefaultCookie(vec![zebra.clone(), zakura.clone()]);
        let started = |cookie: &Path, token: &str, at_secs: u64| {
            std::fs::write(cookie, token)?;
            let at = SystemTime::UNIX_EPOCH + Duration::from_secs(at_secs);
            std::fs::File::options().write(true).open(cookie)?.set_modified(at)
        };

        assert_eq!(outcome(default.credentials().await), "no auth", "neither started");
        started(&zebra, "__cookie__:one", 100).expect("the temp directory is writable");
        assert_eq!(outcome(default.credentials().await), "__cookie__:one", "zebrad started");
        started(&zakura, "__cookie__:two", 200).expect("the temp directory is writable");
        assert_eq!(outcome(default.credentials().await), "__cookie__:two", "zakurad started later");
        started(&zebra, "__cookie__:three", 300).expect("the temp directory is writable");
        assert_eq!(outcome(default.credentials().await), "__cookie__:three", "zebrad restarted");
    }

    #[test]
    fn validator_url_keeps_the_host_and_rejects_non_host_port() {
        for (address, url) in [
            ("127.0.0.1:8232", "http://127.0.0.1:8232"),
            ("zebrad:18232", "http://zebrad:18232"),
            ("[::1]:8232", "http://[::1]:8232"),
        ] {
            assert_eq!(
                validator_url(address).map_err(|e| e.to_string()),
                Ok(url.into()),
                "{address}"
            );
        }
        for address in ["zebrad", "zebrad:", "zebrad:port", "not a host:8232", "zebrad:8232/path"] {
            let url = validator_url(address);
            assert!(matches!(url, Err(EndpointError::Address { .. })), "{address}: {url:?}");
        }
    }
}
