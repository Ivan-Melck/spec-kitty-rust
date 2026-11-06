//! The compatibility-shim registry (`docs/migrations/shim-registry.yaml`).
//!
//! One authored entry declares a legacy dotted module that re-exports a
//! canonical import, the release the shim appeared in, the release it must be
//! gone by, its tracker reference and whether it is grandfathered. The
//! diagnostic answers one question: **has a removal release shipped while the
//! shim file is still on disk?**
//!
//! # Status (Source's four states, in Source's precedence order)
//!
//! | status | rule |
//! | --- | --- |
//! | `grandfathered` | `grandfathered: true` — wins over everything |
//! | `removed` | the shim module is absent from `src/` |
//! | `overdue` | the shim exists **and** `project.version >= removal_target_release` |
//! | `pending` | otherwise |
//!
//! `overdue` is the only finding, and it is the only exit-1 path. A missing
//! registry, a missing `pyproject.toml`, a missing version key or any schema
//! violation is a configuration error (exit 2) — never a clean bill of health.
//!
//! # Schema (Source `compat/registry.py`, preserved exactly)
//!
//! Required per-entry keys: `legacy_path`, `canonical_import`,
//! `introduced_in_release`, `removal_target_release`, `tracker_issue`,
//! `grandfathered`. Optional: `extension_rationale`, `notes`. Unknown keys are
//! refused (a typo must not silently disable a check). `legacy_path` values are
//! unique. `canonical_import` is a dotted identifier or a non-empty list of
//! them. Release fields must be semver-shaped **and** orderable under PEP 440
//! (`removal_target_release >= introduced_in_release`). `tracker_issue` is
//! `#123` or an `http(s)` URL. `grandfathered` is a real boolean.
//!
//! # Read-only
//!
//! The only filesystem effects are `exists()` probes for the shim modules and
//! reads of the registry/`pyproject.toml`. Nothing is written, created or
//! locked; a remediation is printed for the operator, never performed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_kernel::PythonString;
use sk_kernel::python_text::is_whitespace;
use sk_kernel::python_yaml::{GraphDocument, NodeRef};

use super::{
    RegistryDiagnosticsError, RegistryKind, is_dotted_identifier, is_release_field,
    is_tracker_reference, load_document, message_text, parse_version, path_text,
    read_project_version, scalar_text,
};

/// The registry's canonical home inside a project.
pub const SHIM_REGISTRY_REL: &str = "docs/migrations/shim-registry.yaml";

/// Source's required per-entry keys.
pub const REQUIRED_KEYS: [&str; 6] = [
    "legacy_path",
    "canonical_import",
    "introduced_in_release",
    "removal_target_release",
    "tracker_issue",
    "grandfathered",
];

/// Source's optional per-entry keys.
pub const OPTIONAL_KEYS: [&str; 2] = ["extension_rationale", "notes"];

/// `ShimStatus.PENDING`
pub const STATUS_PENDING: &str = "pending";
/// `ShimStatus.OVERDUE`
pub const STATUS_OVERDUE: &str = "overdue";
/// `ShimStatus.GRANDFATHERED`
pub const STATUS_GRANDFATHERED: &str = "grandfathered";
/// `ShimStatus.REMOVED`
pub const STATUS_REMOVED: &str = "removed";

/// Source's release-field message (both release fields share it).
pub const RELEASE_FIELD_REASON: &str = "must be a semver string like '1.2.3' or '1.2.3a1'";

/// The canonical registry path (returned whether or not it exists, so the
/// caller raises a forward-correct not-found naming this home).
pub fn resolve_shim_registry_path(root: &Path) -> PathBuf {
    root.join(SHIM_REGISTRY_REL)
}

/// `canonical_import`, which Source models as `str | list[str]`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalImport {
    /// A single dotted identifier.
    Single(PythonString),
    /// Several canonical imports (a shim re-exporting more than one name).
    Many(Vec<PythonString>),
}

