//! `spec-kitty doctor` — project health diagnostics.
//!
//! This is the command adapter: it parses the upstream subcommand surface,
//! resolves the project root, calls the canonical Workspace/Install diagnostics
//! or Invocation stale-Op sweep, and renders JSON or a human-readable report.
//!
//! # What is implemented
//!
//! | subcommand | state |
//! |---|---|
//! | `identity` | implemented (read-only) |
//! | `topology` | implemented (read-only) |
//! | `mission-type` | implemented (read-only) |
//! | `state-roots` | implemented (read-only) |
//! | `workspaces` | implemented (read-only); `--fix` refused |
//! | `decisions` | implemented (read-only log/index reconciliation + two-surface fork report); `--repair` refused |
//! | `invocation-pairing` | implemented (read-only orphan report) |
//! | `ops` | implemented (read-only listing; `--close-stale` closes abandoned Ops through Invocation and attempts the real spine commit) |
//! | `command-files` | partial read-only command-file diagnostics |
//! | `skills`, `tool-surfaces` | partial read-only diagnostics; `--fix` refused; missing probes remain named |
//! | `shim-registry`, `contracts` | implemented read-only structural/classification diagnostics; advisory sweep and wire details remain open |
//! | `sparse-checkout`, `mission-state`, `charter-packs`, `coordination`, `cutover`, `review-cycle-reconcile` | refused with a named code |
//!
//! Every refusal names the missing native owner and lists the remaining
//! coverage; none of them exits 0, so no caller can read `doctor` as a passing
//! health check. Unsupported `--fix`/`--repair` modes remain explicit refusals;
//! `ops --close-stale` performs the actual executor-owned mutation.
//!
//! # Output contract
//!
//! `--json` prints one JSON document (the upstream shape) on stdout and nothing
//! else. The human output is informational and is not byte-pinned to the
//! reference's Rich rendering; it carries the same rows, findings and remedies.

use std::collections::BTreeSet;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde_json::json;
use sk_invocation::ops_sweep::non_record_op_files;
use sk_workspace::doctor::{self, IdentityState, TrackedPosture};

use super::support::{CompatError, emit_json_pretty, project_root};

/// Read-only coordination health; `--fix` and unowned checks are refused.
pub mod coordination_health;
#[path = "doctor/facts.rs"]
pub mod facts;
#[path = "doctor/ops_sweep.rs"]
pub mod ops_sweep;
/// Read-only shim-registry and contract-registry diagnostics.
pub mod registries;
#[path = "doctor/render.rs"]
mod render;
/// Read-only tool-surface diagnostics; unsupported repairs remain explicit.
pub mod tool_surfaces;

/// `spec-kitty doctor` argument surface.
#[derive(Debug, Parser)]
#[command(
    name = "doctor",
    about = "Project health diagnostics",
    long_about = None,
    disable_help_subcommand = true
)]
pub struct DoctorArgs {
    /// The diagnostic to run. A bare `doctor` runs nothing (exit 2).
    #[command(subcommand)]
    pub command: Option<DoctorCommand>,
}

