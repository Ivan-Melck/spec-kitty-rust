//! Charter-bundle schema versioning, v2 repairs and the unified-bundle seal.
//!
//! Source family:
//!
//! * `charter/offering/versioning.py` — `CURRENT_BUNDLE_SCHEMA_VERSION`,
//!   `check_bundle_compatibility`, `get_bundle_schema_version`,
//!   `migrate_v1_to_v2`, `repair_v2_synthesis_manifest_defaults`,
//!   `run_migration`.
//! * `charter/bundle.py` — `CANONICAL_MANIFEST` (v2.0.0: tracked
//!   `charter.md` + `charter.yaml`, optional `activation.yaml`, no derived
//!   files).
//! * `charter/activation/sync.py` — the (now inert) freshness chokepoint
//!   `ensure_charter_bundle_fresh`, whose only remaining effect is the
//!   staleness verdict.
//!
//! The two effects:
//!
//! * `3.2.0rc35_charter_bundle_v2` — upgrade a v1 bundle: add the mandatory
//!   Phase-7 provenance fields to every sidecar, stamp the v2 synthesis
//!   manifest (including its `manifest_hash`), and write
//!   `metadata.bundle_schema_version: 2` into `charter.yaml`.
//! * `3.2.0rc35_charter_manifest_defaults_repair` — repair a *current* v2
//!   manifest that predates the verifier-visible `built_in_only` default.
//!
//! Byte contract: the hash is `SHA-256(canonical_yaml(fields))` through
//! [`sk_charter::canonical_yaml_sha256_without`], the one canonical writer
//! verified against recorded Source bytes. In-place edits go through the one
//! round-trip CST editor ([`crate::yaml_edit::CstDocument`]) so comments and
//! authored key order survive, and untouched bytes are never reformatted.

use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;
use crate::yaml_edit::{CstDocument, InsertPosition, Scalar, YamlEditor};

/// The v2 bundle schema version (`CURRENT_BUNDLE_SCHEMA_VERSION`).
pub const CURRENT_BUNDLE_SCHEMA_VERSION: i64 = 2;
/// Oldest readable bundle schema (`MIN_READABLE_BUNDLE_SCHEMA`).
pub const MIN_READABLE_BUNDLE_SCHEMA: i64 = 1;
/// Newest readable bundle schema (`MAX_READABLE_BUNDLE_SCHEMA`).
pub const MAX_READABLE_BUNDLE_SCHEMA: i64 = 2;

/// Sentinel for fields that cannot be reconstructed from a v1 bundle.
pub const PRE_PHASE7_MIGRATION_SENTINEL: &str = "(pre-phase7-migration)";

/// `charter_dir`'s `charter.yaml` (the schema-version carrier).
fn charter_yaml(charter_dir: &Path) -> PathBuf {
    charter_dir.join("charter.yaml")
}

/// `charter_dir`'s synthesis manifest.
fn synthesis_manifest(charter_dir: &Path) -> PathBuf {
    charter_dir.join("synthesis-manifest.yaml")
}

/// Bundle compatibility verdict (`BundleCompatibilityResult`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleCompatibility {
    /// Status token, `COMPATIBLE`-style.
    pub status: BundleCompatibilityStatus,
    /// The read schema version, when present.
    pub bundle_version: Option<i64>,
    /// Minimum readable schema.
    pub supported_min: i64,
    /// Maximum readable schema.
    pub supported_max: i64,
    /// Operator-facing message.
    pub message: String,
    /// Process exit code the Source reports with this verdict.
    pub exit_code: i32,
}

/// Compatibility status (`BundleCompatibilityStatus`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleCompatibilityStatus {
    /// Version equals the current schema.
    Compatible,
    /// Version is readable but needs migration.
    NeedsMigration,
    /// Version predates the oldest readable schema.
    IncompatibleOld,
    /// Version is newer than this build reads.
    IncompatibleNew,
    /// The field is absent.
    MissingVersion,
}

impl BundleCompatibilityStatus {
    /// The Python enum's string value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "COMPATIBLE",
            Self::NeedsMigration => "NEEDS_MIGRATION",
            Self::IncompatibleOld => "INCOMPATIBLE_OLD",
            Self::IncompatibleNew => "INCOMPATIBLE_NEW",
            Self::MissingVersion => "MISSING_VERSION",
        }
    }
}

