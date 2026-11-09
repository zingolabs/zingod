//! `[serve] tls` → how the gRPC listener is served
//!
//! - `"off"` | `"self_signed"` | `{ cert_path, key_path }`; unset = off on loopback only

use std::fmt;
use std::net::SocketAddr;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use zingod_grpc::TlsFiles;

use crate::error::IndexerError;

/// `[serve] tls` as written
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(super) enum TlsSetting {
    Mode(TlsMode),
    Files(TlsFiles),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TlsMode {
    Off,
    SelfSigned,
}

const OFF: &str = "off";
const SELF_SIGNED: &str = "self_signed";

/// Hand-written (serde's untagged error names no accepted value)
impl<'de> Deserialize<'de> for TlsSetting {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SettingVisitor;

        impl<'de> Visitor<'de> for SettingVisitor {
            type Value = TlsSetting;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "\"{OFF}\", \"{SELF_SIGNED}\" or {{ cert_path, key_path }}")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<TlsSetting, E> {
                match value {
                    OFF => Ok(TlsSetting::Mode(TlsMode::Off)),
                    SELF_SIGNED => Ok(TlsSetting::Mode(TlsMode::SelfSigned)),
                    other => Err(E::invalid_value(de::Unexpected::Str(other), &self)),
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<TlsSetting, A::Error> {
                let files = TlsFiles::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(TlsSetting::Files(files))
            }
        }

        deserializer.deserialize_any(SettingVisitor)
    }
}

/// [`TlsSetting`], resolved against the listen address
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServeTls {
    Off,
    SelfSigned,
    Files(TlsFiles),
}

/// `Err` = unset off loopback
pub(super) fn resolve(
    setting: Option<TlsSetting>,
    listen: SocketAddr,
) -> Result<ServeTls, IndexerError> {
    match setting {
        None if listen.ip().to_canonical().is_loopback() => Ok(ServeTls::Off),
        None => Err(IndexerError::Config(format!(
            "serve.tls: required off loopback (grpc_listen_address = {listen}); set it to \
             {{ cert_path, key_path }} (a CA-issued pair), \"{OFF}\" (TLS terminated in front) or \
             \"{SELF_SIGNED}\" (clients that pin its fingerprint)"
        ))),
        Some(TlsSetting::Mode(TlsMode::Off)) => Ok(ServeTls::Off),
        Some(TlsSetting::Mode(TlsMode::SelfSigned)) => Ok(ServeTls::SelfSigned),
        Some(TlsSetting::Files(files)) => Ok(ServeTls::Files(files)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_resolves_against_its_listen_address_and_the_unsafe_ones_are_refused() {
        let files =
            |cert: &str, key: &str| TlsFiles { cert_path: cert.into(), key_path: key.into() };
        let parse = |toml: &str| -> Result<Option<TlsSetting>, String> {
            #[derive(Deserialize)]
            struct Serve {
                tls: Option<TlsSetting>,
            }
            toml::from_str::<Serve>(toml).map(|serve| serve.tls).map_err(|e| e.message().to_owned())
        };
        let (loopback, loopback_v6, mapped, any, lan) = (
            "127.0.0.1:8137".parse().expect("v4"),
            "[::1]:8137".parse().expect("v6"),
            "[::ffff:127.0.0.1]:8137".parse().expect("mapped"),
            "0.0.0.0:8137".parse().expect("any"),
            "192.168.1.5:8137".parse().expect("lan"),
        );
        let pem = files("/c.pem", "/k.pem");
        // Err = the one fact the refusal must name (the listen address)
        for (toml, listen, expected) in [
            ("", loopback, Ok(ServeTls::Off)),
            ("", loopback_v6, Ok(ServeTls::Off)),
            ("", mapped, Ok(ServeTls::Off)),
            ("", any, Err("0.0.0.0:8137")),
            ("", lan, Err("192.168.1.5:8137")),
            ("tls = \"off\"", any, Ok(ServeTls::Off)),
            ("tls = \"self_signed\"", loopback, Ok(ServeTls::SelfSigned)),
            (
                "tls = { cert_path = \"/c.pem\", key_path = \"/k.pem\" }",
                any,
                Ok(ServeTls::Files(pem.clone())),
            ),
            ("[tls]\ncert_path = \"/c.pem\"\nkey_path = \"/k.pem\"", lan, Ok(ServeTls::Files(pem))),
        ] {
            let setting = parse(toml).expect(toml);
            let resolved = resolve(setting, listen).map_err(|error| error.to_string());
            match (&resolved, expected) {
                (Err(got), Err(want)) => assert!(got.contains(want), "{toml} @ {listen}: {got}"),
                (got, want) => assert_eq!(got, &want.map_err(str::to_owned), "{toml} @ {listen}"),
            }
        }
        let unknown = parse("tls = \"on\"").expect_err("an unknown mode");
        assert!(unknown.contains("\"off\", \"self_signed\""), "accepted modes named: {unknown}");
    }
}
