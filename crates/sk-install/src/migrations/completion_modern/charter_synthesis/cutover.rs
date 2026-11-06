//! `charter_pack_cutover` (FR-012): rewrite pre-cutover project state into the
//! charter vocabulary, in one idempotent run.
//!
//! `apply` runs the reference's eight steps in contract order, each its own
//! function so a later step is a two-line wiring edit:
//!
//! 1. `preflight`: a path present under both the retired project layer and the
//!    project pack root with different content refuses the run, naming every
//!    colliding path; nothing is written;
//! 2. `move_project_root`: the retired project layer is moved into the project
//!    pack root file by file, each source removed through the ownership guard
//!    only once a byte-identical copy exists ([`Prover::MovedCopy`]), then the
//!    emptied root removed ([`Prover::EmptyTree`]); a locked file stops the run
//!    naming the path, and a re-run finishes the job;
//! 3. `rewrite_path_references`: synthesis manifest `artifacts[].path` (with
//!    `manifest_hash` recomputed through the one finalizer), provenance sidecars,
//!    `.kittify/skills-manifest.json` `source_ref` and `.gitignore` rules;
//! 4. `rewrite_config_keys`: `doctrine.org` (pack list and single-pack form),
//!    `organisation_packs`, `governance.doctrine` / `tracker.doctrine`, the
//!    pointed `charter.yaml`, a standalone `governance.yaml` and the interview
//!    answers' top-level `doctrine:`;
//! 5. `rewrite_activation_pack_ids`: `doctrine_pack_id` → `charter_pack_id` in
//!    the project activation entries;
//! 6. `reset_activation_lists`: stale-list, kind-gate and `[]` resets, on the
//!    FIRST application only ([`is_first_application`]);
//! 7. `remove_retired_skill_copies`: proven copies of removed skills removed,
//!    edited copies kept and reported;
//! 8. record: [`CutoverReport`] becomes the migration outcome.
//!
//! Rules: the canonical key wins when both spellings are present (the legacy one
//! is dropped and named in the report); user-chosen path values are never
//! rewritten (only keys and values that start with the retired root move); the
//! migration reads raw files, never through legacy-aware readers.
//!
//! Selection: [`detect`] is content-driven and TOTAL (it never fails; an
//! unreadable file selects the migration and `apply` names it).
//! [`structural_detect`] is the structural part only (retired root, retired
//! config keys, `doctrine_pack_id`), so the runner re-selects the cutover when a
//! pulled checkout brings legacy state back.
//!
//! Every canonical authority is reused rather than duplicated:
//! [`crate::preserve::guard_destructive_removal`] (with the shared provers),
//! [`sk_charter::ordered_yaml`] (the one emitter, in the reference's
//! `width=4096` round-trip mode), [`sk_charter::synthesis::SynthesisManifest`],
//! [`crate::migrations::charter_pack_gitignore::ensure_pack_yaml_allow_rule`],
//! [`sk_doctrine::prepare_ledger_write`] / `apply_ledger_write`, and the frozen
//! released-preset data generated into [`snapshots`] / `skill_data`.

mod resets;
mod skill_data;
mod skills;
pub mod snapshots;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};
use sk_charter::synthesis::{SYNTHESIS_MANIFEST_PATH, SynthesisManifest};

use crate::error::InstallError;
use crate::migrations::charter_pack_gitignore::{
    CHARTER_PACK_YAML_ALLOW_RULE, ensure_pack_yaml_allow_rule,
};
use crate::preserve::{Prover, guard_destructive_removal, relative_display};
use crate::upgrade::{MigrationOutcome, NativeMigration};

/// The retired project layer, POSIX, relative to the repository root.
pub const LEGACY_PROJECT_ROOT: &str = ".kittify/doctrine";
/// The project pack root the cutover moves into.
pub const PROJECT_PACK_ROOT: &str = ".kittify/charter-packs";
/// The retired key spelling shared by `doctrine.org`, `governance.doctrine`,
/// `tracker.doctrine`, the standalone `governance.yaml` and `answers.yaml`.
pub const LEGACY_KEYWORD: &str = "doctrine";
/// The retired flat org list key.
pub const ORGANISATION_PACKS_KEYWORD: &str = "organisation_packs";

const CONFIG_REL: &str = ".kittify/config.yaml";
const ANSWERS_REL: &str = ".kittify/charter/interview/answers.yaml";
const SKILLS_MANIFEST_REL: &str = ".kittify/skills-manifest.json";
const GOVERNANCE_REL: &str = ".kittify/charter/governance.yaml";
const PROVENANCE_DIRNAME: &str = ".provenance";
const GITIGNORE: &str = ".gitignore";

const CHARTER_KEY: &str = "charter";
const CHARTER_PACKS_KEY: &str = "charter_packs";
const OWNERSHIP_KEY: &str = "ownership";
const ORG_KEY: &str = "org";
const PACKS_KEY: &str = "packs";
const GOVERNANCE_KEY: &str = "governance";
const TRACKER_KEY: &str = "tracker";
const ACTIVATIONS_KEY: &str = "activations";
const LEGACY_PACK_ID_KEY: &str = "doctrine_pack_id";
const PACK_ID_KEY: &str = "charter_pack_id";
const LOCAL_PATH_KEY: &str = "local_path";
/// Fields of the single-pack legacy form, in canonical entry order.
const SINGLE_PACK_FIELDS: [&str; 5] = [LOCAL_PATH_KEY, "subdir", "source_type", "url", "ref"];
/// The preset name a single-pack legacy form must never take (it was auto-named so).
const RESERVED_PACK_NAME: &str = "default";
const FALLBACK_PACK_NAME: &str = "org";

const LEGACY_PREFIX: &str = ".kittify/doctrine/";
const LEGACY_PREFIX_WINDOWS: &str = ".kittify\\doctrine\\";
const PACK_PREFIX: &str = ".kittify/charter-packs/";
const UPGRADE_REMEDY: &str = "fix the file, then run `spec-kitty upgrade` again";
const NOT_A_MAPPING: &str =
    "charter_packs is not a mapping; make it one by hand, then run `spec-kitty upgrade` again";

// ---------------------------------------------------------------------------
// The report (`_charter_pack_cutover_report`)
// ---------------------------------------------------------------------------

/// Ordered result lines of one cutover run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CutoverReport {
    /// Project-layer files moved to the project pack root.
    pub moved: Vec<String>,
    /// Config keys and path references rewritten.
    pub rewritten: Vec<String>,
    /// Stale lists, stale kind gates and `[]` reset to absent.
    pub reset: Vec<String>,
    /// Customised / near-miss lists and unconvertible entries kept.
    pub kept_for_review: Vec<String>,
    /// Lists equal to the released `minimal` preset, kept.
    pub matches_minimal: Vec<String>,
    /// Installed copies of removed skills deleted.
    pub skills_removed: Vec<String>,
    /// Edited skill copies kept.
    pub skills_kept: Vec<String>,
    /// Refusals (collision paths, locked files).
    pub errors: Vec<String>,
    /// Warning-only lines: how to restore each reset `[]` (not a report key).
    pub restore_hints: Vec<String>,
    /// Edited skill copies kept in place (`MigrationOutcome` has no such field;
    /// they are also in `skills_kept`).
    pub preserved_paths: Vec<String>,
}

/// The report keys, in contract order.
const REPORT_KEYS: [&str; 8] = [
    "moved",
    "rewritten",
    "reset",
    "kept_for_review",
    "matches_minimal",
    "skills_removed",
    "skills_kept",
    "errors",
];

impl CutoverReport {
    fn merge(&mut self, other: CutoverReport) {
        self.moved.extend(other.moved);
        self.rewritten.extend(other.rewritten);
        self.reset.extend(other.reset);
        self.kept_for_review.extend(other.kept_for_review);
        self.matches_minimal.extend(other.matches_minimal);
        self.skills_removed.extend(other.skills_removed);
        self.skills_kept.extend(other.skills_kept);
        self.errors.extend(other.errors);
        self.restore_hints.extend(other.restore_hints);
        self.preserved_paths.extend(other.preserved_paths);
    }

    fn list(&self, key: &str) -> &Vec<String> {
        match key {
            "moved" => &self.moved,
            "rewritten" => &self.rewritten,
            "reset" => &self.reset,
            "kept_for_review" => &self.kept_for_review,
            "matches_minimal" => &self.matches_minimal,
            "skills_removed" => &self.skills_removed,
            "skills_kept" => &self.skills_kept,
            "errors" => &self.errors,
            other => unreachable!("unknown report key {other}"),
        }
    }

    /// The contract report: every key present, each an ordered list.
    pub fn as_value(&self) -> Value {
        let mut mapping = Mapping::new();
        for key in REPORT_KEYS {
            mapping.insert(
                Value::String(key.to_string()),
                Value::Sequence(
                    self.list(key)
                        .iter()
                        .map(|line| Value::String(line.clone()))
                        .collect(),
                ),
            );
        }
        Value::Mapping(mapping)
    }

    /// The contract report as `json.dumps(as_dict(), sort_keys=True)` renders
    /// it, through the ONE canonical JSON codec (J1): no private encoder here.
    pub fn to_json(&self) -> String {
        let mut object = sk_kernel::python_json::PythonJsonDict::new();
        for (key, lines) in self.report_members() {
            object.insert(
                sk_kernel::PythonString::from_scalars(key),
                sk_kernel::python_json::PythonJsonValue::Array(
                    lines
                        .iter()
                        .map(|line| sk_kernel::python_json::PythonJsonValue::scalar(line))
                        .collect(),
                ),
            );
        }
        let value = sk_kernel::python_json::PythonJsonValue::ExactObject(object);
        value
            .render_python_with(
                sk_kernel::python_json::PythonJsonRenderOptions::default().sort_keys(true),
            )
            .to_scalar_string()
            .unwrap_or_default()
    }

