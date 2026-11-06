//! Source-first conformance for the advisory offer seam and glossary wire.
//!
//! Two configured surfaces the default dispatch composition never exercises:
//!
//! 1. `charter.offering.model_task_routing.evaluator.evaluate`'s optional
//!    `recommended_model_tier` / `model_tier_override` — resolved through the one
//!    override-wins seam. `model_tier` is a nested dataclass
//!    (`effective`, `source`, `offer`) rendered into the advisory payload, not a
//!    sorted `serde_json` map.
//! 2. A glossary scan that actually reports conflicts:
//!    `semantic_conflict_to_dict` is a plain Python dict whose key order
//!    (`term`, `conflict_type`, `severity`, `confidence`, `candidate_senses`,
//!    `context`) is part of the bytes. The dispatch payload previously projected
//!    it through `serde_json::Map`, which sorts keys and loses that order.
//!
//! The pinned reference runs first in an asserted isolated root and prints the
//! exact `json.dumps` bytes; the native standalone APIs must render the same
//! bytes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod support;

use serde_json::{Value, json};
use sk_invocation::executor::{
    ConflictCandidateView, GlossaryObservation, SemanticConflictView, glossary_json,
    recommendation_json,
};
use sk_invocation::executor::{
    CostTier, LatencyTier, ModelCost, ModelEntry, ModelRoutingCatalog, ModelTierOffer,
    OverridePolicy, ProfileRoutingView, RoutingObjective, RoutingPolicy, RoutingWeights, TaskFit,
    evaluate_with_offers, resolve_model_tier_offer,
};

const ENV_ASSERTIONS: &str = r#"
import json, os, sys
from pathlib import Path
assert sys.version.split()[0] == "3.11.15", sys.version
root = Path(os.environ["SK_G2_OFFER_ROOT"]).resolve()
assert Path.cwd().resolve() == root, (Path.cwd(), root)
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)
"#;

const SOURCE_SCRIPT: &str = r#"
env_assertions
import dataclasses
from charter.offering.missions.step_offer_seam import resolve_model_tier_offer
from charter.offering.model_task_routing.evaluator import evaluate
from charter.offering.model_task_routing.models import ModelToTaskType
from charter.offering.agent_profiles.profile import AgentProfile
from glossary.models import (
    ConflictType, SenseRef, SemanticConflict, Severity, TermSurface,
    semantic_conflict_to_dict,
)

offer_cases = []
for step_offer, override in [
    (None, None),
    ("balanced", None),
    (None, "premium"),
    ("balanced", "premium"),
    ("balanced", "balanced"),
]:
    offer_cases.append(
        dataclasses.asdict(resolve_model_tier_offer(step_offer=step_offer, override=override))
    )

catalog = ModelToTaskType.model_validate({
    "schema_version": "1.0",
    "generated_at": "2026-01-01T00:00:00Z",
    "task_types": [{"id": "code-implementation", "title": "Code implementation"}],
    "models": [{
        "id": "model-a",
        "provider": "acme",
        "task_fit": [{"task_type": "code-implementation", "score": 0.9}],
        "cost": {"tier": "low"},
        "latency_tier": "low",
    }],
    "routing_policy": {
        "objective": "quality_first",
        "weights": {"quality": 1.0, "cost": 0.0, "risk": 0.0, "latency": 0.0},
        "override_policy": {"mode": "advisory", "require_reason": False},
    },
    "sources": [{
        "name": "fixture",
        "url": "https://example.invalid",
        "access_method": "api",
        "snapshot_at": "2026-01-01T00:00:00Z",
    }],
})
profile = AgentProfile.model_validate({
    "profile-id": "offer-fixture",
    "name": "Offer Fixture",
    "purpose": "p",
    "roles": ["implementer"],
    "specialization": {"primary-focus": "x"},
    "model": "model-profile",
    "effort": "high",
})
recommendations = {}
for name, step_offer, override in [
    ("default", None, None),
    ("offer", "balanced", None),
    ("override", "balanced", "premium"),
]:
    recommendation = evaluate(
        catalog,
        "code-implementation",
        profile,
        recommended_model_tier=step_offer,
        model_tier_override=override,
    )
    recommendations[name] = json.dumps(dataclasses.asdict(recommendation))

conflict = SemanticConflict(
    term=TermSurface("deploy"),
    conflict_type=ConflictType.AMBIGUOUS,
    severity=Severity.HIGH,
    confidence=1.0,
    candidate_senses=[
        SenseRef(surface="deploy", scope="team_domain", definition="give out", confidence=0.5),
        SenseRef(surface="deploy", scope="mission_local", definition="ship", confidence=0.9),
    ],
    context="request_text",
)
conflict_json = json.dumps(semantic_conflict_to_dict(conflict))
assert conflict_json.startswith(
    '{"term": {"surface_text": "deploy"}, "conflict_type": "ambiguous", "severity": "high", '
    '"confidence": 1.0, "candidate_senses": [{"surface": "deploy", "scope": "team_domain", '
    '"definition": "give out", "confidence": 0.5}'
), conflict_json
assert recommendations["default"].count('"model_tier": null') == 1, recommendations["default"]
assert '"model_tier": {"effective": "balanced", "source": "offer", "offer": "balanced"}' in recommendations["offer"]
assert '"model_tier": {"effective": "premium", "source": "override", "offer": "balanced"}' in recommendations["override"]

