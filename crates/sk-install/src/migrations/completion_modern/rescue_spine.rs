//! Remaining Source mission rescue and runtime cutover migration effects.
//!
//! This module is the *boundary* for the four ids the modern-completion slice
//! cannot express without the Mission domain:
//!
//! | id | real effect |
//! | --- | --- |
//! | `3.0.0_canonical_context` | the 9-step one-shot runner: backup, identity, ownership, event-log rebuild, frontmatter strip, schema stamp, `.gitignore`, derived copy, shim rewrite — with a real rollback |
//! | `3.1.1_charter_rename` | the 4-phase constitution→charter rename across all three legacy layouts, archive-never-delete |
//! | `3.2.0a4_normalize_mission_lifecycle` | the per-mission identity/event-log/derived-view normalization |
//! | `runtime_state_backfill` | the corpus cutover: seed → fail-closed verify → `status_phase` flip, aborting on the first mission whose verify fails |
//!
//! Only orchestration lives here. The seeded event log, the reducer, the
//! snapshot materializer, the lock and the writers are the existing canonical
//! authorities in [`sk_status`]; the Mission identity, ownership, frontmatter,
//! corpus/residue classification and `status_phase` writers are
//! [`sk_mission::migration_rescue`]. This module adds no parser, catalogue,
//! writer or state machine of its own.
//!
//! # One engine, two modes
//!
//! Each id has exactly ONE implementation — [`MigrationMode::Apply`] or
//! [`MigrationMode::Preview`] — and the four callbacks are thin aliases of it, so
//! the read-only preview cannot drift from the write path. Source's
//! `apply(dry_run=True)` branch is preserved text for text ("Would …" actions,
//! `[DRY RUN]` summary, `dry-run: would seed …`), and a preview writes nothing.
//!
//! `NativeMigration::apply` stays non-dry-run: the four preview entry points
//! ([`preview_canonical_context`], [`preview_charter_rename`],
//! [`preview_normalize_mission_lifecycle`], [`preview_runtime_state_backfill`])
//! are the read-only seam, deliberately parallel to the central registry rather
//! than a second registry.
//!
//! # Failure carrier
//!
//! A Source migration that returns `success=False` keeps its report: changes,
//! errors, warnings and the real per-mission `partial_writes`. Every callback
//! below therefore returns `Ok(MigrationOutcome { success: false, .. })` for a
//! returned Source failure instead of collapsing it into a generic refusal. An
//! *uncaught* Source exception stays an error at its own phase.

use std::path::{Path, PathBuf};

use sk_mission::migration_rescue::corpus::MissionCorpus;
use sk_mission::migration_rescue::{CutoverRequest, PlacementPort};

use crate::error::InstallError;
use crate::migrations::completion_modern::support;
use crate::upgrade::{MigrationOutcome, MigrationPartialWrite, NativeMigration};

mod backup;

use backup::Backup;

/// Corpus directory holding Mission trees.
const KITTY_SPECS_DIR: &str = support::KITTY_SPECS_DIR;

/// `.gitignore` entries the canonical-context runner adds.
const GITIGNORE_ENTRIES: [&str; 3] = [
    ".kittify/derived/",
    ".kittify/runtime/",
    ".kittify/.migration-backup/",
];

/// The schema version the runner stamps (Source `CURRENT_SCHEMA_VERSION`).
const CURRENT_SCHEMA_VERSION: i64 = 3;

/// The capability map the runner stamps (Source `CURRENT_SCHEMA_CAPABILITIES`).
const SCHEMA_CAPABILITIES: [(&str, bool); 4] = [
    ("canonical_context", true),
    ("event_log_authority", true),
    ("ownership_manifest", true),
    ("thin_shims", true),
];

/// Text suffixes the charter rename rewrites.
const TEXT_SUFFIXES: [&str; 5] = [".md", ".yaml", ".yml", ".json", ".txt"];

/// The pre-rename agent artifact / command names.
const LEGACY_CONSTITUTION_COMMAND: &str = "spec-kitty.constitution.md";
const CHARTER_COMMAND: &str = "spec-kitty.charter.md";

/// Whether a shared engine run writes, or only reports what Source's own
/// `dry_run=True` branch reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationMode {
    /// Source `apply(dry_run=False)`: the real effect.
    Apply,
    /// Source `apply(dry_run=True)`: the real read-only branch, zero writes.
    Preview,
}

impl MigrationMode {
    /// Source's `dry_run` flag.
    pub fn is_preview(self) -> bool {
        self == MigrationMode::Preview
    }
}

fn io_error(action: &'static str, path: &Path, source: std::io::Error) -> InstallError {
    InstallError::io(action, path, source)
}

/// Source `kernel.atomic.atomic_write`: metadata.yaml identity and schema edits.
///
/// The atomic operation is chosen because the reference writes those two files
/// through `atomic_write` (`migration/backfill_identity.py`,
/// `migration/runner._update_schema_version`), not because of the file name.
fn write_atomic_text(path: &Path, text: &str) -> Result<(), InstallError> {
    sk_kernel::source_writer::write_source(
        sk_kernel::source_writer::WriterOperation::atomic(),
        path,
        sk_kernel::source_writer::SourcePayload::Text(text),
        None,
    )
    .map(|_receipt| ())
    .map_err(|error| InstallError::UpgradeRefused {
        detail: format!("cannot write {}: {error}", path.display()),
    })
}

/// Source `Path.write_text`: the `.gitignore` update.
///
/// `migration/runner._update_gitignore` uses `write_text` for both the create
/// and the update branch, so this operation truncates in place instead of
/// replacing the inode. It is deliberately a different function from
/// [`write_atomic_text`]: same destination style, different source operation.
fn write_truncated_text(path: &Path, text: &str) -> Result<(), InstallError> {
    sk_kernel::source_writer::write_source(
        sk_kernel::source_writer::WriterOperation::TextTruncate,
        path,
        sk_kernel::source_writer::SourcePayload::Text(text),
        None,
    )
    .map(|_receipt| ())
    .map_err(|error| InstallError::UpgradeRefused {
        detail: format!("cannot write {}: {error}", path.display()),
    })
}

/// Write a generated command file, owning the read-only lifecycle.
///
/// Source `specify_cli.runtime.generated_writer.write_generated_file` restores a
/// read-only destination's write bits itself (with `0o222`) before writing and
/// strips them again in a `finally`, so this call site must not pre-`chmod` and
/// must not stamp `0o444` afterwards: both would diverge from the reference's
/// observed modes and from its failure behaviour.
fn write_generated(
    path: &Path,
    text: &str,
    read_only: bool,
) -> Result<(), sk_kernel::source_writer::SourceWriteError> {
    sk_kernel::source_writer::write_source(
        sk_kernel::source_writer::WriterOperation::generated(read_only),
        path,
        sk_kernel::source_writer::SourcePayload::Text(text),
        None,
    )
    .map(|_receipt| ())
}

/// A failure the migration RETURNS (Source `MigrationReport(success=False)`),
/// carrying the step name and the Source-shaped message.
struct StepFailure {
    step: &'static str,
    message: String,
}

impl StepFailure {
    fn new(step: &'static str, message: impl Into<String>) -> Self {
        Self {
            step,
            message: message.into(),
        }
    }

    /// Source `_fail`: `report.errors.append(f"[{step}] {msg}")`.
    fn source_error(&self) -> String {
        format!("[{}] {}", self.step, self.message)
    }
}

// ---------------------------------------------------------------------------
// 3.0.0_canonical_context — the 9-step runner
// ---------------------------------------------------------------------------

/// The `spec_kitty.schema_version` in `.kittify/metadata.yaml`, if declared.
///
/// Source `get_project_schema_version` reads the value with `int(raw)`, so a
/// float (`3.0`), a numeric string or an integer all answer the same way; an
/// unreadable/absent file answers "no version".
fn project_schema_version(project_root: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(project_root.join(".kittify/metadata.yaml")).ok()?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).ok()?;
    let spec_kitty = value.get("spec_kitty")?;
    let raw = spec_kitty.get("schema_version")?;
    match raw {
        serde_yaml_ng::Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64)),
        serde_yaml_ng::Value::String(text) => {
            text.trim().parse::<f64>().ok().map(|v| v.trunc() as i64)
        }
        _ => None,
    }
}

fn can_apply_canonical_context(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !project_root.join(".kittify").is_dir() {
        return Ok(Some(
            "No .kittify directory found — is this a spec-kitty project?".to_string(),
        ));
    }
    if !project_root.join(".kittify/metadata.yaml").exists() {
        return Ok(Some("No .kittify/metadata.yaml found".to_string()));
    }
    Ok(None)
}

