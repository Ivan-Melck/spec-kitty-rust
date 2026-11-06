//! `spec-kitty migrate`: the migration/adoption administrative surface.
//!
//! `migrate` is *not* the upgrade registry: `upgrade` runs the declared
//! migration catalog against the ledger, while `migrate <command>` is the
//! operator-invoked backfill/layout surface. This adapter implements the
//! ledger read faithfully and refuses the rest with a typed, named message; it
//! never reports an unimplemented backfill as a success.
//!
//! ## Implemented
//!
//! | Command | Behaviour |
//! | --- | --- |
//! | `migrate ledger [--json]` | read-only two-partition ledger report: declared order, partition, applied, pending, `in_place_here` for clone-local steps, drift, and the derived state token |
//! | `migrate backfill-runtime-state [--dry-run] [--mission HANDLE] [--json]` | seed legacy runtime state as events, fail-closed verify, then flip `status_phase` — per mission best-effort over the corpus, or scoped to one `--mission` |
//!
//! The report is built from the same authorities an upgrade uses: the embedded
//! catalog ([`MigrationCatalog`]), the ledger partition routing
//! ([`MigrationLedger`]) and the native `detect` guard of each implemented
//! migration. Drift is **audit-only** — it is reported, never re-applied — and
//! the command never writes or commits.
//!
//! | `migrate` (no subcommand) | the centralised `.kittify/` layout migration: classify every project file against the immutable package default and remove / preserve / move it into `.kittify/overrides/` |
//! | `migrate --dry-run` | the same assessment with nothing written |
//! | `migrate --verbose` | plus one line per file, naming the action and reason |
//!
//! Group flags are never silently dropped (`#4964`): a flag the invoked
//! subcommand does not declare is refused with a usage error naming the flag,
//! the subcommand and the next action — the pinned `_forward_or_refuse_group_flags`
//! contract. `migrate ledger` declares only `--json`, so `--dry-run`,
//! `--verbose` and `--force` are refused there; `migrate backfill-runtime-state`
//! declares `--dry-run`, so that flag is forwarded and `--verbose` / `--force`
//! are refused.
//!
//! ## Refused (named, never simulated)
//!
//! The corpus-wide modern backfills: `backfill-identity`,
//! `backfill-merge-commit`, `backfill-topology`, `backfill-mission-type`,
//! `charter-encoding`, `backfill-provenance`, `normalize-lifecycle`,
//! `rewrite-opposed-by`, `backfill-wp-status`,
//! `rebaseline-dossier-hashes`, `repin-hooks`, `activation-ledger`. Each is a
//! `target_version` major >= 3 domain backfill owned by the modern migration
//! track; the refusal names the command and the authority it needs.

use std::collections::BTreeSet;
use std::path::Path;

use clap::{Args, Subcommand};
use serde_json::{Value, json};

use sk_install::upgrade::native_migration_detect;
use sk_install::{
    MigrationCatalog, MigrationLedger, MigrationSpec, SETTLED_RESULT, select_candidates,
};

use sk_install::migrations::completion_legacy::layout::ConflictReason;

use super::support::{CompatError, emit_json_pretty};

mod runtime_state;

/// `migrate` arguments: the group flags plus an optional subcommand.
#[derive(Clone, Debug, Args)]
pub struct MigrateArgs {
    /// Show what would change without modifying the filesystem.
    #[arg(long)]
    pub dry_run: bool,
    /// Show file-by-file detail.
    #[arg(long, short = 'v')]
    pub verbose: bool,
    /// Skip the confirmation prompt.
    #[arg(long)]
    pub force: bool,
    /// Migration subcommand.
    #[command(subcommand)]
    pub command: Option<MigrateCommand>,
}

