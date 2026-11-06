//! `charter activate --preset` — the thin adapter over the canonical preset
//! planner/writer.
//!
//! Mirrors the reference's `_activate_preset` (`src/specify_cli/cli/commands/
//! charter/activate.py`): the write root is resolved first (a linked worktree
//! fails closed), then one pure plan ([`plan_preset_application`]) and one
//! write through the existing activation writer ([`apply_preset_plan`]), then
//! the same post-change notice the positional path finishes with.
//!
//! This module owns rendering only: no preset parsing, no identity lookup, no
//! activation write. `--resynthesize` stays an explicit pre-mutation refusal
//! because the native charter synthesizer is not ported yet; it is never
//! silently skipped.

use std::path::Path;

use serde_json::{Map, Value, json};

use sk_doctrine::preset_application::{
    PresetApplicationError, PresetOutcome, PresetPlan, apply_preset_plan, plan_preset_application,
};
use sk_doctrine::{DEFAULT_LOCK_TIMEOUT, DoctrineError, acquire_synthesis_lock};

use crate::compat::support::{CompatError, emit_json_pretty};

/// The reference's two run-wide lock codes (`charter activation_write_window`).
const SYNTHESIS_LOCK_HELD: &str = "CHARTER_SYNTHESIS_LOCK_HELD";
/// The lock exists but git could not be invoked to place it.
const SYNTHESIS_LOCK_UNAVAILABLE: &str = "CHARTER_SYNTHESIS_LOCK_UNAVAILABLE";

/// Activate `preset` of `pack` for the project at `root`.
pub(super) fn activate_preset_command(
    root: &Path,
    pack: &str,
    preset: &str,
    force: bool,
    json_output: bool,
    no_compile: bool,
) -> Result<u8, CompatError> {
    // The reference holds the run-wide charter synthesis lock across the whole
    // read-plan-write (`activation_write_window` in `activate.py`), so a
    // `charter synthesize` run cannot append between our read and our write.
    // A lock that cannot be taken writes nothing and refuses with its own code.
    let window = match acquire_synthesis_lock(root, DEFAULT_LOCK_TIMEOUT) {
        Ok(lock) => lock,
        Err(error) => return Ok(render_lock_refusal(&error, json_output)),
    };
    let result = plan_preset_application(root, pack, preset)
        .and_then(|plan| apply_preset_plan(root, &plan, force).map(|outcome| (plan, outcome)));
    drop(window);
    let (plan, outcome) = match result {
        Ok(result) => result,
        Err(error) => return Ok(render_error(&error, json_output)),
    };
    // The activation change is already saved, so a re-stamp the lock kept out is
    // one warning line on stderr, never a refusal (the reference's own posture).
    if let Some(warning) = outcome.restamp_warning() {
        eprintln!("{warning}");
    }
    // A preset never governs a pack skill, so only the catalog notice applies.
    let notices = super::post_change_notices(root, "", no_compile);
    if json_output {
        let _ = emit_json_pretty(&preset_payload(&plan, &outcome));
        return Ok(0);
    }
    render_preset_plan(&plan, &outcome);
    for notice in &notices {
        println!("{notice}");
    }
    Ok(0)
}

/// Render the reference's coded refusal for a run-wide lock that could not be taken.
fn render_lock_refusal(error: &DoctrineError, json_output: bool) -> u8 {
    let refusal = match error {
        DoctrineError::SynthesisLockHeld { .. } => {
            super::Refusal::new(SYNTHESIS_LOCK_HELD, error.to_string())
        }
        DoctrineError::SynthesisLockUnavailable { .. } => {
            super::Refusal::new(SYNTHESIS_LOCK_UNAVAILABLE, error.to_string())
        }
        other => super::Refusal::new(super::ACTIVATION_FAILED, other.to_string()),
    };
    super::render_refusal(&refusal, json_output)
}

