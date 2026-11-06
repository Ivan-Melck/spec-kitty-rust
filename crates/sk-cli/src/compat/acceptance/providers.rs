//! Cross-surface providers the acceptance composition needs but does not own.
//!
//! Two facts acceptance must read live outside `sk-workspace`:
//!
//! 1. **The durable Mission runtime.** Source's
//!    `runtime.next.runtime_bridge_io.read_runtime_acceptance_facts` reads the
//!    indexed run, verifies the frozen template against the cursor's hash and
//!    returns the frozen step list. The run store and template authorities live in
//!    `sk-run-store` / `sk-runtime`, so the CLI — the composition root — implements
//!    [`NonWpRuntimeFactsProvider`] over them instead of letting the collector
//!    re-read `state.json` itself.
//! 2. **The activated sparse mission-type registration.** Source's
//!    `_build_neutral_org_mission` resolves a type that has a bare
//!    `mission_types/<id>.yaml` registration but no `mission.yaml` at any resolver
//!    tier. The activation roster is a Doctrine read, so this module resolves it
//!    through the canonical Doctrine owners (`load_activation_ledger` for the
//!    activation gate, `scan_mission_types_dir` for the layered, schema-admitting
//!    roster scan) and hands the resulting [`RegisteredMissionType`] to the
//!    Mission-definition resolver as a plain value.
//!
//! Nothing here invents a run, a step, a roster entry or a registration field: a
//! missing runtime is `None` (the work-package path), an unactivated type is
//! `None` (the type stays unresolved, exactly as Source's raised
//! `MissionNotFoundError`), and every declared field is projected from the
//! admission-validated document.
//!
//! # Known gaps (recorded in the task report)
//!
//! * `MissionType.action_sequence` is projected from the document's DECLARED
//!   value; Source's `_inject_projected_fields` additionally derives it from the
//!   type's builtin step contracts when those exist. That value only feeds the
//!   neutral identity's descriptive workflow-phase text, not any acceptance gate.
//! * `MissionType.extends` inheritance is not composed.
//! * Source memoizes the layered roster for the process; this reads it fresh per
//!   acceptance run (a fresh CLI process has an empty cache anyway).

use sk_kernel::PythonString;
use sk_kernel::python_yaml::{Dialect, GraphDocument, NodeRef};
use sk_mission::{
    DefinitionProviders, OrderedMap, PyYamlSafe, RegisteredMissionType, SafeDocumentDecoder,
};
use sk_run_store::RunStore;
use sk_workspace::acceptance_source::InstalledMissionProviders;
use sk_workspace::acceptance_source::NonWpRuntimeFacts;
use sk_workspace::acceptance_source::non_wp_runtime::NonWpRuntimeFactsProvider;
use sk_workspace::error::WorkspaceError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::super::support::CompatError;

/// The runtime-facts port over the canonical run store.
pub(crate) struct RunStoreRuntimeFacts<'a> {
    /// The root the acceptance run was invoked from.
    pub(crate) root: &'a Path,
    /// The Mission's canonical identity, when it declares one.
    pub(crate) mission_id: Option<&'a str>,
}

impl NonWpRuntimeFactsProvider for RunStoreRuntimeFacts<'_> {
    fn runtime_acceptance_facts(
        &self,
        mission_slug: &str,
    ) -> Result<Option<NonWpRuntimeFacts>, WorkspaceError> {
        let store = RunStore::open(self.root).map_err(runtime_error)?;
        let Some(run) = store
            .run_for_mission(mission_slug, self.mission_id)
            .map_err(runtime_error)?
        else {
            // No run has been recorded for this Mission: Source's facts read
            // returns `None` here, which keeps the work-package path.
            return Ok(None);
        };
        let (state, template) = store.read_run_template(&run).map_err(runtime_error)?;
        let bound_slug = state
            .mission_slug
            .clone()
            .filter(|slug| !slug.is_empty())
            .or_else(|| {
                state
                    .inputs
                    .get("mission_slug")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            });
        let run_dir_name = run
            .run_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if bound_slug.as_deref() != Some(mission_slug) || state.run_id != run_dir_name {
            return Err(runtime_refusal(
                "Runtime snapshot does not belong to the requested mission/run",
            ));
        }
        if template.mission.key != state.mission_key {
            return Err(runtime_refusal(
                "Frozen runtime type does not match its recorded run",
            ));
        }
        Ok(Some(NonWpRuntimeFacts {
            mission_family: state.mission_key.clone(),
            step_ids: template.steps.iter().map(|step| step.id.clone()).collect(),
            completed_step_ids: state.completed_steps.clone(),
            issued_step_id: state.issued_step_id.clone(),
            blocked_reason: state.blocked_reason.clone(),
            pending_decision_ids: state.pending_decisions.keys().cloned().collect(),
        }))
    }
}

