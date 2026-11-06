//! CF12 focused smoke: the real `charter status` operation.
//!
//! These drive the public `charter::status_payload` seam against fresh
//! `tempfile` fixtures and assert the *actual* reference facts the collectors
//! read (bundle content hash, metadata sync stamp, synthesis manifest state,
//! freshness layers, activation ledger) plus the read-only guarantee.
//!
//! Not a parity campaign: the org-layer, governance-reference, evidence and
//! provenance-entry collectors are unported, and those sections are asserted to
//! report the gap in the reference's own degradation channel instead of a
//! fabricated clean bill. Native-only smoke is not Source-oracle proof.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_charter::synthesis::{
    ManifestArtifactEntry, PROVENANCE_DIR, SynthesisManifest, provenance_path_for,
};
use sk_charter::{compute_bundle_content_hash, hash_content};
use sk_cli::compat::charter;

const CHARTER_MD: &str =
    "# Fixture Charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n";
const CHARTER_YAML: &str =
    "schema_version: 2.0.0\ncatalog: {}\ngenerated_at: '2026-01-01T00:00:00Z'\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A project with an authored, synced charter bundle and activation state.
fn synced_fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\ncharter_packs:\n  project:\n    enabled: true\n",
    );
    write(&root.join(".kittify/charter/charter.md"), CHARTER_MD);
    write(&root.join(".kittify/charter/charter.yaml"), CHARTER_YAML);
    let charter_hash = hash_content(CHARTER_MD);
    write(
        &root.join(".kittify/charter/metadata.yaml"),
        &format!("charter_hash: {charter_hash}\ntimestamp_utc: '2026-01-02T03:04:05+00:00'\n"),
    );
    write(&root.join(".kittify/charter/library/one.md"), "# one\n");
    write(&root.join(".kittify/charter/library/two.md"), "# two\n");
    write(
        &root.join(".kittify/charter/activation.yaml"),
        "# authored activation state\nactivated_directives:\n- 001-fixture-directive\ndeactivated_tactics:\n- fixture-tactic\n",
    );
    temp
}

fn manifest(
    artifacts: Vec<ManifestArtifactEntry>,
    built_in_only: bool,
    bundle_content_hash: Option<String>,
) -> SynthesisManifest {
    SynthesisManifest {
        schema_version: "3".to_owned(),
        mission_id: None,
        created_at: "2026-01-01T00:00:00+00:00".to_owned(),
        run_id: "01J00000000000000000000000".to_owned(),
        adapter_id: "fixture-adapter".to_owned(),
        adapter_version: "1.0.0".to_owned(),
        synthesizer_version: "4.0.0".to_owned(),
        manifest_hash: "0".repeat(64),
        artifacts,
        built_in_only,
        bundle_content_hash,
        raw_field_names: None,
    }
    .finalize()
}

fn write_manifest(root: &Path, manifest: &SynthesisManifest) {
    fs::write(
        root.join(".kittify/charter/synthesis-manifest.yaml"),
        manifest.to_canonical_bytes(),
    )
    .expect("manifest write");
}

fn directive_entry(content_hash: &str) -> ManifestArtifactEntry {
    ManifestArtifactEntry {
        kind: "directive".to_owned(),
        slug: "fixture-directive".to_owned(),
        path: ".kittify/charter-packs/directive/001-fixture-directive.directive.yaml".to_owned(),
        provenance_path: provenance_path_for("directive", "fixture-directive"),
        content_hash: content_hash.to_owned(),
    }
}

/// Every file under `root` with its bytes (the read-only proof).
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = fs::read_dir(&current).expect("fixture read_dir");
        for entry in entries {
            let entry = entry.expect("fixture entry");
            let path = entry.path();
            if entry.file_type().expect("fixture type").is_dir() {
                pending.push(path);
            } else {
                files.insert(path.clone(), fs::read(&path).expect("fixture bytes"));
            }
        }
    }
    files
}