print(json.dumps({
    "offers": offer_cases,
    "recommendations": recommendations,
    "conflict": conflict_json,
}))
"#;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn run_source() -> Value {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = temp.path().canonicalize().expect("canonical root");
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        std::fs::create_dir_all(root.join(directory)).expect("isolated directory");
    }
    let root_repo = repository_root();
    let mut env = vec![
        ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
        ("HOME".to_owned(), root.join("home").display().to_string()),
        (
            "XDG_CACHE_HOME".to_owned(),
            root.join("cache").display().to_string(),
        ),
        (
            "XDG_CONFIG_HOME".to_owned(),
            root.join("config").display().to_string(),
        ),
        (
            "XDG_DATA_HOME".to_owned(),
            root.join("data").display().to_string(),
        ),
        (
            "XDG_STATE_HOME".to_owned(),
            root.join("state").display().to_string(),
        ),
        ("TMPDIR".to_owned(), root.join("tmp").display().to_string()),
        ("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()),
        (
            "PYTHONPATH".to_owned(),
            root_repo.join("src").display().to_string(),
        ),
        (
            "SPEC_KITTY_PACKS_ROOT".to_owned(),
            root_repo.join("packs").display().to_string(),
        ),
        ("SK_G2_OFFER_ROOT".to_owned(), root.display().to_string()),
    ];
    env.push(("SK_G2_SEED".to_owned(), "0".to_owned()));
    let script = format!(
        "{ENV_ASSERTIONS}\n{}",
        SOURCE_SCRIPT.replace("env_assertions\n", "")
    );
    let output = Command::new(support::python())
        .arg("-c")
        .arg(script)
        .env_clear()
        .envs(env)
        .current_dir(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run source oracle");
    assert!(
        output.status.success(),
        "Source oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("source oracle JSON")
}

fn catalog() -> ModelRoutingCatalog {
    ModelRoutingCatalog {
        schema_version: "1.0".to_owned(),
        routing_policy: RoutingPolicy {
            objective: RoutingObjective::QualityFirst,
            weights: RoutingWeights {
                quality: 1.0,
                cost: 0.0,
                risk: 0.0,
                latency: 0.0,
            },
            tier_constraints: Vec::new(),
            override_policy: OverridePolicy {
                mode: "advisory".to_owned(),
            },
        },
        models: vec![ModelEntry {
            id: "model-a".to_owned(),
            cost: ModelCost {
                tier: CostTier::Low,
            },
            latency_tier: Some(LatencyTier::Low),
            task_fit: vec![TaskFit {
                task_type: "code-implementation".to_owned(),
                score: 0.9,
                confidence: None,
                rationale: None,
            }],
        }],
    }
}

fn offer_json(offer: &ModelTierOffer) -> Value {
    json!({
        "effective": offer.effective,
        "source": offer.source,
        "offer": offer.offer,
    })
}

#[test]
fn advisory_offer_seam_and_glossary_conflict_wire_are_source_exact() {
    let source = run_source();

    // 1. The seam itself: override wins, the offer stays observable, neither
    //    present is `effective=None` with source `offer`.
    let cases: [(&str, Option<&str>, Option<&str>); 5] = [
        ("none", None, None),
        ("offer", Some("balanced"), None),
        ("override", None, Some("premium")),
        ("both", Some("balanced"), Some("premium")),
        ("agree", Some("balanced"), Some("balanced")),
    ];
    for (index, (name, step_offer, override_value)) in cases.iter().enumerate() {
        let native = offer_json(&resolve_model_tier_offer(*step_offer, *override_value));
        assert_eq!(
            native, source["offers"][index],
            "resolve_model_tier_offer({name}) must match the reference seam"
        );
    }

    // 2. The standalone evaluator with configured offers: the rendered
    //    recommendation payload (dataclass field order, nested offer order).
    let profile = ProfileRoutingView {
        preferred_model: Some("model-profile".to_owned()),
        effort: Some("high".to_owned()),
    };
    for (name, step_offer, override_value) in [
        ("default", None, None),
        ("offer", Some("balanced"), None),
        ("override", Some("balanced"), Some("premium")),
    ] {
        let recommendation = evaluate_with_offers(
            &catalog(),
            "code-implementation",
            &profile,
            step_offer,
            override_value,
        );
        assert_eq!(
            recommendation_json(&recommendation).render(None),
            source["recommendations"][name]
                .as_str()
                .expect("source JSON"),
            "{name}: advisory recommendation wire diverged"
        );
    }

    // 3. A non-empty glossary conflict body keeps the reference's key order and
    //    the same bytes through the bundle renderer.
    let conflict = SemanticConflictView {
        surface_text: "deploy".to_owned(),
        conflict_type: "ambiguous".to_owned(),
        severity: "high".to_owned(),
        confidence: 1.0,
        candidate_senses: vec![
            ConflictCandidateView {
                surface: "deploy".to_owned(),
                scope: "team_domain".to_owned(),
                definition: "give out".to_owned(),
                confidence: 0.5,
            },
            ConflictCandidateView {
                surface: "deploy".to_owned(),
                scope: "mission_local".to_owned(),
                definition: "ship".to_owned(),
                confidence: 0.9,
            },
        ],
        context: "request_text".to_owned(),
    };
    assert_eq!(
        conflict.to_python_json().render(None),
        source["conflict"].as_str().expect("source conflict JSON"),
        "glossary conflict wire diverged"
    );
    let bundle = GlossaryObservation {
        matched_urns: vec!["glossary:0011aabb".to_owned()],
        high_severity: vec![conflict.to_python_json()],
        all_conflicts: vec![conflict.to_python_json()],
        tokens_checked: 3,
        duration_ms: 0.5,
        error_msg: None,
    };
    let rendered = glossary_json(&bundle).render(None);
    assert!(
        rendered.starts_with(
            r#"{"matched_urns": ["glossary:0011aabb"], "high_severity": [{"term": {"surface_text": "deploy"}, "conflict_type": "ambiguous""#
        ),
        "{rendered}"
    );
    assert!(rendered.contains(r#""tokens_checked": 3, "duration_ms": 0.5, "error_msg": null"#));
}
