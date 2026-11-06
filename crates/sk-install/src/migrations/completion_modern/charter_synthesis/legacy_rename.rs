//! `4.0.0rc8_project_intent_directive_rename`: converge a legacy
//! `mission-type-scope-directive` onto `project-intent-directive`.
//!
//! Synthesis used to key the always-synthesized directive on the slug
//! `mission-type-scope-directive`; it is now named `project-intent-directive` and
//! no longer linked to a mission type. One helper moves an existing project over,
//! and both `charter synthesize`/`resynthesize` and this upgrade step call it, so
//! a second `PROJECT_00N` file can never appear.
//!
//! What moves, when present:
//!
//! * the artifact file under the project charter pack and the harness input under
//!   `.kittify/charter/generated/directives/` (`NNN-<slug>.directive.yaml`);
//! * the provenance sidecar: its `artifact_slug` and file name, and any recorded
//!   path (`source_input_ids` / `source_section`);
//! * the synthesis-manifest entry (`path`, `slug`, `provenance_path`), with
//!   `manifest_hash` recomputed through the one finalizer;
//! * the overlay node label, and the dead `requires --> mission_type:*` edge;
//! * stem tokens in the resolved activation write target: the allow lists, and —
//!   on the activation ledger only — the deny lists too, so an exclusion the user
//!   recorded by the old stem keeps applying;
//! * then the manifest's bundle hash is restamped under the synthesis lock.
//!
//! The declared `PROJECT_00N` id, its `directive:PROJECT_00N` URN, the interview
//! section label `mission_type` and the artifact body are never touched. The
//! helper is idempotent: every step acts only on legacy state, so a second run
//! finishes an interrupted one and then does nothing. It runs in two phases: a
//! read-only preflight that refuses (changing nothing) on an unreadable input or
//! a target collision, then the writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};
use sk_charter::synthesis::{
    PROJECT_GRAPH_FILENAME, SYNTHESIS_MANIFEST_PATH, SynthesisManifest, artifact_filename,
    pack_kind_subdir, project_pack_path, provenance_path_for,
};
use sk_charter::{canonical_yaml, compute_bundle_content_hash};
use sk_doctrine::{
    ActivationSource, DenyListChoice, acquire_synthesis_lock, activation_ledger_path,
    apply_ledger_write, ledger_keys, load_activation_ledger, prepare_ledger_write,
    resolve_activation_source,
};

use crate::error::InstallError;
use crate::migrations::completion_legacy::paths::{relative_display, write_bytes, write_text};
use crate::upgrade::{MigrationOutcome, NativeMigration};
use crate::yaml_edit::CstDocument;

/// The retired directive slug.
pub const LEGACY_SLUG: &str = "mission-type-scope-directive";
/// The directive slug synthesis now emits.
pub const NEW_SLUG: &str = "project-intent-directive";
/// The overlay node label the new slug carries.
const NEW_LABEL: &str = "Project Intent Directive";
/// Labels the legacy directive's overlay node carried.
const LEGACY_LABELS: [&str; 2] = ["Mission Type Directive", "Mission Type Scope Directive"];
const GENERATED_DIRECTIVES: &str = ".kittify/charter/generated/directives";
const PROVENANCE_DIR: &str = ".kittify/charter/provenance";
const CONFIG_REL: &str = ".kittify/config.yaml";
const DIRECTIVE_URN_PREFIX: &str = "directive:PROJECT_";
const MISSION_TYPE_URN_PREFIX: &str = "mission_type:";

/// `(action lines, hard refusals, soft refusals)` of one preflight step.
#[derive(Default)]
struct Finding {
    actions: Vec<String>,
    refusals: Vec<String>,
    soft_refusals: Vec<String>,
}

fn target_filename(number: &str) -> Result<String, InstallError> {
    artifact_filename("directive", NEW_SLUG, Some(&format!("PROJECT_{number}")))
        .map_err(charter_error)
}

