//! Canonical Source consolidation protocols: merge-driver wiring, the real
//! `git merge --squash` / `git merge --no-ff` / `git rebase` shapes, a
//! hook-and-evidence-checked staged commit, ref compare-and-swap with checkout
//! resync, and the typed diff read the dead-code gate needs.
//!
//! # Why this module exists
//!
//! Source integrates a mission into its target in exactly three strides
//! (`specify_cli/lanes/consolidation.py::_merge_branch_into`):
//!
//! 1. create a **detached worktree** at the target tip;
//! 2. run the real merge strategy in it — `git merge --squash <source>` and an
//!    explicit commit, `git rebase <target>` then a ref advance, or
//!    `git merge --no-ff <source>` — with the seven registered merge drivers
//!    activated for the duration;
//! 3. advance the target ref by compare-and-swap, resyncing a checkout that has
//!    the target checked out.
//!
//! A precomputed integration tree materialised into the operator checkout, then
//! committed against a pinned parent, is *not* that protocol: it never runs
//! git's three-way merge, never lets a user hook or driver change the result,
//! and its post-publication tree check cannot prove which source produced the
//! landing. This module supplies the real strides so the consolidation engine
//! can stop simulating them.
//!
//! # What is pinned
//!
//! * The target ref tip observed before the worktree is created.
//! * The worktree's own `HEAD` before a staged commit, and the index tree it
//!   commits (`git write-tree`).
//! * The staged commit is identified only from evidence bound to the
//!   invocation — git's own summary or a reflog entry appended after a
//!   publication watermark — plus the pinned parent and tree. A post-state that
//!   does not match is [`GitError::MergeConclusionUnconfirmed`], never a
//!   fabricated success, and nothing is rolled back.
//! * The target advance is `update-ref --stdin` compare-and-swap; a moved ref is
//!   [`GitError::PlanStale`], never an overwrite.
//!
//! # Protected branches
//!
//! Consolidation landing on a protected target branch is the *documented*,
//! intended Source behaviour (`spec-kitty consolidate` lands on `main`). This
//! module therefore does **not** apply [`crate::ProtectedBranches`] to the
//! mission→target stride; a blanket protected-`main` refusal would make the
//! normal Source flow impossible. Branch *deletion* protection is a separate
//! authority and is not bypassed here.
//!
//! # Deliberate limits
//!
//! * The driver *definitions* are inert without an attribute mapping. This
//!   module installs both halves exactly as Source splits them
//!   (`_ensure_merge_driver_git_config` + `_ephemeral_merge_driver_activation`)
//!   and tears down only the attribute lines it added.
//! * A `rebase` that stops on a conflict is aborted in the scratch worktree and
//!   reported with the conflict paths; the target ref is never touched.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{GitError, MergeConclusionUnconfirmed};
use crate::merge::OperationMarkerState;
use crate::objects::{DEFAULT_MAX_BLOB_BYTES, MAX_BLOB_BYTES};
use crate::oid::Oid;
use crate::path::{GitPath, strip_line_feed, utf8_field};
use crate::refname::{BranchName, RefName, StartPoint};
use crate::refs::RefChange;
use crate::repository::GitRepository;
use crate::status::{StatusOptions, UntrackedMode};

/// The merge-driver configuration namespace this module installs and restores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeDriverConfig {
    /// The `merge.<key>` config suffix (for example `spec-kitty-meta`).
    pub config_key: &'static str,
    /// The human name (`merge.<key>.name`).
    pub name: &'static str,
    /// The command template (`merge.<key>.driver`).
    pub command: &'static str,
    /// The `gitattributes` patterns routed to this driver.
    pub patterns: &'static [&'static str],
    /// Earlier release names that still count as "shipped" during install.
    pub superseded_names: &'static [&'static str],
}

impl MergeDriverConfig {
    /// The CLI subcommand name (`merge-driver-meta`).
    pub fn driver_name(&self) -> Option<&'static str> {
        self.command.split_whitespace().nth(1)
    }

    /// Every `pattern merge=<key>` attributes line for this driver.
    pub fn attributes_lines(&self) -> impl Iterator<Item = String> + '_ {
        self.patterns
            .iter()
            .map(move |pattern| format!("{pattern} merge={}", self.config_key))
    }
}

/// The seven Source merge drivers, in Source registry order.
///
/// This is the single registry: the Git-config definitions, the
/// `gitattributes` routes and the driver CLI names all come from here. The
/// driver *bodies* live in `sk-workspace::merge_drivers`, which consumes this
/// table instead of keeping a second copy.
pub const MERGE_DRIVER_CONFIGS: [MergeDriverConfig; 7] = [
    MergeDriverConfig {
        config_key: "spec-kitty-event-log",
        name: "Spec Kitty event log union merge",
        command: "spec-kitty merge-driver-event-log %O %A %B",
        patterns: &[
            "kitty-specs/**/status.events.jsonl",
            "kitty-specs/**/decisions.events.jsonl",
        ],
        superseded_names: &[],
    },
    MergeDriverConfig {
        config_key: "spec-kitty-meta",
        name: "Spec Kitty mission meta field merge",
        command: "spec-kitty merge-driver-meta %O %A %B",
        patterns: &["kitty-specs/**/meta.json"],
        superseded_names: &[],
    },
    MergeDriverConfig {
        config_key: "spec-kitty-traces",
        name: "Spec Kitty mission traces union merge",
        command: "spec-kitty merge-driver-traces %O %A %B",
        patterns: &["kitty-specs/**/traces/*.md"],
        superseded_names: &[],
    },
    MergeDriverConfig {
        config_key: "spec-kitty-acceptance-matrix",
        name: "Spec Kitty acceptance matrix filled-side merge",
        command: "spec-kitty merge-driver-acceptance-matrix %O %A %B",
        patterns: &["kitty-specs/**/acceptance-matrix.json"],
        superseded_names: &[],
    },
    MergeDriverConfig {
        config_key: "spec-kitty-issue-matrix",
        name: "Spec Kitty issue matrix row-aware merge",
        command: "spec-kitty merge-driver-issue-matrix %O %A %B",
        patterns: &["kitty-specs/**/issue-matrix.json"],
        superseded_names: &["Spec Kitty issue matrix filled-side merge"],
    },
    MergeDriverConfig {
        config_key: "spec-kitty-review-cycle",
        name: "Spec Kitty review-cycle verdict collision refusal",
        command: "spec-kitty merge-driver-review-cycle %O %A %B",
        patterns: &["kitty-specs/**/tasks/*/review-cycle-*.md"],
        superseded_names: &[],
    },
    MergeDriverConfig {
        config_key: "spec-kitty-decision-index",
        name: "Spec Kitty decision-index entry union merge",
        command: "spec-kitty merge-driver-decision-index %O %A %B",
        patterns: &["kitty-specs/**/decisions/index.json"],
        superseded_names: &[],
    },
];