/// `migrate` subcommands.
///
/// A subcommand this build does not implement is captured by the external
/// subcommand arm so the refusal can name it (and echo its arguments) instead of
/// letting clap report an unknown command.
#[derive(Clone, Debug, Subcommand)]
pub enum MigrateCommand {
    /// Report the migration ledger: which setup steps have run, and their state.
    Ledger {
        /// Emit the structured ledger report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Seed legacy runtime state as events, verify fail-closed, and flip `status_phase`.
    BackfillRuntimeState {
        /// Seed nothing and flip nothing; report per-mission would-seed counts and would-flip.
        #[arg(long)]
        dry_run: bool,
        /// Scope to a single mission (mission_id / mid8 / slug). Omit to process the whole corpus.
        #[arg(long, value_name = "HANDLE")]
        mission: Option<String>,
        /// Emit the per-mission cutover result list as structured JSON.
        #[arg(long)]
        json: bool,
    },
    /// Any other subcommand name (refused by name at dispatch).
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// Run `spec-kitty migrate`.
pub fn run(root: &Path, args: &MigrateArgs) -> Result<i32, CompatError> {
    match &args.command {
        Some(MigrateCommand::Ledger { json }) => {
            refuse_group_flags("ledger", &[], args)?;
            ledger(root, *json)
        }
        Some(MigrateCommand::BackfillRuntimeState {
            dry_run,
            mission,
            json,
        }) => {
            // Source's `_forward_or_refuse_group_flags`: `--dry-run` is declared by
            // this subcommand, so a group-level `--dry-run` is forwarded; the flag
            // given after the subcommand still wins (they are the same boolean).
            refuse_group_flags("backfill-runtime-state", &["dry_run"], args)?;
            runtime_state::run(root, *dry_run || args.dry_run, mission.as_deref(), *json)
        }
        Some(MigrateCommand::External(argv)) => {
            let name = argv.first().map(String::as_str).unwrap_or("");
            refuse(name)
        }
        None => layout(root, args),
    }
}

/// Run `spec-kitty migrate` from the current working tree (CLI root wiring entry).
///
/// The upstream command locates the project root by walking upward for
/// `.kittify/`; when there is none the `ledger` surface still reports its own
/// JSON/text envelope and exit 1, so a missing root is not a dispatcher error.
pub fn run_here(args: &MigrateArgs) -> Result<i32, CompatError> {
    let root = super::support::project_root()
        .or_else(|_| std::env::current_dir().map_err(|_| CompatError::domain("no project root")))?;
    run(&root, args)
}

fn refuse(name: &str) -> Result<i32, CompatError> {
    Err(CompatError::unsupported(
        "NOT_IMPLEMENTED",
        format!(
            "`migrate {name}` is an administrative backfill this native build does not implement; \
             it is refused rather than reported as applied. `migrate ledger` stays available."
        ),
    ))
}

/// The group flags as the pinned `_GROUP_FORWARDABLE_PARAMS` names them.
const GROUP_FORWARDABLE: [(&str, &str); 3] = [
    ("dry_run", "--dry-run"),
    ("verbose", "--verbose"),
    ("force", "--force"),
];

/// `_forward_or_refuse_group_flags`: a group flag the subcommand does not
/// declare is a usage error (exit 2), never silently dropped.
///
/// `supported` lists the flag names the subcommand itself declares; a supported
/// flag is forwarded (this build's subcommands read the same `args` fields).
fn refuse_group_flags(
    subcommand: &str,
    supported: &[&str],
    args: &MigrateArgs,
) -> Result<(), CompatError> {
    let given: Vec<(&str, &str)> = GROUP_FORWARDABLE
        .iter()
        .copied()
        .filter(|(name, _)| match *name {
            "dry_run" => args.dry_run,
            "verbose" => args.verbose,
            "force" => args.force,
            _ => false,
        })
        .collect();
    for (name, flag) in given {
        if supported.contains(&name) {
            continue;
        }
        return Err(CompatError::usage(format!(
            "'{flag}' is not supported by 'migrate {subcommand}': that subcommand declares no such option, \
             so it cannot be forwarded. Drop the flag, or run 'spec-kitty migrate {subcommand} --help' \
             to see the options it supports."
        )));
    }
    Ok(())
}

/// The `migrate` (no subcommand) centralised `.kittify/` layout migration.
///
/// Step 1 is the source's global-runtime provisioning (`ensure_runtime`): on a
/// real run the package's managed assets are materialised into `~/.kittify/`
/// (`missions/<type>`, `scripts`, `AGENTS.md`; `config.yaml` and
/// `missions/custom/` preserved) and `cache/version.lock` is stamped, before the
/// per-project `.kittify/` cleanup runs. A dry run reports the check only. When
/// this build has no package assets to provision from, the step says so instead
/// of claiming a runtime it could not write, and classification still uses the
/// **immutable** package defaults.
fn layout(root: &Path, args: &MigrateArgs) -> Result<i32, CompatError> {
    use sk_install::migrations::completion_legacy::assets::kittify_home;
    use sk_install::migrations::completion_legacy::layout::{MigrationReport, execute_migration};
    use sk_install::migrations::completion_modern::global_runtime::ensure_runtime;

    if !sk_install::migrations::completion_legacy::layout::has_kittify(root) {
        let message = if root.join(".kittify").exists() {
            "No .kittify/ directory found in current project."
        } else {
            "Could not locate project root. No .kittify/ directory found in any parent directory."
        };
        eprintln!("{message}");
        return Ok(1);
    }

    if args.dry_run {
        println!("Step 1: Global runtime check (no changes in dry-run)");
    } else if let Some(home) = kittify_home() {
        println!(
            "Step 1: Ensuring global runtime ({}/) is up to date...",
            home.display()
        );
        match ensure_runtime(&home, sk_kernel::COMPAT_CORE_VERSION) {
            Ok(Some(report)) => {
                if report.changed() {
                    println!("  Global runtime is current.");
                } else {
                    println!("  Global runtime is current (no managed asset changed).");
                }
            }
            Ok(None) => println!(
                "  No packaged runtime assets available to provision; shared assets in this project \
                 are classified against the immutable package defaults."
            ),
            Err(error) => {
                return Err(CompatError::domain(format!(
                    "global runtime provisioning failed: {error}"
                )));
            }
        }
    } else {
        println!(
            "  No user home directory resolved; shared assets are classified against the immutable package defaults."
        );
    }

    println!("Step 2: Per-project .kittify/ cleanup...");
    let report: MigrationReport = execute_migration(root, args.dry_run, "software-dev")
        .map_err(|error| CompatError::domain(format!("layout migration failed: {error}")))?;

    if args.dry_run {
        println!("Dry run -- no changes made");
    }
    let (removed, moved, preserved) = if args.dry_run {
        ("would remove", "would move", "would preserve")
    } else {
        ("removed", "moved", "preserved")
    };
    println!(
        "  {} files identical to global -- {removed}",
        report.removed.len()
    );
    if !report.superseded.is_empty() {
        println!(
            "  {} files differ from package defaults (customised or outdated) -- {preserved} in place",
            report.superseded.len()
        );
    }
    println!(
        "  {} files customized -- {moved} to overrides/",
        report.moved.len()
    );
    println!("  {} files project-specific -- kept", report.kept.len());
    if !report.conflicts.is_empty() {
        println!(
            "  {} files NOT moved to overrides/ -- kept in place (review the legacy copies)",
            report.conflicts.len()
        );
    }
    if !report.unknown.is_empty() {
        println!(
            "  {} files unknown -- kept with warning",
            report.unknown.len()
        );
    }
    if args.verbose {
        for path in &report.removed {
            println!("    removed: {}", display(root, path));
        }
        for path in &report.superseded {
            println!(
                "    preserved (differs from package default): {}",
                display(root, path)
            );
        }
        for (source, destination) in &report.moved {
            println!(
                "    moved: {} -> {}",
                display(root, source),
                display(root, destination)
            );
        }
        for conflict in &report.conflicts {
            println!(
                "    not moved ({}): {} -> {}",
                reason_text(conflict.reason),
                display(root, &conflict.source),
                display(root, &conflict.destination)
            );
        }
        for path in &report.kept {
            println!("    kept: {}", display(root, path));
        }
        for path in &report.unknown {
            println!("    unknown: {}", display(root, path));
        }
    }
    if !args.dry_run {
        if report.conflicts.is_empty() {
            println!(
                "Migration complete. Zero legacy warnings expected. Run `spec-kitty config --show-origin` to verify resolution tiers."
            );
        } else {
            println!(
                "Migration complete with {} file(s) not moved. The legacy copies were left in place because an override already exists, a non-folder blocks the overrides path, or the file is a symbolic link; run `spec-kitty migrate --dry-run --verbose` to list them, then delete the ones you no longer need.",
                report.conflicts.len()
            );
        }
    }
    Ok(0)
}

fn reason_text(reason: ConflictReason) -> &'static str {
    reason.text()
}

fn display(root: &Path, path: &Path) -> String {
    sk_install::preserve::relative_display(path, root)
}

/// One ledger row in the report shape.
struct Step {
    id: String,
    identity: String,
    partition: &'static str,
    applied: bool,
    pending: bool,
    in_place_here: Option<bool>,
    drift: Option<bool>,
    result: Option<String>,
    state: &'static str,
}

const STATE_IN_PLACE: &str = "in_place";
const STATE_RECORDED_ELSEWHERE: &str = "recorded_elsewhere";
const STATE_PENDING: &str = "pending";

fn step_state(row: &Step) -> &'static str {
    if !row.applied {
        STATE_PENDING
    } else if row.in_place_here == Some(false) {
        STATE_RECORDED_ELSEWHERE
    } else {
        STATE_IN_PLACE
    }
}