/// Pure compatibility check (`check_bundle_compatibility`).
pub fn check_bundle_compatibility(bundle_version: Option<i64>) -> BundleCompatibility {
    let supported_min = MIN_READABLE_BUNDLE_SCHEMA;
    let supported_max = MAX_READABLE_BUNDLE_SCHEMA;
    match bundle_version {
        None => BundleCompatibility {
            status: BundleCompatibilityStatus::MissingVersion,
            bundle_version: None,
            supported_min,
            supported_max,
            message: "Bundle schema version not found; treating as v1. Run `spec-kitty upgrade`."
                .to_string(),
            exit_code: 1,
        },
        Some(version) if version == CURRENT_BUNDLE_SCHEMA_VERSION => BundleCompatibility {
            status: BundleCompatibilityStatus::Compatible,
            bundle_version,
            supported_min,
            supported_max,
            message: format!("Bundle schema version {version} is supported."),
            exit_code: 0,
        },
        Some(version) if supported_min <= version && version < CURRENT_BUNDLE_SCHEMA_VERSION => {
            BundleCompatibility {
                status: BundleCompatibilityStatus::NeedsMigration,
                bundle_version,
                supported_min,
                supported_max,
                message: format!(
                    "Bundle schema version {version} requires migration. Run `spec-kitty upgrade`."
                ),
                exit_code: 1,
            }
        }
        Some(version) if version < supported_min => BundleCompatibility {
            status: BundleCompatibilityStatus::IncompatibleOld,
            bundle_version,
            supported_min,
            supported_max,
            message: format!(
                "Bundle schema version {version} predates the earliest supported version \
                 ({supported_min}). Contact support."
            ),
            exit_code: 1,
        },
        Some(version) => BundleCompatibility {
            status: BundleCompatibilityStatus::IncompatibleNew,
            bundle_version,
            supported_min,
            supported_max,
            message: format!(
                "Bundle schema version {version} is newer than this CLI supports \
                 ({supported_max}). Upgrade your CLI."
            ),
            exit_code: 1,
        },
    }
}

/// Read `metadata.bundle_schema_version` from `charter_dir/charter.yaml`.
///
/// Absent file/section/key, a null value or a non-integer all read as `None`
/// (the pinned function never raises).
pub fn get_bundle_schema_version(charter_dir: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(charter_yaml(charter_dir)).ok()?;
    let document: Value = serde_yaml_ng::from_str(&text).ok()?;
    let metadata = document.get("metadata")?;
    let value = metadata.get("bundle_schema_version")?;
    let version = value.as_i64()?;
    Some(version)
}

/// One versioning run (`MigrationResult`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VersioningResult {
    /// Files written (or that would be, under `dry_run`), as absolute paths.
    pub changes_made: Vec<String>,
    /// Non-fatal write/parse failures, in the Source's message shape.
    pub errors: Vec<String>,
    /// Schema version migrated from.
    pub from_version: i64,
    /// Schema version migrated to.
    pub to_version: i64,
}

impl VersioningResult {
    /// `true` when nothing was written.
    pub fn is_noop(&self) -> bool {
        self.changes_made.is_empty() && self.errors.is_empty()
    }
}

/// ISO-8601 UTC rendering of a file mtime, matching
/// `kernel.clock.from_epoch(mtime).isoformat()` (microsecond precision, and no
/// fractional part when it is exactly zero).
fn iso_from_mtime(path: &Path) -> Option<String> {
    use chrono::{DateTime, SecondsFormat, Utc};
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let stamp: DateTime<Utc> = modified.into();
    let nanos = stamp.timestamp_subsec_nanos();
    let format = if nanos == 0 {
        SecondsFormat::Secs
    } else {
        SecondsFormat::Micros
    };
    Some(stamp.to_rfc3339_opts(format, false))
}

