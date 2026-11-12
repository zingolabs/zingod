//! `zingod origin block|allow|reset <domain>` + bare list: operator origin policy in
//! `<serve.state_dir>/origins/`, one marker per domain (content `block` | `allow`)
//!
//! - block = every request carrying the origin refused
//! - allow = unblocked under a blocked parent domain (caps unchanged: Origin client-settable)
//! - neither = default
//! - permanent: only `reset` (or `block`/`allow` overwriting) changes a marker
//! - running zingod reads them every second → the gRPC origin gate (a change = WARN)

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tracing::warn;
use zingod_grpc::OriginGate;

use super::{remove_marker, write_marker, MarkerError};

const BLOCK: &str = "block";
const ALLOW: &str = "allow";

#[derive(Debug, Clone, PartialEq, Eq, clap::Subcommand)]
pub(crate) enum Action {
    /// Refuse every request carrying this origin.
    Block { domain: String },
    /// Serve this origin even under a blocked parent domain (limits apply as to any client).
    Allow { domain: String },
    /// Return this origin to the default policy.
    Reset { domain: String },
}

/// Every marker in force, blocked and allowed disjoint (one marker per domain)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Policy {
    blocked: BTreeSet<String>,
    allowed: BTreeSet<String>,
}

/// Operator domain arg → marker key (FQDN kept as given, match-time folding is the gate's)
///
/// - lowercased, scheme + port stripped (lenient on a pasted origin), `/` rejected (no path)
/// - `[a-z0-9.-]` only (key safe as a filename), no leading / trailing `.`
/// - registrable domain required (`psl`): `evil.com`, `sub.evil.com` ok; `com`, `github.io`
///   refused (blocking a public suffix = every site under it)
/// - FQDN kept: `block evil.com` covers subdomains at match time, `allow good.evil.com` carves
///   one back out (most-specific marker wins)
fn canonical_domain(input: &str) -> Result<String, MarkerError> {
    let refused = |why: &str| MarkerError::Invalid(format!("{input:?}: {why}"));
    let lowered = input.trim().to_ascii_lowercase();
    let after_scheme = match lowered.split_once("://") {
        Some((_, rest)) => rest,
        None => &lowered,
    };
    let host = after_scheme.split(':').next().unwrap_or(after_scheme);
    let shaped = !host.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if !shaped {
        return Err(refused("not a domain"));
    }
    if psl::domain_str(host).is_none() {
        return Err(refused("a public suffix, not a registrable domain"));
    }
    Ok(host.to_owned())
}

/// The marker `action` names written or removed (`domain` validated first)
fn apply(dir: &Path, action: &Action) -> Result<(), MarkerError> {
    match action {
        Action::Block { domain } => write_marker(dir, &canonical_domain(domain)?, BLOCK),
        Action::Allow { domain } => write_marker(dir, &canonical_domain(domain)?, ALLOW),
        Action::Reset { domain } => remove_marker(dir, &canonical_domain(domain)?).map(drop),
    }
}

/// `dir`'s markers (no `dir` = empty; stray content ignored, never fatal)
fn read(dir: &Path) -> Result<Policy, MarkerError> {
    let io = |path: &Path, source| MarkerError::Io { path: path.to_owned(), source };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Policy::default()),
        Err(error) => return Err(io(dir, error)),
    };
    let mut policy = Policy::default();
    for entry in entries {
        let entry = entry.map_err(|error| io(dir, error))?;
        let path = entry.path();
        let domain = entry.file_name().to_string_lossy().into_owned();
        match std::fs::read_to_string(&path).map_err(|error| io(&path, error))?.trim() {
            BLOCK => drop(policy.blocked.insert(domain)),
            ALLOW => drop(policy.allowed.insert(domain)),
            _ => continue,
        }
    }
    Ok(policy)
}

/// `zingod origin [block|allow|reset <domain>]`: applies the action (if any), prints every marker
pub(crate) fn command(config_path: &Path, action: Option<Action>) -> Result<ExitCode, MarkerError> {
    let config = crate::config::load_config(config_path)?;
    let dir: PathBuf = config.serve.state_dir.join("origins");
    if let Some(action) = &action {
        apply(&dir, action)?;
    }
    let policy = read(&dir)?;
    for domain in &policy.blocked {
        println!("block {domain}");
    }
    for domain in &policy.allowed {
        println!("allow {domain}");
    }
    eprintln!(
        "{} blocked, {} allowed in {} (a running zingod applies within 1 s)",
        policy.blocked.len(),
        policy.allowed.len(),
        dir.display()
    );
    Ok(ExitCode::SUCCESS)
}

