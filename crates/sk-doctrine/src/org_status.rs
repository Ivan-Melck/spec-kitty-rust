//! Organisation-layer status composition (`_collect_org_layer_status`).
//!
//! The read-only status view of the org tier, composed from the existing
//! doctrine owners — never a second loader, merge or validator:
//!
//! * org-pack declaration order, names and effective roots come from the
//!   canonical `.kittify/config.yaml` registry reader
//!   ([`sk_charter::load_org_pack_registry`] + [`sk_charter::OrgPackConfig`]),
//!   which is what the reference's `load_org_drg` walks;
//! * each pack's fragment comes from [`crate::org::load_org_pack`];
//! * the overrides / conflicts come from the one three-layer merge,
//!   [`crate::merge::merge_three_layers`];
//! * the completeness check comes from the one dangling-endpoint authority in
//!   [`crate::validate`].
//!
//! Ordering matches the reference exactly: packs are loaded first and a load
//! failure returns before any merge; the built-in graph is only read when at
//! least one org pack is configured (`if not fragments: return result`), so a
//! project with no org packs never touches the built-in tier and reports a
//! genuinely blank layer rather than a consulted-clean claim.
//!
//! Known gap, disclosed rather than papered over: the merge's refusal path
//! carries only the [`crate::error::DoctrineError`] text here, because
//! `merge_three_layers` discards the accumulated conflict records when it
//! refuses. The reference's `OrgDRGConflictError` keeps them, so its status
//! payload lists the *advisory* conflicts that preceded a fatal one beside the
//! fatal lines; this composer reports the fatal line only. Closing that needs a
//! partition on the merge owner (`merge_three_layers_partitioned` returning the
//! conflict list on the error path), which is outside this module.

use std::path::{Path, PathBuf};

use sk_charter::{
    Limits, OrgPackConfig, PackRoots, PathExpansion, load_org_pack_registry, load_project_config,
};

use crate::error::DoctrineError;
use crate::loader::load_graph_or_dir;
use crate::merge::merge_three_layers;
use crate::org::{OrgFragment, load_org_pack};
use crate::org_charter::{built_in_search, normalize_packs_root};
use crate::validate::validate_dangling_references;
use sk_kernel::python_repr;

/// The `source_kind` a locally configured pack reports (`load_org_pack` sets
/// `"local_path"` for every pack it loads — no other kind is reachable by a
/// project-configured pack).
pub const ORG_SOURCE_KIND_LOCAL_PATH: &str = "local_path";

/// The reference's org-pack-missing remedy (`OrgPackMissingError.REMEDIATION`).
pub const ORG_PACK_MISSING_REMEDIATION: &str = "Either fetch the pack (`spec-kitty charter fetch --pack <name>`) or remove the entry from \
     `.kittify/config.yaml`.";

/// One configured org pack's status row (`_collect_org_layer_status` packs[]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgPackStatus {
    /// The pack's configured name.
    pub name: String,
    /// The pack's declaration kind (`local_path`).
    pub source_kind: String,
    /// The resolved pack root (the reference reports `str(pack_root)`).
    pub source_ref: String,
    /// Declaration-order layer index (1..N).
    pub layer_index: usize,
    /// Whether the pack's fragment loaded (a failed load is an error row only).
    pub fetched: bool,
    /// Nodes the fragment contributes.
    pub node_count: usize,
    /// Edges the fragment contributes.
    pub edge_count: usize,
}

/// One resolved-by-precedence merge conflict (`collision_warnings[]`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrgCollisionWarning {
    /// The conflict kind (`node_override`, `edge_override`, …).
    pub kind: String,
    /// The URN or token the conflict is about.
    pub target_id: String,
    /// The layers that disagree, first the overridden layer.
    pub conflicting_layers: Vec<String>,
    /// What the merge did (`built_in_wins`, `project_wins`, `org_override`).
    pub resolution: String,
}