/// Every `pattern merge=<key>` line, in registry order, deduplicated.
pub fn merge_driver_attribute_lines() -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut lines = Vec::new();
    for config in MERGE_DRIVER_CONFIGS {
        for line in config.attributes_lines() {
            if seen.insert(line.clone()) {
                lines.push(line);
            }
        }
    }
    lines
}

/// Resolve a driver by its CLI name (`merge-driver-meta`, ...).
pub fn merge_driver_config(name: &str) -> Option<&'static MergeDriverConfig> {
    MERGE_DRIVER_CONFIGS
        .iter()
        .find(|config| config.driver_name() == Some(name))
}

/// How `install_merge_driver_config` ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverInstallOutcome {
    /// At least one shipped value was written.
    Installed,
    /// Every shipped value was already present.
    Present,
    /// At least one key holds a value no release shipped and was kept.
    Customized,
}

impl DriverInstallOutcome {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Present => "present",
            Self::Customized => "customized",
        }
    }
}

/// A value this release ships for one `merge.<key>.<field>` config key.
struct DriverConfigEntry {
    key: String,
    shipped: String,
    accepted: Vec<String>,
}

fn driver_config_entries() -> Vec<DriverConfigEntry> {
    let mut entries = Vec::new();
    for config in MERGE_DRIVER_CONFIGS {
        let name_key = format!("merge.{}.name", config.config_key);
        let driver_key = format!("merge.{}.driver", config.config_key);
        let mut accepted_names = vec![config.name.to_owned()];
        accepted_names.extend(
            config
                .superseded_names
                .iter()
                .map(|name| (*name).to_owned()),
        );
        entries.push(DriverConfigEntry {
            key: name_key,
            shipped: config.name.to_owned(),
            accepted: accepted_names,
        });
        entries.push(DriverConfigEntry {
            key: driver_key,
            shipped: config.command.to_owned(),
            accepted: vec![config.command.to_owned()],
        });
    }
    entries
}

/// An activated driver set. Dropping it is a best-effort teardown; call
/// [`MergeDriverActivation::finish`] to observe a teardown failure.
pub struct MergeDriverActivation {
    repository: GitRepository,
    added_attribute_lines: Vec<String>,
    config_snapshot: Option<BTreeMap<String, Option<String>>>,
}

impl std::fmt::Debug for MergeDriverActivation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MergeDriverActivation")
            .field("added_attribute_lines", &self.added_attribute_lines)
            .field("restores_config", &self.config_snapshot.is_some())
            .finish()
    }
}

impl MergeDriverActivation {
    /// The attribute lines this activation appended (and will remove).
    pub fn added_attribute_lines(&self) -> &[String] {
        &self.added_attribute_lines
    }

    /// Tear the activation down now, reporting any cleanup failure.
    pub fn finish(mut self) -> Result<(), GitError> {
        let added = std::mem::take(&mut self.added_attribute_lines);
        let snapshot = self.config_snapshot.take();
        self.repository.remove_info_attributes(&added)?;
        if let Some(snapshot) = snapshot {
            self.repository.restore_merge_driver_config(&snapshot)?;
        }
        Ok(())
    }
}

impl Drop for MergeDriverActivation {
    fn drop(&mut self) {
        // Best effort: a failure here cannot change the merge result, and the
        // caller may already be unwinding. `finish()` is the observable path.
        let added = std::mem::take(&mut self.added_attribute_lines);
        let _ = self.repository.remove_info_attributes(&added);
        if let Some(snapshot) = self.config_snapshot.take() {
            let _ = self.repository.restore_merge_driver_config(&snapshot);
        }
    }
}

/// The outcome of one real squash merge in the scratch worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SquashMergeStatus {
    /// The worktree index has staged changes; commit them.
    Staged,
    /// The merge produced no staged change (the target already holds the tree).
    NoChanges,
    /// Real unmerged paths remain. The caller may resolve registered-driver
    /// paths and re-check, or refuse with exactly these paths.
    Conflicts(Vec<GitPath>),
}

/// One `git diff --name-status -M` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// The raw status letter (`A`, `M`, `D`, `R100`, `C75`, ...).
    pub status: String,
    /// The path. A rename/copy record reports its destination here.
    pub path: GitPath,
    /// The source path of a rename/copy record.
    pub source: Option<GitPath>,
}

impl DiffEntry {
    /// The leading status letter.
    pub fn kind(&self) -> char {
        self.status.chars().next().unwrap_or(' ')
    }
}

fn optional(value: Option<&Oid>) -> String {
    value
        .map(Oid::to_string)
        .unwrap_or_else(|| "absent".to_owned())
}

impl GitRepository {
    /// Read one git-config value from the local scope.
    fn local_config_get(&self, key: &str) -> Result<Option<String>, GitError> {
        let output = self
            .process()
            .allowing(&["config", "--local", "--get", key], &[1])?;
        if output.status.code() != Some(0) {
            return Ok(None);
        }
        Ok(Some(
            utf8_field(strip_line_feed(&output.stdout), "git config value")?.to_owned(),
        ))
    }

    fn local_config_set(&self, key: &str, value: &str) -> Result<(), GitError> {
        self.process().checked(&["config", "--local", key, value])?;
        Ok(())
    }

    fn local_config_unset(&self, key: &str) -> Result<(), GitError> {
        // Exit 5 means "you tried to unset a key that does not exist"; anything
        // else non-zero is a real failure.
        self.process()
            .allowing(&["config", "--local", "--unset-all", key], &[5])?;
        Ok(())
    }