/// Build the report rows: declared order, partition routing, drift and state.
fn assemble(root: &Path) -> Result<(Vec<Step>, BTreeSet<String>), CompatError> {
    let catalog = MigrationCatalog::embedded().map_err(domain)?;
    let ledger = MigrationLedger::new(root);
    let pending: BTreeSet<String> = select_candidates(root, super::CORE_VERSION)
        .map_err(domain)?
        .into_iter()
        .map(|spec| spec.id)
        .collect();

    let mut rows = Vec::with_capacity(catalog.len());
    for spec in catalog.specs() {
        let records = ledger.records_for(spec).map_err(domain)?;
        let matched = records
            .iter()
            .find(|record| record.identity == spec.identity);
        let applied = matched.is_some_and(|record| record.result == SETTLED_RESULT);
        let drift = matched.map(|record| record.content_hash != spec.content_hash);
        let in_place_here = if spec.clone_local {
            Some(!detect_safely(spec, root))
        } else {
            None
        };
        let mut row = Step {
            id: spec.id.clone(),
            identity: spec.identity.clone(),
            partition: if spec.clone_local {
                "clone-local"
            } else {
                "tracked"
            },
            applied,
            pending: pending.contains(&spec.id),
            in_place_here,
            drift,
            result: matched.map(|record| record.result.clone()),
            state: STATE_PENDING,
        };
        row.state = step_state(&row);
        rows.push(row);
    }
    Ok((rows, pending))
}

