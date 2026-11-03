//! Canonical home, runtime-state, pack-root and mission-asset paths.
//!
//! These are the kernel-floor resolvers upstream keeps in `kernel.paths` and
//! `kernel.sibling_paths`. Layers above (`charter`, `specify_cli`) consume
//! them instead of forking a second rule.
//!
//! All resolvers are **pure** unless their name says otherwise:
//! [`get_kittify_home`], [`get_runtime_state_root`], [`get_built_in_pack_root`],
//! and [`get_package_asset_root`] create no directories. Only
//! [`ensure_runtime_state_root`] does.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::KernelError;
use crate::fs_resolve::{map_resolve_failure, resolve_non_strict};
use crate::pack_tiers::BUILT_IN;

/// Resolve an absolute, symlink-aware path even when its trailing components
/// do not exist, matching `Path.resolve(strict=False)` in the reference.
/// Resolution failures remain typed; callers must not substitute another path.
pub fn resolve_path_non_strict(path: &Path) -> Result<PathBuf, KernelError> {
    resolve_non_strict(path).map_err(|failure| map_resolve_failure(path, failure))
}

/// Environment variable naming the user-global `.kittify` home (all platforms).
pub const KITTIFY_HOME_ENV: &str = "SPEC_KITTY_HOME";

/// Environment variable naming the pack root (default- or operator-supplied).
pub const PACKS_ROOT_ENV: &str = "SPEC_KITTY_PACKS_ROOT";

/// Environment variable naming a template/asset-copy root (CI/testing).
pub const TEMPLATE_ROOT_ENV: &str = "SPEC_KITTY_TEMPLATE_ROOT";

/// The built-in pack shape relative to a checkout or installation root.
pub const BUILT_IN_PACK_SIBLING_PATTERN: &str = "packs/built-in";

/// The built-in missions shape relative to a checkout or installation root.
pub const MISSION_ASSETS_SIBLING_PATTERN: &str = "packs/built-in/missions";

const MISSION_ASSETS_DIR_NAME: &str = "missions";
const SRC_LAYOUT_DIR_NAME: &str = "src";

/// Resolve the operating-system home directory.
///
/// `HOME` on Unix; `USERPROFILE` (then `HOMEDRIVE` + `HOMEPATH`) on Windows.
/// Python's `Path.home()` additionally falls back to the password database;
/// this safe, subprocess-free implementation reports
/// [`KernelError::HomeDirectoryUnavailable`] instead of guessing.
pub fn home_directory() -> Result<PathBuf, KernelError> {
    #[cfg(windows)]
    {
        if let Some(profile) = non_empty("USERPROFILE") {
            return Ok(PathBuf::from(profile));
        }
        let drive = non_empty("HOMEDRIVE");
        let path = non_empty("HOMEPATH");
        if let (Some(drive), Some(path)) = (drive, path) {
            let mut home = PathBuf::from(drive);
            home.push(path);
            return Ok(home);
        }
        Err(KernelError::HomeDirectoryUnavailable)
    }
    #[cfg(not(windows))]
    {
        non_empty("HOME")
            .map(PathBuf::from)
            .ok_or(KernelError::HomeDirectoryUnavailable)
    }
}

/// Return the user-global `~/.kittify` (asset) home.
///
/// Resolution order:
/// 1. `SPEC_KITTY_HOME` verbatim (all platforms); an empty value falls through.
/// 2. Windows: `%LOCALAPPDATA%\spec-kitty`, else `~/.spec-kitty`.
/// 3. POSIX: `~/.kittify`.
///
/// The Windows app name is `spec-kitty` (not `kittify`) so this agrees with the
/// runtime state root.
pub fn get_kittify_home() -> Result<PathBuf, KernelError> {
    if let Some(home) = non_empty(KITTIFY_HOME_ENV) {
        return Ok(PathBuf::from(home));
    }
    if cfg!(windows) {
        if let Some(local_app_data) = non_empty("LOCALAPPDATA") {
            return Ok(PathBuf::from(local_app_data).join("spec-kitty"));
        }
        return Ok(home_directory()?.join(".spec-kitty"));
    }
    Ok(home_directory()?.join(".kittify"))
}

