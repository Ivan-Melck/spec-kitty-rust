//! Per-worktree sparse-checkout operations.
//!
//! Lane isolation needs sparse checkouts that are scoped to one linked
//! worktree. Git keeps both the mode (`core.sparseCheckout`,
//! `core.sparseCheckoutCone`) and the patterns (`info/sparse-checkout`) *per
//! worktree* — the paths resolve through that checkout's own git directory, not
//! the shared one. This module always resolves the pattern file with
//! `git rev-parse --git-path info/sparse-checkout` in the target checkout, so a
//! lane never rewrites the primary checkout's sparse state (or a sibling's).
//!
//! # Preserving unrelated user data
//!
//! The adapter never passes `--force`. Git itself refuses to remove a tracked
//! path with local modifications and leaves out-of-cone directories that hold
//! untracked data in place; those protections stay in force. This module only
//! writes the pattern set the caller names, and reads back the resulting state
//! so a partial application is visible. It also refuses while a merge, rebase,
//! cherry-pick, revert, sequencer run, or held index lock is present, and while
//! the index has unmerged entries: changing sparse patterns there would rewrite
//! conflict state.
//!
//! # Plans
//!
//! A plan records the checkout's `HEAD`, the configured mode, and the logical
//! pattern list. Applying it re-reads all of them and refuses if anything
//! moved, so a plan prepared for one state cannot silently replace another.

use std::path::{Path, PathBuf};

use crate::error::GitError;
use crate::index_authority::OperatorIndex;
use crate::path::GitPath;
use crate::repository::GitRepository;

/// How patterns are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SparseCheckoutMode {
    /// Cone mode: patterns are directory names, and leading directories are
    /// included automatically.
    Cone,
    /// Non-cone mode: patterns are gitignore-style pathspecs.
    NoCone,
}

impl SparseCheckoutMode {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cone => "cone",
            Self::NoCone => "no-cone",
        }
    }

    fn argument(self) -> &'static str {
        match self {
            Self::Cone => "--cone",
            Self::NoCone => "--no-cone",
        }
    }
}

/// The oldest git that supports `git sparse-checkout set`.
pub const SPARSE_CHECKOUT_MIN_VERSION: (u32, u32) = (2, 26);

/// The maximum per-worktree pattern file this adapter reads (1 MiB).
pub const MAX_SPARSE_PATTERN_FILE_BYTES: u64 = 1024 * 1024;

/// The observed sparse-checkout state of one checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparseCheckoutState {
    worktree_root: PathBuf,
    configured: bool,
    mode: SparseCheckoutMode,
    patterns: Vec<String>,
    pattern_file: PathBuf,
}

impl SparseCheckoutState {
    /// The checkout this state belongs to.
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    /// Whether `core.sparseCheckout` is enabled here.
    pub fn configured(&self) -> bool {
        self.configured
    }

    /// The observed mode: `Cone` exactly when `core.sparseCheckoutCone` is true.
    ///
    /// `git sparse-checkout init` (and `git sparse-checkout set --cone`) writes
    /// `core.sparseCheckoutCone=true`, so a checkout configured that way reads
    /// as `Cone`. An absent key reads as false, which is git's own config
    /// default and means a non-cone pattern file, so an *unconfigured* checkout
    /// reports `NoCone` together with `configured() == false` — not `Cone`.
    pub fn mode(&self) -> SparseCheckoutMode {
        self.mode
    }

    /// The logical patterns (`git sparse-checkout list`). Empty when the
    /// checkout is not sparse.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// The per-worktree pattern file this checkout uses.
    pub fn pattern_file(&self) -> &Path {
        &self.pattern_file
    }
}

/// A validated plan to set one checkout's sparse patterns.
#[derive(Debug, Clone)]
pub struct SparseCheckoutPlan {
    worktree_root: PathBuf,
    mode: SparseCheckoutMode,
    patterns: Vec<String>,
    head: Option<crate::Oid>,
    previous: SparseCheckoutState,
    index_authority: OperatorIndex,
}

impl SparseCheckoutPlan {
    /// The checkout that will change.
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    /// The mode that will be applied.
    pub fn mode(&self) -> SparseCheckoutMode {
        self.mode
    }