/// `detect()` for an implemented migration; an evaluator that cannot run reads
/// as "no old state" exactly like the source's `_safe_detect`.
fn detect_safely(spec: &MigrationSpec, root: &Path) -> bool {
    match native_migration_detect(&spec.id, root) {
        Some(Ok(present)) => present,
        Some(Err(_)) | None => false,
    }
}

fn ledger(root: &Path, json_output: bool) -> Result<i32, CompatError> {
    if !root.join(".kittify").is_dir() {
        let message =
            "Could not locate project root. No .kittify/ directory found in any parent directory.";
        if json_output {
            emit_json_pretty(&json!({
                "ok": false,
                "error": {"code": "NO_PROJECT_ROOT", "message": message},
            }))
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
            return Ok(1);
        }
        eprintln!("{message}");
        return Ok(1);
    }

    let (rows, _pending) = assemble(root)?;
    let payload = payload(&rows);
    if json_output {
        emit_json_pretty(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        print_summary(&payload);
    }
    Ok(0)
}

fn payload(rows: &[Step]) -> Value {
    let steps: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "identity": row.identity,
                "partition": row.partition,
                "applied": row.applied,
                "pending": row.pending,
                "in_place_here": row.in_place_here,
                "drift": row.drift.unwrap_or(false),
                "result": row.result,
                "state": row.state,
            })
        })
        .collect();
    json!({
        "summary": {
            "total": steps.len(),
            "applied": rows.iter().filter(|row| row.applied).count(),
            "pending": rows.iter().filter(|row| row.pending).count(),
            "recorded_elsewhere": rows.iter().filter(|row| row.state == STATE_RECORDED_ELSEWHERE).count(),
            "drift": rows.iter().filter(|row| row.drift == Some(true)).count(),
        },
        "steps": steps,
    })
}

