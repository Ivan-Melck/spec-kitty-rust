//! Activation presets: the pack-data format, the strict loader, discovery,
//! the project's offering packs and the manifest hash entries.
//!
//! An **activation preset** is pack data (FR-019): a named file
//! `<pack>/presets/<name>.yaml` holding activation keys that
//! `spec-kitty charter activate --preset <name>` applies with replace
//! semantics. It is not an [`ArtifactKind`], has no URN, and is hashed by the
//! pack manifest ([`enumerate_presets`]).
//!
//! A preset file carries `name` (equal to the file stem), `description`, any of
//! the per-kind `activated_<plural>` keys of the governed kinds,
//! `activated_kinds` and `mission_type_activations`. A key that is absent
//! leaves its kind unrestricted; an empty list activates nothing of that kind.
//!
//! The kinds a preset governs are **derived** from [`ArtifactKind`] (never
//! hand-listed): every charter-activatable kind except `skill` and
//! `glossary_pack`, each of which has its own absence contract.
//!
//! Contract mirror (Python reference, never invoked at runtime):
//! `src/charter/offering/packs/presets.py`. This module is the offering tier:
//! it reads pack data only and knows nothing about activation writes.
//!
//! The loader is strict by construction — it reads the document through the
//! crate's canonical YAML reader and checks every field explicitly, rather
//! than letting a derived deserializer guess at scalar shapes.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::{PathExpansion, load_project_config};
use crate::error::CharterError;
use crate::io::{FieldValue, field, read_bounded, sha256_hex};
use crate::kind::ArtifactKind;
use crate::limits::Limits;
use crate::pack::{BuiltInSearch, PackRoots};
use crate::tier::PackTier;

/// The built-in pack's preset that seeds `mission_type_activations`, and the
/// preset a customised list is compared against (FR-003).
pub const DEFAULT_PRESET_NAME: &str = "default";

/// The kind-gate key.
pub const ACTIVATED_KINDS_KEY: &str = "activated_kinds";

/// The mission-type activation key.
pub const MISSION_TYPE_ACTIVATIONS_KEY: &str = "mission_type_activations";

/// The pack-relative directory presets live in (`kernel.charter_pack_paths`).
pub const PRESETS_DIRNAME: &str = "presets";

/// The project pack root, relative to the repository root.
pub const PROJECT_PACK_ROOT: &str = ".kittify/charter-packs";

/// The name of the built-in pack (`packs/built-in/pack.yaml`).
pub const BUILT_IN_PACK_NAME: &str = "built-in";

/// The name of the project overlay pack in the offering.
pub const PROJECT_PACK_NAME: &str = "project";

/// Longest preset name accepted.
pub const NAME_MAX_LENGTH: usize = 64;

const ACTIVATED_PREFIX: &str = "activated_";
const NAME_KEY: &str = "name";
const DESCRIPTION_KEY: &str = "description";
/// The org-charter key for context-scoped activation entries; a preset has none.
const CONTEXT_SCOPED_KEY: &str = "activations";
const PRESET_GLOB_SUFFIX: &str = ".yaml";

/// Kinds whose activation key a preset may not carry: each has its own absence
/// contract (`skill` is `effective_when_absent == "required"`; glossary packs
/// are governed by their own key), so a preset never writes or clears them.
const OWN_ABSENCE_CONTRACT_KINDS: [ArtifactKind; 2] =
    [ArtifactKind::Skill, ArtifactKind::GlossaryPack];

/// The per-kind activation key of `kind` (`activated_<plural>`).
pub fn preset_activation_key(kind: ArtifactKind) -> String {
    format!("{ACTIVATED_PREFIX}{}", kind.plural())
}

/// The kinds a preset governs with a per-kind `activated_<plural>` key, in
/// [`ArtifactKind::ALL`] declaration order (derived, never hand-listed).
pub fn governed_kinds() -> Vec<ArtifactKind> {
    ArtifactKind::ALL
        .into_iter()
        .filter(|kind| kind.activatable() && !OWN_ABSENCE_CONTRACT_KINDS.contains(kind))
        .collect()
}

