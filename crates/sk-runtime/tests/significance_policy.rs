//! Significance policy: dimensions, bands, hard triggers and cutoff parsing.

use std::collections::BTreeMap;

use serde_json::json;
use sk_runtime::{
    BandCutoffs, DimensionName, HardTriggerId, MissionPolicySnapshot, RoutingBandName,
    RuntimeError, SignificanceScore, SoftGateAction, SoftGateDecision, default_bands,
    evaluate_significance, parse_band_cutoffs_from_policy, validate_band_cutoffs,
    validate_dimension_scores,
};

fn dims(scores: [i32; 6]) -> BTreeMap<String, i32> {
    DimensionName::ALL
        .iter()
        .zip(scores)
        .map(|(name, score)| (name.as_str().to_string(), score))
        .collect()
}

fn low() -> BTreeMap<String, i32> {
    dims([0, 0, 0, 0, 0, 0])
}

fn cutoffs(pairs: [(&str, [i64; 2]); 3]) -> BTreeMap<String, [i64; 2]> {
    pairs
        .into_iter()
        .map(|(name, pair)| (name.to_string(), pair))
        .collect()
}

#[test]
fn default_bands_cover_0_to_18_contiguously() {
    let bands = default_bands();
    assert_eq!(bands[0].name, RoutingBandName::Low);
    assert_eq!((bands[0].min_score, bands[0].max_score), (0, 6));
    assert_eq!((bands[1].min_score, bands[1].max_score), (7, 11));
    assert_eq!((bands[2].min_score, bands[2].max_score), (12, 18));
}

#[test]
fn dimension_scores_must_be_exactly_the_six_v1_dimensions() {
    let mut missing = low();
    missing.remove("cross_team_blast_radius");
    let error = validate_dimension_scores(&missing).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::InvalidSignificanceInput { .. }
    ));
    assert!(error.to_string().contains("missing"), "{error}");

    let mut extra = low();
    extra.insert("novel_dimension".to_string(), 1);
    let error = validate_dimension_scores(&extra).unwrap_err();
    assert!(error.to_string().contains("unexpected"), "{error}");

    let mut out_of_range = low();
    out_of_range.insert("user_customer_impact".to_string(), 4);
    let error = validate_dimension_scores(&out_of_range).unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid significance input: Dimension 'user_customer_impact' score must be 0-3, got 4"
    );
}

#[test]
fn band_boundaries_are_inclusive_and_use_the_numeric_band() {
    for (scores, expected) in [
        ([3, 3, 0, 0, 0, 0], RoutingBandName::Low),
        ([3, 3, 1, 0, 0, 0], RoutingBandName::Medium),
        ([3, 3, 3, 2, 0, 0], RoutingBandName::Medium),
        ([3, 3, 3, 3, 0, 0], RoutingBandName::High),
        ([3, 3, 3, 3, 3, 3], RoutingBandName::High),
    ] {
        let scores = dims(scores);
        let composite: i32 = scores.values().sum();
        let result = evaluate_significance(&scores, &[], None).unwrap();
        assert_eq!(result.composite, composite as u8);
        assert_eq!(result.effective_band.name, expected);
        assert_eq!(result.band.name, expected);
    }
}

#[test]
fn composite_is_the_sum_and_dimensions_are_sorted_by_wire_name() {
    let scores = dims([1, 2, 3, 0, 0, 0]);
    let result = evaluate_significance(&scores, &[], None).unwrap();
    assert_eq!(result.composite, 6);
    let names: Vec<&str> = result
        .dimensions
        .iter()
        .map(|dimension| dimension.name.as_str())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
}

#[test]
fn any_hard_trigger_forces_the_effective_band_high() {
    let result = evaluate_significance(
        &low(),
        &["security_privacy_access_control".to_string()],
        None,
    )
    .unwrap();
    assert_eq!(result.band.name, RoutingBandName::Low);
    assert_eq!(result.effective_band.name, RoutingBandName::High);
    assert_eq!(result.hard_trigger_classes.len(), 1);
    assert_eq!(
        result.hard_trigger_classes[0].class_id,
        HardTriggerId::SecurityPrivacyAccessControl
    );
    assert_eq!(
        result.hard_trigger_classes[0].description,
        "Security/privacy/access-control changes"
    );
}

#[test]
fn every_fixed_hard_trigger_class_resolves() {
    for class in HardTriggerId::ALL {
        let result = evaluate_significance(&low(), &[class.as_str().to_string()], None).unwrap();
        assert_eq!(result.effective_band.name, RoutingBandName::High);
        assert_eq!(result.hard_trigger_classes[0].class_id, class);
    }
    let error = evaluate_significance(&low(), &["made_up_trigger".to_string()], None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Unknown hard-trigger class: \"made_up_trigger\""),
        "{error}"
    );
}

#[test]
fn custom_cutoffs_can_upgrade_a_medium_composite_to_high() {
    let bands = BandCutoffs::new(&cutoffs([
        ("low", [0, 3]),
        ("medium", [4, 7]),
        ("high", [8, 18]),
    ]))
    .unwrap();
    let scores = dims([3, 2, 0, 3, 0, 0]);
    let result = evaluate_significance(&scores, &[], Some(&bands)).unwrap();
    assert_eq!(result.composite, 8);
    assert_eq!(result.band.name, RoutingBandName::High);
}

