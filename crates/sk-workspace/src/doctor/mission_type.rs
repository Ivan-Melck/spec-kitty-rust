//! Mission-type resolution health audit (`doctor mission-type`).
//!
//! Six closed states, exactly one per selectable Mission:
//!
//! | state | condition |
//! |---|---|
//! | `resolved` | `mission_type` is a non-blank string, activated, and resolves in the layered roster |
//! | `activated-unresolvable` | activated, but no loadable definition anywhere in the roster |
//! | `unknown` | a non-blank `mission_type` that is not activated/registered |
//! | `typeless` | no `mission_type` key, or a blank/null/non-string value |
//! | `legacy-key-only` | only the retired `mission` key carries a real string |
//! | `error` | `meta.json` is unreadable or malformed |
//!
//! # Why the facts are supplied, not re-derived
//!
//! "Activated" is the activation ledger's answer and "resolves" is the Mission
//! definition resolver's answer. Both have canonical owners outside this crate
//! (`sk-doctrine`'s ledger, `sk-mission`'s layered definition resolution), so the
//! audit takes them through [`MissionTypeHealth`]. That keeps this module honest
//! about what it knows: it classifies, it does not re-read the activation store
//! nor reimplement the roster.
//!
//! A present-but-blank/null/non-string `mission_type` never falls through to the
//! legacy `mission` key — the key's own presence routes the classification.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::git_probe::{self, GitAnswer};
use super::meta::read_meta;
use super::{DoctorError, require_directory};

/// Every state, in report order.
pub const MISSION_TYPE_STATES: [MissionTypeState; 6] = [
    MissionTypeState::Resolved,
    MissionTypeState::ActivatedUnresolvable,
    MissionTypeState::Unknown,
    MissionTypeState::Typeless,
    MissionTypeState::LegacyKeyOnly,
    MissionTypeState::Error,
];

/// The mission-type resolution state of one Mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MissionTypeState {
    /// Activated and loadable.
    Resolved,
    /// Activated but no loadable definition anywhere in the roster.
    ActivatedUnresolvable,
    /// Present but not activated/registered.
    Unknown,
    /// No mission type at all.
    Typeless,
    /// Only the retired `mission` key.
    LegacyKeyOnly,
    /// `meta.json` unreadable or malformed.
    Error,
}

impl MissionTypeState {
    /// The stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::ActivatedUnresolvable => "activated-unresolvable",
            Self::Unknown => "unknown",
            Self::Typeless => "typeless",
            Self::LegacyKeyOnly => "legacy-key-only",
            Self::Error => "error",
        }
    }

    /// Parse a `--fail-on` token.
    pub fn parse(value: &str) -> Option<Self> {
        MISSION_TYPE_STATES
            .into_iter()
            .find(|state| state.as_str() == value)
    }
}

/// The canonical facts the classifier needs, supplied by their owners.
pub trait MissionTypeHealth {
    /// The activated mission-type ids (the activation ledger's answer).
    fn activated(&self) -> &BTreeSet<String>;

    /// Whether `mission_type` resolves in the layered roster (project > org >
    /// built-in, plus an activated sparse registration).
    fn resolves(&self, mission_type: &str) -> bool;
}

/// Unknown `--fail-on` state names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownFailOnStates {
    /// The tokens that are not state names, sorted.
    pub unknown: Vec<String>,
}

impl UnknownFailOnStates {
    /// The upstream refusal text.
    pub fn message(&self) -> String {
        let valid: Vec<&str> = MISSION_TYPE_STATES
            .iter()
            .map(|state| state.as_str())
            .collect();
        format!(
            "unknown --fail-on state(s): {}; valid states are: {}",
            self.unknown.join(", "),
            valid.join(", ")
        )
    }
}