/// The per-kind activation keys a preset may carry, in declaration order.
pub fn preset_activation_keys() -> Vec<String> {
    governed_kinds()
        .into_iter()
        .map(preset_activation_key)
        .collect()
}

/// Every value `activated_kinds` may hold (the universe the gate is read over).
fn kind_gate_plurals() -> Vec<&'static str> {
    ArtifactKind::ALL
        .into_iter()
        .map(ArtifactKind::plural)
        .collect()
}

fn ungoverned_activation_keys() -> Vec<String> {
    OWN_ABSENCE_CONTRACT_KINDS
        .into_iter()
        .map(preset_activation_key)
        .collect()
}

fn allowed_keys() -> Vec<String> {
    let mut keys = vec![
        NAME_KEY.to_owned(),
        DESCRIPTION_KEY.to_owned(),
        ACTIVATED_KINDS_KEY.to_owned(),
        MISSION_TYPE_ACTIVATIONS_KEY.to_owned(),
    ];
    keys.extend(preset_activation_keys());
    keys
}

/// Python's `repr()` of a plain string, as the reference's messages render it.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            other => out.push(other),
        }
    }
    out.push('\'');
    out
}

fn listing<S: AsRef<str>>(names: &[S]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A preset file is malformed; the message names the file and the field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetFormatError {
    /// The offending file.
    pub path: PathBuf,
    /// The field, or `<file>` for a whole-document problem.
    pub field: String,
    /// What is wrong with it.
    pub detail: String,
}

impl PresetFormatError {
    fn new(path: &Path, field: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            field: field.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for PresetFormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        write!(formatter, "{name}: {}: {}", self.field, self.detail)
    }
}

impl std::error::Error for PresetFormatError {}

/// No preset of that name exists in the pack; carries the available names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetNotFoundError {
    /// The requested preset name.
    pub name: String,
    /// The pack root that was searched.
    pub pack_root: PathBuf,
    /// The names the pack does ship.
    pub available: Vec<String>,
}

impl fmt::Display for PresetNotFoundError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "preset {} not found in {} (available: {})",
            quoted(&self.name),
            self.pack_root.display(),
            listing(&self.available)
        )
    }
}

impl std::error::Error for PresetNotFoundError {}

/// Loading a preset failed: it is missing, or its file is malformed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PresetLoadError {
    /// The pack ships no preset of that name.
    NotFound(PresetNotFoundError),
    /// The preset file is malformed.
    Format(PresetFormatError),
}

