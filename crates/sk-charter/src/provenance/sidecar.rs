//! Per-artifact provenance sidecar reader (`ProvenanceEntry`).
//!
//! Faithful port of `src/charter/activation/synthesizer/provenance.py`'s read
//! half (`load_yaml`) and the `ProvenanceEntry` model it validates against
//! (`src/charter/activation/synthesizer/synthesize_pipeline.py`, schema version
//! 2). The synthesizer that *writes* these sidecars is a separate, unported
//! capability; this module only reads what is on disk.
//!
//! Storage layout (data-model.md §E-4):
//! `.kittify/charter/provenance/<kind>-<slug>.yaml`, i.e. the path
//! [`crate::synthesis::provenance_path_for`] spells.
//!
//! The model is typed, and the typing is load-bearing: the reference validates
//! with Pydantic, so a sidecar that is missing a required field, carries a
//! non-string where a string belongs, names a kind outside the five artifact
//! kinds, omits both provenance sources, or declares a `schema_version` other
//! than `"2"` is a *validation failure* — one reported warning per file by the
//! status collector, never a partially-populated entry and never a fabricated
//! `produced_at` or parsed count.
//!
//! Semantics preserved from the reference model:
//!
//! * `schema_version` is `Literal["2"]` with a `"2"` default, so an absent key
//!   is v2 and any other *present* value fails validation.
//! * `source_section`, `source_urns` and `source_input_ids` are required keys;
//!   `source_section` may be `null` but must be present.
//! * `synthesizer_version` and `synthesis_run_id` are `min_length=1`.
//! * `evidence_bundle_hash` and `adapter_notes` default to `null`.
//! * The model validator requires at least one of a non-empty `source_section`
//!   or a non-empty `source_urns`.
//! * Unknown keys are ignored (Pydantic's default `extra="ignore"`).
//!
//! Divergence kept explicit: the reference serialises Pydantic's multi-line
//! `ValidationError` as the warning text; here the first failing field and its
//! rule are named in one line, because the entry itself (not the exception
//! rendering) is what the status payload reports.

use std::path::Path;

use serde_yaml_ng::{Mapping, Value};

use crate::error::CharterError;
use crate::io::{FieldValue, field};
use crate::limits::Limits;

/// The five artifact kinds a provenance entry may describe
/// (`ProvenanceEntry.artifact_kind`).
pub const PROVENANCE_ARTIFACT_KINDS: [&str; 5] = [
    "directive",
    "tactic",
    "styleguide",
    "procedure",
    "agent_profile",
];

/// The one schema version this reader validates (`Literal["2"]`).
pub const PROVENANCE_SCHEMA_VERSION: &str = "2";

/// A per-artifact provenance sidecar (`ProvenanceEntry`, schema version 2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvenanceEntry {
    /// `schema_version`, always `"2"` once validated.
    pub schema_version: String,
    /// The artifact's URN (`directive:PROJECT_001`).
    pub artifact_urn: String,
    /// One of [`PROVENANCE_ARTIFACT_KINDS`].
    pub artifact_kind: String,
    /// The artifact slug.
    pub artifact_slug: String,
    /// Hex digest over `canonical_yaml(body)` of the artifact.
    pub artifact_content_hash: String,
    /// Full hex digest of the normalized synthesis request.
    pub inputs_hash: String,
    /// The adapter that produced the artifact.
    pub adapter_id: String,
    /// The adapter's version.
    pub adapter_version: String,
    /// The CLI version that produced this entry (`min_length=1`).
    pub synthesizer_version: String,
    /// The authored charter section, when the run had one.
    pub source_section: Option<String>,
    /// The URNs the artifact was derived from.
    pub source_urns: Vec<String>,
    /// Mirror of `source_urns` at write time.
    pub source_input_ids: Vec<String>,
    /// The adapter's ISO-8601 UTC generation stamp.
    pub generated_at: String,
    /// The promote pipeline's ISO-8601 UTC write stamp.
    pub produced_at: String,
    /// `snapshot_id` of the evidence bundle's corpus snapshot (`"(none)"` when
    /// the run had none — the reference's sentinel, not a fabricated id).
    pub corpus_snapshot_id: String,
    /// The staging run that wrote this sidecar (`min_length=1`).
    pub synthesis_run_id: String,
    /// SHA-256 of the serialized evidence bundle, when one was recorded.
    pub evidence_bundle_hash: Option<String>,
    /// Free-form adapter notes.
    pub adapter_notes: Option<String>,
}

/// Load and validate one provenance sidecar (`provenance.load_yaml`).
///
/// # Errors
///
/// [`CharterError`] when the file cannot be read within the default charter
/// read budget, when the document is not YAML, when the root is not a mapping,
/// or when the document does not validate as a [`ProvenanceEntry`]. A missing
/// file is an I/O error (the reference's `FileNotFoundError`), not `None` —
/// callers that glob a directory already know the file exists.
pub fn load_provenance(path: &Path) -> Result<ProvenanceEntry, CharterError> {
    let bytes = crate::io::read_bounded(path, Limits::default().max_charter_file_bytes)?;
    let value: Value =
        serde_yaml_ng::from_slice(&bytes).map_err(|source| CharterError::InvalidYaml {
            path: path.to_path_buf(),
            source,
        })?;
    let Some(mapping) = value.as_mapping() else {
        return Err(invalid(
            path,
            "input should be a valid dictionary or instance of ProvenanceEntry",
        ));
    };
    ProvenanceEntry::from_mapping(mapping, path)
}