/// Return the runtime **state** root, deliberately distinct from
/// [`get_kittify_home`] (the `.kittify` asset home).
///
/// 1. `SPEC_KITTY_HOME` verbatim.
/// 2. Windows: `%LOCALAPPDATA%\spec-kitty`, else `~/.spec-kitty`.
/// 3. POSIX: `~/.spec-kitty`.
pub fn get_runtime_state_root() -> Result<PathBuf, KernelError> {
    if let Some(home) = non_empty(KITTIFY_HOME_ENV) {
        return Ok(PathBuf::from(home));
    }
    if cfg!(windows) {
        if let Some(local_app_data) = non_empty("LOCALAPPDATA") {
            return Ok(PathBuf::from(local_app_data).join("spec-kitty"));
        }
        return Ok(home_directory()?.join(".spec-kitty"));
    }
    Ok(home_directory()?.join(".spec-kitty"))
}

/// Create (or re-harden) the runtime state root at `0o700` and return it.
///
/// Idempotent: an existing directory is re-chmodded rather than trusted at an
/// ambient umask (`0755`).
pub fn ensure_runtime_state_root() -> Result<PathBuf, KernelError> {
    let root = get_runtime_state_root()?;
    fs::create_dir_all(&root)
        .map_err(|source| KernelError::io("create runtime state root", &root, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&root, fs::Permissions::from_mode(0o700));
    }
    Ok(root)
}

/// Where a resolved built-in pack root came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackRootSource {
    /// `SPEC_KITTY_PACKS_ROOT` named an existing `<value>/built-in` directory.
    EnvironmentOverride,
    /// The bounded ancestor walk found the installed/checkout sibling.
    InstalledSibling,
}

/// A resolved built-in pack root plus any override that had to be ignored.
///
/// Upstream warns loudly when `SPEC_KITTY_PACKS_ROOT` is set but resolves to no
/// `built-in` child, then falls through to the installed tree. The kernel emits
/// no log lines itself, so the ignored override is returned here for the caller
/// to surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltInPackRoot {
    /// The resolved `packs/built-in` directory.
    pub path: PathBuf,
    /// Which rule produced it.
    pub source: PackRootSource,
    /// The candidate `<SPEC_KITTY_PACKS_ROOT>/built-in` that did not exist.
    pub ignored_override: Option<PathBuf>,
}

/// Resolve the built-in pack root from an explicit anchor file.
///
/// `anchor` should be the calling executable or module path; the bounded walk
/// looks for `packs/built-in` at each ancestor (see
/// [`resolve_installed_sibling`]). This explicit form is what packaged
/// consumers should use.
pub fn resolve_built_in_pack_root_from(anchor: &Path) -> Result<BuiltInPackRoot, KernelError> {
    let mut ignored_override = None;
    if let Some(packs_root) = non_empty(PACKS_ROOT_ENV) {
        let candidate = PathBuf::from(packs_root).join(BUILT_IN);
        if candidate.is_dir() {
            return Ok(BuiltInPackRoot {
                path: candidate,
                source: PackRootSource::EnvironmentOverride,
                ignored_override: None,
            });
        }
        ignored_override = Some(candidate);
    }
    let path = resolve_installed_sibling(anchor, None, Path::new(BUILT_IN_PACK_SIBLING_PATTERN))?;
    Ok(BuiltInPackRoot {
        path,
        source: PackRootSource::InstalledSibling,
        ignored_override,
    })
}

/// Resolve the built-in pack root using the workspace checkout as the anchor,
/// falling back to the running executable.
pub fn resolve_built_in_pack_root() -> Result<BuiltInPackRoot, KernelError> {
    let mut last_error = None;
    for anchor in default_pack_anchors() {
        match resolve_built_in_pack_root_from(&anchor) {
            Ok(root) => return Ok(root),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| KernelError::PackRootNotFound {
        what: "built-in pack root".to_owned(),
        reason: "no anchor path was available".to_owned(),
    }))
}

/// Resolve the built-in pack root, discarding diagnostics.
pub fn get_built_in_pack_root() -> Result<PathBuf, KernelError> {
    resolve_built_in_pack_root().map(|resolved| resolved.path)
}

/// Return the parent of the built-in pack root (the `${SPEC_KITTY_PACKS_ROOT}`
/// default token).
pub fn get_packs_root_default() -> Result<PathBuf, KernelError> {
    let root = get_built_in_pack_root()?;
    root.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| KernelError::PackRootNotFound {
            what: PACKS_ROOT_ENV.to_owned(),
            reason: "the built-in pack root has no parent".to_owned(),
        })
}

