//! Read-only Mission metadata **without** an invented identity.
//!
//! [`crate::Mission`] deliberately requires a canonical ULID: its
//! [`Mission::id`](crate::Mission::id) is the identity authority, and no caller
//! may weaken it. Legacy Mission directories written before identity minting
//! carry no `mission_id` at all, and the pinned baseline supports them: its
//! `resolve_mission_identity` returns `None`, lifecycle prints
//! `"mission_id": null`, and status rows simply omit the field.
//!
//! This module is the additive, source-backed view for exactly that legacy
//! read: [`MissionRecord`] parses the same `kitty-specs/*/meta.json` inventory
//! with the same containment, symlink, size and duplicate-key rules, but keeps
//! the identity **optional**. It never mints, hashes, zeroes or aliases an id:
//! an absent `mission_id` stays `None` and is rendered as JSON `null`. A present
//! value is validated through [`MissionId`], so an invalid id is still refused.
//!
//! Nothing here writes, creates, locks or selects in a new way: the selector is
//! the same id → mid8 → exact-slug order [`crate::select_mission`] uses, with
//! identity-shaped selectors only matching Records that actually record an id.

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

/// One read-only Mission record whose canonical identity may be absent.
///
/// Accessors mirror [`crate::Mission`] one-for-one except [`Self::id`], which
/// returns `Option<&MissionId>`. That is the whole point of the type: callers
/// that must support pre-identity Missions read this view, and callers that
/// require identity keep using [`crate::Mission`].
#[derive(Clone, Debug, Serialize)]
pub struct MissionRecord {
    slug: MissionSlug,
    id: Option<MissionId>,
    mid8: Option<String>,
    mission_number: Option<u32>,
    mission_type: Option<String>,
    friendly_name: Option<String>,
    spec_dir: PathBuf,
    /// Every `meta.json` field, preserved exactly as decoded.
    meta: Map<String, Value>,
}

impl MissionRecord {
    /// The canonical slug: the `kitty-specs/` directory name.
    pub fn slug(&self) -> &MissionSlug {
        &self.slug
    }

    /// The recorded canonical ULID, or `None` for a legacy Mission.
    ///
    /// A `None` is never replaced by a derived value, a hash, or a zero id.
    pub fn id(&self) -> Option<&MissionId> {
        self.id.as_ref()
    }

    /// The 8-character disambiguator, when an id is recorded.
    pub fn mid8(&self) -> Option<&str> {
        self.mid8.as_deref()
    }

    /// Display-only mission number (`None` when no number is assigned).
    pub fn mission_number(&self) -> Option<u32> {
        self.mission_number
    }

    /// Canonical `mission_type` value, if the Mission records one.
    pub fn mission_type(&self) -> Option<&str> {
        self.mission_type.as_deref()
    }

    /// Human-friendly name recorded in metadata, if present.
    pub fn friendly_name(&self) -> Option<&str> {
        self.friendly_name.as_deref()
    }

    /// The Mission directory.
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

    /// The stored `topology` value, when it names a known topology.
    pub fn topology(&self) -> Option<crate::MissionTopology> {
        crate::MissionTopology::from_stored(self.meta.get("topology").and_then(Value::as_str))
    }
}

