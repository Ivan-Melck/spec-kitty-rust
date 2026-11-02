//! Foundation primitives shared by the Rust Spec Kitty implementation.
//!
//! This crate is the kernel floor of the Rust workspace: it owns validated
//! identifier types, bounded reads, project-root discovery, contained path
//! resolution, and no-op-stable atomic writes. It deliberately has no
//! dependency on any other Spec Kitty crate.

mod atomic;
mod bounded;
mod containment;
mod discovery;
mod error;
mod fs_resolve;
mod identifiers;

pub use atomic::atomic_write;
pub use bounded::read_bounded;
pub use containment::resolve_within;
pub use discovery::discover_project_root;
pub use error::KernelError;
pub use identifiers::{MissionSlug, WpId};
