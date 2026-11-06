//! Mission-identity health audit (`doctor identity`).
//!
//! Every child of `kitty-specs/` is classified into one of four Mission states
//! plus `residue`:
//!
//! | state | condition |
//! |---|---|
//! | `assigned` | `mission_id` present **and** `mission_number` non-null |
//! | `pending` | `mission_id` present **and** `mission_number` null |
//! | `legacy` | `mission_id` missing **and** `mission_number` present |
//! | `orphan` | both missing, or `meta.json` unreadable |
//! | `residue` | not a Mission: no `meta.json`; or, in a git repository, an untracked id-less `meta.json`; or an unreadable one |
//!
//! Two collision reports sit beside the rows: duplicate 3-digit numeric
//! prefixes (`FR-011`) and selector handles that would resolve to more than one
//! Mission (`FR-012`). A `residue` folder is left out of both — Mission
//! selection does not list it, so it can neither collide with nor silence a
//! real Mission.
//!
//! # Read-only contract
//!
//! Nothing here writes, materializes, or repairs. A corrupt `meta.json` is
//! reported *as corrupt* and never as a folder to delete. A git index that
//! cannot be read refuses the audit instead of degrading to "nothing is
//! tracked".
//!
//! # Preserved ambiguity
//!
//! `mission_id` is reported exactly as stored — presence is Python truthiness,
//! not ULID validation — so a stored value the resolver would not accept is
//! still visible here rather than silently reclassified.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::git_probe::{self, GitAnswer};
use super::meta::{MetaRead, coerce_mission_number, python_truthy, read_meta};
use super::{DoctorError, require_directory};

/// The four Mission states, in report order, plus `residue`.
pub const IDENTITY_STATES: [IdentityState; 5] = [
    IdentityState::Assigned,
    IdentityState::Pending,
    IdentityState::Legacy,
    IdentityState::Orphan,
    IdentityState::Residue,
];

/// Residue reasons, in report order.
pub const RESIDUE_REASONS: [ResidueReason; 3] = [
    ResidueReason::NoMeta,
    ResidueReason::UntrackedIdlessMeta,
    ResidueReason::CorruptMeta,
];

/// The identity state of one `kitty-specs/` child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdentityState {
    /// `mission_id` and `mission_number` are both present.
    Assigned,
    /// `mission_id` present, `mission_number` not assigned yet.
    Pending,
    /// `mission_number` present, `mission_id` missing (needs backfill).
    Legacy,
    /// Both missing, or `meta.json` unreadable (needs triage).
    Orphan,
    /// Not a Mission; Mission selection does not list it.
    Residue,
}

impl IdentityState {
    /// The stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Pending => "pending",
            Self::Legacy => "legacy",
            Self::Orphan => "orphan",
            Self::Residue => "residue",
        }
    }

    /// Parse a `--fail-on` token.
    pub fn parse(value: &str) -> Option<Self> {
        IDENTITY_STATES
            .into_iter()
            .find(|state| state.as_str() == value)
    }
}

/// Why a `kitty-specs/` child is residue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResidueReason {
    /// No `meta.json` on disk.
    NoMeta,
    /// A `meta.json` without a `mission_id` that git does not track.
    UntrackedIdlessMeta,
    /// A `meta.json` that cannot be read.
    CorruptMeta,
}

impl ResidueReason {
    /// The stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoMeta => "no_meta",
            Self::UntrackedIdlessMeta => "untracked_idless_meta",
            Self::CorruptMeta => "corrupt_meta",
        }
    }

    /// The operator remedy for this reason (upstream text, verbatim).
    pub fn remedy(self) -> &'static str {
        match self {
            Self::NoMeta => RESIDUE_REMEDY_NO_META,
            Self::UntrackedIdlessMeta => RESIDUE_REMEDY_UNTRACKED_IDLESS_META,
            Self::CorruptMeta => RESIDUE_REMEDY_CORRUPT_META,
        }
    }
}

/// The remedy for `no_meta` residue (`doctor identity`).
pub const RESIDUE_REMEDY_NO_META: &str = "No meta.json on disk, so Mission selection does not list it. If it is leftover residue, delete the folder. If it is a real Mission whose meta.json was deleted: when `git status` shows the deletion, run `git restore <path>/meta.json`; when the deletion was committed, find it with `git log --diff-filter=D --format=%h -1 -- <path>/meta.json` and run `git checkout <sha>~1 -- <path>/meta.json`. If its meta.json was never committed, re-create the Mission with `spec-kitty agent mission create` and move its spec.md into the new folder.";