    /// The patterns that will be applied.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// The state observed when the plan was prepared.
    pub fn previous(&self) -> &SparseCheckoutState {
        &self.previous
    }
}

/// The evidence left by a sparse-checkout change.
#[derive(Debug, Clone)]
pub struct SparseCheckoutOutcome {
    state: SparseCheckoutState,
    skip_worktree: Vec<GitPath>,
}

impl SparseCheckoutOutcome {
    /// The state read back after the change.
    pub fn state(&self) -> &SparseCheckoutState {
        &self.state
    }

    /// Every index entry now marked skip-worktree.
    pub fn skip_worktree(&self) -> &[GitPath] {
        &self.skip_worktree
    }
}

impl GitRepository {
    /// Read this checkout's sparse-checkout state.
    ///
    /// The pattern file is resolved per worktree, so calling this on a lane
    /// checkout never reads the primary checkout's patterns.
    pub fn sparse_checkout_state(&self) -> Result<SparseCheckoutState, GitError> {
        let pattern_file = self.sparse_pattern_file()?;
        let configured = self.config_bool("core.sparseCheckout")?.unwrap_or(false);
        let cone = self
            .config_bool("core.sparseCheckoutCone")?
            .unwrap_or(false);
        let mode = if cone {
            SparseCheckoutMode::Cone
        } else {
            SparseCheckoutMode::NoCone
        };
        let patterns = if configured {
            self.sparse_pattern_list()?
        } else {
            Vec::new()
        };
        Ok(SparseCheckoutState {
            worktree_root: self.root().to_path_buf(),
            configured,
            mode,
            patterns,
            pattern_file,
        })
    }

