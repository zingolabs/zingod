//! `[serve] tls`: a mode name (`"self-signed"` | `"plaintext"`) or the `[serve.tls]` PEM pair

use std::fmt;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use zingod_grpc::TlsFiles;

/// How the gRPC listener is served (default = a pair zingod generates under `state_dir/tls`)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum ServeTls {
    #[default]
    SelfSigned,
    Plaintext,
    Files(TlsFiles),
}

const SELF_SIGNED: &str = "self-signed";
const PLAINTEXT: &str = "plaintext";

impl Serialize for ServeTls {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::SelfSigned => serializer.serialize_str(SELF_SIGNED),
            Self::Plaintext => serializer.serialize_str(PLAINTEXT),
            Self::Files(files) => files.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ServeTls {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ModeOrFiles;

        impl<'de> Visitor<'de> for ModeOrFiles {
            type Value = ServeTls;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "\"{SELF_SIGNED}\", \"{PLAINTEXT}\" or a table with cert_path + key_path")
            }

            fn visit_str<E: de::Error>(self, mode: &str) -> Result<ServeTls, E> {
                match mode {
                    SELF_SIGNED => Ok(ServeTls::SelfSigned),
                    PLAINTEXT => Ok(ServeTls::Plaintext),
                    other => Err(E::unknown_variant(other, &[SELF_SIGNED, PLAINTEXT])),
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<ServeTls, A::Error> {
                TlsFiles::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(ServeTls::Files)
            }
        }

        deserializer.deserialize_any(ModeOrFiles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Deserialize, Serialize)]
    struct Serve {
        #[serde(default)]
        tls: ServeTls,
    }

    /// - absent = self-signed; each mode by name; the table = the pair
    /// - each form serializes back to the TOML it came from
    /// - an unknown name or table key is refused naming the choices
    #[test]
    fn every_form_parses_round_trips_and_a_wrong_one_names_the_choices() {
        let files = TlsFiles { cert_path: "/c.pem".into(), key_path: "/k.pem".into() };
        for (toml, expected) in [
            ("", ServeTls::SelfSigned),
            ("tls = \"self-signed\"\n", ServeTls::SelfSigned),
            ("tls = \"plaintext\"\n", ServeTls::Plaintext),
            ("[tls]\ncert_path = \"/c.pem\"\nkey_path = \"/k.pem\"\n", ServeTls::Files(files)),
        ] {
            let parsed: Serve = toml::from_str(toml).expect(toml);
            assert_eq!(parsed.tls, expected, "{toml:?}");
            let printed = toml::to_string(&parsed).expect("serializes");
            let round_trip: Serve = toml::from_str(&printed).expect(&printed);
            assert_eq!(round_trip, parsed, "{toml:?} → {printed:?} → back");
        }

        let unknown = toml::from_str::<Serve>("tls = \"none\"\n").expect_err("unknown mode");
        let message = unknown.to_string();
        assert!(
            message.contains("unknown variant `none`") && message.contains("`plaintext`"),
            "names the modes: {message}"
        );
        let partial =
            toml::from_str::<Serve>("[tls]\ncert_path = \"/c.pem\"\n").expect_err("no key");
        assert!(partial.to_string().contains("missing field `key_path`"), "{partial}");
        let bogus =
            toml::from_str::<Serve>("[tls]\ncert_path = \"/c\"\nkey_path = \"/k\"\nbogus = 1\n")
                .expect_err("unknown key");
        assert!(bogus.to_string().contains("unknown field `bogus`"), "{bogus}");
    }
}
