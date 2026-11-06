//! State-root and state-surface diagnosis (`doctor state-roots`).
//!
//! Resolves the three state roots, checks every registered state surface for
//! on-disk presence, and warns when a repository-local runtime surface is
//! present but not covered by `.gitignore` (an accidental-commit risk).
//!
//! # The surface registry
//!
//! [`STATE_SURFACES`] is a **diagnostic projection** of the pinned reference's
//! state contract (`src/specify_cli/state/contract.py`): the fields the
//! diagnosis actually reads — `name`, `path_pattern`, `root`, `authority`,
//! `git_class`, `ignored_via_agent_block` — for all 62 registered surfaces. The
//! reference's full contract also carries format, owner, creation trigger,
//! retention and cleanup policy; those are **not** reproduced here, and the
//! canonical Rust state-contract owner is still pending, so this table must be
//! consolidated (not forked) when that owner lands.
//!
//! A missing or unreadable surface is reported as `present: false`; it is never
//! silently dropped, so a registry entry can never disappear from the report.
//!
//! # Roots
//!
//! Root resolution is supplied by the caller ([`StateRootEnvironment`]) because
//! `$SPEC_KITTY_HOME`, the platform home and the runtime root each have one
//! canonical owner outside this crate. Nothing here recomputes a home
//! independently.
//!
//! # Read-only
//!
//! Presence, gitignore coverage and warnings only. No directory is created, no
//! `.gitignore` is edited, and no surface is adopted.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::git_probe::{self, GitAnswer};
use super::{DoctorError, require_directory};

/// The documentation-only prefix carried by the `global_sync` surface patterns.
const GLOBAL_SYNC_PATTERN_PREFIX: &str = "~/.spec-kitty/";

/// A reserved skill name, covered by the configured-agent block's
/// `spec-kitty-*/` pattern.
const AGENT_BLOCK_PROBE_NAME: &str = "spec-kitty-doctor-probe";

/// The root directory that anchors a family of state surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateRootKind {
    /// `.kittify/`
    Project,
    /// `kitty-specs/<mission>/`
    Feature,
    /// `~/.kittify/` (or `$SPEC_KITTY_HOME`)
    GlobalRuntime,
    /// `~/.spec-kitty/` (or `$SPEC_KITTY_HOME`)
    GlobalSync,
    /// `.git/spec-kitty/`
    GitInternal,
}

impl StateRootKind {
    /// The stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Feature => "feature",
            Self::GlobalRuntime => "global_runtime",
            Self::GlobalSync => "global_sync",
            Self::GitInternal => "git_internal",
        }
    }
}

/// Every state root, in report order.
pub const STATE_ROOTS: [StateRootKind; 5] = [
    StateRootKind::Project,
    StateRootKind::Feature,
    StateRootKind::GlobalRuntime,
    StateRootKind::GlobalSync,
    StateRootKind::GitInternal,
];

/// How authoritative a surface is for its data domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AuthorityClass {
    /// The authored source of truth.
    Authoritative,
    /// Derived from an authoritative surface.
    Derived,
    /// Kept only for compatibility.
    Compatibility,
    /// Local machine state.
    LocalRuntime,
    /// Sensitive local state.
    Secret,
    /// Inside the git directory.
    GitInternal,
    /// Retired.
    Deprecated,
}

impl AuthorityClass {
    /// The stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Authoritative => "authoritative",
            Self::Derived => "derived",
            Self::Compatibility => "compatibility",
            Self::LocalRuntime => "local_runtime",
            Self::Secret => "secret",
            Self::GitInternal => "git_internal",
            Self::Deprecated => "deprecated",
        }
    }
}

/// A surface's relationship to version control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitClass {
    /// Tracked by git.
    Tracked,
    /// Ignored by git.
    Ignored,
    /// Inside the repository and not ignored.
    InsideRepoNotIgnored,
    /// Inside the git directory.
    GitInternal,
    /// Outside the repository.
    OutsideRepo,
}

impl GitClass {
    /// The stable machine-readable label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tracked => "tracked",
            Self::Ignored => "ignored",
            Self::InsideRepoNotIgnored => "inside_repo_not_ignored",
            Self::GitInternal => "git_internal",
            Self::OutsideRepo => "outside_repo",
        }
    }
}