fn parse_record_id(
    meta: &Map<String, Value>,
    meta_path: &Path,
) -> Result<Option<MissionId>, MissionError> {
    match meta.get("mission_id") {
        // The legacy, supported pre-identity form. It stays `None`.
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            MissionId::from_str(value)
                .map(Some)
                .map_err(|source| MissionError::InvalidMissionId {
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

fn parse_mission_number(
    meta: &Map<String, Value>,
    meta_path: &Path,
) -> Result<Option<u32>, MissionError> {
    // The same coercion `Mission` applies; duplicated here only because the
    // Mission constructor is not exposed as a metadata-only entry point.
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

fn record_from_meta(
    spec_dir: PathBuf,
    slug: MissionSlug,
    meta: Map<String, Value>,
    meta_path: &Path,
) -> Result<MissionRecord, MissionError> {
    let id = parse_record_id(&meta, meta_path)?;
    let mission_number = parse_mission_number(&meta, meta_path)?;
    let mission_type = meta
        .get("mission_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let friendly_name = meta
        .get("friendly_name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(MissionRecord {
        slug,
        mid8: id.as_ref().map(|id| id.mid8().to_owned()),
        id,
        mission_number,
        mission_type,
        friendly_name,
        spec_dir,
        meta,
    })
}

/// Load one Mission record from its directory (`meta.json` must exist).
pub fn load_mission_record(spec_dir: &Path) -> Result<MissionRecord, MissionError> {
    let name = spec_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MissionError::NonUtf8FileName {
            path: spec_dir.to_path_buf(),
        })?;
    let slug = MissionSlug::from_str(name).map_err(|source| {
        MissionError::InvalidMissionDirectoryName {
            path: spec_dir.to_path_buf(),
            source,
        }
    })?;
    let meta_path = spec_dir.join(META_FILE_NAME);
    let meta = load_meta_object(&meta_path, DEFAULT_META_BYTE_LIMIT)?;
    record_from_meta(spec_dir.to_path_buf(), slug, meta, &meta_path)
}

/// List every Mission record under `<root>/kitty-specs/`, sorted by slug.
///
/// The inventory rules are the ones [`crate::list_missions`] documents: an
/// absent `kitty-specs/` is empty, planning residue is skipped, and corrupt or
/// unsafe entries fail the whole listing. The one difference is the supported
/// legacy case: a directory whose `meta.json` has **no** `mission_id` is a
/// Mission record with `id() == None`, not a refused inventory.
pub fn list_mission_records(root: &Path) -> Result<Vec<MissionRecord>, MissionError> {
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

    let mut records = Vec::new();
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
        let record = record_from_meta(dir, slug, meta, &meta_path)?;
        if let Some(id) = record.id.clone()
            && let Some(first) = seen.insert(id.clone(), record.spec_dir.clone())
        {
            return Err(MissionError::DuplicateMissionId {
                id,
                first,
                second: record.spec_dir.clone(),
            });
        }
        records.push(record);
    }

    records.sort_by(|left, right| left.slug.cmp(&right.slug));
    Ok(records)
}

fn is_mid8(value: &str) -> bool {
    value.len() == MID8_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

fn unique_match<'a>(
    selector: &str,
    matches: Vec<&'a MissionRecord>,
) -> Result<&'a MissionRecord, MissionError> {
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

/// Select exactly one record from an in-memory inventory.
///
/// The same selector order as [`crate::select_mission`]: full id, 8-character
/// `mid8`, exact slug. An identity-shaped selector is terminal: it never falls
/// through to slug matching, and a legacy record with no id simply cannot match
/// it.
pub fn select_mission_record<'a>(
    records: &'a [MissionRecord],
    selector: &str,
) -> Result<&'a MissionRecord, MissionError> {
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
        let matches = records
            .iter()
            .filter(|record| record.id.as_ref() == Some(&id))
            .collect();
        return unique_match(selector, matches);
    }

    if is_mid8(selector) {
        let matches = records
            .iter()
            .filter(|record| record.mid8.as_deref() == Some(selector))
            .collect();
        return unique_match(selector, matches);
    }

    let matches = records
        .iter()
        .filter(|record| record.slug.as_str() == selector)
        .collect();
    unique_match(selector, matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mission_dir(root: &Path, slug: &str, meta: Value) -> PathBuf {
        let dir = root.join(SPECS_DIR_NAME).join(slug);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(META_FILE_NAME), serde_json::to_vec(&meta).unwrap()).unwrap();
        dir
    }

    #[test]
    fn a_legacy_mission_keeps_a_null_identity_and_is_still_selectable_by_slug() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = mission_dir(
            temp.path(),
            "048-noid-demo",
            json!({"mission_slug": "048-noid-demo", "mission_number": null, "mission_type": "software-dev"}),
        );
        let modern = mission_dir(
            temp.path(),
            "049-with-id",
            json!({"mission_slug": "049-with-id", "mission_id": "01K9M0R3W00000000000000000"}),
        );

        let records = list_mission_records(temp.path()).unwrap();
        assert_eq!(records.len(), 2);
        let record = records
            .iter()
            .find(|record| record.spec_dir() == legacy)
            .unwrap();
        assert!(record.id().is_none(), "no invented identity");
        assert!(record.mid8().is_none());
        assert_eq!(record.slug().as_str(), "048-noid-demo");
        assert_eq!(record.mission_type(), Some("software-dev"));

        assert_eq!(
            select_mission_record(&records, "048-noid-demo")
                .unwrap()
                .spec_dir(),
            legacy
        );
        let modern_record = records
            .iter()
            .find(|record| record.spec_dir() == modern)
            .unwrap();
        assert_eq!(modern_record.mid8(), Some("01K9M0R3"));
        // Identity-shaped selectors only match records that record an id.
        assert!(select_mission_record(&records, "01K9M0R3").is_ok());
        assert!(select_mission_record(&records, "01K9M0R3W0000000000000000Z").is_err());
    }

    #[test]
    fn an_invalid_recorded_identity_is_still_refused() {
        let temp = tempfile::tempdir().unwrap();
        mission_dir(
            temp.path(),
            "050-bad-id",
            json!({"mission_id": "not-a-ulid"}),
        );
        let error = list_mission_records(temp.path()).unwrap_err();
        assert!(
            matches!(error, MissionError::InvalidMissionId { .. }),
            "{error}"
        );
    }

    #[test]
    fn an_inventory_without_meta_json_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(SPECS_DIR_NAME).join("residue")).unwrap();
        assert!(list_mission_records(temp.path()).unwrap().is_empty());
    }
}