impl CanonicalImport {
    /// The imports in order, whatever the authored shape.
    pub fn imports(&self) -> Vec<&PythonString> {
        match self {
            Self::Single(import) => vec![import],
            Self::Many(imports) => imports.iter().collect(),
        }
    }

    /// The human rendering Source prints in its table (`", "`-joined).
    pub fn joined(&self) -> String {
        self.imports()
            .into_iter()
            .map(message_text)
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn to_json(&self) -> Result<Value, RegistryDiagnosticsError> {
        let reason = "a canonical import is not scalar text";
        match self {
            Self::Single(import) => Ok(Value::String(scalar_text(import, reason)?)),
            Self::Many(imports) => Ok(Value::Array(
                imports
                    .iter()
                    .map(|import| Ok(Value::String(scalar_text(import, reason)?)))
                    .collect::<Result<Vec<_>, RegistryDiagnosticsError>>()?,
            )),
        }
    }
}

/// One validated registry entry (Source `ShimEntry`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShimEntry {
    /// The legacy dotted module path.
    pub legacy_path: PythonString,
    /// The canonical import(s) it re-exports.
    pub canonical_import: CanonicalImport,
    /// The release the shim was introduced in.
    pub introduced_in_release: PythonString,
    /// The release it must be gone by.
    pub removal_target_release: PythonString,
    /// The tracker reference (`#123` or a URL).
    pub tracker_issue: PythonString,
    /// Whether the entry is permanently exempt from the overdue rule.
    pub grandfathered: bool,
    /// Optional extension rationale.
    pub extension_rationale: Option<PythonString>,
    /// Optional free-form notes.
    pub notes: Option<PythonString>,
}

/// Source `ShimStatus` — the four states, with their wire spellings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShimStatus {
    /// The removal release has not shipped yet.
    Pending,
    /// The removal release shipped and the shim is still present.
    Overdue,
    /// Permanently exempt.
    Grandfathered,
    /// The shim module is gone.
    Removed,
}

impl ShimStatus {
    /// The stable wire spelling (`"pending"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => STATUS_PENDING,
            Self::Overdue => STATUS_OVERDUE,
            Self::Grandfathered => STATUS_GRANDFATHERED,
            Self::Removed => STATUS_REMOVED,
        }
    }
}

/// One entry plus its classification and the observed shim-file presence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShimStatusEntry {
    /// The validated entry.
    pub entry: ShimEntry,
    /// Its status.
    pub status: ShimStatus,
    /// Whether `src/<legacy_path>.py` or `src/<legacy_path>/__init__.py` exists.
    pub shim_exists: bool,
}

/// Source `ShimRegistryReport` — a classified snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShimRegistryReport {
    /// Every entry, in authored order.
    pub entries: Vec<ShimStatusEntry>,
    /// `project.version` from `pyproject.toml`, verbatim.
    pub project_version: String,
    /// The registry path that was read.
    pub registry_path: PathBuf,
}

impl ShimRegistryReport {
    /// True when at least one shim is overdue.
    pub fn has_overdue(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.status == ShimStatus::Overdue)
    }

    /// The command's recommended exit code (`1` when anything is overdue).
    pub fn recommended_exit_code(&self) -> u8 {
        u8::from(self.has_overdue())
    }

    /// The `--json` document, with Source's exact key set and values.
    pub fn to_json(&self) -> Result<Value, RegistryDiagnosticsError> {
        let reason = "a registry entry is not scalar text";
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let mut row = Map::new();
            row.insert(
                "legacy_path".to_owned(),
                Value::String(scalar_text(&entry.entry.legacy_path, reason)?),
            );
            row.insert(
                "canonical_import".to_owned(),
                entry.entry.canonical_import.to_json()?,
            );
            row.insert(
                "removal_target_release".to_owned(),
                Value::String(scalar_text(&entry.entry.removal_target_release, reason)?),
            );
            row.insert(
                "grandfathered".to_owned(),
                Value::Bool(entry.entry.grandfathered),
            );
            row.insert(
                "tracker_issue".to_owned(),
                Value::String(scalar_text(&entry.entry.tracker_issue, reason)?),
            );
            row.insert(
                "status".to_owned(),
                Value::String(entry.status.as_str().to_owned()),
            );
            row.insert("shim_exists".to_owned(), Value::Bool(entry.shim_exists));
            entries.push(Value::Object(row));
        }
        let mut document = Map::new();
        document.insert(
            "project_version".to_owned(),
            Value::String(self.project_version.clone()),
        );
        document.insert(
            "registry_path".to_owned(),
            Value::String(path_text(&self.registry_path)?),
        );
        document.insert("entries".to_owned(), Value::Array(entries));
        document.insert("has_overdue".to_owned(), Value::Bool(self.has_overdue()));
        document.insert("exit_code".to_owned(), json!(self.recommended_exit_code()));
        Ok(Value::Object(document))
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load and validate the registry into typed entries.
pub fn load_shim_registry(root: &Path) -> Result<Vec<ShimEntry>, RegistryDiagnosticsError> {
    let path = resolve_shim_registry_path(root);
    let document = load_document(RegistryKind::Shim, &path)?;
    validate_shim_registry(&document)
}