fn print_summary(payload: &Value) {
    let summary = &payload["summary"];
    let count = |key: &str| summary[key].as_u64().unwrap_or(0);
    println!();
    println!("Migration ledger");
    println!(
        "  Setup steps recorded            : {} of {}",
        count("applied"),
        count("total")
    );
    println!("  Not yet applied                 : {}", count("pending"));
    println!(
        "  Recorded elsewhere but not here : {}",
        count("recorded_elsewhere")
    );
    println!("  Content changed since it ran    : {}", count("drift"));
    let labels = [
        (STATE_IN_PLACE, "already in place"),
        (STATE_RECORDED_ELSEWHERE, "recorded elsewhere but not here"),
        (STATE_PENDING, "not yet applied"),
    ];
    for step in payload["steps"].as_array().into_iter().flatten() {
        let state = step["state"].as_str().unwrap_or("");
        let label = labels
            .iter()
            .find(|(token, _)| *token == state)
            .map(|(_, label)| *label)
            .unwrap_or(state);
        let drift = if step["drift"].as_bool() == Some(true) {
            " (content changed since it ran)"
        } else {
            ""
        };
        println!(
            "    {} ({}): {label}{drift}",
            step["id"].as_str().unwrap_or(""),
            step["partition"].as_str().unwrap_or("")
        );
    }
}

