//! `spec-kitty migrate backfill-runtime-state` — the Source operator command.
//!
//! Source: `src/specify_cli/cli/commands/migrate_cmd.py`
//! (`backfill_runtime_state_cmd`, `app.command(name=_RUNTIME_STATE_CMD)`).
//!
//! The command is deliberately thin. Every load-bearing behaviour already lives
//! in one canonical authority and is reused unchanged:
//!
//! * the seed → fail-closed verify → `status_phase` flip engine is
//!   [`sk_mission::migration_rescue::cutover_mission`] (the same engine the
//!   upgrade migration's `runtime_state_backfill` callback composes);
//! * the two walk policies Source has are
//!   [`cutover_selected_mission`] (the `--mission` arm) and
//!   [`cutover_repo_best_effort`] (the corpus arm, Source's `cutover_repo`);
//! * mission-handle resolution is the CLI's existing selector
//!   ([`resolve_mission`]) — `mission_id` / `mid8` / slug / numeric prefix —
//!   never a raw `kitty-specs/<handle>` join.
//!
//! ## Option surface (Source, preserved)
//!
//! | Option | Meaning |
//! | --- | --- |
//! | `--dry-run` | seed nothing, flip nothing; report would-seed / would-flip counts |
//! | `--mission HANDLE` | scope to one mission; omit to walk the whole corpus |
//! | `--json` | emit the per-mission result list as structured JSON |
//!
//! The group flag `--dry-run` typed before the subcommand is *forwarded* (the
//! subcommand declares it); `--verbose` and `--force` are refused by name.
//!
//! ## Result / exit contract (Source, preserved)
//!
//! * a hard abort (`error` set) is always a failure;
//! * a not-`ok` verify is a failure only on a **live** run — under `--dry-run`
//!   the seeds are not written, so verify is expectedly "not ok" pre-seed and a
//!   healthy legacy corpus must report `0 failed`;
//! * `flipped` / `would_flip` exclude missions that were already at snapshot
//!   authority (the flip's no-write short-circuit), and those are counted as
//!   `already_migrated` instead (#3212);
//! * exit `0` when every visited mission flipped or is already migrated; exit
//!   `1` when any mission failed verify/errored or `--mission` named an unknown
//!   handle.

use std::path::Path;

use serde_json::{Value, json};

use sk_install::migrations::completion_modern::rescue_spine::{
    cutover_repo_best_effort, cutover_selected_mission,
};
use sk_mission::Mission;
use sk_mission::migration_rescue::CutoverOutcome;

use super::super::support::{CompatError, emit_json_pretty, resolve_mission};

/// Source `_RUNTIME_STATE_SUMMARY_TITLE`.
const SUMMARY_TITLE: &str = "backfill-runtime-state summary";
/// Source `_LABEL_FLIPPED`.
const LABEL_FLIPPED: &str = "Flipped";
/// Source `_LABEL_WOULD_FLIP`.
const LABEL_WOULD_FLIP: &str = "Would flip";
/// Source `_LABEL_WOULD_SEED`.
const LABEL_WOULD_SEED: &str = "Would seed (verify pending)";
/// Source `_LABEL_SKIPPED`.
const LABEL_SKIPPED: &str = "Skipped (already migrated)";
/// Source `_LABEL_FAILED`.
const LABEL_FAILED: &str = "Failed";
/// Source's `{label:<27}` summary column width.
const LABEL_WIDTH: usize = 27;
/// Source `_NO_PROJECT_ROOT`.
const NO_PROJECT_ROOT: &str =
    "Could not locate project root. No .kittify/ directory found in any parent directory.";

/// Run `spec-kitty migrate backfill-runtime-state`.
///
/// `dry_run` is the effective value: the subcommand's own `--dry-run` OR the
/// group `--dry-run` forwarded by [`super::run`] (Source's
/// `_forward_or_refuse_group_flags`): a flag given directly after the
/// subcommand still wins, and either one is the same boolean here.
pub fn run(
    root: &Path,
    dry_run: bool,
    mission: Option<&str>,
    json_output: bool,
) -> Result<i32, CompatError> {
    if !root.join(".kittify").is_dir() {
        // Source prints this to stderr and exits 1 even under `--json`.
        eprintln!("Error: {NO_PROJECT_ROOT}");
        return Ok(1);
    }

    let results: Vec<CutoverOutcome> = match mission {
        Some(handle) => {
            let selected = resolve_selected_mission(root, handle, json_output)?;
            let outcome =
                cutover_selected_mission(selected.spec_dir(), dry_run).map_err(|error| {
                    CompatError::coded(
                        "RUNTIME_STATE_CUTOVER_FAILED",
                        format!("runtime-state cutover failed: {error}"),
                    )
                })?;
            vec![outcome]
        }
        None => cutover_repo_best_effort(root, dry_run),
    };

    if json_output {
        emit_json_pretty(&payload(&results, dry_run))
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        print_summary(&results, dry_run);
    }

    // Source: per-mission best-effort — a live run exits non-zero if any mission
    // failed verify / errored; `--dry-run` is a non-mutating preview and never
    // fails on a verify that merely has not seen its seeds yet.
    if !dry_run && results.iter().any(|outcome| failed(outcome, dry_run)) {
        return Ok(1);
    }
    Ok(0)
}

