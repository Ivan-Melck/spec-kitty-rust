//! Stored-topology audit (`doctor topology`).
//!
//! Reads the authoritative `topology` value persisted in `meta.json` **without**
//! re-inferring it from disk or git. A Mission that has not been backfilled yet
//! surfaces `topology: null`; an unreadable `meta.json` surfaces `topology: null`
//! with its `error`, so "no stored value" and "cannot read the stored value" stay
//! distinct.
//!
//! Two structural findings are attached, both read from the **stored** value so
//! they cannot drift from the mutation they name:
//!
//! * `SINGLE_BRANCH_CODE_LANES_UNMIGRATED` — a stored `single_branch` Mission
//!   whose `lanes.json` still carries a code lane.
//! * `LANES_MANIFEST_UNREADABLE` — a stored `single_branch` Mission whose
//!   `lanes.json` exists and cannot be read. This must never read as clean:
//!   an unreadable manifest might still hide a code lane.
//!
//! Both carry the operator remedy. A terminal (completed) Mission is never
//! flagged: it cannot be run again, so the finding would be churn — the same
//! completion predicate the re-stamp migration uses.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::git_probe;
use super::meta::{MetaRead, read_meta};
use super::{DoctorError, require_directory};
use crate::lanes::LanesStore;

/// Finding code: a stored `single_branch` Mission has a code lane.
pub const SINGLE_BRANCH_CODE_LANES_UNMIGRATED: &str = "SINGLE_BRANCH_CODE_LANES_UNMIGRATED";

/// Finding code: a stored `single_branch` Mission's `lanes.json` is unreadable.
pub const LANES_MANIFEST_UNREADABLE: &str = "LANES_MANIFEST_UNREADABLE";

/// A structural topology finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyFinding {
    /// A `single_branch` Mission still carries a code lane.
    SingleBranchCodeLanesUnmigrated,
    /// A `single_branch` Mission's `lanes.json` cannot be read.
    LanesManifestUnreadable,
}

impl TopologyFinding {
    /// The stable machine-readable code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleBranchCodeLanesUnmigrated => SINGLE_BRANCH_CODE_LANES_UNMIGRATED,
            Self::LanesManifestUnreadable => LANES_MANIFEST_UNREADABLE,
        }
    }

    /// The operator remedy, with `{slug}` filled in.
    pub fn remedy(self, slug: &str) -> String {
        match self {
            Self::SingleBranchCodeLanesUnmigrated => format!(
                "spec-kitty migrate backfill-topology --restamp-single-branch (or `spec-kitty upgrade`, which runs migration m_4_0_0rc5_single_branch_code_lanes_restamp) for {slug}"
            ),
            Self::LanesManifestUnreadable => format!(
                "restore lanes.json from git history: `git checkout <ref> -- kitty-specs/{slug}/lanes.json`; or delete it and rebuild: `spec-kitty agent mission finalize-tasks --mission {slug}` if no WP has started, `spec-kitty doctor mission-state --fix --mission {slug}` if execution has begun"
            ),
        }
    }
}

/// One Mission's stored-topology row.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyRow {
    /// The directory name used as the Mission slug.
    pub slug: String,
    /// The stored `topology` value, exactly as decoded (`None` when absent).
    pub topology: Option<Value>,
    /// The stored `flattened` value, exactly as decoded.
    pub flattened: Option<Value>,
    /// Why the stored value could not be read, when it could not.
    pub error: Option<String>,
    /// The structural finding for this Mission, if any.
    pub finding: Option<TopologyFinding>,
    /// The remedy for [`TopologyRow::finding`].
    pub remedy: Option<String>,
}

impl TopologyRow {
    /// The upstream `--json` row shape.
    pub fn to_json(&self) -> Value {
        let mut row = Map::new();
        row.insert("slug".into(), json!(self.slug));
        row.insert(
            "topology".into(),
            self.topology.clone().unwrap_or(Value::Null),
        );
        row.insert(
            "flattened".into(),
            self.flattened.clone().unwrap_or(Value::Null),
        );
        row.insert(
            "error".into(),
            self.error.clone().map(Value::from).unwrap_or(Value::Null),
        );
        row.insert(
            "finding".into(),
            self.finding
                .map(|finding| json!(finding.as_str()))
                .unwrap_or(Value::Null),
        );
        row.insert(
            "remedy".into(),
            self.remedy.clone().map(Value::from).unwrap_or(Value::Null),
        );
        Value::Object(row)
    }
}

/// The whole topology audit for one repository.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyAudit {
    /// One row per Mission, in directory order.
    pub rows: Vec<TopologyRow>,
}

impl TopologyAudit {
    /// The upstream `--json` payload.
    pub fn to_json(&self) -> Value {
        json!({"missions": self.rows.iter().map(TopologyRow::to_json).collect::<Vec<_>>()})
    }
}

fn topology_finding(topology: Option<&Value>, feature_dir: &Path) -> Option<TopologyFinding> {
    if topology.and_then(Value::as_str) != Some("single_branch") {
        return None;
    }

    let manifest = match LanesStore::new(feature_dir).read() {
        Ok(manifest) => manifest,
        // Every read failure of a present manifest is "cannot verify T-1",
        // never "clean": an unreadable manifest might hide a code lane.
        Err(_) => return Some(TopologyFinding::LanesManifestUnreadable),
    };
    let manifest = manifest?;
    if manifest.lanes.iter().all(|lane| lane.is_planning()) {
        return None;
    }
    // A terminal Mission is never run again; suppress the finding on it (the
    // re-stamp migration uses the same completion predicate).
    if sk_status::migration_backfill::mission_is_terminal(feature_dir) {
        return None;
    }
    Some(TopologyFinding::SingleBranchCodeLanesUnmigrated)
}