/// Every second for the process's life: the markers → `gate` (a change = WARN; unreadable =
/// WARN, policy kept)
pub(crate) fn spawn(gate: Arc<OriginGate>, state_dir: &Path) {
    let dir: PathBuf = state_dir.join("origins");
    tokio::spawn(async move {
        let mut current = Policy::default();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let policy = match read(&dir) {
                Ok(policy) if policy == current => continue,
                Ok(policy) => policy,
                Err(error) => {
                    warn!(%error, "Origin markers ignored, policy kept");
                    continue;
                }
            };
            gate.set(policy.blocked.clone(), policy.allowed.clone());
            warn!(
                blocked = policy.blocked.len(),
                allowed = policy.allowed.len(),
                "Origin policy changed by operator"
            );
            current = policy;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session of commands, read back as the running zingod reads it:
    /// - block / allow land under the canonical key; a pasted origin folds to the same marker
    /// - block then allow on one domain = allowed only (one marker per domain, last write wins)
    /// - an FQDN allow lives beside its eTLD+1 block (carve-out kept for the match-time gate)
    /// - reset returns a domain to default
    #[test]
    fn markers_set_the_policy_until_reset() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("origins");
        let run = |action: Action| apply(&dir, &action);
        let block = |d: &str| Action::Block { domain: d.to_owned() };
        let allow = |d: &str| Action::Allow { domain: d.to_owned() };
        let reset = |d: &str| Action::Reset { domain: d.to_owned() };

        assert_eq!(read(&dir).expect("reads"), Policy::default(), "no markers: default");
        for action in [
            block("evil.com"),
            block("HTTPS://Spam.EXAMPLE:443"),
            allow("good.evil.com"),
            allow("konclave.app"),
            block("flip.example"),
            allow("flip.example"),
            reset("spam.example"),
        ] {
            run(action).expect("applies");
        }
        assert_eq!(
            read(&dir).expect("reads"),
            Policy {
                blocked: ["evil.com".to_owned()].into(),
                allowed: [
                    "flip.example".to_owned(),
                    "good.evil.com".to_owned(),
                    "konclave.app".to_owned(),
                ]
                .into(),
            },
            "spam folded + reset away; flip flipped block→allow; good.evil.com carved out",
        );
    }

    /// A malformed domain or a bare public suffix is refused naming why, before any marker lands
    #[test]
    fn a_bad_domain_is_refused_and_never_lands() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("origins");
        #[rustfmt::skip]
        let refusals = [
            ("",               "not a domain"),
            ("   ",            "not a domain"),
            ("a/../b",         "not a domain"),
            ("has space",      "not a domain"),
            ("bad_underscore", "not a domain"),
            (".leading",       "not a domain"),
            ("trailing.",      "not a domain"),
            ("com",            "public suffix"),
            ("github.io",      "public suffix"),
            ("co.uk",          "public suffix"),
        ];
        for (bad, why) in refusals {
            let refused = apply(&dir, &Action::Block { domain: bad.to_owned() });
            assert!(
                matches!(&refused, Err(MarkerError::Invalid(msg)) if msg.contains(why)),
                "{bad:?}: refused containing {why:?}, got {refused:?}",
            );
        }
        assert_eq!(read(&dir).expect("reads"), Policy::default(), "nothing refused landed");
    }

    /// A stray marker (hand-dropped, unknown content) is ignored, not an error
    #[test]
    fn a_stray_marker_is_ignored() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("origins");
        apply(&dir, &Action::Block { domain: "evil.com".to_owned() }).expect("applies");
        std::fs::write(dir.join("junk.example"), "nonsense").expect("stray marker");

        assert_eq!(
            read(&dir).expect("reads"),
            Policy { blocked: ["evil.com".to_owned()].into(), allowed: BTreeSet::new() },
            "stray content skipped, the real marker kept",
        );
    }
}