fn detect_canonical_context(project_root: &Path) -> Result<bool, InstallError> {
    Ok(project_schema_version(project_root)
        .map(|version| version < CURRENT_SCHEMA_VERSION)
        .unwrap_or(true))
}

/// Stamp `spec_kitty.project_uuid` (idempotent).
///
/// Source logs this step and adds nothing to `changes_made`; the value is the
/// observable effect, not a report line.
fn backfill_project_uuid(project_root: &Path) -> Result<(), StepFailure> {
    let path = project_root.join(".kittify/metadata.yaml");
    let text = std::fs::read_to_string(&path).map_err(|error| {
        // Source raises `FileNotFoundError(f"metadata.yaml not found: {path}")`;
        // the shape of that message is what the operator sees as `fail_msg`.
        let detail = if error.kind() == std::io::ErrorKind::NotFound {
            format!("metadata.yaml not found: {}", path.display())
        } else {
            format!("metadata.yaml not found: {}: {error}", path.display())
        };
        StepFailure::new(
            "identity_backfill",
            format!("metadata.yaml missing: {detail}"),
        )
    })?;
    let existing: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).map_err(|error| {
        StepFailure::new(
            "identity_backfill",
            format!("project UUID backfill failed: metadata.yaml is not parseable YAML: {error}"),
        )
    })?;
    if existing
        .get("spec_kitty")
        .and_then(|value| value.get("project_uuid"))
        .is_some_and(|value| !value.is_null())
    {
        return Ok(());
    }
    let uuid = ulid::Ulid::new().to_string();
    let editor = crate::yaml_edit::YamlEditor::new(&path, text);
    let outcome = editor
        .insert_scalars(
            "spec_kitty",
            &[(
                "project_uuid".to_string(),
                crate::yaml_edit::Scalar::Str(uuid),
            )],
            crate::yaml_edit::InsertPosition::Bottom,
        )
        .map_err(|error| {
            StepFailure::new(
                "identity_backfill",
                format!("project UUID backfill failed: {error}"),
            )
        })?;
    if outcome.changed {
        write_atomic_text(&path, &outcome.text).map_err(|error| {
            StepFailure::new(
                "identity_backfill",
                format!("project UUID backfill failed: {error}"),
            )
        })?;
    }
    Ok(())
}

/// Stamp the schema version, capability map and upgrade timestamp.
fn stamp_schema_version(project_root: &Path) -> Result<(), StepFailure> {
    let step = |message: String| StepFailure::new("schema_version_update", message);
    let path = project_root.join(".kittify/metadata.yaml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| step(format!("schema version update failed: {error}")))?;
    let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).map_err(|error| {
        step(format!(
            "schema version update failed: metadata.yaml is not parseable YAML: {error}"
        ))
    })?;
    let spec_kitty = parsed.get("spec_kitty");
    if spec_kitty.is_some_and(|value| !value.is_mapping() && !value.is_null()) {
        return Err(step(
            "schema version update failed: metadata.yaml spec_kitty must be a mapping".to_string(),
        ));
    }
    let current = text;
    let editor = crate::yaml_edit::YamlEditor::new(&path, current);
    let mut text = editor
        .upsert_scalars(
            "spec_kitty",
            &[
                (
                    "schema_version".to_string(),
                    crate::yaml_edit::Scalar::Int(CURRENT_SCHEMA_VERSION),
                ),
                (
                    "last_upgraded_at".to_string(),
                    crate::yaml_edit::Scalar::Str(sk_kernel::clock::now_utc_iso()),
                ),
            ],
            crate::yaml_edit::InsertPosition::Bottom,
        )
        .map_err(|error| step(format!("schema version update failed: {error}")))?
        .text;
    // The capability map is the canonical four keys when absent (never a list,
    // which older releases wrote); an operator-authored map is never touched.
    let declared = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
        .ok()
        .and_then(|value| value.get("spec_kitty").cloned())
        .and_then(|value| value.get("schema_capabilities").cloned());
    let needs_mapping = match &declared {
        None => true,
        Some(serde_yaml_ng::Value::Mapping(_)) => false,
        Some(_) => true,
    };
    if needs_mapping {
        let editor = crate::yaml_edit::YamlEditor::new(&path, text.clone());
        let outcome = editor
            .insert_mapping(
                "spec_kitty",
                &crate::yaml_edit::MappingEntry {
                    key: "schema_capabilities".to_string(),
                    children: SCHEMA_CAPABILITIES
                        .iter()
                        .map(|(name, value)| {
                            ((*name).to_string(), crate::yaml_edit::Scalar::Bool(*value))
                        })
                        .collect(),
                },
                crate::yaml_edit::InsertPosition::Bottom,
            )
            .map_err(|error| step(format!("schema version update failed: {error}")))?;
        if outcome.changed {
            text = outcome.text;
        } else if declared.is_some() {
            // A legacy list: replace the block with the canonical map.
            let editor = crate::yaml_edit::YamlEditor::new(&path, text.clone());
            let document = crate::yaml_edit::CstDocument::parse(path.clone(), &text)
                .map_err(|error| step(format!("schema version update failed: {error}")))?;
            let root = document.root();
            if let Some(spec_kitty) = root.get_mapping("spec_kitty") {
                spec_kitty.remove("schema_capabilities");
                let replaced = editor
                    .insert_mapping(
                        "spec_kitty",
                        &crate::yaml_edit::MappingEntry {
                            key: "schema_capabilities".to_string(),
                            children: SCHEMA_CAPABILITIES
                                .iter()
                                .map(|(name, value)| {
                                    ((*name).to_string(), crate::yaml_edit::Scalar::Bool(*value))
                                })
                                .collect(),
                        },
                        crate::yaml_edit::InsertPosition::Bottom,
                    )
                    .map_err(|error| step(format!("schema version update failed: {error}")))?;
                if replaced.changed {
                    text = replaced.text;
                } else {
                    text = document.text();
                }
            }
        }
    }
    write_atomic_text(&path, &text)
        .map_err(|error| step(format!("schema version update failed: {error}")))?;
    Ok(())
}

/// Add the canonical `.gitignore` entries, keeping every existing line.
fn update_gitignore(project_root: &Path) -> Result<(), StepFailure> {
    let step = |message: String| StepFailure::new("gitignore_update", message);
    let path = project_root.join(".gitignore");
    if !path.exists() {
        let mut text = GITIGNORE_ENTRIES.join("\n");
        text.push('\n');
        write_truncated_text(&path, &text)
            .map_err(|error| step(format!(".gitignore update failed: {error}")))?;
        return Ok(());
    }
    let mut content = std::fs::read_to_string(&path)
        .map_err(|error| step(format!(".gitignore update failed: {error}")))?;
    let original = content.clone();
    for entry in GITIGNORE_ENTRIES {
        if !content.contains(entry) {
            if !content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str(entry);
            content.push('\n');
        }
    }
    if content != original {
        write_truncated_text(&path, &content)
            .map_err(|error| step(format!(".gitignore update failed: {error}")))?;
    }
    Ok(())
}

/// Copy every Mission's legacy `status.json` into `.kittify/derived/<slug>/`.
///
/// Source `_move_derived_files` skips a residue child (the write-seam rule) and
/// reports only the COUNT (`Moved N derived file(s)`), not one line per file.
fn copy_derived_status(project_root: &Path) -> Result<usize, StepFailure> {
    let step = |message: String| {
        StepFailure::new(
            "move_derived_files",
            format!("derived file move failed: {message}"),
        )
    };
    let corpus = MissionCorpus::new(project_root);
    let sources = corpus
        .derived_file_sources()
        .map_err(|error| step(format!("{error}")))?;
    let mut copied = 0usize;
    for entry in sources {
        if !entry.is_mission() {
            continue;
        }
        let source = entry.path.join("status.json");
        if !source.is_file() {
            continue;
        }
        let destination_dir = project_root
            .join(".kittify")
            .join(sk_mission::migration_rescue::DERIVED_DIR_NAME)
            .join(&entry.slug);
        std::fs::create_dir_all(&destination_dir)
            .map_err(|error| step(format!("{}: {error}", destination_dir.display())))?;
        // `shutil.copy2`: the derived copy keeps the observed mode and mtime.
        backup::copy2(&source, &destination_dir.join("status.json"))
            .map_err(|error| step(format!("{}: {error}", source.display())))?;
        copied += 1;
    }
    Ok(copied)
}