/// Apply the v2 sidecar defaults in Source order.
///
/// `value` is the parsed original (used only for the `corpus_snapshot_id is
/// None` predicate, which covers both an absent key and an authored null).
fn apply_sidecar_defaults(document: &CstDocument, path: &Path, value: &Value) {
    let root = document.root();
    if !root.contains("synthesizer_version") {
        root.set_string("synthesizer_version", PRE_PHASE7_MIGRATION_SENTINEL);
    }
    if !root.contains("synthesis_run_id") {
        root.set_string("synthesis_run_id", PRE_PHASE7_MIGRATION_SENTINEL);
    }
    if !root.contains("produced_at") {
        let produced_at =
            iso_from_mtime(path).unwrap_or_else(|| PRE_PHASE7_MIGRATION_SENTINEL.to_string());
        root.set_string("produced_at", &produced_at);
    }
    if !root.contains("source_input_ids") {
        let urns = root.get_string_list("source_urns").unwrap_or_default();
        root.set_string_list("source_input_ids", &urns);
    }
    if value.get("corpus_snapshot_id").is_none_or(Value::is_null) {
        root.set_string("corpus_snapshot_id", "(none)");
    }
    root.set_string("schema_version", "2");
}

fn load_document(path: &Path) -> Result<CstDocument, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("Failed to load {}: {error}", path.display()))?;
    CstDocument::parse(path, &text)
        .map_err(|error| format!("Failed to load {}: {error}", path.display()))
}

fn write_document(path: &Path, document: &CstDocument) -> Result<(), String> {
    let text = document.text();
    std::fs::write(path, text)
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

/// `_migrate_provenance_sidecars`: v1 → v2 sidecar backfill.
fn migrate_provenance_sidecars(
    bundle_root: &Path,
    dry_run: bool,
    changes: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    let provenance = bundle_root.join("provenance");
    if !provenance.is_dir() {
        return;
    }
    let mut sidecars: Vec<PathBuf> = std::fs::read_dir(&provenance)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension().and_then(|ext| ext.to_str()) == Some("yaml") && path.is_file()
                })
                .collect()
        })
        .unwrap_or_default();
    sidecars.sort();
    for sidecar in sidecars {
        let document = match load_document(&sidecar) {
            Ok(document) => document,
            Err(error) => {
                errors.push(format!(
                    "Failed to load sidecar {}: {error}",
                    sidecar.file_name().unwrap_or_default().to_string_lossy()
                ));
                continue;
            }
        };
        if document.root().get_string("schema_version").as_deref() == Some("2") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&sidecar) else {
            errors.push(format!(
                "Failed to load sidecar {}: unreadable",
                sidecar.file_name().unwrap_or_default().to_string_lossy()
            ));
            continue;
        };
        let Ok(value) = serde_yaml_ng::from_str::<Value>(&text) else {
            errors.push(format!(
                "Failed to load sidecar {}: not valid YAML",
                sidecar.file_name().unwrap_or_default().to_string_lossy()
            ));
            continue;
        };
        apply_sidecar_defaults(&document, &sidecar, &value);
        changes.push(sidecar.display().to_string());
        if !dry_run && let Err(error) = write_document(&sidecar, &document) {
            errors.push(format!(
                "Failed to write sidecar {}: {error}",
                sidecar.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }
}

/// The canonical hash for a migrated v2 manifest (`_compute_v2_synthesis_manifest_hash`).
///
/// Fields are the on-disk mapping minus `manifest_hash`, with
/// `schema_version` forced to `"2"` and the two verifier-visible defaults
/// present, then hashed with the canonical writer.
pub fn v2_manifest_hash(manifest: &Value) -> Option<String> {
    let mapping = manifest.as_mapping()?;
    let mut fields = serde_yaml_ng::Mapping::new();
    for (key, value) in mapping {
        if key.as_str() == Some("manifest_hash") {
            continue;
        }
        fields.insert(key.clone(), value.clone());
    }
    fields.insert(
        Value::String("schema_version".to_string()),
        Value::String("2".to_string()),
    );
    fields
        .entry(Value::String("mission_id".to_string()))
        .or_insert(Value::Null);
    fields
        .entry(Value::String("built_in_only".to_string()))
        .or_insert(Value::Bool(false));
    Some(sk_charter::canonical_yaml_sha256(&Value::Mapping(fields)))
}

/// `_migrate_synthesis_manifest`: v1 → v2 manifest backfill.
fn migrate_synthesis_manifest(
    bundle_root: &Path,
    dry_run: bool,
    changes: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    let manifest_path = synthesis_manifest(bundle_root);
    if !manifest_path.is_file() {
        return;
    }
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => {
            errors.push(format!("Failed to load synthesis-manifest.yaml: {error}"));
            return;
        }
    };
    let value: Value = match serde_yaml_ng::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("Failed to load synthesis-manifest.yaml: {error}"));
            return;
        }
    };
    if value
        .get("schema_version")
        .and_then(|version| version.as_str())
        == Some("2")
        || !value.is_mapping()
    {
        return;
    }

    // The hash is over the fully defaulted field set (minus `manifest_hash`).
    let mut defaulted: serde_yaml_ng::Mapping = match value.as_mapping() {
        Some(mapping) => mapping.clone(),
        None => serde_yaml_ng::Mapping::new(),
    };
    defaulted
        .entry(Value::String("synthesizer_version".to_string()))
        .or_insert(Value::String(PRE_PHASE7_MIGRATION_SENTINEL.to_string()));
    defaulted
        .entry(Value::String("mission_id".to_string()))
        .or_insert(Value::Null);
    defaulted
        .entry(Value::String("built_in_only".to_string()))
        .or_insert(Value::Bool(false));
    defaulted.insert(
        Value::String("schema_version".to_string()),
        Value::String("2".to_string()),
    );
    let Some(hash) = v2_manifest_hash(&Value::Mapping(defaulted)) else {
        errors.push("Failed to compute synthesis-manifest.yaml manifest_hash.".to_string());
        return;
    };
    let editor = YamlEditor::new(&manifest_path, &text);
    let entries = vec![
        (
            "synthesizer_version".to_string(),
            Scalar::Str(PRE_PHASE7_MIGRATION_SENTINEL.to_string()),
        ),
        ("mission_id".to_string(), Scalar::Null),
        ("built_in_only".to_string(), Scalar::Bool(false)),
        ("schema_version".to_string(), Scalar::Str("2".to_string())),
        ("manifest_hash".to_string(), Scalar::Str(hash)),
    ];
    let outcome = match editor.upsert_scalars("", &entries, InsertPosition::Bottom) {
        Ok(outcome) => outcome,
        Err(error) => {
            errors.push(format!("Failed to write synthesis-manifest.yaml: {error}"));
            return;
        }
    };
    changes.push(manifest_path.display().to_string());
    if !dry_run
        && let Err(error) = std::fs::write(&manifest_path, &outcome.text)
            .map_err(|error| format!("Failed to write synthesis-manifest.yaml: {error}"))
    {
        errors.push(error);
    }
}

