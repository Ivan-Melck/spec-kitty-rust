use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;
use serde_json::{Map, Value};
use sk_kernel::{MissionSlug, resolve_within};

use crate::error::MissionError;
use crate::identity::{MID8_LENGTH, MissionId};
use crate::meta::{
    DEFAULT_META_BYTE_LIMIT, META_FILE_NAME, SPECS_DIR_NAME, json_kind, load_meta_object,
};
use crate::topology::MissionTopology;

/// Read-only view of one Mission discovered under `<root>/kitty-specs/`.
///
/// The canonical identity is [`Mission::id`] (a ULID). The directory name is
/// the canonical [`MissionSlug`]; the raw `meta.json` `slug`/`mission_slug`
/// values are preserved in [`Mission::meta`] but are never treated as identity
/// (real fixtures contain legacy slugs without the numeric prefix).
///
/// This type is an inspection model only. It does not write `meta.json`, does
/// not create Missions, and carries no runtime, placement, or ownership
/// claims.
#[derive(Clone, Debug, Serialize)]
pub struct Mission {
    slug: MissionSlug,
    id: MissionId,
    mid8: String,
    /// Display-only ordering hint; never identity and never a selector.
    mission_number: Option<u32>,
    mission_type: Option<String>,
    friendly_name: Option<String>,
    /// The stored `topology` value, if it is recognised. Never derived.
    topology: Option<MissionTopology>,
    /// Provenance flag preserved verbatim; never a topology member.
    flattened: bool,
    spec_dir: PathBuf,
    /// Every `meta.json` field, including extension fields this crate does not
    /// interpret. Preserved exactly as decoded.
    meta: Map<String, Value>,
}

impl Mission {
    /// Canonical slug: the `kitty-specs/` directory name.
    pub fn slug(&self) -> &MissionSlug {
        &self.slug
    }

    /// Canonical mission identity (26-character ULID).
    pub fn id(&self) -> &MissionId {
        &self.id
    }

    /// The 8-character disambiguator, derived once from [`Mission::id`].
    pub fn mid8(&self) -> &str {
        &self.mid8
    }

    /// Display-only mission number (`None` when no number is assigned).
    pub fn mission_number(&self) -> Option<u32> {
        self.mission_number
    }

    /// Decimal text for display; no leading-zero padding is invented.
    pub fn mission_number_display(&self) -> Option<String> {
        self.mission_number.map(|number| number.to_string())
    }

    /// Canonical `mission_type` value, if the Mission records one.
    pub fn mission_type(&self) -> Option<&str> {
        self.mission_type.as_deref()
    }

    /// Human-friendly name recorded in metadata, if present.
    pub fn friendly_name(&self) -> Option<&str> {
        self.friendly_name.as_deref()
    }

    /// Name to show for this Mission: `friendly_name`, else the slug.
    pub fn display_name(&self) -> String {
        self.friendly_name
            .clone()
            .unwrap_or_else(|| self.slug.as_str().to_owned())
    }

    /// The stored topology value, or `None` when absent/unrecognised.
    pub fn topology(&self) -> Option<MissionTopology> {
        self.topology
    }

    /// The stored `flattened` provenance flag.
    pub fn flattened(&self) -> bool {
        self.flattened
    }

    /// Absolute (canonicalized-parent) directory holding this Mission.
    pub fn spec_dir(&self) -> &Path {
        &self.spec_dir
    }

    /// All decoded `meta.json` fields, including extension fields.
    pub fn meta(&self) -> &Map<String, Value> {
        &self.meta
    }

    /// One raw `meta.json` field by key.
    pub fn meta_field(&self, key: &str) -> Option<&Value> {
        self.meta.get(key)
    }

    fn from_meta(
        spec_dir: PathBuf,
        slug: MissionSlug,
        meta: Map<String, Value>,
        meta_path: &Path,
    ) -> Result<Self, MissionError> {
        let id = parse_mission_id(&meta, meta_path)?;
        let mission_number = parse_mission_number(&meta, meta_path)?;
        let mission_type = meta
            .get("mission_type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let friendly_name = match meta.get("friendly_name") {
            Some(Value::String(value)) => Some(value.clone()),
            Some(_) => None,
            None => None,
        };
        let topology = MissionTopology::from_stored(meta.get("topology").and_then(Value::as_str));
        let flattened = matches!(meta.get("flattened"), Some(Value::Bool(true)));

        Ok(Self {
            slug,
            mid8: id.mid8().to_owned(),
            id,
            mission_number,
            mission_type,
            friendly_name,
            topology,
            flattened,
            spec_dir,
            meta,
        })
    }
}

