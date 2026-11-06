//! `spec-kitty charter status` — the read-only charter state report.
//!
//! Mirrors the pinned Source command body
//! (`src/specify_cli/cli/commands/charter/status.py`) plus the collectors it
//! calls (`_status_collectors.py`, `charter_runtime/freshness/computer.py`) on
//! the delivered Native owners:
//!
//! * the charter bundle content-identity recipe and the synthesis-manifest
//!   model: [`sk_charter::hash_content`],
//!   [`sk_charter::compute_bundle_content_hash`],
//!   [`sk_charter::synthesis::SynthesisManifest`];
//! * the `charter:` pointer + `.kittify/config.yaml` reader:
//!   [`sk_charter::load_project_config`];
//! * the canonical lossless YAML read authority: [`sk_doctrine::YamlDocument`]
//!   (used strictly as a *reader*: nothing here re-serialises);
//! * the activation ledger and org required floor:
//!   [`sk_doctrine::load_activation_ledger_inspection`],
//!   [`sk_doctrine::read_org_required_provenance`], [`sk_doctrine::resolve_deny`];
//! * digests and wall-clock rendering: `sk_kernel::content_digest`,
//!   `sk_kernel::clock`.
//!
//! This is a pure observer, exactly like the reference: it never generates a
//! charter, never synthesizes, never repairs, never compiles a catalog and never
//! writes `context-state.json` or any project file. Absent, corrupt and
//! unconsulted states stay distinct — a section whose collector is not ported
//! says so in the reference's own degradation channel (`org_layer.errors`,
//! `governance_references.available=false` + `warnings`,
//! `synthesis.provenance.warnings`, `synthesis.evidence.warnings`) rather than
//! reporting invented health, readiness, digests or timestamps.
//!
//! Known functional gaps (see the lane report): the org-DRG layer merge/conflict
//! partition, the governance-reference collector, the code-evidence/corpus
//! collector, the provenance-sidecar entry model, the bundle-schema
//! compatibility gate and the rich-table rendering are not ported. Each one is
//! named at the exact place Source would print it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Map, Value, json};

use sk_charter::synthesis::{
    PROJECT_PACK_ROOT, PROVENANCE_DIR, SYNTHESIS_MANIFEST_PATH, SynthesisManifest,
};
use sk_charter::{
    ArtifactKind, CHARTER_YAML, Limits, compute_bundle_content_hash, hash_content,
    load_project_config,
};
use sk_doctrine::{
    ActivationLedger, ActivationSource, DoctrineError, IdentityIndex, YamlDocument,
    org_requirable_kinds, read_org_required_provenance, resolve_deny,
};
use sk_kernel::clock::{format_stamp, from_epoch};

use super::StatusArgs;
use crate::compat::support::{CompatError, emit_json_pretty, project_root};

/// `.kittify/charter/` (the reference's charter directory).
const CHARTER_DIR: &str = ".kittify/charter";
/// The authoritative bundle filename.
const CHARTER_YAML_FILENAME: &str = "charter.yaml";
/// The curated prose companion (display-only in the reference).
const CHARTER_MD_FILENAME: &str = "charter.md";
/// The retired pre-consolidation metadata file (still read for `last_sync`).
const METADATA_FILENAME: &str = "metadata.yaml";
/// The activation ledger filename (mirrors `sk_doctrine::ACTIVATION_LEDGER_REL`).
const ACTIVATION_LEDGER_FILENAME: &str = "activation.yaml";
/// The synthesis generated-input root.
const GENERATED_DIR: &str = ".kittify/charter/generated";
/// `charter_context` provenance sidecar prefix used by the reference validator.
const PROVENANCE_PATH_PREFIX: &str = PROVENANCE_DIR;
/// `.kittify/config.yaml` (the pointer owner; display only here).
const CONFIG_YAML_REL: &str = ".kittify/config.yaml";
/// The `config.yaml` key naming the active `charter.yaml`.
const CHARTER_POINTER_KEY: &str = "charter";
/// The only bundle `schema_version` major this build understands.
const SUPPORTED_SCHEMA_MAJOR: &str = "2";
/// The reference's "wrote nothing" gate line (dropped by this read-only command).
const NOTHING_WRITTEN: &str = "Nothing was written.";
/// The shared soundness gate's default remedy.
const SOUNDNESS_REMEDIATION: &str = "Remediation: fix the YAML in the charter file named above, or re-run `spec-kitty charter generate --from-interview` to regenerate it.";
/// The remedy for a dangling `charter:` pointer (the reference's status variant).
const DANGLING_POINTER_REMEDIATION: &str = "Remediation: point the `charter:` key in .kittify/config.yaml at an existing file or remove it, or re-run `spec-kitty charter generate --from-interview`.";

/// The reference's freshness remediation strings.
const REMEDIATE_SYNTHESIZE: &str = "spec-kitty charter synthesize";
const REMEDIATE_GENERATE_NO_INTERVIEW: &str = "spec-kitty charter generate --no-from-interview";
const REMEDIATE_UPGRADE_YES: &str = "spec-kitty upgrade --yes";

/// `contracts/errors.md` codes this command renders.
const CHARTER_CONFIG_SHAPE_INVALID: &str = "CHARTER_CONFIG_SHAPE_INVALID";
const ACTIVE_CHARTER_CONFIG_INVALID: &str = "ACTIVE_CHARTER_CONFIG_INVALID";
const INTERNAL_ERROR: &str = "INTERNAL_ERROR";
const ACTIVATION_FAILED: &str = "ACTIVATION_FAILED";
const ACTIVATION_STATE_UNREADABLE: &str = "ACTIVATION_STATE_UNREADABLE";

/// The local `activation` block schema version (`01M5ZCAVRVKZDA0FG068BRK6WZ`).
const ACTIVATION_BLOCK_SCHEMA_VERSION: &str = "1.0.0";

/// The four pre-consolidation bundle files a project may still carry.
const LEGACY_BUNDLE_FILENAMES: [&str; 4] = [
    "governance.yaml",
    "directives.yaml",
    "metadata.yaml",
    "references.yaml",
];

/// The provenance collector gap (named at the sidecar section).
const PROVENANCE_GAP: &str = "provenance sidecar loading is not implemented natively (owner request: \
a `sk_charter::provenance` entry model + `load_provenance`); no sidecar was parsed, so \
`parsed_count`/`corpus_snapshot_ids`/`adapters`/`entries` report nothing — they are unreported, \
not empty";

/// The evidence collector gap (named at the evidence section).
const EVIDENCE_GAP: &str = "the code-evidence/corpus collector (`_collect_evidence_result`) is not \
implemented natively; `code`, the configured-URL count and the corpus snapshot below carry no \
consulted evidence";

/// The org-layer gap (named in the reference's own `errors` channel).
const ORG_LAYER_GAP: &str = "org-layer collector is not implemented natively (owner request: an \
org-DRG load/merge/conflict partition over `load_org_pack`/`merge_three_layers` \
`OrgDRGConflictError`); no org pack was consulted, so the empty `packs` list is not a \
verified-clean organisation layer";