/// Validate one decoded registry document and project it into entries.
pub fn validate_shim_registry(
    document: &GraphDocument,
) -> Result<Vec<ShimEntry>, RegistryDiagnosticsError> {
    let root = document.root();
    if root.as_mapping().is_none() {
        return Err(RegistryKind::Shim.schema_failure(vec![MISSING_CONTAINER.to_owned()]));
    }
    let Some(shims) = root.lookup("shims") else {
        return Err(RegistryKind::Shim.schema_failure(vec![MISSING_CONTAINER.to_owned()]));
    };
    let Some(items) = shims.as_sequence() else {
        return Err(
            RegistryKind::Shim.schema_failure(vec!["top-level.shims: must be a list".to_owned()])
        );
    };

    let mut errors = Vec::new();
    let mut seen_paths: BTreeSet<String> = BTreeSet::new();
    let mut entries = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(entry) = validate_entry(index, shims.view(*item), &mut seen_paths, &mut errors)
        {
            entries.push(entry);
        }
    }
    if errors.is_empty() {
        Ok(entries)
    } else {
        Err(RegistryKind::Shim.schema_failure(errors))
    }
}

const MISSING_CONTAINER: &str = "top-level: must be a mapping with a 'shims' key";

/// Classify one entry (Source `_classify`).
pub fn classify(entry: &ShimEntry, current: &crate::version::Version, exists: bool) -> ShimStatus {
    if entry.grandfathered {
        return ShimStatus::Grandfathered;
    }
    if !exists {
        return ShimStatus::Removed;
    }
    let removal = message_text(&entry.removal_target_release);
    match parse_version(&removal) {
        // Validation guarantees a PEP 440-parseable removal target; an
        // unparseable one can only appear if validation was bypassed.
        Some(removal) if *current >= removal => ShimStatus::Overdue,
        _ => ShimStatus::Pending,
    }
}

/// Whether `src/<dotted path>.py` or `src/<dotted path>/__init__.py` exists.
///
/// Source `_shim_exists`:
/// `(repo_root / "src" / Path(*parts)).with_suffix(".py").exists()
/// or (base / "__init__.py").exists()`. A symlink counts and an unreadable
/// entry does not — exactly `Path.exists()`.
pub fn shim_exists(root: &Path, legacy_path: &PythonString) -> bool {
    let mut base = root.join("src");
    for part in message_text(legacy_path).split('.') {
        base.push(part);
    }
    base.with_extension("py").exists() || base.join("__init__.py").exists()
}