/// The files each WP actually changed on its lane branch (Source
/// `backfill_ownership._git_diff_files`).
///
/// Source only asks Git when the WP frontmatter itself authors
/// `base_branch`/`planning_base_branch`: that authored value is the diff base, and
/// a WP with neither field gets **no diff at all** (no fallback to a shared lane
/// manifest ref). A Git failure is a legitimately empty list for that WP — the
/// same `[]` Source returns — and is never an invented diff.
///
/// Read through the canonical workspace manifest ([`sk_workspace::LanesStore`])
/// and the canonical Git adapter ([`sk_git::GitRepository::diff_name_status`]) —
/// no second manifest reader and no second Git process.
fn lane_changed_files(
    mission_dir: &Path,
    slug: &str,
) -> std::collections::BTreeMap<String, Vec<String>> {
    use sk_workspace::lanes::LanesStore;
    use sk_workspace::naming::code_lane_branch_name;

    let mut changed: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    let Ok(Some(manifest)) = LanesStore::new(mission_dir).read() else {
        return changed;
    };
    let Ok(repository) = sk_git::GitRepository::open(mission_dir) else {
        return changed;
    };
    let Ok(files) = std::fs::read_dir(mission_dir.join("tasks")) else {
        return changed;
    };
    let mut wp_files: Vec<std::path::PathBuf> = files
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("WP"))
        })
        .collect();
    wp_files.sort();
    for path in wp_files {
        let Some(code) = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .and_then(|stem| stem.split('-').next().map(str::to_string))
        else {
            continue;
        };
        // The AUTHORED diff base, and only when the WP declares one.
        let Some(base) = authored_diff_base(&path) else {
            continue;
        };
        let Ok(wp) = code.parse::<sk_kernel::WpId>() else {
            continue;
        };
        let Some(lane) = manifest.lane_for_wp(&wp) else {
            continue;
        };
        let branch = if lane.is_planning() {
            manifest.target_branch.clone()
        } else {
            match code_lane_branch_name(slug, &lane.lane_id) {
                Ok(name) => name,
                Err(_) => continue,
            }
        };
        let Ok(base_name) = sk_git::BranchName::parse(&base) else {
            continue;
        };
        let Ok(branch_name) = sk_git::BranchName::parse(&branch) else {
            continue;
        };
        let (Ok(Some(base_oid)), Ok(Some(tip_oid))) = (
            repository.read_branch_tip(&base_name),
            repository.read_branch_tip(&branch_name),
        ) else {
            continue;
        };
        let Ok(entries) = repository.diff_name_status(&base_oid, &tip_oid) else {
            continue;
        };
        let mut paths: Vec<String> = Vec::new();
        for entry in entries {
            let destination = entry
                .path
                .to_path_buf()
                .to_string_lossy()
                .replace('\\', "/");
            if !destination.is_empty() && !paths.contains(&destination) {
                paths.push(destination);
            }
            if let Some(source) = &entry.source {
                let origin = source.to_path_buf().to_string_lossy().replace('\\', "/");
                if !origin.is_empty() && !paths.contains(&origin) {
                    paths.push(origin);
                }
            }
        }
        paths.sort();
        if !paths.is_empty() {
            changed.insert(code, paths);
        }
    }
    changed
}

