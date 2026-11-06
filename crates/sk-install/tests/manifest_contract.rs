//! The managed-asset manifest wire contract: absence, corruption, duplicates,
//! deterministic serialization, and ownership hashing.

mod support;

use std::path::Path;

use sk_install::manifest::{AssetManifest, MANIFEST_REL, ManifestEntry, content_hash};

fn sample_manifest() -> AssetManifest {
    let mut manifest = AssetManifest::empty();
    manifest.upsert(
        ManifestEntry::new(
            ".agents/skills/spec-kitty-plan/SKILL.md",
            "a".repeat(64),
            ["codex".to_string()],
            "2026-11-02T10:00:00.000000+00:00",
            "4.0.0rc7",
        )
        .expect("entry"),
    );
    manifest.upsert(
        ManifestEntry::new(
            ".agents/skills/spec-kitty-review/SKILL.md",
            "b".repeat(64),
            ["vibe".to_string(), "codex".to_string()],
            "2026-11-02T10:00:01.000000+00:00",
            "4.0.0rc7",
        )
        .expect("entry"),
    );
    manifest
}

#[test]
fn absent_manifest_is_an_empty_manifest_not_an_error() {
    let directory = tempfile::tempdir().expect("temp dir");
    assert!(
        AssetManifest::load(directory.path())
            .expect("load")
            .is_none()
    );
    assert!(AssetManifest::empty().entries().is_empty());
}

#[test]
fn serialization_is_sorted_key_stable_and_byte_deterministic() {
    let manifest = sample_manifest();
    let bytes = manifest.to_bytes();
    let text = std::str::from_utf8(&bytes).expect("utf-8");
    assert!(text.starts_with("{\n  \"entries\": [\n"));
    assert!(text.contains("\"schema_version\": 1"));
    assert!(text.ends_with("}\n"), "trailing newline");
    // Paths sorted, agents sorted, keys sorted.
    let plan_position = text.find("spec-kitty-plan").expect("plan entry");
    let review_position = text.find("spec-kitty-review").expect("review entry");
    assert!(plan_position < review_position);
    assert!(text.contains("\"agents\": [\n        \"codex\",\n        \"vibe\"\n      ]"));
    assert_eq!(bytes, manifest.to_bytes(), "byte-stable");

    // Round-trips through parse unchanged.
    let parsed = AssetManifest::from_bytes(Path::new(MANIFEST_REL), &bytes).expect("parse");
    assert_eq!(parsed.entries(), manifest.entries());
    assert_eq!(parsed.to_bytes(), bytes);
}

#[test]
fn corrupt_and_malformed_manifests_fail_closed() {
    let path = Path::new(MANIFEST_REL);
    let error = AssetManifest::from_bytes(path, b"{ not json").expect_err("corrupt");
    assert_eq!(error.code(), "manifest_corrupt_json");

    let error = AssetManifest::from_bytes(path, b"[]").expect_err("non-object");
    assert_eq!(error.code(), "manifest_corrupt_json");

    let version = br#"{"schema_version": 2, "entries": []}"#;
    let error = AssetManifest::from_bytes(path, version).expect_err("version");
    assert_eq!(error.code(), "manifest_unsupported_schema_version");
    assert!(error.to_string().contains('2'));

    let missing_version = br#"{"entries": []}"#;
    let error = AssetManifest::from_bytes(path, missing_version).expect_err("missing version");
    assert_eq!(error.code(), "manifest_unsupported_schema_version");

    let bad_hash = br#"{"schema_version":1,"entries":[{"path":".agents/skills/spec-kitty-plan/SKILL.md","content_hash":"nope","agents":["codex"],"installed_at":"x","spec_kitty_version":"v"}]}"#;
    let error = AssetManifest::from_bytes(path, bad_hash).expect_err("bad hash");
    assert_eq!(error.code(), "manifest_invalid_entry");

    let unknown_agent = br#"{"schema_version":1,"entries":[{"path":".agents/skills/spec-kitty-plan/SKILL.md","content_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","agents":["ghost"],"installed_at":"x","spec_kitty_version":"v"}]}"#;
    let error = AssetManifest::from_bytes(path, unknown_agent).expect_err("unknown agent");
    assert_eq!(error.code(), "manifest_invalid_entry");

    let traversal = br#"{"schema_version":1,"entries":[{"path":"../escape/SKILL.md","content_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","agents":["codex"],"installed_at":"x","spec_kitty_version":"v"}]}"#;
    let error = AssetManifest::from_bytes(path, traversal).expect_err("unsafe path");
    assert_eq!(error.code(), "unsafe_path");
}