/// Map a run-store failure onto the acceptance collection failure phase.
fn runtime_error(error: sk_run_store::RunStoreError) -> WorkspaceError {
    WorkspaceError::InvalidArtifact {
        artifact: "mission runtime",
        detail: format!(
            "Cannot read canonical mission runtime for acceptance; no state was changed: {error}"
        ),
    }
}

/// Source's typed refusal for a runtime that does not belong to the Mission.
fn runtime_refusal(message: &str) -> WorkspaceError {
    WorkspaceError::InvalidArtifact {
        artifact: "mission runtime",
        detail: message.to_owned(),
    }
}

/// The Mission-definition providers, with the activated sparse registration
/// resolved up front (Source `_registered_org_mission_type`).
pub(crate) struct CompositionMissionProviders {
    installed: InstalledMissionProviders,
    registered: Option<(String, RegisteredMissionType)>,
}

impl CompositionMissionProviders {
    /// Compose the installed providers with this Mission's registration, if any.
    ///
    /// `mission_type` is the Mission's recorded `mission_type`; a typeless
    /// Mission coalesces to `software-dev` exactly as Source's template-selection
    /// path does, and `software-dev` is a packaged mission (never a sparse
    /// registration), so the coalesced name resolves through the normal tiers.
    pub(crate) fn resolve(
        project_root: Option<&Path>,
        mission_type: Option<&str>,
    ) -> Result<Self, CompatError> {
        let name = mission_type.unwrap_or("software-dev");
        let registered = match project_root {
            Some(root) => resolve_registered_mission_type(root, name)?
                .map(|registration| (name.to_owned(), registration)),
            // No `.kittify` project root was found: there is no activation
            // ledger or registration layer to read, so no type is registered.
            None => None,
        };
        Ok(Self {
            installed: InstalledMissionProviders,
            registered,
        })
    }
}

impl DefinitionProviders for CompositionMissionProviders {
    fn org_pack_roots(&self, project_root: &Path) -> Vec<PathBuf> {
        self.installed.org_pack_roots(project_root)
    }

    fn registered_mission_type(
        &self,
        _project_root: &Path,
        name: &str,
    ) -> Option<RegisteredMissionType> {
        self.registered
            .as_ref()
            .filter(|(registered_name, _)| registered_name == name)
            .map(|(_, registration)| registration.clone())
    }

    fn packaged_missions_root(&self, project_root: &Path) -> Option<PathBuf> {
        self.installed.packaged_missions_root(project_root)
    }

    fn kittify_home(&self, project_root: &Path) -> Option<PathBuf> {
        self.installed.kittify_home(project_root)
    }
}