    /// True when the run would change the project or refuses.
    pub fn is_actionable(&self) -> bool {
        !self.moved.is_empty()
            || !self.rewritten.is_empty()
            || !self.reset.is_empty()
            || !self.skills_removed.is_empty()
            || !self.errors.is_empty()
    }

    /// Map onto the native outcome (the reference's `to_migration_result`).
    ///
    /// A returned failure retains the complete Source report. Source's cutover
    /// does not populate `partial_writes`, even after a real completed move:
    /// existing destinations are not independently inferred result records.
    pub fn to_outcome(
        self,
        _project_root: &Path,
        dry_run: bool,
    ) -> Result<MigrationOutcome, InstallError> {
        let mut changes = vec![self.to_json()];
        for (key, verb, dry_verb) in [
            ("moved", "Moved", "Would move"),
            ("rewritten", "Rewrote", "Would rewrite"),
            ("reset", "Reset", "Would reset"),
            ("skills_removed", "Removed skill", "Would remove skill"),
        ] {
            let prefix = if dry_run { dry_verb } else { verb };
            changes.extend(self.list(key).iter().map(|line| format!("{prefix} {line}")));
        }
        let mut warnings = Vec::new();
        for (key, label, dry_label) in [
            (
                "kept_for_review",
                "Kept for review",
                "Would keep for review",
            ),
            (
                "matches_minimal",
                "Matches preset minimal, kept",
                "Would keep (matches preset minimal)",
            ),
            (
                "skills_kept",
                "Kept edited skill copy",
                "Would keep edited skill copy",
            ),
        ] {
            let prefix = if dry_run { dry_label } else { label };
            warnings.extend(
                self.list(key)
                    .iter()
                    .map(|line| format!("{prefix}: {line}")),
            );
        }
        let manual_review: Vec<String> = ["kept_for_review", "matches_minimal", "skills_kept"]
            .iter()
            .flat_map(|key| self.list(key).clone())
            .collect();
        warnings.extend(self.restore_hints.iter().cloned());
        // A Source business failure is `success = false` with the whole report —
        // never a generic refusal that would discard it (FR-005 / the carrier).
        let success = self.errors.is_empty();
        let mut outcome = MigrationOutcome {
            // Native convenience: a returned failure can still have real
            // completed actions. Errors alone are not evidence of a write.
            changed: ["moved", "rewritten", "reset", "skills_removed"]
                .iter()
                .any(|key| !self.list(key).is_empty()),
            changes,
            warnings,
            success,
            errors: self.errors.clone(),
            manual_review_required: !manual_review.is_empty(),
            preserved_paths: self.preserved_paths.clone(),
            ..MigrationOutcome::default()
        };
        outcome.manual_review_required = !outcome.warnings.is_empty();
        Ok(outcome)
    }
}

impl CutoverReport {
    /// The contract members in REPORT_KEYS order (each an ordered list), so the
    /// canonical renderer receives exactly what `as_dict()` returns.
    fn report_members(&self) -> Vec<(&'static str, &Vec<String>)> {
        REPORT_KEYS
            .iter()
            .map(|key| (*key, self.list(key)))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Run state
// ---------------------------------------------------------------------------

/// One `apply` (or a dry run used by `detect`): paths, mode and report.
struct Run {
    project: PathBuf,
    dry_run: bool,
    report: CutoverReport,
    /// The first persistence failure. The reference's `atomic_write` raises, and
    /// an uncaught exception stays an `Err` natively (never a business result).
    io_error: Option<InstallError>,
}

impl Run {
    fn new(project: &Path, dry_run: bool) -> Self {
        Self {
            project: project.to_path_buf(),
            dry_run,
            report: CutoverReport::default(),
            io_error: None,
        }
    }

    fn legacy_root(&self) -> PathBuf {
        self.project.join(LEGACY_PROJECT_ROOT)
    }

    fn pack_root(&self) -> PathBuf {
        self.project.join(PROJECT_PACK_ROOT)
    }

    fn rel(&self, path: &Path) -> String {
        relative_display(path, &self.project)
    }

    fn write_text(&mut self, path: &Path, text: &str) {
        if self.dry_run || self.io_error.is_some() {
            return;
        }
        if let Err(error) = crate::migrations::completion_legacy::paths::write_text(path, text) {
            self.io_error = Some(error);
        }
    }
}

/// A read error is the runner's named failure, never an uncaught panic.
///
/// Source's `_unreadable` raises `MigrationStateUnreadableError`, so this is the typed
/// carrier: the runner's main `apply()` phase returns a failed result with no ledger
/// record for it, and a worktree only flags the step as errored.
fn unreadable(path: &Path, project: &Path, detail: impl std::fmt::Display) -> InstallError {
    InstallError::MigrationStateUnreadable {
        path: Some(path.to_path_buf()),
        detail: format!(
            "{} could not be read ({detail}); {UPGRADE_REMEDY}",
            relative_display(path, project)
        ),
    }
}

fn read_text(path: &Path, project: &Path) -> Result<Option<String>, InstallError> {
    if !path.is_file() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|error| unreadable(path, project, error))
}

fn load_yaml(text: &str, path: &Path, project: &Path) -> Result<Value, InstallError> {
    serde_yaml_ng::from_str::<Value>(text).map_err(|error| unreadable(path, project, error))
}

/// The reference's round-trip dump: authored key order, `width=4096`.
fn dump(value: &Value) -> String {
    String::from_utf8_lossy(&sk_charter::ordered_yaml_width(value, 4096)).into_owned()
}

/// The value moved to the project pack root, or `None` when it is not under the
/// retired root.
fn rewrite_legacy_path(value: &Value) -> Option<String> {
    let Value::String(raw) = value else {
        return None;
    };
    let normalised = if let Some(rest) = raw.strip_prefix(LEGACY_PREFIX_WINDOWS) {
        format!("/{rest}").replacen('\\', "/", usize::MAX)
    } else {
        raw.clone()
    };
    if let Some(rest) = raw.strip_prefix(LEGACY_PREFIX) {
        return Some(format!("{PACK_PREFIX}{rest}"));
    }
    normalised
        .strip_prefix(LEGACY_PREFIX)
        .map(|rest| format!("{PACK_PREFIX}{rest}"))
}

fn mentions_legacy_root(text: &str) -> bool {
    text.contains(LEGACY_PREFIX) || text.contains(LEGACY_PREFIX_WINDOWS)
}

fn string_field(mapping: &Mapping, key: &str) -> Option<String> {
    match mapping.get(Value::String(key.to_string())) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn mapping_field<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Mapping> {
    match mapping.get(Value::String(key.to_string())) {
        Some(Value::Mapping(inner)) => Some(inner),
        _ => None,
    }
}

fn mapping_field_mut<'a>(mapping: &'a mut Mapping, key: &str) -> Option<&'a mut Mapping> {
    match mapping.get_mut(Value::String(key.to_string())) {
        Some(Value::Mapping(inner)) => Some(inner),
        _ => None,
    }
}

/// One-line JSON rendering of a dropped value so the report keeps it recoverable.
///
/// This is the reference's `json.dumps(value, default=str, separators=(",", ":"))`:
/// the canonical COMPACT renderer, mapping order PRESERVED (no `sort_keys`), with
/// non-JSON Python objects projected to `str` upstream by the caller-side value
/// conversion below.
fn compact(value: &Value) -> String {
    let projected = python_json_of(value);
    projected
        .render_python_with(
            sk_kernel::python_json::PythonJsonRenderOptions::default()
                .separators(Some(sk_kernel::python_json::PythonJsonSeparators::COMPACT))
                .default_projection(sk_kernel::python_json::PythonJsonDefault::PythonStr),
        )
        .to_scalar_string()
        .unwrap_or_default()
}

/// The structure-preserving conversion the codec expects a caller to do.
///
/// YAML scalars are already JSON scalars (`serde_yaml_ng` resolves a timestamp to
/// a string), so the only projection needed is for mapping keys: CPython's
/// `json.dumps` renders a scalar key through `str()`, which the canonical key
/// coercion below mirrors. A non-scalar mapping key has no `str()` that keeps it
/// addressable, so it is projected with the one `python_repr` primitive instead
/// of a second formatter.
fn python_json_of(value: &Value) -> sk_kernel::python_json::PythonJsonValue {
    use sk_kernel::python_json::{PythonJsonDict, PythonJsonValue};
    match value {
        Value::Null => PythonJsonValue::Null,
        Value::Bool(flag) => PythonJsonValue::Bool(*flag),
        Value::Number(number) => {
            let token = number.to_string();
            if number.is_f64() {
                PythonJsonValue::Float(token.parse().unwrap_or(0.0))
            } else {
                PythonJsonValue::Integer(token)
            }
        }
        Value::String(text) => PythonJsonValue::scalar(text),
        Value::Sequence(items) => {
            PythonJsonValue::Array(items.iter().map(python_json_of).collect())
        }
        Value::Mapping(mapping) => {
            let mut object = PythonJsonDict::new();
            for (key, child) in mapping {
                object.insert(json_key(key), python_json_of(child));
            }
            PythonJsonValue::ExactObject(object)
        }
        Value::Tagged(tagged) => python_json_of(&tagged.value),
    }
}