/// Resolve the shipped `missions` asset directory.
///
/// Precedence mirrors upstream: `SPEC_KITTY_PACKS_ROOT` governs location when
/// set; otherwise `SPEC_KITTY_TEMPLATE_ROOT` (CI/testing) is accepted in its
/// several legacy shapes; otherwise the installed `packs/built-in/missions`
/// sibling is used. Any failure is fail-closed; there is no legacy
/// `specify_cli/missions` fall-through.
pub fn get_package_asset_root() -> Result<PathBuf, KernelError> {
    let packs_root_set = non_empty(PACKS_ROOT_ENV).is_some();
    if !packs_root_set && let Some(template_root) = non_empty(TEMPLATE_ROOT_ENV) {
        return resolve_template_root(&PathBuf::from(template_root));
    }

    let pack_root = get_built_in_pack_root()?;
    let missions = pack_root.join(MISSION_ASSETS_DIR_NAME);
    if missions.is_dir() {
        Ok(missions)
    } else {
        Err(KernelError::PackRootNotFound {
            what: "package mission assets".to_owned(),
            reason: format!(
                "built-in pack root {} has no {MISSION_ASSETS_DIR_NAME:?} directory",
                pack_root.display()
            ),
        })
    }
}

fn resolve_template_root(root: &Path) -> Result<PathBuf, KernelError> {
    if !root.is_dir() {
        return Err(KernelError::PackRootNotFound {
            what: TEMPLATE_ROOT_ENV.to_owned(),
            reason: format!("path does not exist: {}", root.display()),
        });
    }
    if let Some(relocated) = find_relocated_missions_ancestor(root) {
        return Ok(relocated);
    }

    let mut candidates: Vec<PathBuf> = vec![root.join(MISSION_ASSETS_DIR_NAME)];
    candidates.extend(sorted_glob(root, "src/*/*/missions"));
    if root
        .file_name()
        .is_some_and(|name| name == MISSION_ASSETS_DIR_NAME)
        && root
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|name| name == SRC_LAYOUT_DIR_NAME)
        && let Some(src) = root.parent().and_then(Path::parent)
    {
        candidates.extend(sorted_glob(src, "*/*/missions"));
    }
    candidates.push(root.to_path_buf());

    for candidate in candidates {
        if candidate.is_dir() && looks_like_missions_root(&candidate) {
            return Ok(candidate);
        }
    }
    Err(KernelError::PackRootNotFound {
        what: TEMPLATE_ROOT_ENV.to_owned(),
        reason: format!(
            "{} does not contain mission assets (expected a missions directory or a Spec Kitty checkout root)",
            root.display()
        ),
    })
}

