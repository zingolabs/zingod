//! TLS termination for the gRPC listener: rustls + aws-lc-rs, cert files reloaded on change
//!
//! - Provider passed per `ServerConfig`, never installed process-wide (two providers in one
//!   graph = rustls refuses to pick: zingod 0.5.0's startup panic, #1360)
//! - Renewal = new files on disk
//! - [`Tls::reload`] swaps them in whole, a bad pair keeps the old
//! - [`Tls::self_signed`]: pair generated once into a dir, then served + reloaded as any other

use std::{
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use arc_swap::ArcSwap;
use rcgen::{CertificateParams, DnType, KeyPair};
use rustls::{
    crypto::CryptoProvider,
    pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
    ServerConfig,
};
use serde::{Deserialize, Serialize};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// How often the cert files are checked for a renewal
const RELOAD_EVERY: Duration = Duration::from_secs(60);

/// PEM files: the certificate chain (leaf first) and its private key (PKCS#8, PKCS#1 or SEC1)
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TlsFiles {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

/// Why a certificate pair could not be loaded
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("{path}: {source}")]
    Pem { path: PathBuf, source: rustls::pki_types::pem::Error },
    #[error("{0}: no certificate in the file")]
    NoCertificate(PathBuf),
    #[error("{path}: {source}")]
    Key { path: PathBuf, source: rustls::Error },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {source}")]
    SelfSigned { path: PathBuf, source: rcgen::Error },
    #[error("{0}: certificate with no key.pem beside it; delete it (a new pair is generated) or restore the key")]
    OrphanCertificate(PathBuf),
}

/// Self-signed pair's file names under its directory
const SELF_SIGNED_CERT: &str = "cert.pem";
const SELF_SIGNED_KEY: &str = "key.pem";

/// The gRPC listener's TLS: one acceptor, its certificate swappable underneath
pub struct Tls {
    acceptor: TlsAcceptor,
    certs: Arc<Certs>,
}

impl Tls {
    /// Boot-time load: a missing or mismatched pair fails startup, never serves plaintext
    pub fn load(files: TlsFiles) -> Result<Self, TlsError> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let current = load(&files, &provider)?;
        let certs = Arc::new(Certs {
            seen: std::sync::Mutex::new(stamp(&files).ok()),
            current: ArcSwap::from_pointee(current),
            files,
            provider: Arc::clone(&provider),
        });
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("aws-lc-rs default provider: a suite + kx group for every safe default version")
            .with_no_client_auth()
            .with_cert_resolver(Arc::clone(&certs) as Arc<dyn ResolvesServerCert>);
        // gRPC = h2 (preferred), HTTP/1.1 = load-balancer health monitors (`health.rs`)
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Self { acceptor: TlsAcceptor::from(Arc::new(config)), certs })
    }

    /// `dir/cert.pem` + `dir/key.pem`, generated on first boot (ECDSA P-256, never expires),
    /// then loaded as [`Tls::load`] would
    ///
    /// - SANs: `localhost`, both loopbacks, `listen`'s address when specific
    /// - key alone = certificate re-issued for it (delete `cert.pem` to change the SANs)
    /// - certificate alone = refused ([`TlsError::OrphanCertificate`])
    pub fn self_signed(dir: &Path, listen: SocketAddr) -> Result<Self, TlsError> {
        let files =
            TlsFiles { cert_path: dir.join(SELF_SIGNED_CERT), key_path: dir.join(SELF_SIGNED_KEY) };
        match (files.cert_path.exists(), files.key_path.exists()) {
            (true, true) => {}
            (true, false) => return Err(TlsError::OrphanCertificate(files.cert_path)),
            (false, have_key) => generate(&files, listen, have_key)?,
        }
        Self::load(files)
    }

    /// `sha256:<hex>` of the leaf certificate served now (pinned client-side)
    pub fn leaf_sha256(&self) -> String {
        let leaf = &self.certs.current.load().cert[0];
        let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, leaf.as_ref());
        format!("sha256:{}", const_hex::encode(digest.as_ref()))
    }

    pub(crate) fn acceptor(&self) -> TlsAcceptor {
        self.acceptor.clone()
    }

    /// Checks the files every [`RELOAD_EVERY`] until `cancel`
    pub(crate) fn reload(
        &self,
        cancel: CancellationToken,
    ) -> impl Future<Output = ()> + Send + 'static {
        let certs = Arc::clone(&self.certs);
        async move {
            let mut tick = tokio::time::interval(RELOAD_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    _ = tick.tick() => certs.reload_if_changed(),
                }
            }
        }
    }
}

/// Every handshake's certificate source
#[derive(Debug)]
struct Certs {
    files: TlsFiles,
    provider: Arc<CryptoProvider>,
    current: ArcSwap<CertifiedKey>,
    /// Files' stamp at the last load attempt (a failed one too: no retry until they change)
    seen: std::sync::Mutex<Option<Stamp>>,
}

