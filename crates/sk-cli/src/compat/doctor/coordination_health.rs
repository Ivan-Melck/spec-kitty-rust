//! `spec-kitty doctor coordination` — the read-only coordination health adapter.
//!
//! The domain work lives in [`sk_workspace::coordination_health`]; this module
//! only resolves the project root, refuses the mutating arm, and renders the
//! report in the reference's two shapes (JSON array / human lines).
//!
//! # What is real here
//!
//! * `doctor coordination` (read-only, optional `--json`, `--check-staleness`,
//!   `--mission`) runs the native diagnosis: Git version, tracked `.worktrees/`
//!   content, stranded coordination `done` re-verification, and — per Mission that
//!   declares a `coordination_branch` — the coordination worktree's presence,
//!   branch, cleanliness and freshness, the lane sparse-checkout policy, plus the
//!   optional coordination-branch-vs-target staleness (including the expected
//!   divergence exemption).
//! * A `--mission` handle resolves through the canonical Mission selector, so
//!   `MISSION_NOT_FOUND` / ambiguous / invalid handles fail closed instead of
//!   silently scanning every Mission.
//!
//! # What is refused, and why
//!
//! * `--fix` — every reference `--fix` arm is a mutation (materialize a
//!   coordination worktree, flatten `meta.json`, backfill topology, fast-forward
//!   a branch, revert a stranded `done`). This lane is read-only and refuses
//!   before any read, lock or write.
//! * A read-only run whose own observation failed is refused as **incomplete**
//!   rather than printed: dropping a check and exiting 0 would present a partial
//!   report as a passing health check. The refusal names the exact missing
//!   observation (see [`sk_workspace::coordination_health::UnavailableCheck`]);
//!   every check itself is composed, so this is a failure path, not the normal one.

use serde_json::json;

use sk_workspace::coordination_health::{
    CoordinationHealthError, CoordinationHealthReport, CoordinationHealthRequest,
    coordination_health,
};
use sk_workspace::workspace::ProjectWorkspace;

use crate::compat::support::{CompatError, emit_json_pretty, project_root};

/// `--fix` refusal code (the mutating coordination repair owner does not exist).
pub const UNSUPPORTED_FIX: &str = "UNSUPPORTED_DOCTOR_COORDINATION_FIX";
/// Read-only refusal code: at least one requested check has no native owner yet.
pub const UNSUPPORTED_INCOMPLETE: &str = "UNSUPPORTED_DOCTOR_COORDINATION_INCOMPLETE";

/// Run one `doctor coordination` invocation, returning the process exit code.
pub fn run(
    fix: bool,
    json_output: bool,
    check_staleness: bool,
    mission: Option<&str>,
) -> Result<u8, CompatError> {
    if fix {
        return Err(unsupported_fix());
    }

    let root = project_root()?;
    let workspace = ProjectWorkspace::open(&root).map_err(|error| {
        CompatError::coded(
            "PROJECT_ROOT_NOT_A_REPOSITORY",
            format!(
                "{} is not a readable Git checkout, so coordination health cannot be diagnosed: {error}",
                root.display()
            ),
        )
    })?;

    let mut request = CoordinationHealthRequest::new();
    if check_staleness {
        request = request.with_staleness();
    }
    if let Some(handle) = mission {
        request = request.with_mission(handle);
    }

    let report = coordination_health(&workspace, &request).map_err(map_error)?;
    if !report.is_complete() {
        return Err(incomplete_refusal(&report));
    }

    if json_output {
        emit_json_pretty(&report.to_json())
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render(&report);
    }
    // `is_complete()` was just checked; an exit code therefore always exists.
    Ok(report.exit_code().unwrap_or(1))
}

/// Render the findings exactly as the reference's human surface does.
///
/// Informational only: the reference styles severity with Rich colours and this
/// adapter prints plain text. The rows, messages and remedies are the same.
fn render(report: &CoordinationHealthReport) {
    for finding in report.findings() {
        println!("{}: {}", finding.severity(), finding.message());
        if let Some(next_step) = finding.next_step() {
            println!("  → {next_step}");
        }
    }
}

/// The reference quotes a handle with Python `repr` (`!r`); reuse the Kernel's
/// one canonical formatter rather than `{:?}`.
fn python_repr(text: &str) -> String {
    sk_kernel::PythonString::from_scalars(text).repr()
}

/// The `--fix` refusal: mutating, refused before any read or write.
fn unsupported_fix() -> CompatError {
    CompatError::unsupported(
        UNSUPPORTED_FIX,
        "`doctor coordination --fix` materializes coordination worktrees, flattens `meta.json`, backfills topology, fast-forwards coordination branches and reverts stranded coordination `done` commits; this lane implements the read-only diagnosis only",
    )
    .with(
        "coverage",
        json!([
            "read-only Git version, tracked-.worktrees hygiene and coordination worktree health (implemented)",
            "--fix materialization / flatten / topology backfill / fast-forward / strand revert (missing; mutating)",
        ]),
    )
}

/// The incomplete-report refusal: a named gap per uncomposable check.
fn incomplete_refusal(report: &CoordinationHealthReport) -> CompatError {
    let mut owners: Vec<String> = Vec::new();
    let mut checks: Vec<String> = Vec::new();
    let mut coverage: Vec<serde_json::Value> = Vec::new();
    for check in report.unavailable() {
        owners.push(check.owner().to_owned());
        checks.push(check.check().to_owned());
        coverage.push(json!({
            "check": check.check(),
            "status": "unavailable",
            "owner": check.owner(),
            "detail": check.detail(),
        }));
    }
    coverage.push(json!({
        "check": "git-version / tracked-.worktrees / coordination-worktree-health / coord-branch-staleness",
        "status": "implemented (read-only)",
    }));

    CompatError::unsupported(
        UNSUPPORTED_INCOMPLETE,
        format!(
            "`doctor coordination` cannot report a complete health verdict yet: {} check(s) have no canonical native owner ({}); reporting only the rest would read as a passing check",
            checks.len(),
            owners.join("; ")
        ),
    )
    .with("unavailable_checks", json!(checks))
    .with("coverage", serde_json::Value::Array(coverage))
}

/// Map a domain failure onto the compatibility error surface.
///
/// Mission selector failures keep the reference's stable codes and exit 1; a
/// diagnosis that could not run at all is a domain refusal too (never a
/// fabricated clean report).
fn map_error(error: CoordinationHealthError) -> CompatError {
    match &error {
        CoordinationHealthError::Mission(sk_mission::MissionError::MissionNotFound {
            selector,
        }) => CompatError::coded(
            "MISSION_NOT_FOUND",
            format!("Mission not found: {}", python_repr(selector)),
        )
        .with("handle", json!(selector)),
        CoordinationHealthError::Mission(sk_mission::MissionError::AmbiguousMission {
            selector,
            count,
        }) => CompatError::coded(
            "AMBIGUOUS_HANDLE",
            format!(
                "Ambiguous handle: {} (matches {count} Missions)",
                python_repr(selector)
            ),
        )
        .with("handle", json!(selector)),
        CoordinationHealthError::Mission(sk_mission::MissionError::InvalidSelector {
            selector,
        }) => CompatError::coded(
            "MISSION_NOT_FOUND",
            format!("Mission not found: {}", python_repr(selector)),
        )
        .with("handle", json!(selector)),
        _ => CompatError::coded("COORDINATION_HEALTH_UNAVAILABLE", error.to_string()),
    }
}