impl fmt::Display for PresetLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(error) => error.fmt(formatter),
            Self::Format(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PresetLoadError {}

impl From<PresetFormatError> for PresetLoadError {
    fn from(error: PresetFormatError) -> Self {
        Self::Format(error)
    }
}

impl From<PresetNotFoundError> for PresetLoadError {
    fn from(error: PresetNotFoundError) -> Self {
        Self::NotFound(error)
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One loaded preset.
///
/// `activations` holds only the per-kind keys present in the file (absent means
/// unrestricted; an empty list means none). `activated_kinds` and
/// `mission_type_activations` are `None` when the file does not carry them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationPreset {
    /// The preset name (equal to the file stem).
    pub name: String,
    /// The authored description.
    pub description: String,
    /// Per-kind `activated_<plural>` key -> listed ids, present keys only.
    pub activations: BTreeMap<String, Vec<String>>,
    /// `activated_kinds` when the file carries it.
    pub activated_kinds: Option<Vec<String>>,
    /// `mission_type_activations` when the file carries it.
    pub mission_type_activations: Option<Vec<String>>,
    /// The preset file.
    pub source: PathBuf,
}

impl ActivationPreset {
    /// The kinds the preset lists ids for (a key is present).
    pub fn listed_kinds(&self) -> Vec<ArtifactKind> {
        governed_kinds()
            .into_iter()
            .filter(|kind| self.activations.contains_key(&preset_activation_key(*kind)))
            .collect()
    }

    /// The plurals the preset lists ids for but leaves out of `activated_kinds`.
    ///
    /// Empty when the preset carries no kind gate. A non-empty result means the
    /// preset would switch off a kind it activates ids for (FR-002).
    pub fn kind_gate_omissions(&self) -> Vec<String> {
        let Some(gate) = &self.activated_kinds else {
            return Vec::new();
        };
        self.listed_kinds()
            .into_iter()
            .filter(|kind| !gate.iter().any(|plural| plural == kind.plural()))
            .map(|kind| kind.plural().to_owned())
            .collect()
    }
}

/// The plurals the preset lists ids for but leaves out of `activated_kinds`.
///
/// Empty when the preset carries no kind gate. A non-empty result means the
/// preset would switch off a kind it activates ids for (FR-002).
pub fn kind_gate_omissions(preset: &ActivationPreset) -> Vec<String> {
    preset.kind_gate_omissions()
}

/// Whether [`kind_gate_omissions`] is empty without allocating its result.
pub fn kind_gate_omissions_is_empty(preset: &ActivationPreset) -> bool {
    let Some(gate) = &preset.activated_kinds else {
        return true;
    };
    preset
        .listed_kinds()
        .into_iter()
        .all(|kind| gate.iter().any(|plural| plural == kind.plural()))
}

// ---------------------------------------------------------------------------
// Strict loader
// ---------------------------------------------------------------------------

/// Whether `name` matches the preset-name grammar `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`.
fn name_matches_grammar(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut index = 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            index += 1;
            continue;
        }
        if byte != b'-' {
            return false;
        }
        let next = match bytes.get(index + 1) {
            Some(next) => *next,
            None => return false,
        };
        if !(next.is_ascii_lowercase() || next.is_ascii_digit()) {
            return false;
        }
        index += 2;
    }
    true
}

/// Read one preset's top-level mapping.
///
/// A document whose root is not a mapping — including an empty file, whose
/// scalar is null — is a format error naming `<file>`.
fn read_mapping(path: &Path, limits: &Limits) -> Result<serde_yaml_ng::Mapping, PresetFormatError> {
    let bytes = read_bounded(path, limits.max_pack_descriptor_bytes).map_err(|error| {
        PresetFormatError::new(path, "<file>", format!("cannot be read: {error}"))
    })?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&bytes).map_err(|error| {
        PresetFormatError::new(path, "<file>", format!("YAML parse error: {error}"))
    })?;
    match value {
        serde_yaml_ng::Value::Mapping(mapping) => Ok(mapping),
        _ => Err(PresetFormatError::new(
            path,
            "<file>",
            "expected a YAML mapping at top level",
        )),
    }
}

fn check_keys(path: &Path, mapping: &serde_yaml_ng::Mapping) -> Result<(), PresetFormatError> {
    let allowed = allowed_keys();
    let mut unknown: Vec<String> = Vec::new();
    for (key, _) in mapping {
        let Some(name) = key.as_str() else {
            return Err(PresetFormatError::new(
                path,
                "<file>",
                "preset keys must be strings",
            ));
        };
        if !allowed.iter().any(|allowed| allowed == name) {
            unknown.push(name.to_owned());
        }
    }
    if unknown.is_empty() {
        return Ok(());
    }
    unknown.sort();
    if unknown.iter().any(|key| key == CONTEXT_SCOPED_KEY) {
        return Err(PresetFormatError::new(
            path,
            CONTEXT_SCOPED_KEY,
            "a preset carries no context-scoped activation entries",
        ));
    }
    let ungoverned = ungoverned_activation_keys();
    if let Some(key) = unknown
        .iter()
        .find(|key| ungoverned.iter().any(|own| own == *key))
    {
        return Err(PresetFormatError::new(
            path,
            key.clone(),
            "presets do not govern this kind; it keeps its own activation contract",
        ));
    }
    Err(PresetFormatError::new(
        path,
        unknown[0].clone(),
        format!("unknown key(s): {}", unknown.join(", ")),
    ))
}