/// `_stamp_charter_bundle_version`: `metadata.bundle_schema_version: 2`.
fn stamp_charter_bundle_version(
    bundle_root: &Path,
    dry_run: bool,
    changes: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    let charter_path = charter_yaml(bundle_root);
    if !charter_path.is_file() {
        return;
    }
    let value: Value = match std::fs::read_to_string(&charter_path)
        .map_err(|error| error.to_string())
        .and_then(|text| serde_yaml_ng::from_str(&text).map_err(|error| error.to_string()))
    {
        Ok(value) => value,
        Err(error) => {
            errors.push(format!("Failed to load charter.yaml: {error}"));
            return;
        }
    };
    if !value.is_mapping() {
        return;
    }
    if value
        .get("metadata")
        .and_then(|metadata| metadata.get("bundle_schema_version"))
        .and_then(Value::as_i64)
        == Some(CURRENT_BUNDLE_SCHEMA_VERSION)
    {
        return;
    }

    let document = match load_document(&charter_path) {
        Ok(document) => document,
        Err(error) => {
            errors.push(error);
            return;
        }
    };
    let root = document.root();
    match root.get_mapping("metadata") {
        Some(metadata) => {
            if metadata.get_string("bundle_schema_version").as_deref() != Some("2") {
                metadata.set_raw("bundle_schema_version", "2");
            }
        }
        None if root.contains("metadata") => {
            // An authored non-mapping `metadata:` is not something this
            // migration may silently replace.
            errors.push(
                "Failed to stamp charter.yaml: metadata is not a mapping; bundle_schema_version \
                 was not written."
                    .to_string(),
            );
            return;
        }
        None => {
            root.set_raw("metadata", "bundle_schema_version: 2");
        }
    }
    changes.push(charter_path.display().to_string());
    if !dry_run && let Err(error) = write_document(&charter_path, &document) {
        errors.push(format!("Failed to write charter.yaml: {error}"));
    }
}