    /// The repository's shared git common directory (worktree-safe).
    pub fn git_common_dir(&self) -> Result<PathBuf, GitError> {
        let output = self.process().checked(&["rev-parse", "--git-common-dir"])?;
        let text = utf8_field(strip_line_feed(&output.stdout), "git common dir")?;
        let path = Path::new(text.trim());
        Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root().join(path)
        })
    }

    /// Install the seven merge-driver definitions (`merge.<key>.name/driver`).
    ///
    /// This is Source `install_merge_driver_config` / `#5759`: a key is written
    /// only when it is unset or already holds a value a release shipped; a value
    /// the operator set themselves is kept (User Customization Preservation).
    /// It does not seed `.git/info/attributes` — defining a driver is inert
    /// until an attribute maps a path to it.
    pub fn install_merge_driver_config(&self) -> Result<DriverInstallOutcome, GitError> {
        let mut customized = false;
        let mut installed = false;
        for entry in driver_config_entries() {
            let current = self.local_config_get(&entry.key)?;
            match current {
                None => {
                    self.local_config_set(&entry.key, &entry.shipped)?;
                    installed = true;
                }
                Some(value) if entry.accepted.contains(&value) => {
                    if value != entry.shipped {
                        self.local_config_set(&entry.key, &entry.shipped)?;
                        installed = true;
                    }
                }
                Some(_) => customized = true,
            }
        }
        if customized {
            return Ok(DriverInstallOutcome::Customized);
        }
        Ok(if installed {
            DriverInstallOutcome::Installed
        } else {
            DriverInstallOutcome::Present
        })
    }

    /// One warning per merge-driver key left alone because it holds your value.
    pub fn merge_driver_customization_warnings(&self) -> Result<Vec<String>, GitError> {
        let mut warnings = Vec::new();
        for entry in driver_config_entries() {
            if let Some(current) = self.local_config_get(&entry.key)?
                && !entry.accepted.contains(&current)
            {
                warnings.push(format!(
                    "Kept your customized git config {} = {current:?}; Spec Kitty ships {:?}. Set it to the shipped value to take ours.",
                    entry.key, entry.shipped
                ));
            }
        }
        Ok(warnings)
    }

    fn snapshot_merge_driver_config(&self) -> Result<BTreeMap<String, Option<String>>, GitError> {
        let mut snapshot = BTreeMap::new();
        for entry in driver_config_entries() {
            snapshot.insert(entry.key.clone(), self.local_config_get(&entry.key)?);
        }
        Ok(snapshot)
    }

    fn restore_merge_driver_config(
        &self,
        snapshot: &BTreeMap<String, Option<String>>,
    ) -> Result<(), GitError> {
        for (key, value) in snapshot {
            match value {
                Some(value) => self.local_config_set(key, value)?,
                None => self.local_config_unset(key)?,
            }
        }
        Ok(())
    }

    /// Append the driver attribute lines to `$GIT_COMMON_DIR/info/attributes`.
    ///
    /// Additive and idempotent: operator lines are preserved and only missing
    /// lines are appended. Returns the lines this call added, so teardown can
    /// remove exactly those.
    fn ensure_info_attributes(&self) -> Result<Vec<String>, GitError> {
        let common_dir = self.git_common_dir()?;
        let info_dir = common_dir.join("info");
        let attributes_path = info_dir.join("attributes");
        let existing = match std::fs::read_to_string(&attributes_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(GitError::Io {
                    context: "read .git/info/attributes",
                    source,
                });
            }
        };
        let present: std::collections::BTreeSet<&str> = existing.lines().collect();
        let wanted = merge_driver_attribute_lines();
        let added: Vec<String> = wanted
            .into_iter()
            .filter(|line| !present.contains(line.as_str()))
            .collect();
        if added.is_empty() {
            return Ok(added);
        }
        std::fs::create_dir_all(&info_dir).map_err(|source| GitError::Io {
            context: "create .git/info",
            source,
        })?;
        let mut text = existing;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for line in &added {
            text.push_str(line);
            text.push('\n');
        }
        std::fs::write(&attributes_path, text).map_err(|source| GitError::Io {
            context: "write .git/info/attributes",
            source,
        })?;
        Ok(added)
    }

    fn remove_info_attributes(&self, added: &[String]) -> Result<(), GitError> {
        if added.is_empty() {
            return Ok(());
        }
        let attributes_path = self.git_common_dir()?.join("info").join("attributes");
        let text = match std::fs::read_to_string(&attributes_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(GitError::Io {
                    context: "read .git/info/attributes for teardown",
                    source,
                });
            }
        };
        let remaining: Vec<&str> = text
            .lines()
            .filter(|line| !added.iter().any(|added| added == line))
            .collect();
        if remaining.is_empty() {
            std::fs::remove_file(&attributes_path).map_err(|source| GitError::Io {
                context: "remove an emptied .git/info/attributes",
                source,
            })?;
            return Ok(());
        }
        let mut text = remaining.join("\n");
        text.push('\n');
        std::fs::write(&attributes_path, text).map_err(|source| GitError::Io {
            context: "write .git/info/attributes during teardown",
            source,
        })
    }

    /// Activate the merge drivers for ONE ephemeral consolidation merge.
    ///
    /// Installs the config definitions, seeds the attribute mapping, and returns
    /// a guard that removes exactly the attribute lines it added (and, when
    /// `restore_config` is set, restores the config snapshot).
    pub fn activate_merge_drivers(
        &self,
        restore_config: bool,
    ) -> Result<MergeDriverActivation, GitError> {
        let config_snapshot = if restore_config {
            Some(self.snapshot_merge_driver_config()?)
        } else {
            None
        };
        // Config definitions are owned by `install_merge_driver_config`; the
        // merge path's self-heal restores the shipped values even when an
        // operator had changed them (`_ensure_merge_driver_git_config`).
        for config in MERGE_DRIVER_CONFIGS {
            self.local_config_set(&format!("merge.{}.name", config.config_key), config.name)?;
            self.local_config_set(
                &format!("merge.{}.driver", config.config_key),
                config.command,
            )?;
        }
        let added_attribute_lines = self.ensure_info_attributes()?;
        Ok(MergeDriverActivation {
            repository: self.clone(),
            added_attribute_lines,
            config_snapshot,
        })
    }

    /// Create a detached scratch worktree at `start`.
    ///
    /// The path must be absolute, outside every git directory, not the primary
    /// checkout, and absent or empty. No operation may be in progress.
    pub fn add_detached_worktree(&self, path: &Path, start: &StartPoint) -> Result<Oid, GitError> {
        if !path.is_absolute() {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "a scratch worktree path must be absolute, got {}",
                    path.display()
                ),
            });
        }
        if self.is_operation_in_progress()? {
            let operations = self.in_progress_operations()?;
            return Err(GitError::OperationInProgress {
                operation: operations
                    .iter()
                    .map(|operation| operation.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }
        if path == self.root() {
            return Err(GitError::InvalidPlan {
                detail: "a scratch worktree cannot be the primary checkout".to_owned(),
            });
        }
        if let Ok(git_dirs) = self.git_dirs()
            && path.starts_with(git_dirs.git_dir())
        {
            return Err(GitError::InvalidPlan {
                detail: "a scratch worktree cannot live inside a git directory".to_owned(),
            });
        }
        if self
            .worktrees()?
            .iter()
            .any(|worktree| worktree.path() == path)
        {
            return Err(GitError::WorktreePathUnavailable {
                path: path.to_path_buf(),
                detail: "already registered as a worktree".to_owned(),
            });
        }
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                let mut entries = std::fs::read_dir(path).map_err(|source| GitError::Io {
                    context: "inspect the scratch worktree directory",
                    source,
                })?;
                if entries.next().is_some() {
                    return Err(GitError::WorktreePathUnavailable {
                        path: path.to_path_buf(),
                        detail: "exists and is not empty".to_owned(),
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(GitError::Io {
                    context: "inspect the scratch worktree path",
                    source,
                });
            }
        }
        let oid = self.resolve_commit(start)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| GitError::Io {
                context: "create the scratch worktree parent",
                source,
            })?;
        }
        let args = [
            "worktree".into(),
            "add".into(),
            "--detach".into(),
            path.as_os_str().to_os_string(),
            OsString::from(oid.as_str()),
        ];
        let output = self.process().run(&args)?;
        if !output.status.success() {
            return Err(GitError::WorktreeAddFailed {
                path: path.to_path_buf(),
                branch: format!("detached at {oid}"),
                detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                branch_created: false,
                registered: self
                    .worktrees()?
                    .iter()
                    .any(|worktree| worktree.path() == path),
            });
        }
        if !self
            .worktrees()?
            .iter()
            .any(|worktree| worktree.path() == path)
        {
            return Err(GitError::InvalidOutput {
                detail: format!(
                    "git worktree add succeeded but {} is not registered",
                    path.display()
                ),
            });
        }
        Ok(oid)
    }

    /// Remove a scratch worktree this consolidation created.
    pub fn remove_scratch_worktree(
        &self,
        path: &Path,
        expected_head: &Oid,
    ) -> Result<(), GitError> {
        let plan = self.plan_worktree_removal(
            path,
            Some(expected_head),
            None,
            crate::WorktreeDiscardPolicy::RefuseDirty,
        )?;
        self.apply_worktree_removal(&plan)?;
        Ok(())
    }

    /// Run the REAL `git merge --squash` inside `worktree`.
    ///
    /// The Source two-way meta opt-out is set for exactly this child. A clean
    /// merge leaves the index staged; a conflict returns the actual unmerged
    /// paths (never a message scraped from stderr).
    pub fn squash_merge(
        &self,
        worktree: &Path,
        source: &BranchName,
    ) -> Result<SquashMergeStatus, GitError> {
        let scratch = GitRepository::open_with_options(worktree, self.options())?;
        let output = scratch
            .process()
            .run_merge_activation(&["merge", "--squash", source.as_str()], true)?;
        if output.status.code() == Some(0) {
            return scratch.staged_state();
        }
        let conflicts = scratch.unmerged_paths()?;
        if !conflicts.is_empty() {
            return Ok(SquashMergeStatus::Conflicts(
                conflicts
                    .iter()
                    .map(|conflict| conflict.path().clone())
                    .collect(),
            ));
        }
        Err(GitError::CommandFailed {
            args: vec![
                "merge".to_owned(),
                "--squash".to_owned(),
                source.as_str().to_owned(),
            ],
            cwd: worktree.to_path_buf(),
            status: output.status.code(),
            stderr: {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if detail.is_empty() {
                    "git merge --squash failed without a diagnostic".to_owned()
                } else {
                    detail
                }
            },
        })
    }

    /// Whether the worktree index has staged changes.
    pub fn staged_state(&self) -> Result<SquashMergeStatus, GitError> {
        let output = self
            .process()
            .run(&["diff", "--cached", "--quiet", "--exit-code"])?;
        Ok(match output.status.code() {
            Some(0) => SquashMergeStatus::NoChanges,
            Some(1) => SquashMergeStatus::Staged,
            _ => {
                let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(GitError::CommandFailed {
                    args: vec![
                        "diff".to_owned(),
                        "--cached".to_owned(),
                        "--quiet".to_owned(),
                    ],
                    cwd: self.root().to_path_buf(),
                    status: output.status.code(),
                    stderr: if detail.is_empty() {
                        "git diff --cached failed without a diagnostic".to_owned()
                    } else {
                        detail
                    },
                });
            }
        })
    }

    /// The staged paths (`git diff --cached --name-only -z`).
    pub fn staged_paths(&self) -> Result<Vec<GitPath>, GitError> {
        let output = self
            .process()
            .checked(&["diff", "--cached", "--name-only", "-z"])?;
        let mut paths = Vec::new();
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            paths.push(GitPath::from_git_bytes(record)?);
        }
        Ok(paths)
    }

    /// Read one blob object by id (stage 1/2/3 content, resolved driver inputs).
    pub fn read_blob_object(&self, oid: &Oid) -> Result<Vec<u8>, GitError> {
        let size_output = self.process().checked(&["cat-file", "-s", oid.as_str()])?;
        let size_text = utf8_field(strip_line_feed(&size_output.stdout), "blob size")?;
        let size = size_text
            .trim()
            .parse::<u64>()
            .map_err(|_| GitError::InvalidOutput {
                detail: format!("git cat-file -s printed a non-numeric size {size_text:?}"),
            })?;
        if size > MAX_BLOB_BYTES {
            return Err(GitError::BlobTooLarge {
                path: oid.to_string(),
                limit: MAX_BLOB_BYTES,
            });
        }
        let body = self
            .process()
            .checked_with_output_limit(&["cat-file", "blob", oid.as_str()], MAX_BLOB_BYTES)?;
        if body.stdout.len() as u64 != size {
            return Err(GitError::InvalidOutput {
                detail: format!(
                    "git cat-file blob returned {} bytes for a {size}-byte blob",
                    body.stdout.len()
                ),
            });
        }
        Ok(body.stdout)
    }

    /// Read one blob object with an explicit bound.
    pub fn read_blob_object_bounded(&self, oid: &Oid, limit: u64) -> Result<Vec<u8>, GitError> {
        if limit > MAX_BLOB_BYTES {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "a blob read bound must be at most {MAX_BLOB_BYTES} bytes, got {limit}"
                ),
            });
        }
        let size_output = self.process().checked(&["cat-file", "-s", oid.as_str()])?;
        let size_text = utf8_field(strip_line_feed(&size_output.stdout), "blob size")?;
        let size = size_text
            .trim()
            .parse::<u64>()
            .map_err(|_| GitError::InvalidOutput {
                detail: format!("git cat-file -s printed a non-numeric size {size_text:?}"),
            })?;
        if size > limit {
            return Err(GitError::BlobTooLarge {
                path: oid.to_string(),
                limit,
            });
        }
        let body = self
            .process()
            .checked_with_output_limit(&["cat-file", "blob", oid.as_str()], limit.max(1))?;
        Ok(body.stdout)
    }

    /// Commit the currently staged index of `worktree` as a single-parent commit.
    ///
    /// `expected_head` is the worktree HEAD the merge started from. The commit
    /// this call created is proven from the captured index tree, the pinned
    /// parent, and reflog/summary evidence bound to the invocation. Hooks run
    /// normally (`commit.gpgsign=false` is the one forced setting, matching the
    /// ordinary conclusion route). A post-state that does not confirm the
    /// created commit is [`GitError::MergeConclusionUnconfirmed`]; nothing is
    /// rolled back.
    pub fn commit_staged_merge(&self, expected_head: &Oid, message: &str) -> Result<Oid, GitError> {
        if message.trim().is_empty() {
            return Err(GitError::InvalidPlan {
                detail: "an explicit squash commit message must not be empty".to_owned(),
            });
        }
        if message.contains('\0') {
            return Err(GitError::InvalidPlan {
                detail: "a squash commit message must not contain NUL".to_owned(),
            });
        }
        let head = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_owned(),
            })
            .and_then(|text| Oid::parse(&text))?;
        if &head != expected_head {
            return Err(GitError::PlanStale {
                what: "HEAD".to_owned(),
                detail: format!("expected {expected_head}, observed {head}"),
            });
        }
        let tree_output = self.process().checked(&["write-tree"])?;
        let tree = utf8_field(strip_line_feed(&tree_output.stdout), "index tree")?
            .trim()
            .to_owned();
        if self.staged_paths()?.is_empty() {
            return Err(GitError::InvalidPlan {
                detail: "the squash commit would be empty: nothing is staged".to_owned(),
            });
        }
        let mark = self.publication_mark_for("HEAD");
        let output = self.process().run_commit_hook(
            &["-c", "commit.gpgsign=false", "commit", "-m", message],
            None,
            true,
        )?;
        let observed_head = self
            .head_commit()?
            .map(|text| Oid::parse(&text))
            .transpose()?;
        let expected_parents = [expected_head.clone()];
        let mut notes: Vec<String> = Vec::new();
        let candidate = if output.status.success() {
            self.created_commit_from_summary(&output.stdout, &expected_parents)?
                .or_else(|| self.reflog_conclusion_candidate(&expected_parents, &mark, &mut notes))
        } else {
            self.reflog_conclusion_candidate(&expected_parents, &mark, &mut notes)
        };
        let confirmed = output.status.success()
            && candidate.is_some()
            && observed_head.as_ref() == candidate.as_ref();
        if confirmed {
            let created = candidate.expect("a confirmed staged commit has a created commit");
            let created_tree = self.rev_parse(&format!("{created}^{{tree}}"))?;
            if created_tree.as_deref() != Some(tree.as_str()) {
                return Err(GitError::MergeConclusionUnconfirmed(Box::new(
                    MergeConclusionUnconfirmed {
                        operation: "squash".to_owned(),
                        worktree_root: self.root().to_path_buf(),
                        candidate: Some(created.clone()),
                        previous_head: expected_head.clone(),
                        observed_head,
                        operation_consumed: true,
                        marker: OperationMarkerState::Consumed,
                        detail: format!(
                            "the created commit {created} does not carry the captured index tree {tree} (observed {created_tree:?})"
                        ),
                    },
                )));
            }
            return Ok(created);
        }
        let detail = if output.status.success() {
            format!(
                "git commit reported success but no created commit could be bound to this call ({}); observed HEAD {:?}",
                notes.join("; "),
                observed_head.as_ref().map(Oid::to_string)
            )
        } else {
            format!(
                "git commit failed (status {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        };
        Err(GitError::MergeConclusionUnconfirmed(Box::new(
            MergeConclusionUnconfirmed {
                operation: "squash".to_owned(),
                worktree_root: self.root().to_path_buf(),
                candidate,
                previous_head: expected_head.clone(),
                observed_head,
                operation_consumed: false,
                marker: OperationMarkerState::Consumed,
                detail,
            },
        )))
    }

    /// Identify the commit git's own summary named (`[ref <abbrev>] subject`).
    ///
    /// This is creation evidence for the invocation that printed it: the summary
    /// is the child's output and the named commit must carry the pinned parent
    /// set. A noisy line that names a pre-existing identical commit is refused by
    /// the parent check plus the publication mark in the reflog fallback.
    fn created_commit_from_summary(
        &self,
        stdout: &[u8],
        expected_parents: &[Oid],
    ) -> Result<Option<Oid>, GitError> {
        for line in stdout.split(|byte| *byte == b'\n') {
            if line.first() != Some(&b'[') {
                continue;
            }
            let Some(close) = line.iter().position(|byte| *byte == b']') else {
                continue;
            };
            let token = line[1..close]
                .split(|byte| *byte == b' ')
                .next_back()
                .unwrap_or_default();
            if token.len() < 4 || token.len() > 64 || !token.iter().all(u8::is_ascii_hexdigit) {
                continue;
            }
            let text = String::from_utf8_lossy(token);
            if let Some(oid) = self.resolve_commit_text(&format!("{text}^{{commit}}"))?
                && self.commit_parents_match(&oid, expected_parents)?
            {
                return Ok(Some(oid));
            }
        }
        Ok(None)
    }

    fn resolve_commit_text(&self, spec: &str) -> Result<Option<Oid>, GitError> {
        let output = self
            .process()
            .allowing(&["rev-parse", "--verify", "--quiet", spec], &[1])?;
        if output.status.code() != Some(0) {
            return Ok(None);
        }
        let text = utf8_field(strip_line_feed(&output.stdout), "commit id")?;
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Oid::parse(text.trim()).map(Some)
        }
    }

    fn rev_parse(&self, spec: &str) -> Result<Option<String>, GitError> {
        let output = self
            .process()
            .allowing(&["rev-parse", "--verify", "--quiet", spec], &[1])?;
        if output.status.code() != Some(0) {
            return Ok(None);
        }
        Ok(Some(
            utf8_field(strip_line_feed(&output.stdout), "rev-parse result")?
                .trim()
                .to_owned(),
        ))
    }

    /// Rebase the detached worktree HEAD onto `upstream`.
    ///
    /// This is Source's REBASE stride: `git rebase <target>` in an isolated
    /// checkout, then the caller advances the target ref to the rebased tip. A
    /// conflict aborts the rebase in the scratch worktree and returns the paths;
    /// the target ref is never touched.
    pub fn rebase_onto(&self, upstream: &BranchName) -> Result<RebaseStatus, GitError> {
        let before = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_owned(),
            })
            .and_then(|text| Oid::parse(&text))?;
        let output = self
            .process()
            .run_merge_activation(&["rebase", upstream.as_str()], false)?;
        if output.status.success() {
            let after = self
                .head_commit()?
                .ok_or_else(|| GitError::RefMissing {
                    name: "HEAD".to_owned(),
                })
                .and_then(|text| Oid::parse(&text))?;
            return Ok(RebaseStatus::Rebased {
                previous_tip: before,
                new_tip: after,
            });
        }
        let conflicts: Vec<GitPath> = self
            .unmerged_paths()?
            .iter()
            .map(|conflict| conflict.path().clone())
            .collect();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // Restore the scratch checkout to its pre-rebase state; a rebase that
        // stops must not leave the scratch worktree wedged.
        let abort = self.process().run(&["rebase", "--abort"])?;
        if !abort.status.success() {
            return Err(GitError::MergeAbortFailed {
                worktree_root: self.root().to_path_buf(),
                detail: format!(
                    "rebase of {} failed and could not be aborted: {} (rebase stderr: {stderr})",
                    upstream.as_str(),
                    String::from_utf8_lossy(&abort.stderr).trim()
                ),
            });
        }
        Err(GitError::CommandFailed {
            args: vec!["rebase".to_owned(), upstream.as_str().to_owned()],
            cwd: self.root().to_path_buf(),
            status: output.status.code(),
            stderr: if conflicts.is_empty() {
                if stderr.is_empty() {
                    "git rebase failed without a diagnostic".to_owned()
                } else {
                    stderr
                }
            } else {
                format!(
                    "{stderr} (unmerged paths: {})",
                    conflicts
                        .iter()
                        .map(GitPath::display_lossy)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        })
    }

    /// Run the real `git merge --no-ff --no-edit -m <message> <source>`.
    ///
    /// This is the Source MERGE strategy. `two_way_meta` sets the mission→target
    /// meta driver opt-out for exactly this child. A clean merge commits and is
    /// confirmed from invocation-bound evidence; a conflict returns the real
    /// unmerged paths with `MERGE_HEAD` intact for the caller to resolve and
    /// conclude (or abort).
    pub fn merge_no_ff(
        &self,
        source: &BranchName,
        message: &str,
        two_way_meta: bool,
    ) -> Result<MergeNoFfStatus, GitError> {
        if message.trim().is_empty() || message.contains('\0') {
            return Err(GitError::InvalidPlan {
                detail: "a merge message must be non-empty and free of NUL".to_owned(),
            });
        }
        let before = self
            .head_commit()?
            .ok_or_else(|| GitError::RefMissing {
                name: "HEAD".to_owned(),
            })
            .and_then(|text| Oid::parse(&text))?;
        let Some(source_tip) = self.read_branch_tip(source)? else {
            return Err(GitError::BranchMissing {
                branch: source.as_str().to_owned(),
            });
        };
        if self.is_ancestor(&source_tip, &before)? {
            return Ok(MergeNoFfStatus::AlreadyUpToDate);
        }
        let mark = self.publication_mark_for("HEAD");
        let output = self.process().run_merge_activation(
            &[
                "merge",
                "--no-ff",
                "--no-edit",
                "-m",
                message,
                source.as_str(),
            ],
            two_way_meta,
        )?;
        if !output.status.success() {
            let conflicts: Vec<GitPath> = self
                .unmerged_paths()?
                .iter()
                .map(|conflict| conflict.path().clone())
                .collect();
            if !conflicts.is_empty() {
                return Ok(MergeNoFfStatus::Conflicts(conflicts));
            }
            return Err(GitError::CommandFailed {
                args: vec![
                    "merge".to_owned(),
                    "--no-ff".to_owned(),
                    source.as_str().to_owned(),
                ],
                cwd: self.root().to_path_buf(),
                status: output.status.code(),
                stderr: {
                    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    if detail.is_empty() {
                        "git merge --no-ff failed without a diagnostic".to_owned()
                    } else {
                        detail
                    }
                },
            });
        }
        let observed_head = self
            .head_commit()?
            .map(|text| Oid::parse(&text))
            .transpose()?;
        let expected_parents = [before.clone(), source_tip];
        // Source trusts the committing merge's exit status and then reads HEAD
        // (`integrate_mission_into_target` -> `_rev_parse(target_branch)`); it
        // has no separate creation proof. The pinned parent pair is the native
        // check that HEAD is the merge this call made. A summary/reflog candidate
        // is still preferred when available.
        if let Some(observed) = &observed_head
            && observed != &before
            && self.commit_parents_match(observed, &expected_parents)?
        {
            return Ok(MergeNoFfStatus::Merged {
                commit: observed.clone(),
            });
        }
        let mut notes = Vec::new();
        let candidate = self
            .created_commit_from_summary(&output.stdout, &expected_parents)?
            .or_else(|| self.reflog_conclusion_candidate(&expected_parents, &mark, &mut notes));
        let confirmed = candidate.is_some() && observed_head.as_ref() == candidate.as_ref();
        if confirmed {
            return Ok(MergeNoFfStatus::Merged {
                commit: candidate.expect("a confirmed merge has a created commit"),
            });
        }
        let marker_present = self.merge_head()?.is_some();
        Err(GitError::MergeConclusionUnconfirmed(Box::new(
            MergeConclusionUnconfirmed {
                operation: "merge".to_owned(),
                worktree_root: self.root().to_path_buf(),
                candidate,
                previous_head: before,
                observed_head,
                operation_consumed: !marker_present,
                marker: if marker_present {
                    OperationMarkerState::Unchanged
                } else {
                    OperationMarkerState::Consumed
                },
                detail: format!(
                    "git merge reported success but no created commit could be bound to this call ({}); nothing was rolled back",
                    notes.join("; ")
                ),
            },
        )))
    }

    /// Conclude a conflicted merge after the caller staged resolved paths.
    pub fn conclude_merge_in_progress(
        &self,
        expected_head: &Oid,
        message: &str,
    ) -> Result<Oid, GitError> {
        let plan = self.plan_merge_conclusion(expected_head, Some(message))?;
        Ok(self.apply_merge_conclusion(&plan)?.new_head().clone())
    }

    /// Advance `branch` from `expected_old` to `new`, resyncing clean checkouts.
    ///
    /// The move must be a fast-forward (every consolidation stride produces one
    /// from the observed target tip). Each checkout with the branch checked out
    /// must have no tracked modification and no in-progress operation; untracked
    /// files are preserved by `reset --hard`. A ref that moved is
    /// [`GitError::PlanStale`]; a ref that moved but whose checkout could not
    /// resync is [`GitError::ResyncFailed`] (evidence, not a silent success).
    ///
    /// The ref write carries NO reflog message: Source advances a branch with
    /// `git update-ref <ref> <new> <old>` and no `-m`, so git records an empty
    /// reflog subject. This adapter states that absence explicitly instead of
    /// inventing a message or an empty string (git refuses `-m ""`).
    pub fn advance_branch_ref(
        &self,
        branch: &BranchName,
        expected_old: &Oid,
        new: &Oid,
    ) -> Result<Vec<PathBuf>, GitError> {
        let previous = self.read_branch_tip(branch)?;
        if previous.as_ref() != Some(expected_old) {
            return Err(GitError::PlanStale {
                what: branch.as_ref_name().to_string(),
                detail: format!(
                    "expected {}, observed {}",
                    expected_old,
                    optional(previous.as_ref())
                ),
            });
        }
        if !self.commit_exists(new)? {
            return Err(GitError::RefMissing {
                name: new.to_string(),
            });
        }
        if !self.is_ancestor(expected_old, new)? {
            return Err(GitError::NonFastForward {
                branch: branch.as_str().to_owned(),
                old: expected_old.to_string(),
                new: new.to_string(),
            });
        }
        let checkouts = self.checkouts_of(branch)?;
        for checkout in &checkouts {
            let handle = GitRepository::open_with_options(checkout, self.options())?;
            if handle.is_operation_in_progress()? {
                let operations = handle.in_progress_operations()?;
                return Err(GitError::OperationInProgress {
                    operation: operations
                        .iter()
                        .map(|operation| operation.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                });
            }
            let entries = handle.status_with(&StatusOptions {
                untracked: UntrackedMode::No,
                ignored: false,
            })?;
            if !entries.is_empty() {
                return Err(GitError::DirtyWorktree {
                    path: checkout.clone(),
                    entries: entries.iter().map(|entry| entry.display_lossy()).collect(),
                });
            }
        }
        let transaction = self.plan_silent_ref_transaction(vec![RefChange::set(
            branch.as_ref_name(),
            new.clone(),
            Some(expected_old.clone()),
        )])?;
        self.apply_silent_ref_transaction(&transaction)?;
        let mut resynced = Vec::new();
        for checkout in &checkouts {
            let handle = GitRepository::open_with_options(checkout, self.options())?;
            // Source resyncs with `git reset --hard <branch>`, not the object id
            // (ref_advance._resync_checkouts), so the checkout's own HEAD reflog
            // records `reset: moving to <branch>` exactly as Source's does.
            let output = handle
                .process()
                .run(&["reset", "--hard", branch.as_str()])?;
            if !output.status.success() {
                return Err(GitError::ResyncFailed {
                    checkout: checkout.clone(),
                    detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                });
            }
            resynced.push(checkout.clone());
        }
        Ok(resynced)
    }

    /// Restore `branch` to `restore_to` after a landed consolidation failed a
    /// post-landing gate — the rollback door's ONLY ref rewind.
    ///
    /// `expected_tip` is the tip this run observed: the transaction is a CAS, so a
    /// branch another process advanced since is refused rather than overwritten.
    /// The ref is moved with the ordinary ref transaction authority (no raw
    /// `update-ref`), every checkout of the branch is reset to the restored
    /// commit, and — exactly like Source's `restore_branch_ref` — the reflog
    /// entry carries NO message. Only the caller that proved the landing is being
    /// undone may call this: it is deliberately separate from
    /// [`Self::advance_branch_ref`], which refuses a non-fast-forward.
    pub fn restore_branch_ref(
        &self,
        branch: &BranchName,
        expected_tip: &Oid,
        restore_to: &Oid,
    ) -> Result<Vec<PathBuf>, GitError> {
        let previous = self.read_branch_tip(branch)?;
        if previous.as_ref() != Some(expected_tip) {
            return Err(GitError::PlanStale {
                what: branch.as_ref_name().to_string(),
                detail: format!(
                    "expected {}, observed {}",
                    expected_tip,
                    optional(previous.as_ref())
                ),
            });
        }
        if !self.commit_exists(restore_to)? {
            return Err(GitError::RefMissing {
                name: restore_to.to_string(),
            });
        }
        let checkouts = self.checkouts_of(branch)?;
        for checkout in &checkouts {
            let handle = GitRepository::open_with_options(checkout, self.options())?;
            if handle.is_operation_in_progress()? {
                let operations = handle.in_progress_operations()?;
                return Err(GitError::OperationInProgress {
                    operation: operations
                        .iter()
                        .map(|operation| operation.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                });
            }
        }
        let transaction = self.plan_silent_ref_transaction(vec![RefChange::set(
            branch.as_ref_name(),
            restore_to.clone(),
            Some(expected_tip.clone()),
        )])?;
        self.apply_silent_ref_transaction(&transaction)?;
        let mut resynced = Vec::new();
        for checkout in &checkouts {
            let handle = GitRepository::open_with_options(checkout, self.options())?;
            // Same operand as Source's restore: `reset --hard <branch>`.
            let output = handle
                .process()
                .run(&["reset", "--hard", branch.as_str()])?;
            if !output.status.success() {
                return Err(GitError::ResyncFailed {
                    checkout: checkout.clone(),
                    detail: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                });
            }
            resynced.push(checkout.clone());
        }
        Ok(resynced)
    }

    /// The committer date (Unix seconds) of the last commit touching `path` at
    /// `tip`, or `None` when the path has no commit there.
    ///
    /// This is the Source planning-recency tiebreak basis
    /// (`git log -1 --format=%ct <ref> -- <path>`); never mtime.
    pub fn committer_timestamp(&self, tip: &Oid, path: &GitPath) -> Result<Option<i64>, GitError> {
        let output = self.process().checked(&[
            std::ffi::OsStr::new("--literal-pathspecs"),
            std::ffi::OsStr::new("log"),
            std::ffi::OsStr::new("-1"),
            std::ffi::OsStr::new("--format=%ct"),
            std::ffi::OsStr::new(tip.as_str()),
            std::ffi::OsStr::new("--"),
            path.as_os_str(),
        ])?;
        let text = String::from_utf8_lossy(&output.stdout);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        trimmed
            .parse::<i64>()
            .map(Some)
            .map_err(|error| GitError::InvalidPlan {
                detail: format!(
                    "git log reported a non-numeric committer date {trimmed:?}: {error}"
                ),
            })
    }

    /// Whether two refs currently expose the SAME tree (`git diff --quiet`).
    ///
    /// Source's `_branch_trees_equal` (`consolidation/git_probes.py`): a squash
    /// merge does not preserve ancestry, so reachability alone would call an
    /// already-landed payload un-integrated and produce a no-op merge commit.
    /// Exit 0 is equal, exit 1 is different; every other outcome is an error, so
    /// a failed probe is never read as "equal".
    pub fn trees_equal(&self, left: &RefName, right: &RefName) -> Result<bool, GitError> {
        let output = self.process().allowing(
            &[
                "diff",
                "--quiet",
                "--no-ext-diff",
                left.as_str(),
                right.as_str(),
            ],
            &[1],
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(GitError::InvalidOutput {
                detail: "git diff --quiet returned an unexpected status".to_string(),
            }),
        }
    }

    /// `git add -- <path>` for a path whose bytes the caller already wrote.
    pub fn stage_path(&self, path: &GitPath) -> Result<(), GitError> {
        self.process().checked(&[
            std::ffi::OsStr::new("add"),
            std::ffi::OsStr::new("--"),
            path.as_os_str(),
        ])?;
        Ok(())
    }

    /// `git diff --name-status -M -z <base> <tip>` as typed records.
    pub fn diff_name_status(&self, base: &Oid, tip: &Oid) -> Result<Vec<DiffEntry>, GitError> {
        let output = self.process().checked(&[
            "diff",
            "--name-status",
            "-M",
            "-z",
            base.as_str(),
            tip.as_str(),
        ])?;
        parse_name_status(&output.stdout)
    }

    /// The `+` lines of `git diff --unified=0 <base> <tip> -- <path>`.
    ///
    /// This is the Source dead-code gate's diff basis
    /// (`git diff --unified=0 -M` plus the `+` lines). It is exact for an
    /// addition and does not depend on the previous blob still being present at
    /// `tip` under the same path (a rename is followed like Source's `-M`).
    pub fn diff_added_lines(
        &self,
        base: &Oid,
        tip: &Oid,
        path: &GitPath,
    ) -> Result<Vec<String>, GitError> {
        let output = self.process().checked(&[
            std::ffi::OsStr::new("--literal-pathspecs"),
            std::ffi::OsStr::new("diff"),
            std::ffi::OsStr::new("--unified=0"),
            std::ffi::OsStr::new("--no-color"),
            std::ffi::OsStr::new("--no-ext-diff"),
            std::ffi::OsStr::new(base.as_str()),
            std::ffi::OsStr::new(tip.as_str()),
            std::ffi::OsStr::new("--"),
            path.as_os_str(),
        ])?;
        Ok(parse_added_lines(&output.stdout))
    }

    /// Read a blob at `tip:path` bounded by the ordinary default.
    pub fn read_blob_default(
        &self,
        tip: &Oid,
        path: &GitPath,
    ) -> Result<Option<Vec<u8>>, GitError> {
        self.read_blob_at_bounded(&StartPoint::Oid(tip.clone()), path, DEFAULT_MAX_BLOB_BYTES)
    }
}

/// How a `--no-ff` merge ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeNoFfStatus {
    /// The merge commit this call created, proven from invocation-bound evidence.
    Merged {
        /// The created merge commit.
        commit: Oid,
    },
    /// `source` is already an ancestor of `HEAD`; nothing was written.
    AlreadyUpToDate,
    /// Real unmerged paths remain, with `MERGE_HEAD` still recorded.
    Conflicts(Vec<GitPath>),
}

/// How a rebase ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebaseStatus {
    /// The commits were replayed; `new_tip` is the detached worktree HEAD.
    Rebased {
        /// The pre-rebase tip.
        previous_tip: Oid,
        /// The rebased tip.
        new_tip: Oid,
    },
}