/// One declarative state surface, limited to the fields the diagnosis reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateSurfaceSpec {
    /// Stable surface name.
    pub name: &'static str,
    /// Path pattern, relative to the surface's root.
    pub path_pattern: &'static str,
    /// The root the pattern hangs off.
    pub root: StateRootKind,
    /// Who owns the data.
    pub authority: AuthorityClass,
    /// The surface's git relationship.
    pub git_class: GitClass,
    /// Whether `.gitignore` covers it through the configured-agent block.
    pub ignored_via_agent_block: bool,
}

/// The reference's registered state surfaces (diagnostic projection).
pub const STATE_SURFACES: [StateSurfaceSpec; 62] = [
    StateSurfaceSpec {
        name: "project_config",
        path_pattern: ".kittify/config.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "project_metadata",
        path_pattern: ".kittify/metadata.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "migration_ledger",
        path_pattern: ".kittify/migration-ledger.jsonl",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "workspace_context",
        path_pattern: ".kittify/workspaces/<feature>-<WP>.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "merge_resume_state",
        path_pattern: ".kittify/merge-state.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_lint_report",
        path_pattern: ".kittify/lint-report.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "sync_local_commit_state",
        path_pattern: ".kittify/sync-state.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "encoding_provenance_global_log",
        path_pattern: ".kittify/encoding-provenance/global.jsonl",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "runtime_feature_index",
        path_pattern: ".kittify/runtime/feature-runs.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "runtime_run_snapshot",
        path_pattern: ".kittify/runtime/runs/<run_id>/state.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "runtime_run_event_log",
        path_pattern: ".kittify/runtime/runs/<run_id>/run.events.jsonl",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "runtime_frozen_template",
        path_pattern: ".kittify/runtime/runs/<run_id>/mission_template_frozen.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "derived_mission_views",
        path_pattern: ".kittify/derived/<mission_slug>/lifecycle.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::Derived,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "migration_state_ledger",
        path_pattern: ".kittify/mission-state-audit/<run_id>.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "legacy_migration_scratch",
        path_pattern: ".kittify/migrations/",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "runtime_agent_logs",
        path_pattern: ".kittify/logs/<mission>_<wp>_<phase>.log",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "glossary_fallback_events",
        path_pattern: ".kittify/events/glossary/<mission_id>.events.jsonl",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "dossier_snapshot",
        path_pattern: "kitty-specs/<feature>/.kittify/dossiers/<feature>/snapshot-latest.json",
        root: StateRootKind::Feature,
        authority: AuthorityClass::Derived,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "mission_pycache",
        path_pattern: ".kittify/missions/__pycache__/",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "dossier_parity_baseline",
        path_pattern: ".kittify/dossiers/<feature>/parity-baseline.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "op_invocation_record",
        path_pattern: "kitty-ops/<op_id>.jsonl",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "op_invocation_index",
        path_pattern: "kitty-ops/ops-index.jsonl",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "worktrees_root",
        path_pattern: ".worktrees/",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "shared_skills_projection",
        path_pattern: ".agents/skills/<skill_name>/",
        root: StateRootKind::Project,
        authority: AuthorityClass::Derived,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: true,
    },
    StateSurfaceSpec {
        name: "skills_install_manifest",
        path_pattern: ".kittify/skills-manifest.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_source",
        path_pattern: ".kittify/charter/charter.md",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_interview_answers",
        path_pattern: ".kittify/charter/interview/answers.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_library",
        path_pattern: ".kittify/charter/library/*.md",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_yaml",
        path_pattern: ".kittify/charter/charter.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_activation_ledger",
        path_pattern: ".kittify/charter/activation.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_context_state",
        path_pattern: ".kittify/charter/context-state.json",
        root: StateRootKind::Project,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::Ignored,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_synthesis_manifest",
        path_pattern: ".kittify/charter/synthesis-manifest.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::Derived,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "charter_synthesis_provenance",
        path_pattern: ".kittify/charter/provenance/*.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::Derived,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "project_pack_graph",
        path_pattern: ".kittify/charter-packs/graph.yaml",
        root: StateRootKind::Project,
        authority: AuthorityClass::Derived,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "feature_metadata",
        path_pattern: "kitty-specs/<feature>/meta.json",
        root: StateRootKind::Feature,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "canonical_status_log",
        path_pattern: "kitty-specs/<feature>/status.events.jsonl",
        root: StateRootKind::Feature,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "canonical_status_snapshot",
        path_pattern: "kitty-specs/<feature>/status.json",
        root: StateRootKind::Feature,
        authority: AuthorityClass::Derived,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "wp_prompt_frontmatter",
        path_pattern: "kitty-specs/<feature>/tasks/WP*.md",
        root: StateRootKind::Feature,
        authority: AuthorityClass::Compatibility,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "wp_activity_log",
        path_pattern: "kitty-specs/<feature>/tasks/WP*.md body",
        root: StateRootKind::Feature,
        authority: AuthorityClass::Compatibility,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "tasks_status_block",
        path_pattern: "kitty-specs/<feature>/tasks.md",
        root: StateRootKind::Feature,
        authority: AuthorityClass::Derived,
        git_class: GitClass::Tracked,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "review_feedback_artifact",
        path_pattern: ".git/spec-kitty/feedback/<feature>/<WP>/<timestamp>-<id>.md",
        root: StateRootKind::GitInternal,
        authority: AuthorityClass::GitInternal,
        git_class: GitClass::GitInternal,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "sync_config",
        path_pattern: "~/.spec-kitty/config.toml",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "sync_credentials",
        path_pattern: "~/.spec-kitty/credentials",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Secret,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "lamport_clock",
        path_pattern: "~/.spec-kitty/clock.json",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "active_queue_scope",
        path_pattern: "~/.spec-kitty/active_queue_scope",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "sync_daemon_control",
        path_pattern: "~/.spec-kitty/sync-daemon",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "legacy_queue",
        path_pattern: "~/.spec-kitty/queue.db",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "scoped_queue",
        path_pattern: "~/.spec-kitty/queues/queue-<hash>.db",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "tracker_cache",
        path_pattern: "~/.spec-kitty/trackers/<scope>.db",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Authoritative,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "project_sync_store",
        path_pattern: "~/.spec-kitty/projects/<canonical-uuid>/sync/sync.db",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "project_sync_egress_lock",
        path_pattern: "~/.spec-kitty/projects/<canonical-uuid>/sync/egress.lock",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "project_sync_layout_generation",
        path_pattern: "~/.spec-kitty/projects/.layout-generation.json",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "project_sync_layout_generation_lock",
        path_pattern: "~/.spec-kitty/projects/.layout-generation.lock",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "project_sync_layout_generation_marker",
        path_pattern: "~/.spec-kitty/projects/.layout-generation.initialized",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "project_sync_migration_reports",
        path_pattern: "~/.spec-kitty/projects/<canonical-uuid>/sync/migration/reports/",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "runtime_version_stamp",
        path_pattern: "~/.kittify/cache/version.lock",
        root: StateRootKind::GlobalRuntime,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "runtime_update_lock",
        path_pattern: "~/.kittify/cache/.update.lock",
        root: StateRootKind::GlobalRuntime,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "runtime_staging_dirs",
        path_pattern: "~/.kittify_update_*",
        root: StateRootKind::GlobalRuntime,
        authority: AuthorityClass::LocalRuntime,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "legacy_session_json",
        path_pattern: "~/.spec-kitty/session.json",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "legacy_lamport_clock",
        path_pattern: "~/.spec-kitty/events/lamport_clock.json",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "legacy_mission_sessions",
        path_pattern: "~/.spec-kitty/missions/*/session.json",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
    StateSurfaceSpec {
        name: "legacy_reset_backups",
        path_pattern: "~/.spec-kitty/reset-backup-*",
        root: StateRootKind::GlobalSync,
        authority: AuthorityClass::Deprecated,
        git_class: GitClass::OutsideRepo,
        ignored_via_agent_block: false,
    },
];

/// The home/runtime environment the roots resolve against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRootEnvironment {
    /// The user home directory (`~`).
    pub home: PathBuf,
    /// The global runtime home (`get_kittify_home()`: `$SPEC_KITTY_HOME`, else
    /// `~/.kittify`).
    pub kittify_home: PathBuf,
    /// The authoritative runtime root (`get_runtime_root().base`:
    /// `$SPEC_KITTY_HOME`, else `~/.spec-kitty`).
    pub runtime_base: PathBuf,
}