/// The remedy for `untracked_idless_meta` residue (`doctor identity`).
pub const RESIDUE_REMEDY_UNTRACKED_IDLESS_META: &str = "A meta.json without a mission_id that git does not track (nor the spec.md), so Mission selection does not list it. If it is a real Mission, commit its meta.json or run `spec-kitty migrate backfill-identity`; otherwise delete the folder.";

/// The remedy for `corrupt_meta` residue (`doctor identity`).
pub const RESIDUE_REMEDY_CORRUPT_META: &str = "The Mission content is corrupt: meta.json could not be read (the error is on each row). Do not delete the folder before checking. Repair it: fix its permissions or its JSON. If `git status` shows it staged as deleted, restore it with `git checkout HEAD -- <path>/meta.json`. If a commit removed it, find that commit with `git log --diff-filter=D --format=%h -1 -- <path>/meta.json` and restore it with `git checkout <sha>~1 -- <path>/meta.json`. Otherwise re-create the Mission with `spec-kitty agent mission create` and move its content in.";

/// One audited `kitty-specs/` child.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityRow {
    /// Absolute (or root-joined) path of the child.
    pub path: PathBuf,
    /// The directory name, used as the Mission slug.
    pub slug: String,
    /// The stored `mission_id` value, exactly as decoded, when truthy.
    pub mission_id: Option<Value>,
    /// The coerced `mission_number`, when it coerces.
    pub mission_number: Option<i64>,
    /// The classified state.
    pub state: IdentityState,
    /// The read/coercion diagnostic, when there is one.
    pub error: Option<String>,
    /// Why this row is residue, when it is.
    pub residue_reason: Option<ResidueReason>,
}

impl IdentityRow {
    /// The upstream `--json` row shape.
    pub fn to_json(&self) -> Value {
        let mut row = Map::new();
        row.insert("path".into(), json!(path_text(&self.path)));
        row.insert("slug".into(), json!(self.slug));
        row.insert(
            "mission_id".into(),
            self.mission_id.clone().unwrap_or(Value::Null),
        );
        row.insert(
            "mission_number".into(),
            self.mission_number.map(Value::from).unwrap_or(Value::Null),
        );
        row.insert("state".into(), json!(self.state.as_str()));
        row.insert(
            "error".into(),
            self.error.clone().map(Value::from).unwrap_or(Value::Null),
        );
        row.insert(
            "residue_reason".into(),
            self.residue_reason
                .map(|reason| json!(reason.as_str()))
                .unwrap_or(Value::Null),
        );
        Value::Object(row)
    }
}

/// The whole identity audit for one repository.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityAudit {
    /// One row per audited child, in directory order.
    pub rows: Vec<IdentityRow>,
    /// `prefix -> rows` for every prefix shared by two or more Missions.
    pub duplicate_prefixes: BTreeMap<String, Vec<IdentityRow>>,
    /// `handle -> rows` for every handle that resolves to two or more Missions.
    pub ambiguous_selectors: BTreeMap<String, Vec<IdentityRow>>,
}

impl IdentityAudit {
    /// Zero-filled counts for all five states.
    pub fn counts(&self) -> BTreeMap<&'static str, usize> {
        let mut counts: BTreeMap<&'static str, usize> = IDENTITY_STATES
            .into_iter()
            .map(|state| (state.as_str(), 0))
            .collect();
        for row in &self.rows {
            *counts.entry(row.state.as_str()).or_insert(0) += 1;
        }
        counts
    }

    /// The residue reasons present, in report order.
    pub fn residue_reasons(&self) -> Vec<ResidueReason> {
        RESIDUE_REASONS
            .into_iter()
            .filter(|reason| {
                self.rows
                    .iter()
                    .any(|row| row.residue_reason == Some(*reason))
            })
            .collect()
    }

    /// Whether any row is in one of the requested states.
    pub fn any_in(&self, states: &BTreeSet<IdentityState>) -> bool {
        self.rows.iter().any(|row| states.contains(&row.state))
    }

    /// The upstream `--json` payload (remedies only when residue is present).
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
            Value::Array(self.rows.iter().map(IdentityRow::to_json).collect()),
        );
        payload.insert(
            "duplicate_prefixes".into(),
            Value::Object(
                self.duplicate_prefixes
                    .iter()
                    .map(|(prefix, rows)| {
                        (
                            prefix.clone(),
                            Value::Array(rows.iter().map(IdentityRow::to_json).collect()),
                        )
                    })
                    .collect(),
            ),
        );
        payload.insert(
            "ambiguous_selectors".into(),
            Value::Object(
                self.ambiguous_selectors
                    .iter()
                    .map(|(handle, rows)| {
                        (
                            handle.clone(),
                            Value::Array(rows.iter().map(IdentityRow::to_json).collect()),
                        )
                    })
                    .collect(),
            ),
        );
        payload.insert("fail_on_triggered".into(), json!(fail_on_triggered));
        let reasons = self.residue_reasons();
        if !reasons.is_empty() {
            payload.insert(
                "residue_remedies".into(),
                Value::Object(
                    reasons
                        .into_iter()
                        .map(|reason| (reason.as_str().to_owned(), json!(reason.remedy())))
                        .collect(),
                ),
            );
        }
        Value::Object(payload)
    }
}