fn payload(root: &Path) -> Value {
    charter::status_payload(root, false).expect("status payload")
}

#[test]
fn a_synced_fixture_reports_real_sync_manifest_and_freshness_facts() {
    let temp = synced_fixture();
    let root = temp.path();
    let report = payload(root);

    assert_eq!(report["result"], "success");
    let sync = &report["charter_sync"];
    assert_eq!(sync["available"], true);
    assert_eq!(sync["status"], "synced");
    assert_eq!(sync["charter_path"], ".kittify/charter/charter.md");
    assert_eq!(sync["current_hash"], hash_content(CHARTER_MD));
    assert_eq!(sync["stored_hash"], hash_content(CHARTER_MD));
    assert_eq!(sync["last_sync"], "2026-01-02T03:04:05+00:00");
    assert_eq!(sync["library_docs"], 2);
    assert_eq!(sync["files"][0]["name"], "charter.yaml");
    assert_eq!(sync["files"][0]["exists"], true);
    assert!(sync["files"][0]["size_kb"].as_f64().unwrap_or(0.0) > 0.0);
    assert_eq!(sync["files"][1]["name"], "charter.md");

    let synthesis = &report["synthesis"];
    assert_eq!(synthesis["generation_state"], "not_started");
    assert_eq!(synthesis["manifest"]["state"], "missing");
    assert_eq!(synthesis["generated_inputs"]["total"], 0);
    assert!(
        synthesis["provenance"]["warnings"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let freshness = &report["freshness"];
    assert_eq!(freshness["charter_source"]["state"], "fresh");
    assert_eq!(freshness["synced_bundle"]["state"], "fresh");
    assert_eq!(freshness["synthesized_drg"]["state"], "missing");
    assert_eq!(
        freshness["synthesized_drg"]["remediation"],
        "spec-kitty charter synthesize"
    );

    let activation = &report["activation"];
    assert_eq!(activation["schema_version"], "1.0.0");
    assert_eq!(activation["path"], ".kittify/charter/activation.yaml");
    assert_eq!(activation["exists"], true);
    assert_eq!(
        activation["selected"]["directives"],
        serde_json::json!(["001-fixture-directive"])
    );
    assert_eq!(
        activation["deactivated"]["tactics"],
        serde_json::json!(["fixture-tactic"])
    );
    assert!(activation.get("error").is_none());

    // Unported collectors report the gap, never a clean layer.
    let errors = report["org_layer"]["errors"]
        .as_array()
        .expect("org errors");
    assert!(
        errors.iter().any(|error| error
            .as_str()
            .unwrap_or_default()
            .contains("not implemented natively")),
        "{errors:?}"
    );
    assert_eq!(report["governance_references"]["available"], false);
}

#[test]
fn a_stale_sync_state_reports_both_real_hashes() {
    let temp = synced_fixture();
    let root = temp.path();
    write(
        &root.join(".kittify/charter/charter.md"),
        "# Fixture Charter\n\n## Policy Summary\n\n- changed after the last sync.\n",
    );
    let report = payload(root);
    let sync = &report["charter_sync"];
    assert_eq!(sync["status"], "stale");
    assert_ne!(sync["current_hash"], sync["stored_hash"]);
    assert_eq!(sync["stored_hash"], hash_content(CHARTER_MD));
    assert_eq!(
        sync["current_hash"],
        hash_content(&fs::read_to_string(root.join(".kittify/charter/charter.md")).unwrap())
    );
    // The stale branch reuses the freshness computer's remediation.
    assert_eq!(
        report["freshness"]["charter_source"]["remediation"],
        Value::Null
    );
}

#[test]
fn an_absent_charter_reports_unavailable_sync_and_missing_layers() {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "custom_setting: keep-me\n",
    );
    let report = payload(root);

    let sync = &report["charter_sync"];
    assert_eq!(sync["available"], false);
    assert!(
        sync["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Charter not found at"),
        "{sync}"
    );
    let freshness = &report["freshness"];
    assert_eq!(freshness["charter_source"]["state"], "missing");
    assert_eq!(
        freshness["charter_source"]["remediation"],
        "spec-kitty charter generate --no-from-interview"
    );
    assert!(
        freshness["charter_source"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("this project has no charter at all")
    );
    assert_eq!(freshness["synced_bundle"]["state"], "missing");
    assert_eq!(freshness["synthesized_drg"]["state"], "missing");
}

#[test]
fn a_legacy_bundle_is_distinguished_from_no_charter_at_all() {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(&root.join(".kittify/charter/charter.md"), CHARTER_MD);
    write(
        &root.join(".kittify/charter/references.yaml"),
        "references: []\n",
    );
    let report = payload(root);
    let source = &report["freshness"]["charter_source"];
    assert_eq!(source["state"], "missing");
    assert_eq!(source["remediation"], "spec-kitty upgrade --yes");
    let detail = source["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("charter.md/references.yaml"), "{detail}");
}

#[test]
fn an_unsound_charter_is_refused_before_any_collector() {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "- not\n- a mapping\n",
    );
    let error = charter::status_payload(root, false).expect_err("refusal");
    let document = error.to_json();
    assert_eq!(document["error_code"], "ACTIVE_CHARTER_CONFIG_INVALID");
    let message = error.message();
    assert!(message.contains("root must be a mapping."), "{message}");
    assert!(message.contains("Remediation:"), "{message}");
    assert!(!message.contains("Nothing was written."), "{message}");
}

#[test]
fn a_dangling_charter_pointer_is_refused_with_the_pointer_remedy() {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    write(
        &root.join(".kittify/config.yaml"),
        "charter: .kittify/charter/missing-charter.yaml\n",
    );
    let error = charter::status_payload(root, false).expect_err("refusal");
    assert_eq!(
        error.to_json()["error_code"],
        "ACTIVE_CHARTER_CONFIG_INVALID"
    );
    assert!(
        error
            .message()
            .contains("pointer names .kittify/charter/missing-charter.yaml"),
        "{}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("point the `charter:` key in .kittify/config.yaml"),
        "{}",
        error.message()
    );
}

#[test]
fn an_invalid_manifest_reports_needs_attention_with_the_integrity_error() {
    let temp = synced_fixture();
    let root = temp.path();
    write(
        &root.join(".kittify/charter-packs/directive/001-fixture-directive.directive.yaml"),
        "id: PROJECT_001\n",
    );
    write_manifest(
        root,
        &manifest(vec![directive_entry("deadbeef")], false, None),
    );

    let report = payload(root);
    let synthesis = &report["synthesis"];
    assert_eq!(synthesis["manifest"]["state"], "invalid");
    assert!(
        synthesis["manifest"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Manifest integrity check failed"),
        "{}",
        synthesis["manifest"]["error"]
    );
    assert_eq!(synthesis["manifest"]["artifact_count"], 1);
    assert_eq!(synthesis["manifest"]["live_artifact_count"], 1);
    assert_eq!(
        synthesis["manifest"]["missing_provenance_paths"],
        serde_json::json!([provenance_path_for("directive", "fixture-directive")])
    );
    assert_eq!(synthesis["generation_state"], "needs_attention");
    assert!(
        !synthesis["provenance"]["warnings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_verified_empty_manifest_is_promoted_and_built_in_only_reports_residue() {
    let temp = synced_fixture();
    let root = temp.path();
    write(
        &root.join(".kittify/charter-packs/directive/001-fixture-directive.directive.yaml"),
        "id: PROJECT_001\n",
    );
    write_manifest(root, &manifest(Vec::new(), true, None));
    write(
        &root.join(".kittify/charter-packs/graph.yaml"),
        "nodes: []\nedges: []\n",
    );

    let report = payload(root);
    assert_eq!(report["synthesis"]["manifest"]["state"], "valid");
    assert_eq!(report["synthesis"]["generation_state"], "promoted");
    assert_eq!(
        report["freshness"]["synthesized_drg"]["state"],
        "built_in_only"
    );
    assert!(
        report["freshness"]["synthesized_drg"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("stale graph residue"),
        "{}",
        report["freshness"]["synthesized_drg"]
    );
}

#[test]
fn a_manifest_with_artifacts_stays_needs_attention_while_the_sidecar_model_is_pending() {
    // The reference reaches `promoted` only when every provenance sidecar parsed
    // cleanly. The Native sidecar entry model is unported, so the collector
    // reports the gap in `warnings` and the reference's own precedence turns
    // that into `needs_attention` — never a promoted claim from an unconsulted
    // section.
    let temp = synced_fixture();
    let root = temp.path();
    write(
        &root.join(".kittify/charter-packs/directive/001-fixture-directive.directive.yaml"),
        "id: PROJECT_001\n",
    );
    let artifact_hash = sk_kernel::content_digest::sha256_hex(
        &fs::read(
            root.join(".kittify/charter-packs/directive/001-fixture-directive.directive.yaml"),
        )
        .expect("artifact bytes"),
    );
    fs::create_dir_all(root.join(PROVENANCE_DIR)).expect("provenance dir");
    write(
        &root.join(provenance_path_for("directive", "fixture-directive")),
        "artifact_urn: directive:PROJECT_001\n",
    );
    write_manifest(
        root,
        &manifest(vec![directive_entry(&artifact_hash)], false, None),
    );

    let report = payload(root);
    assert_eq!(report["synthesis"]["manifest"]["state"], "valid");
    assert_eq!(report["synthesis"]["generation_state"], "needs_attention");
    let warnings = report["synthesis"]["provenance"]["warnings"]
        .as_array()
        .expect("provenance warnings");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap_or_default()
            .contains("not implemented natively")),
        "{warnings:?}"
    );
    assert_eq!(report["synthesis"]["provenance"]["parsed_count"], 0);
    assert_eq!(
        report["synthesis"]["provenance"]["missing_for_manifest_count"],
        0
    );
}

#[test]
fn the_synthesized_layer_follows_the_bundle_content_hash() {
    let temp = synced_fixture();
    let root = temp.path();
    write(
        &root.join(".kittify/charter-packs/graph.yaml"),
        "nodes: []\nedges: []\n",
    );
    let current = compute_bundle_content_hash(root).expect("bundle hash");
    write_manifest(root, &manifest(Vec::new(), false, Some(current)));
    assert_eq!(
        payload(root)["freshness"]["synthesized_drg"]["state"],
        "fresh"
    );

    // An authoring-only edit changes the whole-file hash: the reference reports
    // that (accepted, self-healing) stale state rather than a stale-free claim.
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "schema_version: 2.0.0\ncatalog: {}\ngovernance: {mode: strict}\n",
    );
    let after = payload(root);
    assert_eq!(after["freshness"]["synced_bundle"]["state"], "fresh");
    assert_eq!(after["freshness"]["synthesized_drg"]["state"], "stale");
    assert_eq!(
        after["freshness"]["synthesized_drg"]["remediation"],
        "spec-kitty charter synthesize"
    );
}

#[test]
fn status_is_read_only_and_idempotent() {
    let temp = synced_fixture();
    let root = temp.path();
    let before = snapshot(root);
    let first = payload(root);
    let second = payload(root);
    assert_eq!(first, second);
    assert_eq!(
        snapshot(root),
        before,
        "status must not create or rewrite any file"
    );
}

#[test]
fn provenance_mode_keeps_the_payload_shape_without_inventing_entries() {
    let temp = synced_fixture();
    let root = temp.path();
    let with_provenance = charter::status_payload(root, true).expect("status");
    assert_eq!(
        with_provenance["synthesis"]["provenance"]["entries"],
        serde_json::json!([])
    );
    assert!(with_provenance["synthesis"]["provenance"]["count"].is_number());
}