/// `base_branch` or `planning_base_branch` from a WP's own frontmatter.
fn authored_diff_base(wp_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(wp_path).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let split = sk_mission::split_frontmatter(text).ok()?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(split.yaml).ok()?;
    for key in ["base_branch", "planning_base_branch"] {
        if let Some(raw) = value.get(key).and_then(|value| value.as_str()) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Rewrite the agent command shims (non-fatal, like Source step 9).
fn rewrite_shims(project_root: &Path, warnings: &mut Vec<String>) {
    let templates = crate::migrations::completion_legacy::shared::pack_missions_root()
        .map(crate::render::TemplateSource::new);
    match crate::render::shim::rewrite_agent_shims(
        project_root,
        sk_kernel::COMPAT_CORE_VERSION,
        templates.as_ref(),
        false,
    ) {
        Ok(_) => {}
        Err(error) => warnings.push(format!("Shim rewrite warning (non-fatal): {error}")),
    }
}

#[derive(Default)]
struct RunStats {
    wps: usize,
    events: usize,
    derived: usize,
}

/// How the `3.0.0` runner body ended.
///
/// Source's `run_migration` has exactly two failure shapes and they are NOT the
/// same carrier:
///
/// * a step failure it RETURNS through `_fail` — the report survives and the
///   backup is rolled back and settled;
/// * an exception that escapes `run_migration` (the `for feature_dir in
///   _discover_features(...)` walks and the rollback settlement sit outside any
///   `try`) — the `apply` wrapper turns it into a fresh
///   `Migration failed with exception: …` result, and nothing is rolled back or
///   cleaned up.
enum RunFailure {
    /// Source `_fail`: rollback runs, then the report is returned.
    Returned(StepFailure),
    /// An exception escaping `run_migration`: the `apply` wrapper's carrier.
    Escaped(InstallError),
}

impl From<StepFailure> for RunFailure {
    fn from(failure: StepFailure) -> Self {
        Self::Returned(failure)
    }
}

impl From<InstallError> for RunFailure {
    fn from(error: InstallError) -> Self {
        Self::Escaped(error)
    }
}

/// The `3.0.0` boundary: Source `M300CanonicalContext.apply`.
fn execute_canonical_context(
    project_root: &Path,
    mode: MigrationMode,
) -> Result<MigrationOutcome, InstallError> {
    // Source wraps `run_migration` in ONE `try/except` and builds a FRESH
    // `MigrationResult(success=False, errors=[f"Migration failed with exception: {exc}"])`
    // in the handler: the in-progress report's changes and warnings are discarded,
    // and neither the rollback nor the backup cleanup runs (both live on the
    // returned-failure path INSIDE `run_migration`). Catching here — at the same
    // stage as the reference — is what keeps a step failure and an escape distinct.
    match run_canonical_context(project_root, mode) {
        Ok(outcome) => Ok(outcome),
        Err(error) => Ok(exception_carrier(&error)),
    }
}

/// Source `apply`'s exception branch, verbatim.
fn exception_carrier(error: &InstallError) -> MigrationOutcome {
    MigrationOutcome {
        changed: false,
        success: false,
        errors: vec![format!("Migration failed with exception: {error}")],
        ..MigrationOutcome::default()
    }
}

/// The `3.0.0` engine body: Source `run_migration(project_root, dry_run)`.
fn run_canonical_context(
    project_root: &Path,
    mode: MigrationMode,
) -> Result<MigrationOutcome, InstallError> {
    // Source's dry run short-circuits before the backup: it reports the would-be
    // feature count and writes nothing at all.
    if mode.is_preview() {
        let features = MissionCorpus::new(project_root)
            .discover_features()
            .map_err(|error| {
                io_error("read directory", &project_root.join(KITTY_SPECS_DIR), error)
            })?
            .len();
        return Ok(MigrationOutcome {
            changed: true,
            success: true,
            changes: vec![format!(
                "[DRY RUN] Migrated {features} feature(s), 0 WPs backfilled, 0 events generated"
            )],
            warnings: vec![format!(
                "DRY RUN: would migrate {features} feature(s) to schema v3"
            )],
            ..MigrationOutcome::default()
        });
    }

    let mut changes: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut stats = RunStats::default();

    // Step 1: backup. A failure here is NON-FATAL (Source appends the warning
    // and the nine steps still run).
    let backup_step = backup::create_backup(project_root);
    if let Some(warning) = &backup_step.warning {
        warnings.push(warning.clone());
    }
    let backup: Option<Backup> = backup_step.backup;

    if let Err(failure) = run_steps(project_root, &mut warnings, &mut stats) {
        let order = match failure {
            RunFailure::Returned(failure) => failure,
            // Source never rolls back an escaping exception: that path has no
            // report to settle and never reaches `_settle_failed_backup`.
            RunFailure::Escaped(error) => return Err(error),
        };
        if let Some(backup) = &backup {
            if backup::restore_backup(project_root, backup) {
                // Source `_settle_failed_backup`: a fully restored rollback also
                // removes the backup, and reports a cleanup failure with its own
                // warning (it never claims the restore failed).
                if !backup::cleanup_backup(project_root) {
                    warnings.push(format!(
                        "Rollback succeeded but the backup at {} could not be removed; delete it by hand.",
                        backup.dir.display()
                    ));
                }
            } else {
                warnings.push(format!(
                    "Rollback was incomplete: the backup at {} holds the pre-migration state; restore from it by hand and then delete it.",
                    backup.dir.display()
                ));
            }
        }
        // Source returns `MigrationResult(success=False)`: the report survives.
        return Ok(MigrationOutcome {
            changed: false,
            success: false,
            changes: vec![format!("Migration failed at step: {}", order.step)],
            errors: vec![order.source_error()],
            warnings,
            ..MigrationOutcome::default()
        });
    }

    if let Some(backup) = &backup
        && !backup::cleanup_backup(project_root)
    {
        let _ = backup;
        warnings.push(format!(
            "Could not remove the migration backup at {}; delete it by hand.",
            project_root
                .join(".kittify")
                .join(backup::BACKUP_DIR_NAME)
                .display()
        ));
    }
    // Source re-enumerates the corpus for the summary AFTER the backup cleanup; an
    // enumeration error here escapes to the `apply` wrapper (there is nothing to
    // roll back, and the backup is already gone).
    let features = MissionCorpus::new(project_root)
        .discover_features()
        .map_err(|error| io_error("read directory", &project_root.join(KITTY_SPECS_DIR), error))?
        .len();
    changes.push(format!(
        "Migrated {} feature(s), {} WPs backfilled, {} events generated",
        features, stats.wps, stats.events
    ));
    if stats.derived > 0 {
        changes.push(format!("Moved {} derived file(s)", stats.derived));
    }
    Ok(MigrationOutcome {
        changed: true,
        success: true,
        changes,
        warnings,
        ..MigrationOutcome::default()
    })
}

/// The nine ordered steps (Source `run_migration` body after the backup).
///
/// Every walk uses the ONE `3.0.0` population rule — a `kitty-specs/` child with
/// `meta.json` **on disk** (Source `_discover_features`). A tracked-but-absent
/// `meta.json` is therefore not migrated and is never restored here: the
/// tracked-copy restore seam belongs to `3.2.0a4`/`backfill-identity`, not to
/// this runner (Source `backfill_mission_ids` skips a directory with no
/// `meta.json`; the runner never calls `backfill_mission`).
fn run_steps(
    project_root: &Path,
    warnings: &mut Vec<String>,
    stats: &mut RunStats,
) -> Result<(), RunFailure> {
    let corpus = MissionCorpus::new(project_root);
    // Source's `for feature_dir in _discover_features(repo_root):` walks sit OUTSIDE
    // any `try`, so their enumeration error escapes `run_migration` and reaches the
    // `apply` wrapper — it is NOT a step failure.
    let discover = || -> Result<Vec<PathBuf>, InstallError> {
        corpus
            .discover_features()
            .map_err(|error| io_error("read directory", &project_root.join(KITTY_SPECS_DIR), error))
    };

    // Step 2: identity backfill (project uuid, mission ids, WP ids).
    backfill_project_uuid(project_root)?;
    let mut wp_id_maps: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, String>,
    > = std::collections::BTreeMap::new();
    // `backfill_mission_ids` owns its own `try` in Source, so an enumeration error
    // here is this step's RETURNED failure, with the mission-ID wording.
    let identity_missions = corpus.discover_features().map_err(|error| {
        StepFailure::new(
            "identity_backfill",
            format!("mission ID backfill failed: {error}"),
        )
    })?;
    for mission in identity_missions {
        let slug = mission
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let identity = sk_mission::migration_rescue::backfill_mission_id(&mission, false).map_err(
            |error| {
                StepFailure::new(
                    "identity_backfill",
                    format!("mission ID backfill failed: {error}"),
                )
            },
        )?;
        let Some(mission_id) = identity.mission_id.clone() else {
            warnings.push(format!("WP ID backfill skipped for {slug}: no mission_id"));
            continue;
        };
        match sk_mission::migration_rescue::backfill_wp_identity(&mission, &mission_id, false) {
            Ok(mapping) => {
                stats.wps += mapping.len();
                wp_id_maps.insert(slug, mapping);
            }
            Err(error) => warnings.push(format!("WP ID backfill failed for {slug}: {error}")),
        }
    }

    // Step 3: ownership backfill.
    for mission in discover()? {
        let slug = mission
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lane_files = lane_changed_files(&mission, &slug);
        sk_mission::migration_rescue::backfill_ownership_with_files(
            &mission,
            &slug,
            &lane_files,
            false,
        )
        .map_err(|error| {
            StepFailure::new(
                "ownership_backfill",
                format!("ownership backfill failed for {slug}: {error}"),
            )
        })?;
    }

    // Step 4: event-log rebuild.
    for mission in discover()? {
        let slug = mission
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let empty = std::collections::BTreeMap::new();
        let mapping = wp_id_maps.get(&slug).unwrap_or(&empty);
        let rebuild =
            sk_mission::migration_rescue::rebuild_mission_event_log(&mission, &slug, mapping)
                .map_err(|error| {
                    StepFailure::new(
                        "state_rebuild",
                        format!("event log rebuild failed for {slug}: {error}"),
                    )
                })?;
        stats.events += rebuild.events_generated;
        warnings.extend(rebuild.warnings);
        // Source step 4 folds a REPORTED rebuild error into the warning channel
        // (`State rebuild warning for <slug>: <e>`) instead of failing the step.
        for error in &rebuild.errors {
            warnings.push(format!("State rebuild warning for {slug}: {error}"));
        }
    }

    // Step 5: strip the evicted frontmatter fields.
    for mission in discover()? {
        let slug = mission
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        sk_mission::migration_rescue::strip_mutable_fields(&mission).map_err(|error| {
            StepFailure::new(
                "strip_frontmatter",
                format!("frontmatter strip failed for {slug}: {error}"),
            )
        })?;
    }

    // Step 6: schema version.
    stamp_schema_version(project_root)?;

    // Step 7: .gitignore.
    update_gitignore(project_root)?;

    // Step 8: derived status copy.
    stats.derived = copy_derived_status(project_root)?;

    // Step 9: shims (non-fatal).
    rewrite_shims(project_root, warnings);

    Ok(())
}

// ---------------------------------------------------------------------------
// 3.1.1_charter_rename
// ---------------------------------------------------------------------------

fn agent_command_directories(project_root: &Path) -> Vec<PathBuf> {
    crate::migrations::completion_legacy::agent_commands::command_directories(project_root)
        .into_iter()
        .map(|directory| project_root.join(&directory.directory))
        .collect()
}

fn charter_layout_present(project_root: &Path) -> bool {
    let kittify = project_root.join(".kittify");
    if kittify.join("constitution").exists() {
        return true;
    }
    if kittify.join("memory/constitution.md").exists() {
        return true;
    }
    if let Ok(entries) = std::fs::read_dir(kittify.join("missions")) {
        for entry in entries.flatten() {
            if entry.path().is_dir() && entry.path().join("constitution").exists() {
                return true;
            }
        }
    }
    agent_command_directories(project_root)
        .iter()
        .any(|directory| directory.join(LEGACY_CONSTITUTION_COMMAND).exists())
}

fn detect_charter_rename(project_root: &Path) -> Result<bool, InstallError> {
    Ok(charter_layout_present(project_root))
}

fn can_apply_charter_rename(project_root: &Path) -> Result<Option<String>, InstallError> {
    if charter_layout_present(project_root) {
        Ok(None)
    } else {
        Ok(Some("No constitution-era state found".to_string()))
    }
}

/// Case-preserving `constitution` → `charter` replacement (Source
/// `_CONSTITUTION_RE` + `_replace_constitution`: only the first character's case
/// is inspected).
fn rewrite_constitution_words(text: &str) -> String {
    const FROM: &str = "constitution";
    const TO_LOWER: &str = "charter";
    const TO_UPPER: &str = "Charter";
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;
    let bytes = text.as_bytes();
    while index < bytes.len() {
        let mut characters = text[index..].char_indices();
        let mut end = index;
        let matched = FROM.bytes().all(|expected| {
            let Some((offset, character)) = characters.next() else {
                return false;
            };
            if sk_kernel::python_text::regex_ignore_case_key(u32::from(character))
                != sk_kernel::python_text::regex_ignore_case_key(u32::from(expected))
            {
                return false;
            }
            end = index + offset + character.len_utf8();
            true
        });
        if matched {
            out.push_str(if bytes[index] == b'C' {
                TO_UPPER
            } else {
                TO_LOWER
            });
            index = end;
            continue;
        }
        let character = text[index..].chars().next().unwrap_or(' ');
        out.push(character);
        index += character.len_utf8();
    }
    out
}

fn rewrite_file(
    path: &Path,
    project_root: &Path,
    read_only: bool,
    dry_run: bool,
    changes: &mut Vec<String>,
    errors: &mut Vec<String>,
) -> Result<(), InstallError> {
    let relative = crate::preserve::relative_display(path, project_root);
    // Reads catch OSError/UnicodeDecodeError; writes catch ONLY OSError.
    // ValueError must escape before any later artifact or metadata phase.
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) => {
            errors.push(format!("Failed to read {relative}: {error}"));
            return Ok(());
        }
    };
    let text = match sk_kernel::python_utf8::decode_utf8(&raw) {
        Ok(text) => sk_kernel::python_text::universal_newlines(text),
        Err(error) => {
            errors.push(format!("Failed to read {relative}: {error}"));
            return Ok(());
        }
    };
    let updated = rewrite_constitution_words(&text);
    if updated == text {
        return Ok(());
    }
    if dry_run {
        changes.push(format!("Would rewrite content in {relative}"));
        return Ok(());
    }
    if let Err(error) = write_generated(path, &updated, read_only) {
        if error.is_os_error() {
            errors.push(format!("Failed to write {relative}: {error}"));
            return Ok(());
        }
        return Err(InstallError::SourceWrite(Box::new(error)));
    }
    changes.push(format!("Rewrote content in {relative}"));
    Ok(())
}