/// The governance-reference gap (named in the reference's own `warnings` channel).
const GOVERNANCE_REFERENCES_GAP: &str = "Could not inspect governance references: the \
governance-reference collector (`charter.activation.governance_references`) is not implemented \
natively; no reference was consulted";

/// One coded failure of the status command.
struct StatusFailure {
    code: &'static str,
    message: String,
}

impl StatusFailure {
    fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The reference's catch-all: an exception no handler anticipated.
    fn internal(message: impl std::fmt::Display) -> Self {
        Self {
            code: INTERNAL_ERROR,
            message: format!("Unexpected error: {message}"),
        }
    }
}

/// The `charter status` entry point.
pub(super) fn status_command(args: &StatusArgs) -> Result<u8, CompatError> {
    let root = project_root()?;
    match build_payload(&root, args.provenance) {
        Ok(payload) => {
            if args.json {
                let _ = emit_json_pretty(&Value::Object(payload));
            } else {
                render_text(&payload, args.provenance);
            }
            Ok(0)
        }
        Err(failure) => {
            render_error(&failure, args.json);
            Ok(1)
        }
    }
}

/// The public payload seam: the collectors behind a `CompatError` code.
pub(super) fn payload(root: &Path, provenance: bool) -> Result<Value, CompatError> {
    build_payload(root, provenance)
        .map(Value::Object)
        .map_err(|failure| CompatError::coded(failure.code, failure.message))
}

/// One coded error, rendered exactly once (`Error (CODE): message` or the
/// canonical `{"ok": false, "error": {"code", "message"}}` envelope).
fn render_error(failure: &StatusFailure, json_output: bool) {
    if json_output {
        let _ = emit_json_pretty(&json!({
            "ok": false,
            "error": { "code": failure.code, "message": failure.message },
        }));
    } else {
        eprintln!("Error ({}): {}", failure.code, failure.message);
    }
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

/// `_build_payload`: gate, then the six reference sections.
fn build_payload(root: &Path, provenance: bool) -> Result<Map<String, Value>, StatusFailure> {
    assert_charter_sound(root)?;
    let mut payload = Map::new();
    payload.insert("result".to_owned(), json!("success"));
    payload.insert("charter_sync".to_owned(), collect_charter_sync(root)?);
    payload.insert("synthesis".to_owned(), collect_synthesis(root, provenance)?);
    payload.insert("org_layer".to_owned(), collect_org_layer());
    payload.insert(
        "governance_references".to_owned(),
        collect_governance_references(),
    );
    payload.insert("freshness".to_owned(), compute_freshness(root)?);
    payload.insert("activation".to_owned(), collect_activation(root));
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Soundness gate (`_charter_yaml_gate.assert_charter_yaml_sound`)
// ---------------------------------------------------------------------------

/// Refuse an unsound active charter before any collector reads it.
///
/// Soundness is the activation reader's rule (`load_charter_yaml_mapping`): the
/// `charter:` pointer of `.kittify/config.yaml` decides the file (default
/// `.kittify/charter/charter.yaml`); an absent default passes, a dangling
/// pointer / unparseable / non-mapping / empty mapping is
/// `ACTIVE_CHARTER_CONFIG_INVALID`. The refusal text is the reference's
/// status-shaped variant: the gate's "Nothing was written." line is dropped (a
/// read-only command writes nothing) and a dangling pointer gets the pointer
/// remedy.
fn assert_charter_sound(root: &Path) -> Result<(), StatusFailure> {
    let limits = Limits::default();
    let config = load_project_config(root, &limits)
        .map_err(|error| StatusFailure::coded(CHARTER_CONFIG_SHAPE_INVALID, error.to_string()))?;
    let pointer = config.charter_pointer.as_deref().map(|raw| {
        let candidate = PathBuf::from(raw);
        if candidate.is_absolute() {
            candidate
        } else {
            root.join(candidate)
        }
    });
    let charter_path = match &pointer {
        Some(path) => path.clone(),
        None => root.join(CHARTER_YAML),
    };
    let display = display_path(&charter_path, root);
    let dangling = pointer.is_some() && !charter_path.exists();
    if !charter_path.exists() {
        if dangling {
            return Err(soundness_refusal(
                &format!(
                    "{CONFIG_YAML_REL} '{CHARTER_POINTER_KEY}:' pointer names {display}, which does not exist."
                ),
                true,
            ));
        }
        // An absent default charter.yaml passes: the callers own that case.
        return Ok(());
    }
    let document = YamlDocument::open(&charter_path).map_err(|error| {
        let message = match error {
            DoctrineError::YamlRootNotMapping { .. } => {
                format!("{display} root must be a mapping.")
            }
            other => format!("Invalid YAML in {display}: {other}"),
        };
        soundness_refusal(&message, dangling)
    })?;
    let empty = document.mapping().map(BTreeMap::is_empty).unwrap_or(true);
    if empty {
        return Err(soundness_refusal(
            &format!("{display} is not a sound charter: it has no content (an empty mapping)."),
            dangling,
        ));
    }
    Ok(())
}

/// The reference's status-shaped gate refusal body.
fn soundness_refusal(message: &str, dangling_pointer: bool) -> StatusFailure {
    let raw = format!("{message}\n{NOTHING_WRITTEN}\n{SOUNDNESS_REMEDIATION}");
    let mut lines: Vec<&str> = raw
        .split('\n')
        .filter(|line| *line != NOTHING_WRITTEN)
        .collect();
    if dangling_pointer {
        lines.retain(|line| !line.starts_with("Remediation:"));
        lines.push(DANGLING_POINTER_REMEDIATION);
    }
    StatusFailure::coded(ACTIVE_CHARTER_CONFIG_INVALID, lines.join("\n"))
}

// ---------------------------------------------------------------------------
// charter_sync (`_collect_charter_sync_status`)
// ---------------------------------------------------------------------------

/// Read charter sync status without writing to disk.
///
/// Presence keys on the authoritative `charter.yaml`; the legacy
/// `charter.md`-only shape stays available (the reference's supported
/// pre-consolidation branch). Staleness is the retired
/// `charter.md`-vs-`metadata.yaml::charter_hash` comparison, kept because the
/// reference keeps it: absent `metadata.yaml` reports presence of
/// `charter.yaml` and empty hashes rather than a fabricated digest.
fn collect_charter_sync(root: &Path) -> Result<Value, StatusFailure> {
    let charter_yaml_path = root.join(CHARTER_YAML);
    let output_dir = charter_yaml_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join(CHARTER_DIR));
    let charter_md_path = output_dir.join(CHARTER_MD_FILENAME);
    let (charter_path, output_dir) = if charter_yaml_path.exists() {
        let header = if charter_md_path.exists() {
            charter_md_path
        } else {
            charter_yaml_path.clone()
        };
        (header, output_dir)
    } else if charter_md_path.exists() {
        (charter_md_path.clone(), output_dir)
    } else {
        // The reference's `_resolve_charter_path` failure text.
        return Ok(json!({
            "available": false,
            "error": format!(
                "Charter not found at {}\n  Run 'spec-kitty charter interview' to create one,\n  or 'spec-kitty upgrade' if migrating from an older version.",
                charter_md_path.display()
            ),
        }));
    };

    let metadata_path = output_dir.join(METADATA_FILENAME);
    let mut last_sync = Value::Null;
    let (stale, current_hash, stored_hash) = if metadata_path.exists() {
        let metadata = YamlDocument::open(&metadata_path).map_err(StatusFailure::internal)?;
        let text = std::fs::read_to_string(&charter_path).map_err(StatusFailure::internal)?;
        let current_hash = hash_content(&text);
        let stored_hash = metadata
            .get_str("charter_hash")
            .unwrap_or_default()
            .to_owned();
        last_sync = match metadata.get_str("timestamp_utc") {
            Some(value) => json!(value),
            None => match metadata.get_str("extracted_at") {
                Some(value) => json!(value),
                None => Value::Null,
            },
        };
        (current_hash != stored_hash, current_hash, stored_hash)
    } else {
        (!charter_yaml_path.exists(), String::new(), String::new())
    };

    let mut files = Vec::new();
    for filename in [CHARTER_YAML_FILENAME, CHARTER_MD_FILENAME] {
        let path = output_dir.join(filename);
        if path.exists() {
            let size = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            files.push(json!({
                "name": filename,
                "exists": true,
                "size_kb": size as f64 / 1024.0,
            }));
        } else {
            files.push(json!({
                "name": filename,
                "exists": false,
                "size_kb": 0.0,
            }));
        }
    }

    let library_docs = count_library_docs(&output_dir.join("library"));
    let status = if stale { "stale" } else { "synced" };
    Ok(json!({
        "available": true,
        "charter_path": display_path(&charter_path, root),
        "status": status,
        "current_hash": current_hash,
        "stored_hash": stored_hash,
        "last_sync": last_sync,
        "library_docs": library_docs,
        "files": files,
    }))
}

/// The reference's `len(list((output_dir / "library").glob("*.md")))`.
fn count_library_docs(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".md"))
        .count()
}