/// CPython's scalar dict-key coercion (`json.dumps` renders a scalar key with
/// `str()`), falling back to the one `python_repr` primitive for a key that has
/// no scalar `str()` form.
fn json_key(key: &Value) -> sk_kernel::PythonString {
    match key {
        Value::String(text) => sk_kernel::PythonString::from_scalars(text),
        Value::Bool(true) => sk_kernel::PythonString::from_scalars("true"),
        Value::Bool(false) => sk_kernel::PythonString::from_scalars("false"),
        Value::Null => sk_kernel::PythonString::from_scalars("null"),
        Value::Number(number) => sk_kernel::PythonString::from_scalars(&number.to_string()),
        other => {
            let rendered = compact(other);
            sk_kernel::PythonString::from_scalars(&rendered)
        }
    }
}

// ---------------------------------------------------------------------------
// Step 1-2: preflight and move
// ---------------------------------------------------------------------------

/// Every file and symlink under `legacy_root`, relative, sorted (dot-directories
/// included).
fn legacy_entries(legacy_root: &Path) -> Vec<PathBuf> {
    let mut entries = Vec::new();
    let mut stack = vec![legacy_root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                entries.push(path);
            } else if kind.is_dir() {
                stack.push(path);
            } else {
                entries.push(path);
            }
        }
    }
    let mut relatives: Vec<PathBuf> = entries
        .into_iter()
        .filter_map(|path| path.strip_prefix(legacy_root).ok().map(Path::to_path_buf))
        .collect();
    relatives.sort();
    relatives
}

/// True when `copy` holds exactly what `source` holds (bytes, or the same link target).
fn same_entry(source: &Path, copy: &Path) -> bool {
    let Ok(source_meta) = std::fs::symlink_metadata(source) else {
        return false;
    };
    let Ok(copy_meta) = std::fs::symlink_metadata(copy) else {
        return false;
    };
    if source_meta.file_type().is_symlink() {
        return copy_meta.file_type().is_symlink()
            && std::fs::read_link(source).ok() == std::fs::read_link(copy).ok();
    }
    !copy_meta.file_type().is_symlink()
        && source_meta.is_file()
        && copy_meta.is_file()
        && std::fs::read(source).ok() == std::fs::read(copy).ok()
}

/// Step 1: a differing file present under both roots refuses the run.
fn preflight(run: &mut Run) {
    let legacy = run.legacy_root();
    let Ok(metadata) = std::fs::symlink_metadata(&legacy) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        run.report.errors.push(format!(
            "{} is a symlink; replace it with the directory it points to, then run `spec-kitty upgrade` again",
            run.rel(&legacy)
        ));
        return;
    }
    if !metadata.is_dir() {
        return;
    }
    let pack_root = run.pack_root();
    for relative in legacy_entries(&legacy) {
        let source = legacy.join(&relative);
        let target = pack_root.join(&relative);
        let target_present = std::fs::symlink_metadata(&target).is_ok();
        if target_present && !same_entry(&source, &target) {
            run.report.errors.push(format!(
                "{} collides with {} (different content); reconcile the two, then run `spec-kitty upgrade` again",
                run.rel(&source),
                run.rel(&target)
            ));
        }
    }
}

/// The reference's `{exc}` rendering for an `OSError`: `[Errno N] strerror: 'path'`.
///
/// Rust's `io::Error` Display is `strerror (os error N)` on unix, so the Python
/// form is derived from the same two facts (never guessed) and the *path* the
/// reference's exception names is the one the failing call used.
fn python_os_error(error: &std::io::Error, path: &Path) -> String {
    let rendered = error.to_string();
    let message = match rendered.find(" (os error ") {
        Some(position) => rendered[..position].to_string(),
        None => rendered,
    };
    match error.raw_os_error() {
        Some(code) => format!("[Errno {code}] {message}: '{}'", path.display()),
        None => format!("{message}: '{}'", path.display()),
    }
}

/// Move one entry: copy verbatim, prove the copy, then retire the source.
///
/// Returns the reference's exception text on failure (the caller wraps it in the
/// "could not be moved" report line).
fn move_one(run: &mut Run, relative: &Path) -> Result<(), String> {
    let source = run.legacy_root().join(relative);
    let target = run.pack_root().join(relative);
    if std::fs::symlink_metadata(&target).is_err() {
        let parent = target.parent().unwrap_or(&target);
        if let Err(error) = std::fs::create_dir_all(parent) {
            return Err(python_os_error(&error, parent));
        }
        let source_meta = match std::fs::symlink_metadata(&source) {
            Ok(meta) => meta,
            Err(error) => return Err(python_os_error(&error, &source)),
        };
        if source_meta.file_type().is_symlink() {
            let linked = match std::fs::read_link(&source) {
                Ok(linked) => linked,
                Err(error) => return Err(python_os_error(&error, &source)),
            };
            if let Err(error) = std::os::unix::fs::symlink(&linked, &target) {
                return Err(python_os_error(&error, &target));
            }
        } else if let Err(error) = std::fs::copy(&source, &target) {
            return Err(python_os_error(&error, &source));
        }
    }
    let verdict = guard_destructive_removal(
        &source,
        &run.project,
        &Prover::MovedCopy {
            copy: target.clone(),
        },
        false,
        None,
        false,
    )
    .map_err(|error| error.to_string())?;
    if !verdict.removed() {
        return Err(format!("the copy at {} does not match", run.rel(&target)));
    }
    Ok(())
}

/// Step 2: move the retired layer, then retire the emptied root.
fn move_project_root(run: &mut Run) {
    let legacy = run.legacy_root();
    let Ok(metadata) = std::fs::symlink_metadata(&legacy) else {
        return;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return;
    }
    let entries = legacy_entries(&legacy);
    for relative in entries {
        let line = format!(
            "{} -> {}",
            run.rel(&legacy.join(&relative)),
            run.rel(&run.pack_root().join(&relative))
        );
        if !run.dry_run {
            match move_one(run, &relative) {
                Ok(()) => {}
                Err(error) => {
                    run.report.errors.push(format!(
                        "{} could not be moved ({error}); close any program holding it, then run `spec-kitty upgrade` again",
                        run.rel(&legacy.join(&relative))
                    ));
                    return;
                }
            }
        }
        run.report.moved.push(line);
    }
    if run.dry_run {
        return;
    }
    let verdict =
        guard_destructive_removal(&legacy, &run.project, &Prover::EmptyTree, true, None, false);
    match verdict {
        Ok(verdict) if verdict.removed() => {}
        Ok(_) => run.report.errors.push(format!(
            "{} still holds files; move them to {}, then run `spec-kitty upgrade` again",
            run.rel(&legacy),
            run.rel(&run.pack_root())
        )),
        Err(error) => run.report.errors.push(format!(
            "{} could not be removed ({error}); run `spec-kitty upgrade` again",
            run.rel(&legacy)
        )),
    }
}

// ---------------------------------------------------------------------------
// Step 3: path references
// ---------------------------------------------------------------------------

/// The synthesis manifest's `artifacts[].path`, with `manifest_hash` recomputed.
fn rewrite_synthesis_manifest(run: &mut Run) -> Result<(), InstallError> {
    let path = run.project.join(SYNTHESIS_MANIFEST_PATH);
    let Some(text) = read_text(&path, &run.project)? else {
        return Ok(());
    };
    if !mentions_legacy_root(&text) {
        return Ok(());
    }
    let manifest =
        SynthesisManifest::load(&path).map_err(|error| unreadable(&path, &run.project, error))?;
    let rel = run.rel(&path);
    let mut artifacts = Vec::with_capacity(manifest.artifacts.len());
    let mut rewritten = Vec::new();
    for entry in manifest.artifacts {
        let moved = rewrite_legacy_path(&Value::String(entry.path.clone()));
        match moved {
            Some(moved) => {
                rewritten.push(format!("{rel}: artifacts[].path {} -> {moved}", entry.path));
                artifacts.push(sk_charter::synthesis::ManifestArtifactEntry {
                    path: moved,
                    ..entry
                });
            }
            None => artifacts.push(entry),
        }
    }
    if rewritten.is_empty() {
        return Ok(());
    }
    let updated = SynthesisManifest {
        artifacts,
        ..manifest
    }
    .finalize();
    run.report.rewritten.extend(rewritten);
    run.write_text(
        &path,
        &String::from_utf8_lossy(&updated.to_canonical_bytes()),
    );
    Ok(())
}

/// Rewrite, in place, every string value under `node` that starts with the
/// retired root.
fn rewrite_values(node: &mut Value, rel: &str, trail: &str, lines: &mut Vec<String>) {
    match node {
        Value::Mapping(mapping) => {
            let keys: Vec<Value> = mapping.keys().cloned().collect();
            for key in keys {
                let child_trail = match key.as_str() {
                    Some(name) => {
                        if trail.is_empty() {
                            name.to_string()
                        } else {
                            format!("{trail}.{name}")
                        }
                    }
                    None => format!("{trail}[{}]", compact(&key)),
                };
                let Some(child) = mapping.get_mut(&key) else {
                    continue;
                };
                match rewrite_legacy_path(child) {
                    Some(moved) => {
                        let old = child.clone();
                        lines.push(format!(
                            "{rel}: {child_trail} {} -> {moved}",
                            match &old {
                                Value::String(text) => text.clone(),
                                other => compact(other),
                            }
                        ));
                        *child = Value::String(moved);
                    }
                    None => rewrite_values(child, rel, &child_trail, lines),
                }
            }
        }
        Value::Sequence(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                let child_trail = format!("{trail}[{index}]");
                match rewrite_legacy_path(item) {
                    Some(moved) => {
                        let old = match &item {
                            Value::String(text) => text.clone(),
                            other => compact(other),
                        };
                        lines.push(format!("{rel}: {child_trail} {old} -> {moved}"));
                        *item = Value::String(moved);
                    }
                    None => rewrite_values(item, rel, &child_trail, lines),
                }
            }
        }
        _ => {}
    }
}