/// `migrate_v1_to_v2`.
pub fn migrate_v1_to_v2(bundle_root: &Path, dry_run: bool) -> VersioningResult {
    let mut result = VersioningResult {
        from_version: 1,
        to_version: 2,
        ..VersioningResult::default()
    };
    migrate_provenance_sidecars(
        bundle_root,
        dry_run,
        &mut result.changes_made,
        &mut result.errors,
    );
    migrate_synthesis_manifest(
        bundle_root,
        dry_run,
        &mut result.changes_made,
        &mut result.errors,
    );
    stamp_charter_bundle_version(
        bundle_root,
        dry_run,
        &mut result.changes_made,
        &mut result.errors,
    );
    result
}

/// `repair_v2_synthesis_manifest_defaults`.
pub fn repair_v2_synthesis_manifest_defaults(
    bundle_root: &Path,
    dry_run: bool,
) -> VersioningResult {
    let mut result = VersioningResult {
        from_version: 2,
        to_version: 2,
        ..VersioningResult::default()
    };
    let manifest_path = synthesis_manifest(bundle_root);
    if !manifest_path.is_file() {
        return result;
    }
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        Err(error) => {
            result
                .errors
                .push(format!("Failed to load synthesis-manifest.yaml: {error}"));
            return result;
        }
    };
    let value: Value = match serde_yaml_ng::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            result
                .errors
                .push(format!("Failed to load synthesis-manifest.yaml: {error}"));
            return result;
        }
    };
    if !value.is_mapping()
        || value.get("schema_version").and_then(Value::as_str) != Some("2")
        || value.get("built_in_only").is_some()
    {
        return result;
    }
    let Some(stored_hash) = value.get("manifest_hash").and_then(Value::as_str) else {
        result.errors.push(
            "Cannot repair synthesis-manifest.yaml: manifest_hash is missing or invalid."
                .to_string(),
        );
        return result;
    };
    let legacy_hash = sk_charter::canonical_yaml_sha256_without(&value, &["manifest_hash"]);
    if legacy_hash != stored_hash {
        result.errors.push(
            "Cannot repair synthesis-manifest.yaml: existing manifest_hash does not match the \
             pre-built_in_only v2 payload."
                .to_string(),
        );
        return result;
    }

    let mut repaired: serde_yaml_ng::Mapping = match value.as_mapping() {
        Some(mapping) => mapping.clone(),
        None => serde_yaml_ng::Mapping::new(),
    };
    repaired.insert(
        Value::String("built_in_only".to_string()),
        Value::Bool(false),
    );
    let Some(hash) = v2_manifest_hash(&Value::Mapping(repaired)) else {
        result
            .errors
            .push("Failed to compute synthesis-manifest.yaml manifest_hash.".to_string());
        return result;
    };
    let editor = YamlEditor::new(&manifest_path, &text);
    let entries = vec![
        ("built_in_only".to_string(), Scalar::Bool(false)),
        ("manifest_hash".to_string(), Scalar::Str(hash)),
    ];
    let outcome = match editor.upsert_scalars("", &entries, InsertPosition::Bottom) {
        Ok(outcome) => outcome,
        Err(error) => {
            result
                .errors
                .push(format!("Failed to write synthesis-manifest.yaml: {error}"));
            return result;
        }
    };
    result
        .changes_made
        .push(manifest_path.display().to_string());
    if !dry_run && let Err(error) = std::fs::write(&manifest_path, &outcome.text) {
        result
            .errors
            .push(format!("Failed to write synthesis-manifest.yaml: {error}"));
        result.changes_made.clear();
    }
    result
}