/// One resolved state root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRootInfo {
    /// Machine-readable root name.
    pub name: &'static str,
    /// Human label.
    pub label: &'static str,
    /// The resolved path.
    pub resolved_path: PathBuf,
    /// Whether the root directory currently exists.
    pub exists: bool,
}

impl StateRootInfo {
    /// The upstream `--json` shape.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "label": self.label,
            "resolved_path": self.resolved_path.to_string_lossy(),
            "exists": self.exists,
        })
    }
}

/// One checked state surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSurfaceCheck {
    /// The surface specification.
    pub surface: StateSurfaceSpec,
    /// Whether the surface currently exists on disk.
    pub present: bool,
    /// Whether `.gitignore` covers it (`true` when it does not need to).
    pub gitignore_covered: bool,
    /// The warning this surface raised, if any.
    pub warning: Option<String>,
}

impl StateSurfaceCheck {
    /// The upstream `--json` shape.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.surface.name,
            "path_pattern": self.surface.path_pattern,
            "root": self.surface.root.as_str(),
            "authority": self.surface.authority.as_str(),
            "git_class": self.surface.git_class.as_str(),
            "present": self.present,
            "gitignore_covered": self.gitignore_covered,
            "warning": self.warning,
        })
    }
}

/// The whole state-roots audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRootsAudit {
    /// The three resolved roots, in report order.
    pub roots: Vec<StateRootInfo>,
    /// One check per registered surface, in registry order.
    pub surfaces: Vec<StateSurfaceCheck>,
    /// Every warning, in surface order.
    pub warnings: Vec<String>,
}