/// The `3.1.1` engine: Source `CharterRenameMigration.apply(project_path, dry_run)`.
fn execute_charter_rename(
    project_root: &Path,
    mode: MigrationMode,
) -> Result<MigrationOutcome, InstallError> {
    let dry_run = mode.is_preview();
    let mut changes: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let kittify = project_root.join(".kittify");
    let charter_dir = kittify.join("charter");

    // Phase 1: layout normalization.
    normalize_charter_layouts(
        project_root,
        &kittify,
        &charter_dir,
        dry_run,
        &mut changes,
        &mut errors,
        &mut warnings,
    )?;

    // Phase 2: content rewriting (charter files writable, deployed commands
    // read-only).
    if charter_dir.exists() {
        for path in sorted_files(&charter_dir)? {
            let suffix = format!(
                ".{}",
                path.extension()
                    .map(|extension| extension.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            if !TEXT_SUFFIXES.contains(&suffix.as_str()) {
                continue;
            }
            rewrite_file(
                &path,
                project_root,
                false,
                dry_run,
                &mut changes,
                &mut errors,
            )?;
        }
    }
    for directory in agent_command_directories(project_root) {
        if !directory.is_dir() {
            continue;
        }
        for path in sorted_glob(&directory, "spec-kitty.", ".md")? {
            rewrite_file(
                &path,
                project_root,
                true,
                dry_run,
                &mut changes,
                &mut errors,
            )?;
        }
    }

    // Phase 3: agent artifact rename.
    for directory in agent_command_directories(project_root) {
        let old = directory.join(LEGACY_CONSTITUTION_COMMAND);
        let new = directory.join(CHARTER_COMMAND);
        if !old.exists() || new.exists() {
            continue;
        }
        let relative = directory
            .strip_prefix(project_root)
            .unwrap_or(&directory)
            .to_string_lossy()
            .replace('\\', "/");
        if dry_run {
            changes.push(format!(
                "Would rename {relative}/{LEGACY_CONSTITUTION_COMMAND}"
            ));
            continue;
        }
        if let Err(error) = std::fs::rename(&old, &new) {
            errors.push(format!(
                "Failed to rename {relative}/{LEGACY_CONSTITUTION_COMMAND}: {error}"
            ));
            continue;
        }
        changes.push(format!(
            "Renamed {relative}/{LEGACY_CONSTITUTION_COMMAND} -> {CHARTER_COMMAND}"
        ));
        rewrite_file(&new, project_root, true, dry_run, &mut changes, &mut errors)?;
    }

    // Phase 4: metadata normalization is owned by the ledger. Source reads BOTH
    // ledger partitions (`read_all`), not only the committed one.
    let ledger = crate::ledger::MigrationLedger::new(project_root);
    if ledger
        .records(true)
        .map(|records| !records.is_empty())
        .unwrap_or(false)
        || ledger
            .records(false)
            .map(|records| !records.is_empty())
            .unwrap_or(false)
    {
        changes.push("Migration-id normalization is owned by the migration ledger".to_string());
    }

    if !errors.is_empty() {
        // Source returns `MigrationResult(success=False, changes, errors, warnings)`.
        return Ok(MigrationOutcome {
            changed: !changes.is_empty(),
            success: false,
            changes,
            errors,
            warnings,
            ..MigrationOutcome::default()
        });
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        success: true,
        changes,
        warnings,
        ..MigrationOutcome::default()
    })
}

fn sorted_files(directory: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let mut files: Vec<PathBuf> = Vec::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(io_error("read directory", directory, error)),
    };
    for entry in entries {
        let entry = entry.map_err(|error| io_error("read directory", directory, error))?;
        let path = entry.path();
        if path.is_dir() {
            // pathlib.rglob does not recurse into child directory symlinks.
            // A root explicitly passed as a symlink remains its own capability.
            if !path.is_symlink() {
                files.extend(sorted_files(&path)?);
            }
        } else if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn sorted_glob(directory: &Path, prefix: &str, suffix: &str) -> Result<Vec<PathBuf>, InstallError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("read directory", directory, error)),
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name.starts_with(prefix) && name.ends_with(suffix)
                })
        })
        .collect();
    files.sort();
    Ok(files)
}

/// What happened to a governance payload the rename had to retire.
enum GovernanceRemoval {
    /// Removed outright (proven owned) or archived then removed. `diagnostic` is
    /// the shared guard's operator-facing line for whichever verdict it reached;
    /// Source surfaces that line for a collision and for an archive.
    Removed { archived: bool, diagnostic: String },
    /// Unprovable and kept in place; the diagnostic says why.
    Preserved(String),
    /// Already gone.
    Absent,
}

/// Retire a governance payload through the ONE shared archive/ownership authority.
///
/// Governance payloads (constitution/charter files and directories) carry no
/// package version marker and no shipped canonical, so the composite prover used
/// here can never prove them owned: every case is archive-then-remove through
/// `preserve::guard_destructive_removal` with `backup_parent=.kittify`, exactly
/// like Source's `AnyProver([CanonicalContentProver(), ManagedPathProver()])`.
/// There is no private archive branch here — the shared guard owns the policy.
fn drop_governance_payload(
    project_root: &Path,
    kittify: &Path,
    path: &Path,
) -> Result<GovernanceRemoval, InstallError> {
    let prover = crate::preserve::Prover::Any(vec![
        crate::preserve::Prover::CanonicalContent,
        crate::preserve::Prover::ManagedPath(Vec::new()),
    ]);
    let verdict = crate::preserve::guard_destructive_removal(
        path,
        project_root,
        &prover,
        path.is_dir(),
        Some(kittify),
        false,
    )?;
    // The shared guard owns the operator-facing line for every verdict, including
    // a proven-owned removal (`Removed package-owned <rel> (proof: …)`), which the
    // verdict accessor does not expose.
    let relative = crate::preserve::relative_display(path, project_root);
    Ok(match verdict {
        crate::preserve::RemovalVerdict::Owned { proof } => GovernanceRemoval::Removed {
            archived: false,
            diagnostic: format!("Removed package-owned {relative} (proof: {})", proof.kind),
        },
        crate::preserve::RemovalVerdict::Archived { diagnostic, .. } => {
            GovernanceRemoval::Removed {
                archived: true,
                diagnostic,
            }
        }
        crate::preserve::RemovalVerdict::Preserved { diagnostic, .. } => {
            GovernanceRemoval::Preserved(diagnostic)
        }
        crate::preserve::RemovalVerdict::Absent => GovernanceRemoval::Absent,
    })
}