// ---------------------------------------------------------------------------
// synthesis (`_collect_synthesis_status`)
// ---------------------------------------------------------------------------

fn collect_synthesis(root: &Path, include_provenance: bool) -> Result<Value, StatusFailure> {
    let generated_inputs = collect_generated_inputs(root);
    let (manifest, parsed_manifest) = collect_manifest_status(root)?;
    let provenance = collect_provenance_status(root, parsed_manifest.as_ref(), include_provenance);
    let evidence = collect_evidence();
    let generation_state = generation_state_of(&manifest, &provenance, &generated_inputs);
    Ok(json!({
        "generation_state": generation_state,
        "generated_inputs": generated_inputs,
        "manifest": manifest,
        "provenance": provenance,
        "evidence": evidence,
    }))
}

/// `_collect_generated_input_status`: the project's generated directive/tactic/
/// styleguide inputs.
fn collect_generated_inputs(root: &Path) -> Value {
    let input_root = root.join(GENERATED_DIR);
    let counts = [
        (
            "directive",
            count_pattern(&input_root.join("directives"), "*.yaml"),
        ),
        (
            "tactic",
            count_pattern(&input_root.join("tactics"), "*.yaml"),
        ),
        (
            "styleguide",
            count_pattern(&input_root.join("styleguides"), "*.yaml"),
        ),
    ];
    let total: usize = counts.iter().map(|(_, count)| *count).sum();
    json!({
        "path": display_path(&input_root, root),
        "exists": input_root.exists(),
        "counts": {
            "directive": counts[0].1,
            "tactic": counts[1].1,
            "styleguide": counts[2].1,
        },
        "total": total,
    })
}

/// `_collect_manifest_status`: the synthesis manifest and its live-file counts.
fn collect_manifest_status(
    root: &Path,
) -> Result<(Value, Option<SynthesisManifest>), StatusFailure> {
    let manifest_path = root.join(SYNTHESIS_MANIFEST_PATH);
    let manifest_display = display_path(&manifest_path, root);
    let provenance_root = root.join(PROVENANCE_DIR);
    let live_artifact_count = count_live_artifacts(root);
    let live_provenance_count = count_pattern(&provenance_root, "*.yaml");

    if !manifest_path.exists() {
        let state = if live_artifact_count > 0 || live_provenance_count > 0 {
            "partial"
        } else {
            "missing"
        };
        return Ok((
            json!({
                "path": manifest_display,
                "exists": false,
                "state": state,
                "artifact_count": 0,
                "live_artifact_count": live_artifact_count,
                "live_provenance_count": live_provenance_count,
                "run_id": Value::Null,
                "created_at": Value::Null,
                "adapter_id": Value::Null,
                "adapter_version": Value::Null,
                "missing_provenance_paths": [],
                "error": Value::Null,
            }),
            None,
        ));
    }

    let manifest = match SynthesisManifest::load(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok((
                json!({
                    "path": manifest_display,
                    "exists": true,
                    "state": "invalid",
                    "artifact_count": 0,
                    "live_artifact_count": live_artifact_count,
                    "live_provenance_count": live_provenance_count,
                    "run_id": Value::Null,
                    "created_at": Value::Null,
                    "adapter_id": Value::Null,
                    "adapter_version": Value::Null,
                    "missing_provenance_paths": [],
                    "error": format!("manifest could not be parsed: {error}"),
                }),
                None,
            ));
        }
    };

    let (state, error) = match verify_manifest(&manifest, root) {
        Ok(()) => ("valid", Value::Null),
        Err(message) => ("invalid", json!(message)),
    };
    let missing_provenance_paths: Vec<String> = manifest
        .artifacts
        .iter()
        .filter(|entry| !root.join(&entry.provenance_path).exists())
        .map(|entry| entry.provenance_path.clone())
        .collect();
    let payload = json!({
        "path": manifest_display,
        "exists": true,
        "state": state,
        "artifact_count": manifest.artifacts.len(),
        "live_artifact_count": live_artifact_count,
        "live_provenance_count": live_provenance_count,
        "run_id": manifest.run_id.clone(),
        "created_at": manifest.created_at.clone(),
        "adapter_id": manifest.adapter_id.clone(),
        "adapter_version": manifest.adapter_version.clone(),
        "missing_provenance_paths": missing_provenance_paths,
        "error": error,
    });
    Ok((payload, Some(manifest)))
}

/// `manifest.verify`: every listed artifact must carry its repo-relative,
/// prefix-anchored path and match its recorded content hash on disk.
fn verify_manifest(manifest: &SynthesisManifest, root: &Path) -> Result<(), String> {
    for entry in &manifest.artifacts {
        let artifact_rel =
            validate_manifest_path(&entry.path, "manifest artifact path", PROJECT_PACK_ROOT)?;
        validate_manifest_path(
            &entry.provenance_path,
            "manifest provenance path",
            PROVENANCE_PATH_PREFIX,
        )?;
        let artifact_path = resolve_under_repo(root, &artifact_rel, "manifest artifact path")?;
        let integrity = || {
            format!(
                "Manifest integrity check failed: artifact '{}' listed in {} does not match its on-disk content hash. Re-run 'charter synthesize' to repair.",
                entry.path, SYNTHESIS_MANIFEST_PATH
            )
        };
        if !artifact_path.exists() {
            return Err(integrity());
        }
        let bytes = std::fs::read(&artifact_path).map_err(|_| integrity())?;
        if sk_kernel::content_digest::sha256_hex(&bytes) != entry.content_hash {
            return Err(integrity());
        }
    }
    Ok(())
}

