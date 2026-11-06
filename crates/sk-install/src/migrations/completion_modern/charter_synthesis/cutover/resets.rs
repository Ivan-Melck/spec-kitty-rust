//! Step 6 of the charter-pack cutover: stale activation lists, kind gates and `[]`.
//!
//! Surfaces: the resolved activation store (the `charter.yaml` that
//! `config.yaml`'s `charter:` pointer names, else `config.yaml`), plus the other
//! file when it also carries activation keys (a leftover `config.yaml` mirror).
//! Per key, in the reference's `ACTIVATION_YAML_KEYS` order (see
//! [`activation_yaml_keys`], derived from the one artifact-kind authority):
//!
//! * `mission_type_activations` is never touched and never reported;
//! * `activated_kinds`: the released `default` 8-kind gate is reset; the released
//!   `minimal` gate `[directives, tactics]` is reset and a warning says it was a
//!   defect; any other gate is kept;
//! * a per-artifact `activated_<kind>: []` is reset with a warning naming the
//!   file and the key to set back to `[]`;
//! * a list set-equal (after [`normalise_id`]) to a released `default` list for
//!   that key is reset;
//! * a list set-equal to a released `minimal` list is kept (`matches_minimal`);
//! * any other list is kept (`kept_for_review`).
//!
//! Writes only remove keys, through the one activation writer (the activation
//! ledger once it is the resolved source, else the resolved file) and the
//! round-trip CST editor; every other byte is kept. The comparison reads the
//! FROZEN released-preset snapshots, never a live preset.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};
use sk_charter::ArtifactKind;
use sk_doctrine::{
    ActivationSource, EffectiveWhenAbsent, activatable_kinds, allow_key, apply_ledger_write,
    prepare_ledger_write, resolve_activation_source,
};

use super::snapshots::{
    DEFAULT_KIND_GATE, DEFAULT_SNAPSHOTS, DIRECTIVE_ID_TO_STEM, MINIMAL_KIND_GATE,
    MINIMAL_SNAPSHOTS,
};
use super::{CONFIG_REL, CutoverReport, LEGACY_KEYWORD, UPGRADE_REMEDY};
use crate::error::InstallError;
use crate::migrations::completion_legacy::paths::write_text;
use crate::yaml_edit::CstDocument;

const KINDS_KEY: &str = "activated_kinds";
const KEY_PREFIX: &str = "activated_";
const EMPTY_MAPPING: &str = "{}\n";

/// `ACTIVATION_YAML_KEYS` minus `mission_type_activations`, derived from the
/// canonical artifact-kind authority: `activated_kinds`, every activatable
/// kind's allow key in enum order **except** the tokenless anti-patterns, then
/// `activated_anti_patterns` (the reference's `_TOKENLESS_ACTIVATION_KEYS`).
pub fn activation_yaml_keys() -> Vec<String> {
    let mut keys = vec![KINDS_KEY.to_string()];
    for kind in activatable_kinds() {
        if kind == ArtifactKind::AntiPattern {
            continue;
        }
        if let Ok(key) = allow_key(kind) {
            keys.push(key);
        }
    }
    keys.push("activated_anti_patterns".to_string());
    keys
}

/// What the cutover does with one activation key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The key is removed.
    Reset,
    /// The key is kept and reported as customised.
    KeptForReview,
    /// The key is kept because it equals the released `minimal` preset.
    MatchesMinimal,
}

impl Outcome {
    fn report_field(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::KeptForReview => "kept_for_review",
            Self::MatchesMinimal => "matches_minimal",
        }
    }
}

/// One activation key of one file and what the cutover does with it.
#[derive(Clone, Debug)]
pub struct ResetAction {
    /// The file the key lives in.
    pub path: PathBuf,
    /// The activation key.
    pub key: String,
    /// The classification.
    pub outcome: Outcome,
    /// The report line.
    pub line: String,
    /// A warning-only line: how to restore what the reset removed.
    pub hint: Option<String>,
}

