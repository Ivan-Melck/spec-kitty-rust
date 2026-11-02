//! Read-only Mission inventory, work-package dependency planning, and
//! dependency readiness for the Rust Spec Kitty implementation.
//!
//! # Scope and boundaries
//!
//! This crate inspects on-disk planning artifacts; it never mutates them.
//! [`list_missions`] reads `kitty-specs/*/meta.json` with a bounded kernel
//! read, preserving every field (including extension fields) exactly as
//! decoded. [`WorkPackage`] parses the YAML frontmatter of `WP*.md` prompt
//! files without rewriting the Markdown. [`WorkPackageGraph`] validates the
//! dependency DAG (unknown ids, self-dependencies, duplicates, cycles) and
//! produces a deterministic topological order.
//!
//! Authoritative work-package *state* is **not** reconstructed here: there is
//! no event-log replay, no `status.json` reading, and no use of the retired
//! frontmatter `lane`/`history` values. [`dependency_readiness`] evaluates only
//! an explicit [`WpStateMap`] supplied by the caller, and distinguishes a
//! dependency with no entry ([`UnsatisfiedReason::MissingState`]) from one in a
//! non-acceptable lane.
//!
//! Mission rooms, worktrees, lane placement, ownership, git, and status writes
//! are out of scope. This crate makes no placement or ownership claim and
//! never edits `meta.json`.
//!
//! # Selection
//!
//! [`select_mission`] accepts a full canonical mission id (ULID), an 8-character
//! `mid8`, or an exact slug. It has no numeric-prefix, slug-tail, or sole-mission
//! fallback, and refuses an ambiguous match rather than guessing.
//!
//! # Limits
//!
//! `meta.json` reads default to [`meta::DEFAULT_META_BYTE_LIMIT`] and
//! work-package reads to [`work_package::DEFAULT_WORK_PACKAGE_BYTE_LIMIT`].
//! Corrupt, oversized, symlinked, or duplicate-identity input fails closed.

mod error;
mod frontmatter;
mod graph;
mod identity;
mod meta;
mod mission;
mod readiness;
mod topology;
mod work_package;

pub use error::MissionError;
pub use frontmatter::{Frontmatter, FrontmatterError, split_frontmatter};
pub use graph::WorkPackageGraph;
pub use identity::{MID8_LENGTH, MISSION_ID_LENGTH, MissionId, MissionIdError};
pub use meta::{DEFAULT_META_BYTE_LIMIT, META_FILE_NAME, SPECS_DIR_NAME};
pub use mission::{Mission, list_missions, select_mission};
pub use readiness::{
    DependencyReadiness, UnsatisfiedDependency, UnsatisfiedReason, WpState, WpStateMap,
    dependency_readiness, readiness_for_dependencies, ready_work_packages,
};
pub use topology::{MissionTopology, classify_topology, routes_through_coordination};
pub use work_package::{DEFAULT_WORK_PACKAGE_BYTE_LIMIT, WorkPackage};

// Re-exported so callers can build selectors and state maps without depending
// on the kernel crate directly.
pub use sk_kernel::{MissionSlug, WpId};
