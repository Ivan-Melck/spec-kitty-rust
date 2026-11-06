//! Canonical facts the `doctor` adapter supplies to the workspace diagnosis.
//!
//! Three things live here, all adapter-level because their owners are
//! elsewhere:
//!
//! * [`capture_environment`] reads `$HOME` / `$SPEC_KITTY_HOME` for the
//!   state-roots report. Both roots (`get_kittify_home()`, and the runtime root
//!   `get_runtime_root().base`) resolve `$SPEC_KITTY_HOME` verbatim and fall back
//!   to `~/.kittify` / `~/.spec-kitty` on POSIX.
//! * [`CharterMissionTypeHealth`] answers the mission-type audit's two questions
//!   from their canonical owners: the activated set comes from
//!   [`sk_doctrine::load_activation_ledger_inspection`] (the activation ledger,
//!   never a second config reader), and "resolves" is
//!   [`sk_mission::resolve_mission_definition`] over the layered roots (project →
//!   org pack → packaged built-in → activated sparse registration).
//! * [`DecisionSurfaces`] resolves `doctor decisions`' two read homes through the
//!   one placement authority: the Mission event log is `STATUS_STATE` (the
//!   coordination surface when the stored topology routes through it) and the
//!   ledger is the PRIMARY-partition `DECISION_LEDGER` home (#4966 AC-D2). It
//!   also carries the two-surface fork report
//!   ([`sk_workspace::decision_fork::detect_decision_forks`]) for the same
//!   Mission, so `DECISION_LOG_FORKED` / `DECISION_LEDGER_ONLY_ON_COORDINATION`
//!   are real facts rather than an unevaluated disclosure.
//!
//! None of these functions writes. A malformed config or ledger is surfaced as
//! a refusal, never as an empty activation set.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_charter::{Limits, PathExpansion, load_project_config};
use sk_mission::definition::{
    DefinitionProviders, PyYamlSafe, RegisteredMissionType, resolve_mission_definition,
};
use sk_workspace::artifact::MissionArtifactKind;
use sk_workspace::decision_fork::{DecisionsForkReport, detect_decision_forks};
use sk_workspace::doctor::{MissionTypeHealth, StateRootEnvironment};
use sk_workspace::surface::{TopologySurface, coord_read_surface};
use sk_workspace::{ProjectWorkspace, WorkspaceError, declared_read_surface};

use super::CompatError;

/// The user home directory (`$HOME`, else `%USERPROFILE%`).
fn home_directory() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            "could not determine the user home directory (HOME/USERPROFILE is unset)".to_owned()
        })
}

/// Capture the state-root environment from the process environment.
pub fn capture_environment() -> Result<StateRootEnvironment, String> {
    let home = home_directory()?;
    // `$SPEC_KITTY_HOME` wins verbatim for both roots; an empty value falls
    // through to the platform default, exactly like the reference's
    // `if env_home := os.environ.get(...)`.
    let spec_kitty_home = std::env::var_os("SPEC_KITTY_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    Ok(match spec_kitty_home {
        Some(base) => StateRootEnvironment {
            home,
            kittify_home: base.clone(),
            runtime_base: base,
        },
        None => StateRootEnvironment {
            home: home.clone(),
            kittify_home: home.join(".kittify"),
            runtime_base: home.join(".spec-kitty"),
        },
    })
}

/// The layered mission-type facts for one project.
pub struct CharterMissionTypeHealth {
    project_root: PathBuf,
    activated: BTreeSet<String>,
    providers: DoctorDefinitionProviders,
}

impl CharterMissionTypeHealth {
    /// Resolve the activation ledger and the layered definition roots.
    ///
    /// Source's doctor `ledger_findings` reads with `report_stray=False` and
    /// carries stray legacy keys as findings, so this diagnostic read must not
    /// print the reader's warnings.
    pub fn resolve(root: &Path) -> Result<Self, CompatError> {
        let ledger = sk_doctrine::load_activation_ledger_inspection(root).map_err(|error| {
            CompatError::coded(
                "ACTIVATION_STORE_ERROR",
                format!("could not read the activation ledger: {error}"),
            )
        })?;
        let activated = ledger.mission_types();

        let expansion = PathExpansion::from_env();
        let config = load_project_config(root, &Limits::default()).map_err(|error| {
            CompatError::coded(
                "PROJECT_CONFIG_INVALID",
                format!("could not read .kittify/config.yaml: {error}"),
            )
        })?;
        let mut org_roots = Vec::new();
        for pack in &config.org_packs {
            if let Ok(path) = pack.effective_root(root, &expansion)
                && path.is_dir()
            {
                org_roots.push(path);
            }
        }

        let packaged = packaged_missions_root(root);
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value).join(".kittify"));

        Ok(Self {
            project_root: root.to_path_buf(),
            activated,
            providers: DoctorDefinitionProviders {
                org_roots,
                packaged,
                home,
            },
        })
    }
}

