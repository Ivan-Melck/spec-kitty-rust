//! Foundation primitives shared by the Rust Spec Kitty implementation.
//!
//! This crate is the kernel floor of the Rust workspace: it owns validated
//! identifier types, bounded and guarded reads, project-root discovery,
//! contained path resolution, no-op-stable atomic writes, the canonical clock
//! and digest helpers, the cross-process file lock, and the locked event-log
//! append protocol. It deliberately has no dependency on any other Spec Kitty
//! crate and forbids `unsafe` through the workspace lint.

mod atomic;
mod bounded;
pub mod clock;
mod containment;
pub mod content_digest;
mod discovery;
mod error;
pub mod event_log_append;
mod fs_resolve;
pub mod git_topology;
pub mod guarded_read;
mod identifiers;
pub mod lock_families;
pub mod locks;
pub mod no_follow;
pub mod pack_tiers;
pub mod paths;
pub mod reference_path;
pub mod user_dirs;

pub use atomic::{
    AtomicWriteOptions, atomic_write, atomic_write_with_options, substantively_equal,
    substantively_equal_projected,
};
pub use bounded::read_bounded;
pub use containment::resolve_within;
pub use discovery::discover_project_root;
pub use error::KernelError;
pub use identifiers::{MissionSlug, WpId};
pub use lock_families::{
    StatusLock, StatusLockHolder, acquire_checkout_claim_lock, acquire_feature_status_lock,
    acquire_project_event_log_lock, checkout_claim_lock_path, feature_status_lock_path,
    project_event_log_lock_path, status_lock_common_dir,
};
pub use locks::{
    AnchorDigestAlgorithm, AnchoredLockName, FileLock, LockHolder, LockOptions, LockWait,
    anchored_lock_path, anchored_lock_path_with,
};
pub use pack_tiers::{BUILT_IN, ORG, PACK_TIERS, PROJECT, pin_filename};
pub use paths::{
    BuiltInPackRoot, PACKS_ROOT_ENV, PackRootSource, TEMPLATE_ROOT_ENV, ensure_runtime_state_root,
    get_built_in_pack_root, get_kittify_home, get_package_asset_root, get_packs_root_default,
    get_runtime_state_root, home_directory, posix_tree_path, render_runtime_path, repo_tree_path,
    resolve_built_in_pack_root, resolve_built_in_pack_root_from, resolve_installed_sibling,
    to_posix,
};
pub use reference_path::{ReferencePathError, resolve_reference_path};
pub use user_dirs::{user_cache_dir, user_state_dir};