/// The org-layer status block (`org_layer` in `charter status --json`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OrgLayerStatus {
    /// The built-in tier is part of every install; the reference reports it as
    /// a build fact, not as a consulted merge.
    pub has_built_in: bool,
    /// One row per configured pack that loaded.
    pub packs: Vec<OrgPackStatus>,
    /// Conflicts the merge resolved by precedence.
    pub collision_warnings: Vec<OrgCollisionWarning>,
    /// Load failures, merge refusals and dangling endpoints — the reference's
    /// single `errors` channel.
    pub errors: Vec<String>,
}

/// The org-layer status for a project root, resolving packs the documented way
/// (`SPEC_KITTY_PACKS_ROOT`, else the packaged search).
#[must_use]
pub fn org_layer_status(repo_root: &Path) -> OrgLayerStatus {
    org_layer_status_with_packs_root(repo_root, None)
}

/// [`org_layer_status`] with an explicit packs-root override.
///
/// `packs_root` accepts either the documented parent (`<...>/packs`) or the
/// built-in leaf, exactly like the other `*_with_packs_root` readers in this
/// crate.
#[must_use]
pub fn org_layer_status_with_packs_root(
    repo_root: &Path,
    packs_root: Option<&Path>,
) -> OrgLayerStatus {
    let mut status = OrgLayerStatus {
        has_built_in: true,
        ..OrgLayerStatus::default()
    };
    let limits = Limits::default();
    let config = match load_project_config(repo_root, &limits) {
        Ok(config) => config,
        Err(error) => {
            status.errors.push(load_error(&error));
            return status;
        }
    };
    let registry = match load_org_pack_registry(repo_root, &limits) {
        Ok(registry) => registry,
        Err(error) => {
            status.errors.push(load_error(&error));
            return status;
        }
    };
    let expansion = PathExpansion::from_env();
    let mut loaded: Vec<(OrgFragment, PathBuf)> = Vec::new();
    for (index, pack) in registry.iter().enumerate() {
        let layer_index = index + 1;
        let root = match resolve_pack_root(pack, repo_root, &expansion) {
            Ok(root) => root,
            Err(error) => {
                status.errors.push(load_error(&error));
                return status;
            }
        };
        match load_org_pack(&pack.name, &root, layer_index) {
            Ok(fragment) => loaded.push((fragment, root)),
            Err(error) => {
                status.errors.push(pack_failure(&pack.name, &error));
                return status;
            }
        }
    }
    // The reference returns here before it reads the built-in tier.
    if loaded.is_empty() {
        return status;
    }

    let search = match packs_root {
        Some(root) => built_in_search(repo_root).with_packs_root_env(normalize_packs_root(root)),
        None => built_in_search(repo_root),
    };
    let roots = match PackRoots::resolve(repo_root, &config, &expansion, Some(&search)) {
        Ok(roots) => roots,
        Err(error) => {
            status.errors.push(completeness_error(&error));
            return status;
        }
    };
    let Some(built_in_root) = roots.built_in else {
        status.errors.push(completeness_error(
            &"no built-in pack root could be resolved",
        ));
        return status;
    };
    let built_in = match load_graph_or_dir(&built_in_root) {
        Ok(graph) => graph,
        Err(error) => {
            status.errors.push(completeness_error(&error));
            return status;
        }
    };

    let fragments: Vec<OrgFragment> = loaded.iter().map(|(f, _)| f.clone()).collect();
    let merged = match merge_three_layers(&built_in, &fragments, None) {
        Ok(report) => {
            for conflict in &report.conflicts {
                status.collision_warnings.push(OrgCollisionWarning {
                    kind: conflict.kind.clone(),
                    target_id: conflict.target_id.clone(),
                    conflicting_layers: conflict.conflicting_layers.clone(),
                    resolution: conflict.resolution_applied.clone(),
                });
            }
            report.graph
        }
        Err(error) => {
            // A refused merge means there is no merged graph to inspect, and the
            // refusal is fatal — never an advisory warning.
            status.errors.push(format!("org-DRG hard failure: {error}"));
            return status;
        }
    };

    for (fragment, root) in &loaded {
        status.packs.push(OrgPackStatus {
            name: fragment.pack_name.clone(),
            source_kind: ORG_SOURCE_KIND_LOCAL_PATH.to_owned(),
            source_ref: match root.to_str() {
                Some(text) => text.to_owned(),
                None => {
                    status.errors.push(load_error(
                        &"org pack root path is not valid UTF-8, so its status row cannot name it",
                    ));
                    return status;
                }
            },
            layer_index: fragment.layer_index,
            fetched: true,
            node_count: fragment.nodes.len(),
            edge_count: fragment.edges.len(),
        });
    }

    status.errors.extend(validate_dangling_references(&merged));
    status
}

