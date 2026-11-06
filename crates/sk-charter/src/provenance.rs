//! Portable provenance-path normalisation for committed charter state.
//!
//! Faithful port of `src/charter/offering/provenance.py`: ONE three-class
//! normalizer shared by both provenance carriers — the charter catalog
//! (`charter.yaml` `catalog.references[].source_path`) and the agent-profile
//! projection manifest (`agent_profiles_manifest.json` `entries[].source_path`)
//! — so a committed file never bakes in an operator- or platform-specific
//! absolute filesystem path for a built-in-pack source.
//!
//! Three classes, checked in this priority order:
//!
//! 1. **Built-in pack path** — anything resolving under the built-in pack root
//!    becomes `${SPEC_KITTY_PACKS_ROOT}/built-in/<rest>`. The token is the
//!    stored artifact; it is never re-baked to an absolute path even when
//!    `SPEC_KITTY_PACKS_ROOT` is set in the emitting process (this module only
//!    ever composes the literal token string, it never expands one).
//! 2. **In-tree project/org path** — anything under the caller-supplied project
//!    root becomes a repo-relative POSIX string.
//! 3. **Out-of-tree, non-pack path** — the resolved absolute path is preserved
//!    unchanged (there is no portable form for it).
//!
//! The built-in root itself is the one kernel-floor resolver
//! ([`sk_kernel::paths::get_built_in_pack_root`]); this module never reads the
//! environment variable separately.

use std::path::{Path, PathBuf};

use sk_kernel::paths::{get_built_in_pack_root, resolve_path_non_strict, to_posix};

pub mod sidecar;

/// The literal token prefix every built-in-pack path normalizes to.
pub const PACKS_ROOT_TOKEN: &str = "${SPEC_KITTY_PACKS_ROOT}";

/// The fixed `built-in` path segment inside the token.
pub const BUILT_IN_SEGMENT: &str = "built-in";

/// The full `${SPEC_KITTY_PACKS_ROOT}/built-in` prefix.
pub fn built_in_token_prefix() -> String {
    format!("{PACKS_ROOT_TOKEN}/{BUILT_IN_SEGMENT}")
}

/// Resolve the built-in pack root, tolerating an unavailable install.
///
/// An offering-side caller (a fixture with no packaged `packs/built-in` sibling
/// reachable, for example) must not hard-fail the normalizer: classes (2) and
/// (3) still need to work when the built-in tree genuinely cannot be located.
fn resolve_built_in_root() -> Option<PathBuf> {
    let root = get_built_in_pack_root().ok()?;
    resolve_path_non_strict(&root).ok()
}

/// The built-in root for a call: the explicit override, else the kernel resolver.
fn built_in_root(explicit: Option<&Path>) -> Option<PathBuf> {
    match explicit {
        Some(root) => resolve_path_non_strict(root).ok(),
        None => resolve_built_in_root(),
    }
}

fn relative_to_built_in(resolved: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    let root = built_in_root(explicit)?;
    resolved.strip_prefix(&root).ok().map(Path::to_path_buf)
}

/// Whether `path` resolves under the built-in pack root.
///
/// Shared classification predicate: the provenance-heal migration and the
/// `doctor provenance` leak check both answer "is this already-committed
/// absolute path a healable built-in-pack path" without re-deriving
/// [`to_portable_source_path`]'s own class-(1) branch.
pub fn is_built_in_pack_path(path: &Path) -> bool {
    is_built_in_pack_path_with(path, None)
}

/// [`is_built_in_pack_path`] with an explicit built-in root.
///
/// The override exists for callers that already resolved the tier (a probe with
/// a recorded pack tree, an org pack positioned inside a checkout) so they do
/// not have to mutate the process environment to be classified correctly.
pub fn is_built_in_pack_path_with(path: &Path, built_in: Option<&Path>) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    match resolve_path_non_strict(path) {
        Ok(resolved) => relative_to_built_in(&resolved, built_in).is_some(),
        Err(_) => false,
    }
}

/// The portable string form of `path` for committed provenance.
///
/// * `path` empty → `""` (mirrors the pre-existing `_trim_source_path`
///   empty-input contract).
/// * A path under the built-in pack root → `${SPEC_KITTY_PACKS_ROOT}/built-in/<rest>`.
/// * A path under `project_root` → repo-relative POSIX (class 2).
/// * Anything else → the resolved absolute path (class 3).
///
/// `project_root: None` disables class (2) entirely, so such a path falls
/// through to class (3).
pub fn to_portable_source_path(path: &Path, project_root: Option<&Path>) -> String {
    to_portable_source_path_with(path, project_root, None)
}

/// [`to_portable_source_path`] with an explicit built-in root.
pub fn to_portable_source_path_with(
    path: &Path,
    project_root: Option<&Path>,
    built_in: Option<&Path>,
) -> String {
    if path.as_os_str().is_empty() {
        return String::new();
    }
    let Ok(resolved) = resolve_path_non_strict(path) else {
        // An unresolvable path has no portable form; keep it as authored.
        return to_posix(path);
    };

    if let Some(rest) = relative_to_built_in(&resolved, built_in) {
        let rest_posix = if rest.as_os_str().is_empty() || rest == Path::new(".") {
            String::new()
        } else {
            to_posix(&rest)
        };
        let prefix = built_in_token_prefix();
        return if rest_posix.is_empty() {
            prefix
        } else {
            format!("{prefix}/{rest_posix}")
        };
    }

    if let Some(root) = project_root
        && let Ok(root) = resolve_path_non_strict(root)
        && let Ok(relative) = resolved.strip_prefix(&root)
    {
        return to_posix(relative);
    }

    to_posix(&resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_path_stays_empty() {
        assert_eq!(to_portable_source_path(Path::new(""), None), "");
        assert!(!is_built_in_pack_path(Path::new("")));
    }

    #[test]
    fn a_path_under_the_project_is_repo_relative() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        std::fs::create_dir_all(project.join("directives")).unwrap();
        let source = project.join("directives/one.directive.yaml");
        std::fs::write(&source, "id: ONE\n").unwrap();
        assert_eq!(
            to_portable_source_path(&source, Some(&project)),
            "directives/one.directive.yaml"
        );
    }

    #[test]
    fn an_out_of_tree_path_keeps_its_resolved_absolute_form() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let outside = directory.path().join("outside/value.yaml");
        assert_eq!(
            to_portable_source_path(&outside, Some(&project)),
            to_posix(&resolve_path_non_strict(&outside).unwrap())
        );
    }
}