fn yaml_files_below(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(root) else {
        return;
    };
    let mut paths: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            yaml_files_below(&path, out);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "yaml")
        {
            out.push(path);
        }
    }
}

/// Every provenance sidecar: the bundle directory, and any `.../.provenance/`.
fn provenance_sidecars(run: &Run) -> Vec<PathBuf> {
    let mut sidecars = Vec::new();
    let bundle = run.project.join(".kittify/charter/provenance");
    let Ok(read) = std::fs::read_dir(&bundle) else {
        return sidecars;
    };
    let mut paths: Vec<PathBuf> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .collect();
    paths.sort();
    sidecars.extend(paths);
    for root in [run.legacy_root(), run.pack_root()] {
        let mut files = Vec::new();
        yaml_files_below(&root, &mut files);
        for file in files {
            if file
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == PROVENANCE_DIRNAME)
                && file
                    .extension()
                    .is_some_and(|extension| extension == "yaml")
            {
                sidecars.push(file);
            }
        }
    }
    sidecars.sort();
    sidecars.dedup();
    sidecars
}

fn rewrite_provenance_sidecars(run: &mut Run) -> Result<(), InstallError> {
    for path in provenance_sidecars(run) {
        let Some(text) = read_text(&path, &run.project)? else {
            continue;
        };
        if !mentions_legacy_root(&text) {
            continue;
        }
        let mut data = load_yaml(&text, &path, &run.project)?;
        let mut lines = Vec::new();
        let rel = run.rel(&path);
        rewrite_values(&mut data, &rel, "", &mut lines);
        if !lines.is_empty() {
            run.report.rewritten.extend(lines);
            run.write_text(&path, &dump(&data));
        }
    }
    Ok(())
}

/// `.kittify/skills-manifest.json` `entries[].source_ref`.
fn rewrite_skills_manifest(run: &mut Run) -> Result<(), InstallError> {
    let path = run.project.join(SKILLS_MANIFEST_REL);
    let Some(text) = read_text(&path, &run.project)? else {
        return Ok(());
    };
    if !mentions_legacy_root(&text) {
        return Ok(());
    }
    let mut data: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| unreadable(&path, &run.project, error))?;
    let rel = run.rel(&path);
    let mut rewritten = Vec::new();
    if let Some(entries) = data
        .get_mut("entries")
        .and_then(|value| value.as_array_mut())
    {
        for entry in entries {
            let Some(map) = entry.as_object_mut() else {
                continue;
            };
            let Some(source_ref) = map.get("source_ref").cloned() else {
                continue;
            };
            let moved = rewrite_legacy_path(&Value::String(
                source_ref.as_str().unwrap_or_default().to_string(),
            ));
            if let Some(moved) = moved
                && source_ref.as_str() != Some(moved.as_str())
            {
                rewritten.push(format!(
                    "{rel}: entries[].source_ref {} -> {moved}",
                    source_ref.as_str().unwrap_or_default()
                ));
                map.insert("source_ref".to_string(), serde_json::Value::String(moved));
            }
        }
    }
    if !rewritten.is_empty() {
        let mut rendered = serde_json::to_string_pretty(&data)
            .map_err(|error| unreadable(&path, &run.project, error))?;
        rendered.push('\n');
        run.report.rewritten.extend(rewritten);
        run.write_text(&path, &rendered);
    }
    Ok(())
}

/// One `.gitignore` rule naming the retired root: the rule is `<entry>` when it
/// is the retired root itself or starts with it followed by a separator.
fn legacy_rule(rule: &str) -> bool {
    let mut search = rule;
    while let Some(position) = search.find(LEGACY_PROJECT_ROOT) {
        let before = &search[..position];
        let after = &search[position + LEGACY_PROJECT_ROOT.len()..];
        let boundary_ok = before.is_empty()
            || before.ends_with('/')
            || before.ends_with('!')
            || before.ends_with(' ')
            || before.ends_with('\t');
        let end_ok = after.is_empty()
            || after.starts_with('/')
            || after.starts_with(' ')
            || after.starts_with('\t');
        if boundary_ok && end_ok {
            return true;
        }
        search = &search[position + LEGACY_PROJECT_ROOT.len()..];
    }
    false
}

fn rewrite_gitignore_rule(rule: &str) -> String {
    rule.replace(LEGACY_PROJECT_ROOT, PROJECT_PACK_ROOT)
}

/// Step 3's `.gitignore` leg: rewrite retired rules and emit the `pack.yaml`
/// allow-rule through the ONE placement authority.
fn rewrite_gitignore(run: &mut Run) {
    let path = run.project.join(GITIGNORE);
    let text = match read_ignore_text(&path) {
        Ok(Some(text)) => text,
        Ok(None) => return,
        Err(detail) => {
            run.report.kept_for_review.push(format!(
                "{GITIGNORE} could not be read safely ({detail}); check it for {LEGACY_PROJECT_ROOT} rules by hand"
            ));
            return;
        }
    };
    if !text.contains(LEGACY_PROJECT_ROOT) {
        return;
    }
    let existing: BTreeSet<String> = text.lines().map(|line| line.trim().to_string()).collect();
    let mut kept: Vec<String> = Vec::new();
    let mut changes: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = existing.clone();
    for line in text.split_inclusive('\n') {
        let rule = line.trim();
        if rule.is_empty() || rule.starts_with('#') || !legacy_rule(rule) {
            kept.push(line.to_string());
            continue;
        }
        let new_rule = rewrite_gitignore_rule(rule);
        if seen.contains(&new_rule) {
            changes.push(format!(
                "dropped rule {rule} ({new_rule} is already present)"
            ));
            continue;
        }
        kept.push(line.replace(rule, &new_rule));
        seen.insert(new_rule.clone());
        changes.push(format!("rule {rule} -> {new_rule}"));
    }
    let joined: String = kept.concat();
    let (with_allow, added) = ensure_pack_yaml_allow_rule(&joined);
    if added {
        changes.push(format!(
            "added {CHARTER_PACK_YAML_ALLOW_RULE} (keeps the project pack.yaml tracked under the charter-pack ignore rule)"
        ));
    }
    if changes.is_empty() {
        return;
    }
    if !run.dry_run
        && let Err(detail) = write_ignore_text(&path, &with_allow)
    {
        run.report.errors.push(format!(
            "{GITIGNORE} could not be written ({detail}); make it writable, then run `spec-kitty upgrade` again"
        ));
        return;
    }
    run.report.rewritten.extend(
        changes
            .into_iter()
            .map(|change| format!("{GITIGNORE}: {change}")),
    );
}

/// Read `.gitignore` without following a symlink: `Ok(None)` when absent, `Err`
/// when it cannot be read safely (a symlink, not UTF-8, a directory, no access).
fn read_ignore_text(path: &Path) -> Result<Option<String>, String> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(None);
    };
    // The reference's `GitignorePathError` texts are report lines, so they are
    // reproduced verbatim (`read_gitignore_text`).
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Refusing to read symlinked .gitignore: {}",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Refusing to read symlinked .gitignore: {}",
            path.display()
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Could not safely open .gitignore: {error}"))?;
    let mut text = String::from_utf8(bytes)
        .map_err(|error| format!("Could not decode .gitignore as UTF-8: {error}"))?;
    // `encoding="utf-8-sig"`: a leading BOM is consumed, never kept as content.
    if let Some(stripped) = text.strip_prefix('\u{feff}') {
        text = stripped.to_string();
    }
    Ok(Some(text))
}

fn write_ignore_text(path: &Path, text: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("it is not a regular file".to_string());
    }
    std::fs::write(path, text).map_err(|error| error.to_string())
}

