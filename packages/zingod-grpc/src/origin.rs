//! Origin gate: a browser (gRPC-web) request's `Origin` → block / allow / default
//!
//! - markers set operator-side (`zingod origin`, FQDN keys); this folds an incoming host to the
//!   most-specific marker covering it (`allow good.evil.com` beats `block evil.com`)
//! - block = refuse; allow = carve-out from a parent block; either unblocked = normal admission
//! - swapped whole each second by the poll loop (zingod's `command::origin::spawn`); the serve
//!   path only reads

use std::collections::BTreeSet;
use std::sync::Arc;

use arc_swap::ArcSwap;

/// One origin's standing against the markers
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginVerdict {
    Default,
    Allowed,
    Blocked,
}

/// Blocked + allowed FQDN keys, disjoint (one marker per domain)
#[derive(Debug, Default, PartialEq, Eq)]
struct OriginSets {
    blocked: BTreeSet<String>,
    allowed: BTreeSet<String>,
}

/// Shared origin policy: the poll loop `set`s it whole, every request `verdict`s against it
#[derive(Debug)]
pub struct OriginGate {
    sets: ArcSwap<OriginSets>,
}

impl OriginGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { sets: ArcSwap::from_pointee(OriginSets::default()) })
    }

    /// Replace every marker (poll loop, on a change)
    pub fn set(&self, blocked: BTreeSet<String>, allowed: BTreeSet<String>) {
        self.sets.store(Arc::new(OriginSets { blocked, allowed }));
    }

    /// `host`'s verdict (lowercased, no scheme / port): most-specific marker wins, walking label
    /// boundaries up from the FQDN
    ///
    /// - markers are never public suffixes (write-time guard), so probing up to the bare TLD is
    ///   safe and never matches
    pub fn verdict(&self, host: &str) -> OriginVerdict {
        let sets = self.sets.load();
        let mut candidate = host;
        loop {
            if sets.blocked.contains(candidate) {
                return OriginVerdict::Blocked;
            }
            if sets.allowed.contains(candidate) {
                return OriginVerdict::Allowed;
            }
            match candidate.split_once('.') {
                Some((_, parent)) => candidate = parent,
                None => return OriginVerdict::Default,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fold: a parent block covers subdomains, an FQDN allow carves one back out, and a
    /// label-boundary walk never matches a lookalike (`notevil.com`, `evil.com.au`)
    #[test]
    fn verdict_folds_to_the_most_specific_marker() {
        let gate = OriginGate::new();
        gate.set(
            ["evil.com".to_owned(), "flip.example".to_owned()].into(),
            ["good.evil.com".to_owned(), "konclave.app".to_owned()].into(),
        );
        use OriginVerdict::{Allowed, Blocked, Default};
        #[rustfmt::skip]
        let cases = [
            ("evil.com",        Blocked),   // the block itself
            ("sub.evil.com",    Blocked),   // covered by the parent block
            ("a.b.evil.com",    Blocked),   // covered several levels down
            ("good.evil.com",   Allowed),   // FQDN allow beats the parent block
            ("x.good.evil.com", Allowed),   // under the carve-out
            ("notevil.com",     Default),   // not a label-boundary suffix of evil.com
            ("evil.com.au",     Default),   // the parent walk never yields evil.com
            ("konclave.app",    Allowed),
            ("flip.example",    Blocked),
            ("other.example",   Default),   // no marker at all
        ];
        for (host, want) in cases {
            assert_eq!(gate.verdict(host), want, "{host}");
        }
    }

    /// Empty reads default; a `set` replaces the policy whole (no merge with the prior one)
    #[test]
    fn an_empty_or_replaced_gate_reads_clean() {
        let gate = OriginGate::new();
        assert_eq!(gate.verdict("evil.com"), OriginVerdict::Default, "empty: default");
        gate.set(["evil.com".to_owned()].into(), BTreeSet::new());
        assert_eq!(gate.verdict("sub.evil.com"), OriginVerdict::Blocked, "block in force");
        gate.set(BTreeSet::new(), ["evil.com".to_owned()].into());
        assert_eq!(gate.verdict("sub.evil.com"), OriginVerdict::Allowed, "prior block gone");
        gate.set(BTreeSet::new(), BTreeSet::new());
        assert_eq!(gate.verdict("evil.com"), OriginVerdict::Default, "replaced empty: default");
    }
}