#[allow(clippy::too_many_arguments)]
fn normalize_charter_layouts(
    project_root: &Path,
    kittify: &Path,
    charter_dir: &Path,
    dry_run: bool,
    changes: &mut Vec<String>,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(), InstallError> {
    // Layout C: mission-specific constitution directories.
    if let Ok(entries) = std::fs::read_dir(kittify.join("missions")) {
        let mut missions: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        missions.sort();
        for mission_dir in missions {
            if !mission_dir.is_dir() {
                continue;
            }
            let constitution = mission_dir.join("constitution");
            if !constitution.exists() {
                continue;
            }
            let name = mission_dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if dry_run {
                changes.push(format!("Would remove {name}/constitution/"));
                continue;
            }
            match drop_governance_payload(project_root, kittify, &constitution) {
                Ok(GovernanceRemoval::Absent) => {}
                Ok(GovernanceRemoval::Removed {
                    archived,
                    diagnostic,
                }) => {
                    // Source: a proven-owned removal is a change; an
                    // archive-then-remove is a WARNING ONLY (the bytes were
                    // preserved, so upgrade output must not claim a deletion).
                    if archived {
                        warnings.push(diagnostic);
                    } else {
                        changes.push(format!("Removed mission-specific: {name}/constitution/"));
                    }
                }
                Ok(GovernanceRemoval::Preserved(diagnostic)) => warnings.push(diagnostic),
                Err(error) => {
                    errors.push(format!("Failed to remove {name}/constitution/: {error}"))
                }
            }
        }
    }

    // Layout B: memory/constitution.md.
    let memory_constitution = kittify.join("memory/constitution.md");
    if memory_constitution.exists() {
        if charter_dir.join("charter.md").exists() {
            if dry_run {
                changes.push("Would remove stale .kittify/memory/constitution.md".to_string());
            } else {
                match drop_governance_payload(project_root, kittify, &memory_constitution) {
                    Ok(GovernanceRemoval::Absent) => {}
                    Ok(GovernanceRemoval::Removed {
                        archived,
                        diagnostic,
                    }) => {
                        if archived {
                            warnings.push(diagnostic);
                        } else {
                            changes
                                .push("Removed stale .kittify/memory/constitution.md".to_string());
                        }
                    }
                    Ok(GovernanceRemoval::Preserved(diagnostic)) => warnings.push(diagnostic),
                    Err(error) => errors.push(format!(
                        "Failed to remove stale memory/constitution.md: {error}"
                    )),
                }
            }
        } else if dry_run {
            changes.push(
                "Would move .kittify/memory/constitution.md -> .kittify/charter/charter.md"
                    .to_string(),
            );
        } else {
            std::fs::create_dir_all(charter_dir)
                .map_err(|error| io_error("create directory", charter_dir, error))?;
            match std::fs::rename(&memory_constitution, charter_dir.join("charter.md")) {
                Ok(()) => changes.push(
                    "Moved .kittify/memory/constitution.md -> .kittify/charter/charter.md"
                        .to_string(),
                ),
                Err(error) => {
                    errors.push(format!("Failed to move memory/constitution.md: {error}"))
                }
            }
        }
    }

    // Layout A: constitution/ -> charter/.
    let constitution_dir = kittify.join("constitution");
    if constitution_dir.exists() {
        if charter_dir.exists() {
            if dry_run {
                changes
                    .push("Would merge .kittify/constitution/ into .kittify/charter/".to_string());
            } else {
                // Source wraps the merge loop AND the residual `rmdir` in one
                // `try`: the first failure is reported as this one message and the
                // residual directory is left in place (never a silent success).
                let entries = sorted_entries(&constitution_dir)?;
                let merged = (|| -> std::io::Result<()> {
                    for entry in entries {
                        let name = entry
                            .file_name()
                            .map(|name| name.to_os_string())
                            .unwrap_or_default();
                        let destination = charter_dir.join(&name);
                        if !destination.exists() {
                            std::fs::rename(&entry, &destination)?;
                            changes.push(format!(
                                "Merged {} from constitution/ to charter/",
                                name.to_string_lossy()
                            ));
                            continue;
                        }
                        // A same-named file already in `charter/`: the shared
                        // guard archives it outside the doomed directory (or
                        // removes it when owned) and its diagnostic is a warning.
                        match drop_governance_payload(project_root, kittify, &entry) {
                            Ok(GovernanceRemoval::Absent) => {}
                            Ok(GovernanceRemoval::Removed { diagnostic, .. })
                            | Ok(GovernanceRemoval::Preserved(diagnostic)) => {
                                warnings.push(diagnostic);
                            }
                            Err(error) => {
                                return Err(std::io::Error::other(error.to_string()));
                            }
                        }
                    }
                    // Colliding items were archived-out (or removed when owned) and
                    // merged items moved, so the residual dir is now empty; remove it
                    // with an empty-only rmdir — never a raw rmtree.
                    std::fs::remove_dir(&constitution_dir)
                })();
                match merged {
                    Ok(()) => changes
                        .push("Removed residual .kittify/constitution/ after merge".to_string()),
                    Err(error) => errors.push(format!(
                        "Failed to merge constitution/ into charter/: {error}"
                    )),
                }
            }
        } else if dry_run {
            changes.push("Would rename .kittify/constitution/ -> .kittify/charter/".to_string());
        } else {
            match std::fs::rename(&constitution_dir, charter_dir) {
                Ok(()) => {
                    changes.push("Renamed .kittify/constitution/ -> .kittify/charter/".to_string())
                }
                Err(error) => errors.push(format!(
                    "Failed to rename constitution/ -> charter/: {error}"
                )),
            }
        }
    }

    // constitution.md -> charter.md inside the charter directory.
    if charter_dir.exists() {
        let old_md = charter_dir.join("constitution.md");
        let new_md = charter_dir.join("charter.md");
        if old_md.exists() && !new_md.exists() {
            if dry_run {
                changes
                    .push("Would rename charter/constitution.md -> charter/charter.md".to_string());
            } else {
                match std::fs::rename(&old_md, &new_md) {
                    Ok(()) => changes
                        .push("Renamed charter/constitution.md -> charter/charter.md".to_string()),
                    Err(error) => errors.push(format!(
                        "Failed to rename constitution.md -> charter.md: {error}"
                    )),
                }
            }
        }
    }
    Ok(())
}

fn sorted_entries(directory: &Path) -> Result<Vec<PathBuf>, InstallError> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| io_error("read directory", directory, error))?;
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    Ok(paths)
}

// ---------------------------------------------------------------------------
// 3.2.0a4_normalize_mission_lifecycle
// ---------------------------------------------------------------------------

fn detect_normalize_mission_lifecycle(project_root: &Path) -> Result<bool, InstallError> {
    // Source `_discover_candidates` raises when `kitty-specs/` cannot be listed;
    // `detect()` must propagate that rather than answering "nothing to do".
    MissionCorpus::new(project_root)
        .children()
        .map_err(|error| io_error("read directory", &project_root.join(KITTY_SPECS_DIR), error))?;
    Ok(
        sk_mission::migration_rescue::normalize_mission_lifecycle(project_root, true)
            .iter()
            .any(|outcome| outcome.status != "skipped"),
    )
}

fn can_apply_normalize_mission_lifecycle(
    project_root: &Path,
) -> Result<Option<String>, InstallError> {
    if project_root.join(KITTY_SPECS_DIR).exists() || project_root.join(".kittify").exists() {
        Ok(None)
    } else {
        Ok(Some("No kitty-specs/ directory found".to_string()))
    }
}

/// The `3.2.0a4` engine: Source `NormalizeMissionLifecycleMigration.apply`.
fn execute_normalize_mission_lifecycle(
    project_root: &Path,
    mode: MigrationMode,
) -> Result<MigrationOutcome, InstallError> {
    let dry_run = mode.is_preview();
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    for outcome in sk_mission::migration_rescue::normalize_mission_lifecycle(project_root, dry_run)
    {
        if outcome.status == "normalized" {
            for action in &outcome.actions {
                changes.push(format!("{}: {action}", outcome.slug));
            }
        }
        for warning in &outcome.warnings {
            warnings.push(format!("{}: {warning}", outcome.slug));
        }
        if outcome.status == "error" {
            errors.push(format!(
                "{}: {}",
                outcome.slug,
                outcome.error.as_deref().unwrap_or("unknown error")
            ));
        }
    }
    if !errors.is_empty() {
        return Ok(MigrationOutcome {
            changed: !changes.is_empty(),
            success: false,
            changes,
            errors,
            warnings,
            ..MigrationOutcome::default()
        });
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        success: true,
        changes,
        warnings,
        ..MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// runtime_state_backfill
// ---------------------------------------------------------------------------

/// The remediation hint appended to every fail-closed abort message.
const REMEDIATION: &str =
    "run `spec-kitty migrate backfill-runtime-state --mission {slug} --dry-run` to inspect";

/// Result of the runtime-state cutover step.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CutoverReport {
    /// Per-mission outcomes visited (stops at the first failure).
    pub outcomes: Vec<sk_mission::migration_rescue::CutoverOutcome>,
    /// The operator-actionable abort message, when the step aborted.
    pub abort: Option<String>,
    /// Every file the aborted walk already persisted (Source `partial_writes`).
    pub partial_writes: Vec<MigrationPartialWrite>,
}

impl CutoverReport {
    /// Total NEW seed events appended.
    pub fn seeded_count(&self) -> usize {
        self.outcomes
            .iter()
            .map(|outcome| outcome.seeded_count)
            .sum()
    }

    /// Missions flipped this run.
    pub fn flipped(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.flipped)
            .count()
    }

    /// Residue children reported and left untouched.
    pub fn residue(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.residue)
            .count()
    }
}