/// Source `_registered_org_mission_type`: the activated sparse registration for
/// `name`, or `None` when the type is not activated / no layer admits a
/// registration for it.
fn resolve_registered_mission_type(
    project_root: &Path,
    name: &str,
) -> Result<Option<RegisteredMissionType>, CompatError> {
    // FR-006 activation gate (Source `existing_mission_types`): the ledger's
    // `mission_type_activations` set is the sole activation authority — an
    // unactivated id is unregistered regardless of what any layer ships.
    let ledger = sk_doctrine::load_activation_ledger(project_root).map_err(|error| {
        CompatError::coded(
            "ACTIVATION_STORE_ERROR",
            format!("could not read the activation ledger: {error}"),
        )
    })?;
    if !ledger.mission_types().contains(name) {
        return Ok(None);
    }

    // Source's layered roster: built-in first, then each org pack (the earliest
    // root wins), then the project layer (which always wins). `scan_mission_types_dir`
    // is the canonical admission authority — it enforces the `mission_types/<id>.yaml`
    // document schema and `id == filename stem`, and loud-fails on a malformed
    // layer instead of shrinking the roster.
    let installed = InstalledMissionProviders;
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(packaged) = installed.packaged_missions_root(project_root) {
        roots.push(packaged.join("mission_types"));
    }
    for org_root in installed.org_pack_roots(project_root) {
        roots.push(org_root.join(sk_doctrine::ORG_MISSION_TYPES_SUBDIR));
    }
    roots.push(
        project_root
            .join(".kittify/missions/mission_types")
            .to_path_buf(),
    );

    let mut roster: BTreeMap<String, PathBuf> = BTreeMap::new();
    for root in roots {
        let ids = sk_doctrine::scan_mission_types_dir(&root).map_err(|error| {
            CompatError::coded(
                "MISSION_TYPE_ROSTER_INVALID",
                format!(
                    "the mission-type registration roster at {} is invalid: {error}",
                    root.display()
                ),
            )
        })?;
        for id in ids {
            let Ok(text) = id.to_scalar_string() else {
                continue;
            };
            roster.insert(text.clone(), root.join(format!("{text}.yaml")));
        }
    }
    let Some(path) = roster.get(name).cloned() else {
        return Ok(None);
    };
    project_registration(name, &path)
}

/// Project an admission-validated `mission_types/<id>.yaml` document onto the
/// neutral identity's declared fields.
///
/// The scan above already proved the document's schema and `id == stem`; this
/// only reads the declarations the neutral identity needs, through the shared
/// Kernel graph and the one PyYAML-safe decoder, so a value that is not the
/// declared type leaves the registration unloadable (`None`) instead of being
/// invented.
fn project_registration(
    name: &str,
    path: &Path,
) -> Result<Option<RegisteredMissionType>, CompatError> {
    let bytes = sk_kernel::read_bounded(
        path,
        sk_mission::expected_artifacts::DEFAULT_EXPECTED_ARTIFACTS_BYTE_LIMIT,
    )
    .map_err(|error| {
        CompatError::coded(
            "MISSION_TYPE_ROSTER_INVALID",
            format!(
                "the mission-type registration {} could not be read: {error}",
                path.display()
            ),
        )
    })?;
    let text = sk_kernel::python_yaml::read_document_text(&bytes, Dialect::PyYamlSafe11).map_err(
        |error| {
            CompatError::coded(
                "MISSION_TYPE_ROSTER_INVALID",
                format!(
                    "the mission-type registration {} is not decodable YAML: {error}",
                    path.display()
                ),
            )
        },
    )?;
    let decoder = PyYamlSafe;
    let document: GraphDocument = decoder.decode(path, &text).map_err(|detail| {
        CompatError::coded(
            "MISSION_TYPE_ROSTER_INVALID",
            format!(
                "the mission-type registration {} is not a decodable document: {detail}",
                path.display()
            ),
        )
    })?;
    let root = document.root();
    let Some(display_name) = root.lookup("display_name").and_then(NodeRef::as_str) else {
        return Ok(None);
    };
    let action_sequence = match root.lookup("action_sequence") {
        None => None,
        Some(value) if value.is_null() => None,
        Some(value) => match value.as_sequence() {
            Some(items) => {
                let mut steps = Vec::with_capacity(items.len());
                for item in items {
                    match value.view(*item).as_str() {
                        Some(step) => steps.push(step.clone()),
                        None => return Ok(None),
                    }
                }
                Some(steps)
            }
            None => return Ok(None),
        },
    };
    let path_conventions = match root.lookup("path_conventions") {
        None => None,
        Some(value) if value.is_null() => None,
        Some(value) => match value.as_mapping() {
            Some(entries) => {
                let mut conventions: OrderedMap<PythonString> = OrderedMap::new();
                for (key, item) in entries {
                    let key = value.view(*key);
                    let item = value.view(*item);
                    let (Some(key), Some(item)) = (key.as_str(), item.as_str()) else {
                        return Ok(None);
                    };
                    conventions.insert(key.clone(), item.clone());
                }
                Some(conventions)
            }
            None => return Ok(None),
        },
    };
    Ok(Some(RegisteredMissionType {
        id: PythonString::from_scalars(name),
        display_name: display_name.clone(),
        action_sequence,
        path_conventions,
    }))
}
