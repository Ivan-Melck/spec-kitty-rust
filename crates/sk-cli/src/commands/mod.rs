//! One module per read-only command family.

pub mod capabilities;
pub mod charter;
pub mod events;
pub mod git;
pub mod glossary;
pub mod mission;
pub mod project;
pub mod runtime;
pub mod transition;

use std::path::Path;

/// Render a path for JSON. Paths that are not UTF-8 are preserved lossily;
/// diagnostics are not a byte-transparent transport.
pub(crate) fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