/// `text` with every legacy artifact file name replaced by its target name.
fn rename_text(text: &str) -> String {
    let suffix = format!("-{LEGACY_SLUG}.directive.yaml");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find(&suffix) {
        let (before, after) = rest.split_at(position);
        let digits_start = before
            .rfind(|character: char| !character.is_ascii_digit())
            .map(|index| index + 1)
            .unwrap_or(0);
        let digits = &before[digits_start..];
        if digits.is_empty() {
            out.push_str(&before[..position]);
            out.push_str(&suffix);
            rest = &rest[position + suffix.len()..];
            continue;
        }
        out.push_str(&before[..digits_start]);
        match target_filename(digits) {
            Ok(name) => out.push_str(&name),
            Err(_) => {
                out.push_str(&before[digits_start..]);
                out.push_str(&suffix);
            }
        }
        rest = &after[suffix.len()..];
    }
    out.push_str(rest);
    out
}

/// The target stem for a legacy stem token, or `None` when `token` is not one.
fn rename_stem(token: &str) -> Option<String> {
    let stem = token.strip_suffix(&format!("-{LEGACY_SLUG}"))?;
    if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    target_filename(stem).ok().map(|name| {
        name.strip_suffix(".directive.yaml")
            .unwrap_or(&name)
            .to_string()
    })
}

fn charter_error(error: sk_charter::CharterError) -> InstallError {
    InstallError::UpgradeRefused {
        detail: error.to_string(),
    }
}

fn doctrine_error(error: sk_doctrine::DoctrineError) -> InstallError {
    InstallError::UpgradeRefused {
        detail: error.to_string(),
    }
}

fn rel(project_root: &Path, path: &Path) -> String {
    relative_display(project_root, path)
}

fn unreadable(project_root: &Path, path: &Path, detail: &str) -> String {
    format!("{} is unreadable ({detail})", rel(project_root, path))
}

fn read_value(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let value: Value = serde_yaml_ng::from_str(&text).map_err(|error| error.to_string())?;
    if !value.is_mapping() {
        return Err("top-level YAML document must be a mapping".to_string());
    }
    Ok(value)
}

fn write_value(path: &Path, value: &Value) -> Result<(), InstallError> {
    write_bytes(path, &canonical_yaml(value)).map(|_| ())
}

// ---------------------------------------------------------------------------
// Step: artifact file and harness input
// ---------------------------------------------------------------------------

fn file_dirs(project_root: &Path) -> Result<Vec<PathBuf>, InstallError> {
    Ok(vec![
        project_pack_path(
            project_root,
            &[pack_kind_subdir("directive").map_err(charter_error)?],
        ),
        project_root.join(GENERATED_DIRECTIVES),
    ])
}

fn file_moves(
    project_root: &Path,
) -> Result<Vec<(PathBuf, PathBuf, String, String)>, InstallError> {
    let suffix = format!("-{LEGACY_SLUG}.directive.yaml");
    let mut moves = Vec::new();
    for directory in file_dirs(project_root)? {
        if !directory.is_dir() {
            continue;
        }
        let mut names: Vec<String> = std::fs::read_dir(&directory)
            .map_err(|error| InstallError::io("read directory", &directory, error))?
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(&suffix))
            .collect();
        names.sort();
        for name in names {
            let stem = &name[..name.len() - suffix.len()];
            if stem.is_empty() || !stem.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            moves.push((
                directory.join(&name),
                directory.join(target_filename(stem)?),
                stem.to_string(),
                name.clone(),
            ));
        }
    }
    Ok(moves)
}

fn check_files(project_root: &Path) -> Result<Finding, InstallError> {
    let mut finding = Finding::default();
    for (source, destination, _stem, _name) in file_moves(project_root)? {
        if destination.exists() {
            finding.refusals.push(format!(
                "{} already exists, so {} cannot move onto it",
                rel(project_root, &destination),
                rel(project_root, &source)
            ));
        } else {
            finding.actions.push(format!(
                "move {} -> {}",
                rel(project_root, &source),
                rel(project_root, &destination)
            ));
        }
    }
    Ok(finding)
}

