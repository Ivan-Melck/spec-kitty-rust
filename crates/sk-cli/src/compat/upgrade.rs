//! `spec-kitty upgrade`: the native project-upgrade route.
//!
//! The command is an adapter over the canonical upgrade authority in
//! [`sk_install::upgrade`] — [`sk_install::plan_upgrade`] for the read-only
//! preview and selection, and [`sk_install::run_upgrade`] for the real run
//! (ledger writes, per-migration `detect`/`can_apply`/`apply`, metadata
//! version stamp). Nothing about selection, ordering, partitions or failure
//! recording is re-implemented here.
//!
//! Source-grounded surface (pinned `cli/commands/upgrade.py`):
//!
//! | Option | Behaviour |
//! | --- | --- |
//! | `--dry-run` | plan only; exit 0 when the plan completes; writes nothing |
//! | `--force` / `--yes` / `-y` | non-interactive confirmation (native is always non-interactive, so this only records intent) |
//! | `--target` | upper bound for selection; below the project version is exit 1 |
//! | `--json` | the project-upgrade JSON contract (see below) |
//! | `-v` / `--verbose` | per-migration change detail |
//! | `--no-worktrees` | skip the per-worktree fan-out (the default upgrades every non-integrating `.worktrees/` checkout) |
//! | `--cli` | CLI guidance only; works outside a project |
//! | `--project` | project migrations only; outside a project is exit 1 |
//! | `--no-nag` | accepted; this build never nags |
//! | `--agent-check` | the agent-host readiness payload |
//! | `--agent-choice` + `--agent-latest` | record an agent-host choice in the user nag cache |
//!
//! **Exit codes** (upstream R-08): `0` success or a completed preview; `2` a
//! flag conflict (including `--cli --project`), an invalid `--agent-choice`, or
//! a missing `--agent-latest`; `5` the project is too new for this CLI (schema
//! or version, not bypassable); `6` the project metadata is missing or corrupt;
//! `1` anything else, including a failed migration run.
//!
//! **Compatibility planner contract.** `upgrade --plan-json` emits the full
//! preview plan and `upgrade --json` with `--project` or `--dry-run` emits the
//! frozen `contracts/compat-planner.json` envelope (schema_version 1), both built
//! here from the same native authorities the real run uses:
//!
//! - `project` — the schema scan over `.kittify/metadata.yaml`
//!   (`no_project`/`uninitialized`/`legacy`/`stale`/`compatible`/`too_new`/`corrupt`),
//!   with the pinned 256 KiB size guard and integer range check;
//! - `pending_migrations` — [`sk_install::select_candidates`], i.e. declared
//!   minus applied, capped by the run's target, never a version window;
//! - `effects`/`dispositions` (plan-json) — the observed `.kittify/` shared-asset
//!   classification for the project root and each worktree, as
//!   `sk_install::migrations::completion_legacy::layout` performs it, with before/
//!   after content hashes;
//! - `diagnostics` — a selected migration whose `detect` cannot evaluate the
//!   project, and any migration this build cannot run.
//!
//! **Documented deviations**, all fail-closed or explicitly narrower:
//!
//! - `cli.latest_version` is `null` and `latest_source` is `"none"`: this build
//!   makes no network lookup, which is the source's own `NoNetworkProvider`
//!   answer, never a guessed "latest" fact.
//! - `effects` and `dispositions` describe the `.kittify/` layout assessment,
//!   not the tool-surface repair plan (`prepare_upgrade_repairs`); the finalizer's
//!   read-only repair plan is reported separately under `finalizer`.
//!   `execution_artifacts` is empty because this build's writer stages no
//!   temporary artifact (the Source array describes its staging directories).
//! - Auto-commit of upgrade churn **is** performed: `commit_policy` reports the
//!   project's configured `auto_commit` (`project_enabled`) and whether the
//!   worktree fan-out runs (`worktrees_enabled`), and the run commits the main
//!   checkout churn plus each touched non-integrating worktree under that policy.
//!   `roots` comes from the same fan-out plan the run executes.
//!
//! The agent-choice cache is the source's `NagCache` record shape at
//! `<cache>/spec-kitty/upgrade-nag.json`, so the Python CLI reads back exactly
//! what this command writes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use serde_json::{Map, Value, json};

use sk_install::{
    AgentCheckInput, AgentUpgradeChoice, CURRENT_SCHEMA_VERSION, InstallError, MigrationCatalog,
    MigrationLedger, MigrationPlanRow, PlannedOutcome, ProjectMetadata, UpgradePlan,
    UpgradeRequest, plan_agent_check, plan_upgrade, run_upgrade,
};

use super::support::{CompatError, emit_json, emit_json_pretty};

pub mod upgrade_worktrees;

/// Project-upgrade exit code: the project is too new for this CLI.
const EXIT_PROJECT_TOO_NEW: i32 = 5;
/// Project-upgrade exit code: missing or corrupt project metadata.
const EXIT_PROJECT_CORRUPT: i32 = 6;

/// `upgrade` options.
#[derive(Clone, Debug, Args)]
pub struct UpgradeArgs {
    /// Preview changes without applying them.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip confirmation prompts (native never prompts).
    #[arg(long)]
    pub force: bool,
    /// Target version (defaults to the current CLI version).
    #[arg(long, value_name = "VERSION")]
    pub target: Option<String>,
    /// Output results as JSON.
    #[arg(long)]
    pub json: bool,
    /// Output the complete preview plan as JSON.
    #[arg(long = "plan-json")]
    pub plan_json: bool,
    /// Show detailed migration information.
    #[arg(long, short = 'v')]
    pub verbose: bool,
    /// Skip upgrading worktrees.
    #[arg(long = "no-worktrees")]
    pub no_worktrees: bool,
    /// Restrict to CLI guidance only; works outside any project.
    #[arg(long)]
    pub cli: bool,
    /// Restrict to current-project migrations.
    #[arg(long)]
    pub project: bool,
    /// Non-interactive confirmation (alias for --force).
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Suppress upgrade-nag output explicitly.
    #[arg(long = "no-nag")]
    pub no_nag: bool,
    /// Emit the agent-host upgrade prompt JSON.
    #[arg(long = "agent-check")]
    pub agent_check: bool,
    /// Record an agent-host upgrade choice.
    #[arg(long = "agent-choice", value_name = "CHOICE")]
    pub agent_choice: Option<String>,
    /// Latest version for --agent-choice.
    #[arg(long = "agent-latest", value_name = "VERSION")]
    pub agent_latest: Option<String>,
}