impl ResolvesServerCert for Certs {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current.load_full())
    }
}

impl Certs {
    fn reload_if_changed(&self) {
        let Ok(now) = stamp(&self.files) else { return };
        let mut seen = self.seen.lock().expect("seen-certs mutex poisoned");
        if *seen == Some(now) {
            return;
        }
        *seen = Some(now);
        match load(&self.files, &self.provider) {
            Ok(renewed) => {
                self.current.store(Arc::new(renewed));
                info!(cert = %self.files.cert_path.display(), "TLS certificate reloaded");
            }
            // e.g. cert written, key not yet: retried when the key lands (new stamp)
            Err(error) => warn!(%error, "TLS certificate reload failed; serving the previous one"),
        }
    }
}

/// (mtime, len) of cert then key
type Stamp = [(SystemTime, u64); 2];

fn stamp(files: &TlsFiles) -> io::Result<Stamp> {
    let of = |path: &Path| -> io::Result<(SystemTime, u64)> {
        let meta = std::fs::metadata(path)?;
        Ok((meta.modified()?, meta.len()))
    };
    Ok([of(&files.cert_path)?, of(&files.key_path)?])
}

fn pem(path: &Path) -> impl FnOnce(rustls::pki_types::pem::Error) -> TlsError + '_ {
    move |source| TlsError::Pem { path: path.to_owned(), source }
}

fn load(files: &TlsFiles, provider: &CryptoProvider) -> Result<CertifiedKey, TlsError> {
    let chain = CertificateDer::pem_file_iter(&files.cert_path)
        .map_err(pem(&files.cert_path))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(pem(&files.cert_path))?;
    if chain.is_empty() {
        return Err(TlsError::NoCertificate(files.cert_path.clone()));
    }
    let key = PrivateKeyDer::from_pem_file(&files.key_path).map_err(pem(&files.key_path))?;
    let key_error = |source| TlsError::Key { path: files.key_path.clone(), source };
    let signing = provider.key_provider.load_private_key(key).map_err(key_error)?;
    let certified = CertifiedKey::new(chain, signing);
    // a key from another pair = every handshake fails: refuse it here instead
    certified.keys_match().map_err(key_error)?;
    Ok(certified)
}

/// Writes `files.cert_path` (+ `files.key_path` unless `have_key`) as a fresh self-signed pair
fn generate(files: &TlsFiles, listen: SocketAddr, have_key: bool) -> Result<(), TlsError> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| TlsError::Io { path, source }
    };
    let self_signed = |path: &Path| {
        let path = path.to_owned();
        move |source| TlsError::SelfSigned { path, source }
    };
    let dir = files.cert_path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let key = if have_key {
        let pem = std::fs::read_to_string(&files.key_path).map_err(io(&files.key_path))?;
        KeyPair::from_pem(&pem).map_err(self_signed(&files.key_path))?
    } else {
        KeyPair::generate().map_err(self_signed(&files.key_path))?
    };
    let mut params =
        CertificateParams::new(subject_alt_names(listen)).map_err(self_signed(&files.cert_path))?;
    params.distinguished_name.push(DnType::CommonName, "zingod");
    let cert = params.self_signed(&key).map_err(self_signed(&files.cert_path))?;
    if !have_key {
        write_private(&files.key_path, key.serialize_pem().as_bytes())
            .map_err(io(&files.key_path))?;
    }
    std::fs::write(&files.cert_path, cert.pem()).map_err(io(&files.cert_path))?;
    info!(
        cert = %files.cert_path.display(),
        reused_key = have_key,
        "self-signed TLS certificate generated"
    );
    Ok(())
}

/// `localhost`, both loopbacks, + `listen`'s own address unless unspecified (0.0.0.0 / ::)
fn subject_alt_names(listen: SocketAddr) -> Vec<String> {
    let mut sans = vec!["localhost".to_owned(), "127.0.0.1".to_owned(), "::1".to_owned()];
    let ip: IpAddr = listen.ip();
    if !ip.is_unspecified() && !ip.is_loopback() {
        sans.push(ip.to_string());
    }
    sans
}