/// The comparable form of `raw` in `kind_key` (the reference's `normalise_id`).
///
/// Directives: `DIRECTIVE_<NNN>` becomes its numbered stem, and an unnumbered
/// UPPER_SNAKE id becomes its kebab stem; a numbered id with no snapshot stem is
/// left as written. Every other kind's ids are file stems already.
pub fn normalise_id(kind_key: &str, raw: &str) -> String {
    if kind_key != "activated_directives" {
        return raw.to_string();
    }
    if let Some((_, stem)) = DIRECTIVE_ID_TO_STEM.iter().find(|(id, _)| *id == raw) {
        return (*stem).to_string();
    }
    if is_upper_snake(raw) && !is_numbered_directive(raw) {
        return raw.to_lowercase().replace('_', "-");
    }
    raw.to_string()
}

fn is_upper_snake(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    let mut previous_underscore = false;
    for character in characters {
        if character == '_' {
            if previous_underscore {
                return false;
            }
            previous_underscore = true;
            continue;
        }
        if !(character.is_ascii_uppercase() || character.is_ascii_digit()) {
            return false;
        }
        previous_underscore = false;
    }
    !previous_underscore
}

fn is_numbered_directive(value: &str) -> bool {
    match value.strip_prefix("DIRECTIVE_") {
        Some(rest) => !rest.is_empty() && rest.chars().all(|character| character.is_numeric()),
        None => false,
    }
}

fn rel(path: &Path, project: &Path) -> String {
    crate::preserve::relative_display(path, project)
}

fn load_mapping(path: &Path, project: &Path) -> Result<Option<Mapping>, InstallError> {
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| InstallError::io("read", path, error))?;
    let value: Value =
        serde_yaml_ng::from_str(&text).map_err(|error| InstallError::MigrationStateUnreadable {
            path: Some(path.to_path_buf()),
            detail: format!(
                "{} could not be read ({error}); {UPGRADE_REMEDY}",
                rel(path, project)
            ),
        })?;
    Ok(value.as_mapping().cloned())
}

/// `(file, mapping)` per surface carrying activation keys; the resolved store first.
fn surfaces(project: &Path) -> Result<Vec<(PathBuf, Mapping)>, InstallError> {
    let config_path = project.join(CONFIG_REL);
    let config = load_mapping(&config_path, project)?.unwrap_or_default();
    let pointed: Option<PathBuf> = match config.get(Value::String("charter".to_string())) {
        Some(Value::String(pointer)) if !pointer.trim().is_empty() => {
            let candidate = PathBuf::from(pointer.trim());
            Some(if candidate.is_absolute() {
                candidate
            } else {
                project.join(candidate)
            })
        }
        _ => None,
    };
    let keys = activation_yaml_keys();
    let mut paths: Vec<PathBuf> = Vec::new();
    match pointed {
        Some(path) => {
            paths.push(path);
            paths.push(config_path.clone());
        }
        None => paths.push(config_path.clone()),
    }
    let mut out = Vec::new();
    for path in paths {
        let data = if path == config_path {
            config.clone()
        } else {
            match load_mapping(&path, project)? {
                Some(data) => data,
                None => continue,
            }
        };
        if keys
            .iter()
            .any(|key| data.contains_key(Value::String(key.clone())))
        {
            out.push((path, data));
        }
    }
    Ok(out)
}

fn id_set(key: &str, value: &[Value]) -> Option<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for item in value {
        let text = item.as_str()?;
        out.insert(normalise_id(key, text));
    }
    Some(out)
}

fn absent_meaning(key: &str) -> String {
    let plural = key.strip_prefix(KEY_PREFIX).unwrap_or(key);
    let noun = plural.replace('_', " ");
    match ArtifactKind::from_plural(plural) {
        Ok(kind) if EffectiveWhenAbsent::for_kind(kind) == EffectiveWhenAbsent::Required => {
            format!("only the {noun} your org packs require are in force")
        }
        _ => format!("all {noun} available"),
    }
}