/// Run `spec-kitty upgrade` with `root` as the project directory.
///
/// Returns the process exit code on success paths; refusals that the upstream
/// command reports as an error envelope plus a specific exit class are returned
/// as [`CompatError`] carrying that class.
pub fn run(root: &Path, args: &UpgradeArgs) -> Result<i32, CompatError> {
    // 1. Flag-intent conflicts (source: `_check_upgrade_intent_conflicts`).
    if args.agent_check && args.agent_choice.is_some() {
        return Err(CompatError::usage(
            "--agent-check and --agent-choice are mutually exclusive.",
        ));
    }

    // 2. Agent-host flags own the invocation when present.
    if args.agent_check {
        return agent_check(args.json);
    }
    if let Some(choice) = &args.agent_choice {
        return record_agent_choice(choice, args.agent_latest.as_deref(), args.json);
    }

    // 3. `--cli` and `--project` are mutually exclusive (exit 2).
    if args.cli && args.project {
        return fail(
            2,
            "--cli and --project are mutually exclusive. Use --cli for CLI guidance only, or --project for project migrations only.",
            args.json,
            "INCOMPATIBLE_FLAGS",
        );
    }

    if args.cli {
        return cli_mode(args.json);
    }

    // 4. Project resolution: `--project` fails outside a project; the bare
    //    invocation falls back to CLI guidance. `--plan-json` always answers with
    //    its own blocked envelope (the source's `project_not_initialized`).
    let initialized = root.join(".kittify").is_dir() || root.join(".specify").is_dir();
    if !initialized {
        if args.plan_json {
            return blocked_full_plan(
                root,
                args,
                1,
                "project_not_initialized",
                "Not a Spec Kitty project",
            );
        }
        if args.project {
            return fail(
                1,
                "Not a Spec Kitty project. Run 'spec-kitty init' to initialize a project.",
                args.json,
                "PROJECT_NOT_INITIALIZED",
            );
        }
        return cli_mode(args.json);
    }

    // 5. Source order: the full preview plan first, then the too-new guard, then
    //    the `--json` + (`--project` | `--dry-run`) compat-planner contract.
    if args.plan_json {
        return full_plan_json(root, args);
    }
    if let Some(message) = project_too_new(root) {
        if args.json {
            // The source's too-new JSON answer is the compat-planner envelope
            // (`BLOCK_CLI_UPGRADE`, exit 5), never a bare error object.
            return planner_json(root, args);
        }
        return fail(EXIT_PROJECT_TOO_NEW, &message, false, "PROJECT_TOO_NEW");
    }
    if args.json && (args.project || args.dry_run) {
        return planner_json(root, args);
    }

    let target = args.target.clone();
    let cli_version = super::CORE_VERSION;

    let request = build_request(root, target.clone(), args.dry_run);

    if args.dry_run {
        let plan = match plan_upgrade(&request) {
            Ok(plan) => plan,
            Err(error) => {
                let (code, message) = classify_run_error(&error);
                return fail(code, &message, args.json, error_code(code));
            }
        };
        report_plan(
            &plan,
            &request,
            args.json,
            args.verbose,
            &preview_rows(&request),
        )?;
        // A preview that could not be completed exits 1: a `detect`/`can_apply`
        // that cannot evaluate the project, or a selected migration this build
        // cannot run (the real run would refuse), so the preview must not claim
        // a clean 0.
        let incomplete = plan.rows.iter().any(|row| {
            matches!(
                row.outcome,
                PlannedOutcome::Failed | PlannedOutcome::Unimplemented
            )
        });
        return Ok(if incomplete { 1 } else { 0 });
    }

    // The main checkout's churn baseline is captured BEFORE the first write
    // (Source `capture_upgrade_baseline`), so the commit set is exactly what
    // this run introduced.
    let auto_commit = auto_commit_default(root).map_err(|error| {
        CompatError::domain(format!("could not read the project config: {error}"))
    })?;
    let main_baseline = if auto_commit {
        upgrade_worktrees::capture_baseline(root)
    } else {
        None
    };
    // Source `version_stamp_boundary`: this outer snapshot is authoritative over the
    // runner's own inner capture (it is taken before any stamp write), and it also
    // restores when the terminal surface repair fails. A dry run never writes, so it
    // takes no snapshot.
    let pre_run_stamp = sk_install::metadata::VersionStamp::capture(&root.join(".kittify"));
    match run_upgrade(&request) {
        Ok(report) => {
            // Per-worktree fan-out (#2385), then the terminal finalizer. Both run
            // after the main checkout reached its target, through the canonical
            // planner/ledger/commit seams (`upgrade_worktrees`).
            let fanout = if args.no_worktrees {
                None
            } else {
                Some(
                    upgrade_worktrees::run_worktree_upgrade(
                        root,
                        &request.cli_version,
                        Some(&report.to_version),
                        auto_commit,
                        request.lock_wait,
                        &report.selected,
                    )
                    .map_err(|error| {
                        CompatError::domain(format!("worktree upgrade failed: {error}"))
                    })?,
                )
            };
            let agents = agent_keys(root);
            let finalized =
                upgrade_worktrees::finalize_project(root, &report.to_version, &agents, false)
                    .map_err(|error| {
                        CompatError::domain(format!("upgrade finalizer failed: {error}"))
                    })?;
            let worktree_failures = fanout
                .as_ref()
                .map(upgrade_worktrees::WorktreeUpgradeReport::failures)
                .unwrap_or_default();
            // The single main-checkout churn commit (Source C4 order position 3:
            // after the terminal surface repair). A failed main run commits nothing;
            // files dirty before the run are never candidates. Source's
            // `_REASONS_THAT_KEEP_THE_MAIN_COMMIT` keeps this commit on a
            // worktree-only failure: that gates only the failed root's own commit.
            let mut main_commit_failures: Vec<String> = Vec::new();
            let main_commit = if !auto_commit {
                None
            } else if !report.success {
                if !args.json {
                    println!(
                        "⚠ {}",
                        upgrade_worktrees::FAILED_RUN_LEFT_UNCOMMITTED_WARNING
                    );
                }
                None
            } else {
                match &main_baseline {
                    None => {
                        println!("⚠ {}", upgrade_worktrees::BASELINE_UNAVAILABLE_WARNING);
                        None
                    }
                    Some(baseline) => {
                        match upgrade_worktrees::commit_touched_checkout(
                            root,
                            baseline,
                            &report.from_version,
                            &report.to_version,
                        ) {
                            Ok(outcome) => Some(outcome),
                            Err(error) => {
                                main_commit_failures.push(format!(
                                    "could not commit the main checkout churn: {error}"
                                ));
                                None
                            }
                        }
                    }
                }
            };
            // Source `version_stamp_boundary`: the outcome's FAILED kind includes the
            // fatal worktree channel, and the boundary restores the main version stamp
            // only after the whole run (the churn commit above included the stamp the
            // run reached). The restore itself is left uncommitted, as Source prints.
            let failed_outcome = !report.success || !worktree_failures.is_empty();
            let restored = if failed_outcome {
                match pre_run_stamp.restore(&root.join(".kittify")) {
                    Ok(restored) => restored,
                    Err(error) => {
                        if !args.json {
                            println!(
                                "⚠ Could not restore .kittify/metadata.yaml after the failed upgrade: {error}"
                            );
                        }
                        false
                    }
                }
            } else {
                false
            };
            if args.json {
                emit_json_pretty(&report_json(
                    &report,
                    &request,
                    main_commit.as_ref(),
                    &worktree_failures,
                ))
                .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
            } else {
                println!("Current version: {}", report.from_version);
                println!("Target version:  {}", report.to_version);
                println!();
                let applied = report.applied();
                let skipped = report.skipped();
                if !report.success {
                    println!("✗ Upgrade did not complete");
                    for error in &report.errors {
                        println!("✗ {error}");
                    }
                } else if applied.is_empty() {
                    println!(
                        "✓ Project is already at {} ({} setup step(s) not needed)",
                        report.to_version,
                        skipped.len()
                    );
                } else {
                    for row in &report.rows {
                        if row.result == "applied" {
                            for change in &row.changes {
                                println!("  {}: {change}", row.id);
                            }
                        }
                    }
                    println!();
                    println!("✓ Upgraded to {}", report.to_version);
                    println!("  Migrations applied: {}", applied.join(", "));
                }
                for row in &report.rows {
                    for warning in &row.warnings {
                        println!("⚠ {}: {warning}", row.id);
                    }
                }
                if report.metadata_written {
                    println!("   version stamp: .kittify/metadata.yaml");
                }
                if let Some(fanout) = &fanout {
                    for root in &fanout.roots {
                        for warning in &root.warnings {
                            println!("⚠ {warning}");
                        }
                        if let Some(commit) = &root.commit
                            && let Some(id) = &commit.commit
                        {
                            println!(
                                "   committed upgrade churn in {} ({id})",
                                commit.paths.len().max(1)
                            );
                        }
                    }
                    if !fanout.roots.is_empty() {
                        println!(
                            "   worktrees: {} examined, {} upgraded",
                            fanout.roots.len(),
                            fanout
                                .roots
                                .iter()
                                .filter(|root| !root.rows.is_empty() || root.metadata_written)
                                .count()
                        );
                    }
                }
                if let Some(commit) = &main_commit {
                    if let Some(reason) = &commit.reason {
                        println!("⚠ {reason}");
                    }
                    if let Some(id) = &commit.commit {
                        println!(
                            "   committed main-checkout churn ({} file(s), {id})",
                            commit.paths.len()
                        );
                    }
                }
                for path in &finalized.artifacts {
                    println!("   finalized {}: {}", path.path, path.change);
                }
                for warning in &finalized.warnings {
                    println!("⚠ {warning}");
                }
                if restored {
                    println!(
                        "⚠ .kittify/metadata.yaml restored to {}; the change is uncommitted",
                        pre_run_stamp
                            .version()
                            .unwrap_or_else(|| "its previous version".to_string())
                    );
                }
                for failure in &worktree_failures {
                    println!("✗ {failure}");
                }
            }
            for failure in &main_commit_failures {
                println!("✗ {failure}");
            }
            Ok(upgrade_exit_code(
                &report,
                &worktree_failures,
                &main_commit_failures,
            ))
        }
        Err(error) => {
            // Source's boundary restores the pre-run stamp when the body raises, and a
            // restore failure must not replace the reported error with a traceback.
            if let Err(restore_error) = pre_run_stamp.restore(&root.join(".kittify"))
                && !args.json
            {
                println!(
                    "⚠ Could not restore .kittify/metadata.yaml after the failed upgrade: {restore_error}"
                );
            }
            let (code, message) = classify_run_error(&error);
            if args.json {
                emit_json(&json!({
                    "status": "failed",
                    "outcome": "failed",
                    "failure_reasons": [message],
                    "current_version": null,
                    "target_version": target.clone().unwrap_or_else(|| cli_version.to_string()),
                    "dry_run": false,
                    "success": false,
                    "errors": [message],
                    "warnings": [],
                    "manual_review_required": false,
                    "manual_review_paths": [],
                    "auto_committed": false,
                    "auto_commit_paths": [],
                }))
                .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
                return Ok(code);
            }
            fail(code, &message, false, error_code(code))
        }
    }
}

