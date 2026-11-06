//! Foundation primitives shared by the Rust Spec Kitty implementation.
//!
//! This crate is the kernel floor of the Rust workspace: it owns validated
//! identifier types, bounded and guarded reads, project-root discovery,
//! contained path resolution, no-op-stable atomic writes, the canonical clock
//! and digest helpers, the cross-process file lock, the locked event-log
//! append protocol, and the canonical Python `str` code-point plus `repr`
//! primitives. It deliberately has no dependency on any other Spec Kitty
//! crate and forbids `unsafe` through the workspace lint.

// Generated-data tests keep their original cross-crate import without changing
// any verified numeric facts or exporting a test-only public surface.
#[cfg(test)]
extern crate self as sk_kernel;

/// Pinned Source compatibility target for CLI and authored artifact protocols.
/// This is not the Native Cargo package version or a completion claim.
pub const COMPAT_CORE_VERSION: &str = "4.0.0rc8";

mod atomic;
mod bounded;
pub mod clock;
mod codepoint;
mod containment;
pub mod content_digest;
mod discovery;
pub mod encoding_provenance;
mod error;
pub mod event_log_append;
mod fs_resolve;
pub mod git_topology;
pub mod guarded_read;
mod identifiers;
pub mod lock_families;
pub mod locks;
pub mod model_json;
pub mod no_follow;
pub mod os_locale;
pub mod pack_tiers;
pub mod paths;
pub mod python_console;
mod python_decimal_data;
mod python_digit_data;
pub mod python_int;
pub mod python_json;
pub mod python_logging;
pub mod python_os_encoding;
mod python_regex_case_data;
pub mod python_repr;
pub mod python_stream;
pub mod python_text;
mod python_text_data;
mod python_text_upper_data;
mod python_unicode;
pub mod python_utf8;
pub mod python_value;
pub mod python_yaml;
pub mod reference_path;
// Source writer families are explicit capabilities, distinct from Native stable writes.
pub mod source_glob;
pub mod source_writer;
pub mod text_decode;
pub mod user_dirs;

#[cfg(unix)]
pub use atomic::atomic_write_with_requested_mode;
pub use atomic::{
    AtomicWriteOptions, atomic_write, atomic_write_with_options, atomic_write_with_permissions,
    substantively_equal, substantively_equal_projected,
};
pub use bounded::{read_bounded, read_source_bytes};
pub use codepoint::{InvalidCodepoint, PythonString};
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
    source_name_before_first_dot, source_path_component_key, source_path_exists,
    source_path_is_dir, source_path_text, source_pure_path_name, source_pure_path_stem,
    source_pure_path_stem_from_name, to_posix,
};
pub use python_repr::{python_repr, python_repr_codepoints};
pub use python_value::{OrderedMap, PythonValue};
pub use reference_path::{ReferencePathError, resolve_reference_path};
pub use source_glob::{SourceGlobError, source_rglob_yaml};
pub use source_writer::write_observer::{
    BoundWriteContext, ObserverCallback, WriteContext, WriteContextError, WriteContextGuard,
    WriteObserverError, WriteObserverToken, bind_write_context, notify_written, observe_writes,
    observe_writes_callback, observing_writes, stop_observing_writes,
};
pub use source_writer::{
    AtomicWrite, GeneratedWrite, SourcePayload, SourceWriteError, WriteListener,
    WriteListenerFailure, WriteObservation, WriteReceipt, WriterOperation,
    derived_json_temporary_path, generated_temporary_path, rebuild_log_temporary_path,
    write_source,
};
pub use user_dirs::{user_cache_dir, user_state_dir};
