//! Lane ownership and allocation: `spec-kitty implement`.
//!
//! Allocation is executed by `sk-workspace` under the documented protected
//! branch policy; this module resolves the mission, the lane manifest and the
//! protected set, then renders the upstream `implement --json` payload.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_mission::{Mission, MissionTopology};
use sk_workspace::{
    AllocationRoute, LanesStore, MutationPolicy, PROTECTED_BRANCH_REFUSED, PolicyVerdict,
};

use super::support::{CompatError, emit_json, path_string};

/// Branches protected by default, matching the upstream refusal floor.
const DEFAULT_PROTECTED: &[&str] = &["main", "master"];

fn protected_branches(root: &Path) -> Result<sk_git::ProtectedBranches, CompatError> {
    let mut names: Vec<String> = DEFAULT_PROTECTED
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let config = root.join(".kittify/config.yaml");
    let invalid = |message: String| CompatError::coded("PROTECTION_CONFIG_INVALID", message);
    let bytes = match sk_kernel::read_bounded(&config, 4 * 1024 * 1024) {
        Ok(bytes) => Some(bytes),
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        Err(error) => return Err(invalid(error.to_string())),
    };
    if let Some(bytes) = bytes {
        let value: serde_yaml_ng::Value =
            serde_yaml_ng::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
        if !value.is_mapping() && !value.is_null() {
            return Err(invalid(
                "protection config root must be a mapping".to_owned(),
            ));
        }
        if let Some(raw) = value
            .get("protection")
            .and_then(serde_yaml_ng::Value::as_mapping)
            .and_then(|block| {
                block.get(serde_yaml_ng::Value::String(
                    "protected_branches".to_owned(),
                ))
            })
            .filter(|raw| !raw.is_null())
        {
            let list = raw.as_sequence().ok_or_else(|| {
                invalid("protection.protected_branches must be a list".to_owned())
            })?;
            // An explicit list, including [], REPLACES the defaults.
            names.clear();
            for entry in list {
                names.push(match entry {
                    serde_yaml_ng::Value::String(name) => name.clone(),
                    serde_yaml_ng::Value::Number(number) => number.to_string(),
                    serde_yaml_ng::Value::Bool(true) => "True".to_owned(),
                    serde_yaml_ng::Value::Bool(false) => "False".to_owned(),
                    serde_yaml_ng::Value::Null => "None".to_owned(),
                    _ => {
                        return Err(invalid(
                            "protected branch entries must be scalar names".to_owned(),
                        ));
                    }
                });
            }
        }
    }
    sk_git::ProtectedBranches::parse(names).map_err(|error| {
        CompatError::domain(format!(
            "could not validate protected branch names: {error}"
        ))
    })
}

fn mission_topology(mission: &Mission) -> Option<MissionTopology> {
    mission.topology()
}