fn upgrade_exit_code(
    report: &sk_install::UpgradeReport,
    worktree_failures: &[String],
    main_commit_failures: &[String],
) -> i32 {
    if report.success && worktree_failures.is_empty() && main_commit_failures.is_empty() {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod result_tests {
    use super::upgrade_exit_code;

    #[test]
    fn a_returned_main_failure_never_exits_success() {
        let mut report = sk_install::UpgradeReport::default();
        assert_eq!(upgrade_exit_code(&report, &[], &[]), 1);
        report.success = true;
        assert_eq!(upgrade_exit_code(&report, &[], &[]), 0);
        assert_eq!(
            upgrade_exit_code(&report, &["worktree failure".to_owned()], &[]),
            1
        );
        assert_eq!(
            upgrade_exit_code(&report, &[], &["commit failure".to_owned()]),
            1
        );
    }
}

fn error_code(code: i32) -> &'static str {
    match code {
        EXIT_PROJECT_TOO_NEW => "PROJECT_TOO_NEW",
        EXIT_PROJECT_CORRUPT => "PROJECT_CORRUPT",
        2 => "INCOMPATIBLE_FLAGS",
        _ => "UPGRADE_FAILED",
    }
}

/// Report a command-level refusal and return its process exit class.
///
/// The upstream command prints its own diagnostic and exits with the class; the
/// dispatcher must not add a second envelope on top of it.
fn fail(
    code: i32,
    message: &str,
    json_output: bool,
    error_code: &'static str,
) -> Result<i32, CompatError> {
    if json_output {
        emit_json(&json!({ "error": message, "error_code": error_code, "exit_code": code }))
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        eprintln!("Error: {message}");
    }
    Ok(code)
}

/// Run `spec-kitty upgrade` from the current directory (CLI root wiring entry).
///
/// The upstream command is project-*directory* scoped, not root-discovered: a
/// bare `upgrade` outside a project falls back to CLI guidance, and `--project`
/// outside one is exit 1. This wrapper supplies exactly that contract.
pub fn run_here(args: &UpgradeArgs) -> Result<i32, CompatError> {
    let cwd = std::env::current_dir()
        .map_err(|error| CompatError::domain(format!("cannot read current directory: {error}")))?;
    run(&cwd, args)
}

fn build_request(root: &Path, target: Option<String>, dry_run: bool) -> UpgradeRequest {
    let mut request = UpgradeRequest::new(root, super::CORE_VERSION);
    request.lock_wait = Duration::from_secs(10);
    if let Some(target) = target {
        request = request.with_target(target);
    }
    if dry_run {
        request = request.dry_run();
    }
    request
}

/// The upstream `--agent-check` payload.
fn agent_check(json_output: bool) -> Result<i32, CompatError> {
    let choice = read_nag_cache().and_then(|record| {
        if record.never_ask {
            Some(AgentUpgradeChoice::NeverAsk)
        } else if record.always_upgrade {
            Some(AgentUpgradeChoice::Always)
        } else {
            None
        }
    });
    let input = AgentCheckInput {
        installed_version: super::CORE_VERSION.to_string(),
        // No network lookup in this build: the source's own offline answer.
        latest_version: None,
        latest_source: None,
        install_method: install_method(),
        auto_upgrade_safe: false,
        choice,
        upgrade_command: upgrade_hint().map(|(command, _)| command),
        guidance_note: upgrade_hint().map(|(_, note)| note),
    };
    let payload = plan_agent_check(&input).map_err(|error| {
        CompatError::coded("AGENT_CHECK_FAILED", format!("agent check failed: {error}"))
    })?;
    let mut value = serde_json::to_value(&payload)
        .map_err(|error| CompatError::domain(format!("could not serialize payload: {error}")))?;
    if let Some(map) = value.as_object_mut() {
        map.insert("reason".to_string(), json!("up_to_date"));
    }
    if json_output {
        emit_json_pretty(&value)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if payload.action != "none" {
        println!(
            "Spec Kitty upgrade available: {} -> {}",
            payload.installed_version,
            payload
                .latest_version
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        );
    }
    Ok(0)
}

/// `--agent-choice` with the source's nag-cache record shape.
fn record_agent_choice(
    choice_raw: &str,
    latest_version: Option<&str>,
    json_output: bool,
) -> Result<i32, CompatError> {
    let choice = match AgentUpgradeChoice::parse(choice_raw) {
        Ok(choice) => choice,
        Err(_) => {
            if json_output {
                let _ = emit_json(&json!({
                    "status": "error",
                    "error": "invalid_agent_choice",
                    "choice": choice_raw,
                }));
            }
            return fail(
                2,
                &format!("invalid agent choice: {choice_raw}"),
                false,
                "INVALID_AGENT_CHOICE",
            );
        }
    };
    let Some(latest) = latest_version else {
        if json_output {
            let _ = emit_json(&json!({"status": "error", "error": "missing_agent_latest"}));
        }
        return fail(
            2,
            "--agent-latest is required with --agent-choice",
            false,
            "MISSING_AGENT_LATEST",
        );
    };

    let now = sk_install::now_utc_iso();
    let existing = read_nag_cache();
    let record = NagRecord {
        cli_version_key: super::CORE_VERSION.to_string(),
        latest_version: Some(latest.to_string()),
        latest_source: "pypi".to_string(),
        fetched_at: now.clone(),
        last_shown_at: Some(now.clone()),
        remote_version_seen: Some(latest.to_string()),
        snooze_step: existing
            .as_ref()
            .and_then(|record| record.snooze_step.clone()),
        snoozed_at: existing
            .as_ref()
            .and_then(|record| record.snoozed_at.clone()),
        always_upgrade: choice == AgentUpgradeChoice::Always,
        never_ask: choice.suppresses_future_prompts(),
    };
    write_nag_cache(&record).map_err(|error| {
        CompatError::domain(format!("could not record the agent choice: {error}"))
    })?;
    let payload = json!({
        "status": "recorded",
        "choice": choice.as_str(),
        "latest_version": latest,
    });
    if json_output {
        emit_json_pretty(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("Recorded {}", choice.as_str());
    }
    Ok(0)
}

/// `--cli`: CLI guidance only, valid outside a project.
fn cli_mode(json_output: bool) -> Result<i32, CompatError> {
    let hint = upgrade_hint();
    if json_output {
        let payload = json!({
            "schema_version": 1,
            "status": "cli_only",
            "installed_version": super::CORE_VERSION,
            "latest_version": Value::Null,
            "latest_source": Value::Null,
            "install_method": install_method(),
            "upgrade_command": hint.as_ref().map(|(command, _)| command.clone()),
            "upgrade_note": hint.as_ref().map(|(_, note)| note.clone()),
            "reason": "up_to_date",
            "rendered_human": cli_guidance_line(),
        });
        emit_json_pretty(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        println!("{}", cli_guidance_line());
    }
    Ok(0)
}

fn cli_guidance_line() -> String {
    match upgrade_hint() {
        Some((command, _)) => format!(
            "Spec Kitty CLI {} is installed. Upgrade with: {command}",
            super::CORE_VERSION
        ),
        None => format!(
            "Spec Kitty CLI {} is installed. See the installation guide for your installer's upgrade command.",
            super::CORE_VERSION
        ),
    }
}

/// The install method this binary can *prove*, or `unknown`.
///
/// Only an evidence-bearing path prefix counts; a guess would turn an advisory
/// hint into a fabricated fact.
fn install_method() -> String {
    let Ok(executable) = std::env::current_exe() else {
        return "unknown".to_string();
    };
    let spelled = executable.to_string_lossy().replace('\\', "/");
    if spelled.contains("/uv/tools/spec-kitty-cli/") {
        "uv-tool".to_string()
    } else if spelled.contains("/pipx/venvs/spec-kitty-cli/") {
        "pipx".to_string()
    } else {
        "unknown".to_string()
    }
}

/// The upgrade command and note for the detected install method.
fn upgrade_hint() -> Option<(String, String)> {
    match install_method().as_str() {
        "uv-tool" => Some((
            "uv tool upgrade spec-kitty-cli".to_string(),
            "Upgrade the uv-managed tool installation.".to_string(),
        )),
        "pipx" => Some((
            "pipx upgrade spec-kitty-cli".to_string(),
            "Upgrade the pipx-managed installation.".to_string(),
        )),
        _ => None,
    }
}

/// The message when the project's recorded version or schema is newer than this
/// CLI, or `None` when the project is within range (or has no metadata yet).
fn project_too_new(root: &Path) -> Option<String> {
    let metadata = ProjectMetadata::load(&root.join(".kittify"))?;
    if let Some(schema) = metadata.schema_version
        && schema > i64::from(CURRENT_SCHEMA_VERSION)
    {
        return Some(format!(
            "This project uses Spec Kitty project schema {schema}, but this CLI supports up to schema {CURRENT_SCHEMA_VERSION}. Upgrade your CLI."
        ));
    }
    if let (Ok(project), Ok(cli)) = (
        sk_install::Version::parse(&metadata.version),
        sk_install::Version::parse(super::CORE_VERSION),
    ) && project > cli
    {
        return Some(format!(
            "Project version {} is newer than this Spec Kitty CLI ({}); upgrade the CLI first.",
            metadata.version,
            super::CORE_VERSION
        ));
    }
    None
}

/// Map an upgrade failure to the upstream exit class plus its message.
fn classify_run_error(error: &InstallError) -> (i32, String) {
    match error {
        InstallError::SchemaVersionNewer { found, supported } => (
            EXIT_PROJECT_TOO_NEW,
            format!(
                "This project uses Spec Kitty project schema {found}, but this CLI supports up to schema {supported}. Upgrade your CLI."
            ),
        ),
        InstallError::ProjectVersionNewer { project, cli } => (
            EXIT_PROJECT_TOO_NEW,
            format!(
                "Project version {project} is newer than this Spec Kitty CLI ({cli}); upgrade the CLI first."
            ),
        ),
        InstallError::UpgradeRefused { detail } => {
            if detail.contains("metadata.yaml") {
                (EXIT_PROJECT_CORRUPT, detail.clone())
            } else {
                (1, detail.clone())
            }
        }
        other => (1, other.to_string()),
    }
}

/// The real preview change lines per migration id (Source `upgrade(dry_run=True)`).
///
/// The plan stays the selection and verdict authority (the CF10 contract). This asks
/// each selected migration's OWN dry-run door what it would write, through the same
/// `run_upgrade(dry_run = true)` dispatch the real run uses, so the operator sees the
/// actual change lines instead of only "would apply N". It is read-only: the doors
/// write nothing, record nothing and stamp nothing.
///
/// A preview that cannot run at all leaves the map without that id; the plan's own
/// `failed`/`unimplemented` rows still drive the non-zero exit, so nothing is hidden.
fn preview_rows(request: &UpgradeRequest) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut changes: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let Ok(report) = run_upgrade(&request.clone().dry_run()) else {
        return changes;
    };
    for row in report.rows {
        if !row.changes.is_empty() {
            changes.insert(row.id, row.changes);
        }
    }
    changes
}

/// Print (or emit) the read-only preview.
fn report_plan(
    plan: &UpgradePlan,
    request: &UpgradeRequest,
    json_output: bool,
    verbose: bool,
    previews: &std::collections::BTreeMap<String, Vec<String>>,
) -> Result<(), CompatError> {
    if json_output {
        let payload = plan_json(plan, request);
        emit_json_pretty(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
        return Ok(());
    }
    println!("Current version: {}", plan.from_version);
    println!("Target version:  {}", plan.to_version);
    println!();
    let unimplemented = plan.unimplemented_ids();
    let apply = plan.apply_ids();
    let not_needed = plan.skip_ids();
    for row in &plan.rows {
        if row.outcome == PlannedOutcome::NotNeeded && !verbose {
            continue;
        }
        println!(
            "  {:<44} {:<20} {}",
            row.spec.id,
            row.outcome.as_str(),
            row.detail.clone().unwrap_or_default()
        );
        if let Some(changes) = previews.get(&row.spec.id) {
            for change in changes {
                println!("      {change}");
            }
        }
    }
    println!();
    if !unimplemented.is_empty() {
        println!(
            "✗ {} selected migration(s) have no native implementation; a real run refuses before the first write.",
            unimplemented.len()
        );
        println!("  {}", unimplemented.join(", "));
        println!("Dry run — no changes made");
        return Ok(());
    }
    println!(
        "Would apply {} migration(s); {} already in the target state.",
        apply.len(),
        not_needed.len()
    );
    println!("Dry run — no changes made");
    Ok(())
}

fn plan_json(plan: &UpgradePlan, request: &UpgradeRequest) -> Value {
    let migrations: Vec<Value> = plan
        .rows
        .iter()
        .map(|row| {
            json!({
                "id": row.spec.id,
                "description": row.spec.id,
                "target_version": row.spec.target_version,
                "partition": if row.spec.clone_local { "clone-local" } else { "tracked" },
                "status": planned_status(row),
                "detail": row.detail,
                "manual_review_required": false,
                "preserved_paths": [],
            })
        })
        .collect();
    let mut payload = json!({
        "status": if plan.is_blocked() { "blocked" } else { "dry_run" },
        "outcome": if plan.is_blocked() { "blocked" } else { "dry_run" },
        "failure_reasons": if plan.is_blocked() { vec!["migration_not_implemented"] } else { Vec::<&str>::new() },
        "current_version": plan.from_version,
        "target_version": plan.to_version,
        "dry_run": true,
        "migrations": migrations,
        "migrations_applied": plan.apply_ids(),
        "migrations_skipped": plan.skip_ids(),
        "migration_reports": {},
        "success": !plan.is_blocked(),
        "errors": if plan.is_blocked() {
            vec![format!("no native implementation for: {}", plan.unimplemented_ids().join(", "))]
        } else {
            Vec::new()
        },
        "warnings": [],
        "manual_review_required": false,
        "manual_review_paths": [],
        "auto_committed": false,
        "auto_commit_paths": [],
        "merge_driver_config": Value::Null,
        "surface_repair": Value::Null,
        "requested_target": request.target_version,
    });
    if let Some(map) = payload.as_object_mut() {
        map.insert("plan_contract".to_string(), json!("project-upgrade"));
    }
    payload
}

fn planned_status(row: &MigrationPlanRow) -> &'static str {
    match row.outcome {
        PlannedOutcome::Apply => "pending",
        PlannedOutcome::NotNeeded => "not_needed",
        PlannedOutcome::SkipOutsideGit => "skipped_outside_git",
        PlannedOutcome::NotApplicable => "not_applicable",
        PlannedOutcome::Failed => "failed",
        PlannedOutcome::Unimplemented => "unimplemented",
    }
}

/// The configured agent keys (the canonical config reader).
fn agent_keys(root: &Path) -> Vec<String> {
    sk_install::migrations::completion_modern::support::load_agent_config(root)
        .map(|config| config.available)
        .unwrap_or_default()
}

/// The project's `auto_commit` default.
///
/// This is the canonical `AgentConfig.auto_commit` field
/// (`support::load_agent_config`), i.e. the same projection
/// `core/agent_config.get_auto_commit_default` returns: the `agents` section
/// (then the legacy `tools` section), else the top-level key, else `true`, with
/// a non-boolean value counting as `true`. No second config parser lives here.
fn auto_commit_default(root: &Path) -> Result<bool, InstallError> {
    Ok(sk_install::migrations::completion_modern::support::load_agent_config(root)?.auto_commit)
}

fn report_json(
    report: &sk_install::UpgradeReport,
    request: &UpgradeRequest,
    main_commit: Option<&upgrade_worktrees::CommitOutcome>,
    worktree_failures: &[String],
) -> Value {
    let migrations: Vec<Value> = report
        .rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "status": row.result,
                "changes": row.changes,
                "warnings": row.warnings,
            })
        })
        .collect();
    // Source `UpgradeOutcome.effective_success`: the fatal worktree channel is a
    // real failure, so the machine document never claims success while the exit
    // code is non-zero. The reason vocabulary is Source's `UpgradeFailureReason`.
    let effective_success = report.success && worktree_failures.is_empty();
    let mut failure_reasons: Vec<&str> = Vec::new();
    if !report.success {
        failure_reasons.push("migration_failed");
    }
    if !worktree_failures.is_empty() {
        failure_reasons.push("worktree_failure");
    }
    let mut errors: Vec<String> = report.errors.clone();
    errors.extend(worktree_failures.iter().cloned());
    json!({
        "status": if effective_success { "success" } else { "failed" },
        "outcome": if effective_success { "applied" } else { "failed" },
        "failure_reasons": failure_reasons,
        "current_version": report.from_version,
        "target_version": report.to_version,
        "dry_run": report.dry_run,
        "migrations": migrations,
        "migrations_applied": report.applied(),
        "migrations_skipped": report.skipped(),
        "migration_reports": {},
        "success": effective_success,
        "errors": errors,
        "warnings": report.warnings,
        "manual_review_required": false,
        "manual_review_paths": [],
        "auto_committed": main_commit.is_some_and(|commit| commit.committed),
        "auto_commit_paths": main_commit
            .map(|commit| commit.paths.clone())
            .unwrap_or_default(),
        "auto_commit_commit": main_commit.and_then(|commit| commit.commit.clone()),
        "auto_commit_warning": main_commit.and_then(|commit| commit.reason.clone()),
        "merge_driver_config": Value::Null,
        "surface_repair": Value::Null,
        "requested_target": request.target_version,
    })
}