fn domain(error: sk_install::InstallError) -> CompatError {
    CompatError::coded("LEDGER_UNAVAILABLE", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_install::{LedgerRecord, SETTLED_RESULT};

    fn write(path: &Path, content: impl AsRef<[u8]>) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn project() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        write(
            &directory.path().join(".kittify/config.yaml"),
            "agents:\n  available:\n  - claude\n",
        );
        write(
            &directory.path().join(".kittify/metadata.yaml"),
            "spec_kitty:\n  schema_version: 3\n  version: 4.0.0rc8\n",
        );
        directory
    }

    fn record_success(root: &Path, id: &str) {
        let catalog = MigrationCatalog::embedded().expect("catalog");
        let spec = catalog.get(id).unwrap_or_else(|| panic!("{id} declared"));
        let ledger = MigrationLedger::new(root);
        ledger
            .append(&LedgerRecord::for_spec(spec, SETTLED_RESULT, "4.0.0rc8"))
            .expect("append");
    }

    fn args(command: Option<MigrateCommand>) -> MigrateArgs {
        MigrateArgs {
            dry_run: false,
            verbose: false,
            force: false,
            command,
        }
    }

    #[test]
    fn the_ledger_report_lists_every_declared_step_in_catalog_order() {
        let directory = project();
        let (rows, _) = assemble(directory.path()).expect("assemble");
        let catalog = MigrationCatalog::embedded().expect("catalog");
        assert_eq!(rows.len(), catalog.len());
        for (row, spec) in rows.iter().zip(catalog.specs()) {
            assert_eq!(row.id, spec.id);
            assert_eq!(
                row.partition,
                if spec.clone_local {
                    "clone-local"
                } else {
                    "tracked"
                }
            );
        }
    }

    #[test]
    fn a_recorded_step_is_applied_and_an_unrecorded_step_is_pending() {
        let directory = project();
        let root = directory.path();
        record_success(root, "2.0.11_remove_clarify_command");
        let (rows, pending) = assemble(root).expect("assemble");
        let applied = rows
            .iter()
            .find(|row| row.id == "2.0.11_remove_clarify_command")
            .expect("row");
        assert!(applied.applied);
        assert_eq!(applied.state, STATE_IN_PLACE);
        assert!(!applied.pending);
        let other = rows
            .iter()
            .find(|row| row.id == "0.13.1_exclude_worktrees")
            .expect("row");
        assert!(!other.applied);
        assert_eq!(other.state, STATE_PENDING);
        assert!(pending.contains("0.13.1_exclude_worktrees"));
        assert!(!pending.contains("2.0.11_remove_clarify_command"));
    }

    #[test]
    fn drift_is_reported_from_the_recorded_hash() {
        let directory = project();
        let root = directory.path();
        let catalog = MigrationCatalog::embedded().expect("catalog");
        let spec = catalog.get("2.0.11_remove_clarify_command").expect("spec");
        let mut record = LedgerRecord::for_spec(spec, SETTLED_RESULT, "4.0.0rc8");
        record.content_hash = "sha256:0000".to_string();
        MigrationLedger::new(root).append(&record).expect("append");
        let (rows, _) = assemble(root).expect("assemble");
        let row = rows
            .iter()
            .find(|row| row.id == "2.0.11_remove_clarify_command")
            .expect("row");
        assert_eq!(row.drift, Some(true), "a drifted source is audit-only");
        assert!(row.applied, "drift never un-settles a recorded step");
    }

    #[test]
    fn a_clone_local_step_recorded_without_its_effect_reads_recorded_elsewhere() {
        let directory = project();
        let root = directory.path();
        // 0.13.1's effect lives in `.git/info/exclude`. With a repository whose
        // exclude file lacks the entry, `detect()` reports the old state present,
        // so `in_place_here` is false and the state is "recorded elsewhere".
        write(&root.join(".git/info/exclude"), "# git exclude\n");
        record_success(root, "0.13.1_exclude_worktrees");
        let (rows, _) = assemble(root).expect("assemble");
        let row = rows
            .iter()
            .find(|row| row.id == "0.13.1_exclude_worktrees")
            .expect("row");
        assert_eq!(row.partition, "clone-local");
        assert_eq!(row.in_place_here, Some(false));
        assert_eq!(row.state, STATE_RECORDED_ELSEWHERE);
    }

    #[test]
    fn the_json_payload_carries_the_summary_and_step_shape() {
        let directory = project();
        let root = directory.path();
        record_success(root, "2.0.11_remove_clarify_command");
        let (rows, _) = assemble(root).expect("assemble");
        let payload = payload(&rows);
        assert_eq!(payload["summary"]["total"], rows.len());
        assert_eq!(payload["summary"]["applied"], 1);
        assert!(payload["summary"]["pending"].as_u64().unwrap() > 0);
        let steps = payload["steps"].as_array().expect("steps");
        let step = steps
            .iter()
            .find(|step| step["id"] == "2.0.11_remove_clarify_command")
            .expect("step");
        for key in [
            "id",
            "identity",
            "partition",
            "applied",
            "pending",
            "in_place_here",
            "drift",
            "result",
            "state",
        ] {
            assert!(step.get(key).is_some(), "step carries {key}");
        }
    }

    #[test]
    fn the_ledger_command_is_read_only() {
        let directory = project();
        let root = directory.path();
        let before = std::fs::read_to_string(root.join(".kittify/config.yaml")).unwrap();
        let code = run(root, &args(Some(MigrateCommand::Ledger { json: false }))).expect("ledger");
        assert_eq!(code, 0);
        let after = std::fs::read_to_string(root.join(".kittify/config.yaml")).unwrap();
        assert_eq!(before, after);
        assert!(!MigrationLedger::new(root).tracked_path().exists());
    }

    #[test]
    fn the_ledger_command_outside_a_project_exits_one() {
        let directory = tempfile::tempdir().unwrap();
        let code = run(
            directory.path(),
            &args(Some(MigrateCommand::Ledger { json: true })),
        )
        .expect("ledger");
        assert_eq!(code, 1);
    }

    #[test]
    fn the_modern_backfills_are_refused_by_name() {
        let directory = project();
        let refused = run(
            directory.path(),
            &args(Some(MigrateCommand::External(vec![
                "backfill-identity".to_string(),
            ]))),
        )
        .expect_err("refused");
        assert_eq!(refused.exit(), 2);
        assert!(refused.message().contains("backfill-identity"));
    }

    #[test]
    fn the_bare_command_migrates_user_created_assets_into_overrides() {
        let directory = project();
        let root = directory.path();
        write(&root.join(".kittify/templates/mine.md"), "# mine\n");
        write(&root.join(".kittify/config.yaml"), "tools: {}\n");
        let code = run(root, &args(None)).expect("migrate");
        assert_eq!(code, 0);
        assert!(root.join(".kittify/overrides/templates/mine.md").is_file());
        assert!(!root.join(".kittify/templates/mine.md").exists());
        assert!(root.join(".kittify/config.yaml").is_file());
    }

    #[test]
    fn the_dry_run_reports_without_writing() {
        let directory = project();
        let root = directory.path();
        write(&root.join(".kittify/templates/mine.md"), "# mine\n");
        let mut args = args(None);
        args.dry_run = true;
        assert_eq!(run(root, &args).expect("migrate"), 0);
        assert!(root.join(".kittify/templates/mine.md").is_file());
        assert!(!root.join(".kittify/overrides").exists());
    }

    #[test]
    fn a_group_flag_the_subcommand_does_not_declare_is_refused() {
        let directory = project();
        let mut args = args(Some(MigrateCommand::Ledger { json: true }));
        args.dry_run = true;
        let error = run(directory.path(), &args).expect_err("refused");
        assert_eq!(error.exit(), 2);
        assert!(error.message().contains("--dry-run"), "{}", error.message());
        assert!(
            error.message().contains("migrate ledger"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn the_bare_command_outside_a_project_exits_one() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(run(directory.path(), &args(None)).expect("migrate"), 1);
    }
}
