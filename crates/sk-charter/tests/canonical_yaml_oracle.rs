//! Permanent Source-first differential for the canonical YAML writer.
//!
//! `fixtures/canonical_yaml_oracle.json` was produced by the pinned Source
//! (`charter/offering/yaml_utils.canonical_yaml` under
//! `.venv-rust-oracle`, CPython 3.11.15 / 53 distributions) by
//! `/tmp/sk-cont-f3/make_canonical_fixture.py`. Each case records the ruamel
//! dump of the input value and the exact canonical bytes the Writer must
//! reproduce.
//!
//! The fixture is committed, so the comparison is offline; a missing or
//! unreadable fixture fails the test instead of skipping (a permanent
//! `oracle_dir()->None` skip would let a regression pass silently).

use std::collections::BTreeMap;
use std::path::PathBuf;

use sk_charter::canonical_yaml;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("canonical_yaml_oracle.json")
}

#[derive(Debug)]
struct Case {
    input: String,
    expected: String,
    hash_excluding_manifest_hash: Option<String>,
}

fn load_cases() -> BTreeMap<String, Case> {
    let path = fixture_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "canonical YAML oracle fixture is required but unreadable at {}: {error}",
            path.display()
        )
    });
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("oracle fixture is not valid JSON: {error}"));
    let object = parsed
        .as_object()
        .unwrap_or_else(|| panic!("oracle fixture must be a JSON object"));
    object
        .iter()
        .map(|(name, value)| {
            let input = value
                .get("input")
                .and_then(|input| input.as_str())
                .unwrap_or_else(|| panic!("case {name} has no string input"));
            let expected = value
                .get("expected")
                .and_then(|expected| expected.as_str())
                .unwrap_or_else(|| panic!("case {name} has no string expected"));
            let recorded_hash = value
                .get("hash_excluding_manifest_hash")
                .and_then(|hash| hash.as_str())
                .map(str::to_string);
            (
                name.clone(),
                Case {
                    input: input.to_string(),
                    expected: expected.to_string(),
                    hash_excluding_manifest_hash: recorded_hash,
                },
            )
        })
        .collect()
}

#[test]
fn every_recorded_source_case_matches_byte_for_byte() {
    let cases = load_cases();
    assert!(
        cases.len() >= 4,
        "oracle fixture must carry the recorded Source corpus, found {} cases",
        cases.len()
    );
    let mut failures = Vec::new();
    for (name, case) in &cases {
        let value: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&case.input) {
            Ok(value) => value,
            Err(error) => {
                failures.push(format!("{name}: fixture input did not parse: {error}"));
                continue;
            }
        };
        let actual = String::from_utf8(canonical_yaml(&value))
            .unwrap_or_else(|error| panic!("{name}: writer produced non-UTF-8 bytes: {error}"));
        if actual != case.expected {
            failures.push(format!(
                "{name}: byte mismatch\n--- source ---\n{}\n--- native ---\n{}",
                case.expected, actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "canonical YAML diverges from the pinned Source:\n{}",
        failures.join("\n\n")
    );
}

#[test]
fn manifest_hash_matches_recorded_source_digest() {
    // The Source recipe is SHA-256 over canonical_yaml(fields minus
    // manifest_hash). The recorded digests were produced by the pinned oracle;
    // they are the verifier-visible values the v2 repair migration must
    // reproduce.
    let cases = load_cases();
    let mut checked = 0usize;
    for (name, case) in &cases {
        let Some(expected) = &case.hash_excluding_manifest_hash else {
            continue;
        };
        let value: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&case.input).expect("fixture input parses");
        let digest = sk_charter::canonical_yaml_sha256_without(&value, &["manifest_hash"]);
        assert_eq!(&digest, expected, "manifest hash mismatch for case {name}");
        checked += 1;
    }
    assert!(checked >= 2, "expected recorded Source manifest digests");
}

#[test]
fn hashing_is_deterministic_and_key_order_independent() {
    let left: serde_yaml_ng::Value = serde_yaml_ng::from_str("b: 2\na: 1\n").expect("parse");
    let right: serde_yaml_ng::Value = serde_yaml_ng::from_str("a: 1\nb: 2\n").expect("parse");
    assert_eq!(canonical_yaml(&left), canonical_yaml(&right));
    assert_eq!(
        sk_charter::canonical_yaml_sha256(&left),
        sk_charter::canonical_yaml_sha256(&right)
    );
}