// ------------------------------------------------- compatibility planner

/// `MIN_SUPPORTED_SCHEMA` / `MAX_SUPPORTED_SCHEMA` (the pin supports schema 3).
const MIN_SUPPORTED_SCHEMA: i64 = 3;
/// The highest project schema this build reads.
const MAX_SUPPORTED_SCHEMA: i64 = 3;
/// The pinned metadata size guard (`CHK020`).
const MAX_METADATA_BYTES: u64 = 262_144;

/// The planner's project-state vocabulary (`ProjectState`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectState {
    /// No `.kittify/` anywhere above the caller.
    NoProject,
    /// A project root without `metadata.yaml`.
    Uninitialized,
    /// Metadata without a `schema_version` field.
    Legacy,
    /// Schema below the supported minimum.
    Stale,
    /// Schema within the supported range.
    Compatible,
    /// Schema above the supported maximum.
    TooNew,
    /// Metadata that cannot be read at all.
    Corrupt,
}

impl ProjectState {
    fn as_str(self) -> &'static str {
        match self {
            Self::NoProject => "no_project",
            Self::Uninitialized => "uninitialized",
            Self::Legacy => "legacy",
            Self::Stale => "stale",
            Self::Compatible => "compatible",
            Self::TooNew => "too_new",
            Self::Corrupt => "corrupt",
        }
    }
}

/// One project-state scan (`compat.planner._scan_project`).
#[derive(Clone, Debug)]
struct ProjectScan {
    state: ProjectState,
    root: Option<PathBuf>,
    schema_version: Option<i64>,
    metadata_error: Option<String>,
    /// `metadata.yaml`'s recorded project version, when readable.
    version: Option<String>,
}