/// Resolve the canonical placement port's PRIMARY answer for one mission.
///
/// Source `_resolve_primary_home_or_degrade` catches exactly four resolver
/// failures on the UNOWNED branch (`WorkspaceRootNotFound`,
/// `MissionSelectorAmbiguous`, `StatusReadPathNotFound`, `ActionContextError`)
/// and treats them as a degrade; every other failure — and every failure on the
/// OWNED branch — propagates. This maps the native typed errors onto those four
/// and keeps everything else fatal.
///
/// The OWNED leg is [`owned_placement`]: Source lets every owned-leg failure out
/// of the resolver and never degrades there.
fn placement_port(feature_dir: &Path) -> Result<PlacementPort, String> {
    use sk_workspace::{MissionArtifactKind, ProjectWorkspace, WorkspaceError};

    let slug = feature_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let root = match canonical_root(feature_dir) {
        Some(root) => root,
        // `WorkspaceRootNotFound`: no `.git` entry anywhere above.
        None => {
            return Ok(PlacementPort::Degraded(
                "workspace root not found".to_string(),
            ));
        }
    };
    let specs_dir = match ProjectWorkspace::open(&root) {
        Ok(workspace) => {
            // `MissionSelectorAmbiguous` / `StatusReadPathNotFound` are the two
            // handle-level Source degrade classes.
            match workspace.mission(&slug) {
                Ok(_) => {}
                Err(WorkspaceError::Mission(sk_mission::MissionError::AmbiguousMission {
                    ..
                })) => {
                    return Ok(PlacementPort::Degraded(
                        "mission selector ambiguous".to_string(),
                    ));
                }
                Err(WorkspaceError::Mission(sk_mission::MissionError::MissionNotFound {
                    ..
                })) => {
                    return Ok(PlacementPort::Degraded(
                        "status read path not found".to_string(),
                    ));
                }
                Err(_) => {}
            }
            workspace.specs_dir()
        }
        Err(WorkspaceError::Git(sk_git::GitError::NotARepository { .. })) => {
            root.join(KITTY_SPECS_DIR)
        }
        Err(error) => return Err(error.to_string()),
    };
    if !MissionArtifactKind::PrimaryMetadata.is_primary_partition() {
        return Err("PRIMARY_METADATA is not a primary-partition kind".to_string());
    }
    Ok(PlacementPort::Resolved(specs_dir.join(slug)))
}

/// The OWNED leg of Source `_resolve_primary_home_or_degrade`.
///
/// The routing question is answered by the canonical artifact-home authority over
/// the ownership fact's STORED topology: a PRIMARY-partition kind
/// (`PRIMARY_METADATA`) resolves PRIMARY for every topology and then answers the
/// owned mission directory (Source `_owned_read_dir_for_kind` — a PRIMARY kind
/// never probes the coordination surface). Unlike the unowned leg this never
/// degrades: Source lets every owned-leg resolver failure out, and so does this.
fn owned_placement(owned: &sk_workspace::OwnedCheckout) -> Result<PlacementPort, String> {
    let surface = sk_workspace::declared_read_surface(
        sk_workspace::MissionArtifactKind::PrimaryMetadata,
        owned.topology(),
    );
    if surface != sk_workspace::TopologySurface::Primary {
        return Err(format!(
            "owned PRIMARY_METADATA resolved the {surface:?} surface, not PRIMARY"
        ));
    }
    Ok(PlacementPort::Resolved(owned.mission_dir().to_path_buf()))
}

/// Source `resolve_canonical_root`: the nearest ancestor carrying a `.git` entry.
fn canonical_root(feature_dir: &Path) -> Option<std::path::PathBuf> {
    feature_dir
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
}