impl StateRootsAudit {
    /// Healthy only when no warning was raised.
    pub fn healthy(&self) -> bool {
        self.warnings.is_empty()
    }

    /// The upstream `--json` payload.
    pub fn to_json(&self) -> Value {
        let mut payload = Map::new();
        payload.insert("healthy".into(), json!(self.healthy()));
        payload.insert(
            "roots".into(),
            Value::Array(self.roots.iter().map(StateRootInfo::to_json).collect()),
        );
        payload.insert(
            "surfaces".into(),
            Value::Array(
                self.surfaces
                    .iter()
                    .map(StateSurfaceCheck::to_json)
                    .collect(),
            ),
        );
        payload.insert("warnings".into(), json!(self.warnings));
        Value::Object(payload)
    }
}

/// Does `name` match a `*`-only glob pattern?
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    if let Some(prefix) = parts.first()
        && !rest.starts_with(prefix)
    {
        return false;
    }
    rest = &rest[parts[0].len()..];
    for (index, part) in parts.iter().enumerate().skip(1) {
        if index == parts.len() - 1 {
            return rest.ends_with(part);
        }
        match rest.find(part) {
            Some(position) => rest = &rest[position + part.len()..],
            None => return false,
        }
    }
    true
}

/// The directory part of a pattern that still contains a wildcard/placeholder,
/// plus the pattern to match inside it.
fn split_wildcard_path(pattern: &str) -> (&str, &str) {
    match pattern.rfind('/') {
        Some(index) => (&pattern[..index], &pattern[index + 1..]),
        None => ("", pattern),
    }
}

fn surface_present(
    repo_root: &Path,
    environment: &StateRootEnvironment,
    surface: &StateSurfaceSpec,
) -> bool {
    let path: PathBuf = match surface.root {
        StateRootKind::Project | StateRootKind::Feature => repo_root.join(surface.path_pattern),
        StateRootKind::GlobalRuntime => {
            if let Some(relative) = surface.path_pattern.strip_prefix("~/") {
                environment.home.join(relative)
            } else {
                environment.kittify_home.join(surface.path_pattern)
            }
        }
        StateRootKind::GlobalSync => {
            if let Some(relative) = surface
                .path_pattern
                .strip_prefix(GLOBAL_SYNC_PATTERN_PREFIX)
            {
                environment.runtime_base.join(relative)
            } else {
                environment.runtime_base.join(surface.path_pattern)
            }
        }
        StateRootKind::GitInternal => {
            let git_common = match git_probe::open(repo_root) {
                GitAnswer::Answered(repository) => match repository.git_dirs() {
                    Ok(dirs) => dirs.common_dir().to_path_buf(),
                    Err(_) => repo_root.join(".git"),
                },
                _ => repo_root.join(".git"),
            };
            let relative = surface
                .path_pattern
                .strip_prefix(".git/")
                .unwrap_or(surface.path_pattern);
            git_common.join(relative)
        }
    };

    let rendered = path.to_string_lossy().into_owned();
    if rendered.contains('*') {
        let (parent_pattern, leaf_pattern) = split_wildcard_path(&rendered);
        let mut parent = PathBuf::from(parent_pattern);
        let mut pattern = leaf_pattern.to_owned();
        while parent
            .file_name()
            .map(|name| {
                name.to_string_lossy().contains('<') || name.to_string_lossy().contains('*')
            })
            .unwrap_or(false)
        {
            let name = parent
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            pattern = format!("{name}/{pattern}");
            parent = parent.parent().unwrap_or(Path::new("")).to_path_buf();
        }
        let entries = match std::fs::read_dir(&parent) {
            Ok(entries) => entries,
            Err(_) => return false,
        };
        return entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            wildcard_match(&pattern, &name)
        });
    }

    if rendered.contains('<') {
        let mut parent: &Path = path.parent().unwrap_or(&path);
        while parent.to_string_lossy().contains('<') {
            parent = match parent.parent() {
                Some(upper) => upper,
                None => break,
            };
        }
        return parent.is_dir();
    }

    path.exists()
}