/// How an unreadable git index is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackedPosture {
    /// Refuse the audit — `doctor identity` is a reporter, so a broken index is
    /// one error line, never a weaker answer.
    Strict,
    /// Degrade to file presence — the hardened posture Mission selection uses.
    Degrade,
}

/// A lazily probed, memoized tracked-path set for one walk.
struct TrackedMemo<'a> {
    root: &'a Path,
    posture: TrackedPosture,
    answer: Option<GitAnswer<BTreeSet<PathBuf>>>,
}

impl<'a> TrackedMemo<'a> {
    fn new(root: &'a Path, posture: TrackedPosture) -> Self {
        Self {
            root,
            posture,
            answer: None,
        }
    }

    fn get(&mut self) -> Result<&GitAnswer<BTreeSet<PathBuf>>, DoctorError> {
        if self.answer.is_none() {
            let probed = git_probe::tracked_paths(self.root, git_probe::SPECS_DIR_NAME);
            if let (TrackedPosture::Strict, GitAnswer::Unreadable(detail)) = (self.posture, &probed)
            {
                return Err(DoctorError::GitUnavailable {
                    what: "the `kitty-specs/` tracked paths",
                    detail: detail.clone(),
                });
            }
            self.answer = Some(probed);
        }
        Ok(self
            .answer
            .as_ref()
            .expect("the tracked probe was just initialised"))
    }
}

/// The read-side Mission test: is this child listed by Mission selection?
///
/// 1. No `meta.json` on disk → not a Mission (a `spec.md` alone never makes one).
/// 2. A non-empty **string** `mission_id` → a Mission, with no git call.
/// 3. Otherwise git decides: an id-less `meta.json` counts only when git tracks
///    `spec.md` or `meta.json`. With no usable repository the file exists, so it
///    counts.
fn is_selectable(
    dir: &Path,
    meta: &MetaRead,
    memo: &mut TrackedMemo<'_>,
) -> Result<bool, DoctorError> {
    if !meta.present_on_disk() {
        return Ok(false);
    }
    if carries_identity(meta) {
        return Ok(true);
    }
    let root = memo.root;
    match memo.get()? {
        GitAnswer::Answered(tracked) => Ok(["spec.md", "meta.json"].into_iter().any(|name| {
            git_probe::relative_to(root, &dir.join(name))
                .is_some_and(|relative| tracked.contains(&relative))
        })),
        GitAnswer::NoRepository => Ok(true),
        GitAnswer::Unreadable(_) => match memo.posture {
            // The strict posture already refused in `get`.
            TrackedPosture::Strict => Err(DoctorError::GitUnavailable {
                what: "the `kitty-specs/` tracked paths",
                detail: "the tracked-path probe failed".to_owned(),
            }),
            // A hardened caller falls back to file existence.
            TrackedPosture::Degrade => Ok(true),
        },
    }
}