impl MissionTypeHealth for CharterMissionTypeHealth {
    fn activated(&self) -> &BTreeSet<String> {
        &self.activated
    }

    fn resolves(&self, mission_type: &str) -> bool {
        resolve_mission_definition(
            mission_type,
            &self.project_root,
            &self.providers,
            &PyYamlSafe,
        )
        .is_ok()
    }
}

/// The layered definition roots this adapter can see.
struct DoctorDefinitionProviders {
    org_roots: Vec<PathBuf>,
    packaged: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl DoctorDefinitionProviders {
    /// Every `mission_types/<id>.yaml` registration root, in precedence order.
    fn registration_roots(&self, project_root: &Path) -> Vec<PathBuf> {
        let mut roots = vec![project_root.join(".kittify/missions/mission_types")];
        roots.extend(
            self.org_roots
                .iter()
                .map(|root| root.join("missions/mission_types")),
        );
        if let Some(packaged) = &self.packaged {
            roots.push(packaged.join("mission_types"));
        }
        roots
    }
}

impl DefinitionProviders for DoctorDefinitionProviders {
    fn org_pack_roots(&self, _project_root: &Path) -> Vec<PathBuf> {
        self.org_roots.clone()
    }

    fn registered_mission_type(
        &self,
        project_root: &Path,
        name: &str,
    ) -> Option<RegisteredMissionType> {
        // Presence of the registration is the fact Source's roster contributes;
        // the neutral identity is built by the resolver from the same name.
        let exists = self
            .registration_roots(project_root)
            .iter()
            .any(|root| root.join(format!("{name}.yaml")).is_file());
        exists.then(|| RegisteredMissionType {
            id: sk_kernel::PythonString::from_scalars(name),
            display_name: sk_kernel::PythonString::from_scalars(name),
            action_sequence: None,
            path_conventions: None,
        })
    }

    fn packaged_missions_root(&self, _project_root: &Path) -> Option<PathBuf> {
        self.packaged.clone()
    }

