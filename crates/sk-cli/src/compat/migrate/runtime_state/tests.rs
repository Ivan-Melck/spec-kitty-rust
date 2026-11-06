//! Focused tests for the `migrate backfill-runtime-state` adapter surface.
//!
//! These cover the command's OWN contract — the Source dry-run-aware failure
//! predicate, the would-flip/ already-migrated result shape, and the
//! `--mission` refusal envelopes. The end-to-end seed/verify/flip behaviour is
//! exercised against a real temporary repository in
//! `crates/sk-cli/tests/cf25_runtime_cutover_command.rs`.

use super::*;
use sk_mission::migration_rescue::CutoverOutcome;
use sk_status::migration_backfill::RuntimeVerifyOutcome;

fn outcome(seeded_count: usize, flipped: bool) -> CutoverOutcome {
    CutoverOutcome {
        slug: "mission-a".to_string(),
        flipped,
        residue: false,
        already_migrated: false,
        seeded_count,
        verify: None,
        error: None,
        skipped_notice: None,
    }
}

fn verified(mut base: CutoverOutcome, ok: bool) -> CutoverOutcome {
    base.verify = Some(RuntimeVerifyOutcome {
        ok,
        wp_count: 1,
        mismatches: if ok {
            Vec::new()
        } else {
            vec!["WP01: claim mismatch (deterministic seed missing)".to_string()]
        },
    });
    base
}

#[test]
fn a_hard_abort_is_a_failure_in_both_modes() {
    let mut aborted = outcome(0, false);
    aborted.error = Some("event log unreadable: boom".to_string());
    assert!(failed(&aborted, false));
    assert!(failed(&aborted, true));
    assert_eq!(detail(&aborted), "event log unreadable: boom");
}

#[test]
fn a_not_ok_verify_fails_only_on_a_live_run() {
    let not_ok = verified(outcome(0, false), false);
    assert!(
        !failed(&not_ok, true),
        "pre-seed verify is not a dry-run failure"
    );
    assert!(failed(&not_ok, false));
    assert_eq!(
        detail(&not_ok),
        "WP01: claim mismatch (deterministic seed missing)"
    );
}

#[test]
fn would_flip_is_the_dry_run_signal_only() {
    let ok = verified(outcome(0, false), true);
    assert!(would_flip(&ok, true));
    assert!(
        !would_flip(&ok, false),
        "a live run reports `flipped`, not would_flip"
    );

    let not_ok = verified(outcome(0, false), false);
    assert!(!would_flip(&not_ok, true));
}

#[test]
fn the_payload_counts_flips_apart_from_already_migrated_missions() {
    let mut migrated = verified(outcome(0, true), true);
    migrated.already_migrated = true;
    let flipped = verified(outcome(3, true), true);
    let dry = verified(outcome(2, false), true);
    let results = vec![migrated, flipped, dry];

    let live = payload(&results, false);
    assert_eq!(live["dry_run"], json!(false));
    assert_eq!(live["summary"]["total"], 3);
    // A mission the flip short-circuited on is counted, never reported as a flip.
    assert_eq!(live["summary"]["already_migrated"], 1);
    assert_eq!(live["summary"]["flipped"], 1);
    assert_eq!(live["summary"]["would_flip"], 0);
    assert_eq!(live["summary"]["would_seed"], 2);
    assert_eq!(live["summary"]["seeded"], 5);
    assert_eq!(live["summary"]["failed"], 0);
    let first = &live["results"][0];
    assert_eq!(first["flipped"], json!(false));
    assert_eq!(first["already_migrated"], json!(true));
    for key in [
        "slug",
        "flipped",
        "already_migrated",
        "would_flip",
        "would_seed",
        "seeded_count",
        "verify_ok",
        "failed",
        "residue",
        "mismatches",
        "error",
    ] {
        assert!(first.get(key).is_some(), "result carries {key}");
    }

    let preview = payload(&results, true);
    assert_eq!(preview["dry_run"], json!(true));
    // The two verify-ok missions that are not already at snapshot authority would
    // be flipped by a live run; the already-migrated one is excluded.
    assert_eq!(preview["summary"]["would_flip"], 2);
    assert_eq!(preview["summary"]["already_migrated"], 1);
}

#[test]
fn a_failed_result_carries_its_mismatches_and_others_do_not() {
    let broken = verified(outcome(1, false), false);
    let healthy = verified(outcome(1, true), true);
    let live = payload(&[broken, healthy], false);
    assert_eq!(live["results"][0]["failed"], json!(true));
    assert_eq!(
        live["results"][0]["mismatches"],
        json!(["WP01: claim mismatch (deterministic seed missing)"])
    );
    assert_eq!(live["results"][1]["failed"], json!(false));
    assert_eq!(live["results"][1]["mismatches"], json!([]));
}

#[test]
fn an_unknown_handle_refuses_with_the_source_message_and_exit() {
    let error = not_found_error("nope", false);
    assert_eq!(error.exit(), 2, "human mode exits 2");
    assert_eq!(
        error.message(),
        "No mission found for handle \"nope\". Check that the handle is correct and that the mission exists in kitty-specs/."
    );
    let json_error = not_found_error("nope", true);
    assert_eq!(json_error.exit(), 1, "json mode exits 1");
    let payload = json_error.to_json();
    assert_eq!(payload["success"], json!(false));
    assert_eq!(payload["error_code"], json!("MISSION_NOT_FOUND"));
    assert_eq!(payload["handle"], json!("nope"));
    let message = payload["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("backfill-identity"),
        "Source names the remediation: {message}"
    );
    assert!(
        message.contains("meta.json lacks a mission_id"),
        "{message}"
    );
}