fn pin_hint(rel: &str, key: &str, value: &BTreeSet<String>, what: &str) -> String {
    let ids: Vec<&str> = value.iter().map(String::as_str).collect();
    format!(
        "{rel}: {key} matched {what} and is now absent. To keep this exact list pinned, set {key}: [{}] in {rel}.",
        ids.join(", ")
    )
}

fn snapshot_contains(table: &[(&str, &[&[&str]])], key: &str, value: &BTreeSet<String>) -> bool {
    table
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, snapshots)| {
            snapshots.iter().any(|snapshot| {
                snapshot.len() == value.len() && snapshot.iter().all(|item| value.contains(*item))
            })
        })
        .unwrap_or(false)
}

fn classify_kind_gate(path: &Path, rel: &str, value: &BTreeSet<String>) -> ResetAction {
    let equals =
        |gate: &[&str]| gate.len() == value.len() && gate.iter().all(|item| value.contains(*item));
    if equals(&DEFAULT_KIND_GATE) {
        let line =
            format!("{rel}: {KINDS_KEY} (matched the released default kind gate; now absent)");
        return ResetAction {
            path: path.to_path_buf(),
            key: KINDS_KEY.to_string(),
            outcome: Outcome::Reset,
            line,
            hint: Some(pin_hint(
                rel,
                KINDS_KEY,
                value,
                "the released default kind gate",
            )),
        };
    }
    if equals(&MINIMAL_KIND_GATE) {
        let line = format!(
            "{rel}: {KINDS_KEY} [directives, tactics] (the released minimal kind gate; now absent)"
        );
        let hint = format!(
            "{rel}: {KINDS_KEY} was [directives, tactics], the kind gate of the released `minimal` preset; \
             it switched every other kind off, which was a defect, so it is now absent (every kind available). \
             To gate kinds again, set {KINDS_KEY} in {rel}."
        );
        return ResetAction {
            path: path.to_path_buf(),
            key: KINDS_KEY.to_string(),
            outcome: Outcome::Reset,
            line,
            hint: Some(hint),
        };
    }
    ResetAction {
        path: path.to_path_buf(),
        key: KINDS_KEY.to_string(),
        outcome: Outcome::KeptForReview,
        line: format!("{rel}: {KINDS_KEY} customised; not changed"),
        hint: None,
    }
}

fn classify(path: &Path, rel: &str, key: &str, value: &Value) -> ResetAction {
    let kept = |outcome: Outcome, line: String| ResetAction {
        path: path.to_path_buf(),
        key: key.to_string(),
        outcome,
        line,
        hint: None,
    };
    let Some(items) = value.as_sequence() else {
        return kept(
            Outcome::KeptForReview,
            format!("{rel}: {key} is not a list; not changed"),
        );
    };
    let Some(ids) = id_set(key, items) else {
        return kept(
            Outcome::KeptForReview,
            format!("{rel}: {key} holds a non-string id; not changed"),
        );
    };
    if key == KINDS_KEY {
        return classify_kind_gate(path, rel, &ids);
    }
    if items.is_empty() {
        let meaning = absent_meaning(key);
        let line = format!("{rel}: {key} was [] (nothing active); now absent ({meaning})");
        let hint = format!(
            "{rel}: {key} was [] (nothing active); it is now absent ({meaning}). To switch the kind off again, set {key}: [] in {rel}."
        );
        return ResetAction {
            path: path.to_path_buf(),
            key: key.to_string(),
            outcome: Outcome::Reset,
            line,
            hint: Some(hint),
        };
    }
    if snapshot_contains(&DEFAULT_SNAPSHOTS, key, &ids) {
        let line = format!(
            "{rel}: {key} (matched a released default list; now absent, every built-in available)"
        );
        return ResetAction {
            path: path.to_path_buf(),
            key: key.to_string(),
            outcome: Outcome::Reset,
            line,
            hint: Some(pin_hint(rel, key, &ids, "a released default list")),
        };
    }
    if snapshot_contains(&MINIMAL_SNAPSHOTS, key, &ids) {
        return kept(
            Outcome::MatchesMinimal,
            format!("{rel}: {key} matches preset minimal; not changed"),
        );
    }
    kept(
        Outcome::KeptForReview,
        format!("{rel}: {key} customised; not changed"),
    )
}