/// `create_new` (never overwrites a key) at mode 0600 where modes exist
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{pki_types::ServerName, ClientConfig, RootCertStore};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_rustls::TlsConnector;

    /// - serves its pair over a real h2 handshake
    /// - renewal swaps in without a restart
    /// - mismatched renewal keeps the last good pair, mismatched boot pair refuses to start
    #[tokio::test]
    async fn serves_renews_and_refuses_a_bad_pair() {
        let dir = tempfile::tempdir().expect("tempdir");
        let files = TlsFiles {
            cert_path: dir.path().join("cert.pem"),
            key_path: dir.path().join("key.pem"),
        };
        let a = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert a");
        let b = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert b");
        // mtime set explicitly: a rewrite inside one clock tick must still read as a change
        let mut written = SystemTime::now();
        let mut write = |path: &Path, pem: String| {
            std::fs::write(path, pem).expect("write pem");
            written += Duration::from_secs(1);
            std::fs::File::options()
                .write(true)
                .open(path)
                .expect("open")
                .set_modified(written)
                .expect("mtime");
        };
        write(&files.cert_path, a.cert.pem());
        write(&files.key_path, a.signing_key.serialize_pem());

        let tls = Tls::load(files.clone()).expect("pair a loads");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let acceptor = tls.acceptor();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move { drop(acceptor.accept(socket).await) });
            }
        });
        let mut roots = RootCertStore::empty();
        roots.add(a.cert.der().clone()).expect("root a");
        roots.add(b.cert.der().clone()).expect("root b");
        let mut client = ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        client.alpn_protocols = vec![b"h2".to_vec()];
        let connector = TlsConnector::from(Arc::new(client));
        let presented = || async {
            let socket = TcpStream::connect(addr).await.expect("connect");
            let name = ServerName::try_from("localhost").expect("name");
            let stream = connector.connect(name, socket).await.expect("handshake");
            let (_, session) = stream.get_ref();
            assert_eq!(session.alpn_protocol(), Some(&b"h2"[..]), "ALPN must negotiate h2");
            session.peer_certificates().expect("server certs")[0].clone()
        };
        assert_eq!(presented().await, *a.cert.der(), "boot pair served");

        write(&files.cert_path, b.cert.pem());
        write(&files.key_path, b.signing_key.serialize_pem());
        tls.certs.reload_if_changed();
        assert_eq!(presented().await, *b.cert.der(), "renewed pair served without a restart");

        write(&files.key_path, a.signing_key.serialize_pem());
        tls.certs.reload_if_changed();
        assert_eq!(presented().await, *b.cert.der(), "mismatched renewal keeps the last good pair");

        let refused = Tls::load(files).err();
        let mismatched = matches!(refused, Some(TlsError::Key { .. }));
        assert!(mismatched, "booting cert b + key a refused for its key: {refused:?}");
    }

    /// - first boot writes `cert.pem` + `key.pem` (key 0600) under a dir it creates
    /// - the listen address is a SAN a verifying client accepts
    /// - the next boot serves the same pair, byte for byte
    /// - cert deleted → re-issued for the kept key; key deleted → refused, cert kept
    #[tokio::test]
    async fn self_signed_pair_is_generated_once_then_reused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dir = dir.path().join("state/tls");
        let listen: SocketAddr = "10.1.2.3:8137".parse().expect("addr");

        let tls = Tls::self_signed(&dir, listen).expect("first boot generates");
        let cert_pem = std::fs::read(dir.join("cert.pem")).expect("cert written");
        let key_pem = std::fs::read(dir.join("key.pem")).expect("key written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("key.pem")).expect("key meta").permissions();
            assert_eq!(mode.mode() & 0o777, 0o600, "key.pem mode");
        }
        let fingerprint = tls.leaf_sha256();
        assert!(fingerprint.starts_with("sha256:") && fingerprint.len() == 7 + 64, "{fingerprint}");

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let acceptor = tls.acceptor();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move { drop(acceptor.accept(socket).await) });
            }
        });
        let mut roots = RootCertStore::empty();
        let leaf = CertificateDer::from_pem_slice(&cert_pem).expect("leaf pem");
        roots.add(leaf.clone()).expect("self-signed leaf = its own root");
        let client = ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(client));
        for name in ["10.1.2.3", "localhost", "127.0.0.1"] {
            let socket = TcpStream::connect(addr).await.expect("connect");
            let server_name = ServerName::try_from(name).expect("name");
            let stream = connector.connect(server_name, socket).await;
            assert!(stream.is_ok(), "{name}: verified against the generated cert: {stream:?}");
        }

        let again = Tls::self_signed(&dir, listen).expect("second boot loads");
        assert_eq!(again.leaf_sha256(), fingerprint, "same certificate across boots");
        assert_eq!(std::fs::read(dir.join("cert.pem")).expect("cert"), cert_pem, "cert kept");
        assert_eq!(std::fs::read(dir.join("key.pem")).expect("key"), key_pem, "key kept");

        std::fs::remove_file(dir.join("cert.pem")).expect("drop cert");
        let reissued = Tls::self_signed(&dir, listen).expect("cert re-issued for the kept key");
        assert_ne!(reissued.leaf_sha256(), fingerprint, "a new certificate");
        assert_eq!(std::fs::read(dir.join("key.pem")).expect("key"), key_pem, "key untouched");

        std::fs::remove_file(dir.join("key.pem")).expect("drop key");
        let refused = Tls::self_signed(&dir, listen).err();
        let orphan = matches!(refused, Some(TlsError::OrphanCertificate(_)));
        assert!(orphan, "cert without key refused, never regenerated over: {refused:?}");
        assert!(dir.join("cert.pem").exists(), "orphan cert left for the operator");
    }
}