fn coordination_branch(mission: &Mission) -> Option<String> {
    mission
        .meta_field("coordination_branch")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn map_workspace_error(error: sk_workspace::WorkspaceError) -> CompatError {
    match &error {
        sk_workspace::WorkspaceError::OwnedRefused { code, message } => CompatError::coded(
            if *code == PROTECTED_BRANCH_REFUSED {
                PROTECTED_BRANCH_REFUSED
            } else {
                "OWNED_REFUSED"
            },
            message.clone(),
        ),
        sk_workspace::WorkspaceError::RetentionRefused { reason, .. } => {
            CompatError::coded("RETENTION_REFUSED", reason.clone())
        }
        sk_workspace::WorkspaceError::LaneNotFound { wp } => CompatError::coded(
            "LANE_NOT_FOUND",
            format!("work package {wp} is not assigned to any execution lane"),
        ),
        _ => CompatError::domain(error.to_string()),
    }
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(path_string)
        .unwrap_or_else(|_| path_string(path))
}

/// Allocate or reuse the lane workspace for `wp_id`.
pub fn implement(
    root: &Path,
    handle: Option<&str>,
    wp_id: &str,
    base: Option<&str>,
    json_output: bool,
) -> Result<Value, CompatError> {
    let handle = handle.map(str::trim).filter(|value| !value.is_empty());
    let Some(handle) = handle else {
        return Err(CompatError::usage("--mission <slug> is required"));
    };
    let mission = super::support::resolve_mission(root, Some(handle))?;
    let wp: sk_kernel::WpId = wp_id
        .parse()
        .map_err(|error| CompatError::domain(format!("invalid work package id: {error}")))?;

    let store = LanesStore::new(mission.spec_dir());
    let manifest = store.read().map_err(map_workspace_error)?.ok_or_else(|| {
        CompatError::coded(
            "MISSING_LANES",
            format!(
                "lanes.json not found for {}; run the tasks and finalize-tasks steps first",
                mission.slug().as_str()
            ),
        )
    })?;

    let lane = manifest.lane_for_wp(&wp).ok_or_else(|| {
        CompatError::coded(
            "LANE_NOT_FOUND",
            format!(
                "work package {wp} is not assigned to any execution lane in {}",
                store.path().display()
            ),
        )
    })?;

    let mission_type = mission.mission_type().unwrap_or("software-dev").to_owned();
    let identity = |payload: &mut Map<String, Value>| {
        payload.insert("mission_slug".into(), json!(mission.slug().as_str()));
        payload.insert(
            "mission_number".into(),
            mission
                .mission_number()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        payload.insert("mission_type".into(), json!(mission_type));
        payload.insert("wp_id".into(), json!(wp_id));
    };

    if lane.is_planning() {
        let mut payload = Map::new();
        payload.insert("workspace".into(), json!(""));
        payload.insert("workspace_path".into(), json!(""));
        payload.insert("branch".into(), Value::Null);
        identity(&mut payload);
        payload.insert("lane_id".into(), json!(lane.lane_id));
        payload.insert("execution_mode".into(), json!("direct_repo"));
        payload.insert("status".into(), json!("created"));
        payload.insert("lane_test_env".into(), json!({}));
        let value = Value::Object(payload);
        if json_output {
            emit_json(&value)
                .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
        } else {
            println!("✓ Repository-root workspace ready: {}", root.display());
        }
        return Ok(value);
    }

    let repository = sk_git::GitRepository::open(root).map_err(|error| {
        CompatError::domain(format!(
            "could not open a Git work tree at {}: {error}",
            root.display()
        ))
    })?;
    let protected = protected_branches(root)?;
    let policy = MutationPolicy::new(protected.clone());
    if let Some(topology) = mission_topology(&mission) {
        let coordination = coordination_branch(&mission);
        match policy.evaluate_destination(
            &manifest.mission_branch,
            topology,
            coordination.as_deref(),
        ) {
            PolicyVerdict::Allowed { .. } => {}
            PolicyVerdict::Refused {
                error_code,
                message,
                next_step,
                ..
            } => {
                return Err(CompatError::coded(
                    if error_code == PROTECTED_BRANCH_REFUSED {
                        PROTECTED_BRANCH_REFUSED
                    } else {
                        "DESTINATION_REFUSED"
                    },
                    message,
                )
                .with("next_step", json!(next_step)));
            }
        }
    }

    let allocator =
        sk_workspace::LaneAllocator::new(&repository, protected, mission.slug().as_str())
            .map_err(map_workspace_error)?;
    let allocation = allocator
        .allocate(
            &manifest,
            mission_topology(&mission),
            coordination_branch(&mission).as_deref(),
            &wp,
            base,
        )
        .map_err(map_workspace_error)?;

    let mut payload = Map::new();
    payload.insert(
        "workspace".into(),
        json!(relative_to(root, &allocation.path)),
    );
    payload.insert(
        "workspace_path".into(),
        json!(relative_to(root, &allocation.path)),
    );
    payload.insert("branch".into(), json!(allocation.branch.as_str()));
    identity(&mut payload);
    payload.insert("lane_id".into(), json!(lane.lane_id));
    payload.insert("execution_mode".into(), json!("worktree"));
    payload.insert(
        "status".into(),
        json!(if allocation.route == AllocationRoute::Reuse {
            "reused"
        } else {
            "created"
        }),
    );
    payload.insert("allocation_route".into(), json!(allocation.route.as_str()));
    payload.insert("base_ref".into(), json!(allocation.decision.parent_ref));
    payload.insert("lane_test_env".into(), json!({}));
    let value = Value::Object(payload);
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "✓ Lane {} workspace ready: {}",
            lane.lane_id,
            allocation.path.display()
        );
        println!("  branch: {}", allocation.branch.as_str());
    }
    Ok(value)
}

