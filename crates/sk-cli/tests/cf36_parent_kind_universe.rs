//! Existing canonical kind universe must not be manually extended twice.
use std::{collections::BTreeSet, fs};
#[test]
fn charter_list_emits_each_canonical_kind_once_in_source_order() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".kittify")).unwrap();
    fs::write(
        temp.path().join(".kittify/config.yaml"),
        b"agents:\n  available: [codex]\n",
    )
    .unwrap();
    let payload = sk_cli::compat::charter::list_payload(temp.path(), false, false).unwrap();
    let tokens: Vec<&str> = payload["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["kind"].as_str().unwrap())
        .collect();
    assert_eq!(tokens, sk_charter::CHARTER_KIND_TOKENS);
    assert_eq!(
        tokens.len(),
        tokens.iter().copied().collect::<BTreeSet<_>>().len()
    );
    assert_eq!(
        tokens
            .iter()
            .filter(|kind| **kind == "mission-type")
            .count(),
        1
    );
    assert!(payload["templates"].is_null());
}