impl ProvenanceEntry {
    /// Validate a parsed sidecar mapping.
    fn from_mapping(mapping: &Mapping, path: &Path) -> Result<Self, CharterError> {
        let schema_version = match field(mapping, "schema_version") {
            FieldValue::Absent => PROVENANCE_SCHEMA_VERSION.to_owned(),
            FieldValue::String(text) if text == PROVENANCE_SCHEMA_VERSION => text.to_owned(),
            _ => {
                return Err(invalid(
                    path,
                    &format!(
                        "schema_version: input should be {PROVENANCE_SCHEMA_VERSION:?} \
                         (this reader validates the v2 sidecar only)"
                    ),
                ));
            }
        };
        let artifact_kind = required_string(mapping, "artifact_kind", path)?;
        if !PROVENANCE_ARTIFACT_KINDS.contains(&artifact_kind.as_str()) {
            return Err(invalid(
                path,
                &format!(
                    "artifact_kind: input should be one of {PROVENANCE_ARTIFACT_KINDS:?}, \
                     got {artifact_kind:?}"
                ),
            ));
        }
        let synthesizer_version = required_non_empty(mapping, "synthesizer_version", path)?;
        let synthesis_run_id = required_non_empty(mapping, "synthesis_run_id", path)?;
        let source_section = required_nullable_string(mapping, "source_section", path)?;
        let source_urns = required_string_list(mapping, "source_urns", path)?;
        let source_input_ids = required_string_list(mapping, "source_input_ids", path)?;
        if !source_section
            .as_deref()
            .is_some_and(|text| !text.is_empty())
            && source_urns.is_empty()
        {
            return Err(invalid(
                path,
                "ProvenanceEntry requires at least one of 'source_section' (non-empty) or \
                 'source_urns' (non-empty list). Both are absent/empty.",
            ));
        }
        Ok(Self {
            schema_version,
            artifact_urn: required_string(mapping, "artifact_urn", path)?,
            artifact_kind,
            artifact_slug: required_string(mapping, "artifact_slug", path)?,
            artifact_content_hash: required_string(mapping, "artifact_content_hash", path)?,
            inputs_hash: required_string(mapping, "inputs_hash", path)?,
            adapter_id: required_string(mapping, "adapter_id", path)?,
            adapter_version: required_string(mapping, "adapter_version", path)?,
            synthesizer_version,
            source_section,
            source_urns,
            source_input_ids,
            generated_at: required_string(mapping, "generated_at", path)?,
            produced_at: required_string(mapping, "produced_at", path)?,
            corpus_snapshot_id: required_string(mapping, "corpus_snapshot_id", path)?,
            synthesis_run_id,
            evidence_bundle_hash: optional_string(mapping, "evidence_bundle_hash", path)?,
            adapter_notes: optional_string(mapping, "adapter_notes", path)?,
        })
    }
}

fn invalid(path: &Path, reason: &str) -> CharterError {
    CharterError::InvalidConfig {
        path: path.to_path_buf(),
        reason: format!("invalid provenance sidecar: {reason}"),
    }
}

/// A field's presence, distinguishing an absent key from an explicit `null`.
fn required_string(mapping: &Mapping, key: &str, path: &Path) -> Result<String, CharterError> {
    match field(mapping, key) {
        FieldValue::Absent => Err(invalid(path, &format!("{key}: field required"))),
        FieldValue::String(text) => Ok(text.to_owned()),
        FieldValue::Null | FieldValue::Other(_) => Err(invalid(
            path,
            &format!("{key}: input should be a valid string"),
        )),
    }
}

fn required_non_empty(mapping: &Mapping, key: &str, path: &Path) -> Result<String, CharterError> {
    let text = required_string(mapping, key, path)?;
    if text.is_empty() {
        return Err(invalid(
            path,
            &format!("{key}: string should have at least 1 character"),
        ));
    }
    Ok(text)
}

fn required_nullable_string(
    mapping: &Mapping,
    key: &str,
    path: &Path,
) -> Result<Option<String>, CharterError> {
    match field(mapping, key) {
        FieldValue::Absent => Err(invalid(path, &format!("{key}: field required"))),
        FieldValue::Null => Ok(None),
        FieldValue::String(text) => Ok(Some(text.to_owned())),
        FieldValue::Other(_) => Err(invalid(
            path,
            &format!("{key}: input should be a valid string"),
        )),
    }
}

fn optional_string(
    mapping: &Mapping,
    key: &str,
    path: &Path,
) -> Result<Option<String>, CharterError> {
    match field(mapping, key) {
        FieldValue::Absent | FieldValue::Null => Ok(None),
        FieldValue::String(text) => Ok(Some(text.to_owned())),
        FieldValue::Other(_) => Err(invalid(
            path,
            &format!("{key}: input should be a valid string"),
        )),
    }
}