fn required_string<'a>(
    path: &Path,
    mapping: &'a serde_yaml_ng::Mapping,
    key: &str,
) -> Result<&'a str, PresetFormatError> {
    match field(mapping, key) {
        FieldValue::Absent => Err(PresetFormatError::new(path, key, "is required")),
        FieldValue::Null | FieldValue::Other(_) => {
            Err(PresetFormatError::new(path, key, "must be a string"))
        }
        FieldValue::String(value) => Ok(value),
    }
}

fn check_name(path: &Path, value: &str) -> Result<String, PresetFormatError> {
    if value.chars().count() > NAME_MAX_LENGTH {
        return Err(PresetFormatError::new(
            path,
            NAME_KEY,
            format!("is longer than {NAME_MAX_LENGTH} characters"),
        ));
    }
    if !name_matches_grammar(value) {
        return Err(PresetFormatError::new(
            path,
            NAME_KEY,
            format!(
                "{} does not match ^[a-z][a-z0-9]*(-[a-z0-9]+)*$",
                quoted(value)
            ),
        ));
    }
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    if value != stem {
        return Err(PresetFormatError::new(
            path,
            NAME_KEY,
            format!(
                "{} must equal the file stem {}",
                quoted(value),
                quoted(&stem)
            ),
        ));
    }
    Ok(value.to_owned())
}

fn check_description(path: &Path, value: &str) -> Result<String, PresetFormatError> {
    if value.trim().is_empty() {
        return Err(PresetFormatError::new(
            path,
            DESCRIPTION_KEY,
            "must be a non-empty string",
        ));
    }
    Ok(value.to_owned())
}

fn check_id_list(
    path: &Path,
    key: &str,
    value: Option<&serde_yaml_ng::Value>,
) -> Result<Vec<String>, PresetFormatError> {
    let Some(value) = value else {
        return Err(PresetFormatError::new(
            path,
            key,
            "must be a list of strings",
        ));
    };
    let serde_yaml_ng::Value::Sequence(items) = value else {
        return Err(PresetFormatError::new(
            path,
            key,
            "must be a list of strings",
        ));
    };
    let mut ids: Vec<String> = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(PresetFormatError::new(
                path,
                key,
                format!("entry {item:?} is not a non-empty string"),
            ));
        };
        if text.trim().is_empty() {
            return Err(PresetFormatError::new(
                path,
                key,
                format!("entry {} is not a non-empty string", quoted(text)),
            ));
        }
        if ids.iter().any(|id| id == text) {
            return Err(PresetFormatError::new(
                path,
                key,
                format!("duplicate entry {}", quoted(text)),
            ));
        }
        ids.push(text.to_owned());
    }
    Ok(ids)
}

fn mapping_value<'a>(
    mapping: &'a serde_yaml_ng::Mapping,
    key: &str,
) -> Option<&'a serde_yaml_ng::Value> {
    mapping.iter().find_map(|(candidate, value)| {
        if candidate.as_str() == Some(key) {
            Some(value)
        } else {
            None
        }
    })
}

fn optional_id_list(
    path: &Path,
    mapping: &serde_yaml_ng::Mapping,
    key: &str,
) -> Result<Option<Vec<String>>, PresetFormatError> {
    if !mapping
        .iter()
        .any(|(candidate, _)| candidate.as_str() == Some(key))
    {
        return Ok(None);
    }
    check_id_list(path, key, mapping_value(mapping, key)).map(Some)
}