/// Every upstream `doctor` subcommand, with its real options.
#[derive(Debug, Subcommand)]
pub enum DoctorCommand {
    /// Check all agent command files for correctness.
    CommandFiles {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Check command-skill manifest drift.
    Skills {
        /// Repair missing command-skill files.
        #[arg(long)]
        fix: bool,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Audit (and optionally repair) every configured tool surface.
    ToolSurfaces {
        /// Filter to surface kind(s), e.g. command-skill.
        #[arg(long, value_name = "KIND")]
        kind: Vec<String>,
        /// Filter to a single configured tool key.
        #[arg(long, value_name = "TOOL")]
        tool: Option<String>,
        /// Repair missing or stale surfaces.
        #[arg(long)]
        fix: bool,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Show state roots, surface classification, and safety warnings.
    StateRoots {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Report .worktrees/ husk directories (entries lacking a .git entry).
    Workspaces {
        /// Remove husks that are NOT registered in `git worktree list`.
        #[arg(long)]
        fix: bool,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Report mission-identity health across kitty-specs/.
    Identity {
        /// Emit structured JSON output (suitable for CI).
        #[arg(long)]
        json: bool,
        /// Scope report to a single mission slug.
        #[arg(long)]
        mission: Option<String>,
        /// Exit non-zero if any mission is in the given state(s).
        #[arg(long, value_name = "STATES")]
        fail_on: Option<String>,
    },
    /// Report each mission's STORED topology across kitty-specs/.
    Topology {
        /// Emit structured JSON output (suitable for CI).
        #[arg(long)]
        json: bool,
        /// Scope report to a single mission slug.
        #[arg(long)]
        mission: Option<String>,
    },
    /// Report mission-type resolution health across kitty-specs/.
    MissionType {
        /// Emit structured JSON output (suitable for CI).
        #[arg(long)]
        json: bool,
        /// Scope report to a single mission slug.
        #[arg(long)]
        mission: Option<String>,
        /// Exit non-zero if any mission is in the given state(s).
        #[arg(long, value_name = "STATES")]
        fail_on: Option<String>,
    },
    /// Detect and optionally remediate legacy sparse-checkout state.
    SparseCheckout {
        /// Apply remediation (disable sparse-checkout on primary + worktrees).
        #[arg(long)]
        fix: bool,
    },
    /// Check for overdue compatibility shims in the shim registry.
    ShimRegistry {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Validate the Contract Registry for well-formedness.
    Contracts {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// List orphan profile-invocation lifecycle records.
    InvocationPairing {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// List orphan Op records; --close-stale sweeps stale ones closed.
    Ops {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
        /// Close open Ops older than --threshold as abandoned.
        #[arg(long)]
        close_stale: bool,
        /// Staleness threshold in hours (default 24; 0 closes all).
        #[arg(long, value_name = "HOURS")]
        threshold: Option<f64>,
    },
    /// Audit, repair, or TeamSpace-validate mission-state shapes.
    MissionState {
        /// Run mission-state audit (required to proceed).
        #[arg(long)]
        audit: bool,
        /// Repair mission-state artifacts in place.
        #[arg(long)]
        fix: bool,
        /// Synthesize canonical TeamSpace envelopes from local state.
        #[arg(long)]
        teamspace_dry_run: bool,
        /// Emit JSON report to stdout.
        #[arg(long)]
        json: bool,
        /// Scope to a single mission handle.
        #[arg(long)]
        mission: Option<String>,
        /// Exit 1 if findings meet a gate.
        #[arg(long, value_name = "GATE")]
        fail_on: Option<String>,
        /// Override scan root (for testing).
        #[arg(long, value_name = "DIR")]
        fixture_dir: Option<PathBuf>,
        /// Audit the bundled mission-state survey fixtures.
        #[arg(long)]
        include_fixtures: bool,
        /// Path for --fix migration manifest.
        #[arg(long, value_name = "PATH")]
        manifest_path: Option<PathBuf>,
        /// Allow --fix when relevant git paths are already dirty.
        #[arg(long)]
        allow_dirty: bool,
    },
    /// Check org charter pack snapshot status.
    CharterPacks {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Run the coordination + sparse-checkout health checks.
    Coordination {
        /// Repair coordination topology.
        #[arg(long)]
        fix: bool,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
        /// Also report coord-branch-vs-target-branch staleness.
        #[arg(long)]
        check_staleness: bool,
        /// Scope the checks to a single mission handle.
        #[arg(long)]
        mission: Option<String>,
    },
    /// Audit every mission's cut-over status outside CI.
    Cutover {
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Find review-cycle records stranded under a retired resolver path.
    ReviewCycleReconcile {
        /// Scope to a single mission.
        #[arg(long)]
        mission: Option<String>,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Diagnose divergence between the decision index and its event log.
    Decisions {
        /// Mission handle (mission_id / mid8 / slug).
        #[arg(long)]
        mission: String,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
        /// Rebuild decisions/index.json from the event log.
        #[arg(long)]
        repair: bool,
    },
}

/// Run one `doctor` invocation, returning the process exit code.
pub fn run(args: &DoctorArgs) -> Result<u8, CompatError> {
    let Some(command) = &args.command else {
        // Never read as a passing check: a bare `doctor` runs nothing.
        let _ = <DoctorArgs as clap::CommandFactory>::command().print_help();
        eprintln!(
            "Error: no diagnostic ran. `spec-kitty doctor` needs a subcommand (for example `spec-kitty doctor skills`); see the list above or run `spec-kitty doctor --help`."
        );
        return Ok(2);
    };

    match command {
        DoctorCommand::Identity {
            json,
            mission,
            fail_on,
        } => identity(*json, mission.as_deref(), fail_on.as_deref()),
        DoctorCommand::Topology { json, mission } => topology(*json, mission.as_deref()),
        DoctorCommand::MissionType {
            json,
            mission,
            fail_on,
        } => mission_type(*json, mission.as_deref(), fail_on.as_deref()),
        DoctorCommand::StateRoots { json } => state_roots(*json),
        DoctorCommand::Workspaces { fix, json } => workspaces(*fix, *json),
        DoctorCommand::Decisions {
            mission,
            json,
            repair,
        } => decisions(*json, mission, *repair),
        DoctorCommand::InvocationPairing { json } => invocation_pairing(*json),
        DoctorCommand::Ops {
            json,
            close_stale,
            threshold,
        } => ops(*json, *close_stale, *threshold),
        DoctorCommand::CommandFiles { json } => tool_surfaces::run_command_files(*json),
        DoctorCommand::Skills { fix, json } => tool_surfaces::run_skills(*fix, *json),
        DoctorCommand::ToolSurfaces {
            kind,
            tool,
            fix,
            json,
        } => tool_surfaces::run_tool_surfaces(kind, tool.as_deref(), *fix, *json),
        DoctorCommand::ShimRegistry { json } => registries::run_shim_registry(*json),
        DoctorCommand::Contracts { json } => registries::run_contracts(*json),
        other => Err(refuse(other)),
    }
}

fn root() -> Result<PathBuf, CompatError> {
    project_root()
}

fn identity(
    json_output: bool,
    mission: Option<&str>,
    fail_on: Option<&str>,
) -> Result<u8, CompatError> {
    let root = root()?;
    let audit = doctor::audit_identity(&root, mission, TrackedPosture::Strict)
        .map_err(map_doctor_error)?
        .ok_or_else(|| mission_not_found(mission.unwrap_or_default()))?;

    // A `--fail-on` token is matched exactly (the upstream audit does not
    // validate the list; an unknown token simply never matches).
    let fail_on_states: BTreeSet<IdentityState> = fail_on
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .filter_map(IdentityState::parse)
                .collect()
        })
        .unwrap_or_default();
    let triggered = audit.any_in(&fail_on_states);

    if json_output {
        emit_json_pretty(&audit.to_json(triggered))
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render::identity(&audit, &fail_on_states, triggered, fail_on);
    }
    Ok(u8::from(triggered))
}

fn topology(json_output: bool, mission: Option<&str>) -> Result<u8, CompatError> {
    let root = root()?;
    let audit = doctor::audit_topology(&root, mission)
        .map_err(map_doctor_error)?
        .ok_or_else(|| mission_not_found(mission.unwrap_or_default()))?;
    if json_output {
        emit_json_pretty(&audit.to_json())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render::topology(&audit);
    }
    Ok(0)
}

fn mission_type(
    json_output: bool,
    mission: Option<&str>,
    fail_on: Option<&str>,
) -> Result<u8, CompatError> {
    let states = doctor::parse_fail_on(fail_on).map_err(|error| {
        // A misspelled state is a usage error (exit 2), never a vacuous green.
        CompatError::usage(error.message())
    })?;

    let root = root()?;
    let health = facts::CharterMissionTypeHealth::resolve(&root)?;
    let mut audit = doctor::audit_mission_types(&root, &health).map_err(map_doctor_error)?;

    if let Some(handle) = mission.filter(|value| !value.is_empty()) {
        let scoped: Vec<_> = audit
            .rows
            .iter()
            .filter(|row| row.slug == handle)
            .cloned()
            .collect();
        if scoped.is_empty() {
            eprintln!("Error: Mission not found: {handle:?}");
            return Ok(1);
        }
        audit.rows = scoped;
    }

    let triggered = audit.any_in(&states);
    if json_output {
        emit_json_pretty(&audit.to_json(triggered))
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render::mission_type(&audit, &states, triggered, fail_on);
    }
    Ok(u8::from(triggered))
}

fn state_roots(json_output: bool) -> Result<u8, CompatError> {
    let root = root()?;
    let environment = facts::capture_environment().map_err(CompatError::domain)?;
    let audit = doctor::audit_state_roots(&root, &environment).map_err(map_doctor_error)?;
    if json_output {
        emit_json_pretty(&audit.to_json())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render::state_roots(&audit, &environment);
    }
    Ok(u8::from(!audit.healthy()))
}

fn workspaces(fix: bool, json_output: bool) -> Result<u8, CompatError> {
    if fix {
        return Err(unsupported(
            "UNSUPPORTED_DOCTOR_WORKSPACES_FIX",
            "`doctor workspaces --fix` deletes unregistered `.worktrees/` husks behind the reference's destructive guard; this slice implements the read-only report only",
            &[
                "husk detection with the registered-worktree annotation (implemented)",
                "guarded tree deletion + stale coordination-worktree refresh (missing: destructive-guard owner, `sk-git` worktree removal)",
            ],
        ));
    }
    let root = root()?;
    let report = doctor::scan_workspace_husks(&root).map_err(map_doctor_error)?;
    if json_output {
        emit_json_pretty(&report.to_json())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render::workspaces(&report);
    }
    Ok(u8::from(!report.healthy()))
}

/// Run `doctor decisions`: Source's read-only log/index reconciliation plus the
/// two-surface fork diagnosis.
///
/// The read-only path is report-only (exit 0 even when the Mission is not
/// clean). `--repair` is refused here, before any read, lock or write.
fn decisions(json_output: bool, mission: &str, repair: bool) -> Result<u8, CompatError> {
    if repair {
        return Err(unsupported(
            "UNSUPPORTED_DOCTOR_DECISIONS_REPAIR",
            "`doctor decisions --repair` rebuilds `decisions/index.json` from the event log under the ledger's sidecar lock; this slice implements the read-only diagnosis only",
            &[
                "log/index reconciliation and the two-surface fork report (implemented, read-only)",
                "--repair rebuild under the sidecar lock, with lossy-attribution retention (missing; mutating)",
                "--repair's coordination-only ledger copy (missing; mutating)",
            ],
        ));
    }
    let root = root()?;
    let surfaces = facts::DecisionSurfaces::resolve(&root, mission)?;
    let audit = doctor::audit_decisions_with_fork(
        &surfaces.events_dir,
        &surfaces.ledger_dir,
        &surfaces.mission_slug,
        Some(&surfaces.fork_report),
    )
    .map_err(map_doctor_error)?;
    let report = doctor::DecisionsReport::new(audit, Some(&surfaces.fork_report));
    if json_output {
        let payload = report.to_json().map_err(map_doctor_error)?;
        emit_json_pretty(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render::decisions(&report.audit);
        render_fork_report(&surfaces.fork_report);
    }
    // Source's read-only path is report-only: a diverged Mission still exits 0.
    Ok(0)
}

/// Source's `_emit_fork_report_human`: one line per forked stream, the ledger
/// home, then the never-executed reconcile steps.
fn render_fork_report(fork: &sk_workspace::decision_fork::DecisionsForkReport) {
    for finding in &fork.streams {
        if finding.state == sk_workspace::decision_fork::StreamState::Forked {
            let primary = finding
                .primary
                .as_ref()
                .map(|log| log.event_ids.len())
                .unwrap_or(0);
            let coordination = finding
                .coordination
                .as_ref()
                .map(|log| log.event_ids.len())
                .unwrap_or(0);
            println!(
                "  {}: forked (primary={primary} event(s), coordination={coordination} event(s))",
                finding.stream
            );
        }
    }
    if sk_workspace::decision_fork::ledger_is_coordination_only(&fork.ledger) {
        println!(
            "  decisions ledger: only on the coordination branch ({} entr(ies), {} DM file(s))",
            fork.ledger.entries_only_on_coordination.len(),
            fork.ledger.dm_files_only_on_coordination.len()
        );
    }
    for step in &fork.reconcile_steps {
        println!("  reconcile: {step}");
    }
}

/// Run `doctor invocation-pairing`: the unpaired `started` record report.
fn invocation_pairing(json_output: bool) -> Result<u8, CompatError> {
    let root = root()?;
    let audit = doctor::audit_invocation_pairing(&root).map_err(map_doctor_error)?;
    if json_output {
        emit_json_pretty(&audit.to_json())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render::invocation_pairing(&audit);
    }
    Ok(u8::from(!audit.healthy()))
}

/// Run `doctor ops`: the open Op record listing.
///
/// `--close-stale` runs the Invocation-owned sweep instead of the listing;
/// `--threshold` without it remains a usage error like Source's
/// `typer.BadParameter`, before project-root resolution.
fn ops(json_output: bool, close_stale: bool, threshold: Option<f64>) -> Result<u8, CompatError> {
    if threshold.is_some() && !close_stale {
        return Err(CompatError::usage("`--threshold` requires `--close-stale`"));
    }
    let root = root()?;
    if close_stale {
        return ops_sweep::run_sweep(&root, threshold, json_output);
    }
    let closed = sk_invocation::closed_invocation_ids(&root).map_err(|error| {
        CompatError::coded(
            "OP_CLOSURE_SPINE_UNREADABLE",
            format!("could not read the Op closure spine: {error}"),
        )
    })?;
    let closed_ids: BTreeSet<String> = closed
        .into_iter()
        .map(|invocation_id| invocation_id.as_str().to_owned())
        .collect();
    let ops_dir = root.join(sk_invocation::EVENTS_DIR);
    let non_record = non_record_op_files();
    let layout = doctor::OpStoreLayout::new(
        &ops_dir,
        sk_invocation::OP_CLOSURES_FILENAME,
        &non_record,
        &closed_ids,
    );
    let audit = doctor::audit_open_ops(&layout).map_err(map_doctor_error)?;
    if json_output {
        emit_json_pretty(&audit.to_json(&root))
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render::ops(&audit, &root);
    }
    Ok(u8::from(!audit.healthy()))
}

fn mission_not_found(handle: &str) -> CompatError {
    CompatError::coded(
        "MISSION_NOT_FOUND",
        format!("Mission not found: {handle:?}"),
    )
}

fn map_doctor_error(error: doctor::DoctorError) -> CompatError {
    let mut mapped = CompatError::coded(error.code(), error.to_string());
    if matches!(error, doctor::DoctorError::GitUnavailable { .. }) {
        mapped = mapped.with("remediation", json!(doctor::GIT_INDEX_REMEDIATION));
    }
    mapped
}

fn unsupported(code: &'static str, message: &str, coverage: &[&str]) -> CompatError {
    CompatError::unsupported(code, message.to_owned()).with("coverage", json!(coverage))
}

/// The refusal for an unimplemented subcommand.
///
/// Each one names the missing canonical owner, so the gap is actionable rather
/// than a generic "not implemented".
fn refuse(command: &DoctorCommand) -> CompatError {
    let (code, message, coverage): (&'static str, &str, &[&str]) = match command {
        DoctorCommand::CommandFiles { .. }
        | DoctorCommand::Skills { .. }
        | DoctorCommand::ToolSurfaces { .. } => (
            "UNSUPPORTED_DOCTOR_INTERNAL",
            "internal dispatch error: tool-surface diagnostics have dedicated runners",
            &[],
        ),
        DoctorCommand::SparseCheckout { .. } => (
            "UNSUPPORTED_DOCTOR_SPARSE_CHECKOUT",
            "`doctor sparse-checkout` needs the managed-lane sparse policy owner (managed-lane classification, pattern expectations); reporting only the raw git flag could misclassify a managed lane as a defect, so it is refused instead",
            &[
                "managed-lane sparse policy + worktree state classification (missing)",
                "interactive remediation (missing)",
            ],
        ),
        DoctorCommand::InvocationPairing { .. } => (
            "UNSUPPORTED_DOCTOR_INTERNAL",
            "internal dispatch error: `doctor invocation-pairing` is implemented",
            &[],
        ),
        DoctorCommand::Ops { .. } => (
            "UNSUPPORTED_DOCTOR_INTERNAL",
            "internal dispatch error: `doctor ops` is implemented, including `--close-stale`",
            &[],
        ),
        DoctorCommand::MissionState { .. } => (
            "UNSUPPORTED_DOCTOR_MISSION_STATE",
            "`doctor mission-state` needs the mission-state audit/repair owner; `--fix` writes state and is refused outright",
            &[
                "mission-state audit (missing)",
                "--fix repair + migration manifest (missing; mutating)",
                "TeamSpace dry-run (missing)",
            ],
        ),
        DoctorCommand::CharterPacks { .. } => (
            "UNSUPPORTED_DOCTOR_CHARTER_PACKS",
            "`doctor charter-packs` needs the org-charter pack health collector (override sanctions, cross-grain collisions, profile-load diagnostics); this slice does not implement it",
            &["org pack snapshot + profile health report (missing)"],
        ),
        DoctorCommand::Coordination { .. } => (
            "UNSUPPORTED_DOCTOR_COORDINATION",
            "`doctor coordination` needs the coordination-topology repair owner; `--fix` materializes state and is refused outright",
            &[
                "coordination worktree + lane sparse-drift health checks (missing)",
                "--fix materialization/fast-forward (missing; mutating)",
            ],
        ),
        DoctorCommand::Cutover { .. } => (
            "UNSUPPORTED_DOCTOR_CUTOVER",
            "`doctor cutover` needs the runtime-state cut-over auditor owner; this slice does not implement it",
            &["per-mission cut-over audit (missing)"],
        ),
        DoctorCommand::ReviewCycleReconcile { .. } => (
            "UNSUPPORTED_DOCTOR_REVIEW_CYCLE_RECONCILE",
            "`doctor review-cycle-reconcile` needs the retired-resolver census owner; this slice does not implement it",
            &["stranded review-cycle record detection (missing)"],
        ),
        DoctorCommand::Decisions { .. } => (
            "UNSUPPORTED_DOCTOR_INTERNAL",
            "internal dispatch error: `doctor decisions` is implemented (only `--repair` is refused, before dispatch)",
            &[],
        ),
        // The implemented arms never reach `refuse`.
        DoctorCommand::Identity { .. }
        | DoctorCommand::Topology { .. }
        | DoctorCommand::MissionType { .. }
        | DoctorCommand::StateRoots { .. }
        | DoctorCommand::Workspaces { .. }
        | DoctorCommand::ShimRegistry { .. }
        | DoctorCommand::Contracts { .. } => (
            "UNSUPPORTED_DOCTOR_INTERNAL",
            "internal dispatch error: this subcommand is implemented",
            &[],
        ),
    };
    unsupported(code, message, coverage)
}

/// Parse a `doctor` command line (used by the tests and by the root dispatch).
pub fn parse(argv: &[&str]) -> Result<DoctorArgs, clap::Error> {
    DoctorArgs::try_parse_from(argv.iter().copied())
}

/// The list of implemented leaf subcommand names, for the capability matrix.
pub const IMPLEMENTED_LEAVES: [&str; 13] = [
    "identity",
    "topology",
    "mission-type",
    "state-roots",
    "workspaces",
    "decisions",
    "invocation-pairing",
    "ops",
    "command-files",
    "skills",
    "tool-surfaces",
    "shim-registry",
    "contracts",
];

/// The list of refused leaf subcommand names, for the capability matrix.
pub const REFUSED_LEAVES: [&str; 6] = [
    "sparse-checkout",
    "mission-state",
    "charter-packs",
    "coordination",
    "cutover",
    "review-cycle-reconcile",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upstream_leaf_surface_parses() {
        for argv in [
            vec!["doctor", "identity"],
            vec!["doctor", "identity", "--json", "--mission", "083-foo"],
            vec!["doctor", "identity", "--fail-on", "legacy,orphan"],
            vec!["doctor", "topology", "--json"],
            vec!["doctor", "mission-type", "--mission", "083-foo", "--json"],
            vec!["doctor", "state-roots", "--json"],
            vec!["doctor", "workspaces", "--fix"],
            vec![
                "doctor",
                "tool-surfaces",
                "--kind",
                "command-skill",
                "--tool",
                "codex",
                "--fix",
            ],
            vec!["doctor", "ops", "--close-stale", "--threshold", "0"],
            vec!["doctor", "decisions", "--mission", "083-foo", "--repair"],
        ] {
            assert!(parse(&argv).is_ok(), "should parse: {argv:?}");
        }
    }

    #[test]
    fn every_leaf_name_in_the_capability_lists_is_real() {
        let command = <DoctorArgs as clap::CommandFactory>::command();
        let leaves: BTreeSet<String> = command
            .get_subcommands()
            .map(|sub| sub.get_name().to_owned())
            .collect();
        for leaf in IMPLEMENTED_LEAVES.into_iter().chain(REFUSED_LEAVES) {
            assert!(leaves.contains(leaf), "unknown leaf {leaf}");
        }
        assert_eq!(
            leaves.len(),
            IMPLEMENTED_LEAVES.len() + REFUSED_LEAVES.len()
        );
    }

    #[test]
    fn a_bare_doctor_runs_no_diagnostic() {
        let args = parse(&["doctor"]).unwrap();
        assert!(args.command.is_none());
        assert_eq!(run(&args).unwrap(), 2);
    }

    #[test]
    fn the_mission_not_found_shape_is_a_domain_refusal() {
        let error = mission_not_found("083-missing");
        assert_eq!(error.exit(), 1);
        assert_eq!(error.to_json()["error_code"], json!("MISSION_NOT_FOUND"));
    }

    #[test]
    fn a_mutating_flag_is_refused_rather_than_silently_ignored() {
        let args = parse(&["doctor", "workspaces", "--fix"]).unwrap();
        let error = run(&args).unwrap_err();
        assert_eq!(error.exit(), 2);
        assert_eq!(
            error.to_json()["error_code"],
            json!("UNSUPPORTED_DOCTOR_WORKSPACES_FIX")
        );
    }

    #[test]
    fn every_refusal_names_its_remaining_coverage() {
        for argv in [
            vec!["doctor", "cutover"],
            vec!["doctor", "sparse-checkout"],
            vec!["doctor", "mission-state", "--audit"],
            vec!["doctor", "charter-packs"],
        ] {
            let command = parse(&argv).unwrap().command.expect("leaf");
            let error = refuse(&command);
            assert_eq!(error.exit(), 2);
            let payload = error.to_json();
            assert!(payload["error_code"].is_string(), "{argv:?}");
            assert!(
                payload["coverage"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty()),
                "{argv:?} must name its remaining coverage"
            );
        }
    }

    #[test]
    fn the_mutating_flags_of_the_new_leaves_are_refused_before_any_write() {
        let argv = ["doctor", "decisions", "--mission", "x", "--repair"];
        let args = parse(&argv).unwrap();
        let error = run(&args).unwrap_err();
        assert_eq!(error.exit(), 2, "{argv:?}");
        let payload = error.to_json();
        assert!(payload["error_code"].is_string(), "{argv:?}");
        assert!(
            payload["coverage"]
                .as_array()
                .is_some_and(|items| items.len() >= 2),
            "{argv:?}"
        );
    }

    #[test]
    fn a_threshold_without_close_stale_is_a_usage_refusal() {
        let args = parse(&["doctor", "ops", "--threshold", "4"]).unwrap();
        let error = run(&args).unwrap_err();
        assert_eq!(error.exit(), 2);
        assert!(error.message().contains("--close-stale"));
    }

    #[test]
    fn the_non_record_op_file_list_comes_from_the_owners() {
        let names = non_record_op_files();
        assert!(names.contains(&"ops-index.jsonl"));
        assert!(names.contains(&"lifecycle.jsonl"));
        assert!(names.contains(&"propagation-errors.jsonl"));
    }
}