#[test]
fn malformed_cutoffs_fail_closed_with_specific_messages() {
    let cases = [
        (
            cutoffs([("low", [0, 6]), ("medium", [7, 11]), ("high", [13, 18])]),
            "Gap between band 'medium' (max=11) and 'high' (min=13)",
        ),
        (
            cutoffs([("low", [0, 6]), ("medium", [6, 11]), ("high", [12, 18])]),
            "Overlap between band 'low' (max=6) and 'medium' (min=6)",
        ),
        (
            cutoffs([("low", [1, 6]), ("medium", [7, 11]), ("high", [12, 18])]),
            "Band 'low' must start at 0, starts at 1",
        ),
        (
            cutoffs([("low", [0, 6]), ("medium", [7, 11]), ("high", [12, 17])]),
            "Band 'high' must end at 18, ends at 17",
        ),
        (
            cutoffs([("low", [6, 0]), ("medium", [7, 11]), ("high", [12, 18])]),
            "Band 'low': min_score (6) > max_score (0)",
        ),
    ];
    for (cutoff, message) in cases {
        let error = validate_band_cutoffs(&cutoff).unwrap_err();
        assert!(matches!(error, RuntimeError::InvalidPolicy { .. }));
        assert!(error.to_string().contains(message), "{error}");
    }

    let two_bands = cutoffs([("low", [0, 8]), ("medium", [9, 18]), ("medium", [0, 0])]);
    let error = validate_band_cutoffs(&two_bands).unwrap_err();
    assert!(error.to_string().contains("Expected exactly 3 bands"));
}

#[test]
fn policy_cutoffs_are_optional_but_malformed_policy_is_typed() {
    let default = MissionPolicySnapshot::default();
    assert!(parse_band_cutoffs_from_policy(&default).unwrap().is_none());

    let mut policy = MissionPolicySnapshot::default();
    policy.extras.insert(
        "significance_band_cutoffs".to_string(),
        json!({"low": [0, 5], "medium": [6, 11], "high": [12, 18]}),
    );
    assert!(parse_band_cutoffs_from_policy(&policy).unwrap().is_some());

    for malformed in [
        json!("nonsense"),
        json!({"low": [0, 5]}),
        json!({"low": [0, 5], "medium": [6, 11], "high": [12]}),
        json!({"low": [0, 5.5], "medium": [6, 11], "high": [12, 18]}),
        json!({"low": [0, 5], "medium": [6, 11], "high": [12, 17]}),
    ] {
        let mut policy = MissionPolicySnapshot::default();
        policy
            .extras
            .insert("significance_band_cutoffs".to_string(), malformed.clone());
        let error = parse_band_cutoffs_from_policy(&policy).unwrap_err();
        assert!(
            matches!(error, RuntimeError::InvalidPolicy { .. }),
            "malformed {malformed} must be a typed policy error"
        );
    }
}

#[test]
fn significance_score_wire_shape_matches_upstream() {
    let result =
        evaluate_significance(&low(), &["architecture_foundation".to_string()], None).unwrap();
    let wire = serde_json::to_value(&result).unwrap();
    assert_eq!(
        wire,
        json!({
            "dimensions": [
                {"name": "architectural_system_impact", "score": 0, "description": ""},
                {"name": "cross_team_blast_radius", "score": 0, "description": ""},
                {"name": "data_security_compliance_impact", "score": 0, "description": ""},
                {"name": "financial_commercial_impact", "score": 0, "description": ""},
                {"name": "operational_reliability_impact", "score": 0, "description": ""},
                {"name": "user_customer_impact", "score": 0, "description": ""}
            ],
            "composite": 0,
            "band": {"name": "low", "min_score": 0, "max_score": 6},
            "hard_trigger_classes": [{
                "class_id": "architecture_foundation",
                "description": "Architecture-foundation changes (language, framework, runtime, datastore, infrastructure)"
            }],
            "effective_band": {"name": "high", "min_score": 12, "max_score": 18}
        })
    );
    let round_trip: SignificanceScore = serde_json::from_value(wire).unwrap();
    assert_eq!(round_trip, result);
}

#[test]
fn soft_gate_action_set_is_closed_and_serializes_snake_case() {
    for action in [
        SoftGateAction::DecideSolo,
        SoftGateAction::OpenStandUp,
        SoftGateAction::Defer,
    ] {
        assert_eq!(SoftGateAction::from_name(action.as_str()), Some(action));
    }
    assert_eq!(SoftGateAction::from_name("approve"), None);

    let score = evaluate_significance(&low(), &[], None).unwrap();
    let gate = SoftGateDecision {
        decision_id: "audit:g".to_string(),
        action: SoftGateAction::DecideSolo,
        actor: sk_runtime::RaciRoleBinding {
            actor_type: sk_runtime::ActorType::Human,
            actor_id: "owner-1".to_string(),
        },
        timestamp: "<TIMESTAMP>".to_string(),
        significance_score: score,
        participants: Vec::new(),
        outcome: Some(SoftGateAction::DecideSolo),
        rationale: None,
    };
    let wire = serde_json::to_value(&gate).unwrap();
    assert_eq!(wire["action"], "decide_solo");
    assert_eq!(wire["outcome"], "decide_solo");
    assert_eq!(wire["participants"], json!([]));
    assert_eq!(wire["rationale"], serde_json::Value::Null);
}