/// Load, classify and report (Source `check_shim_registry`).
///
/// The order is Source's: the project version is read first (a missing
/// `pyproject.toml` or key is the first error), then the registry is loaded and
/// validated, then the version string is parsed for the comparison.
pub fn check_shim_registry(root: &Path) -> Result<ShimRegistryReport, RegistryDiagnosticsError> {
    let project_version = read_project_version(root)?;
    let registry_path = resolve_shim_registry_path(root);
    let entries = load_shim_registry(root)?;
    let current =
        parse_version(&project_version).ok_or_else(|| {
            RegistryDiagnosticsError::ProjectVersionInvalid {
                detail: format!(
                    "project.version {project_version:?} in pyproject.toml is not a valid PEP 440 version"
                ),
            }
        })?;

    let status_entries = entries
        .into_iter()
        .map(|entry| {
            let exists = shim_exists(root, &entry.legacy_path);
            let status = classify(&entry, &current, exists);
            ShimStatusEntry {
                entry,
                status,
                shim_exists: exists,
            }
        })
        .collect();

    Ok(ShimRegistryReport {
        entries: status_entries,
        project_version,
        registry_path,
    })
}

// ---------------------------------------------------------------------------
// Validation (Source `_validate_entry` order and wording)
// ---------------------------------------------------------------------------

fn is_known_key(name: &str) -> bool {
    REQUIRED_KEYS.contains(&name) || OPTIONAL_KEYS.contains(&name)
}

/// The mapping's string keys, deduplicated in Python-sorted order.
fn string_keys(node: NodeRef<'_>) -> BTreeSet<String> {
    node.as_mapping()
        .unwrap_or_default()
        .iter()
        .filter_map(|(key, _)| node.view(*key).as_str().map(message_text))
        .collect()
}

/// Non-string mapping keys, rendered by `repr`.
///
/// Source's `sorted(set(entry) - known)` would raise `TypeError` on a mixed
/// str/int key set. Reporting the key by its `repr` keeps the document
/// refusable without a panic, and never lets a dict with an unusable key pass.
fn non_string_keys(node: NodeRef<'_>) -> Vec<String> {
    node.as_mapping()
        .unwrap_or_default()
        .iter()
        .filter(|(key, _)| node.view(*key).as_str().is_none())
        .map(|(key, _)| node.view(*key).repr())
        .collect()
}

fn validate_dotted_field(
    index: usize,
    node: NodeRef<'_>,
    key: &str,
    errors: &mut Vec<String>,
) -> Option<PythonString> {
    let value = node.lookup(key)?;
    match value.as_str() {
        Some(text) if is_dotted_identifier(text) => Some(text.clone()),
        _ => {
            errors.push(format!(
                "entry[{index}].{key}: must be a dotted identifier string"
            ));
            None
        }
    }
}