fn read_row(feature_dir: &Path) -> TopologyRow {
    let slug = feature_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let meta = read_meta(feature_dir);
    let (topology, flattened, error) = match &meta {
        MetaRead::Absent => (None, None, Some("meta.json not found".to_owned())),
        MetaRead::Unreadable(detail) => (None, None, Some(format!("corrupt json: {detail}"))),
        MetaRead::Present(object) => (
            object.get("topology").cloned(),
            object.get("flattened").cloned(),
            None,
        ),
    };

    let finding = match (&meta, error.is_some()) {
        (MetaRead::Present(_), false) => topology_finding(topology.as_ref(), feature_dir),
        _ => None,
    };

    TopologyRow {
        slug: slug.clone(),
        topology,
        flattened,
        error,
        remedy: finding.map(|finding| finding.remedy(&slug)),
        finding,
    }
}

/// Audit each Mission's stored topology (optionally scoped to one directory).
///
/// Returns `Ok(None)` when `mission` is given and its directory does not exist
/// (the caller renders "Mission not found").
pub fn audit_topology(
    root: &Path,
    mission: Option<&str>,
) -> Result<Option<TopologyAudit>, DoctorError> {
    require_directory(root)?;

    if let Some(handle) = mission.filter(|value| !value.is_empty()) {
        let target: PathBuf = root.join(git_probe::SPECS_DIR_NAME).join(handle);
        if !std::fs::metadata(&target).is_ok_and(|metadata| metadata.is_dir()) {
            return Ok(None);
        }
        return Ok(Some(TopologyAudit {
            rows: vec![read_row(&target)],
        }));
    }

    Ok(Some(TopologyAudit {
        rows: git_probe::kitty_specs_children(root)
            .iter()
            .map(|feature_dir| read_row(feature_dir))
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mission(root: &Path, slug: &str, meta: &str) -> PathBuf {
        let dir = root.join("kitty-specs").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meta.json"), meta).unwrap();
        dir
    }

    #[test]
    fn stored_topology_is_reported_verbatim_and_missing_is_null() {
        let root = tempfile::tempdir().unwrap();
        mission(root.path(), "001-lanes", "{\"topology\":\"lanes\"}");
        mission(root.path(), "002-unset", "{}");
        mission(root.path(), "003-corrupt", "{[oops");

        let audit = audit_topology(root.path(), None).unwrap().expect("audit");
        assert_eq!(audit.rows[0].topology, Some(json!("lanes")));
        assert_eq!(audit.rows[1].topology, None);
        assert_eq!(audit.rows[1].error, None);
        assert!(
            audit.rows[2]
                .error
                .as_deref()
                .unwrap()
                .starts_with("corrupt json:")
        );
    }

    #[test]
    fn a_single_branch_mission_with_a_code_lane_is_flagged_with_a_remedy() {
        let root = tempfile::tempdir().unwrap();
        let dir = mission(root.path(), "010-sb", "{\"topology\":\"single_branch\"}");
        std::fs::write(
            dir.join("lanes.json"),
            r#"{"version":1,"mission_slug":"010-sb","mission_branch":"kitty/mission-010-sb","target_branch":"main","lanes":[{"lane_id":"lane-a","wp_ids":["WP01"]}],"computed_at":"2026-01-01T00:00:00+00:00","computed_from":"test"}"#,
        )
        .unwrap();

        let audit = audit_topology(root.path(), None).unwrap().expect("audit");
        assert_eq!(
            audit.rows[0].finding,
            Some(TopologyFinding::SingleBranchCodeLanesUnmigrated)
        );
        assert!(
            audit.rows[0]
                .remedy
                .as_deref()
                .unwrap()
                .contains("--restamp-single-branch")
        );
    }

    #[test]
    fn a_planning_only_single_branch_mission_is_clean() {
        let root = tempfile::tempdir().unwrap();
        let dir = mission(root.path(), "011-sb", "{\"topology\":\"single_branch\"}");
        std::fs::write(
            dir.join("lanes.json"),
            r#"{"version":1,"mission_slug":"011-sb","mission_branch":"kitty/mission-011-sb","target_branch":"main","lanes":[{"lane_id":"lane-planning","wp_ids":["WP01"]}],"computed_at":"2026-01-01T00:00:00+00:00","computed_from":"test"}"#,
        )
        .unwrap();

        let audit = audit_topology(root.path(), None).unwrap().expect("audit");
        assert_eq!(audit.rows[0].finding, None);
        assert_eq!(audit.rows[0].remedy, None);
    }

    #[test]
    fn an_unreadable_lanes_manifest_never_reads_as_clean() {
        let root = tempfile::tempdir().unwrap();
        let dir = mission(root.path(), "012-sb", "{\"topology\":\"single_branch\"}");
        std::fs::write(dir.join("lanes.json"), "{not json").unwrap();

        let audit = audit_topology(root.path(), None).unwrap().expect("audit");
        assert_eq!(
            audit.rows[0].finding,
            Some(TopologyFinding::LanesManifestUnreadable)
        );
        assert!(
            audit.rows[0]
                .remedy
                .as_deref()
                .unwrap()
                .contains("restore lanes.json")
        );
    }

    #[test]
    fn scoping_reports_one_row_or_refuses_an_unknown_directory() {
        let root = tempfile::tempdir().unwrap();
        mission(root.path(), "020-scoped", "{\"topology\":\"lanes\"}");
        let scoped = audit_topology(root.path(), Some("020-scoped"))
            .unwrap()
            .expect("known");
        assert_eq!(scoped.rows.len(), 1);
        assert!(
            audit_topology(root.path(), Some("021-missing"))
                .unwrap()
                .is_none()
        );
    }
}