fn scan_project(root: &Path) -> ProjectScan {
    let kittify = root.join(".kittify");
    if !kittify.is_dir() {
        return ProjectScan {
            state: ProjectState::NoProject,
            root: None,
            schema_version: None,
            metadata_error: None,
            version: None,
        };
    }
    let metadata_path = kittify.join("metadata.yaml");
    if !metadata_path.exists() {
        return ProjectScan {
            state: ProjectState::Uninitialized,
            root: Some(root.to_path_buf()),
            schema_version: None,
            metadata_error: None,
            version: None,
        };
    }
    let scan = |state, schema_version, metadata_error, version| ProjectScan {
        state,
        root: Some(root.to_path_buf()),
        schema_version,
        metadata_error,
        version,
    };
    let size = match std::fs::metadata(&metadata_path) {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            return scan(
                ProjectState::Corrupt,
                None,
                Some(format!("stat failed: {error}")),
                None,
            );
        }
    };
    if size > MAX_METADATA_BYTES {
        return scan(
            ProjectState::Corrupt,
            None,
            Some("oversized".to_string()),
            None,
        );
    }
    let text = match std::fs::read_to_string(&metadata_path) {
        Ok(text) => text,
        Err(error) => {
            return scan(
                ProjectState::Corrupt,
                None,
                Some(format!("read error: {error}")),
                None,
            );
        }
    };
    let value: Value = match serde_yaml_ng::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            return scan(
                ProjectState::Corrupt,
                None,
                Some(format!("read error: {error}")),
                None,
            );
        }
    };
    let spec_kitty = value.get("spec_kitty");
    let version = spec_kitty
        .and_then(|block| block.get("version"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(schema_value) = spec_kitty.and_then(|block| block.get("schema_version")) else {
        return scan(ProjectState::Legacy, None, None, version);
    };
    let Some(schema) = schema_value.as_i64() else {
        return scan(
            ProjectState::Corrupt,
            None,
            Some(format!("schema_version out of range: {schema_value}")),
            version,
        );
    };
    if !(0..=1000).contains(&schema) {
        return scan(
            ProjectState::Corrupt,
            None,
            Some(format!("schema_version out of range: {schema}")),
            version,
        );
    }
    let state = if schema < MIN_SUPPORTED_SCHEMA {
        ProjectState::Stale
    } else if schema > MAX_SUPPORTED_SCHEMA {
        ProjectState::TooNew
    } else {
        ProjectState::Compatible
    };
    scan(state, Some(schema), None, version)
}

/// `validate_upgrade_target`: `None` when the target is usable.
fn validate_upgrade_target(current: &str, target: &str) -> Option<String> {
    let requested = match sk_install::Version::parse(target) {
        Ok(version) => version,
        Err(_) => return Some(format!("Invalid upgrade target version: {target}")),
    };
    if current == "unknown" {
        return None;
    }
    let current = match sk_install::Version::parse(current) {
        Ok(version) => version,
        Err(_) => return Some(format!("Invalid project metadata version: {current}")),
    };
    (requested < current)
        .then(|| format!("Refusing to downgrade project metadata from {current} to {requested}"))
}

/// The resolved target: the explicit `--target`, else this CLI's version.
fn resolved_target(args: &UpgradeArgs) -> String {
    args.target
        .clone()
        .unwrap_or_else(|| super::CORE_VERSION.to_string())
}

/// The `pending_migrations` contract rows: `migration_id`,
/// `target_schema_version`, `description`, `files_modified`.
///
/// The set is [`sk_install::select_candidates`] — declared minus applied, capped
/// by the target — so a preview can never under-report what a real run applies.
fn pending_migrations_contract(root: &Path, target: &str) -> Value {
    let rows: Vec<Value> = match sk_install::select_candidates(root, target) {
        Ok(specs) => specs
            .into_iter()
            .map(|spec| {
                let schema = spec
                    .target_version
                    .split('.')
                    .next()
                    .and_then(|major| major.parse::<i64>().ok())
                    .unwrap_or(0);
                let description = sk_install::migrations::completion_legacy::binding(&spec.id)
                    .map(|binding| binding.effect.to_string())
                    .unwrap_or_else(|| spec.id.clone());
                json!({
                    "migration_id": spec.id,
                    "target_schema_version": schema,
                    "description": description,
                    "files_modified": Value::Null,
                })
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    Value::Array(rows)
}

/// The compat-planner envelope (`contracts/compat-planner.json`, schema 1).
///
/// `decision` is the pinned truth table over `(project state, safety)`; the
/// `__project_compat_check__` command path is deliberately unregistered, which
/// is why `safety` is `unsafe` and a stale project reports
/// `BLOCK_PROJECT_MIGRATION` instead of the quietly-allowed `upgrade` path.
fn compatibility_envelope(root: &Path, target: &str, pending: Value) -> (Value, i32) {
    let scan = scan_project(root);
    let safety = "unsafe";
    let (decision, case, exit_code) = match scan.state {
        ProjectState::Corrupt => ("BLOCK_PROJECT_CORRUPT", "project_metadata_corrupt", 6),
        ProjectState::TooNew => ("BLOCK_CLI_UPGRADE", "project_too_new_for_cli", 5),
        ProjectState::Stale | ProjectState::Legacy => {
            ("BLOCK_PROJECT_MIGRATION", "project_migration_needed", 4)
        }
        ProjectState::NoProject | ProjectState::Uninitialized => {
            ("ALLOW", "project_not_initialized", 0)
        }
        ProjectState::Compatible => ("ALLOW", "none", 0),
    };
    // `cli.is_outdated` is always false here: this build makes no network lookup,
    // so the source's `NoNetworkProvider` answer applies. That also settles the
    // nag row of the pinned truth table — with nothing to report as outdated,
    // `ALLOW_WITH_NAG` is unreachable and neither `CI` nor a non-TTY stdout can
    // change the verdict, which is why the invocation's nag flags are not read.
    let install_method = install_method();
    let hint = upgrade_hint();
    let rendered_human = match decision {
        "BLOCK_PROJECT_CORRUPT" => format!(
            "Project metadata could not be read ({})",
            scan.metadata_error.clone().unwrap_or_default()
        ),
        "BLOCK_CLI_UPGRADE" => format!(
            "This project uses Spec Kitty project schema {}, newer than this CLI supports ({MAX_SUPPORTED_SCHEMA}). Upgrade your CLI.",
            scan.schema_version.unwrap_or_default()
        ),
        "BLOCK_PROJECT_MIGRATION" => format!(
            "Project schema {} needs a migration before this command can act. Run 'spec-kitty upgrade'.",
            scan.schema_version
                .map(|schema| schema.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
        _ => match scan.state {
            ProjectState::NoProject | ProjectState::Uninitialized => {
                "Not a Spec Kitty project. Run 'spec-kitty init' to initialize one.".to_string()
            }
            _ => format!(
                "Project schema {} is compatible with this CLI ({})",
                scan.schema_version.unwrap_or_default(),
                super::CORE_VERSION
            ),
        },
    };
    let payload = json!({
        "schema_version": 1,
        "case": case,
        "decision": decision,
        "exit_code": exit_code,
        "cli": {
            "installed_version": super::CORE_VERSION,
            "latest_version": Value::Null,
            "latest_source": "none",
            "is_outdated": false,
            "fetched_at": Value::Null,
        },
        "project": {
            "state": scan.state.as_str(),
            "project_root": scan.root.as_ref().map(|root| root.display().to_string()),
            "schema_version": scan.schema_version,
            "min_supported": MIN_SUPPORTED_SCHEMA,
            "max_supported": MAX_SUPPORTED_SCHEMA,
            "metadata_error": scan.metadata_error,
        },
        "safety": safety,
        "install_method": install_method,
        "upgrade_hint": {
            "install_method": hint.as_ref().map(|(_, _)| install_method.clone()).unwrap_or_else(|| install_method.clone()),
            "command": hint.as_ref().map(|(command, _)| command.clone()),
            "note": hint.as_ref().map(|(_, note)| note.clone()),
        },
        "pending_migrations": pending,
        "rendered_human": rendered_human,
    });
    let _ = target;
    (payload, exit_code)
}

/// `_run_planner_json`: the `--json` + (`--project` | `--dry-run`) contract.
fn planner_json(root: &Path, args: &UpgradeArgs) -> Result<i32, CompatError> {
    let target = resolved_target(args);
    let current = scan_project(root)
        .version
        .unwrap_or_else(|| "unknown".to_string());
    let target_error = validate_upgrade_target(&current, &target);
    let pending = if target_error.is_some() {
        Value::Array(Vec::new())
    } else {
        pending_migrations_contract(root, &target)
    };
    let (mut payload, mut semantic) = compatibility_envelope(root, &target, pending);
    if let Some(reason) = &target_error {
        let stronger = matches!(
            payload["decision"].as_str(),
            Some("BLOCK_PROJECT_CORRUPT" | "BLOCK_CLI_UPGRADE" | "BLOCK_PROJECT_MIGRATION")
        );
        if stronger {
            let existing = payload["rendered_human"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            payload["rendered_human"] = json!(format!("{existing}\n{reason}"));
        } else {
            payload["decision"] = json!("BLOCK_INCOMPATIBLE_FLAGS");
            payload["case"] = json!("none");
            payload["exit_code"] = json!(2);
            payload["rendered_human"] = json!(reason);
            semantic = 2;
        }
    }
    let mut exit = semantic;
    if args.dry_run && semantic != 5 {
        exit = 0;
        if target_error.is_none() && !matches!(semantic, 2 | 5 | 6) {
            // A preview whose selected migrations this build cannot run is
            // incomplete: never report a clean 0 for a run that would refuse.
            let refusal = unimplemented_selection(root, &target);
            if let Some(refusal) = refusal {
                payload["exit_code"] = json!(1);
                let existing = payload["rendered_human"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(1023 - refusal.len().min(1023))
                    .collect::<String>();
                payload["rendered_human"] = json!(format!("{existing}\n{refusal}"));
                exit = 1;
            }
        }
    }
    payload["exit_code"] = json!(exit);
    emit_json_pretty(&payload)
        .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    Ok(exit)
}

/// The selected migrations without a native implementation, as one message.
fn unimplemented_selection(root: &Path, target: &str) -> Option<String> {
    let specs = sk_install::select_candidates(root, target).ok()?;
    let missing: Vec<String> = specs
        .into_iter()
        .filter(|spec| {
            // The canonical registry is the coverage authority: one iterator over
            // the original tranche, the pre-3.x completions and the modern ones.
            sk_install::native_migrations().all(|migration| migration.id != spec.id)
        })
        .map(|spec| spec.id)
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "no native implementation for: {}",
            missing.join(", ")
        ))
    }
}

/// Whether every selected migration can run here (a real run would not refuse).
fn selection_is_complete(root: &Path, target: &str) -> bool {
    unimplemented_selection(root, target).is_none()
}

/// One observed `.kittify/` file effect, in the plan-json `effects` shape.
fn layout_effects(root: &Path, worktrees: &[PathBuf]) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    use sk_install::migrations::completion_legacy::layout::{
        AssetDisposition, classify_asset, execute_migration,
    };
    let mut effects = Vec::new();
    let mut dispositions = Vec::new();
    let mut diagnostics = Vec::new();
    let mut roots = vec![("project".to_string(), root.to_path_buf())];
    for worktree in worktrees {
        let name = worktree
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        roots.push((format!("worktree:{name}"), worktree.clone()));
    }
    let home = sk_install::migrations::completion_legacy::assets::kittify_home();
    let package = sk_install::migrations::completion_legacy::assets::package_asset_root();
    for (root_id, checkout) in &roots {
        let kittify = checkout.join(".kittify");
        if !kittify.is_dir() {
            continue;
        }
        let report = match execute_migration(checkout, true, "software-dev") {
            Ok(report) => report,
            Err(error) => {
                diagnostics.push(json!({
                    "code": "layout_assessment_failed",
                    "owner": root_id,
                    "severity": "error",
                    "message": error.to_string(),
                }));
                continue;
            }
        };
        for path in &report.removed {
            effects.push(json!({
                "id": format!("layout-remove:{}", path.display()),
                "phase": "layout",
                "owner": root_id,
                "logical_owners": [root_id],
                "surface_ids": Vec::<String>::new(),
                "root_id": root_id,
                "path": path.display().to_string(),
                "action": "remove",
                "before": state_json(path),
                "after": json!({"kind": "absent", "sha256": Value::Null, "target": Value::Null, "mode": Value::Null}),
                "reason": "byte-identical to the shipped package default",
                "ownership": {"kind": "canonical_content", "reference": "package default"},
            }));
        }
        for path in &report.superseded {
            effects.push(json!({
                "id": format!("layout-preserve:{}", path.display()),
                "phase": "layout",
                "owner": root_id,
                "logical_owners": [root_id],
                "surface_ids": Vec::<String>::new(),
                "root_id": root_id,
                "path": path.display().to_string(),
                "action": "preserve",
                "before": state_json(path),
                "after": state_json(path),
                "reason": "differs from the package default (customised or outdated)",
                "ownership": {"kind": "unknown", "reference": Value::Null},
            }));
        }
        for (from, to) in &report.moved {
            effects.push(json!({
                "id": format!("layout-move:{}", from.display()),
                "phase": "layout",
                "owner": root_id,
                "logical_owners": [root_id],
                "surface_ids": Vec::<String>::new(),
                "root_id": root_id,
                "path": to.display().to_string(),
                "action": "move",
                "before": state_json(from),
                "after": json!({"kind": "absent", "sha256": Value::Null, "target": Value::Null, "mode": Value::Null}),
                "reason": format!("user-created legacy asset moved to {}", to.display()),
                "ownership": {"kind": "unknown", "reference": Value::Null},
            }));
        }
        for (path, relative) in layout_candidates(checkout) {
            let disposition = classify_asset(
                &path,
                home.as_deref().unwrap_or_else(|| Path::new("")),
                &kittify,
                "software-dev",
                package.as_deref(),
            );
            let state = match disposition {
                AssetDisposition::Identical => "identical",
                AssetDisposition::Superseded => "superseded",
                AssetDisposition::Customized => "customized",
                AssetDisposition::ProjectSpecific => "project_specific",
                AssetDisposition::Unknown => "unknown",
            };
            dispositions.push(json!({
                "owner": root_id,
                "root_id": root_id,
                "path": relative,
                "state": state,
                "reason": "classified against the shipped package default",
            }));
        }
    }
    (effects, dispositions, diagnostics)
}

/// Every `.kittify/` file the layout sweep classifies, with its project-relative
/// POSIX spelling.
fn layout_candidates(checkout: &Path) -> Vec<(PathBuf, String)> {
    let kittify = checkout.join(".kittify");
    if !kittify.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![kittify.clone()];
    while let Some(directory) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for entry in entries {
            let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry);
            } else {
                files.push(entry);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let relative = path
                .strip_prefix(&kittify)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            Some((path, relative))
        })
        .collect()
}

/// `_state_json`: the observed file state of one effect target.
fn state_json(path: &Path) -> Value {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => {
            return json!({"kind": "absent", "sha256": Value::Null, "target": Value::Null, "mode": Value::Null});
        }
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = std::fs::read_link(path)
            .map(|target| target.display().to_string())
            .unwrap_or_default();
        return json!({"kind": "symlink", "sha256": Value::Null, "target": target, "mode": Value::Null});
    }
    if file_type.is_dir() {
        return json!({"kind": "directory", "sha256": Value::Null, "target": Value::Null, "mode": Value::Null});
    }
    let sha256 = std::fs::read(path)
        .ok()
        .map(|bytes| format!("sha256:{}", sk_install::manifest::content_hash(&bytes)));
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(format!("{:o}", metadata.permissions().mode() & 0o7777))
    };
    #[cfg(not(unix))]
    let mode: Option<String> = None;
    json!({"kind": "file", "sha256": sha256, "target": Value::Null, "mode": mode})
}