/// `run_migration`: dispatch on the from-version.
pub fn run_migration(
    from_version: i64,
    bundle_root: &Path,
    dry_run: bool,
) -> Option<VersioningResult> {
    match from_version {
        1 => Some(migrate_v1_to_v2(bundle_root, dry_run)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The two upgrade effects
// ---------------------------------------------------------------------------

fn charter_dir(project_root: &Path) -> PathBuf {
    project_root.join(".kittify").join("charter")
}

fn incompatible(compatibility: &BundleCompatibility) -> bool {
    matches!(
        compatibility.status,
        BundleCompatibilityStatus::IncompatibleOld | BundleCompatibilityStatus::IncompatibleNew
    )
}

fn outcome_from(result: &VersioningResult) -> MigrationOutcome {
    let mut changes: Vec<String> = result.changes_made.clone();
    if changes.is_empty() && result.errors.is_empty() {
        changes.push("Charter bundle already at v2; nothing to migrate".to_string());
    }
    MigrationOutcome {
        changed: !result.changes_made.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    }
}

/// `3.2.0rc35_charter_bundle_v2` — `detect()`.
pub fn detect_charter_bundle_v2(project_root: &Path) -> Result<bool, InstallError> {
    let dir = charter_dir(project_root);
    if !dir.is_dir() {
        return Ok(false);
    }
    let Some(version) = get_bundle_schema_version(&dir) else {
        return Ok(true);
    };
    if version < CURRENT_BUNDLE_SCHEMA_VERSION {
        return Ok(true);
    }
    if version == CURRENT_BUNDLE_SCHEMA_VERSION {
        let repair = repair_v2_synthesis_manifest_defaults(&dir, true);
        return Ok(!repair.changes_made.is_empty() || !repair.errors.is_empty());
    }
    Ok(false)
}

/// `3.2.0rc35_charter_bundle_v2` — `can_apply()`.
pub fn can_apply_charter_bundle_v2(project_root: &Path) -> Result<Option<String>, InstallError> {
    let dir = charter_dir(project_root);
    if !dir.is_dir() {
        return Ok(Some(
            "No charter directory found at .kittify/charter".to_string(),
        ));
    }
    let compatibility = check_bundle_compatibility(get_bundle_schema_version(&dir));
    if incompatible(&compatibility) {
        return Ok(Some(compatibility.message));
    }
    Ok(None)
}

/// `3.2.0rc35_charter_bundle_v2` — `apply()`.
pub fn apply_charter_bundle_v2(
    project_root: &Path,
    dry_run: bool,
) -> Result<MigrationOutcome, InstallError> {
    let dir = charter_dir(project_root);
    let compatibility = check_bundle_compatibility(get_bundle_schema_version(&dir));
    if incompatible(&compatibility) {
        return Err(InstallError::UpgradeRefused {
            detail: compatibility.message,
        });
    }
    let current = get_bundle_schema_version(&dir).unwrap_or(1);
    if current >= CURRENT_BUNDLE_SCHEMA_VERSION {
        let repair = repair_v2_synthesis_manifest_defaults(&dir, dry_run);
        if !repair.errors.is_empty() {
            return Err(InstallError::UpgradeRefused {
                detail: repair.errors.join("; "),
            });
        }
        return Ok(outcome_from(&repair));
    }

    let mut merged = VersioningResult {
        changes_made: Vec::new(),
        errors: Vec::new(),
        from_version: current,
        to_version: CURRENT_BUNDLE_SCHEMA_VERSION,
    };
    let mut version = current;
    while version < CURRENT_BUNDLE_SCHEMA_VERSION {
        let Some(result) = run_migration(version, &dir, dry_run) else {
            merged.errors.push(format!(
                "No migration registered for bundle version {version}"
            ));
            break;
        };
        merged.changes_made.extend(result.changes_made);
        merged.errors.extend(result.errors);
        version += 1;
    }
    if merged.errors.is_empty() {
        let repair = repair_v2_synthesis_manifest_defaults(&dir, dry_run);
        merged.changes_made.extend(repair.changes_made);
        merged.errors.extend(repair.errors);
    }
    if !merged.errors.is_empty() {
        return Err(InstallError::UpgradeRefused {
            detail: merged.errors.join("; "),
        });
    }
    Ok(outcome_from(&merged))
}

/// `3.2.0rc35_charter_manifest_defaults_repair` — `detect()`.
pub fn detect_charter_manifest_defaults_repair(project_root: &Path) -> Result<bool, InstallError> {
    let dir = charter_dir(project_root);
    if !dir.is_dir() {
        return Ok(false);
    }
    if get_bundle_schema_version(&dir) != Some(CURRENT_BUNDLE_SCHEMA_VERSION) {
        return Ok(false);
    }
    let repair = repair_v2_synthesis_manifest_defaults(&dir, true);
    Ok(!repair.changes_made.is_empty() || !repair.errors.is_empty())
}

/// `3.2.0rc35_charter_manifest_defaults_repair` — `can_apply()`.
pub fn can_apply_charter_manifest_defaults_repair(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    let dir = charter_dir(project_root);
    if !dir.is_dir() {
        return Ok(Some(
            "No charter directory found at .kittify/charter".to_string(),
        ));
    }
    let version = get_bundle_schema_version(&dir);
    let compatibility = check_bundle_compatibility(version);
    if incompatible(&compatibility) {
        return Ok(Some(compatibility.message));
    }
    if version != Some(CURRENT_BUNDLE_SCHEMA_VERSION) {
        return Ok(Some(
            "Charter bundle is not at current v2 schema.".to_string(),
        ));
    }
    Ok(None)
}

/// `3.2.0rc35_charter_manifest_defaults_repair` — `apply()`.
pub fn apply_charter_manifest_defaults_repair(
    project_root: &Path,
    dry_run: bool,
) -> Result<MigrationOutcome, InstallError> {
    let repair = repair_v2_synthesis_manifest_defaults(&charter_dir(project_root), dry_run);
    if !repair.errors.is_empty() {
        return Err(InstallError::UpgradeRefused {
            detail: repair.errors.join("; "),
        });
    }
    Ok(outcome_from(&repair))
}

// ---------------------------------------------------------------------------
// 3.2.0rc35_unified_bundle
// ---------------------------------------------------------------------------

/// The v2.0.0 canonical bundle manifest's tracked files.
///
/// `charter.md` + `charter.yaml` are required; `activation.yaml` is optional
/// (`CANONICAL_MANIFEST` v2.0.0 has no derived files).
pub const CANONICAL_TRACKED_FILES: [&str; 2] = [
    ".kittify/charter/charter.md",
    ".kittify/charter/charter.yaml",
];
/// Optional tracked members of the canonical bundle.
pub const CANONICAL_OPTIONAL_TRACKED_FILES: [&str; 1] = [".kittify/charter/activation.yaml"];

/// The structured report `3.2.0rc35_unified_bundle` returns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnifiedBundleReport {
    /// `migration_id`.
    pub migration_id: String,
    /// `target_version`.
    pub target_version: String,
    /// Whether the chokepoint reported a write.
    pub applied: bool,
    /// Whether `charter.md` exists at the canonical root.
    pub charter_present: bool,
    /// Bundle validation block.
    pub bundle_validation: BundleValidation,
    /// Whether the freshness chokepoint refreshed anything.
    pub chokepoint_refreshed: bool,
    /// Errors (the Source's `sync()` error, if any).
    pub errors: Vec<String>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

/// The `bundle_validation` block.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BundleValidation {
    /// No required tracked or derived file is missing.
    pub passed: bool,
    /// Missing tracked files, canonical-root relative.
    pub missing_tracked: Vec<String>,
    /// Missing derived files (empty for v2.0.0).
    pub missing_derived: Vec<String>,
    /// Files under `.kittify/charter/` the manifest does not declare.
    pub unexpected: Vec<String>,
}

impl UnifiedBundleReport {
    /// The report in the contract's JSON shape (keys sorted, compact).
    pub fn to_json(&self) -> String {
        let mut object = serde_json::Map::new();
        object.insert("applied".to_string(), serde_json::Value::Bool(self.applied));
        object.insert("bundle_validation".to_string(), {
            let mut validation = serde_json::Map::new();
            validation.insert(
                "missing_derived".to_string(),
                serde_json::Value::Array(
                    self.bundle_validation
                        .missing_derived
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
            validation.insert(
                "missing_tracked".to_string(),
                serde_json::Value::Array(
                    self.bundle_validation
                        .missing_tracked
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
            validation.insert(
                "passed".to_string(),
                serde_json::Value::Bool(self.bundle_validation.passed),
            );
            validation.insert(
                "unexpected".to_string(),
                serde_json::Value::Array(
                    self.bundle_validation
                        .unexpected
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
            serde_json::Value::Object(validation)
        });
        object.insert(
            "charter_present".to_string(),
            serde_json::Value::Bool(self.charter_present),
        );
        object.insert(
            "chokepoint_refreshed".to_string(),
            serde_json::Value::Bool(self.chokepoint_refreshed),
        );
        object.insert(
            "duration_ms".to_string(),
            serde_json::Value::Number(self.duration_ms.into()),
        );
        object.insert(
            "errors".to_string(),
            serde_json::Value::Array(
                self.errors
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
        object.insert(
            "migration_id".to_string(),
            serde_json::Value::String(self.migration_id.clone()),
        );
        object.insert(
            "target_version".to_string(),
            serde_json::Value::String(self.target_version.clone()),
        );
        serde_json::Value::Object(object).to_string()
    }
}

/// `detect()`: main checkout only (`.git` is a file in a linked worktree).
pub fn detect_unified_bundle(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!project_root.join(".git").is_file())
}

/// `can_apply()`: always applicable.
pub fn can_apply_unified_bundle(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(None)
}

/// `ensure_charter_bundle_fresh` as it exists today: report staleness, write
/// nothing (`sync()` is inert).
fn chokepoint_error(canonical_root: &Path) -> Option<String> {
    let charter_path = canonical_root
        .join(".kittify")
        .join("charter")
        .join("charter.md");
    if !charter_path.is_file() {
        return None;
    }
    let content = match std::fs::read_to_string(&charter_path) {
        Ok(content) => content,
        Err(error) => return Some(error.to_string()),
    };
    // `sync()` only reads the charter and reports `stale_before`; no write can
    // fail, so the only reachable Source error is a read/decode failure.
    let _ = content;
    None
}

/// `3.2.0rc35_unified_bundle` — `apply()`.
pub fn apply_unified_bundle(
    project_root: &Path,
    dry_run: bool,
) -> Result<(UnifiedBundleReport, MigrationOutcome), InstallError> {
    let started = std::time::Instant::now();
    let mut errors = Vec::new();
    let charter_dir = charter_dir(project_root);
    let charter_md = charter_dir.join("charter.md");
    let charter_present = charter_md.is_file();

    if !charter_present {
        let report = UnifiedBundleReport {
            migration_id: "3.2.0rc35_unified_bundle".to_string(),
            target_version: "3.2.0rc35".to_string(),
            applied: false,
            charter_present: false,
            bundle_validation: BundleValidation {
                passed: true,
                ..BundleValidation::default()
            },
            chokepoint_refreshed: false,
            errors,
            duration_ms: elapsed_ms(started),
        };
        let mut outcome = MigrationOutcome::unchanged(vec![report.to_json()]);
        outcome.changed = false;
        return Ok((report, outcome));
    }

    if !dry_run && let Some(error) = chokepoint_error(project_root) {
        errors.push(error);
    }
    // `sync()` is inert in the pinned Source: `ensure_charter_bundle_fresh`
    // can never report `synced`, so neither flag is ever raised by a real run.
    let applied = false;
    let chokepoint_refreshed = false;

    let missing_tracked: Vec<String> = CANONICAL_TRACKED_FILES
        .iter()
        .filter(|relative| !project_root.join(relative).exists())
        .map(|relative| relative.to_string())
        .collect();
    // `CANONICAL_MANIFEST.derived_files` is empty in v2.0.0.
    let missing_derived: Vec<String> = Vec::new();

    let declared: std::collections::BTreeSet<&str> = CANONICAL_TRACKED_FILES
        .iter()
        .copied()
        .chain(CANONICAL_OPTIONAL_TRACKED_FILES.iter().copied())
        .collect();
    let mut unexpected: Vec<String> = Vec::new();
    if charter_dir.is_dir() {
        collect_files(&charter_dir, &mut |path| {
            let Ok(relative) = path.strip_prefix(project_root) else {
                return;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if !declared.contains(relative.as_str()) {
                unexpected.push(relative);
            }
        })?;
    }
    unexpected.sort();

    let passed = missing_tracked.is_empty() && missing_derived.is_empty();
    let report = UnifiedBundleReport {
        migration_id: "3.2.0rc35_unified_bundle".to_string(),
        target_version: "3.2.0rc35".to_string(),
        applied,
        charter_present,
        bundle_validation: BundleValidation {
            passed,
            missing_tracked,
            missing_derived,
            unexpected,
        },
        chokepoint_refreshed,
        errors,
        duration_ms: elapsed_ms(started),
    };
    let mut outcome = MigrationOutcome::unchanged(vec![report.to_json()]);
    outcome.changed = false;
    Ok((report, outcome))
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn collect_files(directory: &Path, visit: &mut impl FnMut(&Path)) -> Result<(), InstallError> {
    let mut children: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|error| InstallError::io("read", directory, error))?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        let metadata = std::fs::symlink_metadata(&child)
            .map_err(|error| InstallError::io("inspect", &child, error))?;
        if metadata.is_dir() {
            collect_files(&child, visit)?;
        } else if metadata.is_file() {
            visit(&child);
        }
    }
    Ok(())
}