/// `_validate_manifest_path`: safe, repo-relative, prefix-anchored.
fn validate_manifest_path(
    raw: &str,
    field_name: &str,
    required_prefix: &str,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    if path.is_absolute() || path.components().any(|part| part.as_os_str() == "..") {
        return Err(format!(
            "{field_name} must be repo-relative and stay under {required_prefix}: {raw}"
        ));
    }
    if !path.starts_with(Path::new(required_prefix)) {
        return Err(format!(
            "{field_name} must be under {required_prefix}: {raw}"
        ));
    }
    Ok(path)
}

/// `_resolve_under_repo`: a symlink may not escape the repository root.
fn resolve_under_repo(root: &Path, rel: &Path, field_name: &str) -> Result<PathBuf, String> {
    let repo_resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let joined = root.join(rel);
    let resolved = joined.canonicalize().unwrap_or(joined.clone());
    if !resolved.starts_with(&repo_resolved) {
        return Err(format!(
            "{field_name} resolves outside repository root: {}",
            rel.to_string_lossy()
        ));
    }
    Ok(joined)
}

/// The five live doctrine kinds the manifest status counts.
const LIVE_KINDS: [ArtifactKind; 5] = [
    ArtifactKind::Directive,
    ArtifactKind::Tactic,
    ArtifactKind::Styleguide,
    ArtifactKind::Procedure,
    ArtifactKind::AgentProfile,
];

/// `live_artifact_count`: files under the project pack root per live kind.
fn count_live_artifacts(root: &Path) -> usize {
    let pack_root = root.join(PROJECT_PACK_ROOT);
    LIVE_KINDS
        .iter()
        .map(|kind| count_rglob(&pack_root.join(kind.project_dir()), kind.glob_pattern()))
        .sum()
}

/// `_collect_provenance_status`.
///
/// Presence, the manifest coverage delta and the entries are the reference's
/// four channels. The sidecar *entry* model is not ported, so a project that
/// actually carries sidecars reports the gap in `warnings` (Source's own
/// degradation channel, which also drives `generation_state`
/// to `needs_attention`) instead of claiming parsed/adapter/corpus facts it
/// never read.
fn collect_provenance_status(
    root: &Path,
    manifest: Option<&SynthesisManifest>,
    _include_entries: bool,
) -> Value {
    let provenance_root = root.join(PROVENANCE_DIR);
    let paths = list_pattern(&provenance_root, "*.yaml");
    let visible_paths: BTreeSet<String> =
        paths.iter().map(|path| display_path(path, root)).collect();
    let manifest_paths: BTreeSet<String> = manifest
        .map(|manifest| {
            manifest
                .artifacts
                .iter()
                .map(|entry| entry.provenance_path.clone())
                .collect()
        })
        .unwrap_or_default();
    let missing_for_manifest: Vec<String> =
        manifest_paths.difference(&visible_paths).cloned().collect();
    let unread = !paths.is_empty();
    let warnings: Vec<String> = if unread || !missing_for_manifest.is_empty() {
        vec![PROVENANCE_GAP.to_owned()]
    } else {
        Vec::new()
    };
    let parsed_count = if unread { 0 } else { paths.len() };
    // `--provenance` can add nothing until the sidecar entry model is ported:
    // the reference prints nothing for an empty entry list, so an absent list
    // is the honest shape here.
    let entries: Vec<Value> = Vec::new();
    json!({
        "path": display_path(&provenance_root, root),
        "count": paths.len(),
        "parsed_count": parsed_count,
        "manifest_artifact_count": manifest_paths.len(),
        "missing_for_manifest_count": missing_for_manifest.len(),
        "missing_for_manifest": missing_for_manifest,
        "corpus_snapshot_ids": [],
        "adapters": [],
        "warnings": warnings,
        "entries": entries,
    })
}

/// `_summarize_evidence` — the collector is not ported.
fn collect_evidence() -> Value {
    json!({
        "warnings": [EVIDENCE_GAP],
        "code": Value::Null,
        "configured_urls": [],
        "configured_url_count": 0,
        "corpus_snapshot_id": Value::Null,
        "corpus_entry_count": 0,
    })
}

/// The reference's `generation_state` precedence.
fn generation_state_of(
    manifest: &Value,
    provenance: &Value,
    generated_inputs: &Value,
) -> &'static str {
    let manifest_state = manifest["state"].as_str().unwrap_or("missing");
    let missing_for_manifest = provenance["missing_for_manifest_count"]
        .as_u64()
        .unwrap_or(0);
    let provenance_warnings = provenance["warnings"]
        .as_array()
        .map(|warnings| !warnings.is_empty())
        .unwrap_or(false);
    let generated_total = generated_inputs["total"].as_u64().unwrap_or(0);
    if manifest_state == "valid" && missing_for_manifest == 0 && !provenance_warnings {
        "promoted"
    } else if matches!(manifest_state, "invalid" | "partial")
        || missing_for_manifest > 0
        || provenance_warnings
    {
        "needs_attention"
    } else if generated_total > 0 {
        "ready_for_validation"
    } else {
        "not_started"
    }
}

// ---------------------------------------------------------------------------
// org_layer / governance_references (collectors not ported)
// ---------------------------------------------------------------------------

/// `_collect_org_layer_status` — reports the gap in the reference's `errors`
/// channel rather than an empty-clean layer.
fn collect_org_layer() -> Value {
    json!({
        "has_built_in": true,
        "packs": [],
        "collision_warnings": [],
        "errors": [ORG_LAYER_GAP],
    })
}

/// `_collect_governance_reference_status` — the reference's own
/// `available: false` + `warnings` degradation.
fn collect_governance_references() -> Value {
    json!({
        "available": false,
        "references": [],
        "warnings": [GOVERNANCE_REFERENCES_GAP],
    })
}

// ---------------------------------------------------------------------------
// freshness (`compute_freshness`)
// ---------------------------------------------------------------------------

/// The three freshness sub-states (`charter -> bundle -> synthesized DRG`).
fn compute_freshness(root: &Path) -> Result<Value, StatusFailure> {
    let charter_source = charter_source_state(root);
    let synced_bundle = synced_bundle_state(root, &charter_source);
    let synthesized_drg = synthesized_drg_state(root, &synced_bundle);
    Ok(json!({
        "charter_source": charter_source,
        "synced_bundle": synced_bundle,
        "synthesized_drg": synthesized_drg,
    }))
}