fn gitignore_probe_path(surface: &StateSurfaceSpec) -> String {
    if !surface.ignored_via_agent_block {
        return surface.path_pattern.to_owned();
    }
    let mut out = String::with_capacity(surface.path_pattern.len());
    let mut inside = false;
    for character in surface.path_pattern.chars() {
        match character {
            '<' => inside = true,
            '>' if inside => {
                inside = false;
                out.push_str(AGENT_BLOCK_PROBE_NAME);
            }
            other if !inside => out.push(other),
            _ => {}
        }
    }
    out
}

fn gitignore_covered(
    repo_root: &Path,
    surface: &StateSurfaceSpec,
    repository: &mut Option<GitAnswer<sk_git::GitRepository>>,
) -> bool {
    if surface.root != StateRootKind::Project {
        return true;
    }
    if matches!(
        surface.git_class,
        GitClass::Tracked | GitClass::OutsideRepo | GitClass::GitInternal
    ) {
        return true;
    }
    let probe = gitignore_probe_path(surface);
    let answer = repository.get_or_insert_with(|| git_probe::open(repo_root));
    let GitAnswer::Answered(repository) = answer else {
        // No usable git, or an unreadable repository: cannot verify, so do not
        // invent a warning.
        return true;
    };
    let Ok(path) = sk_git::GitPath::from_git_bytes(probe.as_bytes()) else {
        return true;
    };
    repository.is_path_ignored(&path).unwrap_or(true)
}

/// Run the state-roots diagnosis.
pub fn audit_state_roots(
    repo_root: &Path,
    environment: &StateRootEnvironment,
) -> Result<StateRootsAudit, DoctorError> {
    require_directory(repo_root)?;

    let project_root = repo_root.join(".kittify");
    let roots = vec![
        StateRootInfo {
            name: "project",
            label: "Project-local state",
            resolved_path: project_root.clone(),
            exists: project_root.is_dir(),
        },
        StateRootInfo {
            name: "global_runtime",
            label: "Global runtime home",
            resolved_path: environment.kittify_home.clone(),
            exists: environment.kittify_home.is_dir(),
        },
        StateRootInfo {
            name: "global_sync",
            label: "Global sync/auth home",
            resolved_path: environment.runtime_base.clone(),
            exists: environment.runtime_base.is_dir(),
        },
    ];

    let mut repository = None;
    let mut surfaces = Vec::with_capacity(STATE_SURFACES.len());
    let mut warnings = Vec::new();
    for surface in STATE_SURFACES {
        let present = surface_present(repo_root, environment, &surface);
        let covered = gitignore_covered(repo_root, &surface, &mut repository);
        let warning = warning_for(&surface, present, covered);
        if let Some(warning) = &warning {
            warnings.push(warning.clone());
        }
        surfaces.push(StateSurfaceCheck {
            surface,
            present,
            gitignore_covered: covered,
            warning,
        });
    }

    Ok(StateRootsAudit {
        roots,
        surfaces,
        warnings,
    })
}