/// `_emit_blocked_full_plan`: the blocked plan-json envelope.
fn blocked_full_plan(
    root: &Path,
    args: &UpgradeArgs,
    code: i32,
    diagnostic_code: &str,
    message: &str,
) -> Result<i32, CompatError> {
    let target = resolved_target(args);
    let current = scan_project(root)
        .version
        .unwrap_or_else(|| "unknown".to_string());
    let (compatibility, _) = compatibility_envelope(root, &target, Value::Array(Vec::new()));
    let auto_commit = auto_commit_default(root).unwrap_or(false);
    let payload = json!({
        "schema_version": 1,
        "mode": "preview",
        "intent": {
            "project_mode": args.project,
            "requested_target": args.target,
            "include_worktrees": !args.no_worktrees,
            "confirm": false,
        },
        "target": {
            "current": current,
            "requested": target,
            "relation": "unknown",
            "valid": diagnostic_code != "invalid_target",
            "reason": message,
        },
        "compatibility": compatibility,
        "complete": false,
        "decision": "blocked",
        "process_exit_code": code,
        "roots": [{"id": "project", "scope": "project", "path": root.display().to_string()}],
        "migrations": Vec::<Value>::new(),
        "effects": Vec::<Value>::new(),
        "dispositions": Vec::<Value>::new(),
        "diagnostics": [{
            "code": diagnostic_code,
            "owner": Value::Null,
            "severity": "error",
            "message": message,
        }],
        "execution_artifacts": Vec::<Value>::new(),
        "finalizer": Value::Null,
        "commit_policy": {
            "project_enabled": auto_commit,
            "worktrees_enabled": !args.no_worktrees,
            "mission_repair_included": false,
            "auto_commit": auto_commit,
        },
    });
    emit_json_pretty(&payload)
        .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    Ok(code)
}