fn freshness_substate(
    state: &str,
    last_change: Option<String>,
    remediation: Option<&str>,
    detail: Option<String>,
) -> Value {
    json!({
        "state": state,
        "last_change": last_change,
        "remediation": remediation,
        "detail": detail,
    })
}

fn substate_field<'a>(substate: &'a Value, key: &str) -> Option<&'a str> {
    substate.get(key).and_then(Value::as_str)
}

/// `_compute_charter_source`: presence/parseability of `charter.yaml`. Never
/// `stale` (the self-referential hash comparison is retired in the reference).
fn charter_source_state(root: &Path) -> Value {
    let charter_yaml_path = root.join(CHARTER_YAML);
    if !charter_yaml_path.exists() {
        let legacy = legacy_bundle_present(root);
        return freshness_substate(
            "missing",
            None,
            Some(if legacy {
                REMEDIATE_UPGRADE_YES
            } else {
                REMEDIATE_GENERATE_NO_INTERVIEW
            }),
            Some(missing_charter_source_detail(root)),
        );
    }
    let last_change = mtime_iso(&charter_yaml_path);
    let document = match YamlDocument::open(&charter_yaml_path) {
        Ok(document) => document,
        Err(_) => {
            return freshness_substate(
                "invalid",
                last_change,
                None,
                Some(
                    "charter.yaml exists but cannot be parsed; `charter generate` needs it to already parse — restore from VCS or hand-repair, then re-run"
                        .to_owned(),
                ),
            );
        }
    };
    if !is_supported_charter_bundle(&document) {
        return freshness_substate(
            "invalid",
            last_change,
            None,
            Some(
                "charter.yaml exists and parses as YAML but does not match the charter bundle contract (empty mapping, or a `schema_version` this install does not support); `charter generate` needs it to already be a valid bundle to merge into — restore from VCS or hand-repair, then re-run"
                    .to_owned(),
            ),
        );
    }
    freshness_substate("fresh", last_change, None, None)
}

/// `_is_supported_charter_bundle`: non-empty mapping with a supported
/// `major.minor.patch` `schema_version`.
fn is_supported_charter_bundle(document: &YamlDocument) -> bool {
    let Some(mapping) = document.mapping() else {
        return false;
    };
    if mapping.is_empty() {
        return false;
    }
    let Some(version) = document.get_str("schema_version") else {
        return false;
    };
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty() || !part.chars().all(|character| character.is_ascii_digit())
        })
    {
        return false;
    }
    parts[0] == SUPPORTED_SCHEMA_MAJOR
}

/// `_compute_synced_bundle`.
fn synced_bundle_state(root: &Path, charter_source: &Value) -> Value {
    let bundle_path = root.join(CHARTER_YAML);
    if !bundle_path.exists() {
        let legacy = legacy_bundle_present(root);
        return freshness_substate(
            "missing",
            None,
            Some(if legacy {
                REMEDIATE_UPGRADE_YES
            } else {
                REMEDIATE_GENERATE_NO_INTERVIEW
            }),
            substate_field(charter_source, "detail").map(str::to_owned),
        );
    }
    let last_change = mtime_iso(&bundle_path);
    if substate_field(charter_source, "state") != Some("fresh") {
        return freshness_substate(
            "stale",
            last_change,
            None,
            Some(
                "synced bundle mirrors charter_source, which is unparseable; the parse error must be fixed there first — restore charter.yaml from VCS or hand-repair, then re-run"
                    .to_owned(),
            ),
        );
    }
    let ledger_path = root.join(CHARTER_DIR).join(ACTIVATION_LEDGER_FILENAME);
    if ledger_path.exists() && YamlDocument::open(&ledger_path).is_err() {
        return freshness_substate(
            "stale",
            last_change,
            None,
            Some(format!(
                "{} is not a readable YAML mapping; restore it from VCS or hand-repair it, then re-run",
                display_path(&ledger_path, root)
            )),
        );
    }
    freshness_substate("fresh", last_change, None, None)
}

/// `_compute_synthesized_drg` plus its three sub-computers.
fn synthesized_drg_state(root: &Path, synced_bundle: &Value) -> Value {
    let manifest_path = root.join(SYNTHESIS_MANIFEST_PATH);
    let graph_path = root.join(PROJECT_PACK_ROOT).join("graph.yaml");
    let manifest = if manifest_path.exists() {
        SynthesisManifest::load(&manifest_path).ok()
    } else {
        None
    };
    let built_in_only = manifest
        .as_ref()
        .map(|manifest| manifest.built_in_only)
        .unwrap_or(false);
    let graph_exists = graph_path.exists();

    if built_in_only {
        return if graph_exists {
            freshness_substate(
                "built_in_only",
                mtime_iso(&graph_path),
                None,
                Some(
                    "stale graph residue: graph.yaml present but the synthesis manifest declares built_in_only; the manifest is authoritative, the residual graph.yaml is ignored"
                        .to_owned(),
                ),
            )
        } else {
            freshness_substate("built_in_only", mtime_iso(&manifest_path), None, None)
        };
    }

    if !graph_exists {
        let legacy_seed = root.join(PROJECT_PACK_ROOT).join("PROVENANCE.md");
        if looks_like_legacy_fresh_seed(&legacy_seed) {
            return freshness_substate(
                "built_in_only",
                mtime_iso(&legacy_seed),
                None,
                Some(
                    "legacy fresh-project seed marker; re-run `spec-kitty charter synthesize` to write synthesis-manifest.yaml"
                        .to_owned(),
                ),
            );
        }
        return freshness_substate("missing", None, Some(REMEDIATE_SYNTHESIZE), None);
    }

    let graph_mtime = mtime_iso(&graph_path);
    let bundle_fresh = substate_field(synced_bundle, "state") == Some("fresh")
        && synced_bundle
            .get("last_change")
            .is_some_and(|value| !value.is_null());
    if !bundle_fresh {
        return freshness_substate(
            "stale",
            graph_mtime,
            substate_field(synced_bundle, "remediation"),
            Some(
                substate_field(synced_bundle, "detail")
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "synced_bundle is {}; resolve it before synthesizing",
                            substate_field(synced_bundle, "state").unwrap_or("unknown")
                        )
                    }),
            ),
        );
    }
    let current_hash = compute_bundle_content_hash(root);
    let stored_hash = manifest
        .as_ref()
        .and_then(|manifest| manifest.bundle_content_hash.clone());
    match (stored_hash, current_hash) {
        (Some(stored), Some(current)) if stored == current => {
            freshness_substate("fresh", graph_mtime, None, None)
        }
        _ => freshness_substate("stale", graph_mtime, Some(REMEDIATE_SYNTHESIZE), None),
    }
}

/// `_looks_like_legacy_fresh_seed`.
fn looks_like_legacy_fresh_seed(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let lowered = text.to_lowercase();
    lowered.contains("fresh project seed")
        && lowered.contains("llm-authored yaml")
        && lowered.contains("built-in doctrine")
}