fn required_string_list(
    mapping: &Mapping,
    key: &str,
    path: &Path,
) -> Result<Vec<String>, CharterError> {
    match field(mapping, key) {
        FieldValue::Absent => Err(invalid(path, &format!("{key}: field required"))),
        FieldValue::Other(Value::Sequence(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => Ok(text.clone()),
                _ => Err(invalid(
                    path,
                    &format!("{key}: each item should be a valid string"),
                )),
            })
            .collect(),
        FieldValue::Null | FieldValue::String(_) | FieldValue::Other(_) => Err(invalid(
            path,
            &format!("{key}: input should be a valid list"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "artifact_urn: directive:PROJECT_001\n\
artifact_kind: directive\n\
artifact_slug: project-001\n\
artifact_content_hash: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
inputs_hash: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n\
adapter_id: claude\n\
adapter_version: '1.0.0'\n\
synthesizer_version: 4.0.0\n\
source_section: governance\n\
source_urns:\n\
- directive:PROJECT_001\n\
source_input_ids:\n\
- directive:PROJECT_001\n\
generated_at: '2026-01-01T00:00:00Z'\n\
produced_at: '2026-01-02T00:00:00Z'\n\
corpus_snapshot_id: '(none)'\n\
synthesis_run_id: 01J00000000000000000000001\n";

    fn write(dir: &Path, text: &str) -> std::path::PathBuf {
        let path = dir.join("directive-project-001.yaml");
        std::fs::write(&path, text).expect("write");
        path
    }

    #[test]
    fn a_v2_sidecar_round_trips_the_reference_fields() {
        let temp = tempfile::tempdir().expect("temp");
        let path = write(temp.path(), VALID);
        let entry = load_provenance(&path).expect("entry");
        assert_eq!(entry.schema_version, "2");
        assert_eq!(entry.artifact_kind, "directive");
        assert_eq!(entry.source_section.as_deref(), Some("governance"));
        assert_eq!(entry.source_urns, vec!["directive:PROJECT_001".to_owned()]);
        assert_eq!(entry.corpus_snapshot_id, "(none)");
        assert_eq!(entry.evidence_bundle_hash, None);
        assert_eq!(entry.adapter_notes, None);
        assert_eq!(entry.produced_at, "2026-01-02T00:00:00Z");
    }

    #[test]
    fn an_absent_schema_version_is_v2_and_a_null_section_is_allowed() {
        let temp = tempfile::tempdir().expect("temp");
        let text = VALID.replace("source_section: governance\n", "source_section: null\n");
        let entry = load_provenance(&write(temp.path(), &text)).expect("entry");
        assert_eq!(entry.schema_version, "2");
        assert_eq!(entry.source_section, None);
    }

    #[test]
    fn a_partial_sidecar_is_a_validation_failure_not_a_partial_entry() {
        let temp = tempfile::tempdir().expect("temp");
        let path = write(temp.path(), "artifact_urn: directive:PROJECT_001\n");
        let error = load_provenance(&path).expect_err("must validate");
        let text = error.to_string();
        assert!(text.contains("field required"), "{text}");
        assert!(text.contains("artifact_kind"), "{text}");
    }

    #[test]
    fn every_typed_rule_is_enforced() {
        let cases = [
            (
                VALID.replace("artifact_kind: directive", "artifact_kind: template"),
                "artifact_kind",
            ),
            (
                VALID.replace("synthesizer_version: 4.0.0", "synthesizer_version: ''"),
                "at least 1 character",
            ),
            (
                VALID.replace("corpus_snapshot_id: '(none)'", "corpus_snapshot_id: 7"),
                "valid string",
            ),
            (
                VALID
                    .replace("source_section: governance\n", "source_section: null\n")
                    .replace(
                        "source_urns:\n- directive:PROJECT_001\n",
                        "source_urns: []\n",
                    ),
                "at least one of",
            ),
            (
                VALID.replace(
                    "synthesis_run_id: 01J00000000000000000000001",
                    "synthesis_run_id: 01J00000000000000000000001\nschema_version: '1'",
                ),
                "schema_version",
            ),
        ];
        for (text, needle) in cases {
            let temp = tempfile::tempdir().expect("temp");
            let error = load_provenance(&write(temp.path(), &text)).expect_err(needle);
            assert!(error.to_string().contains(needle), "{needle}: {error}");
        }
    }

    #[test]
    fn unknown_keys_are_ignored_like_pydantic_extras() {
        let temp = tempfile::tempdir().expect("temp");
        let text = format!("{VALID}some_future_field: 1\n");
        assert!(load_provenance(&write(temp.path(), &text)).is_ok());
    }

    #[test]
    fn a_non_mapping_root_is_refused() {
        let temp = tempfile::tempdir().expect("temp");
        let error = load_provenance(&write(temp.path(), "- a\n- b\n")).expect_err("refused");
        assert!(error.to_string().contains("valid dictionary"), "{error}");
    }
}