/// `--plan-json`: the complete preview plan (never replayable).
fn full_plan_json(root: &Path, args: &UpgradeArgs) -> Result<i32, CompatError> {
    let target = resolved_target(args);
    let include_worktrees = !args.no_worktrees;
    let confirm = args.yes || args.force;
    let scan = scan_project(root);
    let current = scan
        .version
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let target_error = validate_upgrade_target(&current, &target);
    let relation = match (
        sk_install::Version::parse(&current),
        sk_install::Version::parse(&target),
    ) {
        (Ok(current), Ok(requested)) if requested < current => "lower",
        (Ok(current), Ok(requested)) if requested == current => "equal",
        (Ok(_), Ok(_)) => "higher",
        _ => "unknown",
    };
    let worktrees: Vec<PathBuf> = if include_worktrees {
        sk_install::migrations::completion_legacy::paths::worktrees(root)
    } else {
        Vec::new()
    };
    // The roots and the commit policy below come from the SAME fan-out plan the
    // real run executes (`plan_worktree_upgrade` shares discovery, scope
    // partition and policy with `run_worktree_upgrade`), so a preview can never
    // describe an execution the run does not perform.
    let auto_commit = auto_commit_default(root).map_err(|error| {
        CompatError::domain(format!("could not read the project config: {error}"))
    })?;
    let mut plan_diagnostics: Vec<Value> = Vec::new();
    // The MAIN run's selection is the fan-out's inherited scope: Source `_upgrade`
    // passes its own `migrations` list to `_upgrade_worktrees`, in catalogue order,
    // captured before any settlement. A worktree never re-selects for itself.
    let inherited: Vec<sk_install::MigrationSpec> = if include_worktrees {
        let request = build_request(root, Some(target.clone()), true);
        match plan_upgrade(&request) {
            Ok(plan) => plan.specs(),
            Err(error) => {
                plan_diagnostics.push(json!({
                    "code": "main_plan_failed",
                    "owner": "project",
                    "severity": "error",
                    "message": error.to_string(),
                }));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let mut worktree_plan = None;
    if include_worktrees {
        match upgrade_worktrees::plan_worktree_upgrade(
            root,
            super::CORE_VERSION,
            Some(&target),
            auto_commit,
            &inherited,
        ) {
            Ok(plan) => worktree_plan = Some(plan),
            Err(error) => plan_diagnostics.push(json!({
                "code": "worktree_plan_failed",
                "owner": "worktrees",
                "severity": "error",
                "message": error.to_string(),
            })),
        }
    }
    let mut roots: Vec<Value> = vec![json!({
        "id": "project",
        "scope": "project",
        "path": root.display().to_string(),
        "disposition": "upgrade",
    })];
    match &worktree_plan {
        Some(plan) => {
            for planned in &plan.roots {
                roots.push(json!({
                    "id": format!("worktree:{}", planned.root.name),
                    "scope": "worktree",
                    "path": planned.root.path.display().to_string(),
                    "disposition": planned.root.disposition.as_str(),
                    "branch": planned.root.branch,
                    "from_version": planned.from_version,
                    "to_version": planned.to_version,
                    "migrations": planned.scope,
                    "project_only_migrations": planned.project_only,
                    "metadata_written": planned.metadata_written,
                }));
            }
        }
        None => {
            for worktree in &worktrees {
                let name = worktree
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                roots.push(json!({
                    "id": format!("worktree:{name}"),
                    "scope": "worktree",
                    "path": worktree.display().to_string(),
                }));
            }
        }
    }
    // The finalizer's read-only repair plan: what the run would restore, report
    // or preserve on the terminal step. `execution_artifacts` stays empty because
    // this build's writer is atomic and stages no temporary artifact (Source's
    // array describes its staging directories).
    let finalizer_keys = agent_keys(root);
    let finalizer = match upgrade_worktrees::plan_surface_repair(root, &finalizer_keys) {
        Ok(actions) => json!({
            "agents": finalizer_keys,
            "repair_plan": actions
                .iter()
                .map(|action| json!({
                    "path": action.path,
                    "action": action.action,
                    "reason": action.reason,
                }))
                .collect::<Vec<Value>>(),
        }),
        Err(error) => {
            plan_diagnostics.push(json!({
                "code": "finalizer_plan_failed",
                "owner": "finalizer",
                "severity": "error",
                "message": error.to_string(),
            }));
            json!({"agents": finalizer_keys, "repair_plan": Vec::<Value>::new()})
        }
    };
    let pending = if target_error.is_some() {
        Value::Array(Vec::new())
    } else {
        pending_migrations_contract(root, &target)
    };
    let (compatibility, compatibility_exit) =
        compatibility_envelope(root, &target, pending.clone());
    let (effects, dispositions, mut diagnostics) = layout_effects(root, &worktrees);
    if let Some(reason) = &target_error {
        diagnostics.push(json!({
            "code": "invalid_target",
            "owner": Value::Null,
            "severity": "error",
            "message": reason,
        }));
    }
    diagnostics.extend(plan_diagnostics);
    let complete = target_error.is_none()
        && selection_is_complete(root, &target)
        && !matches!(compatibility_exit, 5 | 6)
        && !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.get("severity").and_then(Value::as_str) == Some("error"));
    let (decision, exit) = if compatibility_exit == 6 {
        ("blocked", 6)
    } else if compatibility_exit == 5 {
        // A project newer than this CLI cannot be migrated from the project
        // side; the preview says so instead of pretending it could run.
        ("blocked", 5)
    } else if target_error.is_some() {
        ("blocked", 2)
    } else if complete {
        ("ready", 0)
    } else {
        ("incomplete", 1)
    };
    let payload = json!({
        "schema_version": 1,
        "mode": "preview",
        "intent": {
            "project_mode": args.project,
            "requested_target": args.target,
            "include_worktrees": include_worktrees,
            "confirm": confirm,
        },
        "target": {
            "current": current,
            "requested": target,
            "relation": relation,
            "valid": target_error.is_none(),
            "reason": target_error.clone().unwrap_or_else(|| "Target is valid".to_string()),
        },
        "compatibility": compatibility,
        "complete": complete,
        "decision": decision,
        "process_exit_code": exit,
        "roots": roots,
        "migrations": pending,
        "effects": effects,
        "dispositions": dispositions,
        "diagnostics": diagnostics,
        "execution_artifacts": Vec::<Value>::new(),
        "finalizer": finalizer,
        "commit_policy": {
            // Source `should_auto_commit(dry_run=False)`: exactly the project's
            // configured `auto_commit`. The run commits the main checkout churn
            // and each touched non-integrating worktree under this one value.
            "project_enabled": auto_commit,
            "worktrees_enabled": include_worktrees,
            "mission_repair_included": false,
            "auto_commit": auto_commit,
            "worktrees": worktree_plan.as_ref().map(|plan| json!({
                "auto_commit": plan.commit_policy.auto_commit,
                "reason": plan.commit_policy.reason,
            })),
        },
    });
    emit_json_pretty(&payload)
        .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    Ok(exit)
}

/// The user nag cache record, in the source's `NagCacheRecord` wire shape.
#[derive(Clone, Debug, Default)]
struct NagRecord {
    cli_version_key: String,
    latest_version: Option<String>,
    latest_source: String,
    fetched_at: String,
    last_shown_at: Option<String>,
    remote_version_seen: Option<String>,
    snooze_step: Option<String>,
    snoozed_at: Option<String>,
    always_upgrade: bool,
    never_ask: bool,
}

fn nag_cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("spec-kitty").join("upgrade-nag.json"))
}