/// `_legacy_bundle_files_present`.
fn legacy_bundle_files_present(root: &Path) -> Vec<&'static str> {
    let charter_dir = root.join(CHARTER_DIR);
    LEGACY_BUNDLE_FILENAMES
        .iter()
        .copied()
        .filter(|name| charter_dir.join(name).exists())
        .collect()
}

fn legacy_bundle_present(root: &Path) -> bool {
    !legacy_bundle_files_present(root).is_empty()
}

/// `_missing_charter_source_detail`: F1 ("no charter at all") vs F2 (a legacy
/// bundle is present, just not folded into `charter.yaml`).
fn missing_charter_source_detail(root: &Path) -> String {
    let mut present: Vec<&str> = Vec::new();
    if root.join(CHARTER_DIR).join(CHARTER_MD_FILENAME).exists() {
        present.push(CHARTER_MD_FILENAME);
    }
    present.extend(legacy_bundle_files_present(root));
    if present.is_empty() {
        return "no charter.yaml and no legacy charter bundle files; this project has no charter at all"
            .to_owned();
    }
    let file_list = present.join("/");
    if present.len() == 1 {
        format!(
            "no charter.yaml, but a legacy charter bundle file ({file_list}) is present; this project has a charter, just not in the required form"
        )
    } else {
        format!(
            "no charter.yaml, but legacy charter bundle files ({file_list}) are present; this project has a charter, just not in the required form"
        )
    }
}

// ---------------------------------------------------------------------------
// activation (`_collect_activation_status`)
// ---------------------------------------------------------------------------

/// The local `activation` block.
///
/// Read only through the canonical ledger owner. A ledger read error degrades
/// this block only (the reference's behaviour); a malformed org charter becomes
/// `required_error` beside the lenient floor.
///
/// Source's `_status_collectors` reads with `report_stray=False`: this status
/// report carries stray keys as data and must not print warnings.
fn collect_activation(root: &Path) -> Value {
    let ledger = match sk_doctrine::load_activation_ledger_inspection(root) {
        Ok(ledger) => ledger,
        Err(error) => return activation_error_block(&activation_code(&error), &error.to_string()),
    };
    let (required, required_error, identity_failure) = collect_required_entries(root, &ledger);
    if let Some(message) = identity_failure {
        return activation_error_block(ACTIVATION_FAILED, &message);
    }
    let selected: Map<String, Value> = ledger
        .allow
        .iter()
        .map(|(kind, ids)| {
            (
                kind.plural().to_owned(),
                match ids {
                    Some(ids) => json!(ids),
                    None => Value::Null,
                },
            )
        })
        .collect();
    let deactivated: Map<String, Value> = ledger
        .deny
        .iter()
        .filter(|(_, ids)| !ids.is_empty())
        .map(|(kind, ids)| (kind.plural().to_owned(), json!(ids)))
        .collect();
    let mut block = Map::new();
    block.insert(
        "schema_version".to_owned(),
        json!(ACTIVATION_BLOCK_SCHEMA_VERSION),
    );
    block.insert("source".to_owned(), json!(source_token(ledger.source)));
    block.insert(
        "path".to_owned(),
        json!(ledger_relative_path(root, &ledger.path)),
    );
    block.insert("exists".to_owned(), json!(ledger.exists));
    block.insert("selected".to_owned(), Value::Object(selected));
    block.insert("deactivated".to_owned(), Value::Object(deactivated));
    block.insert(
        "mission_types".to_owned(),
        match &ledger.mission_type_activations {
            Some(ids) => json!(ids),
            None => Value::Null,
        },
    );
    block.insert(
        "activated_kinds".to_owned(),
        match &ledger.activated_kinds {
            Some(ids) => json!(ids),
            None => Value::Null,
        },
    );
    block.insert("required".to_owned(), Value::Array(required));
    if let Some(error) = required_error {
        block.insert("required_error".to_owned(), error);
    }
    Value::Object(block)
}

/// The org required floor as `{kind, id, in_force, packs}` rows, plus the first
/// strict-read error and any identity failure.
fn collect_required_entries(
    root: &Path,
    ledger: &ActivationLedger,
) -> (Vec<Value>, Option<Value>, Option<String>) {
    let mut floors: Vec<(
        ArtifactKind,
        BTreeMap<String, Vec<sk_doctrine::RequiringPack>>,
    )> = Vec::new();
    let mut required_error: Option<Value> = None;
    let mut identity: Option<IdentityIndex> = None;
    for kind in org_requirable_kinds() {
        if identity.is_none() {
            match IdentityIndex::from_project(root) {
                Ok(index) => identity = Some(index),
                Err(error) => return (Vec::new(), required_error, Some(error.to_string())),
            }
        }
        let index = identity.as_ref().expect("identity loaded above");
        let strict = read_org_required_provenance(root, kind, index, true);
        let floor = match strict {
            Ok(floor) => floor,
            Err(error) => {
                let coded = json!({
                    "code": activation_code(&error),
                    "message": error.to_string(),
                });
                if required_error.is_none() {
                    required_error = Some(coded);
                }
                match read_org_required_provenance(root, kind, index, false) {
                    Ok(floor) => floor,
                    Err(error) => return (Vec::new(), required_error, Some(error.to_string())),
                }
            }
        };
        if !floor.is_empty() {
            floors.push((kind, floor));
        }
    }

    let mut omitted: BTreeSet<(ArtifactKind, String)> = BTreeSet::new();
    for (kind, floor) in &floors {
        let Some(Some(allow)) = ledger.allow.get(kind) else {
            continue;
        };
        let Some(index) = identity.as_ref() else {
            continue;
        };
        let allowed = resolve_deny(*kind, allow.clone(), index).spellings();
        for stem in floor.keys() {
            let spellings = resolve_deny(*kind, [stem.clone()], index).spellings();
            if spellings.is_disjoint(&allowed) {
                omitted.insert((*kind, stem.clone()));
            }
        }
    }

    let mut entries = Vec::new();
    for (kind, floor) in &floors {
        for (stem, packs) in floor {
            let mut names: Vec<&str> = Vec::new();
            for pack in packs {
                if !names.contains(&pack.pack.as_str()) {
                    names.push(pack.pack.as_str());
                }
            }
            entries.push((
                kind.operator_token().to_owned(),
                stem.clone(),
                json!({
                    "kind": kind.operator_token(),
                    "id": stem,
                    "in_force": !kind.activatable() || !omitted.contains(&(*kind, stem.clone())),
                    "packs": names,
                }),
            ));
        }
    }
    entries.sort_by(|left, right| {
        (left.0.as_str(), left.1.as_str()).cmp(&(right.0.as_str(), right.1.as_str()))
    });
    (
        entries.into_iter().map(|(_, _, value)| value).collect(),
        required_error,
        None,
    )
}

fn activation_error_block(code: &str, message: &str) -> Value {
    json!({
        "schema_version": ACTIVATION_BLOCK_SCHEMA_VERSION,
        "error": { "code": code, "message": message },
    })
}