#[test]
fn duplicate_paths_are_refused() {
    let duplicate = br#"{"schema_version":1,"entries":[
        {"path":".agents/skills/spec-kitty-plan/SKILL.md","content_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","agents":["codex"],"installed_at":"x","spec_kitty_version":"v"},
        {"path":".agents/skills/spec-kitty-plan/SKILL.md","content_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","agents":["vibe"],"installed_at":"y","spec_kitty_version":"v"}
    ]}"#;
    let error =
        AssetManifest::from_bytes(Path::new(MANIFEST_REL), duplicate).expect_err("duplicate");
    assert_eq!(error.code(), "manifest_duplicate_path");
    assert!(error.to_string().contains("spec-kitty-plan"));
}

#[test]
fn unknown_top_level_fields_are_tolerated_and_dropped() {
    let future = br#"{"schema_version":1,"future_field":{"a":1},"entries":[]}"#;
    let manifest = AssetManifest::from_bytes(Path::new(MANIFEST_REL), future).expect("parse");
    assert!(
        !std::str::from_utf8(&manifest.to_bytes())
            .expect("utf-8")
            .contains("future_field")
    );
}

#[test]
fn ownership_is_reference_counted_and_normalized() {
    let mut manifest = AssetManifest::empty();
    let entry = ManifestEntry::new(
        ".agents/skills/spec-kitty-plan/SKILL.md",
        "c".repeat(64),
        ["vibe".to_string(), "codex".to_string(), "codex".to_string()],
        "2026-11-02T10:00:00.000000+00:00",
        "4.0.0rc7",
    )
    .expect("entry");
    assert_eq!(entry.agents, vec!["codex", "vibe"]);
    manifest.upsert(entry);

    let entry = manifest
        .find(".agents/skills/spec-kitty-plan/SKILL.md")
        .expect("entry")
        .with_agent_added("pi");
    manifest.upsert(entry);
    assert_eq!(
        manifest
            .find(".agents/skills/spec-kitty-plan/SKILL.md")
            .expect("entry")
            .agents,
        vec!["codex", "pi", "vibe"]
    );

    let released = manifest.remove_entries_for_agent("codex");
    assert_eq!(released.len(), 1);
    assert!(manifest.entries().is_empty());
    assert_eq!(manifest.owners().len(), 0);
}

#[test]
fn timestamps_are_ignored_for_change_detection() {
    let mut first = sample_manifest();
    let bytes = first.to_bytes();
    let mut second = AssetManifest::from_bytes(Path::new(MANIFEST_REL), &bytes).expect("parse");
    // Rewrite only the volatile timestamp on one entry.
    let mut entry = second.entries()[0].clone();
    entry.installed_at = "2030-01-01T00:00:00.000000+00:00".to_string();
    second.upsert(entry);
    assert!(first.equals_ignoring_timestamps(&second));
    assert_ne!(first.to_bytes(), second.to_bytes());

    // A content change is not ignored.
    let mut entry = second.entries()[0].clone();
    entry.content_hash = "d".repeat(64);
    second.upsert(entry);
    assert!(!first.equals_ignoring_timestamps(&second));
    let _ = &mut first;
}

#[test]
fn ownership_hash_is_lowercase_sha256() {
    assert_eq!(
        content_hash(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(content_hash(b"abc").len(), 64);
    assert_eq!(content_hash(b"abc"), content_hash(b"abc"));
}

#[test]
fn a_symlinked_manifest_is_not_a_regular_file() {
    let directory = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(directory.path().join(".kittify")).expect("dir");
    let target = directory.path().join("elsewhere.json");
    support::write(&target, "{}");
    std::os::unix::fs::symlink(&target, directory.path().join(MANIFEST_REL)).expect("symlink");
    let error = AssetManifest::load(directory.path()).expect_err("symlink refused");
    assert_eq!(error.code(), "manifest_not_regular_file");
}