fn rewrite_path_references(run: &mut Run) -> Result<(), InstallError> {
    rewrite_synthesis_manifest(run)?;
    rewrite_provenance_sidecars(run)?;
    rewrite_skills_manifest(run)?;
    rewrite_gitignore(run);
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 4: config keys
// ---------------------------------------------------------------------------

/// One `old` → `new` key rename in a mapping (or a drop when `new` is present).
#[derive(Clone)]
struct KeyEdit {
    old: String,
    new: String,
    label: String,
    drops: bool,
}

impl KeyEdit {
    fn describe(&self, rel: &str, dropped: Option<&Value>) -> String {
        let new_label = match self.label.rsplit_once('.') {
            Some((prefix, _)) => format!("{prefix}.{}", self.new),
            None => self.new.clone(),
        };
        if self.drops {
            return format!(
                "{rel}: dropped {} ({new_label} is already present; the canonical value wins; dropped value: {})",
                self.label,
                dropped.map(compact).unwrap_or_else(|| "null".to_string())
            );
        }
        format!("{rel}: {} -> {new_label}", self.label)
    }
}

fn rename_key(mapping: &mut Mapping, old: &str, new: &str) {
    let mut rebuilt = Mapping::new();
    let taken = std::mem::take(mapping);
    for (key, value) in taken {
        if key.as_str() == Some(old) {
            rebuilt.insert(Value::String(new.to_string()), value);
        } else {
            rebuilt.insert(key, value);
        }
    }
    *mapping = rebuilt;
}

/// The `(offset, length)` of a `key:` token on one line, including its quotes
/// and an optional `- ` sequence marker before it.
fn key_token_span(line: &str, key: &str) -> Option<(usize, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let offset = match rest.strip_prefix("- ") {
        Some(_) => indent + 2,
        None => indent,
    };
    let content = &line[offset..];
    for quote in ["", "\"", "'"] {
        let token = format!("{quote}{key}{quote}");
        if let Some(rest) = content.strip_prefix(&token)
            && rest.trim_start().starts_with(':')
        {
            return Some((offset, token.len()));
        }
    }
    None
}

/// Rename `old:` to `new:` on the one line that carries the key belonging to
/// `section` (the document root when `None`), so every other byte survives.
///
/// `None` when the key line cannot be located: the caller then falls back to the
/// reference's own round-trip dump.
fn rename_key_in_text(text: &str, section: Option<&str>, old: &str, new: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let indent_of = |line: &str| line.len() - line.trim_start().len();
    let mut start = 0usize;
    if let Some(section) = section {
        let found = lines
            .iter()
            .position(|line| indent_of(line) == 0 && key_token_span(line, section).is_some())?;
        start = found + 1;
    }
    let mut best: Option<(usize, usize, usize)> = None;
    let mut child_indent: Option<usize> = None;
    for (index, line) in lines.iter().enumerate().skip(start) {
        if line.trim().is_empty() {
            continue;
        }
        let indent = indent_of(line);
        if section.is_some() {
            if indent == 0 {
                break;
            }
            if child_indent.is_none_or(|limit| indent <= limit) {
                child_indent = Some(indent);
            } else {
                continue;
            }
        } else if indent != 0 {
            continue;
        }
        if let Some((offset, length)) = key_token_span(line, old) {
            best = Some((index, offset, length));
            break;
        }
    }
    let (index, offset, length) = best?;
    let mut updated = String::with_capacity(lines[index].len() + new.len());
    updated.push_str(&lines[index][..offset]);
    updated.push_str(new);
    updated.push_str(&lines[index][offset + length..]);
    let mut out = String::with_capacity(text.len() + new.len());
    for (position, candidate) in lines.iter().enumerate() {
        out.push_str(if position == index {
            &updated
        } else {
            candidate
        });
    }
    Some(out)
}

fn apply_key_edits(
    run: &mut Run,
    path: &Path,
    text: &str,
    data: &mut Value,
    section: Option<&str>,
    edits: &[KeyEdit],
) {
    if edits.is_empty() {
        return;
    }
    let rel = run.rel(path);
    let mut descriptions = Vec::new();
    for edit in edits {
        let dropped = if edit.drops {
            mapping_view(data, section)
                .and_then(|mapping| mapping.get(Value::String(edit.old.clone())).cloned())
        } else {
            None
        };
        descriptions.push(edit.describe(&rel, dropped.as_ref()));
    }
    let mut edited: Option<String> = if edits.iter().any(|edit| edit.drops) {
        None
    } else {
        let mut current = text.to_string();
        let mut ok = true;
        for edit in edits {
            match rename_key_in_text(&current, section, &edit.old, &edit.new) {
                Some(next) => current = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        ok.then_some(current)
    };
    if edited.is_none() {
        if let Some(mapping) = mapping_view_mut(data, section) {
            for edit in edits {
                if edit.drops {
                    mapping.remove(Value::String(edit.old.clone()));
                } else {
                    rename_key(mapping, &edit.old, &edit.new);
                }
            }
        }
        edited = Some(dump(data));
    }
    run.report.rewritten.extend(descriptions);
    if let Some(edited) = edited {
        run.write_text(path, &edited);
    }
}

fn mapping_view<'a>(data: &'a Value, section: Option<&str>) -> Option<&'a Mapping> {
    match section {
        None => data.as_mapping(),
        Some(section) => data
            .as_mapping()
            .and_then(|mapping| mapping_field(mapping, section)),
    }
}

fn mapping_view_mut<'a>(data: &'a mut Value, section: Option<&str>) -> Option<&'a mut Mapping> {
    match section {
        None => data.as_mapping_mut(),
        Some(section) => data
            .as_mapping_mut()
            .and_then(|mapping| mapping_field_mut(mapping, section)),
    }
}

fn key_edit(
    data: &Value,
    section: Option<&str>,
    old: &str,
    new: &str,
    label: &str,
) -> Vec<KeyEdit> {
    let Some(mapping) = mapping_view(data, section) else {
        return Vec::new();
    };
    if !mapping.contains_key(Value::String(old.to_string())) {
        return Vec::new();
    }
    vec![KeyEdit {
        old: old.to_string(),
        new: new.to_string(),
        label: label.to_string(),
        drops: mapping.contains_key(Value::String(new.to_string())),
    }]
}

/// Apply `doctrine_pack_id` → `charter_pack_id` to every activation entry, in
/// place, and return how many entries carried the legacy key.
fn fix_activation_pack_ids(data: &mut Value) -> usize {
    let mut count = 0;
    let Some(root) = data.as_mapping_mut() else {
        return count;
    };
    count += fix_pack_ids_in_holder(root);
    if let Some(Value::Mapping(inner)) = root.get_mut(Value::String(GOVERNANCE_KEY.to_string())) {
        count += fix_pack_ids_in_holder(inner);
    }
    count
}

fn fix_pack_ids_in_holder(holder: &mut Mapping) -> usize {
    let mut count = 0;
    let Some(Value::Sequence(entries)) = holder.get_mut(Value::String(ACTIVATIONS_KEY.to_string()))
    else {
        return count;
    };
    let old = Value::String(LEGACY_PACK_ID_KEY.to_string());
    let new = Value::String(PACK_ID_KEY.to_string());
    for entry in entries.iter_mut() {
        let Value::Mapping(entry) = entry else {
            continue;
        };
        if !entry.contains_key(&old) {
            continue;
        }
        if entry.contains_key(&new) {
            entry.remove(&old);
        } else {
            rename_key(entry, LEGACY_PACK_ID_KEY, PACK_ID_KEY);
        }
        count += 1;
    }
    count
}

fn pack_name(local_path: Option<&Value>, taken: &BTreeSet<String>) -> String {
    let raw = match local_path {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };
    let base = raw.replace('\\', "/");
    let base = base.rsplit('/').next().unwrap_or_default();
    let mut name: String = base
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_lowercase() || character.is_ascii_digit() {
                character
            } else {
                '-'
            }
        })
        .collect();
    while name.contains("--") {
        name = name.replace("--", "-");
    }
    let name = name.trim_matches('-').to_string();
    let mut name = if name.is_empty() || name == RESERVED_PACK_NAME {
        FALLBACK_PACK_NAME.to_string()
    } else {
        name
    };
    let mut suffix = 2usize;
    while taken.contains(&name) {
        name = format!(
            "{}-{suffix}",
            name.trim_end_matches(|c: char| c == '-' || c.is_ascii_digit())
        );
        suffix += 1;
    }
    name
}

fn canonical_org(config: &Mapping) -> Option<&Mapping> {
    mapping_field(config, CHARTER_PACKS_KEY).and_then(|section| mapping_field(section, ORG_KEY))
}

/// Set `charter_packs.org` to `org`; `false` when `charter_packs` is not a mapping.
fn place_canonical_org(config: &mut Mapping, org: Mapping, anchor: &str) -> bool {
    let key = Value::String(CHARTER_PACKS_KEY.to_string());
    match config.get_mut(&key) {
        None => {
            let mut section = Mapping::new();
            section.insert(Value::String(ORG_KEY.to_string()), Value::Mapping(org));
            let mut rebuilt = Mapping::new();
            let anchor_value = Value::String(anchor.to_string());
            let mut inserted = false;
            for (existing, value) in std::mem::take(config) {
                if !inserted && existing == anchor_value {
                    rebuilt.insert(key.clone(), Value::Mapping(section.clone()));
                    inserted = true;
                }
                rebuilt.insert(existing, value);
            }
            if !inserted {
                rebuilt.insert(key, Value::Mapping(section));
            }
            *config = rebuilt;
            true
        }
        Some(Value::Mapping(section)) => {
            section.insert(Value::String(ORG_KEY.to_string()), Value::Mapping(org));
            true
        }
        Some(_) => false,
    }
}

fn drop_empty_legacy_section(run: &mut Run, config: &mut Mapping, rel: &str) {
    let key = Value::String(LEGACY_KEYWORD.to_string());
    let Some(Value::Mapping(section)) = config.get(&key) else {
        return;
    };
    if section.is_empty() {
        config.remove(&key);
    } else {
        let names: Vec<String> = section
            .keys()
            .map(|key| key.as_str().unwrap_or_default().to_string())
            .collect();
        run.report.kept_for_review.push(format!(
            "{rel}: {LEGACY_KEYWORD}: keeps {} (not a charter-pack key)",
            names.join(", ")
        ));
    }
}