/// Resolve `--mission` through the canonical selector, preserving Source's
/// `resolve_mission_handle` output contract for this command.
///
/// Source prints (human) and exits 2 for an unknown/ambiguous handle, and emits
/// a structured envelope with exit 1 under `--json`. The two failure classes
/// carry different messages and payloads, so they are rendered here rather than
/// leaking the generic compat error shape.
fn resolve_selected_mission(
    root: &Path,
    handle: &str,
    json_output: bool,
) -> Result<Mission, CompatError> {
    match resolve_mission(root, Some(handle)) {
        Ok(mission) => Ok(mission),
        Err(error) => {
            let native = error.to_json();
            match native.get("error_code").and_then(Value::as_str) {
                Some("MISSION_NOT_FOUND") => Err(not_found_error(handle, json_output)),
                Some("MISSION_SELECTOR_AMBIGUOUS") => {
                    Err(ambiguous_error(root, handle, &native, json_output))
                }
                // Any other selector failure is a real domain error already.
                _ => Err(error),
            }
        }
    }
}

/// Source `MissionNotFoundError` rendering (`resolve_mission_handle` human and
/// JSON arms).
fn not_found_error(handle: &str, json_output: bool) -> CompatError {
    let human = format!(
        "No mission found for handle \"{handle}\". Check that the handle is correct and that the mission exists in kitty-specs/."
    );
    let source_message = format!(
        "No mission found for handle \"{handle}\". If this is a legacy mission whose meta.json lacks a mission_id, run `spec-kitty migrate backfill-identity` to backfill it."
    );
    let exit = if json_output { 1 } else { 2 };
    CompatError::envelope_with_exit(
        exit,
        json!({
            "success": false,
            "error_code": "MISSION_NOT_FOUND",
            "error": source_message,
            "handle": handle,
        }),
        human,
    )
}

/// Source `AmbiguousHandleError` rendering.
///
/// The candidate list is the canonical selector's own ambiguity payload; the
/// per-candidate `mission_id` / `feature_dir` Source prints are looked up from
/// the same identity-validated inventory (a display read, not a second
/// selector).
fn ambiguous_error(root: &Path, handle: &str, native: &Value, json_output: bool) -> CompatError {
    let inventory = sk_mission::list_missions(root).unwrap_or_default();
    let candidates: Vec<&Value> = native
        .get("candidates")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().collect())
        .unwrap_or_default();

    let mut lines = vec![
        format!("mission handle \"{handle}\" matches multiple missions."),
        String::new(),
        "Candidates:".to_string(),
    ];
    let mut payload_candidates: Vec<Value> = Vec::new();
    for candidate in &candidates {
        let slug = candidate
            .get("mission_slug")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mid8 = candidate.get("mid8").and_then(Value::as_str);
        let mission = inventory
            .iter()
            .find(|mission| mission.slug().as_str() == slug);
        let mission_id = mission.map(|mission| mission.id().as_str());
        match (mission_id, mid8) {
            (Some(id), Some(mid8)) => {
                lines.push(format!("  {slug} (mission_id {id}, mid8 {mid8})"));
            }
            (None, Some(mid8)) => lines.push(format!("  {slug} (mid8 {mid8})")),
            _ => lines.push(format!("  {slug}")),
        }
        payload_candidates.push(json!({
            "mission_id": mission_id,
            "mid8": mid8,
            "slug": slug,
            "feature_dir": mission
                .map(|mission| mission.spec_dir().display().to_string()),
        }));
    }
    lines.push(String::new());
    lines.push("Re-run with a more specific handle:".to_string());
    for candidate in &candidates {
        let slug = candidate
            .get("mission_slug")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mid8 = candidate.get("mid8").and_then(Value::as_str);
        if let Some(mid8) = mid8 {
            lines.push(format!("  spec-kitty <command> --mission {mid8}"));
        }
        lines.push(format!("  spec-kitty <command> --mission {slug}"));
    }
    lines.push(String::new());
    lines.push("For JSON output of all candidates:".to_string());
    lines.push(format!(
        "  spec-kitty doctor identity --mission {handle} --json"
    ));
    let body = lines.join("\n");

    let exit = if json_output { 1 } else { 2 };
    CompatError::envelope_with_exit(
        exit,
        json!({
            "success": false,
            "error_code": "MISSION_AMBIGUOUS_SELECTOR",
            // Source's `str(exc)` begins with the rendered "Error:" line.
            "error": format!("Error: {body}"),
            "handle": handle,
            "candidates": payload_candidates,
        }),
        body,
    )
}

/// Source `_cutover_failed`.
///
/// A hard abort (`error` set) is always a failure. A not-`ok` verify is a
/// failure only on a live run: under `--dry-run` nothing was seeded, so a
/// not-ok verify is the expected "would seed (verify pending)" state.
fn failed(outcome: &CutoverOutcome, dry_run: bool) -> bool {
    if outcome.error.is_some() {
        return true;
    }
    if dry_run {
        return false;
    }
    outcome.verify.as_ref().is_some_and(|verify| !verify.ok)
}