/// The reference's `(code, message)` mapping for an activation read error.
fn activation_code(error: &DoctrineError) -> String {
    if matches!(error, DoctrineError::Io { .. }) {
        return ACTIVATION_STATE_UNREADABLE.to_owned();
    }
    error
        .reference_code()
        .unwrap_or(ACTIVE_CHARTER_CONFIG_INVALID)
        .to_owned()
}

fn source_token(source: ActivationSource) -> &'static str {
    match source {
        ActivationSource::Ledger => "ledger",
        ActivationSource::Legacy => "legacy",
    }
}

fn ledger_relative_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) => relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

// ---------------------------------------------------------------------------
// Text rendering (human-readable only; JSON was emitted above)
// ---------------------------------------------------------------------------

fn render_text(payload: &Map<String, Value>, provenance: bool) {
    render_charter_sync(payload);
    render_synthesis(payload, provenance);
    render_org_layer(payload);
    render_freshness(payload);
}

/// A payload section (an absent section renders as an empty value, never a
/// panic).
fn section<'a>(payload: &'a Map<String, Value>, key: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    payload.get(key).unwrap_or(&NULL)
}

fn render_charter_sync(payload: &Map<String, Value>) {
    let sync = section(payload, "charter_sync");
    println!("Charter sync");
    if sync["available"].as_bool() != Some(true) {
        println!("Unavailable: {}", text(sync, "error"));
        return;
    }
    println!("Charter: {}", text(sync, "charter_path"));
    if text(sync, "status") == "stale" {
        println!("Status: STALE (modified since last sync)");
        let stored = text(sync, "stored_hash");
        if !stored.is_empty() {
            println!("Expected hash: {stored}");
        }
        println!("Current hash:  {}", text(sync, "current_hash"));
        let remediation = section(payload, "freshness")["charter_source"]["remediation"]
            .as_str()
            .unwrap_or_default();
        if !remediation.is_empty() {
            println!("\nRun: {remediation}");
        }
    } else {
        println!("Status: SYNCED");
        let last_sync = text(sync, "last_sync");
        if !last_sync.is_empty() {
            println!("Last sync: {last_sync}");
        }
        println!("Hash: {}", text(sync, "current_hash"));
    }
    println!(
        "Library docs: {}",
        sync["library_docs"].as_u64().unwrap_or(0)
    );
    if let Some(warnings) = section(payload, "governance_references")["warnings"].as_array()
        && !warnings.is_empty()
    {
        println!("\nGovernance reference warnings");
        for warning in warnings {
            println!("  - {}", warning.as_str().unwrap_or_default());
        }
    }
    println!("\nExtracted files:");
    if let Some(files) = sync["files"].as_array() {
        for file in files {
            let name = text(file, "name");
            let exists = file["exists"].as_bool().unwrap_or(false);
            let size = file["size_kb"].as_f64().unwrap_or(0.0);
            if exists {
                println!("  {name}  Y  {size:.1} KB");
            } else {
                println!("  {name}  N  -");
            }
        }
    }
    // Disclosed rendering debt: the extracted-file rows are plain lines.
}

fn render_synthesis(payload: &Map<String, Value>, provenance: bool) {
    let synthesis = section(payload, "synthesis");
    let generated = &synthesis["generated_inputs"];
    println!("\nSynthesis");
    println!(
        "Generation state: {}",
        text(synthesis, "generation_state").to_uppercase()
    );
    println!(
        "Generated inputs: {} directive, {} tactic, {} styleguide under {}",
        generated["counts"]["directive"].as_u64().unwrap_or(0),
        generated["counts"]["tactic"].as_u64().unwrap_or(0),
        generated["counts"]["styleguide"].as_u64().unwrap_or(0),
        text(generated, "path"),
    );
    render_manifest(&synthesis["manifest"]);
    render_evidence(&synthesis["evidence"]);
    render_provenance_summary(&synthesis["provenance"]);
    if provenance {
        let entries = &synthesis["provenance"]["entries"];
        match entries.as_array() {
            Some(entries) if !entries.is_empty() => {
                println!("\nProvenance entries:");
                for entry in entries {
                    println!(
                        "  {}  {}  {}  {} v{}  {}  {}",
                        text(entry, "kind"),
                        text(entry, "slug"),
                        text(entry, "artifact_urn"),
                        text(entry, "adapter_id"),
                        text(entry, "adapter_version"),
                        text(entry, "corpus_snapshot_id"),
                        text(entry, "evidence_bundle_hash"),
                    );
                }
            }
            _ => {}
        }
    }
}

fn render_manifest(manifest: &Value) {
    println!(
        "Manifest: {} ({})",
        text(manifest, "state").to_uppercase(),
        text(manifest, "path")
    );
    if manifest["exists"].as_bool() == Some(true) {
        let run_id = text(manifest, "run_id");
        let adapter_id = text(manifest, "adapter_id");
        let adapter_version = text(manifest, "adapter_version");
        if !run_id.is_empty() && !adapter_id.is_empty() && !adapter_version.is_empty() {
            println!("  Run: {run_id}  Adapter: {adapter_id} v{adapter_version}");
        }
        println!(
            "  Artifacts: {} (live doctrine files: {})",
            manifest["artifact_count"].as_u64().unwrap_or(0),
            manifest["live_artifact_count"].as_u64().unwrap_or(0),
        );
    }
    let error = text(manifest, "error");
    if !error.is_empty() {
        println!("  Error: {error}");
    }
    if let Some(paths) = manifest["missing_provenance_paths"].as_array()
        && !paths.is_empty()
    {
        println!("  Missing provenance paths:");
        for path in paths {
            println!("    - {}", path.as_str().unwrap_or_default());
        }
    }
}