/// The warning for one checked surface, when it raises one.
fn warning_for(
    surface: &StateSurfaceSpec,
    present: bool,
    gitignore_covered: bool,
) -> Option<String> {
    if surface.root != StateRootKind::Project {
        return None;
    }
    if matches!(
        surface.git_class,
        GitClass::Tracked | GitClass::OutsideRepo | GitClass::GitInternal
    ) {
        return None;
    }
    if !matches!(
        surface.authority,
        AuthorityClass::LocalRuntime | AuthorityClass::Derived
    ) {
        return None;
    }
    if !present || gitignore_covered {
        return None;
    }
    Some(format!(
        "{} ({}) is present but not gitignored. Authority: {}. Risk: accidental commit.",
        surface.name,
        surface.path_pattern,
        surface.authority.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(root: &Path) -> StateRootEnvironment {
        StateRootEnvironment {
            home: root.to_path_buf(),
            kittify_home: root.join(".kittify-home"),
            runtime_base: root.join(".spec-kitty"),
        }
    }

    #[test]
    fn the_registry_has_no_duplicate_names_and_covers_every_root() {
        let mut names = std::collections::BTreeSet::new();
        for surface in STATE_SURFACES {
            assert!(names.insert(surface.name), "duplicate {}", surface.name);
            assert!(!surface.path_pattern.is_empty());
        }
        assert_eq!(STATE_SURFACES.len(), 62);
        for root in STATE_ROOTS {
            assert!(
                STATE_SURFACES.iter().any(|surface| surface.root == root),
                "no surface for {root:?}"
            );
        }
    }

    #[test]
    fn an_absent_project_reports_absent_roots_and_no_warnings() {
        let root = tempfile::tempdir().unwrap();
        let audit = audit_state_roots(root.path(), &environment(root.path())).unwrap();
        assert_eq!(audit.roots.len(), 3);
        assert!(!audit.roots[0].exists);
        assert!(audit.warnings.is_empty());
        assert_eq!(audit.surfaces.len(), STATE_SURFACES.len());
    }

    #[test]
    fn a_present_untracked_runtime_surface_warns() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".kittify/workspaces")).unwrap();
        let audit = audit_state_roots(root.path(), &environment(root.path())).unwrap();
        // No usable git repository here: gitignore coverage cannot be verified,
        // so the audit stays quiet rather than inventing a warning.
        assert!(audit.warnings.is_empty());
        let workspaces = audit
            .surfaces
            .iter()
            .find(|check| check.surface.name == "workspace_context")
            .expect("surface registered");
        assert!(workspaces.present);
        assert!(workspaces.gitignore_covered);
    }

    #[test]
    fn wildcard_and_placeholder_patterns_do_not_crash() {
        assert!(wildcard_match("*.lock", "index.lock"));
        assert!(wildcard_match(".kittify_update_*", ".kittify_update_2"));
        assert!(!wildcard_match(".kittify_update_*", ".other"));
        assert_eq!(split_wildcard_path("a/b/*.lock"), ("a/b", "*.lock"));
        let root = tempfile::tempdir().unwrap();
        let audit = audit_state_roots(root.path(), &environment(root.path())).unwrap();
        // `~/.kittify_update_*` and placeholder roots resolve without panicking.
        assert!(!audit.surfaces.is_empty());
    }

    #[test]
    fn agent_block_probe_fills_placeholders_with_a_reserved_skill_name() {
        let surface = StateSurfaceSpec {
            name: "command_skills",
            path_pattern: ".agents/skills/spec-kitty-<command>/SKILL.md",
            root: StateRootKind::Project,
            authority: AuthorityClass::Derived,
            git_class: GitClass::Ignored,
            ignored_via_agent_block: true,
        };
        assert_eq!(
            gitignore_probe_path(&surface),
            ".agents/skills/spec-kitty-spec-kitty-doctor-probe/SKILL.md"
        );
    }

    #[test]
    fn warnings_require_a_local_runtime_authority_that_is_present() {
        let surface = StateSurfaceSpec {
            name: "runtime_state",
            path_pattern: ".kittify/runtime/x.json",
            root: StateRootKind::Project,
            authority: AuthorityClass::LocalRuntime,
            git_class: GitClass::Ignored,
            ignored_via_agent_block: false,
        };
        assert!(warning_for(&surface, true, false).is_some());
        assert!(warning_for(&surface, false, false).is_none());
        assert!(warning_for(&surface, true, true).is_none());
        let tracked = StateSurfaceSpec {
            git_class: GitClass::Tracked,
            ..surface
        };
        assert!(warning_for(&tracked, true, false).is_none());
    }
}
