//! The ledger's record format (RAL-11).
//!
//! A record is one JSON object:
//!
//! ```json
//! {
//!   "version": 1,
//!   "kind": "declared",
//!   "source": { "credential": "a1", "declared_name": "gleaming-tench" },
//!   "at_ms": 1793653015261,
//!   "body": {}
//! }
//! ```
//!
//! - `kind` says how the record is known: `observed`, `declared` or `verified`.
//! - `source.credential` is the machine credential or system that wrote it. It isn't a proven
//!   person or team (see `docs/threat-model.md`).
//! - `source.declared_name` is who the writer says it is. Declared and verified records need one.
//!   Observed records came from a system rather than an agent's account, so they can't have one.
//! - `at_ms` is when the record was made, in milliseconds since the Unix epoch.
//! - `body` is an object whose contents are defined by later stories.
//!
//! Anything else is refused, including unknown fields, so that a newer record is never silently
//! read as an older one.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only record format this build reads and writes.
pub const FORMAT_VERSION: u64 = 1;

/// The longest credential or declared name, which others read and has to fit on a line.
const MAX_NAME_CHARS: usize = 64;

fn is_good_name(name: &str) -> bool {
    name.chars().count() <= MAX_NAME_CHARS && !name.chars().any(char::is_control)
}

/// How a record is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Came from a system, not from an agent's account of itself.
    Observed,
    /// An agent or person said so.
    Declared,
    /// Someone other than the author checked it.
    Verified,
}

/// Who or what a record came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub credential: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_name: Option<String>,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.declared_name {
            Some(name) => write!(f, "{} credential, declaring {name}", self.credential),
            None => write!(f, "{} credential", self.credential),
        }
    }
}

/// One entry in the ledger. Fields are private and deserializing goes through [`RawRecord`], so
/// every record has passed validation however it was built.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawRecord")]
pub struct Record {
    version: u64,
    kind: Kind,
    source: Source,
    at_ms: i64,
    body: Value,
}

/// A record's fields before validation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecord {
    version: u64,
    kind: Kind,
    source: Source,
    at_ms: i64,
    body: Value,
}

impl TryFrom<RawRecord> for Record {
    type Error = RecordError;

    fn try_from(raw: RawRecord) -> Result<Self, RecordError> {
        let record = Self {
            version: raw.version,
            kind: raw.kind,
            source: raw.source,
            at_ms: raw.at_ms,
            body: raw.body,
        };
        record.validate()?;
        Ok(record)
    }
}

/// Why a record was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum RecordError {
    /// The text isn't a valid record, for example a required field is missing. Holds the reason.
    Malformed(String),
    /// The record has no usable version, or one this build doesn't read.
    UnsupportedVersion(String),
    /// `source.credential` is empty.
    EmptyCredential,
    /// A declared or verified record has no declared name, or an empty one.
    MissingDeclaredName(Kind),
    /// An observed record claims a declared name.
    ObservedWithDeclaredName,
    /// A credential or declared name is too long or has a control character. Holds the field's name.
    BadName(&'static str),
    /// `at_ms` isn't after the Unix epoch.
    InvalidTime(i64),
    /// `body` isn't a JSON object.
    BodyNotObject,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(reason) => write!(f, "malformed record: {reason}"),
            Self::UnsupportedVersion(found) => {
                write!(
                    f,
                    "unsupported record version {found}; this build reads {FORMAT_VERSION}"
                )
            }
            Self::EmptyCredential => write!(f, "source.credential is empty"),
            Self::MissingDeclaredName(kind) => {
                write!(f, "a {kind:?} record needs source.declared_name")
            }
            Self::ObservedWithDeclaredName => {
                write!(f, "an observed record can't have source.declared_name")
            }
            Self::BadName(field) => write!(
                f,
                "{field} must be at most {MAX_NAME_CHARS} characters, with no control characters"
            ),
            Self::InvalidTime(at_ms) => write!(f, "at_ms {at_ms} isn't after the Unix epoch"),
            Self::BodyNotObject => write!(f, "body isn't a JSON object"),
        }
    }
}

impl std::error::Error for RecordError {}

impl Record {
    /// Builds a record in the current format, refusing one that isn't valid.
    pub fn new(kind: Kind, source: Source, at_ms: i64, body: Value) -> Result<Self, RecordError> {
        let record = Self {
            version: FORMAT_VERSION,
            kind,
            source,
            at_ms,
            body,
        };
        record.validate()?;
        Ok(record)
    }