fn find_relocated_missions_ancestor(root: &Path) -> Option<PathBuf> {
    for ancestor in std::iter::once(root).chain(root.ancestors().skip(1)) {
        let candidate = ancestor.join(MISSION_ASSETS_SIBLING_PATTERN);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

fn looks_like_missions_root(path: &Path) -> bool {
    first_match(path, &["*", "templates", "*.md"], true).is_some()
        || first_match(path, &["*", "command-templates", "*.md"], true).is_some()
        || first_match(path, &["mission-steps", "*", "*", "prompt.md"], true).is_some()
}

fn sorted_glob(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let components: Vec<&str> = pattern.split('/').collect();
    let mut matches = Vec::new();
    collect_matches(root, &components, &mut matches);
    matches.sort();
    matches
}

fn collect_matches(directory: &Path, components: &[&str], matches: &mut Vec<PathBuf>) {
    let Some((component, remaining)) = components.split_first() else {
        matches.push(directory.to_path_buf());
        return;
    };
    if *component == "*" {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let candidate = entry.path();
            if candidate.is_dir() {
                collect_matches(&candidate, remaining, matches);
            }
        }
        return;
    }
    let candidate = directory.join(component);
    if remaining.is_empty() {
        if candidate.is_dir() {
            matches.push(candidate);
        }
    } else if candidate.is_dir() {
        collect_matches(&candidate, remaining, matches);
    }
}

fn first_match(directory: &Path, components: &[&str], leaf_is_file: bool) -> Option<PathBuf> {
    let (component, remaining) = components.split_first()?;
    if *component == "*" {
        let mut entries: Vec<PathBuf> = fs::read_dir(directory)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        for candidate in entries {
            if remaining.is_empty() {
                if if leaf_is_file {
                    candidate.is_file()
                } else {
                    candidate.is_dir()
                } {
                    return Some(candidate);
                }
                continue;
            }
            if candidate.is_dir()
                && let Some(found) = first_match(&candidate, remaining, leaf_is_file)
            {
                return Some(found);
            }
        }
        return None;
    }
    if component.contains('*') {
        // Leaf wildcard such as `*.md`.
        let mut entries: Vec<PathBuf> = fs::read_dir(directory)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        return entries.into_iter().find(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| wildcard_match(component, name))
                && if remaining.is_empty() {
                    if leaf_is_file {
                        candidate.is_file()
                    } else {
                        candidate.is_dir()
                    }
                } else {
                    candidate.is_dir()
                }
        });
    }
    let candidate = directory.join(component);
    if remaining.is_empty() {
        if leaf_is_file {
            candidate.is_file().then_some(candidate)
        } else {
            candidate.is_dir().then_some(candidate)
        }
    } else if candidate.is_dir() {
        first_match(&candidate, remaining, leaf_is_file)
    } else {
        None
    }
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let mut pattern_index = 0;
    let mut name_index = 0;
    let mut star = None;
    let mut star_match = 0;
    while name_index < name.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == name[name_index])
        {
            pattern_index += 1;
            name_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star = Some(pattern_index);
            star_match = name_index;
            pattern_index += 1;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            star_match += 1;
            name_index = star_match;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

/// Resolve a directory that ships as a filesystem sibling of `anchor_file`.
///
/// This is the bounded, domain-agnostic ancestor walk from upstream
/// `kernel.sibling_paths`. `pattern` may contain `*` segments. The walk stops
/// one hop past a recognised `src` / `site-packages` / `dist-packages`
/// boundary, or after four ancestors when no boundary is seen, so a broken
/// install fails closed instead of matching an unrelated tree.
pub fn resolve_installed_sibling(
    anchor_file: &Path,
    env_override: Option<&Path>,
    pattern: &Path,
) -> Result<PathBuf, KernelError> {
    if let Some(override_dir) = env_override
        && override_dir.is_dir()
    {
        return Ok(override_dir.to_path_buf());
    }

    let anchor = resolve_non_strict(anchor_file)
        .map_err(|failure| map_resolve_failure(anchor_file, failure))?;
    let components: Vec<&str> = pattern
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    if components.is_empty() {
        return Err(KernelError::PackRootNotFound {
            what: pattern.display().to_string(),
            reason: "the sibling pattern has no usable components".to_owned(),
        });
    }

    let mut hops_past_boundary: Option<usize> = None;
    let mut ancestors_without_boundary = 0;
    for ancestor in anchor.as_path().ancestors().skip(1) {
        if let Some(candidate) = first_match(ancestor, &components, false)
            && candidate.is_dir()
        {
            return Ok(candidate);
        }
        if let Some(hops) = hops_past_boundary.as_mut() {
            *hops += 1;
            if *hops > 1 {
                break;
            }
        } else if ancestor
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "src" | "site-packages" | "dist-packages"))
        {
            hops_past_boundary = Some(0);
        } else {
            ancestors_without_boundary += 1;
            if ancestors_without_boundary >= 4 {
                break;
            }
        }
    }

    Err(KernelError::PackRootNotFound {
        what: pattern.display().to_string(),
        reason: format!(
            "could not resolve it as a sibling of {}: no env override or ancestor sibling directory matched",
            anchor_file.display()
        ),
    })
}

/// Render a runtime-state path for user-facing output.
///
/// POSIX: `~/<relpath>` when under `$HOME` (if `for_user`), else absolute.
/// Windows: always the absolute path (no tilde substitution).
pub fn render_runtime_path(path: &Path, for_user: bool) -> Result<String, KernelError> {
    let absolute =
        resolve_non_strict(path).map_err(|failure| map_resolve_failure(path, failure))?;
    if !for_user || cfg!(windows) {
        return Ok(absolute.display().to_string());
    }
    let home = match home_directory().and_then(|home| {
        resolve_non_strict(&home).map_err(|failure| map_resolve_failure(&home, failure))
    }) {
        Ok(home) => home,
        Err(_) => return Ok(absolute.display().to_string()),
    };
    match absolute.strip_prefix(&home) {
        Ok(relative) => Ok(format!("~/{}", to_posix(relative))),
        Err(_) => Ok(absolute.display().to_string()),
    }
}