fn check_kind_gate(
    path: &Path,
    mapping: &serde_yaml_ng::Mapping,
) -> Result<Option<Vec<String>>, PresetFormatError> {
    let Some(plurals) = optional_id_list(path, mapping, ACTIVATED_KINDS_KEY)? else {
        return Ok(None);
    };
    let known = kind_gate_plurals();
    let unknown: Vec<&String> = plurals
        .iter()
        .filter(|plural| !known.iter().any(|candidate| candidate == &plural.as_str()))
        .collect();
    if !unknown.is_empty() {
        let names: Vec<String> = unknown.iter().map(|plural| (*plural).clone()).collect();
        return Err(PresetFormatError::new(
            path,
            ACTIVATED_KINDS_KEY,
            format!("unknown kind(s): {}", names.join(", ")),
        ));
    }
    Ok(Some(plurals))
}

/// Load and strictly validate one preset file.
///
/// Raises [`PresetFormatError`] naming the offending field for an unreadable
/// file, an unknown key (`activated_skills` and `activated_glossary_packs` with
/// their own message), a context-scoped `activations:` list, a `name` that
/// breaks the grammar or differs from the file stem, and a list value that is
/// not unique non-empty strings.
pub fn load_preset_file(path: &Path) -> Result<ActivationPreset, PresetFormatError> {
    load_preset_file_with_limits(path, &Limits::default())
}

/// [`load_preset_file`] under explicit read budgets.
pub fn load_preset_file_with_limits(
    path: &Path,
    limits: &Limits,
) -> Result<ActivationPreset, PresetFormatError> {
    let mapping = read_mapping(path, limits)?;
    check_keys(path, &mapping)?;
    let name = check_name(path, required_string(path, &mapping, NAME_KEY)?)?;
    let description = check_description(path, required_string(path, &mapping, DESCRIPTION_KEY)?)?;
    let mut activations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for key in preset_activation_keys() {
        if !mapping
            .iter()
            .any(|(candidate, _)| candidate.as_str() == Some(key.as_str()))
        {
            continue;
        }
        activations.insert(
            key.clone(),
            check_id_list(path, &key, mapping_value(&mapping, &key))?,
        );
    }
    let kind_gate = check_kind_gate(path, &mapping)?;
    let mission_types = optional_id_list(path, &mapping, MISSION_TYPE_ACTIVATIONS_KEY)?;
    Ok(ActivationPreset {
        name,
        description,
        activations,
        activated_kinds: kind_gate,
        mission_type_activations: mission_types,
        source: path.to_path_buf(),
    })
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// The presets directory of the pack at `pack_root`.
pub fn pack_presets_dir(pack_root: &Path) -> PathBuf {
    pack_root.join(PRESETS_DIRNAME)
}

/// The preset files of the pack at `pack_root`, sorted by name.
///
/// A pack without a `presets/` directory has none (a pack without presets
/// works).
pub fn preset_files(pack_root: &Path) -> Result<Vec<PathBuf>, PresetFormatError> {
    let directory = pack_presets_dir(pack_root);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&directory).map_err(|error| {
        PresetFormatError::new(&directory, "<file>", format!("cannot be read: {error}"))
    })?;
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(PRESET_GLOB_SUFFIX))
        })
        .collect();
    files.sort_by(|left, right| {
        left.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .cmp(
                &right
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned()),
            )
            .then_with(|| left.cmp(right))
    });
    Ok(files)
}

/// Load every preset of the pack at `pack_root`, sorted by name.
///
/// Raises [`PresetFormatError`] on the first malformed file.
pub fn discover_presets(pack_root: &Path) -> Result<Vec<ActivationPreset>, PresetFormatError> {
    preset_files(pack_root)?
        .iter()
        .map(|path| load_preset_file(path))
        .collect()
}