    /// Reads a record from JSON, refusing one that isn't valid.
    pub fn parse(text: &str) -> Result<Self, RecordError> {
        // Check the version first, so a future format gets a clear refusal rather than a complaint
        // about its fields. Then parse the text itself, not the intermediate value, because
        // building that value would hide a duplicated field.
        let value: Value =
            serde_json::from_str(text).map_err(|e| RecordError::Malformed(e.to_string()))?;
        let version = value
            .get("version")
            .ok_or_else(|| RecordError::Malformed("missing field `version`".into()))?;
        if version.as_u64() != Some(FORMAT_VERSION) {
            return Err(RecordError::UnsupportedVersion(version.to_string()));
        }
        // Go through the raw form so that a validation failure keeps its specific error.
        let raw: RawRecord =
            serde_json::from_str(text).map_err(|e| RecordError::Malformed(e.to_string()))?;
        raw.try_into()
    }

    /// Writes the record as one line of JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a record has only string keys")
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    pub fn at_ms(&self) -> i64 {
        self.at_ms
    }

    pub fn body(&self) -> &Value {
        &self.body
    }

    fn validate(&self) -> Result<(), RecordError> {
        if self.version != FORMAT_VERSION {
            return Err(RecordError::UnsupportedVersion(self.version.to_string()));
        }
        if self.source.credential.trim().is_empty() {
            return Err(RecordError::EmptyCredential);
        }
        let has_name = self
            .source
            .declared_name
            .as_deref()
            .is_some_and(|name| !name.trim().is_empty());
        match self.kind {
            Kind::Observed if self.source.declared_name.is_some() => {
                return Err(RecordError::ObservedWithDeclaredName);
            }
            Kind::Declared | Kind::Verified if !has_name => {
                return Err(RecordError::MissingDeclaredName(self.kind));
            }
            _ => {}
        }
        if !is_good_name(&self.source.credential) {
            return Err(RecordError::BadName("source.credential"));
        }
        if let Some(name) = &self.source.declared_name
            && !is_good_name(name)
        {
            return Err(RecordError::BadName("source.declared_name"));
        }
        if self.at_ms <= 0 {
            return Err(RecordError::InvalidTime(self.at_ms));
        }
        if !self.body.is_object() {
            return Err(RecordError::BodyNotObject);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid() -> Value {
        json!({
            "version": 1,
            "kind": "declared",
            "source": { "credential": "a1", "declared_name": "gleaming-tench" },
            "at_ms": 1_793_653_015_261_i64,
            "body": {}
        })
    }

    fn parse(value: &Value) -> Result<Record, RecordError> {
        Record::parse(&value.to_string())
    }

    fn without(field: &str) -> Value {
        let mut value = valid();
        value.as_object_mut().unwrap().remove(field);
        value
    }

    fn assert_malformed_naming(result: Result<Record, RecordError>, field: &str) {
        match result {
            Err(RecordError::Malformed(reason)) => {
                assert!(reason.contains(field), "{reason} should name {field}");
            }
            other => panic!("expected a malformed record naming {field}, got {other:?}"),
        }
    }

    #[test]
    fn accepts_each_kind_and_round_trips() {
        let source = |name: Option<&str>| Source {
            credential: "a1".into(),
            declared_name: name.map(Into::into),
        };
        for (kind, name) in [
            (Kind::Observed, None),
            (Kind::Declared, Some("copper-gulch")),
            (Kind::Verified, Some("gleaming-tench")),
        ] {
            let record = Record::new(kind, source(name), 1, json!({"story": "RAL-11"})).unwrap();
            assert_eq!(Record::parse(&record.to_json()).unwrap(), record);
        }
    }

    #[test]
    fn refuses_a_missing_version() {
        assert_malformed_naming(parse(&without("version")), "version");
    }

    #[test]
    fn refuses_a_missing_kind() {
        assert_malformed_naming(parse(&without("kind")), "kind");
    }

    #[test]
    fn refuses_a_missing_source() {
        assert_malformed_naming(parse(&without("source")), "source");
    }

    #[test]
    fn refuses_a_missing_credential() {
        let mut value = valid();
        value["source"]
            .as_object_mut()
            .unwrap()
            .remove("credential");
        assert_malformed_naming(parse(&value), "credential");
    }

    #[test]
    fn refuses_a_missing_time() {
        assert_malformed_naming(parse(&without("at_ms")), "at_ms");
    }

    #[test]
    fn refuses_a_missing_body() {
        assert_malformed_naming(parse(&without("body")), "body");
    }

    #[test]
    fn refuses_other_versions() {
        for version in [json!(0), json!(2), json!("1"), json!(1.5), json!(null)] {
            let mut value = valid();
            value["version"] = version;
            assert!(
                matches!(parse(&value), Err(RecordError::UnsupportedVersion(_))),
                "{value}"
            );
        }
    }

    #[test]
    fn refuses_an_unknown_kind() {
        let mut value = valid();
        value["kind"] = json!("trusted");
        assert_malformed_naming(parse(&value), "trusted");
    }

    #[test]
    fn refuses_unknown_fields() {
        let mut value = valid();
        value["points"] = json!(10);
        assert_malformed_naming(parse(&value), "points");

        let mut value = valid();
        value["source"]["proven"] = json!(true);
        assert_malformed_naming(parse(&value), "proven");
    }

    #[test]
    fn refuses_a_duplicated_field() {
        let text = r#"{"version":1,"kind":"observed","kind":"verified",
            "source":{"credential":"a1"},"at_ms":1,"body":{}}"#;
        assert_malformed_naming(Record::parse(text), "kind");
    }

    #[test]
    fn refuses_text_that_is_not_json() {
        assert!(matches!(
            Record::parse("not json"),
            Err(RecordError::Malformed(_))
        ));
    }

    #[test]
    fn refuses_an_empty_credential() {
        for credential in ["", "  "] {
            let mut value = valid();
            value["source"]["credential"] = json!(credential);
            assert_eq!(parse(&value), Err(RecordError::EmptyCredential));
        }
    }

    #[test]
    fn declared_and_verified_records_need_a_name() {
        for kind in [("declared", Kind::Declared), ("verified", Kind::Verified)] {
            for name in [None, Some(""), Some("  ")] {
                let mut value = valid();
                value["kind"] = json!(kind.0);
                match name {
                    Some(name) => value["source"]["declared_name"] = json!(name),
                    None => {
                        value["source"]
                            .as_object_mut()
                            .unwrap()
                            .remove("declared_name");
                    }
                }
                assert_eq!(parse(&value), Err(RecordError::MissingDeclaredName(kind.1)));
            }
        }
    }

    #[test]
    fn observed_records_cannot_declare_a_name() {
        let mut value = valid();
        value["kind"] = json!("observed");
        assert_eq!(parse(&value), Err(RecordError::ObservedWithDeclaredName));
    }

    #[test]
    fn refuses_a_time_not_after_the_epoch() {
        for at_ms in [0, -1] {
            let mut value = valid();
            value["at_ms"] = json!(at_ms);
            assert_eq!(parse(&value), Err(RecordError::InvalidTime(at_ms)));
        }
    }

    #[test]
    fn refuses_a_body_that_is_not_an_object() {
        for body in [json!(null), json!([]), json!("done")] {
            let mut value = valid();
            value["body"] = body;
            assert_eq!(parse(&value), Err(RecordError::BodyNotObject));
        }
    }

    #[test]
    fn deserializing_directly_also_validates() {
        let bad = [
            ("credential", json!({"credential": ""})),
            ("declared_name", json!({"credential": "a1"})),
        ];
        for (why, source) in bad {
            let mut value = valid();
            value["source"] = source;
            let from_text = serde_json::from_str::<Record>(&value.to_string());
            assert!(from_text.is_err(), "{why} from text");
            let from_value = serde_json::from_value::<Record>(value);
            assert!(from_value.is_err(), "{why} from value");
        }
        let mut value = valid();
        value["version"] = json!(2);
        assert!(serde_json::from_value::<Record>(value).is_err());
        let mut value = valid();
        value["points"] = json!(10);
        assert!(serde_json::from_value::<Record>(value).is_err());
        assert!(serde_json::from_value::<Record>(valid()).is_ok());
    }

    #[test]
    fn refuses_a_long_or_control_character_name() {
        for (field, bad) in [
            ("credential", "x".repeat(65)),
            ("declared_name", "x".repeat(65)),
            ("declared_name", "a\u{1b}[31m".to_owned()),
            ("credential", "a\nb".to_owned()),
        ] {
            let mut value = valid();
            value["source"][field] = json!(bad);
            assert!(parse(&value).is_err(), "{field}: {bad:?}");
        }
    }

    #[test]
    fn new_validates_too() {
        let source = Source {
            credential: "a1".into(),
            declared_name: None,
        };
        assert_eq!(
            Record::new(Kind::Declared, source, 1, json!({})),
            Err(RecordError::MissingDeclaredName(Kind::Declared))
        );
    }

    #[test]
    fn source_shows_a_credential_rather_than_a_proven_team() {
        let declared = Source {
            credential: "a1".into(),
            declared_name: Some("gleaming-tench".into()),
        };
        assert_eq!(
            declared.to_string(),
            "a1 credential, declaring gleaming-tench"
        );
        let observed = Source {
            credential: "github".into(),
            declared_name: None,
        };
        assert_eq!(observed.to_string(), "github credential");
    }
}
