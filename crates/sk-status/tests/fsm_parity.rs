//! Whole-matrix parity against the committed Python FSM baseline.
//!
//! The baseline is the 2057-row golden fixture captured from the pre-genesis
//! Python behaviour. Genesis rows are intentionally excluded from the parity
//! envelope by the Python suite (they recorded `Unknown lane: genesis`, which
//! the first-class genesis lane supersedes); they are governed by the
//! dedicated genesis assertions in `fsm_semantics.rs` instead.

mod support;

use sk_status::{GuardContext, Lane, validate_transition};

const BASELINE: &str = include_str!("../../../tests/status/fsm_parity_baseline.jsonl");

#[derive(Debug, Clone)]
struct Row {
    from: String,
    to: String,
    context: String,
    expected_ok: bool,
    expected_err: Option<String>,
}

fn load_rows() -> Vec<Row> {
    BASELINE
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("baseline line must be JSON");
            let array = value.as_array().expect("baseline row must be a JSON array");
            assert_eq!(array.len(), 5, "baseline row arity");
            Row {
                from: array[0].as_str().expect("from lane").to_owned(),
                to: array[1].as_str().expect("to lane").to_owned(),
                context: array[2].as_str().expect("context name").to_owned(),
                expected_ok: array[3].as_bool().expect("expected ok"),
                expected_err: array[4].as_str().map(str::to_owned),
            }
        })
        .collect()
}

fn is_genesis_row(row: &Row) -> bool {
    row.from == "genesis" || row.to == "genesis"
}

#[test]
fn baseline_fixture_has_exact_expected_shape_and_counts() {
    let rows = load_rows();

    // The committed fixture, not a regenerated or sampled approximation.
    assert_eq!(rows.len(), 2057, "total baseline rows");
    let genesis = rows.iter().filter(|row| is_genesis_row(row)).count();
    let checked = rows.len() - genesis;
    assert_eq!(genesis, 357, "pre-genesis rows excluded from parity");
    assert_eq!(checked, 1700, "nine-lane parity envelope rows");

    let accepts = rows
        .iter()
        .filter(|row| !is_genesis_row(row))
        .filter(|row| row.expected_ok)
        .count();
    let rejects = checked - accepts;
    assert_eq!(accepts, 351, "accepted envelope rows");
    assert_eq!(rejects, 1349, "rejected envelope rows");
    assert!(accepts > 0 && rejects > 0, "both branches exercised");

    // All 17 context variants and the `doing` alias are really present.
    let mut contexts: Vec<&str> = rows.iter().map(|row| row.context.as_str()).collect();
    contexts.sort_unstable();
    contexts.dedup();
    assert_eq!(contexts.len(), 17, "17 named context variants");
    for name in support::CONTEXT_NAMES {
        assert!(
            contexts.contains(&name),
            "context {name:?} is missing from the baseline"
        );
    }

    let alias_rows = rows
        .iter()
        .filter(|row| !is_genesis_row(row))
        .filter(|row| row.from == "doing" || row.to == "doing")
        .count();
    assert_eq!(alias_rows, 323, "doing-alias rows in the parity envelope");
}

#[test]
fn every_non_genesis_baseline_row_matches_exactly() {
    let rows = load_rows();
    let mut mismatches = Vec::new();
    let mut checked = 0usize;

    for row in rows.iter().filter(|row| !is_genesis_row(row)) {
        checked += 1;
        let ctx = support::context(&row.context);
        // `doing` is an input alias only; the canonical FSM never sees it.
        let from = Lane::parse_input(&row.from)
            .unwrap_or_else(|error| panic!("baseline from lane {:?}: {error}", row.from));
        let to = Lane::parse_input(&row.to)
            .unwrap_or_else(|error| panic!("baseline to lane {:?}: {error}", row.to));

        let typed = validate_transition(from, to, &ctx);
        let named = sk_status::validate_transition_names(&row.from, &row.to, &ctx);
        if typed != named {
            mismatches.push(format!(
                "{}->{}[{}]: typed path {typed:?} disagreed with name/alias path {named:?}",
                row.from, row.to, row.context
            ));
            continue;
        }

        let (actual_ok, actual_err) = match typed {
            Ok(()) => (true, None),
            Err(error) => (false, Some(error.to_string())),
        };

        if actual_ok != row.expected_ok || actual_err != row.expected_err {
            mismatches.push(format!(
                "{}->{}[{}]: expected ok={} error={:?}, got ok={} error={:?}",
                row.from,
                row.to,
                row.context,
                row.expected_ok,
                row.expected_err,
                actual_ok,
                actual_err
            ));
        }
    }

    assert_eq!(
        checked, 1700,
        "the parity loop must cover the whole envelope"
    );
    assert!(
        mismatches.is_empty(),
        "validate_transition diverged on {} row(s):\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

#[test]
fn alias_input_validation_matches_canonical_validation() {
    let ctx = support::context("force_full");
    // The by-name entry point resolves `doing` before the typed decision.
    let via_alias = sk_status::validate_transition_names("doing", "planned", &ctx);
    let via_canonical = validate_transition(Lane::InProgress, Lane::Planned, &ctx);
    assert_eq!(via_alias, via_canonical);

    let alias_reject = sk_status::validate_transition_names("for_review", "doing", &ctx)
        .expect_err("forced rewind still needs a review_ref");
    assert_eq!(
        alias_reject.to_string(),
        "Transition for_review -> in_progress requires review_ref: a forced rewind out of review \
         must cite the review feedback it is rewinding (pass `--review-ref <ref>`)."
    );
}

#[test]
fn guard_context_is_the_documented_typed_surface() {
    // Keep the fixture builder honest about every field the FSM consults.
    let ctx: GuardContext = support::context("empty");
    assert!(!ctx.force);
    assert!(ctx.actor.is_none());
    assert!(ctx.review_result.is_none());
}