fn parse_mission_id(
    meta: &Map<String, Value>,
    meta_path: &Path,
) -> Result<MissionId, MissionError> {
    match meta.get("mission_id") {
        None | Some(Value::Null) => Err(MissionError::MissingMissionId {
            path: meta_path.to_path_buf(),
        }),
        Some(Value::String(value)) => {
            MissionId::from_str(value).map_err(|source| MissionError::InvalidMissionId {
                path: meta_path.to_path_buf(),
                value: value.clone(),
                source,
            })
        }
        Some(other) => Err(MissionError::InvalidMissionIdType {
            path: meta_path.to_path_buf(),
            kind: json_kind(other),
        }),
    }
}

/// Coerce the stored `mission_number` to a display number.
///
/// Accepted: JSON `null`, a non-negative integer, or a numeric string (legacy
/// `"042"` becomes `42`). Refused: booleans, floats, arrays, objects, and the
/// `"pending"`/`"unassigned"`/`"TBD"` sentinels — the caller must run an
/// upstream identity backfill rather than have this crate invent a number.
fn parse_mission_number(
    meta: &Map<String, Value>,
    meta_path: &Path,
) -> Result<Option<u32>, MissionError> {
    let invalid = |value: String, detail: &str| MissionError::InvalidMissionNumber {
        path: meta_path.to_path_buf(),
        value,
        detail: detail.to_owned(),
    };

    match meta.get("mission_number") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let Some(value) = number.as_u64() else {
                return Err(invalid(
                    number.to_string(),
                    "expected a non-negative integer, numeric string, or null",
                ));
            };
            u32::try_from(value).map(Some).map_err(|_| {
                invalid(
                    number.to_string(),
                    "mission_number does not fit in an unsigned 32-bit display number",
                )
            })
        }
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            if matches!(trimmed, "pending" | "unassigned" | "TBD") {
                return Err(invalid(
                    raw.clone(),
                    "sentinel values are not display numbers; run the identity backfill first",
                ));
            }
            if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid(
                    raw.clone(),
                    "expected a non-negative integer, numeric string, or null",
                ));
            }
            trimmed.parse::<u32>().map(Some).map_err(|_| {
                invalid(
                    raw.clone(),
                    "mission_number does not fit in an unsigned 32-bit display number",
                )
            })
        }
        Some(other) => Err(invalid(
            other.to_string(),
            "expected a non-negative integer, numeric string, or null",
        )),
    }
}