/// Normalise a path (or path-like string) to a forward-slashed string.
pub fn to_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Join path parts into a Git tree path (always forward-slashed).
pub fn posix_tree_path(parts: &[&str]) -> String {
    parts.join("/")
}

/// Return `(git_cwd, tree_path)` for a repo file, stripping `.worktrees/<name>`.
pub fn repo_tree_path(
    file_path: &Path,
    repo_root: &Path,
) -> Result<(PathBuf, String), KernelError> {
    let repo =
        resolve_non_strict(repo_root).map_err(|failure| map_resolve_failure(repo_root, failure))?;
    let file =
        resolve_non_strict(file_path).map_err(|failure| map_resolve_failure(file_path, failure))?;
    let relative = file
        .strip_prefix(&repo)
        .map_err(|_| KernelError::PathEscape {
            root: repo.clone(),
            resolved: file.clone(),
        })?;
    let parts: Vec<&str> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    if parts.len() > 2 && parts[0] == ".worktrees" {
        let worktree_root = repo.join(parts[0]).join(parts[1]);
        if worktree_root.is_dir() {
            return Ok((worktree_root, posix_tree_path(&parts[2..])));
        }
    }
    Ok((repo, posix_tree_path(&parts)))
}

fn default_pack_anchors() -> Vec<PathBuf> {
    let mut anchors = Vec::new();
    // A Rust workspace checkout puts `packs/` two levels above the crate, not
    // one above `src/` as the Python source layout does. Anchoring the walk at
    // the workspace manifest keeps the shared boundary rule unchanged.
    let workspace_manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("Cargo.toml");
    if workspace_manifest.is_file() {
        anchors.push(workspace_manifest);
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("paths.rs");
    if manifest.is_file() {
        anchors.push(manifest);
    }
    if let Ok(executable) = env::current_exe() {
        anchors.push(executable);
    }
    if anchors.is_empty() {
        anchors.push(PathBuf::from("packs"));
    }
    anchors
}

fn non_empty(name: &str) -> Option<OsString> {
    match env::var_os(name) {
        Some(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matching_is_the_glob_contract() {
        assert!(wildcard_match("*.md", "spec.md"));
        assert!(wildcard_match("m???", "main"));
        assert!(!wildcard_match("*.md", "spec.txt"));
        assert!(wildcard_match("a*b*c", "aXbYc"));
    }

    #[test]
    fn posix_tree_path_is_forward_slashed() {
        assert_eq!(
            posix_tree_path(&["kitty-specs", "m", "spec.md"]),
            "kitty-specs/m/spec.md"
        );
        assert_eq!(posix_tree_path(&[]), "");
    }

    #[test]
    fn sibling_walk_finds_a_checkout_pack_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let anchor = root.join("src").join("pkg").join("module.py");
        fs::create_dir_all(anchor.parent().unwrap()).unwrap();
        fs::write(&anchor, b"").unwrap();
        fs::create_dir_all(root.join("packs").join("built-in")).unwrap();
        let found =
            resolve_installed_sibling(&anchor, None, Path::new(BUILT_IN_PACK_SIBLING_PATTERN))
                .unwrap();
        assert_eq!(found, root.join("packs").join("built-in"));
    }

    #[test]
    fn sibling_walk_fails_closed_without_a_match() {
        let temp = tempfile::tempdir().unwrap();
        let anchor = temp.path().join("src").join("pkg").join("module.py");
        fs::create_dir_all(anchor.parent().unwrap()).unwrap();
        assert!(resolve_installed_sibling(&anchor, None, Path::new("never/matches")).is_err());
    }

    #[test]
    fn template_root_accepts_the_relocated_pack_shape() {
        let temp = tempfile::tempdir().unwrap();
        let checkout = temp.path().join("checkout");
        let missions = checkout.join("packs").join("built-in").join("missions");
        fs::create_dir_all(missions.join("software-dev").join("templates")).unwrap();
        fs::write(
            missions
                .join("software-dev")
                .join("templates")
                .join("spec.md"),
            b"# spec",
        )
        .unwrap();
        assert_eq!(resolve_template_root(&checkout).unwrap(), missions);
    }
}