/// Parse a `--fail-on` list; a misspelled state is a refusal, not a silent
/// vacuous pass.
pub fn parse_fail_on(raw: Option<&str>) -> Result<BTreeSet<MissionTypeState>, UnknownFailOnStates> {
    let Some(raw) = raw else {
        return Ok(BTreeSet::new());
    };
    let mut states = BTreeSet::new();
    let mut unknown = Vec::new();
    for token in raw
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        match MissionTypeState::parse(token) {
            Some(state) => {
                states.insert(state);
            }
            None => unknown.push(token.to_owned()),
        }
    }
    if unknown.is_empty() {
        Ok(states)
    } else {
        unknown.sort();
        unknown.dedup();
        Err(UnknownFailOnStates { unknown })
    }
}

/// One Mission's mission-type row.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionTypeRow {
    /// Path of the Mission directory.
    pub path: PathBuf,
    /// Directory name used as the slug.
    pub slug: String,
    /// The raw `mission_type` string, when it is a string.
    pub mission_type_raw: Option<String>,
    /// The canonical key the row resolves from (mention type or legacy key).
    pub resolved_key: Option<String>,
    /// The classified state.
    pub state: MissionTypeState,
    /// The read diagnostic, when there is one.
    pub error: Option<String>,
}

impl MissionTypeRow {
    /// The upstream `--json` row shape.
    pub fn to_json(&self) -> Value {
        let mut row = Map::new();
        row.insert("path".into(), json!(self.path.to_string_lossy()));
        row.insert("slug".into(), json!(self.slug));
        row.insert(
            "mission_type_raw".into(),
            self.mission_type_raw
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        row.insert(
            "resolved_key".into(),
            self.resolved_key
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        row.insert("state".into(), json!(self.state.as_str()));
        row.insert(
            "error".into(),
            self.error.clone().map(Value::from).unwrap_or(Value::Null),
        );
        Value::Object(row)
    }
}

/// The whole mission-type audit for one repository.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionTypeAudit {
    /// One row per selectable Mission, in directory order.
    pub rows: Vec<MissionTypeRow>,
}

impl MissionTypeAudit {
    /// Zero-filled counts for all six states.
    pub fn counts(&self) -> std::collections::BTreeMap<&'static str, usize> {
        let mut counts: std::collections::BTreeMap<&'static str, usize> = MISSION_TYPE_STATES
            .into_iter()
            .map(|state| (state.as_str(), 0))
            .collect();
        for row in &self.rows {
            *counts.entry(row.state.as_str()).or_insert(0) += 1;
        }
        counts
    }

    /// Whether any row is in one of the requested states.
    pub fn any_in(&self, states: &BTreeSet<MissionTypeState>) -> bool {
        self.rows.iter().any(|row| states.contains(&row.state))
    }

    /// The upstream `--json` payload.
    pub fn to_json(&self, fail_on_triggered: bool) -> Value {
        let mut payload = Map::new();
        payload.insert(
            "summary".into(),
            Value::Object(
                self.counts()
                    .into_iter()
                    .map(|(state, count)| (state.to_owned(), json!(count)))
                    .collect(),
            ),
        );
        payload.insert(
            "missions".into(),
            Value::Array(self.rows.iter().map(MissionTypeRow::to_json).collect()),
        );
        payload.insert("fail_on_triggered".into(), json!(fail_on_triggered));
        Value::Object(payload)
    }
}