/// One named pack entry built from the single-pack legacy fields of `org`.
fn single_pack_org(org: &Mapping) -> (Mapping, String) {
    let mut entry = Mapping::new();
    let name = pack_name(
        org.get(Value::String(LOCAL_PATH_KEY.to_string())),
        &BTreeSet::new(),
    );
    entry.insert(
        Value::String("name".to_string()),
        Value::String(name.clone()),
    );
    for field in SINGLE_PACK_FIELDS {
        if let Some(value) = org.get(Value::String(field.to_string())) {
            entry.insert(Value::String(field.to_string()), value.clone());
        }
    }
    let mut wrapper = Mapping::new();
    wrapper.insert(
        Value::String(PACKS_KEY.to_string()),
        Value::Sequence(vec![Value::Mapping(entry)]),
    );
    (wrapper, name)
}

/// `doctrine.org` → `charter_packs.org`.
fn rewrite_legacy_org(run: &mut Run, config: &mut Mapping, rel: &str) -> bool {
    let Some(legacy_section) = config.get(Value::String(LEGACY_KEYWORD.to_string())) else {
        return false;
    };
    let Some(legacy_section) = legacy_section.as_mapping() else {
        return false;
    };
    let Some(org) = legacy_section.get(Value::String(ORG_KEY.to_string())) else {
        return false;
    };
    let Some(org) = org.as_mapping() else {
        return false;
    };
    let has_list = org.contains_key(Value::String(PACKS_KEY.to_string()));
    let has_single = org.contains_key(Value::String(LOCAL_PATH_KEY.to_string()));
    if !has_list && !has_single {
        return false;
    }
    let legacy_label = if has_list {
        format!("{LEGACY_KEYWORD}.{ORG_KEY}.{PACKS_KEY}")
    } else {
        format!("{LEGACY_KEYWORD}.{ORG_KEY} (single-pack form)")
    };
    let canonical_present = canonical_org(config).is_some();
    let legacy_section = legacy_section.clone();
    let org = org.clone();
    if canonical_present {
        let dropped = compact(&Value::Mapping(org));
        if let Some(section) = config.get_mut(Value::String(LEGACY_KEYWORD.to_string()))
            && let Some(section) = section.as_mapping_mut()
        {
            section.remove(Value::String(ORG_KEY.to_string()));
        }
        run.report.rewritten.push(format!(
            "{rel}: dropped {legacy_label} ({CHARTER_PACKS_KEY}.{ORG_KEY} is already present; the canonical value wins; dropped value: {dropped})"
        ));
    } else if has_list {
        let placed = place_canonical_org(config, org.clone(), LEGACY_KEYWORD);
        if !placed {
            run.report
                .kept_for_review
                .push(format!("{rel}: {legacy_label} kept ({NOT_A_MAPPING})"));
            return false;
        }
        if let Some(section) = config.get_mut(Value::String(LEGACY_KEYWORD.to_string()))
            && let Some(section) = section.as_mapping_mut()
        {
            section.remove(Value::String(ORG_KEY.to_string()));
        }
        run.report.rewritten.push(format!(
            "{rel}: {legacy_label} -> {CHARTER_PACKS_KEY}.{ORG_KEY}.{PACKS_KEY}"
        ));
    } else {
        let (wrapper, name) = single_pack_org(&org);
        if !place_canonical_org(config, wrapper, LEGACY_KEYWORD) {
            run.report
                .kept_for_review
                .push(format!("{rel}: {legacy_label} kept ({NOT_A_MAPPING})"));
            return false;
        }
        let mut remainder = org.clone();
        for field in SINGLE_PACK_FIELDS {
            remainder.remove(Value::String(field.to_string()));
        }
        if let Some(section) = config.get_mut(Value::String(LEGACY_KEYWORD.to_string()))
            && let Some(section) = section.as_mapping_mut()
        {
            if remainder.is_empty() {
                section.remove(Value::String(ORG_KEY.to_string()));
            } else {
                section.insert(
                    Value::String(ORG_KEY.to_string()),
                    Value::Mapping(remainder),
                );
            }
        }
        run.report.rewritten.push(format!(
            "{rel}: {legacy_label} -> {CHARTER_PACKS_KEY}.{ORG_KEY}.{PACKS_KEY} entry named {}",
            sk_kernel::python_repr::python_repr(&name)
        ));
    }
    let _ = legacy_section;
    drop_empty_legacy_section(run, config, rel);
    true
}

/// Whether an `organisation_packs[]` entry is expressible as a canonical entry.
fn convertible_org_pack(entry: &Value) -> bool {
    let Some(mapping) = entry.as_mapping() else {
        return false;
    };
    let source = mapping
        .get(Value::String("source".to_string()))
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .unwrap_or_else(|| LOCAL_PATH_KEY.to_string());
    mapping
        .get(Value::String("name".to_string()))
        .is_some_and(Value::is_string)
        && mapping
            .get(Value::String("path".to_string()))
            .is_some_and(Value::is_string)
        && source == LOCAL_PATH_KEY
}