/// List every Mission under `<root>/kitty-specs/`, sorted by slug.
///
/// Rules:
///
/// * The project root must exist and be a directory.
/// * An absent `kitty-specs/` is an empty inventory, not an error.
/// * `kitty-specs/` children that are files are ignored; a hidden directory
///   without `meta.json` is planning residue and is skipped too.
/// * A directory without `meta.json` is not a Mission (planning residue) and is
///   skipped.
/// * A directory with `meta.json` must be a valid Mission. Corrupt metadata,
///   an unsafe directory name, a symlinked entry, or a duplicate canonical
///   `mission_id` fails the whole listing: the inventory is all-or-nothing so
///   callers never silently lose a Mission.
pub fn list_missions(root: &Path) -> Result<Vec<Mission>, MissionError> {
    let root_metadata = match fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(MissionError::RootNotDirectory {
                root: root.to_path_buf(),
            });
        }
        Err(source) => return Err(MissionError::io("inspect", root, source)),
    };
    if !root_metadata.is_dir() {
        return Err(MissionError::RootNotDirectory {
            root: root.to_path_buf(),
        });
    }

    // Inspect the authored entry before containment expands symlinks.
    let specs_candidate = root.join(SPECS_DIR_NAME);
    if fs::symlink_metadata(&specs_candidate)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(MissionError::SymlinkedEntry {
            path: specs_candidate,
        });
    }
    let specs_dir = resolve_within(root, Path::new(SPECS_DIR_NAME))
        .map_err(|source| MissionError::kernel(root, source))?;

    match fs::symlink_metadata(&specs_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(MissionError::SymlinkedEntry { path: specs_dir });
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(MissionError::SpecsDirectoryNotDirectory { path: specs_dir });
        }
        Ok(_) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(MissionError::io("inspect", &specs_dir, source)),
    }

    let mut entries = Vec::new();
    for entry in
        fs::read_dir(&specs_dir).map_err(|source| MissionError::io("list", &specs_dir, source))?
    {
        entries.push(entry.map_err(|source| MissionError::io("list", &specs_dir, source))?);
    }
    entries.sort_by_key(|entry| entry.file_name());

    let mut missions = Vec::new();
    let mut seen: BTreeMap<MissionId, PathBuf> = BTreeMap::new();

    for entry in entries {
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| MissionError::NonUtf8FileName { path: entry.path() })?;
        let file_type = entry
            .file_type()
            .map_err(|source| MissionError::io("inspect", entry.path(), source))?;
        if file_type.is_symlink() {
            return Err(MissionError::SymlinkedEntry { path: entry.path() });
        }
        if !file_type.is_dir() {
            continue;
        }

        let dir = specs_dir.join(name);
        let meta_candidate = dir.join(META_FILE_NAME);

        // Inspect the entry itself first so a symlinked `meta.json` is reported
        // as such; containment resolution follows and refuses anything that
        // still escapes the project root.
        match fs::symlink_metadata(&meta_candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(MissionError::SymlinkedEntry {
                    path: meta_candidate,
                });
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(MissionError::MetadataNotFile {
                    path: meta_candidate,
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(MissionError::io("inspect", &meta_candidate, source)),
        }

        let meta_relative = Path::new(SPECS_DIR_NAME).join(name).join(META_FILE_NAME);
        let meta_path = resolve_within(root, &meta_relative)
            .map_err(|source| MissionError::kernel(&dir, source))?;

        let slug = MissionSlug::from_str(name).map_err(|source| {
            MissionError::InvalidMissionDirectoryName {
                path: dir.clone(),
                source,
            }
        })?;
        let meta = load_meta_object(&meta_path, DEFAULT_META_BYTE_LIMIT)?;
        let mission = Mission::from_meta(dir, slug, meta, &meta_path)?;

        if let Some(first) = seen.insert(mission.id.clone(), mission.spec_dir.clone()) {
            return Err(MissionError::DuplicateMissionId {
                id: mission.id.clone(),
                first,
                second: mission.spec_dir.clone(),
            });
        }
        missions.push(mission);
    }

    missions.sort_by(|left, right| left.slug.cmp(&right.slug));
    Ok(missions)
}

fn is_mid8(value: &str) -> bool {
    value.len() == MID8_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

fn unique_match<'a>(
    selector: &str,
    matches: Vec<&'a Mission>,
) -> Result<&'a Mission, MissionError> {
    match matches.len() {
        0 => Err(MissionError::MissionNotFound {
            selector: selector.to_owned(),
        }),
        1 => Ok(matches[0]),
        count => Err(MissionError::AmbiguousMission {
            selector: selector.to_owned(),
            count,
        }),
    }
}

/// Select exactly one Mission from an in-memory inventory.
///
/// Accepted selector forms, checked in this order:
///
/// 1. a full 26-character canonical mission id (ULID),
/// 2. an 8-character `mid8` (uppercase alphanumeric prefix of a mission id),
/// 3. an exact mission slug.
///
/// There is deliberately no numeric-prefix, mission-number, slug-tail, or
/// "sole mission" fallback. Ambiguity is always an error; a caller that wants
/// auto-selection must check the inventory length itself. An identity-shaped
/// selector never falls through to slug matching: a successfully parsed full
/// id and any 8-character uppercase-alphanumeric `mid8` shape are terminal.
pub fn select_mission<'a>(
    missions: &'a [Mission],
    selector: &str,
) -> Result<&'a Mission, MissionError> {
    if selector.is_empty()
        || selector.trim() != selector
        || selector.contains('/')
        || selector.contains('\\')
        || matches!(selector, "." | "..")
    {
        return Err(MissionError::InvalidSelector {
            selector: selector.to_owned(),
        });
    }

    if let Ok(id) = MissionId::from_str(selector) {
        let matches = missions.iter().filter(|mission| mission.id == id).collect();
        return unique_match(selector, matches);
    }

    if is_mid8(selector) {
        let matches = missions
            .iter()
            .filter(|mission| mission.mid8 == selector)
            .collect();
        return unique_match(selector, matches);
    }

    let matches = missions
        .iter()
        .filter(|mission| mission.slug.as_str() == selector)
        .collect();
    unique_match(selector, matches)
}