/// Source `CutoverResult.would_flip`: true only on the dry-run branch, and only
/// when verify passed against the current (already-seeded) state.
fn would_flip(outcome: &CutoverOutcome, dry_run: bool) -> bool {
    dry_run && outcome.verify.as_ref().is_some_and(|verify| verify.ok) && !outcome.already_migrated
}

/// Source `_cutover_detail`.
fn detail(outcome: &CutoverOutcome) -> String {
    if let Some(error) = &outcome.error {
        return error.clone();
    }
    if let Some(verify) = &outcome.verify {
        return verify.mismatches.join("; ");
    }
    "unknown failure".to_string()
}

/// Source `_cutover_payload`: the one structured result shape.
fn payload(results: &[CutoverOutcome], dry_run: bool) -> Value {
    let count = |predicate: &dyn Fn(&CutoverOutcome) -> bool| -> usize {
        results.iter().filter(|outcome| predicate(outcome)).count()
    };
    let entries: Vec<Value> = results
        .iter()
        .map(|outcome| {
            let is_failed = failed(outcome, dry_run);
            json!({
                "slug": outcome.slug,
                "flipped": outcome.flipped && !outcome.already_migrated,
                "already_migrated": outcome.already_migrated,
                "would_flip": would_flip(outcome, dry_run),
                "would_seed": outcome.seeded_count > 0,
                "seeded_count": outcome.seeded_count,
                "verify_ok": outcome.verify.as_ref().map(|verify| verify.ok),
                "failed": is_failed,
                "residue": outcome.residue,
                "mismatches": if is_failed {
                    outcome
                        .verify
                        .as_ref()
                        .map(|verify| verify.mismatches.clone())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                "error": outcome.error,
            })
        })
        .collect();
    json!({
        "dry_run": dry_run,
        "summary": {
            "total": results.len(),
            "flipped": count(&|outcome| outcome.flipped && !outcome.already_migrated),
            "already_migrated": count(&|outcome| outcome.already_migrated),
            "would_seed": count(&|outcome| outcome.seeded_count > 0),
            "would_flip": count(&|outcome| would_flip(outcome, dry_run)),
            "seeded": results.iter().map(|outcome| outcome.seeded_count).sum::<usize>(),
            "failed": count(&|outcome| failed(outcome, dry_run)),
        },
        "results": entries,
    })
}

/// Source `_print_cutover_summary` (plain-text rendering of the same counters).
fn print_summary(results: &[CutoverOutcome], dry_run: bool) {
    let is_failed = |outcome: &CutoverOutcome| failed(outcome, dry_run);
    let failed_rows: Vec<&CutoverOutcome> = results.iter().filter(|o| is_failed(o)).collect();
    let active: Vec<&CutoverOutcome> = results.iter().filter(|o| !is_failed(o)).collect();
    let seeded: usize = results.iter().map(|outcome| outcome.seeded_count).sum();

    let prefix = if dry_run { "(dry-run) " } else { "" };
    println!();
    println!("{prefix}{SUMMARY_TITLE}");
    println!("  Total missions scanned : {}", results.len());
    if dry_run {
        let would_flip_rows = active
            .iter()
            .filter(|outcome| would_flip(outcome, dry_run))
            .count();
        let would_seed_rows = active
            .iter()
            .filter(|outcome| outcome.seeded_count > 0)
            .count();
        let skipped = active
            .iter()
            .filter(|outcome| !would_flip(outcome, dry_run) && outcome.seeded_count == 0)
            .count();
        println!("  {LABEL_WOULD_FLIP:<LABEL_WIDTH$} : {would_flip_rows}");
        println!("  {LABEL_WOULD_SEED:<LABEL_WIDTH$} : {would_seed_rows}");
        println!("  {LABEL_SKIPPED:<LABEL_WIDTH$} : {skipped}");
    } else {
        let migrated = active
            .iter()
            .filter(|outcome| outcome.flipped && !outcome.already_migrated)
            .count();
        let skipped = active
            .iter()
            .filter(|outcome| !(outcome.flipped && !outcome.already_migrated))
            .count();
        println!("  {LABEL_FLIPPED:<LABEL_WIDTH$} : {migrated}");
        println!("  {LABEL_SKIPPED:<LABEL_WIDTH$} : {skipped}");
    }
    println!("  Seed events                 : {seeded}");
    println!("  {LABEL_FAILED:<LABEL_WIDTH$} : {}", failed_rows.len());

    if !failed_rows.is_empty() {
        println!();
        println!("Failed (status_phase left untouched):");
        for outcome in failed_rows {
            println!("  {}: {}", outcome.slug, detail(outcome));
        }
    }

    if dry_run {
        println!();
        println!("Dry run — no seeds or flips written; verify runs post-seed on a live run.");
    }
}

#[cfg(test)]
mod tests;