/// `organisation_packs[]` → `charter_packs.org.packs`.
fn rewrite_organisation_packs(run: &mut Run, config: &mut Mapping, rel: &str) -> bool {
    let key = Value::String(ORGANISATION_PACKS_KEYWORD.to_string());
    let Some(flat) = config.get(&key) else {
        return false;
    };
    let entries = match flat.as_sequence() {
        Some(entries) => entries.clone(),
        None => {
            run.report.kept_for_review.push(format!(
                "{rel}: {ORGANISATION_PACKS_KEYWORD} kept (no entry converts to a local_path pack)"
            ));
            return false;
        }
    };
    let converted: Vec<Value> = entries
        .iter()
        .filter(|entry| convertible_org_pack(entry))
        .map(|entry| {
            let mapping = entry.as_mapping().expect("checked mapping");
            let mut converted = Mapping::new();
            converted.insert(
                Value::String("name".to_string()),
                mapping
                    .get(Value::String("name".to_string()))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            converted.insert(
                Value::String(LOCAL_PATH_KEY.to_string()),
                mapping
                    .get(Value::String("path".to_string()))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            Value::Mapping(converted)
        })
        .collect();
    if converted.is_empty() {
        run.report.kept_for_review.push(format!(
            "{rel}: {ORGANISATION_PACKS_KEYWORD} kept (no entry converts to a local_path pack)"
        ));
        return false;
    }
    if canonical_org(config).is_some() {
        let dropped = compact(&Value::Sequence(entries));
        config.remove(&key);
        run.report.rewritten.push(format!(
            "{rel}: dropped {ORGANISATION_PACKS_KEYWORD} ({CHARTER_PACKS_KEY}.{ORG_KEY} is already present; the canonical value wins; dropped value: {dropped})"
        ));
        return true;
    }
    let mut wrapper = Mapping::new();
    wrapper.insert(
        Value::String(PACKS_KEY.to_string()),
        Value::Sequence(converted.clone()),
    );
    if !place_canonical_org(config, wrapper, ORGANISATION_PACKS_KEYWORD) {
        run.report.kept_for_review.push(format!(
            "{rel}: {ORGANISATION_PACKS_KEYWORD} kept ({NOT_A_MAPPING})"
        ));
        return false;
    }
    let names: Vec<String> = converted
        .iter()
        .filter_map(|entry| entry.as_mapping())
        .filter_map(|entry| string_field(entry, "name"))
        .collect();
    run.report.rewritten.push(format!(
        "{rel}: {ORGANISATION_PACKS_KEYWORD} -> {CHARTER_PACKS_KEY}.{ORG_KEY}.{PACKS_KEY} ({})",
        names.join(", ")
    ));
    let kept: Vec<Value> = entries
        .into_iter()
        .filter(|entry| !convertible_org_pack(entry))
        .collect();
    if kept.is_empty() {
        config.remove(&key);
    } else {
        config.insert(key, Value::Sequence(kept.clone()));
        let count = if kept.len() == 1 {
            "1 entry".to_string()
        } else {
            format!("{} entries", kept.len())
        };
        run.report.kept_for_review.push(format!(
            "{rel}: {ORGANISATION_PACKS_KEYWORD} keeps {count} with a source other than local_path"
        ));
    }
    true
}

fn rewrite_config_yaml(run: &mut Run) -> Result<(), InstallError> {
    let path = run.project.join(CONFIG_REL);
    let Some(text) = read_text(&path, &run.project)? else {
        return Ok(());
    };
    if !text.contains(LEGACY_KEYWORD) && !text.contains(ORGANISATION_PACKS_KEYWORD) {
        return Ok(());
    }
    let mut config = load_yaml(&text, &path, &run.project)?;
    let Some(mapping) = config.as_mapping_mut() else {
        return Ok(());
    };
    let rel = run.rel(&path);
    let mut changed = rewrite_legacy_org(run, mapping, &rel);
    changed = rewrite_organisation_packs(run, mapping, &rel) || changed;
    let governance_edits = key_edit(
        &config,
        Some(GOVERNANCE_KEY),
        LEGACY_KEYWORD,
        CHARTER_KEY,
        &format!("{GOVERNANCE_KEY}.{LEGACY_KEYWORD}"),
    );
    let tracker_edits = key_edit(
        &config,
        Some(TRACKER_KEY),
        LEGACY_KEYWORD,
        OWNERSHIP_KEY,
        &format!("{TRACKER_KEY}.{LEGACY_KEYWORD}"),
    );
    let edits: Vec<KeyEdit> = governance_edits
        .iter()
        .chain(tracker_edits.iter())
        .cloned()
        .collect();
    if changed {
        let rel = run.rel(&path);
        for edit in &edits {
            let dropped = mapping_view(&config, Some(GOVERNANCE_KEY))
                .and_then(|mapping| mapping.get(Value::String(edit.old.clone())).cloned())
                .or_else(|| {
                    mapping_view(&config, Some(TRACKER_KEY))
                        .and_then(|mapping| mapping.get(Value::String(edit.old.clone())).cloned())
                });
            run.report
                .rewritten
                .push(edit.describe(&rel, dropped.as_ref()));
        }
        if let Some(mapping) = config.as_mapping_mut() {
            for (section, old, new) in [
                (GOVERNANCE_KEY, LEGACY_KEYWORD, CHARTER_KEY),
                (TRACKER_KEY, LEGACY_KEYWORD, OWNERSHIP_KEY),
            ] {
                if let Some(inner) = mapping_field_mut(mapping, section) {
                    if inner.contains_key(Value::String(new.to_string())) {
                        inner.remove(Value::String(old.to_string()));
                    } else {
                        rename_key(inner, old, new);
                    }
                }
            }
        }
        run.write_text(&path, &dump(&config));
        return Ok(());
    }
    if governance_edits.is_empty() && tracker_edits.is_empty() {
        return Ok(());
    }
    if !governance_edits.is_empty() {
        apply_key_edits(
            run,
            &path,
            &text,
            &mut config,
            Some(GOVERNANCE_KEY),
            &governance_edits,
        );
    }
    if !tracker_edits.is_empty() {
        let text_after = read_text(&path, &run.project)?.unwrap_or_else(|| text.clone());
        let mut reloaded = load_yaml(&text_after, &path, &run.project)?;
        apply_key_edits(
            run,
            &path,
            &text_after,
            &mut reloaded,
            Some(TRACKER_KEY),
            &tracker_edits,
        );
    }
    Ok(())
}

fn config_mapping(run: &Run) -> Result<Mapping, InstallError> {
    let path = run.project.join(CONFIG_REL);
    let Some(text) = read_text(&path, &run.project)? else {
        return Ok(Mapping::new());
    };
    let data = load_yaml(&text, &path, &run.project)?;
    Ok(data.as_mapping().cloned().unwrap_or_default())
}

/// The `charter.yaml` the config's `charter:` pointer names, else the bundle default.
fn charter_yaml_path(run: &Run) -> Result<PathBuf, InstallError> {
    let config = config_mapping(run)?;
    let pointer = string_field(&config, CHARTER_KEY);
    if let Some(pointer) = pointer
        && !pointer.trim().is_empty()
    {
        let path = PathBuf::from(&pointer);
        return Ok(if path.is_absolute() {
            path
        } else {
            run.project.join(path)
        });
    }
    Ok(run.project.join(".kittify/charter/charter.yaml"))
}

fn governance_file_path(run: &Run) -> PathBuf {
    run.project.join(GOVERNANCE_REL)
}

/// Load a candidate document when the cheap prefilter hits.
fn load_candidate(
    run: &Run,
    path: &Path,
    predicate: impl Fn(&str) -> bool,
) -> Result<Option<(String, Value)>, InstallError> {
    let Some(text) = read_text(path, &run.project)? else {
        return Ok(None);
    };
    if !predicate(&text) {
        return Ok(None);
    }
    let data = load_yaml(&text, path, &run.project)?;
    Ok(Some((text, data)))
}

/// The retired-key prefilter: a key line naming the retired keyword or pack-id key.
fn legacy_key_line(text: &str) -> bool {
    // The reference's `_LEGACY_KEY_LINE`: the retired selection keyword OR the
    // retired pack-id key, at any depth, optionally behind a `- ` marker.
    const KEYS: [&str; 2] = [LEGACY_KEYWORD, LEGACY_PACK_ID_KEY];
    text.lines().any(|line| {
        let trimmed = line.trim_start().trim_start_matches("- ").trim_start();
        KEYS.iter().any(|key| {
            ["", "\"", "'"].iter().any(|quote| {
                let token = format!("{quote}{key}{quote}");
                trimmed.starts_with(&token) && trimmed[token.len()..].trim_start().starts_with(':')
            })
        })
    })
}

fn top_level_legacy_key_line(text: &str) -> bool {
    ["", "\"", "'"].iter().any(|quote| {
        let token = format!("{quote}{LEGACY_KEYWORD}{quote}");
        text.lines().any(|line| {
            let trimmed = line.trim_start();
            line.len() == trimmed.len()
                && trimmed.starts_with(&token)
                && trimmed[token.len()..].trim_start().starts_with(':')
        })
    })
}

/// `governance.doctrine` in the pointed `charter.yaml`; top-level `doctrine:` in
/// the standalone `governance.yaml`.
fn rewrite_governance_selection(run: &mut Run) -> Result<(), InstallError> {
    let charter = charter_yaml_path(run)?;
    if let Some((text, mut data)) = load_candidate(run, &charter, legacy_key_line)? {
        let edits = key_edit(
            &data,
            Some(GOVERNANCE_KEY),
            LEGACY_KEYWORD,
            CHARTER_KEY,
            &format!("{GOVERNANCE_KEY}.{LEGACY_KEYWORD}"),
        );
        apply_key_edits(
            run,
            &charter,
            &text,
            &mut data,
            Some(GOVERNANCE_KEY),
            &edits,
        );
    }
    let governance = governance_file_path(run);
    if let Some((text, mut data)) = load_candidate(run, &governance, top_level_legacy_key_line)? {
        let edits = key_edit(&data, None, LEGACY_KEYWORD, CHARTER_KEY, LEGACY_KEYWORD);
        apply_key_edits(run, &governance, &text, &mut data, None, &edits);
    }
    Ok(())
}

fn rewrite_answers(run: &mut Run) -> Result<(), InstallError> {
    let path = run.project.join(ANSWERS_REL);
    if let Some((text, mut data)) = load_candidate(run, &path, top_level_legacy_key_line)? {
        let edits = key_edit(&data, None, LEGACY_KEYWORD, CHARTER_KEY, LEGACY_KEYWORD);
        apply_key_edits(run, &path, &text, &mut data, None, &edits);
    }
    Ok(())
}

fn rewrite_config_keys(run: &mut Run) -> Result<(), InstallError> {
    rewrite_config_yaml(run)?;
    rewrite_governance_selection(run)?;
    rewrite_answers(run)
}

// ---------------------------------------------------------------------------
// Step 5: doctrine_pack_id -> charter_pack_id
// ---------------------------------------------------------------------------

fn rewrite_activation_pack_ids(run: &mut Run) -> Result<(), InstallError> {
    let mut paths = vec![charter_yaml_path(run)?, governance_file_path(run)];
    paths.dedup();
    for path in paths {
        let Some((text, mut data)) = load_candidate(run, &path, legacy_key_line)? else {
            continue;
        };
        if fix_activation_pack_ids(&mut data) == 0 {
            continue;
        }
        let rel = run.rel(&path);
        let edited = rename_pack_id_lines(&text);
        run.report.rewritten.push(format!(
            "{rel}: {ACTIVATIONS_KEY}[].{LEGACY_PACK_ID_KEY} -> {ACTIVATIONS_KEY}[].{PACK_ID_KEY}"
        ));
        run.write_text(&path, &edited);
    }
    Ok(())
}

/// Rename `doctrine_pack_id:` to `charter_pack_id:` on every line that carries
/// the key (optionally behind a `- ` sequence marker), keeping every other byte.
fn rename_pack_id_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match key_token_span(line, LEGACY_PACK_ID_KEY) {
            Some((offset, length)) => {
                out.push_str(&line[..offset]);
                out.push_str(PACK_ID_KEY);
                out.push_str(&line[offset + length..]);
            }
            None => out.push_str(line),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The legacy-layout predicate (`legacy_charter_layout.detect_legacy_charter_layout`)
// ---------------------------------------------------------------------------

/// Finding: the retired project layer exists (a directory or a symlink).
pub const LEGACY_PROJECT_ROOT_FINDING: &str = "legacy_project_root";
/// Finding: the retired project layer could not be inspected.
pub const UNREADABLE_PROJECT_ROOT: &str = "unreadable_project_root";
/// Finding: a standalone `governance.yaml` with the retired selection key.
pub const LEGACY_GOVERNANCE_FILE: &str = "legacy_governance_file";
/// Finding: the standalone `governance.yaml` is not readable YAML.
pub const UNREADABLE_GOVERNANCE_FILE: &str = "unreadable_governance_file";
/// Finding: `.kittify/config.yaml` is not readable YAML.
pub const UNREADABLE_CONFIG: &str = "unreadable_config";
/// Finding: `doctrine.org` carries a pack list or the single-pack form.
pub const LEGACY_ORG_PACKS_KEY: &str = "legacy_org_packs_key";
/// Finding: `organisation_packs` holds a convertible entry.
pub const LEGACY_ORGANISATION_PACKS_KEY: &str = "legacy_organisation_packs_key";
/// Finding: `governance.doctrine` in `config.yaml`.
pub const LEGACY_GOVERNANCE_SELECTION_KEY: &str = "legacy_governance_selection_key";
/// Finding: `tracker.doctrine` in `config.yaml`.
pub const LEGACY_TRACKER_OWNERSHIP_KEY: &str = "legacy_tracker_ownership_key";

/// The findings that name a retired key in `config.yaml`.
pub const CONFIG_KEY_FINDINGS: [&str; 4] = [
    LEGACY_ORG_PACKS_KEY,
    LEGACY_ORGANISATION_PACKS_KEY,
    LEGACY_GOVERNANCE_SELECTION_KEY,
    LEGACY_TRACKER_OWNERSHIP_KEY,
];

const PREFILTER_KEYWORDS: [&str; 2] = ["doctrine", ORGANISATION_PACKS_KEYWORD];

/// `doctrine.org` when it carries a pack list or the single-pack form.
pub fn legacy_org_block(config: &Mapping) -> Option<&Mapping> {
    let section = config.get(Value::String(LEGACY_KEYWORD.to_string()))?;
    let org = section
        .as_mapping()?
        .get(Value::String(ORG_KEY.to_string()))?;
    let org = org.as_mapping()?;
    let forms = [PACKS_KEY, LOCAL_PATH_KEY];
    forms
        .iter()
        .any(|field| org.contains_key(Value::String(field.to_string())))
        .then_some(org)
}

/// True for an `organisation_packs[]` entry expressible as a canonical entry.
pub fn is_convertible_organisation_pack(entry: &Value) -> bool {
    if !convertible_org_pack(entry) {
        return false;
    }
    entry
        .as_mapping()
        .and_then(|mapping| mapping.get(Value::String("source".to_string())))
        .is_none_or(|source| source.as_str() == Some(LOCAL_PATH_KEY))
}

fn config_findings(data: &Mapping) -> Vec<&'static str> {
    let mut findings = Vec::new();
    if legacy_org_block(data).is_some() {
        findings.push(LEGACY_ORG_PACKS_KEY);
    }
    if let Some(Value::Sequence(flat)) =
        data.get(Value::String(ORGANISATION_PACKS_KEYWORD.to_string()))
        && flat.iter().any(is_convertible_organisation_pack)
    {
        findings.push(LEGACY_ORGANISATION_PACKS_KEY);
    }
    for (section, finding) in [
        (GOVERNANCE_KEY, LEGACY_GOVERNANCE_SELECTION_KEY),
        (TRACKER_KEY, LEGACY_TRACKER_OWNERSHIP_KEY),
    ] {
        if mapping_field(data, section)
            .is_some_and(|block| block.contains_key(Value::String(LEGACY_KEYWORD.to_string())))
        {
            findings.push(finding);
        }
    }
    findings
}

/// `(readable, data)`: `data` is `None` when the prefilter misses.
fn load_prefiltered(path: &Path) -> (bool, Option<Mapping>) {
    let Ok(raw) = std::fs::read(path) else {
        return match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (true, None),
            Err(error) if error.kind() == std::io::ErrorKind::NotADirectory => (true, None),
            _ => (false, None),
        };
    };
    if !PREFILTER_KEYWORDS.iter().any(|keyword| {
        raw.windows(keyword.len())
            .any(|window| window == keyword.as_bytes())
    }) {
        return (true, None);
    }
    match String::from_utf8(raw) {
        Ok(text) => match serde_yaml_ng::from_str::<Value>(&text) {
            Ok(Value::Mapping(mapping)) => (true, Some(mapping)),
            Ok(_) => (true, None),
            Err(_) => (false, None),
        },
        Err(_) => (false, None),
    }
}

fn legacy_root_finding(root: &Path) -> Option<&'static str> {
    match std::fs::symlink_metadata(root.join(LEGACY_PROJECT_ROOT)) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || metadata.is_dir() {
                Some(LEGACY_PROJECT_ROOT_FINDING)
            } else {
                None
            }
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.kind() == std::io::ErrorKind::NotADirectory =>
        {
            None
        }
        Err(_) => Some(UNREADABLE_PROJECT_ROOT),
    }
}