fn validate_entry(
    index: usize,
    node: NodeRef<'_>,
    seen_paths: &mut BTreeSet<String>,
    errors: &mut Vec<String>,
) -> Option<ShimEntry> {
    if node.as_mapping().is_none() {
        errors.push(format!("entry[{index}]: must be a mapping"));
        return None;
    }

    for key in non_string_keys(node) {
        errors.push(format!("entry[{index}].{key}: unknown field"));
    }
    let names = string_keys(node);
    for name in names.iter().filter(|name| !is_known_key(name)) {
        errors.push(format!("entry[{index}].{name}: unknown field"));
    }
    let missing: Vec<&str> = REQUIRED_KEYS
        .into_iter()
        .filter(|key| !names.contains(*key))
        .collect();
    for key in &missing {
        errors.push(format!("entry[{index}].{key}: required field is missing"));
    }
    if !missing.is_empty() {
        return None;
    }

    let legacy_path = validate_dotted_field(index, node, "legacy_path", errors);
    if let Some(legacy_path) = &legacy_path {
        let name = message_text(legacy_path);
        if !seen_paths.insert(name.clone()) {
            errors.push(format!(
                "entry[{index}].legacy_path: duplicate value '{name}'"
            ));
        }
    }

    let canonical_import =
        validate_canonical_import(index, node.lookup("canonical_import"), errors);

    let mut releases: [Option<PythonString>; 2] = [None, None];
    for (slot, key) in ["introduced_in_release", "removal_target_release"]
        .into_iter()
        .enumerate()
    {
        let Some(value) = node.lookup(key) else {
            continue;
        };
        match value.as_str() {
            Some(text) if is_release_field(text) => releases[slot] = Some(text.clone()),
            _ => errors.push(format!("entry[{index}].{key}: {RELEASE_FIELD_REASON}")),
        }
    }
    validate_version_order(index, &releases, errors);

    let tracker_issue = match node.lookup("tracker_issue").and_then(NodeRef::as_str) {
        Some(text) if is_tracker_reference(text) => Some(text.clone()),
        _ => {
            errors.push(format!(
                "entry[{index}].tracker_issue: must be '#123' or a URL"
            ));
            None
        }
    };

    let grandfathered = match node.lookup("grandfathered") {
        Some(value) => match value.as_bool() {
            Some(flag) => Some(flag),
            None => {
                errors.push(format!(
                    "entry[{index}].grandfathered: must be a boolean (true/false), got {}",
                    value.type_name()
                ));
                None
            }
        },
        None => None,
    };

    let extension_rationale = node.lookup("extension_rationale").and_then(|value| {
        if value.is_null() {
            return None;
        }
        match value.as_str() {
            Some(text) if !is_blank(text) => Some(text.clone()),
            _ => {
                errors.push(format!(
                    "entry[{index}].extension_rationale: if present, must be a non-empty string"
                ));
                None
            }
        }
    });

    let notes = node.lookup("notes").and_then(|value| {
        if value.is_null() {
            return None;
        }
        match value.as_str() {
            Some(text) => Some(text.clone()),
            None => {
                errors.push(format!(
                    "entry[{index}].notes: if present, must be a string"
                ));
                None
            }
        }
    });

    Some(ShimEntry {
        legacy_path: legacy_path?,
        canonical_import: canonical_import?,
        introduced_in_release: releases[0].clone()?,
        removal_target_release: releases[1].clone()?,
        tracker_issue: tracker_issue?,
        grandfathered: grandfathered?,
        extension_rationale,
        notes,
    })
}

fn is_blank(text: &PythonString) -> bool {
    text.codepoints().iter().all(|code| is_whitespace(*code))
}

fn validate_canonical_import(
    index: usize,
    value: Option<NodeRef<'_>>,
    errors: &mut Vec<String>,
) -> Option<CanonicalImport> {
    let value = value?;
    if let Some(text) = value.as_str() {
        if is_dotted_identifier(text) {
            return Some(CanonicalImport::Single(text.clone()));
        }
        errors.push(format!(
            "entry[{index}].canonical_import: must be a dotted identifier string"
        ));
        return None;
    }
    let Some(items) = value.as_sequence() else {
        errors.push(format!(
            "entry[{index}].canonical_import: must be a string or list of strings"
        ));
        return None;
    };
    if items.is_empty() {
        errors.push(format!(
            "entry[{index}].canonical_import: list must not be empty"
        ));
        return None;
    }
    let mut imports = Vec::with_capacity(items.len());
    for (position, item) in items.iter().enumerate() {
        match value.view(*item).as_str() {
            Some(text) if is_dotted_identifier(text) => imports.push(text.clone()),
            _ => errors.push(format!(
                "entry[{index}].canonical_import[{position}]: must be a dotted identifier string"
            )),
        }
    }
    if imports.len() != items.len() {
        return None;
    }
    Some(CanonicalImport::Many(imports))
}

fn validate_version_order(
    index: usize,
    releases: &[Option<PythonString>; 2],
    errors: &mut Vec<String>,
) {
    let (Some(introduced), Some(removal)) = (&releases[0], &releases[1]) else {
        return;
    };
    let (Some(introduced_version), Some(removal_version)) = (
        parse_version(&message_text(introduced)),
        parse_version(&message_text(removal)),
    ) else {
        errors.push(format!(
            "entry[{index}]: version strings are not valid semver"
        ));
        return;
    };
    if removal_version < introduced_version {
        errors.push(format!(
            "entry[{index}].removal_target_release: must be >= introduced_in_release"
        ));
    }
}