/// The `--json` shape of an applied preset: the exclusion fields appear only
/// when they hold something.
fn preset_payload(plan: &PresetPlan, outcome: &PresetOutcome) -> Value {
    let mut payload = Map::new();
    payload.insert("pack".to_owned(), json!(plan.pack));
    payload.insert("preset".to_owned(), json!(plan.preset));
    payload.insert(
        "written".to_owned(),
        json!(
            outcome
                .written
                .iter()
                .map(|(key, ids)| (key.clone(), json!(ids)))
                .collect::<Map<String, Value>>()
        ),
    );
    payload.insert("removed".to_owned(), json!(plan.removed));
    payload.insert(
        "target_file".to_owned(),
        json!(plan.target_file.display().to_string()),
    );
    if !outcome.kept_deactivated.is_empty() {
        payload.insert(
            "kept_deactivated".to_owned(),
            json!(
                outcome
                    .kept_deactivated
                    .iter()
                    .map(|(key, ids)| (key.clone(), json!(ids)))
                    .collect::<Map<String, Value>>()
            ),
        );
    }
    if !outcome.cleared.is_empty() {
        payload.insert(
            "cleared_exclusions".to_owned(),
            json!(
                outcome
                    .cleared
                    .iter()
                    .map(|(key, ids)| (key.clone(), json!(ids)))
                    .collect::<Map<String, Value>>()
            ),
        );
    }
    Value::Object(payload)
}

/// Print what applying the preset did, in the reference's order.
fn render_preset_plan(plan: &PresetPlan, outcome: &PresetOutcome) {
    if plan.is_noop() && outcome.cleared.is_empty() {
        println!(
            "Preset '{}' of pack '{}' is already in force in {}; nothing changed.",
            plan.preset,
            plan.pack,
            plan.target_file.display()
        );
        render_exclusions(outcome);
        return;
    }
    println!(
        "Applied preset '{}' of pack '{}' to {}",
        plan.preset,
        plan.pack,
        plan.target_file.display()
    );
    for (key, ids) in &outcome.written {
        let listed = if ids.is_empty() {
            "(none)".to_owned()
        } else {
            ids.join(", ")
        };
        println!("  Wrote {key}: {listed}");
    }
    for key in &plan.removed {
        println!("  Removed {key} (unrestricted)");
    }
    render_exclusions(outcome);
}

/// Name the listed ids a preset left deactivated, or whose exclusion `--force`
/// cleared (FR-009, FR-014).
fn render_exclusions(outcome: &PresetOutcome) {
    for (key, ids) in &outcome.kept_deactivated {
        println!(
            "  Kept deactivated: {key}: {} (listed by the preset; left out; --force clears the exclusion)",
            ids.join(", ")
        );
    }
    for (key, ids) in &outcome.cleared {
        println!("  Cleared exclusion: {key}: {}", ids.join(", "));
    }
}

/// Render one coded refusal exactly once and exit 1.
fn render_error(error: &PresetApplicationError, json_output: bool) -> u8 {
    let details = error.detail_lines();
    if json_output {
        let mut body = Map::new();
        body.insert("result".to_owned(), json!("error"));
        body.insert("success".to_owned(), json!(false));
        body.insert("code".to_owned(), json!(error.code()));
        body.insert("error".to_owned(), json!(error.to_string()));
        if let Some(pack) = error.pack() {
            body.insert("pack".to_owned(), json!(pack));
        }
        if let Some(preset) = error.preset() {
            body.insert("preset".to_owned(), json!(preset));
        }
        if !error.available().is_empty() {
            body.insert("available".to_owned(), json!(error.available()));
        }
        if let Some(preset_file) = error.preset_file() {
            body.insert(
                "preset_file".to_owned(),
                json!(preset_file.display().to_string()),
            );
        }
        if let Some(unresolved) = error.unresolved()
            && !unresolved.is_empty()
        {
            body.insert(
                "unresolved".to_owned(),
                json!(
                    unresolved
                        .iter()
                        .map(|(key, ids)| (key.clone(), json!(ids)))
                        .collect::<Map<String, Value>>()
                ),
            );
        }
        if let Some(reasons) = error.reasons()
            && !reasons.is_empty()
        {
            body.insert("reasons".to_owned(), json!(reasons));
        }
        if !error.customised_changes().is_empty() {
            body.insert("customised".to_owned(), json!(error.customised_changes()));
        }
        if let Some(plan) = error.plan() {
            body.insert(
                "diff".to_owned(),
                json!(
                    plan.changes
                        .iter()
                        .map(|change| (
                            change.key.clone(),
                            json!({"before": change.before, "after": change.after})
                        ))
                        .collect::<Map<String, Value>>()
                ),
            );
        }
        if !details.is_empty() {
            body.insert("details".to_owned(), json!(details));
        }
        let _ = emit_json_pretty(&Value::Object(body));
    } else {
        eprintln!("Error ({}): {error}", error.code());
        for line in &details {
            eprintln!("{line}");
        }
    }
    1
}
