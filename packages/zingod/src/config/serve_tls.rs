//! `[serve.tls]` → how the gRPC listener is served
//!
//! - table absent = self-signed pair under `state_dir/tls`
//! - `cert_path = "plaintext"` + `key_path = "plaintext"` = no TLS (one alone refused)
//! - any other pair = the operator's PEM files

use std::path::Path;

use zingod_grpc::TlsFiles;

use crate::error::IndexerError;

/// Path value standing in for "no TLS" in both `[serve.tls]` fields
const PLAINTEXT: &str = "plaintext";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServeTls {
    SelfSigned,
    Plaintext,
    Files(TlsFiles),
}

/// `Err` = exactly one of the two paths is `"plaintext"`
pub(super) fn resolve(table: Option<TlsFiles>) -> Result<ServeTls, IndexerError> {
    let Some(files) = table else { return Ok(ServeTls::SelfSigned) };
    let plaintext = |path: &Path| path == Path::new(PLAINTEXT);
    match (plaintext(&files.cert_path), plaintext(&files.key_path)) {
        (true, true) => Ok(ServeTls::Plaintext),
        (false, false) => Ok(ServeTls::Files(files)),
        _ => Err(IndexerError::Config(format!(
            "[serve.tls]: cert_path and key_path must both be \"{PLAINTEXT}\" (no TLS) or both \
             name a PEM file; got cert_path = {:?}, key_path = {:?}",
            files.cert_path, files.key_path
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// - absent = self-signed; both paths "plaintext" = plaintext; any other pair = the files
    /// - one path "plaintext" alone is refused naming both fields
    #[test]
    fn every_table_resolves_and_a_half_plaintext_one_is_refused() {
        let files =
            |cert: &str, key: &str| TlsFiles { cert_path: cert.into(), key_path: key.into() };
        for (table, expected) in [
            (None, ServeTls::SelfSigned),
            (Some(files("plaintext", "plaintext")), ServeTls::Plaintext),
            (Some(files("/c.pem", "/k.pem")), ServeTls::Files(files("/c.pem", "/k.pem"))),
            // a file called plaintext in a directory = a file
            (
                Some(files("/etc/plaintext", "/k.pem")),
                ServeTls::Files(files("/etc/plaintext", "/k.pem")),
            ),
        ] {
            let resolved = resolve(table.clone()).map_err(|error| error.to_string());
            assert_eq!(resolved, Ok(expected), "{table:?}");
        }
        for (cert, key) in [("plaintext", "/k.pem"), ("/c.pem", "plaintext")] {
            let refused = resolve(Some(files(cert, key))).expect_err("half plaintext").to_string();
            assert!(
                refused.contains("cert_path and key_path must both") && refused.contains(cert),
                "{cert} + {key}: {refused}"
            );
        }
    }
}