/// Classify every activation key of every surface (pure; nothing is written).
pub fn plan(project: &Path) -> Result<Vec<ResetAction>, InstallError> {
    let keys = activation_yaml_keys();
    let mut actions = Vec::new();
    for (path, data) in surfaces(project)? {
        let rel = rel(&path, project);
        for key in &keys {
            let Some(value) = data.get(Value::String(key.clone())) else {
                continue;
            };
            actions.push(classify(&path, &rel, key, value));
        }
    }
    Ok(actions)
}

/// Remove `keys` from `path`, through the one activation writer when `path` is
/// the resolved store and through the round-trip CST editor otherwise.
fn remove_keys(project: &Path, path: &Path, keys: &[String]) -> Result<(), InstallError> {
    let existing = load_mapping(path, project)?.unwrap_or_default();
    let only_activation_keys = existing.keys().all(|key| {
        key.as_str()
            .is_some_and(|name| keys.contains(&name.to_string()))
    });
    if only_activation_keys {
        return write_text(path, EMPTY_MAPPING).map(|_| ());
    }
    let resolved =
        resolve_activation_source(project).map_err(|error| InstallError::UpgradeRefused {
            detail: error.to_string(),
        })?;
    let store = match resolved.source() {
        ActivationSource::Ledger => Some(project.join(sk_doctrine::ACTIVATION_LEDGER_REL)),
        ActivationSource::Legacy => resolved
            .pointer
            .clone()
            .filter(|_| resolved.pointer_exists)
            .or_else(|| Some(project.join(CONFIG_REL))),
    };
    if store.as_deref() == Some(path) && resolved.source() == ActivationSource::Ledger {
        let remove: Vec<String> = keys.to_vec();
        let prepared =
            prepare_ledger_write(project, &BTreeMap::new(), &remove, None).map_err(|error| {
                InstallError::UpgradeRefused {
                    detail: error.to_string(),
                }
            })?;
        apply_ledger_write(prepared).map_err(|error| InstallError::UpgradeRefused {
            detail: error.to_string(),
        })?;
        return Ok(());
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| InstallError::io("read", path, error))?;
    let document = CstDocument::parse(path, &text)?;
    for key in keys {
        document.root().remove(key);
    }
    write_text(path, &document.text()).map(|_| ())
}

/// Classify, then remove every `Reset` key (unless `dry_run`).
pub fn run(project: &Path, dry_run: bool) -> Result<CutoverReport, InstallError> {
    let actions = plan(project)?;
    let mut report = CutoverReport::default();
    let mut by_file: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for action in actions {
        match action.outcome {
            Outcome::Reset => report.reset.push(action.line),
            Outcome::KeptForReview => report.kept_for_review.push(action.line),
            Outcome::MatchesMinimal => report.matches_minimal.push(action.line),
        }
        debug_assert!(matches!(
            action.outcome.report_field(),
            "reset" | "kept_for_review" | "matches_minimal"
        ));
        if let Some(hint) = action.hint {
            report.restore_hints.push(hint);
        }
        if action.outcome == Outcome::Reset {
            by_file.entry(action.path).or_default().push(action.key);
        }
    }
    if dry_run {
        return Ok(report);
    }
    for (path, keys) in by_file {
        if let Err(error) = remove_keys(project, &path, &keys) {
            report.errors.push(format!(
                "{} could not be written ({error}); {UPGRADE_REMEDY}",
                rel(&path, project)
            ));
        }
    }
    let _ = LEGACY_KEYWORD;
    Ok(report)
}