/// `str.strip()` — the canonical mission-type key normalization (no case
/// folding, no default).
fn canonical_key(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn classify(
    path: &Path,
    slug: &str,
    meta: &super::meta::MetaRead,
    health: &dyn MissionTypeHealth,
) -> MissionTypeRow {
    let error = match meta {
        super::meta::MetaRead::Unreadable(detail) => Some(detail.clone()),
        _ => None,
    };
    if let Some(error) = error {
        return MissionTypeRow {
            path: path.to_path_buf(),
            slug: slug.to_owned(),
            mission_type_raw: None,
            resolved_key: None,
            state: MissionTypeState::Error,
            error: Some(error),
        };
    }
    let Some(object) = meta.object() else {
        // Absent `meta.json` cannot happen for a selectable child, but the
        // classifier must not invent a type if it does: report it as an error.
        return MissionTypeRow {
            path: path.to_path_buf(),
            slug: slug.to_owned(),
            mission_type_raw: None,
            resolved_key: None,
            state: MissionTypeState::Error,
            error: Some("meta.json not found".to_owned()),
        };
    };

    if let Some(raw_value) = object.get("mission_type") {
        let raw_string = raw_value.as_str();
        let Some(key) = raw_string.and_then(canonical_key) else {
            return MissionTypeRow {
                path: path.to_path_buf(),
                slug: slug.to_owned(),
                mission_type_raw: raw_string.map(str::to_owned),
                resolved_key: None,
                state: MissionTypeState::Typeless,
                error: None,
            };
        };
        let state = if !health.activated().contains(key) {
            MissionTypeState::Unknown
        } else if health.resolves(key) {
            MissionTypeState::Resolved
        } else {
            MissionTypeState::ActivatedUnresolvable
        };
        return MissionTypeRow {
            path: path.to_path_buf(),
            slug: slug.to_owned(),
            mission_type_raw: raw_string.map(str::to_owned),
            resolved_key: Some(key.to_owned()),
            state,
            error: None,
        };
    }

    let legacy_key = object
        .get("mission")
        .and_then(Value::as_str)
        .and_then(canonical_key);
    let state = if legacy_key.is_some() {
        MissionTypeState::LegacyKeyOnly
    } else {
        MissionTypeState::Typeless
    };
    MissionTypeRow {
        path: path.to_path_buf(),
        slug: slug.to_owned(),
        mission_type_raw: None,
        resolved_key: legacy_key.map(str::to_owned),
        state,
        error: None,
    }
}

/// Audit every selectable Mission's mission-type resolution.
pub fn audit_mission_types(
    root: &Path,
    health: &dyn MissionTypeHealth,
) -> Result<MissionTypeAudit, DoctorError> {
    require_directory(root)?;
    let mut rows = Vec::new();
    // One tracked-path probe per walk, and none when every child carries an
    // identity (the hardened posture Mission selection uses).
    let mut tracked: Option<GitAnswer<BTreeSet<PathBuf>>> = None;

    for child in git_probe::kitty_specs_children(root) {
        let meta = read_meta(&child);
        // Mission selection's population: a child the read-side rule rejects is
        // reported by `doctor identity` as residue, not here as typeless.
        let tracked = tracked
            .get_or_insert_with(|| git_probe::tracked_paths(root, git_probe::SPECS_DIR_NAME));
        let selectable = match &meta {
            super::meta::MetaRead::Absent => false,
            super::meta::MetaRead::Unreadable(_) => match tracked {
                // Hardened posture: an unreadable index degrades to file
                // existence, never to a silent skip.
                GitAnswer::Answered(tracked) => marker_tracked(root, &child, tracked),
                GitAnswer::NoRepository => true,
                GitAnswer::Unreadable(_) => true,
            },
            super::meta::MetaRead::Present(object) => {
                object
                    .get("mission_id")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
                    || match tracked {
                        GitAnswer::Answered(tracked) => marker_tracked(root, &child, tracked),
                        GitAnswer::NoRepository => true,
                        GitAnswer::Unreadable(_) => true,
                    }
            }
        };
        if !selectable {
            continue;
        }
        let slug = child
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        rows.push(classify(&child, &slug, &meta, health));
    }

    Ok(MissionTypeAudit { rows })
}

fn marker_tracked(root: &Path, child: &Path, tracked: &BTreeSet<PathBuf>) -> bool {
    ["spec.md", "meta.json"].into_iter().any(|name| {
        git_probe::relative_to(root, &child.join(name))
            .is_some_and(|relative| tracked.contains(&relative))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct Facts {
        activated: BTreeSet<String>,
        resolvable: BTreeSet<String>,
    }

    impl MissionTypeHealth for Facts {
        fn activated(&self) -> &BTreeSet<String> {
            &self.activated
        }

        fn resolves(&self, mission_type: &str) -> bool {
            self.resolvable.contains(mission_type)
        }
    }

    fn facts(activated: &[&str], resolvable: &[&str]) -> Facts {
        Facts {
            activated: activated.iter().map(|item| (*item).to_owned()).collect(),
            resolvable: resolvable.iter().map(|item| (*item).to_owned()).collect(),
        }
    }

    fn mission(root: &Path, slug: &str, meta: &str) {
        let dir = root.join("kitty-specs").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meta.json"), meta).unwrap();
    }

    #[test]
    fn the_six_states_classify_from_meta_plus_the_supplied_facts() {
        let root = tempfile::tempdir().unwrap();
        mission(
            root.path(),
            "001-resolved",
            "{\"mission_id\":\"01A\",\"mission_type\":\"research\"}",
        );
        mission(
            root.path(),
            "002-unresolvable",
            "{\"mission_id\":\"01B\",\"mission_type\":\" custom \"}",
        );
        mission(
            root.path(),
            "003-unknown",
            "{\"mission_id\":\"01C\",\"mission_type\":\"nope\"}",
        );
        mission(root.path(), "004-typeless", "{\"mission_id\":\"01D\"}");
        mission(
            root.path(),
            "005-legacy",
            "{\"mission_id\":\"01E\",\"mission\":\"research\"}",
        );
        mission(root.path(), "006-error", "{[oops");

        let audit =
            audit_mission_types(root.path(), &facts(&["research", "custom"], &["research"]))
                .unwrap();
        let state = |slug: &str| {
            audit
                .rows
                .iter()
                .find(|row| row.slug == slug)
                .expect("row")
                .state
        };
        assert_eq!(state("001-resolved"), MissionTypeState::Resolved);
        assert_eq!(
            state("002-unresolvable"),
            MissionTypeState::ActivatedUnresolvable
        );
        assert_eq!(state("003-unknown"), MissionTypeState::Unknown);
        assert_eq!(state("004-typeless"), MissionTypeState::Typeless);
        assert_eq!(state("005-legacy"), MissionTypeState::LegacyKeyOnly);
        assert_eq!(state("006-error"), MissionTypeState::Error);
        assert_eq!(audit.counts()["resolved"], 1);
    }

    #[test]
    fn a_present_blank_mission_type_never_falls_through_to_the_legacy_key() {
        let root = tempfile::tempdir().unwrap();
        mission(
            root.path(),
            "010-blank",
            "{\"mission_id\":\"01F\",\"mission_type\":\"  \",\"mission\":\"research\"}",
        );
        mission(
            root.path(),
            "011-null",
            "{\"mission_id\":\"01G\",\"mission_type\":null,\"mission\":\"research\"}",
        );
        let audit = audit_mission_types(root.path(), &facts(&["research"], &["research"])).unwrap();
        for row in &audit.rows {
            assert_eq!(row.state, MissionTypeState::Typeless, "{}", row.slug);
        }
    }

    #[test]
    fn residue_folders_are_not_classified_as_typeless() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("kitty-specs/020-residue")).unwrap();
        mission(root.path(), "021-typeless", "{\"mission_id\":\"01H\"}");
        let audit = audit_mission_types(root.path(), &facts(&[], &[])).unwrap();
        assert_eq!(audit.rows.len(), 1);
        assert_eq!(audit.rows[0].slug, "021-typeless");
    }

    #[test]
    fn fail_on_states_are_validated_not_ignored() {
        let states = parse_fail_on(Some("unknown, typeless")).unwrap();
        assert!(states.contains(&MissionTypeState::Unknown));
        let error = parse_fail_on(Some("unkown")).unwrap_err();
        assert_eq!(error.unknown, vec!["unkown".to_owned()]);
        assert!(error.message().contains("valid states are: resolved"));
        assert!(parse_fail_on(None).unwrap().is_empty());
    }
}