fn apply_files(project_root: &Path) -> Result<(), InstallError> {
    for (source, destination, _stem, _name) in file_moves(project_root)? {
        std::fs::rename(&source, &destination)
            .map_err(|error| InstallError::io("move", &source, error))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step: provenance sidecars
// ---------------------------------------------------------------------------

fn rewrite_provenance(data: &Value) -> Value {
    let Some(mapping) = data.as_mapping() else {
        return data.clone();
    };
    let mut updated = mapping.clone();
    if mapping
        .get(Value::String("artifact_slug".to_string()))
        .and_then(Value::as_str)
        == Some(LEGACY_SLUG)
    {
        updated.insert(
            Value::String("artifact_slug".to_string()),
            Value::String(NEW_SLUG.to_string()),
        );
    }
    if let Some(Value::String(section)) = mapping.get(Value::String("source_section".to_string())) {
        updated.insert(
            Value::String("source_section".to_string()),
            Value::String(rename_text(section)),
        );
    }
    if let Some(Value::Sequence(inputs)) =
        mapping.get(Value::String("source_input_ids".to_string()))
    {
        let rewritten: Vec<Value> = inputs
            .iter()
            .map(|item| match item {
                Value::String(text) => Value::String(rename_text(text)),
                other => other.clone(),
            })
            .collect();
        updated.insert(
            Value::String("source_input_ids".to_string()),
            Value::Sequence(rewritten),
        );
    }
    Value::Mapping(updated)
}

struct ProvenancePlan {
    source: PathBuf,
    destination: PathBuf,
    data: Value,
    error: Option<String>,
}

fn provenance_plan(project_root: &Path) -> Result<Vec<ProvenancePlan>, InstallError> {
    let directory = project_root.join(PROVENANCE_DIR);
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = std::fs::read_dir(&directory)
        .map_err(|error| InstallError::io("read directory", &directory, error))?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("directive-") && name.ends_with(".yaml"))
        .collect();
    names.sort();
    let mut plan = Vec::new();
    for name in names {
        let path = directory.join(&name);
        let data = match read_value(&path) {
            Ok(data) => data,
            Err(detail) => {
                if name.contains(LEGACY_SLUG) {
                    plan.push(ProvenancePlan {
                        source: path.clone(),
                        destination: path.clone(),
                        data: Value::Mapping(Mapping::new()),
                        error: Some(unreadable(project_root, &path, &detail)),
                    });
                }
                continue;
            }
        };
        let updated = rewrite_provenance(&data);
        if updated == data {
            continue;
        }
        let destination = if data
            .get(Value::String("artifact_slug".to_string()))
            .and_then(Value::as_str)
            == Some(LEGACY_SLUG)
        {
            project_root.join(provenance_path_for("directive", NEW_SLUG))
        } else {
            path.clone()
        };
        plan.push(ProvenancePlan {
            source: path,
            destination,
            data: updated,
            error: None,
        });
    }
    Ok(plan)
}

/// Whether `destination` already holds exactly `data` (an interrupted move).
fn is_resumable_move(destination: &Path, data: &Value) -> bool {
    read_value(destination).is_ok_and(|existing| &existing == data)
}

fn check_provenance(project_root: &Path) -> Result<Finding, InstallError> {
    let mut finding = Finding::default();
    for plan in provenance_plan(project_root)? {
        if let Some(error) = plan.error {
            finding.refusals.push(error);
        } else if plan.destination != plan.source
            && plan.destination.exists()
            && !is_resumable_move(&plan.destination, &plan.data)
        {
            finding.refusals.push(format!(
                "{} already exists, so {} cannot move onto it",
                rel(project_root, &plan.destination),
                rel(project_root, &plan.source)
            ));
        } else {
            let verb = if plan.destination == plan.source {
                "rewrite"
            } else {
                "move and rewrite"
            };
            let source = rel(project_root, &plan.source);
            finding.actions.push(if plan.destination == plan.source {
                format!("{verb} provenance {source}")
            } else {
                format!(
                    "{verb} provenance {source} -> {}",
                    rel(project_root, &plan.destination)
                )
            });
        }
    }
    Ok(finding)
}

fn apply_provenance(project_root: &Path) -> Result<(), InstallError> {
    for plan in provenance_plan(project_root)? {
        write_value(&plan.destination, &plan.data)?;
        if plan.destination != plan.source {
            std::fs::remove_file(&plan.source)
                .map_err(|error| InstallError::io("remove", &plan.source, error))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step: synthesis manifest
// ---------------------------------------------------------------------------

fn rename_manifest_entries(
    artifacts: &[sk_charter::synthesis::ManifestArtifactEntry],
) -> Vec<sk_charter::synthesis::ManifestArtifactEntry> {
    let legacy_provenance = provenance_path_for("directive", LEGACY_SLUG);
    artifacts
        .iter()
        .map(|entry| {
            let mut renamed = entry.clone();
            if entry.kind == "directive" {
                if entry.slug == LEGACY_SLUG {
                    renamed.slug = NEW_SLUG.to_string();
                }
                let path = rename_text(&entry.path);
                if path != entry.path {
                    renamed.path = path;
                }
                if entry.provenance_path == legacy_provenance {
                    renamed.provenance_path = provenance_path_for("directive", NEW_SLUG);
                }
            }
            renamed
        })
        .collect()
}

fn manifest_collisions(
    artifacts: &[sk_charter::synthesis::ManifestArtifactEntry],
    renamed: &[sk_charter::synthesis::ManifestArtifactEntry],
) -> Vec<String> {
    if renamed == artifacts {
        return Vec::new();
    }
    let mut reasons = Vec::new();
    for field in ["slug", "path"] {
        let values: Vec<&str> = renamed
            .iter()
            .filter(|entry| entry.kind == "directive")
            .map(|entry| {
                if field == "slug" {
                    entry.slug.as_str()
                } else {
                    entry.path.as_str()
                }
            })
            .collect();
        let distinct: std::collections::BTreeSet<&str> = values.iter().copied().collect();
        if distinct.len() != values.len() {
            reasons.push(format!(
                "the synthesis manifest would list two directive entries with the same {field} after the rename"
            ));
        }
    }
    reasons
}

fn check_manifest(project_root: &Path) -> Result<Finding, InstallError> {
    let mut finding = Finding::default();
    let path = project_root.join(SYNTHESIS_MANIFEST_PATH);
    if !path.is_file() {
        return Ok(finding);
    }
    let manifest = match SynthesisManifest::load(&path) {
        Ok(manifest) => manifest,
        Err(error) => {
            finding
                .soft_refusals
                .push(unreadable(project_root, &path, &error.to_string()));
            return Ok(finding);
        }
    };
    let renamed = rename_manifest_entries(&manifest.artifacts);
    if renamed != manifest.artifacts {
        finding.actions.push(format!(
            "rewrite {} (path/slug/provenance_path, manifest_hash)",
            rel(project_root, &path)
        ));
        finding
            .refusals
            .extend(manifest_collisions(&manifest.artifacts, &renamed));
    }
    Ok(finding)
}

fn apply_manifest(project_root: &Path) -> Result<(), InstallError> {
    let path = project_root.join(SYNTHESIS_MANIFEST_PATH);
    if !path.is_file() {
        return Ok(());
    }
    let manifest = SynthesisManifest::load(&path).map_err(charter_error)?;
    let renamed = rename_manifest_entries(&manifest.artifacts);
    if renamed == manifest.artifacts {
        return Ok(());
    }
    let mut updated = manifest;
    updated.artifacts = renamed;
    let finalized = updated.finalize();
    write_bytes(&path, &finalized.to_canonical_bytes()).map(|_| ())
}

// ---------------------------------------------------------------------------
// Step: overlay graph
// ---------------------------------------------------------------------------

fn is_dead_edge(edge: &Value) -> bool {
    let Some(mapping) = edge.as_mapping() else {
        return false;
    };
    let relation = mapping
        .get(Value::String("relation".to_string()))
        .and_then(Value::as_str);
    let source = mapping
        .get(Value::String("source".to_string()))
        .and_then(Value::as_str);
    let target = mapping
        .get(Value::String("target".to_string()))
        .and_then(Value::as_str);
    relation == Some("requires")
        && source.is_some_and(|value| value.starts_with(DIRECTIVE_URN_PREFIX))
        && target.is_some_and(|value| value.starts_with(MISSION_TYPE_URN_PREFIX))
}

fn is_legacy_node(node: &Value) -> bool {
    let Some(mapping) = node.as_mapping() else {
        return false;
    };
    mapping
        .get(Value::String("kind".to_string()))
        .and_then(Value::as_str)
        == Some("directive")
        && mapping
            .get(Value::String("label".to_string()))
            .and_then(Value::as_str)
            .is_some_and(|label| LEGACY_LABELS.contains(&label))
}

fn rewrite_overlay(data: &Value) -> (Value, Vec<String>) {
    let mut actions = Vec::new();
    let mapping = data.as_mapping().cloned().unwrap_or_default();
    let mut updated = mapping.clone();
    if let Some(Value::Sequence(nodes)) = mapping.get(Value::String("nodes".to_string()))
        && nodes.iter().any(is_legacy_node)
    {
        let nodes: Vec<Value> = nodes
            .iter()
            .map(|node| {
                if !is_legacy_node(node) {
                    return node.clone();
                }
                let mut relabelled = node.as_mapping().cloned().unwrap_or_default();
                relabelled.insert(
                    Value::String("label".to_string()),
                    Value::String(NEW_LABEL.to_string()),
                );
                Value::Mapping(relabelled)
            })
            .collect();
        updated.insert(Value::String("nodes".to_string()), Value::Sequence(nodes));
        actions.push(format!(
            "relabel the directive overlay node as '{NEW_LABEL}'"
        ));
    }
    if let Some(Value::Sequence(edges)) = mapping.get(Value::String("edges".to_string()))
        && edges.iter().any(is_dead_edge)
    {
        let kept: Vec<Value> = edges
            .iter()
            .filter(|edge| !is_dead_edge(edge))
            .cloned()
            .collect();
        updated.insert(Value::String("edges".to_string()), Value::Sequence(kept));
        actions.push("drop the dead 'requires --> mission_type:*' overlay edge".to_string());
    }
    (Value::Mapping(updated), actions)
}

fn check_overlay(project_root: &Path) -> Result<Finding, InstallError> {
    let mut finding = Finding::default();
    let path = project_pack_path(project_root, &[PROJECT_GRAPH_FILENAME]);
    if !path.is_file() {
        return Ok(finding);
    }
    match read_value(&path) {
        Ok(data) => {
            let (_updated, actions) = rewrite_overlay(&data);
            finding.actions.extend(
                actions
                    .into_iter()
                    .map(|action| format!("{action} in {}", rel(project_root, &path))),
            );
        }
        Err(detail) => finding
            .soft_refusals
            .push(unreadable(project_root, &path, &detail)),
    }
    Ok(finding)
}

fn apply_overlay(project_root: &Path) -> Result<(), InstallError> {
    let path = project_pack_path(project_root, &[PROJECT_GRAPH_FILENAME]);
    if !path.is_file() {
        return Ok(());
    }
    let data = read_value(&path).map_err(|detail| InstallError::UpgradeRefused { detail })?;
    let (updated, actions) = rewrite_overlay(&data);
    if !actions.is_empty() {
        write_value(&path, &updated)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step: stem tokens in the activation write target
// ---------------------------------------------------------------------------

fn renamed_tokens(tokens: &[Value]) -> Vec<Value> {
    let mut renamed: Vec<Value> = Vec::new();
    for token in tokens {
        let value = match token.as_str().and_then(rename_stem) {
            Some(new) => Value::String(new),
            None => token.clone(),
        };
        if !renamed.contains(&value) {
            renamed.push(value);
        }
    }
    renamed
}

fn holds_legacy_stem(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Sequence(tokens))
        if tokens.iter().any(|token| token.as_str().and_then(rename_stem).is_some()))
}

fn legacy_token_keys(data: &Mapping, include_deny: bool) -> Vec<String> {
    ledger_keys()
        .into_iter()
        .filter(|key| include_deny || !key.starts_with("deactivated_"))
        .filter(|key| holds_legacy_stem(data.get(Value::String(key.clone()))))
        .collect()
}

/// The activation surface a write targets: the ledger, or the legacy file.
enum ActivationTarget {
    Ledger { keys: Vec<String> },
    Legacy { path: PathBuf, keys: Vec<String> },
}

fn activation_target(project_root: &Path) -> Result<ActivationTarget, InstallError> {
    let resolved = resolve_activation_source(project_root).map_err(doctrine_error)?;
    let include_deny = resolved.source() == ActivationSource::Ledger;
    match resolved.source() {
        ActivationSource::Ledger => {
            let ledger = load_activation_ledger(project_root).map_err(doctrine_error)?;
            let data = ledger.raw();
            let mut mapping = Mapping::new();
            for (key, value) in &data {
                if let Some(ids) = value {
                    mapping.insert(
                        Value::String(key.clone()),
                        Value::Sequence(ids.iter().cloned().map(Value::String).collect()),
                    );
                }
            }
            Ok(ActivationTarget::Ledger {
                keys: legacy_token_keys(&mapping, include_deny),
            })
        }
        ActivationSource::Legacy => {
            let path = resolved
                .pointer
                .clone()
                .filter(|_| resolved.pointer_exists)
                .unwrap_or_else(|| project_root.join(CONFIG_REL));
            let data = read_value(&path).map_err(|detail| InstallError::UpgradeRefused {
                detail: format!("the activation write target is unreadable ({detail})"),
            })?;
            let mapping = data.as_mapping().cloned().unwrap_or_default();
            Ok(ActivationTarget::Legacy {
                path,
                keys: legacy_token_keys(&mapping, include_deny),
            })
        }
    }
}

fn check_tokens(project_root: &Path) -> Result<Finding, InstallError> {
    let mut finding = Finding::default();
    match activation_target(project_root) {
        Ok(ActivationTarget::Ledger { keys }) | Ok(ActivationTarget::Legacy { keys, .. }) => {
            for key in keys {
                finding.actions.push(format!(
                    "rewrite '{key}' stem token(s) in {}",
                    rel(project_root, &activation_ledger_path(project_root))
                ));
            }
        }
        Err(error) => finding.soft_refusals.push(format!(
            "the activation write target is unreadable ({error})"
        )),
    }
    Ok(finding)
}

fn apply_tokens(project_root: &Path) -> Result<(), InstallError> {
    match activation_target(project_root)? {
        ActivationTarget::Ledger { keys } => {
            if keys.is_empty() {
                return Ok(());
            }
            let ledger = load_activation_ledger(project_root).map_err(doctrine_error)?;
            let mut values: BTreeMap<String, Option<Vec<String>>> = BTreeMap::new();
            for key in &keys {
                let current: Vec<String> =
                    ledger.raw().get(key).cloned().flatten().unwrap_or_default();
                let tokens: Vec<Value> = current.into_iter().map(Value::String).collect();
                values.insert(
                    key.clone(),
                    Some(
                        renamed_tokens(&tokens)
                            .into_iter()
                            .filter_map(|token| token.as_str().map(str::to_string))
                            .collect(),
                    ),
                );
            }
            let deny_change = keys
                .iter()
                .any(|key| key.starts_with("deactivated_"))
                .then_some(DenyListChoice::Rename);
            let prepared = prepare_ledger_write(project_root, &values, &[], deny_change)
                .map_err(doctrine_error)?;
            apply_ledger_write(prepared).map_err(doctrine_error)?;
            Ok(())
        }
        ActivationTarget::Legacy { path, keys } => {
            if keys.is_empty() {
                return Ok(());
            }
            let text = std::fs::read_to_string(&path)
                .map_err(|error| InstallError::io("read", &path, error))?;
            let document = CstDocument::parse(&path, &text)?;
            let root = document.root();
            for key in &keys {
                let Some(current) = root.get_string_list(key) else {
                    continue;
                };
                let tokens: Vec<Value> = current.into_iter().map(Value::String).collect();
                let renamed: Vec<String> = renamed_tokens(&tokens)
                    .into_iter()
                    .filter_map(|token| token.as_str().map(str::to_string))
                    .collect();
                if renamed != root.get_string_list(key).unwrap_or_default() {
                    root.set_string_list(key, &renamed);
                }
            }
            let rendered = sk_doctrine::YamlDocument::parse(&document.text(), &path)
                .and_then(|document| document.to_source_text())
                .map_err(|error| InstallError::UpgradeRefused {
                    detail: error.to_string(),
                })?;
            write_text(&path, &rendered).map(|_| ())
        }
    }
}

// ---------------------------------------------------------------------------
// The helper
// ---------------------------------------------------------------------------

/// One preflight/apply step pair.
struct Step {
    check: fn(&Path) -> Result<Finding, InstallError>,
    apply: fn(&Path) -> Result<(), InstallError>,
}

const STEPS: [Step; 5] = [
    Step {
        check: check_files,
        apply: apply_files,
    },
    Step {
        check: check_provenance,
        apply: apply_provenance,
    },
    Step {
        check: check_manifest,
        apply: apply_manifest,
    },
    Step {
        check: check_overlay,
        apply: apply_overlay,
    },
    Step {
        check: check_tokens,
        apply: apply_tokens,
    },
];

/// The rename's read-only preflight: every action, or the reason it must refuse.
pub fn preflight(project_root: &Path) -> Result<Vec<String>, InstallError> {
    let mut actions = Vec::new();
    let mut refusals = Vec::new();
    let mut soft = Vec::new();
    for step in &STEPS {
        let finding = (step.check)(project_root)?;
        actions.extend(finding.actions);
        refusals.extend(finding.refusals);
        soft.extend(finding.soft_refusals);
    }
    if !actions.is_empty() {
        refusals.extend(soft);
    }
    if !refusals.is_empty() {
        // The reference's `LegacyRenameRefusedError.__str__` is the whole
        // message the upgrade runner records, so the joined reasons are wrapped
        // here instead of at the call site.
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                "Refused to rename the legacy '{LEGACY_SLUG}' directive to '{NEW_SLUG}': {}. \
                 Nothing was changed. Resolve the above and re-run.",
                refusals.join("; ")
            ),
        });
    }
    Ok(actions)
}

/// Restamp the manifest's bundle hash under the synthesis lock.
fn restamp(project_root: &Path, tolerate_unreadable: bool) -> Result<(), InstallError> {
    let path = project_root.join(SYNTHESIS_MANIFEST_PATH);
    if !path.is_file() {
        return Ok(());
    }
    let _lock = acquire_synthesis_lock(project_root, sk_doctrine::DEFAULT_LOCK_TIMEOUT)
        .map_err(doctrine_error)?;
    let manifest = match SynthesisManifest::load(&path) {
        Ok(manifest) => manifest,
        Err(error) => {
            return if tolerate_unreadable {
                Ok(())
            } else {
                Err(charter_error(error))
            };
        }
    };
    if manifest.built_in_only || manifest.bundle_content_hash.is_none() {
        return Ok(());
    }
    let Some(current) = compute_bundle_content_hash(project_root) else {
        return Ok(());
    };
    if manifest.bundle_content_hash.as_deref() == Some(current.as_str()) {
        return Ok(());
    }
    let mut updated = manifest;
    updated.bundle_content_hash = Some(current);
    let finalized = updated.finalize();
    write_bytes(&path, &finalized.to_canonical_bytes())?;
    Ok(())
}

/// `rename_legacy_project_intent_directive`: move the legacy directive onto the new slug.
pub fn rename(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let actions = preflight(project_root)?;
    commit(project_root, &actions)
}

/// The writes after a successful preflight, then the re-stamp.
fn commit(project_root: &Path, actions: &[String]) -> Result<MigrationOutcome, InstallError> {
    if !actions.is_empty() {
        for step in &STEPS {
            (step.apply)(project_root)?;
        }
    }
    restamp(project_root, actions.is_empty())?;
    Ok(MigrationOutcome {
        changed: !actions.is_empty(),
        changes: actions.to_vec(),
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// `4.0.0rc8_project_intent_directive_rename.detect`.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    match preflight(project_root) {
        Ok(actions) => Ok(!actions.is_empty()),
        // A refusal is not "nothing to do": route it through apply() so the
        // runner records the reason.
        Err(InstallError::UpgradeRefused { .. }) => Ok(true),
        Err(error) => Err(error),
    }
}

/// `4.0.0rc8_project_intent_directive_rename.can_apply`.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !project_root.exists() {
        return Ok(Some(format!(
            "Project path does not exist: {}",
            project_root.display()
        )));
    }
    Ok(None)
}