fn read_nag_cache() -> Option<NagRecord> {
    let path = nag_cache_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let text_field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    Some(NagRecord {
        cli_version_key: text_field("cli_version_key").unwrap_or_default(),
        latest_version: text_field("latest_version"),
        latest_source: text_field("latest_source").unwrap_or_else(|| "none".to_string()),
        fetched_at: text_field("fetched_at").unwrap_or_default(),
        last_shown_at: text_field("last_shown_at"),
        remote_version_seen: text_field("remote_version_seen"),
        snooze_step: text_field("snooze_step"),
        snoozed_at: text_field("snoozed_until"),
        always_upgrade: value
            .get("always_upgrade")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        never_ask: value
            .get("never_ask")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn write_nag_cache(record: &NagRecord) -> Result<(), std::io::Error> {
    let Some(path) = nag_cache_path() else {
        return Err(std::io::Error::other("no user cache directory"));
    };
    let mut payload = Map::new();
    payload.insert("cli_version_key".into(), json!(record.cli_version_key));
    payload.insert("latest_version".into(), json!(record.latest_version));
    payload.insert("latest_source".into(), json!(record.latest_source));
    payload.insert("fetched_at".into(), json!(record.fetched_at));
    payload.insert("last_shown_at".into(), json!(record.last_shown_at));
    payload.insert(
        "remote_version_seen".into(),
        json!(record.remote_version_seen),
    );
    payload.insert("snooze_step".into(), json!(record.snooze_step));
    payload.insert("snoozed_until".into(), json!(record.snoozed_at));
    payload.insert("always_upgrade".into(), json!(record.always_upgrade));
    payload.insert("never_ask".into(), json!(record.never_ask));
    let text =
        serde_json::to_string_pretty(&Value::Object(payload)).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{text}\n"))
}

/// The declared migration count and the implemented subset, for `capabilities`.
///
/// Kept here so the upgrade route reports its own coverage from the same two
/// authorities the run uses: the declared catalog and the canonical native
/// registry iterator ([`sk_install::native_migrations`]).
pub fn migration_coverage() -> (usize, usize) {
    let declared = MigrationCatalog::embedded()
        .map(MigrationCatalog::len)
        .unwrap_or(0);
    (declared, sk_install::native_migrations().count())
}

/// The ledger's recorded ids per partition, for diagnostics.
pub fn ledger_snapshot(root: &Path) -> (Vec<String>, Vec<String>) {
    let ledger = MigrationLedger::new(root);
    let tracked = ledger
        .records(false)
        .map(|records| records.into_iter().map(|record| record.id).collect())
        .unwrap_or_default();
    let clone_local = ledger
        .records(true)
        .map(|records| records.into_iter().map(|record| record.id).collect())
        .unwrap_or_default();
    (tracked, clone_local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_install::{LedgerRecord, MigrationCatalog, MigrationLedger, SETTLED_RESULT};

    fn write(path: &Path, content: impl AsRef<[u8]>) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn project(version: &str, schema: i64) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join(".kittify/config.yaml"),
            "agents:\n  available:\n  - claude\n",
        );
        write(
            &directory.path().join(".kittify/metadata.yaml"),
            format!(
                "spec_kitty:\n  schema_version: {schema}\n  version: {version}\n  initialized_at: '2026-01-01T00:00:00+00:00'\n"
            ),
        );
        directory
    }

    fn seed_except(root: &Path, open: &[&str]) {
        let catalog = MigrationCatalog::embedded().expect("catalog");
        let ledger = MigrationLedger::new(root);
        let mut tracked = Vec::new();
        let mut clone_local = Vec::new();
        for spec in catalog.specs() {
            if open.contains(&spec.id.as_str()) {
                continue;
            }
            let record = LedgerRecord::for_spec(spec, SETTLED_RESULT, super::super::CORE_VERSION);
            if spec.clone_local {
                clone_local.push(record);
            } else {
                tracked.push(record);
            }
        }
        for (path, records) in [
            (ledger.tracked_path(), tracked),
            (ledger.clone_local_path(), clone_local),
        ] {
            if records.is_empty() {
                continue;
            }
            let text: Vec<String> = records.iter().map(LedgerRecord::to_line).collect();
            write(&path, format!("{}\n", text.join("\n")));
        }
    }

    fn args() -> UpgradeArgs {
        UpgradeArgs {
            dry_run: false,
            force: false,
            target: None,
            json: false,
            plan_json: false,
            verbose: false,
            no_worktrees: false,
            cli: false,
            project: false,
            yes: false,
            no_nag: false,
            agent_check: false,
            agent_choice: None,
            agent_latest: None,
        }
    }

    #[test]
    fn cli_and_project_together_exit_two() {
        let directory = project("4.0.0rc8", 3);
        let mut args = args();
        args.cli = true;
        args.project = true;
        assert_eq!(run(directory.path(), &args).unwrap(), 2);
    }

    #[test]
    fn cli_mode_succeeds_outside_a_project() {
        let directory = tempfile::tempdir().unwrap();
        let mut args = args();
        args.cli = true;
        assert_eq!(run(directory.path(), &args).unwrap(), 0);
    }

    #[test]
    fn the_bare_invocation_falls_back_to_cli_guidance_outside_a_project() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(run(directory.path(), &args()).unwrap(), 0);
    }

    #[test]
    fn project_mode_outside_a_project_exits_one() {
        let directory = tempfile::tempdir().unwrap();
        let mut args = args();
        args.project = true;
        assert_eq!(run(directory.path(), &args).unwrap(), 1);
    }

    #[test]
    fn a_project_with_a_newer_schema_exits_five_before_any_write() {
        let directory = project("4.0.0rc8", 99);
        let before =
            std::fs::read_to_string(directory.path().join(".kittify/metadata.yaml")).unwrap();
        assert_eq!(run(directory.path(), &args()).unwrap(), 5);
        let after =
            std::fs::read_to_string(directory.path().join(".kittify/metadata.yaml")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn plan_json_emits_the_full_preview_envelope() {
        let directory = project("4.0.0rc8", 3);
        let root = directory.path();
        seed_except(root, &[]);
        let mut args = args();
        args.plan_json = true;
        assert_eq!(run(root, &args).unwrap(), 0);
        let ledger = MigrationLedger::new(root);
        assert!(ledger.records(false).unwrap().len() > 100);
    }

    #[test]
    fn plan_json_reports_an_unimplemented_selection_as_incomplete() {
        let directory = project("0.1.0", 3);
        let mut args = args();
        args.plan_json = true;
        // Every declared migration is selected and most have no native
        // implementation, so a real run would refuse: the preview must not
        // claim to be ready.
        assert_eq!(run(directory.path(), &args).unwrap(), 1);
    }

    #[test]
    fn plan_json_blocks_a_downgrade_target() {
        let directory = project("4.0.0rc8", 3);
        let mut args = args();
        args.plan_json = true;
        args.target = Some("4.0.0rc7".to_string());
        assert_eq!(run(directory.path(), &args).unwrap(), 2);
    }

    #[test]
    fn json_dry_run_emits_the_compat_planner_contract() {
        let directory = project("4.0.0rc8", 3);
        seed_except(directory.path(), &[]);
        let mut args = args();
        args.json = true;
        args.dry_run = true;
        assert_eq!(run(directory.path(), &args).unwrap(), 0);
    }

    #[test]
    fn a_legacy_project_blocks_the_migration_in_the_contract() {
        let directory = project("4.0.0rc8", 3);
        // A metadata file with no schema_version field is the planner's `legacy`
        // state, which blocks a project-mutating command.
        write(
            &directory.path().join(".kittify/metadata.yaml"),
            "spec_kitty:\n  version: 4.0.0rc8\n",
        );
        let mut args = args();
        args.json = true;
        args.project = true;
        assert_eq!(run(directory.path(), &args).unwrap(), 4);
    }

    #[test]
    fn a_corrupt_metadata_file_is_exit_six() {
        let directory = project("4.0.0rc8", 3);
        write(
            &directory.path().join(".kittify/metadata.yaml"),
            "spec_kitty: [not a mapping\n",
        );
        let mut args = args();
        args.json = true;
        args.project = true;
        assert_eq!(run(directory.path(), &args).unwrap(), 6);
    }

    #[test]
    fn a_preview_that_cannot_be_completed_exits_one_and_writes_nothing() {
        let directory = project("3.2.0", 3);
        let mut args = args();
        args.dry_run = true;
        // No ledger: every declared migration is selected, most of them without a
        // native implementation, so the preview is incomplete.
        assert_eq!(run(directory.path(), &args).unwrap(), 1);
        let metadata =
            std::fs::read_to_string(directory.path().join(".kittify/metadata.yaml")).unwrap();
        assert!(metadata.contains("version: 3.2.0"), "{metadata}");
        assert!(
            !MigrationLedger::new(directory.path())
                .tracked_path()
                .exists()
        );
    }

    #[test]
    fn a_real_run_refuses_an_unimplemented_selection_with_exit_one() {
        let directory = project("3.2.0", 3);
        assert_eq!(run(directory.path(), &args()).unwrap(), 1);
        let metadata =
            std::fs::read_to_string(directory.path().join(".kittify/metadata.yaml")).unwrap();
        assert!(metadata.contains("version: 3.2.0"));
        assert!(
            !MigrationLedger::new(directory.path())
                .tracked_path()
                .exists()
        );
    }

    #[test]
    fn a_real_run_applies_the_selected_retirement_and_stamps_the_version() {
        let directory = project("4.0.0rc5", 3);
        let root = directory.path();
        write(
            &root.join(".claude/commands/spec-kitty.clarify.md"),
            b"<!-- spec-kitty-command-version: 2.0.11 -->\n# clarify\n",
        );
        seed_except(root, &["2.0.11_remove_clarify_command"]);
        assert_eq!(run(root, &args()).unwrap(), 0);
        assert!(!root.join(".claude/commands/spec-kitty.clarify.md").exists());
        let metadata = std::fs::read_to_string(root.join(".kittify/metadata.yaml")).unwrap();
        assert!(metadata.contains("version: 4.0.0rc8"), "{metadata}");
        let records = MigrationLedger::new(root).records(false).unwrap();
        assert!(records.iter().any(|record| {
            record.id == "2.0.11_remove_clarify_command" && record.result == SETTLED_RESULT
        }));
    }

    #[test]
    fn the_agent_check_payload_is_honest_about_an_unknown_latest_version() {
        let directory = tempfile::tempdir().unwrap();
        let mut args = args();
        args.agent_check = true;
        // No network lookup exists, so the payload action is `none` (the source's
        // own offline answer) and no latest version is invented.
        assert_eq!(run(directory.path(), &args).unwrap(), 0);
    }

    #[test]
    fn agent_check_and_agent_choice_are_mutually_exclusive() {
        let directory = tempfile::tempdir().unwrap();
        let mut args = args();
        args.agent_check = true;
        args.agent_choice = Some("never_ask".to_string());
        let error = run(directory.path(), &args).unwrap_err();
        assert_eq!(error.exit(), 2);
    }

    #[test]
    fn an_unknown_agent_choice_exits_two() {
        let directory = tempfile::tempdir().unwrap();
        let mut args = args();
        args.agent_choice = Some("sometimes".to_string());
        args.agent_latest = Some("4.0.0".to_string());
        assert_eq!(run(directory.path(), &args).unwrap(), 2);
    }

    #[test]
    fn a_missing_agent_latest_exits_two() {
        let directory = tempfile::tempdir().unwrap();
        let mut args = args();
        args.agent_choice = Some("never_ask".to_string());
        assert_eq!(run(directory.path(), &args).unwrap(), 2);
    }

    #[test]
    fn the_ledger_snapshot_separates_the_partitions() {
        let directory = project("4.0.0rc8", 3);
        let root = directory.path();
        seed_except(root, &["4.0.0rc8_retire_antigravity_workflows"]);
        let (tracked, clone_local) = ledger_snapshot(root);
        assert!(!tracked.is_empty());
        assert!(
            !clone_local
                .iter()
                .any(|id| id == "4.0.0rc8_retire_antigravity_workflows")
        );
        let catalog = MigrationCatalog::embedded().unwrap();
        assert!(
            clone_local
                .iter()
                .all(|id| { catalog.get(id).is_some_and(|spec| spec.clone_local) })
        );
        assert_eq!(migration_coverage().0, 136);
    }
}