fn parse_name_status(bytes: &[u8]) -> Result<Vec<DiffEntry>, GitError> {
    let mut records = bytes.split(|byte| *byte == 0);
    let mut entries = Vec::new();
    while let Some(status) = records.next() {
        if status.is_empty() {
            continue;
        }
        let status = String::from_utf8(status.to_vec()).map_err(|_| GitError::InvalidOutput {
            detail: "git diff --name-status printed a non-UTF-8 status".to_owned(),
        })?;
        let Some(first) = records.next() else {
            return Err(GitError::InvalidOutput {
                detail: format!("git diff --name-status printed status {status:?} with no path"),
            });
        };
        let path = GitPath::from_git_bytes(first)?;
        if status.starts_with('R') || status.starts_with('C') {
            let Some(second) = records.next() else {
                return Err(GitError::InvalidOutput {
                    detail: format!(
                        "git diff --name-status printed a {status} record with no destination"
                    ),
                });
            };
            entries.push(DiffEntry {
                status,
                path: GitPath::from_git_bytes(second)?,
                source: Some(path),
            });
        } else {
            entries.push(DiffEntry {
                status,
                path,
                source: None,
            });
        }
    }
    Ok(entries)
}

fn parse_added_lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(added) = line.strip_prefix('+') {
            lines.push(added.to_owned());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_parses_add_modify_delete_and_rename() {
        let bytes = b"A\0a.py\0M\0b.py\0D\0c.py\0R100\0old.py\0new.py\0";
        let entries = parse_name_status(bytes).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].kind(), 'A');
        assert_eq!(entries[0].path.display_lossy(), "a.py");
        assert_eq!(entries[3].kind(), 'R');
        assert_eq!(entries[3].path.display_lossy(), "new.py");
        assert_eq!(
            entries[3].source.as_ref().map(GitPath::display_lossy),
            Some("old.py".to_owned())
        );
    }

    #[test]
    fn added_lines_reads_only_plus_lines_and_skips_file_headers() {
        let diff = b"diff --git a/x.py b/x.py\n--- a/x.py\n+++ b/x.py\n@@ -0,0 +1,2 @@\n+def added():\n+    pass\n";
        assert_eq!(
            parse_added_lines(diff),
            vec!["def added():".to_owned(), "    pass".to_owned()]
        );
    }

    #[test]
    fn the_config_table_is_the_seven_source_drivers() {
        let commands: Vec<&str> = MERGE_DRIVER_CONFIGS
            .iter()
            .map(|config| config.command)
            .collect();
        assert_eq!(
            commands,
            vec![
                "spec-kitty merge-driver-event-log %O %A %B",
                "spec-kitty merge-driver-meta %O %A %B",
                "spec-kitty merge-driver-traces %O %A %B",
                "spec-kitty merge-driver-acceptance-matrix %O %A %B",
                "spec-kitty merge-driver-issue-matrix %O %A %B",
                "spec-kitty merge-driver-review-cycle %O %A %B",
                "spec-kitty merge-driver-decision-index %O %A %B",
            ]
        );
    }
}