fn resolve_pack_root(
    pack: &OrgPackConfig,
    repo_root: &Path,
    expansion: &PathExpansion,
) -> Result<PathBuf, sk_charter::CharterError> {
    pack.effective_root(repo_root, expansion)
}

/// A load-time failure on the reference's `org-DRG load error` channel.
fn load_error(error: &dyn std::fmt::Display) -> String {
    format!("org-DRG load error: {error}")
}

/// A completeness/merge-input failure on the reference's
/// `org-layer merge check failed` channel.
fn completeness_error(error: &dyn std::fmt::Display) -> String {
    format!("org-layer merge check failed: {error}")
}

/// The reference's org-pack failure line.
///
/// A pack whose configured root is not a directory and a pack whose
/// `drg/fragment.yaml` is absent are the *same* reference exception
/// (`OrgPackMissingError`), whose message names the path the loader was given.
fn pack_failure(pack_name: &str, error: &DoctrineError) -> String {
    match error {
        DoctrineError::OrgPackMissing { path, .. }
        | DoctrineError::OrgFragmentMissing { path, .. } => format!(
            "Org pack {} configured at {} not found. {ORG_PACK_MISSING_REMEDIATION}",
            python_repr(pack_name),
            python_repr(&path.display().to_string()),
        ),
        other => load_error(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(org_pack: Option<&str>) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path();
        std::fs::create_dir_all(root.join(".kittify")).expect("kittify");
        let mut config = String::from("charter_packs:\n");
        match org_pack {
            Some(name) => {
                config.push_str(&format!(
                    "  org:\n    packs:\n    - name: {name}\n      local_path: org-packs/{name}\n"
                ));
                let pack_root = root.join("org-packs").join(name);
                std::fs::create_dir_all(pack_root.join("drg")).expect("pack root");
                std::fs::write(
                    pack_root.join("drg/fragment.yaml"),
                    "nodes:\n- id: org-directive\n  kind: directive\n  title: Org directive\n\
                     edges: []\n",
                )
                .expect("fragment");
            }
            None => config.push_str("  project:\n    enabled: true\n"),
        }
        std::fs::write(root.join(".kittify/config.yaml"), config).expect("config");
        temp
    }

    #[test]
    fn a_project_without_org_packs_reports_a_blank_layer_not_a_clean_bill() {
        let temp = fixture(None);
        let status = org_layer_status(temp.path());
        assert!(status.has_built_in);
        assert!(status.packs.is_empty());
        assert!(status.errors.is_empty(), "{:?}", status.errors);
    }

    #[test]
    fn a_missing_pack_uses_the_reference_message_and_no_pack_rows() {
        let temp = fixture(Some("missing-pack"));
        // Remove the fragment so the pack is configured but not fetched.
        std::fs::remove_dir_all(temp.path().join("org-packs/missing-pack")).expect("remove");
        let status = org_layer_status(temp.path());
        assert!(status.packs.is_empty());
        let error = status.errors.first().expect("error");
        assert!(
            error.starts_with("Org pack 'missing-pack' configured at '"),
            "{error}"
        );
        assert!(
            error.contains("not found. Either fetch the pack"),
            "{error}"
        );
    }

    #[test]
    fn python_repr_matches_the_reference_quoting() {
        assert_eq!(python_repr("pack"), "'pack'");
        assert_eq!(python_repr("a'b"), "\"a'b\"");
        assert_eq!(python_repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(python_repr("a\\b"), "'a\\\\b'");
    }
}