    fn kittify_home(&self, _project_root: &Path) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// The two read homes `doctor decisions` needs for one Mission, plus its
/// two-surface fork report.
pub struct DecisionSurfaces {
    /// The Mission directory whose `status.events.jsonl` the diagnosis reads.
    pub events_dir: PathBuf,
    /// The PRIMARY-partition Mission directory holding `decisions/index.json`.
    pub ledger_dir: PathBuf,
    /// The resolved Mission slug (the directory name, for the report).
    pub mission_slug: String,
    /// The PRIMARY-vs-COORDINATION stream/ledger diagnosis.
    pub fork_report: DecisionsForkReport,
}

impl DecisionSurfaces {
    /// Resolve both surfaces through the canonical placement authority.
    ///
    /// * The event log is `STATUS_STATE`; on a coordination-routing topology it
    ///   lives on the coordination surface, so the registered-worktree probe
    ///   decides. Source's read-only doctor degrades an unmaterialized/deleted
    ///   coordination surface to a deliberately nonexistent placeholder, so the
    ///   event log reads as empty instead of crashing; the same degrade is kept
    ///   here and the placeholder is never created.
    /// * The ledger is `DECISION_LEDGER`, a PRIMARY-partition kind (#4966
    ///   AC-D2), so its declared home is the primary Mission directory.
    ///
    /// A Mission whose stored topology cannot be resolved, and a mission that
    /// does not exist, are refusals — never an empty-but-clean report.
    pub fn resolve(root: &Path, handle: &str) -> Result<Self, CompatError> {
        let workspace = ProjectWorkspace::open(root).map_err(|error| {
            CompatError::coded(
                "PROJECT_ROOT_NOT_FOUND",
                format!(
                    "could not open the project repository at {}: {error}",
                    root.display()
                ),
            )
        })?;
        let mission = workspace.mission(handle).map_err(selection_error)?;
        let mission_slug = mission.slug().to_owned();
        let primary_dir = mission.mission_dir().to_path_buf();

        let probe = mission.probe_coordination().map_err(|error| {
            CompatError::coded(
                "COORDINATION_PROBE_FAILED",
                format!("could not probe the coordination surface: {error}"),
            )
        })?;
        let coord_surface = coord_read_surface(
            probe.state,
            &mission_slug,
            probe.branch.as_deref(),
            &primary_dir,
            probe.mission_dir.as_deref(),
        );
        let events_dir = match coord_surface {
            Ok(surface) if surface.surface_kind == TopologySurface::Coord => surface.path,
            Ok(_) => primary_dir.clone(),
            // Source `_resolve_events_dir_for_doctor`: nothing is live there yet,
            // so the log degrades to empty rather than failing the diagnosis.
            Err(WorkspaceError::CoordinationWorktreeUnmaterialized { .. })
            | Err(WorkspaceError::CoordinationBranchDeleted { .. }) => root.join(format!(
                ".spec-kitty-doctor-unmaterialized-coord~{mission_slug}"
            )),
            Err(error) => {
                return Err(CompatError::coded(
                    "COORDINATION_PROBE_FAILED",
                    format!("could not resolve the Mission event-log surface: {error}"),
                ));
            }
        };

        let ledger_partition =
            declared_read_surface(MissionArtifactKind::DecisionLedger, mission.topology());
        if ledger_partition != TopologySurface::Primary {
            // A PRIMARY-partition kind can never declare another surface;
            // refusing is honest where silently picking one would not be.
            return Err(CompatError::coded(
                "DECISION_LEDGER_PARTITION_INVALID",
                format!(
                    "the decision ledger declared the {} surface for {mission_slug}",
                    ledger_partition.as_str()
                ),
            ));
        }
        let ledger_dir = primary_dir;

        let fork_report = detect_decision_forks(&mission).map_err(|error| {
            CompatError::coded(
                "DECISION_FORK_UNREADABLE",
                format!("could not diagnose the decision streams for {mission_slug}: {error}"),
            )
        })?;

        Ok(Self {
            events_dir,
            ledger_dir,
            mission_slug,
            fork_report,
        })
    }
}

/// Map a Mission-selection failure onto the adapter's domain refusal.
///
/// A stored topology the placement resolver cannot recognise is reported as
/// its own condition: `MISSION_NOT_FOUND` would send the operator looking for a
/// Mission that exists.
fn selection_error(error: WorkspaceError) -> CompatError {
    match error {
        WorkspaceError::TopologyUnresolved { mission, stored } => CompatError::coded(
            "MISSION_TOPOLOGY_UNRESOLVED",
            format!(
                "mission {mission} has no recognised stored topology ({stored:?}); refusing to guess"
            ),
        ),
        other => CompatError::coded("MISSION_NOT_FOUND", format!("Mission not found: {other}")),
    }
}

/// The packaged missions tree this installation carries, if any.
///
/// A consumer install ships `packs/built-in/missions`; a source checkout keeps
/// the legacy `src/specify_cli/missions` tree. Both are read-only lookups, and
/// an absent tree is `None` rather than an invented path.
fn packaged_missions_root(root: &Path) -> Option<PathBuf> {
    let pack = root.join("packs/built-in/missions");
    if pack.is_dir() {
        return Some(pack);
    }
    let legacy = root.join("src/specify_cli/missions");
    legacy.is_dir().then_some(legacy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_root_prefers_the_pack_tree_and_never_invents_one() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(packaged_missions_root(root.path()), None);
        std::fs::create_dir_all(root.path().join("src/specify_cli/missions")).unwrap();
        assert_eq!(
            packaged_missions_root(root.path()),
            Some(root.path().join("src/specify_cli/missions"))
        );
        std::fs::create_dir_all(root.path().join("packs/built-in/missions")).unwrap();
        assert_eq!(
            packaged_missions_root(root.path()),
            Some(root.path().join("packs/built-in/missions"))
        );
    }

    #[test]
    fn a_registration_anywhere_in_the_roots_makes_a_type_resolvable() {
        let root = tempfile::tempdir().unwrap();
        let registrations = root.path().join(".kittify/missions/mission_types");
        std::fs::create_dir_all(&registrations).unwrap();
        std::fs::write(registrations.join("custom.yaml"), b"id: custom\n").unwrap();
        let providers = DoctorDefinitionProviders {
            org_roots: Vec::new(),
            packaged: None,
            home: None,
        };
        assert!(
            providers
                .registered_mission_type(root.path(), "custom")
                .is_some()
        );
        assert!(
            providers
                .registered_mission_type(root.path(), "absent")
                .is_none()
        );
    }
}