/// The structural legacy charter-layout findings of the project at `root`.
///
/// An empty vector means the project carries none of them. This never fails: an
/// unreadable or unparseable file is a finding.
pub fn detect_legacy_charter_layout(root: &Path) -> Vec<&'static str> {
    let mut findings = Vec::new();
    if let Some(finding) = legacy_root_finding(root) {
        findings.push(finding);
    }
    let (readable, governance) = load_prefiltered(&root.join(GOVERNANCE_REL));
    if !readable {
        findings.push(UNREADABLE_GOVERNANCE_FILE);
    } else if governance
        .is_some_and(|data| data.contains_key(Value::String(LEGACY_KEYWORD.to_string())))
    {
        findings.push(LEGACY_GOVERNANCE_FILE);
    }
    let (readable, config) = load_prefiltered(&root.join(CONFIG_REL));
    if !readable {
        findings.push(UNREADABLE_CONFIG);
    } else if let Some(config) = config {
        findings.extend(config_findings(&config));
    }
    findings
}

// ---------------------------------------------------------------------------
// The migration
// ---------------------------------------------------------------------------

fn require_readable_layout(project: &Path) -> Result<(), InstallError> {
    let findings = detect_legacy_charter_layout(project);
    if findings.contains(&UNREADABLE_PROJECT_ROOT) {
        return Err(InstallError::MigrationStateUnreadable {
            path: Some(project.join(LEGACY_PROJECT_ROOT)),
            detail: format!(
                "{LEGACY_PROJECT_ROOT} could not be inspected (permission or I/O error); check its permissions, then run `spec-kitty upgrade` again"
            ),
        });
    }
    if findings.contains(&UNREADABLE_CONFIG) {
        return Err(InstallError::MigrationStateUnreadable {
            path: Some(project.join(CONFIG_REL)),
            detail: format!("{CONFIG_REL} is not readable YAML; {UPGRADE_REMEDY}"),
        });
    }
    if findings.contains(&UNREADABLE_GOVERNANCE_FILE) {
        return Err(InstallError::MigrationStateUnreadable {
            path: Some(project.join(GOVERNANCE_REL)),
            detail: format!("{GOVERNANCE_REL} is not readable YAML; {UPGRADE_REMEDY}"),
        });
    }
    Ok(())
}

/// Step 6's guard: true while the migration ledger does not record the cutover
/// as applied (`is_settled` is success-only).
pub fn is_first_application(project_root: &Path) -> bool {
    let Ok(catalog) = crate::ledger::MigrationCatalog::embedded() else {
        return true;
    };
    let Some(spec) = catalog.get(MIGRATION_ID) else {
        return true;
    };
    let ledger = crate::ledger::MigrationLedger::new(project_root);
    !ledger.is_settled(spec).unwrap_or(false)
}

fn reset_activation_lists(run: &mut Run) -> Result<(), InstallError> {
    if !is_first_application(&run.project) {
        return Ok(());
    }
    let fragment = resets::run(&run.project, run.dry_run)?;
    run.report.merge(fragment);
    Ok(())
}

fn remove_retired_skill_copies(run: &mut Run) {
    let fragment = skills::run(&run.project, run.dry_run);
    run.report.merge(fragment);
}

type Step = fn(&mut Run) -> Result<(), InstallError>;

/// `apply` steps in contract order; a step that records an error stops the run.
const STEPS: [Step; 7] = [
    |run| {
        preflight(run);
        Ok(())
    },
    |run| {
        move_project_root(run);
        Ok(())
    },
    rewrite_path_references,
    rewrite_config_keys,
    rewrite_activation_pack_ids,
    reset_activation_lists,
    |run| {
        remove_retired_skill_copies(run);
        Ok(())
    },
];

/// The structural-key steps `structural_detect` re-runs in a dry run.
const STRUCTURAL_KEY_STEPS: [Step; 2] = [rewrite_config_keys, rewrite_activation_pack_ids];

fn run_steps(project: &Path, dry_run: bool, steps: &[Step]) -> Result<CutoverReport, InstallError> {
    require_readable_layout(project)?;
    let mut run = Run::new(project, dry_run);
    for step in steps {
        step(&mut run)?;
        if let Some(error) = run.io_error.take() {
            return Err(error);
        }
        if !run.report.errors.is_empty() {
            break;
        }
    }
    Ok(run.report)
}

/// `charter_pack_cutover.detect` — TOTAL: any failure selects the migration.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    if !detect_legacy_charter_layout(project_root).is_empty() {
        return Ok(true);
    }
    Ok(run_steps(project_root, true, &STEPS)
        .map(|report| report.is_actionable())
        .unwrap_or(true))
}

/// `charter_pack_cutover.structural_detect`.
pub fn structural_detect(project_root: &Path) -> Result<bool, InstallError> {
    let findings = detect_legacy_charter_layout(project_root);
    if findings
        .iter()
        .any(|finding| !CONFIG_KEY_FINDINGS.contains(finding))
    {
        return Ok(true);
    }
    let steps: &[Step] = if findings.is_empty() {
        &STRUCTURAL_KEY_STEPS[1..]
    } else {
        &STRUCTURAL_KEY_STEPS
    };
    Ok(run_steps(project_root, true, steps)
        .map(|report| report.is_actionable())
        .unwrap_or(true))
}

/// `charter_pack_cutover.can_apply` — always applicable; the collision preflight
/// runs inside `apply` so a dry run reports it.
pub fn can_apply(_project_root: &Path) -> Result<Option<String>, InstallError> {
    Ok(None)
}

/// `charter_pack_cutover.apply`.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let report = run_steps(project_root, false, &STEPS)?;
    report.to_outcome(project_root, false)
}

/// A dry run (the reference's `apply(dry_run=True)`), for tests and `detect`.
pub fn dry_run(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let report = run_steps(project_root, true, &STEPS)?;
    report.to_outcome(project_root, true)
}

/// The cutover migration id.
pub const MIGRATION_ID: &str = "charter_pack_cutover";

/// The cutover migration.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[NativeMigration {
    id: MIGRATION_ID,
    detect,
    can_apply,
    apply,
}];