/// Load the preset `name` of the pack at `pack_root`.
///
/// Raises [`PresetNotFoundError`] (carrying the available names) when the pack
/// has no such preset, including for a name outside the preset grammar.
pub fn load_preset(pack_root: &Path, name: &str) -> Result<ActivationPreset, PresetLoadError> {
    let files = preset_files(pack_root)?;
    if name_matches_grammar(name) {
        for path in &files {
            if path.file_stem().and_then(|stem| stem.to_str()) == Some(name) {
                return load_preset_file(path).map_err(Into::into);
            }
        }
    }
    let available: Vec<String> = files
        .iter()
        .filter_map(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect();
    Err(PresetLoadError::NotFound(PresetNotFoundError {
        name: name.to_owned(),
        pack_root: pack_root.to_path_buf(),
        available,
    }))
}

// ---------------------------------------------------------------------------
// Offering packs
// ---------------------------------------------------------------------------

/// One pack in a project's offering, in layer order.
///
/// The `project` tier ships no presets: callers never consult
/// [`discover_presets`] for it ([`OfferingPack::ships_presets`] is false).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfferingPack {
    /// The pack name (`built-in`, an org pack handle, or `project`).
    pub name: String,
    /// The pack tier.
    pub tier: PackTier,
    /// The pack's root directory.
    pub root: PathBuf,
}

impl OfferingPack {
    /// Whether presets are read from this pack (every tier but `project`).
    pub fn ships_presets(&self) -> bool {
        self.tier != PackTier::Project
    }
}

/// The packs of the project at `project_root`: built-in, org packs, project.
///
/// Org packs come from the project's pack registry in declaration order; a pack
/// fetched with `spec-kitty charter fetch` is an org pack whose root is
/// populated. An unfetched root is still listed, and [`discover_presets`]
/// returns nothing for it. No artifact id is resolved here.
pub fn list_offering_packs(
    project_root: &Path,
    limits: &Limits,
) -> Result<Vec<OfferingPack>, CharterError> {
    let config = load_project_config(project_root, limits)?;
    let expansion = PathExpansion::from_env();
    let search = BuiltInSearch::from_env(project_root);
    let roots = PackRoots::resolve(project_root, &config, &expansion, Some(&search))?;
    let mut packs: Vec<OfferingPack> = Vec::new();
    if let Some(root) = roots.built_in {
        packs.push(OfferingPack {
            name: BUILT_IN_PACK_NAME.to_owned(),
            tier: PackTier::BuiltIn,
            root,
        });
    }
    packs.extend(roots.organizations.into_iter().map(|pack| OfferingPack {
        name: pack.name,
        tier: PackTier::Org,
        root: pack.root,
    }));
    packs.push(OfferingPack {
        name: PROJECT_PACK_NAME.to_owned(),
        tier: PackTier::Project,
        root: project_root.join(PROJECT_PACK_ROOT),
    });
    Ok(packs)
}

// ---------------------------------------------------------------------------
// Manifest hashing
// ---------------------------------------------------------------------------

/// One preset recorded in a pack manifest (`presets:`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresetEntry {
    /// The preset name (its file stem).
    pub name: String,
    /// Pack-relative POSIX path of the preset file.
    pub path: String,
    /// SHA-256 (hex) over the LF-normalised file bytes.
    pub content_hash: String,
}

fn lf_normalised(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        let byte = raw[index];
        if byte == b'\r' {
            out.push(b'\n');
            index += if raw.get(index + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            continue;
        }
        out.push(byte);
        index += 1;
    }
    out
}

/// A manifest entry per preset file of `pack_root`, sorted by name.
///
/// Hashes the bytes as shipped (LF-normalised) and does not validate them;
/// [`discover_presets`] owns validation.
pub fn enumerate_presets(pack_root: &Path) -> Result<Vec<PresetEntry>, PresetFormatError> {
    let mut entries = Vec::new();
    for path in preset_files(pack_root)? {
        let raw = std::fs::read(&path).map_err(|error| {
            PresetFormatError::new(&path, "<file>", format!("cannot be read: {error}"))
        })?;
        let relative = path.strip_prefix(pack_root).unwrap_or(&path);
        entries.push(PresetEntry {
            name: path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
            content_hash: sha256_hex(&lf_normalised(&raw)),
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}