fn carries_identity(meta: &MetaRead) -> bool {
    meta.object()
        .and_then(|object| object.get("mission_id"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

/// Classify one Mission directory from its already-read `meta.json`.
fn classify_rows(slug: &str, path: &Path, meta: &MetaRead) -> IdentityRow {
    match meta {
        MetaRead::Absent => IdentityRow {
            path: path.to_path_buf(),
            slug: slug.to_owned(),
            mission_id: None,
            mission_number: None,
            state: IdentityState::Orphan,
            error: Some("meta.json not found".to_owned()),
            residue_reason: None,
        },
        MetaRead::Unreadable(detail) => IdentityRow {
            path: path.to_path_buf(),
            slug: slug.to_owned(),
            mission_id: None,
            mission_number: None,
            state: IdentityState::Orphan,
            error: Some(detail.clone()),
            residue_reason: None,
        },
        MetaRead::Present(object) => {
            let mission_id = object
                .get("mission_id")
                .filter(|value| python_truthy(value))
                .cloned();
            let (mission_number, coercion_error) = match object.get("mission_number") {
                Some(raw) => match coerce_mission_number(raw) {
                    Ok(number) => (number, None),
                    Err(error) => (None, Some(error.detail)),
                },
                None => (None, None),
            };
            let state = match (mission_id.is_some(), mission_number.is_some()) {
                (true, true) => IdentityState::Assigned,
                (true, false) => IdentityState::Pending,
                (false, true) => IdentityState::Legacy,
                (false, false) => IdentityState::Orphan,
            };
            IdentityRow {
                path: path.to_path_buf(),
                slug: slug.to_owned(),
                mission_id,
                mission_number,
                state,
                error: coercion_error,
                residue_reason: None,
            }
        }
    }
}

/// The residue row for a child that is not a Mission, with its reason.
fn residue_row(slug: &str, path: &Path, meta: &MetaRead) -> IdentityRow {
    let (reason, error) = match meta {
        MetaRead::Absent => (ResidueReason::NoMeta, None),
        MetaRead::Unreadable(detail) => (ResidueReason::CorruptMeta, Some(detail.clone())),
        MetaRead::Present(_) => (ResidueReason::UntrackedIdlessMeta, None),
    };
    IdentityRow {
        path: path.to_path_buf(),
        slug: slug.to_owned(),
        mission_id: None,
        mission_number: None,
        state: IdentityState::Residue,
        error,
        residue_reason: Some(reason),
    }
}

fn slug_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Audit the whole `kitty-specs/` inventory (optionally scoped to one handle).
///
/// Returns `Ok(None)` when `mission` is given and no such child exists — the
/// caller renders the upstream "Mission not found" line — and
/// [`DoctorError::GitUnavailable`] when the index cannot be read.
pub fn audit_identity(
    root: &Path,
    mission: Option<&str>,
    posture: TrackedPosture,
) -> Result<Option<IdentityAudit>, DoctorError> {
    require_directory(root)?;

    let mut memo = TrackedMemo::new(root, posture);
    let mut rows: Vec<IdentityRow> = Vec::new();
    let mut duplicate_prefixes: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut prefix_members: Vec<(String, usize)> = Vec::new();

    for child in git_probe::kitty_specs_children(root) {
        let slug = slug_of(&child);
        let meta = read_meta(&child);
        let selectable = is_selectable(&child, &meta, &mut memo)?;
        let row = if selectable {
            classify_rows(&slug, &child, &meta)
        } else {
            residue_row(&slug, &child, &meta)
        };

        if let Some(prefix) = git_probe::numeric_prefix(&slug)
            && row.state != IdentityState::Residue
        {
            prefix_members.push((prefix.to_owned(), rows.len()));
        }
        rows.push(row);
    }

    for (prefix, index) in prefix_members {
        duplicate_prefixes.entry(prefix).or_default().push(index);
    }
    let duplicate_prefixes: BTreeMap<String, Vec<IdentityRow>> = duplicate_prefixes
        .into_iter()
        .filter(|(_, indices)| indices.len() >= 2)
        .map(|(prefix, indices)| {
            (
                prefix,
                indices
                    .into_iter()
                    .map(|index| rows[index].clone())
                    .collect(),
            )
        })
        .collect();

    let all_selectors = ambiguous_selectors(&rows);

    if let Some(handle) = mission.filter(|value| !value.is_empty()) {
        let scoped: Vec<IdentityRow> = {
            let matching: Vec<IdentityRow> = rows
                .iter()
                .filter(|row| row.slug == handle)
                .cloned()
                .collect();
            if !matching.is_empty() {
                matching
            } else {
                // The upstream audit classifies a residue folder addressed
                // directly too, so `--mission <residue>` reports `residue`.
                let target = root.join(git_probe::SPECS_DIR_NAME).join(handle);
                if std::fs::metadata(&target).is_ok_and(|metadata| metadata.is_dir()) {
                    let meta = read_meta(&target);
                    let slug = slug_of(&target);
                    let selectable = is_selectable(&target, &meta, &mut memo)?;
                    vec![if selectable {
                        classify_rows(&slug, &target, &meta)
                    } else {
                        residue_row(&slug, &target, &meta)
                    }]
                } else {
                    Vec::new()
                }
            }
        };
        if scoped.is_empty() {
            return Ok(None);
        }
        let prefix = git_probe::numeric_prefix(handle).unwrap_or_default();
        let duplicate_prefixes = if prefix.is_empty() {
            BTreeMap::new()
        } else {
            duplicate_prefixes
                .into_iter()
                .filter(|(candidate, _)| candidate == prefix)
                .collect()
        };
        return Ok(Some(IdentityAudit {
            ambiguous_selectors: ambiguous_selectors(&scoped),
            rows: scoped,
            duplicate_prefixes,
        }));
    }

    Ok(Some(IdentityAudit {
        rows,
        duplicate_prefixes,
        ambiguous_selectors: all_selectors,
    }))
}

/// Handles that resolve to two or more Missions (full slug, numeric prefix,
/// and human slug forms). Residue rows are skipped: selection does not list
/// them.
fn ambiguous_selectors(rows: &[IdentityRow]) -> BTreeMap<String, Vec<IdentityRow>> {
    let mut handles: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        if row.state == IdentityState::Residue {
            continue;
        }
        let mut forms: BTreeSet<&str> = BTreeSet::new();
        forms.insert(row.slug.as_str());
        if let Some(prefix) = git_probe::numeric_prefix(&row.slug) {
            forms.insert(prefix);
        }
        let human = crate::naming::strip_numeric_prefix(&row.slug);
        if human != row.slug {
            forms.insert(human);
        }
        for handle in forms {
            handles.entry(handle.to_owned()).or_default().push(index);
        }
    }
    handles
        .into_iter()
        .filter(|(_, indices)| indices.len() >= 2)
        .map(|(handle, indices)| {
            (
                handle,
                indices
                    .into_iter()
                    .map(|index| rows[index].clone())
                    .collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_meta(dir: &Path, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("meta.json"), body).unwrap();
    }

    fn mission(root: &Path, slug: &str) -> PathBuf {
        let dir = root.join("kitty-specs").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn row<'a>(audit: &'a IdentityAudit, slug: &str) -> &'a IdentityRow {
        audit
            .rows
            .iter()
            .find(|row| row.slug == slug)
            .expect("row present")
    }

    #[test]
    fn the_four_states_classify_from_the_stored_pair() {
        let root = tempfile::tempdir().unwrap();
        write_meta(
            &mission(root.path(), "001-assigned"),
            "{\"mission_id\":\"01A\",\"mission_number\":3}",
        );
        write_meta(
            &mission(root.path(), "002-pending"),
            "{\"mission_id\":\"01B\",\"mission_number\":null}",
        );
        write_meta(
            &mission(root.path(), "003-legacy"),
            "{\"mission_number\":7}",
        );
        write_meta(&mission(root.path(), "004-orphan"), "{}");

        let audit = audit_identity(root.path(), None, TrackedPosture::Degrade)
            .unwrap()
            .expect("audit");
        assert_eq!(row(&audit, "001-assigned").state, IdentityState::Assigned);
        assert_eq!(row(&audit, "002-pending").state, IdentityState::Pending);
        assert_eq!(row(&audit, "003-legacy").state, IdentityState::Legacy);
        assert_eq!(row(&audit, "003-legacy").mission_number, Some(7));
        assert_eq!(row(&audit, "004-orphan").state, IdentityState::Orphan);
        assert_eq!(audit.counts()["assigned"], 1);
        assert_eq!(audit.counts()["residue"], 0);
    }

    #[test]
    fn residue_reasons_are_distinguished_and_never_healthy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("kitty-specs/050-no-meta")).unwrap();
        write_meta(&mission(root.path(), "051-corrupt"), "{[oops");
        write_meta(&mission(root.path(), "052-idless"), "{\"slug\":\"x\"}");

        let audit = audit_identity(root.path(), None, TrackedPosture::Degrade)
            .unwrap()
            .expect("audit");
        assert_eq!(
            row(&audit, "050-no-meta").residue_reason,
            Some(ResidueReason::NoMeta)
        );
        // No usable git repository: an existing `meta.json` still counts, so a
        // corrupt one is a Mission row carrying its error (the reference's
        // existence fallback for `tracked=None`), not residue.
        assert_eq!(
            row(&audit, "052-idless").state,
            IdentityState::Orphan,
            "without git, file existence keeps it a Mission row"
        );
        assert!(
            row(&audit, "051-corrupt").error.is_some(),
            "a corrupt meta keeps its read error even when it is a Mission row"
        );
        assert_eq!(row(&audit, "051-corrupt").residue_reason, None);
        assert_eq!(audit.residue_reasons(), vec![ResidueReason::NoMeta]);
    }

    #[test]
    fn duplicate_prefixes_and_ambiguous_selectors_ignore_residue() {
        let root = tempfile::tempdir().unwrap();
        write_meta(
            &mission(root.path(), "060-alpha"),
            "{\"mission_id\":\"01C\",\"mission_number\":1}",
        );
        write_meta(
            &mission(root.path(), "060-beta"),
            "{\"mission_id\":\"01D\",\"mission_number\":2}",
        );
        write_meta(
            &mission(root.path(), "061-alpha"),
            "{\"mission_id\":\"01E\",\"mission_number\":3}",
        );
        std::fs::create_dir_all(root.path().join("kitty-specs/060-residue")).unwrap();

        let audit = audit_identity(root.path(), None, TrackedPosture::Degrade)
            .unwrap()
            .expect("audit");
        assert_eq!(
            audit.duplicate_prefixes.get("060").map(Vec::len),
            Some(2),
            "the residue folder must not join the collision"
        );
        assert_eq!(
            audit
                .ambiguous_selectors
                .get("alpha")
                .map(|rows| rows.len()),
            Some(2)
        );
        assert!(!audit.ambiguous_selectors.contains_key("060-residue"));
    }

    #[test]
    fn scoping_reports_a_known_child_and_refuses_an_unknown_one() {
        let root = tempfile::tempdir().unwrap();
        write_meta(
            &mission(root.path(), "070-foo"),
            "{\"mission_id\":\"01F\",\"mission_number\":4}",
        );
        std::fs::create_dir_all(root.path().join("kitty-specs/071-residue")).unwrap();

        let scoped = audit_identity(root.path(), Some("070-foo"), TrackedPosture::Degrade)
            .unwrap()
            .expect("known mission");
        assert_eq!(scoped.rows.len(), 1);
        assert_eq!(scoped.rows[0].state, IdentityState::Assigned);

        let residue = audit_identity(root.path(), Some("071-residue"), TrackedPosture::Degrade)
            .unwrap()
            .expect("residue is classified when addressed directly");
        assert_eq!(residue.rows[0].state, IdentityState::Residue);

        assert!(
            audit_identity(root.path(), Some("999-missing"), TrackedPosture::Degrade)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn fail_on_state_membership_is_exact() {
        let root = tempfile::tempdir().unwrap();
        write_meta(
            &mission(root.path(), "080-legacy"),
            "{\"mission_number\":9}",
        );
        let audit = audit_identity(root.path(), None, TrackedPosture::Degrade)
            .unwrap()
            .expect("audit");
        let mut states = BTreeSet::new();
        states.insert(IdentityState::Legacy);
        assert!(audit.any_in(&states));
        states.clear();
        states.insert(IdentityState::Assigned);
        assert!(!audit.any_in(&states));
        assert_eq!(IdentityState::parse("orphan"), Some(IdentityState::Orphan));
        assert_eq!(IdentityState::parse("nope"), None);
    }

    #[test]
    fn payload_carries_remedies_only_when_residue_exists() {
        let root = tempfile::tempdir().unwrap();
        write_meta(
            &mission(root.path(), "090-mission"),
            "{\"mission_id\":\"01G\",\"mission_number\":1}",
        );
        let clean = audit_identity(root.path(), None, TrackedPosture::Degrade)
            .unwrap()
            .expect("audit")
            .to_json(false);
        assert!(clean.get("residue_remedies").is_none());

        std::fs::create_dir_all(root.path().join("kitty-specs/091-residue")).unwrap();
        let dirty = audit_identity(root.path(), None, TrackedPosture::Degrade)
            .unwrap()
            .expect("audit")
            .to_json(true);
        assert!(dirty["residue_remedies"]["no_meta"].is_string());
        assert_eq!(dirty["fail_on_triggered"], json!(true));
    }
}