    /// The per-worktree pattern file text, or `None` when it does not exist.
    ///
    /// Read-only: user-authored patterns (including comment lines) are returned
    /// exactly, never reserialised. The final symlink is not followed and the
    /// read is bounded; an oversized file is
    /// [`GitError::SparsePatternFileTooLarge`].
    pub fn sparse_pattern_file_text(&self) -> Result<Option<String>, GitError> {
        let path = self.sparse_pattern_file()?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(GitError::Io {
                    context: "inspect the sparse-checkout pattern file",
                    source,
                });
            }
        }
        let bytes = sk_kernel::no_follow::read_no_follow(&path, MAX_SPARSE_PATTERN_FILE_BYTES + 1)
            .map_err(|error| GitError::Io {
                context: "read the sparse-checkout pattern file",
                source: std::io::Error::other(error.to_string()),
            })?;
        if bytes.len() as u64 > MAX_SPARSE_PATTERN_FILE_BYTES {
            return Err(GitError::SparsePatternFileTooLarge {
                path,
                limit: MAX_SPARSE_PATTERN_FILE_BYTES,
            });
        }
        Ok(Some(crate::path::utf8_field(
            &bytes,
            "sparse-checkout pattern file",
        )?))
    }

    /// Every index entry currently marked skip-worktree in this checkout.
    pub fn skip_worktree_paths(&self) -> Result<Vec<GitPath>, GitError> {
        let output = self.process().checked(&["ls-files", "-t", "-z"])?;
        let mut paths = Vec::new();
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            // `H <path>` for a normal entry, `S <path>` for skip-worktree.
            let (tag, rest) = match record.split_at_checked(1) {
                Some((tag, rest)) if rest.starts_with(b" ") => (tag, &rest[1..]),
                _ => {
                    return Err(GitError::InvalidOutput {
                        detail: format!(
                            "malformed git ls-files -t record {:?}",
                            String::from_utf8_lossy(record)
                        ),
                    });
                }
            };
            if tag == b"S" {
                paths.push(GitPath::from_git_bytes(rest)?);
            }
        }
        Ok(paths)
    }

    /// Prepare a sparse-checkout change for this checkout.
    ///
    /// Validates the patterns (non-empty, one per line, no NUL, no leading `-`),
    /// refuses an in-progress operation or unmerged index, and records the
    /// checkout's current state and `HEAD`.
    pub fn plan_sparse_checkout(
        &self,
        mode: SparseCheckoutMode,
        patterns: &[String],
    ) -> Result<SparseCheckoutPlan, GitError> {
        let patterns = validate_patterns(patterns)?;
        self.require_sparse_preconditions()?;
        self.require_sparse_supported()?;
        let previous = self.sparse_checkout_state()?;
        let head = self
            .head_commit()?
            .map(|text| crate::Oid::parse(&text))
            .transpose()?;
        Ok(SparseCheckoutPlan {
            worktree_root: self.root().to_path_buf(),
            mode,
            patterns,
            head,
            previous,
            index_authority: self.operator_index().clone(),
        })
    }

    /// Apply a prepared sparse-checkout change.
    ///
    /// Refuses if `HEAD`, the configured mode, or the logical pattern list moved
    /// since the plan. Pattern input goes to `git sparse-checkout set` on stdin,
    /// so a pattern that looks like an option stays data.
    pub fn apply_sparse_checkout(
        &self,
        plan: &SparseCheckoutPlan,
    ) -> Result<SparseCheckoutOutcome, GitError> {
        self.require_sparse_plan_current(plan)?;
        let state_before = self.sparse_checkout_state()?;
        if state_before != plan.previous {
            return Err(GitError::PlanStale {
                what: format!("sparse checkout in {}", plan.worktree_root.display()),
                detail: format!(
                    "planned {:?}/{:?}, observed {:?}/{:?}",
                    plan.previous.mode,
                    plan.previous.patterns,
                    state_before.mode,
                    state_before.patterns
                ),
            });
        }

        let mut payload = plan.patterns.join("\n");
        payload.push('\n');
        let output = self.process().checked_with_stdin(
            &["sparse-checkout", "set", plan.mode.argument(), "--stdin"],
            payload.as_bytes(),
        )?;
        let _ = output;

        let state = self.sparse_checkout_state()?;
        if !state.configured || state.mode != plan.mode || state.patterns != plan.patterns {
            return Err(GitError::SparseCheckoutFailed {
                worktree_root: self.root().to_path_buf(),
                detail: format!(
                    "git sparse-checkout set left configured={} mode={} patterns={:?}, planned mode={} patterns={:?}",
                    state.configured,
                    state.mode.as_str(),
                    state.patterns,
                    plan.mode.as_str(),
                    plan.patterns
                ),
            });
        }
        Ok(SparseCheckoutOutcome {
            state,
            skip_worktree: self.skip_worktree_paths()?,
        })
    }

    /// Prepare and apply a sparse-checkout change.
    pub fn set_sparse_checkout(
        &self,
        mode: SparseCheckoutMode,
        patterns: &[String],
    ) -> Result<SparseCheckoutOutcome, GitError> {
        let plan = self.plan_sparse_checkout(mode, patterns)?;
        self.apply_sparse_checkout(&plan)
    }

    /// Disable sparse checkout in this checkout.
    ///
    /// Git restores the full work tree. Locally modified and untracked files are
    /// left alone by git's own checks; this adapter never deletes them.
    pub fn disable_sparse_checkout(&self) -> Result<SparseCheckoutOutcome, GitError> {
        let state = self.sparse_checkout_state()?;
        if !state.configured {
            return Err(GitError::SparseCheckoutNotConfigured {
                worktree_root: self.root().to_path_buf(),
            });
        }
        self.require_sparse_preconditions()?;
        self.require_sparse_supported()?;
        self.process().checked(&["sparse-checkout", "disable"])?;
        let state = self.sparse_checkout_state()?;
        if state.configured {
            return Err(GitError::SparseCheckoutFailed {
                worktree_root: self.root().to_path_buf(),
                detail: "git sparse-checkout disable left the checkout sparse".to_string(),
            });
        }
        Ok(SparseCheckoutOutcome {
            state,
            skip_worktree: self.skip_worktree_paths()?,
        })
    }

    fn require_sparse_plan_current(&self, plan: &SparseCheckoutPlan) -> Result<(), GitError> {
        if self.root() != plan.worktree_root {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "sparse plan was prepared for {} but applied to {}",
                    plan.worktree_root.display(),
                    self.root().display()
                ),
            });
        }
        if self.operator_index() != &plan.index_authority {
            return Err(GitError::PlanStale {
                what: "operator index".to_string(),
                detail: format!(
                    "planned {:?}, observed {:?}",
                    plan.index_authority.explicit_path(),
                    self.operator_index().explicit_path()
                ),
            });
        }
        self.require_sparse_preconditions()?;
        self.require_sparse_supported()?;
        let head = self
            .head_commit()?
            .map(|text| crate::Oid::parse(&text))
            .transpose()?;
        if head != plan.head {
            return Err(GitError::PlanStale {
                what: "HEAD".to_string(),
                detail: format!("planned {:?}, observed {head:?}", plan.head),
            });
        }
        Ok(())
    }

    fn require_sparse_preconditions(&self) -> Result<(), GitError> {
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
        let unmerged = self.unmerged_paths()?;
        if !unmerged.is_empty() {
            return Err(GitError::OperationInProgress {
                operation: "unmerged index entries".to_string(),
            });
        }
        Ok(())
    }

    fn require_sparse_supported(&self) -> Result<(), GitError> {
        let (major, minor, _patch) = self.git_version()?;
        if (major, minor) < SPARSE_CHECKOUT_MIN_VERSION {
            return Err(GitError::Unsupported {
                feature: "git sparse-checkout set (requires git >= 2.26)",
            });
        }
        Ok(())
    }

    /// The per-worktree pattern file, resolved through this checkout's git dir.
    fn sparse_pattern_file(&self) -> Result<PathBuf, GitError> {
        let output =
            self.process()
                .checked(&["rev-parse", "--git-path", "info/sparse-checkout"])?;
        let text = crate::path::utf8_field(
            crate::path::strip_line_feed(&output.stdout),
            "sparse-checkout pattern path",
        )?;
        let path = PathBuf::from(text.trim());
        Ok(if path.is_absolute() {
            path
        } else {
            self.root().join(path)
        })
    }

    fn sparse_pattern_list(&self) -> Result<Vec<String>, GitError> {
        let output = self
            .process()
            .allowing(&["sparse-checkout", "list"], &[1])?;
        if output.status.code() != Some(0) {
            return Ok(Vec::new());
        }
        let text = crate::path::utf8_field(&output.stdout, "sparse-checkout patterns")?;
        Ok(text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// A boolean git config value, absent when the key is unset.
    fn config_bool(&self, key: &str) -> Result<Option<bool>, GitError> {
        let output = self
            .process()
            .allowing(&["config", "--bool", "--get", key], &[1])?;
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        let value = crate::path::strip_line_feed(&output.stdout);
        match value {
            b"true" => Ok(Some(true)),
            b"false" => Ok(Some(false)),
            other => Err(GitError::InvalidOutput {
                detail: format!(
                    "git config --bool --get {key} printed {:?}",
                    String::from_utf8_lossy(other)
                ),
            }),
        }
    }
}

fn validate_patterns(patterns: &[String]) -> Result<Vec<String>, GitError> {
    if patterns.is_empty() {
        return Err(GitError::InvalidPlan {
            detail: "a sparse-checkout change needs at least one pattern".to_string(),
        });
    }
    let mut validated = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        if pattern.is_empty() {
            return Err(GitError::InvalidPlan {
                detail: "an empty sparse-checkout pattern is not allowed".to_string(),
            });
        }
        if pattern.len() > 4096 {
            return Err(GitError::InvalidPlan {
                detail: "a sparse-checkout pattern must be at most 4096 bytes".to_string(),
            });
        }
        if pattern.contains('\0') || pattern.contains('\n') || pattern.contains('\r') {
            return Err(GitError::InvalidPlan {
                detail: "a sparse-checkout pattern must be one line without NUL".to_string(),
            });
        }
        if pattern.starts_with('-') {
            return Err(GitError::InvalidPlan {
                detail: format!(
                    "sparse-checkout pattern {pattern:?} starts with `-` and could be read as an option"
                ),
            });
        }
        validated.push(pattern.clone());
    }
    Ok(validated)
}