fn render_evidence(evidence: &Value) {
    match evidence["code"].as_object() {
        Some(code) => {
            let languages = code["languages"].as_array().cloned().unwrap_or_default();
            let language = if languages.len() > 1 {
                languages
                    .iter()
                    .map(|value| value.as_str().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("+")
            } else {
                map_text(code, "primary_language").to_owned()
            };
            println!(
                "Evidence: stack={} (lang={}, frameworks={}, test_frameworks={})",
                map_text(code, "stack_id"),
                language,
                code["frameworks"].as_array().map(Vec::len).unwrap_or(0),
                code["test_frameworks"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or(0),
            );
        }
        None => println!("Evidence: code signals unavailable"),
    }
    let corpus = text(evidence, "corpus_snapshot_id");
    println!(
        "  Configured URLs: {}  Corpus snapshot: {}",
        evidence["configured_url_count"].as_u64().unwrap_or(0),
        if corpus.is_empty() { "(none)" } else { corpus },
    );
    for warning in evidence["warnings"].as_array().into_iter().flatten() {
        println!("  Warning: {}", warning.as_str().unwrap_or_default());
    }
}

fn render_provenance_summary(provenance: &Value) {
    println!(
        "Provenance: {} visible sidecar(s)",
        provenance["parsed_count"].as_u64().unwrap_or(0)
    );
    let manifest_artifact_count = provenance["manifest_artifact_count"].as_u64().unwrap_or(0);
    if manifest_artifact_count > 0 {
        let missing = provenance["missing_for_manifest_count"]
            .as_u64()
            .unwrap_or(0);
        println!(
            "  Manifest coverage: {}/{}",
            manifest_artifact_count.saturating_sub(missing),
            manifest_artifact_count
        );
    }
    if let Some(ids) = provenance["corpus_snapshot_ids"].as_array()
        && !ids.is_empty()
    {
        println!(
            "  Corpus snapshots: {}",
            ids.iter()
                .map(|value| value.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for warning in provenance["warnings"].as_array().into_iter().flatten() {
        println!("  Warning: {}", warning.as_str().unwrap_or_default());
    }
}

fn render_org_layer(payload: &Map<String, Value>) {
    let org_layer = section(payload, "org_layer");
    println!("\nOrganisation Layer");
    println!(
        "  built-in: {}",
        if org_layer["has_built_in"].as_bool() == Some(true) {
            "present"
        } else {
            "absent"
        }
    );
    match org_layer["packs"].as_array() {
        Some(packs) if !packs.is_empty() => {
            for pack in packs {
                println!(
                    "  org:{}: {} ({} nodes, {} edges)",
                    text(pack, "name"),
                    text(pack, "source_ref"),
                    pack["node_count"].as_u64().unwrap_or(0),
                    pack["edge_count"].as_u64().unwrap_or(0),
                );
            }
        }
        _ => println!("  org: (no packs configured)"),
    }
    for error in org_layer["errors"].as_array().into_iter().flatten() {
        println!("  Error: {}", error.as_str().unwrap_or_default());
    }
}

/// The reference's layer labels, in its own order.
const FRESHNESS_LAYERS: [(&str, &str); 3] = [
    ("Charter source ", "charter_source"),
    ("Synced bundle  ", "synced_bundle"),
    ("Synthesized DRG", "synthesized_drg"),
];

fn render_freshness(payload: &Map<String, Value>) {
    let freshness = section(payload, "freshness");
    println!("\nFreshness");
    for (label, key) in FRESHNESS_LAYERS {
        let sub = &freshness[key];
        let state = text(sub, "state").to_uppercase();
        let mut line = format!("  {label}: {state}");
        let last_change = text(sub, "last_change");
        if !last_change.is_empty() {
            line.push_str(&format!("  last_change={last_change}"));
        }
        println!("{line}");
        let detail = text(sub, "detail");
        if !detail.is_empty() {
            println!("    {detail}");
        }
        let remediation = text(sub, "remediation");
        if !remediation.is_empty() {
            println!("    Run: {remediation}");
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// A string field, or `""` (the reference's `or ""` formatting behaviour).
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// A string field of a mapping, or `""`.
fn map_text<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// The reference's `_display_path`: repo-relative POSIX text when possible.
fn display_path(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) => relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Every entry of `dir` whose name matches a leading-`*` pattern
/// (`*.yaml`), sorted, non-recursive (`glob`).
fn list_pattern(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let Some(suffix) = pattern.strip_prefix('*') else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(suffix))
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
}

/// `len(list(dir.glob(pattern)))` for a single `*` prefix pattern.
fn count_pattern(dir: &Path, pattern: &str) -> usize {
    list_pattern(dir, pattern).len()
}

/// `len(list(dir.rglob(pattern)))`: the same name pattern, recursively.
fn count_rglob(dir: &Path, pattern: &str) -> usize {
    let Some(suffix) = pattern.strip_prefix('*') else {
        return 0;
    };
    let mut pending = vec![dir.to_path_buf()];
    let mut count = 0usize;
    while let Some(current) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(path);
            } else if entry.file_name().to_string_lossy().ends_with(suffix) {
                count += 1;
            }
        }
    }
    count
}

/// The reference's `from_epoch(mtime).isoformat()` (`+00:00`, microseconds only
/// when non-zero).
fn mtime_iso(path: &Path) -> Option<String> {
    let modified: SystemTime = std::fs::metadata(path).ok()?.modified().ok()?;
    let epoch = match modified.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs_f64(),
        Err(error) => -error.duration().as_secs_f64(),
    };
    let instant = from_epoch(epoch).ok()?;
    Some(isoformat(instant))
}

/// Python `datetime.isoformat()` for an aware-UTC instant.
fn isoformat(instant: chrono::DateTime<chrono::Utc>) -> String {
    let base = format_stamp(instant, "%Y-%m-%dT%H:%M:%S");
    let micros = instant.timestamp_subsec_micros();
    if micros == 0 {
        format!("{base}+00:00")
    } else {
        format!("{base}.{micros:06}+00:00")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soundness_refusal_drops_the_wrote_nothing_line() {
        let failure = soundness_refusal("bad.yaml root must be a mapping.", false);
        assert_eq!(failure.code, ACTIVE_CHARTER_CONFIG_INVALID);
        assert!(!failure.message.contains(NOTHING_WRITTEN));
        assert!(failure.message.contains(SOUNDNESS_REMEDIATION));
    }

    #[test]
    fn a_dangling_pointer_gets_the_pointer_remedy() {
        let failure = soundness_refusal("pointer names missing.yaml", true);
        assert!(failure.message.contains(DANGLING_POINTER_REMEDIATION));
        assert!(!failure.message.contains(SOUNDNESS_REMEDIATION));
    }

    #[test]
    fn the_bundle_contract_needs_a_supported_major() {
        let supported =
            YamlDocument::parse("schema_version: 2.0.0\ncatalog: {}\n", Path::new("<test>"))
                .unwrap();
        assert!(is_supported_charter_bundle(&supported));
        let wrong_major =
            YamlDocument::parse("schema_version: 1.0.0\ncatalog: {}\n", Path::new("<test>"))
                .unwrap();
        assert!(!is_supported_charter_bundle(&wrong_major));
        let empty = YamlDocument::parse("{}\n", Path::new("<test>")).unwrap();
        assert!(!is_supported_charter_bundle(&empty));
    }

    #[test]
    fn manifest_paths_must_be_relative_and_anchored() {
        assert!(validate_manifest_path("a", "f", PROJECT_PACK_ROOT).is_err());
        assert!(validate_manifest_path("/abs", "f", PROJECT_PACK_ROOT).is_err());
        assert!(
            validate_manifest_path(".kittify/charter-packs/../x", "f", PROJECT_PACK_ROOT).is_err()
        );
        assert!(
            validate_manifest_path(
                ".kittify/charter-packs/directive/001-x.directive.yaml",
                "f",
                PROJECT_PACK_ROOT
            )
            .is_ok()
        );
    }

    #[test]
    fn recursive_and_flat_counts_disagree_only_by_depth() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("nested/deeper")).unwrap();
        std::fs::write(root.join("a.yaml"), "a: 1\n").unwrap();
        std::fs::write(root.join("nested/b.yaml"), "b: 1\n").unwrap();
        std::fs::write(root.join("nested/deeper/c.yaml"), "c: 1\n").unwrap();
        assert_eq!(count_pattern(root, "*.yaml"), 1);
        assert_eq!(count_rglob(root, "*.yaml"), 3);
    }
}