/// Native extension: recover interrupted lane allocations.
pub fn recover(root: &Path, handle: Option<&str>, json_output: bool) -> Result<Value, CompatError> {
    let handle = handle.map(str::trim).filter(|value| !value.is_empty());
    let Some(handle) = handle else {
        return Err(CompatError::usage("--mission <slug> is required"));
    };
    let mission = super::support::resolve_mission(root, Some(handle))?;
    let repository = sk_git::GitRepository::open(root)
        .map_err(|error| CompatError::domain(format!("could not open a Git work tree: {error}")))?;
    let allocator = sk_workspace::LaneAllocator::new(
        &repository,
        protected_branches(root)?,
        mission.slug().as_str(),
    )
    .map_err(map_workspace_error)?;
    let report = allocator
        .recover_interrupted()
        .map_err(map_workspace_error)?;
    let value = json!({
        "repairs": report.repairs.iter().map(|repair| match repair {
            sk_workspace::RecoveryRepair::Completed { lane_id, path } => json!({"action": "completed", "lane_id": lane_id, "path": path_string(path)}),
            sk_workspace::RecoveryRepair::Reattached { lane_id, path } => json!({"action": "reattached", "lane_id": lane_id, "path": path_string(path)}),
            sk_workspace::RecoveryRepair::Cleared { lane_id, path } => json!({"action": "cleared", "lane_id": lane_id, "path": path_string(path)}),
        }).collect::<Vec<_>>(),
        "conflicts": report.conflicts.iter().map(|conflict| json!({
            "lane_id": conflict.record.lane_id,
            "path": path_string(&conflict.record.path),
            "detail": conflict.detail,
        })).collect::<Vec<_>>(),
    });
    if json_output {
        emit_json(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!(
            "lane allocation recovery: {} repairs, {} conflicts",
            report.repairs.len(),
            report.conflicts.len()
        );
    }
    Ok(value)
}

/// Resolve the manifest path for diagnostics (native extension helper).
pub fn lanes_path(mission: &Mission) -> PathBuf {
    LanesStore::new(mission.spec_dir()).path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_protected_lists_replace_defaults_including_an_empty_list() {
        for (yaml, main, release) in [
            ("protection:\n  protected_branches: []\n", false, false),
            (
                "protection:\n  protected_branches: [release]\n",
                false,
                true,
            ),
            ("protection:\n  protected_branches: null\n", true, false),
        ] {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join(".kittify")).unwrap();
            std::fs::write(root.path().join(".kittify/config.yaml"), yaml).unwrap();
            let policy = protected_branches(root.path()).unwrap();
            assert_eq!(policy.contains(&"main".parse().unwrap()), main);
            assert_eq!(policy.contains(&"release".parse().unwrap()), release);
        }
    }

    #[test]
    fn unreadable_or_malformed_protection_config_is_never_silently_ignored() {
        for bytes in [
            b"[unclosed".as_slice(),
            b"\xff",
            b"protection:\n  protected_branches: release\n",
        ] {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join(".kittify")).unwrap();
            std::fs::write(root.path().join(".kittify/config.yaml"), bytes).unwrap();
            assert!(protected_branches(root.path()).is_err());
        }
    }
}
