//! Source-first control for the glossary chokepoint.
//!
//! Fixture and expectations are the pinned reference's own output for
//! `glossary.chokepoint.GlossaryChokepoint.run` over the same seeds (CPython
//! 3.11.15, Source `9aed3a80`); `duration_ms` is the only field excluded from
//! the comparison. The reference fixture run produced:
//!
//! ```text
//! matched_urns: ["glossary:d93244e7", "glossary:ceb00a91", "glossary:ac49ad74"]
//! all_conflicts: one `ambiguous` / `medium` `lane` conflict with two senses
//! tokens_checked: 5
//! error_msg: null
//! ```

use std::fs;

use serde_json::{Value, json};
use sk_glossary::chokepoint::{
    GlossaryChokepoint, build_index, checked_tokens, glossary_urn, normalize,
};
use tempfile::TempDir;

const CORE_SEED: &str = "terms:
  - surface: lane
    definition: A unit of work in the mission topology.
    confidence: 0.9
    status: active
  - surface: mission
    definition: The canonical unit of delivery.
    confidence: 1.0
    status: active
";

const TEAM_SEED: &str = "terms:
  - surface: lane
    definition: A team-specific lane meaning.
    confidence: 0.8
    status: active
  - surface: glossaries
    definition: Seed files for terms.
    confidence: 0.7
    status: active
";

fn fixture() -> TempDir {
    let temp = TempDir::new().expect("temp");
    let glossaries = temp.path().join(".kittify/glossaries");
    fs::create_dir_all(&glossaries).expect("glossaries dir");
    fs::write(glossaries.join("spec_kitty_core.yaml"), CORE_SEED).expect("core seed");
    fs::write(glossaries.join("team_domain.yaml"), TEAM_SEED).expect("team seed");
    temp
}

fn bundle_json(temp: &TempDir, request: &str) -> Value {
    let mut chokepoint = GlossaryChokepoint::new(temp.path());
    let bundle = chokepoint.run(request, "", "unknown");
    let mut value = serde_json::to_value(&bundle).expect("bundle serialises");
    let object = value.as_object_mut().expect("bundle is an object");
    object.remove("duration_ms");
    value
}

#[test]
fn scan_matches_the_reference_bundle() {
    let temp = fixture();
    let actual = bundle_json(
        &temp,
        "Run lanes and missions with glossaries inside the work.",
    );
    let expected = json!({
        "matched_urns": [
            "glossary:d93244e7",
            "glossary:ceb00a91",
            "glossary:ac49ad74"
        ],
        "high_severity": [],
        "all_conflicts": [
            {
                "term": { "surface_text": "lane" },
                "conflict_type": "ambiguous",
                "severity": "medium",
                "confidence": 1.0,
                "candidate_senses": [
                    {
                        "surface": "lane",
                        "scope": "spec_kitty_core",
                        "definition": "A unit of work in the mission topology.",
                        "confidence": 0.9
                    },
                    {
                        "surface": "lane",
                        "scope": "team_domain",
                        "definition": "A team-specific lane meaning.",
                        "confidence": 0.8
                    }
                ],
                "context": "request_text"
            }
        ],
        "tokens_checked": 5,
        "error_msg": null
    });
    // Candidate-sense order within a conflict is the reference's frozenset
    // iteration order (not stable across processes), so compare the sets and
    // keep the native deterministic order as a separate assertion.
    let mut actual = actual;
    let mut expected = expected;
    for side in [&mut actual, &mut expected] {
        if let Some(senses) = side["all_conflicts"][0]["candidate_senses"].as_array_mut() {
            senses.sort_by_key(|sense| sense["scope"].as_str().unwrap_or("").to_string());
        }
    }
    assert_eq!(actual, expected);
    let native = bundle_json(
        &temp,
        "Run lanes and missions with glossaries inside the work.",
    );
    let scopes: Vec<&str> = native["all_conflicts"][0]["candidate_senses"]
        .as_array()
        .expect("senses")
        .iter()
        .map(|sense| sense["scope"].as_str().unwrap_or(""))
        .collect();
    // Canonical scope precedence: team_domain outranks spec_kitty_core.
    assert_eq!(scopes, vec!["team_domain", "spec_kitty_core"]);
}

#[test]
fn bundle_payload_keys_match_the_reference() {
    // Key *order* is not asserted: the workspace's `serde_json` has no
    // `preserve_order`, so every JSON object is emitted sorted (the same
    // pre-existing projection the rest of the CLI payloads use). The key SET
    // and every value are the reference contract.
    let temp = fixture();
    let mut chokepoint = GlossaryChokepoint::new(temp.path());
    let bundle = chokepoint.run("Run lanes.", "", "unknown");
    let value = serde_json::to_value(&bundle).expect("bundle serialises");
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "all_conflicts",
            "duration_ms",
            "error_msg",
            "high_severity",
            "matched_urns",
            "tokens_checked"
        ]
    );
}

#[test]
fn tokenisation_filters_common_words_and_lemmatises() {
    // The reference's `run`/`lane`/`mission`/`glossarie`/`inside` set: `and`,
    // `with`, `the` are common words, `work` is a common word, and every token
    // shorter than three characters is dropped.
    assert_eq!(
        checked_tokens("Run lanes and missions with glossaries inside the work."),
        vec!["run", "lane", "mission", "glossarie", "inside"]
    );
    assert_eq!(normalize("lanes"), "lane");
    assert_eq!(normalize("missions"), "mission");
    // The reference strips `ments$` first, leaving "require".
    assert_eq!(normalize("requirements"), "require");
    assert_eq!(normalize("processes"), "processe");
    assert_eq!(normalize("works"), "work");
    assert_eq!(normalize("ab"), "ab");
    // `es` is intentionally not a rule: "lanes" -> "lane", never "lan".
    assert_eq!(normalize("lanes"), "lane");
}

#[test]
fn glossary_urn_is_the_reference_hash_prefix() {
    assert_eq!(glossary_urn("lane"), "glossary:d93244e7");
    assert_eq!(glossary_urn("mission"), "glossary:ceb00a91");
    assert_eq!(glossary_urn("glossaries"), "glossary:ac49ad74");
}

#[test]
fn index_counts_distinct_canonical_terms() {
    let temp = fixture();
    let scopes = [
        sk_glossary::GlossaryScope::SpecKittyCore,
        sk_glossary::GlossaryScope::TeamDomain,
    ];
    let index = build_index(temp.path(), &scopes).expect("index");
    assert_eq!(index.term_count, 3);
    // The canonical surface and its lemmatised alias both resolve.
    assert_eq!(
        index.surface_to_urn.get("glossaries"),
        index.surface_to_urn.get("glossarie")
    );
}

#[test]
fn a_missing_or_malformed_glossary_is_the_error_envelope() {
    let temp = TempDir::new().expect("temp");
    let glossaries = temp.path().join(".kittify/glossaries");
    fs::create_dir_all(&glossaries).expect("glossaries dir");
    fs::write(
        glossaries.join("spec_kitty_core.yaml"),
        "terms: [not-a-mapping\n",
    )
    .expect("malformed seed");
    let mut chokepoint = GlossaryChokepoint::new(temp.path());
    let bundle = chokepoint.run("lane", "", "unknown");
    assert!(bundle.error_msg.is_some());
    assert!(bundle.matched_urns.is_empty());
    assert_eq!(bundle.tokens_checked, 0);
}