/// `4.0.0rc8_project_intent_directive_rename.apply`.
///
/// The reference's `apply` CATCHES the helper's refusal
/// (`LegacyRenameRefusedError`) and returns `success = false` with its text as
/// the error, changing nothing; a refusal is therefore a recorded failure with
/// the reason, never a raised error that would drop the payload. A write that
/// fails after the preflight passed is still a raised error (the reference does
/// not catch it either).
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    match preflight(project_root) {
        Ok(actions) => commit(project_root, &actions),
        Err(InstallError::UpgradeRefused { detail }) => Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            success: false,
            errors: vec![detail],
            ..crate::upgrade::MigrationOutcome::default()
        }),
        Err(error) => Err(error),
    }
}

/// `4.0.0rc8_project_intent_directive_rename.apply(dry_run=True)`.
///
/// The reference's `apply` passes `dry_run=True` to the one charter-layer helper and
/// prefixes every reported action with `Would `. The helper's refusal is still a
/// CAUGHT failure (`success = false` carrying its text), in dry-run mode too, so a
/// preview never claims a state the real run would refuse. Nothing is renamed,
/// rewritten or re-stamped.
pub fn preview(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    match preflight(project_root) {
        Ok(actions) => Ok(MigrationOutcome {
            changed: !actions.is_empty(),
            changes: actions
                .iter()
                .map(|action| format!("Would {action}"))
                .collect(),
            warnings: Vec::new(),
            ..MigrationOutcome::default()
        }),
        Err(InstallError::UpgradeRefused { detail }) => Ok(MigrationOutcome {
            changed: false,
            changes: Vec::new(),
            success: false,
            errors: vec![detail],
            ..MigrationOutcome::default()
        }),
        Err(error) => Err(error),
    }
}

/// The rename migration this module implements.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[NativeMigration {
    id: "4.0.0rc8_project_intent_directive_rename",
    detect,
    can_apply,
    apply,
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_filename_becomes_its_target_name() {
        assert_eq!(
            rename_text(
                ".kittify/charter-packs/directive/001-mission-type-scope-directive.directive.yaml"
            ),
            ".kittify/charter-packs/directive/001-project-intent-directive.directive.yaml"
        );
        assert_eq!(
            rename_text("directive:PROJECT_001"),
            "directive:PROJECT_001"
        );
        assert_eq!(
            rename_text("mission-type-scope-directive"),
            "mission-type-scope-directive"
        );
        assert_eq!(
            rename_text("12-mission-type-scope-directive.directive.yaml"),
            "012-project-intent-directive.directive.yaml"
        );
    }

    #[test]
    fn only_numbered_legacy_stems_are_renamed() {
        assert_eq!(
            rename_stem("004-mission-type-scope-directive").as_deref(),
            Some("004-project-intent-directive")
        );
        assert_eq!(rename_stem("mission-type-scope-directive"), None);
        assert_eq!(rename_stem("004-other"), None);
    }
}