fn detect_runtime_state_backfill(project_root: &Path) -> Result<bool, InstallError> {
    let corpus = MissionCorpus::new(project_root);
    for entry in corpus
        .cutover_candidates()
        .map_err(|error| io_error("read directory", &project_root.join(KITTY_SPECS_DIR), error))?
    {
        if !entry.is_mission() {
            // Source's `detect()` runs `_mission_needs_cutover` on every
            // directory; a residue folder answers "nothing to cut over".
            continue;
        }
        // Source lets `LegacyRuntimeReadError` escape `detect()`: an unreadable
        // WP artifact is a loud error, never "the corpus is finished".
        if sk_status::migration_backfill::runtime_state_needs_cutover(&entry.path).map_err(
            |error| InstallError::UpgradeRefused {
                detail: format!("cannot read {} for legacy runtime: {error}", entry.slug),
            },
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn can_apply_runtime_state_backfill(project_root: &Path) -> Result<Option<String>, InstallError> {
    if detect_runtime_state_backfill(project_root)? {
        Ok(None)
    } else {
        Ok(Some(
            "no un-migrated mission runtime state found under kitty-specs/".to_string(),
        ))
    }
}

/// One mission the runtime-cutover walk must visit.
#[derive(Clone, Debug)]
struct CutoverCandidate {
    path: PathBuf,
    slug: String,
    /// The write-seam residue answer the walk already holds for this child.
    residue: bool,
}

/// Seed → fail-closed verify → `status_phase` flip over the whole corpus.
///
/// Aborts the step on the first mission whose verify fails, with the specific
/// mismatch and an operator-actionable message. Missions that already passed
/// stay flipped: each was independently verified before it flipped, so nothing
/// is ever half-flipped (per-mission atomicity, not a corpus rollback).
///
/// A residue child is reported (`residue`) and never seeded or given a
/// `meta.json` stub.
pub fn cutover_corpus(project_root: &Path, dry_run: bool) -> Result<CutoverReport, InstallError> {
    cutover_walk(project_root, dry_run, None)
}

/// The OWNED runtime-cutover leg: Source's `cutover_mission` for the one Mission
/// an ownership fact names, with owned routing instead of the corpus walk.
///
/// The facts come from `sk_workspace::OwnedCheckout`'s own accessors — the mission
/// directory, the repository root (the lock root) and the stored topology — and
/// travel into the Mission domain through the neutral port, so no dependency
/// cycle is created and no second ownership model exists. Source has no owned
/// corpus walk (`cutover_repo` takes no owned context), which is why this is a
/// single-mission entry rather than a second walk.
pub fn cutover_owned_checkout(
    project_root: &Path,
    owned: &sk_workspace::OwnedCheckout,
    dry_run: bool,
) -> Result<CutoverReport, InstallError> {
    cutover_walk(project_root, dry_run, Some(owned))
}

/// Source `cutover_repo`: the operator CLI's per-mission best-effort corpus walk.
///
/// The upgrade migration uses [`cutover_corpus`] (abort on the first failure —
/// Source's `_cutover_corpus`); the `migrate backfill-runtime-state` command
/// uses Source's `cutover_repo`, which visits every mission and folds each
/// failure into that mission's own outcome instead of stranding the missions
/// sorted after it. Both walkers run the SAME
/// [`sk_mission::migration_rescue::cutover_mission`] engine through the same
/// placement/residue authorities — this is a walk-policy composition, not a
/// second cutover.
///
/// Source has no owned corpus walk, so the owned leg stays
/// [`cutover_owned_checkout`] (one Mission).
pub fn cutover_repo_best_effort(
    project_root: &Path,
    dry_run: bool,
) -> Vec<sk_mission::migration_rescue::CutoverOutcome> {
    sk_mission::migration_rescue::cutover_repo(project_root, dry_run, &placement_port)
}

/// Source CLI `--mission` arm: one already-resolved mission directory.
///
/// `resolve_mission_handle` has already selected the mission, so Source calls
/// `cutover_mission(feature_dir, dry_run=...)` directly: no corpus walk and no
/// owned context. The residue answer is the engine's own (Source's
/// single-mission path decides it inside `cutover_mission`), which is why
/// `residue` stays `None` here.
pub fn cutover_selected_mission(
    feature_dir: &Path,
    dry_run: bool,
) -> Result<sk_mission::migration_rescue::CutoverOutcome, InstallError> {
    let placement =
        placement_port(feature_dir).map_err(|detail| InstallError::UpgradeRefused { detail })?;
    Ok(sk_mission::migration_rescue::cutover_mission(
        CutoverRequest {
            feature_dir,
            status_feature_dir: None,
            dry_run,
            placement: &placement,
            residue: None,
            owned: None,
        },
    ))
}

/// The shared walk behind both legs: one candidate list, one outcome loop.
fn cutover_walk(
    project_root: &Path,
    dry_run: bool,
    owned: Option<&sk_workspace::OwnedCheckout>,
) -> Result<CutoverReport, InstallError> {
    let mut report = CutoverReport::default();
    let candidates: Vec<CutoverCandidate> = match owned {
        Some(fact) => vec![CutoverCandidate {
            path: fact.mission_dir().to_path_buf(),
            slug: fact.mission_slug().as_str().to_string(),
            // Source: an explicit owned context names its Mission, so the residue
            // check is skipped for it.
            residue: false,
        }],
        None => MissionCorpus::new(project_root)
            .cutover_candidates()
            .map_err(|error| {
                io_error("read directory", &project_root.join(KITTY_SPECS_DIR), error)
            })?
            .into_iter()
            .map(|entry| CutoverCandidate {
                residue: !entry.is_mission(),
                path: entry.path,
                slug: entry.slug,
            })
            .collect(),
    };
    let owned_context = owned.map(|fact| {
        sk_mission::migration_rescue::OwnedMission::new(
            fact.repository_root(),
            fact.mission_dir(),
            fact.mission_slug().as_str(),
            fact.topology(),
        )
    });
    for candidate in candidates {
        let mission = &candidate.path;
        let placement = match owned {
            Some(fact) => owned_placement(fact),
            None => placement_port(mission),
        };
        let outcome = match placement {
            Ok(port) => sk_mission::migration_rescue::cutover_mission(CutoverRequest {
                feature_dir: mission,
                status_feature_dir: None,
                dry_run,
                placement: &port,
                residue: Some(candidate.residue),
                owned: owned_context,
            }),
            Err(error) => {
                report
                    .outcomes
                    .push(sk_mission::migration_rescue::CutoverOutcome {
                        slug: candidate.slug.clone(),
                        flipped: false,
                        residue: false,
                        already_migrated: false,
                        seeded_count: 0,
                        verify: None,
                        error: Some(error),
                        skipped_notice: None,
                    });
                break;
            }
        };
        let slug = outcome.slug.clone();
        let failed = outcome.error.is_some()
            || (!dry_run && outcome.verify.as_ref().is_some_and(|verify| !verify.ok));
        report.outcomes.push(outcome);
        if !failed {
            continue;
        }
        let last = report.outcomes.last().expect("just pushed");
        let detail = match (&last.verify, &last.error) {
            (Some(verify), _) if !verify.ok => {
                if verify.mismatches.is_empty() {
                    "verify reported a failure with no detail".to_string()
                } else {
                    verify.mismatches.join("; ")
                }
            }
            (_, Some(error)) => error.clone(),
            _ => "unknown cutover failure".to_string(),
        };
        // Source `_abort_message`, unmutated.
        report.abort = Some(format!(
            "Runtime-state cutover aborted: mission '{slug}' failed fail-closed verify ({detail}); {}",
            REMEDIATION.replace("{slug}", &slug)
        ));
        break;
    }
    if report.abort.is_some() {
        let kitty_specs = project_root.join(KITTY_SPECS_DIR);
        for outcome in &report.outcomes {
            if outcome.residue {
                continue;
            }
            let mission_dir = kitty_specs.join(&outcome.slug);
            if outcome.seeded_count > 0 {
                report.partial_writes.push(MigrationPartialWrite {
                    mission: outcome.slug.clone(),
                    path: mission_dir
                        .join("status.events.jsonl")
                        .display()
                        .to_string(),
                });
            }
            if outcome.flipped {
                report.partial_writes.push(MigrationPartialWrite {
                    mission: outcome.slug.clone(),
                    path: mission_dir.join("meta.json").display().to_string(),
                });
            }
        }
    }
    Ok(report)
}

/// The `runtime_state_backfill` engine: Source `RuntimeStateBackfillMigration.apply`.
fn execute_runtime_state_backfill(
    project_root: &Path,
    mode: MigrationMode,
) -> Result<MigrationOutcome, InstallError> {
    let dry_run = mode.is_preview();
    let corpus = MissionCorpus::new(project_root);
    let missions = corpus
        .cutover_candidates()
        .map_err(|error| io_error("read directory", &project_root.join(KITTY_SPECS_DIR), error))?;
    if missions.is_empty() {
        // Source: `MigrationResult(success=True, changes_made=[])`.
        return Ok(MigrationOutcome::unchanged(Vec::new()));
    }
    let report = cutover_corpus(project_root, dry_run)?;
    if let Some(abort) = report.abort.clone() {
        // Source keeps the returned failure AND the real residue that survived.
        return Ok(MigrationOutcome {
            changed: !report.partial_writes.is_empty(),
            success: false,
            changes: Vec::new(),
            errors: vec![abort],
            warnings: Vec::new(),
            partial_writes: report.partial_writes,
            ..MigrationOutcome::default()
        });
    }
    let seeded = report.seeded_count();
    if seeded == 0 {
        // A clean no-op: `success: true`, empty `changes_made` (NOT a synthetic
        // "skipped" outcome with an inferred change).
        return Ok(MigrationOutcome::unchanged(Vec::new()));
    }
    let total = report.outcomes.len();
    let summary = if dry_run {
        format!("dry-run: would seed {seeded} event(s) across {total} mission(s) scanned")
    } else {
        format!(
            "Seeded {seeded} event(s) and flipped {} of {total} mission(s) scanned",
            report.flipped()
        )
    };
    Ok(MigrationOutcome {
        changed: !dry_run,
        success: true,
        changes: vec![summary],
        warnings: Vec::new(),
        ..MigrationOutcome::default()
    })
}

// ---------------------------------------------------------------------------
// The four implementations and their read-only previews
// ---------------------------------------------------------------------------

/// The rescue/runtime-cutover ids implemented by this module, in catalog order.
pub const OWNED_IDS: [&str; 4] = [
    "3.0.0_canonical_context",
    "3.1.1_charter_rename",
    "3.2.0a4_normalize_mission_lifecycle",
    "runtime_state_backfill",
];

/// Source `3.0.0_canonical_context` preview: the real dry-run branch, zero writes.
pub fn preview_canonical_context(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    execute_canonical_context(project_root, MigrationMode::Preview)
}

/// Source `3.1.1_charter_rename` preview: the real dry-run branch, zero writes.
pub fn preview_charter_rename(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    execute_charter_rename(project_root, MigrationMode::Preview)
}

/// Source `3.2.0a4_normalize_mission_lifecycle` preview: the real dry-run branch,
/// zero writes.
pub fn preview_normalize_mission_lifecycle(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    execute_normalize_mission_lifecycle(project_root, MigrationMode::Preview)
}

/// Source `runtime_state_backfill` preview: the real dry-run branch, zero writes.
pub fn preview_runtime_state_backfill(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    execute_runtime_state_backfill(project_root, MigrationMode::Preview)
}

fn apply_canonical_context(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    execute_canonical_context(project_root, MigrationMode::Apply)
}

fn apply_charter_rename(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    execute_charter_rename(project_root, MigrationMode::Apply)
}

fn apply_normalize_mission_lifecycle(
    project_root: &Path,
) -> Result<MigrationOutcome, InstallError> {
    execute_normalize_mission_lifecycle(project_root, MigrationMode::Apply)
}

fn apply_runtime_state_backfill(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    execute_runtime_state_backfill(project_root, MigrationMode::Apply)
}

/// The four typed migration effects this module owns.
pub const IMPLEMENTATIONS: [NativeMigration; 4] = [
    NativeMigration {
        id: "3.0.0_canonical_context",
        detect: detect_canonical_context,
        can_apply: can_apply_canonical_context,
        apply: apply_canonical_context,
    },
    NativeMigration {
        id: "3.1.1_charter_rename",
        detect: detect_charter_rename,
        can_apply: can_apply_charter_rename,
        apply: apply_charter_rename,
    },
    NativeMigration {
        id: "3.2.0a4_normalize_mission_lifecycle",
        detect: detect_normalize_mission_lifecycle,
        can_apply: can_apply_normalize_mission_lifecycle,
        apply: apply_normalize_mission_lifecycle,
    },
    NativeMigration {
        id: "runtime_state_backfill",
        detect: detect_runtime_state_backfill,
        can_apply: can_apply_runtime_state_backfill,
        apply: apply_runtime_state_backfill,
    },
];

#[cfg(test)]
mod tests;
