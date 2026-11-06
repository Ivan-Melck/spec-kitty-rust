//! Package-asset and global-runtime resolution for the pre-3.x migrations.
//!
//! The pinned modules reach into their own installation for the bytes they
//! copy: `src/specify_cli/missions/<name>` (`_find_package_missions`,
//! `_find_source_mission`), `charter.offering/skills/<name>`
//! (`files("charter.offering")`), and `get_package_asset_root()` for the
//! `.kittify/` legacy-asset movement (`runtime/migrate.py`).
//!
//! A native binary has no Python package directory, so the same assets are
//! resolved from the shipped checkout/pack layout, in this order:
//!
//! 1. `SPEC_KITTY_TEMPLATE_ROOT` — the source's own CI override; a missions
//!    tree is `<root>/.kittify/missions` (source `_find_package_missions`
//!    rule 3) or `<root>/src/specify_cli/missions` (rule 1 shape).
//! 2. `<checkout>/src/specify_cli/missions` — the legacy bundled mission tree
//!    the pinned migrations copy from, where `<checkout>` is the nearest
//!    ancestor of this crate or of the current directory holding it.
//! 3. `<checkout>/packs/built-in/missions` (honouring
//!    `SPEC_KITTY_PACKS_ROOT`) — the pinned replacement for that bundled tree;
//!    its `software-dev`, `research` and `documentation` missions carry the
//!    same `mission.yaml` contract.
//!
//! Every lookup is fallible and reports *absent*, never a guess: a migration
//! whose asset is missing records the source's own "not applicable" refusal
//! (`can_apply` → skip) instead of copying something invented.

use std::path::{Path, PathBuf};

use sk_charter::BuiltInSearch;

use crate::error::InstallError;
use crate::metadata::ProjectMetadata;
use crate::version::Version;

/// The mission names the pre-3.x migrations repair, in repository order.
pub const LEGACY_MISSIONS: [&str; 3] = ["software-dev", "research", "documentation"];

/// The environment variable the pinned source uses for the template root.
pub const TEMPLATE_ROOT_ENV: &str = "SPEC_KITTY_TEMPLATE_ROOT";

fn absolute(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

/// The checkout root this binary can prove, if any.
///
/// `CARGO_MANIFEST_DIR/../..` is the build-time checkout; a binary run from a
/// source tree also resolves through the current directory's ancestors, which
/// is the source's own repository-root fallback.
pub fn checkout_root() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.push(absolute(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
    ));
    if let Ok(current) = std::env::current_dir() {
        let mut cursor = Some(current.as_path());
        while let Some(directory) = cursor {
            candidates.push(absolute(directory.to_path_buf()));
            cursor = directory.parent();
        }
    }
    candidates.into_iter().find(|root| {
        root.join("src/specify_cli/missions").is_dir() || root.join("packs/built-in").is_dir()
    })
}

/// The bundled/legacy missions root (`_find_package_missions`).
pub fn package_missions_root() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(TEMPLATE_ROOT_ENV) {
        let root = absolute(PathBuf::from(value));
        let legacy = root.join(".kittify/missions");
        if legacy.is_dir() {
            return Some(legacy);
        }
        let bundled = root.join("src/specify_cli/missions");
        if bundled.is_dir() {
            return Some(bundled);
        }
    }
    if let Some(checkout) = checkout_root() {
        let bundled = checkout.join("src/specify_cli/missions");
        if bundled.is_dir() {
            return Some(bundled);
        }
        if let Ok(pack) = BuiltInSearch::from_env(checkout.join("src")).find_root() {
            let missions = pack.join("missions");
            if missions.is_dir() {
                return Some(missions);
            }
        }
    }
    None
}

/// The **pinned pack** missions root (`packs/built-in/missions`), the tree the
/// canonical `mission-steps/<mission>/<command>/prompt.md` templates live in.
///
/// Distinct from [`package_missions_root`]: that answers "which tree do the
/// pre-3.x copy migrations read?", which may be the legacy
/// `src/specify_cli/missions` layout. Command *rendering* must use this one,
/// because only the pack tree carries the canonical step prompts.
pub fn pack_missions_root() -> Option<PathBuf> {
    if let Some(checkout) = checkout_root()
        && let Some(missions) = crate::template_tree::discover_pack_missions(&checkout)
    {
        return Some(missions);
    }
    let checkout = checkout_root()?;
    let pack = BuiltInSearch::from_env(checkout.join("src"))
        .find_root()
        .ok()?;
    let missions = pack.join("missions");
    missions.is_dir().then_some(missions)
}

/// One bundled mission directory, when the packaged tree ships it.
pub fn package_mission(name: &str) -> Option<PathBuf> {
    let missions = package_missions_root()?;
    let mission = missions.join(name);
    mission.is_dir().then_some(mission)
}

/// The bundled Python package root the legacy migrations read from
/// (`Path(specify_cli.__file__).parent`): `<checkout>/src/specify_cli`.
///
/// Only the resources a migration copies are read from here (the bash script
/// tree, the legacy mission tree); a binary without a checkout reports absent.
pub fn package_cli_root() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(TEMPLATE_ROOT_ENV) {
        let root = absolute(PathBuf::from(value));
        for candidate in [
            root.join("src/specify_cli"),
            root.join("specify_cli"),
            root.clone(),
        ] {
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    let checkout = checkout_root()?;
    let root = checkout.join("src/specify_cli");
    root.is_dir().then_some(root)
}

/// The package asset root `get_package_asset_root()` returns: the built-in
/// `missions` tree used by `.kittify/` legacy-asset classification.
pub fn package_asset_root() -> Option<PathBuf> {
    if std::env::var_os(TEMPLATE_ROOT_ENV).is_some()
        && let Some(root) = package_missions_root()
    {
        return Some(root);
    }
    let checkout = checkout_root()?;
    let pack = BuiltInSearch::from_env(checkout.join("src"))
        .find_root()
        .ok()?;
    let missions = pack.join("missions");
    missions.is_dir().then_some(missions)
}

/// The charter offering skill catalog root (`charter.offering/skills`).
pub fn offering_skills_root() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(TEMPLATE_ROOT_ENV) {
        let root = absolute(PathBuf::from(value));
        let skills = root.join("src/charter/offering/skills");
        if skills.is_dir() {
            return Some(skills);
        }
    }
    let checkout = checkout_root()?;
    [
        checkout.join("src/charter/offering/skills"),
        checkout.join("packs/built-in/skills"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

/// The canonical files of one offering skill, `SKILL.md` first then the rest in
/// sorted order (a missing skill is `None`, never an empty success).
pub fn canonical_skill_files(skill: &str) -> Option<Vec<(String, Vec<u8>)>> {
    let root = offering_skills_root()?.join(skill);
    if !root.is_dir() {
        return None;
    }
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory).ok()?;
        let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        children.sort();
        for child in children {
            let metadata = std::fs::symlink_metadata(&child).ok()?;
            if metadata.is_dir() {
                stack.push(child.clone());
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let relative = child
                .strip_prefix(&root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            if relative
                .rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            files.push((relative, std::fs::read(&child).ok()?));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    if !files.iter().any(|(relative, _)| relative == "SKILL.md") {
        return None;
    }
    Some(files)
}

/// The user-global `.kittify/` runtime home (`get_kittify_home`).
pub fn kittify_home() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("SPEC_KITTY_HOME") {
        return Some(PathBuf::from(value));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".kittify"))
}

/// Whether `~/.kittify/cache/version.lock` proves a bootstrapped global runtime.
pub fn global_runtime_configured() -> bool {
    kittify_home()
        .map(|home| home.join("cache/version.lock").is_file())
        .unwrap_or(false)
}

/// `upgrade/compat.py::uses_centralized_runtime`.
pub fn uses_centralized_runtime(project_root: &Path) -> bool {
    if !global_runtime_configured() {
        return false;
    }
    if let Some(metadata) = ProjectMetadata::load(&project_root.join(".kittify"))
        && let Ok(version) = Version::parse(&metadata.version)
        && let Ok(floor) = Version::parse("2.0.0")
    {
        return version >= floor;
    }
    !project_root.join(".kittify").exists() && project_root.join("kitty-specs").exists()
}

/// `shutil.copytree(source, destination)` for a mission/skill tree.
///
/// The source semantics this migration depends on: the destination must be
/// absent (`dirs_exist_ok=False`), directories are created recursively, file
/// modes are copied, and a symlink is *dereferenced* — `copytree`'s default
/// `symlinks=False` writes the target's bytes, which is what keeps a packaged
/// mission's linked template readable inside the project.
pub fn copy_tree(source: &Path, destination: &Path) -> Result<usize, InstallError> {
    if destination.exists() || destination.is_symlink() {
        return Err(InstallError::io(
            "copy tree",
            destination,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "destination already exists",
            ),
        ));
    }
    let mut files = 0usize;
    copy_tree_into(source, destination, &mut files)?;
    Ok(files)
}

fn copy_tree_into(
    source: &Path,
    destination: &Path,
    files: &mut usize,
) -> Result<(), InstallError> {
    let metadata =
        std::fs::metadata(source).map_err(|error| InstallError::io("inspect", source, error))?;
    if metadata.is_dir() {
        std::fs::create_dir_all(destination)
            .map_err(|error| InstallError::io("create directory", destination, error))?;
        let mut children: Vec<PathBuf> = std::fs::read_dir(source)
            .map_err(|error| InstallError::io("read directory", source, error))?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        children.sort();
        for child in children {
            let name = child.file_name().map(PathBuf::from).ok_or_else(|| {
                InstallError::UpgradeRefused {
                    detail: format!("{} has no file name", child.display()),
                }
            })?;
            copy_tree_into(&child, &destination.join(name), files)?;
        }
        std::fs::set_permissions(destination, metadata.permissions())
            .map_err(|error| InstallError::io("set permissions", destination, error))?;
        return Ok(());
    }
    let bytes = std::fs::read(source).map_err(|error| InstallError::io("read", source, error))?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| InstallError::io("create directory", parent, error))?;
    }
    std::fs::write(destination, &bytes)
        .map_err(|error| InstallError::io("write", destination, error))?;
    std::fs::set_permissions(destination, metadata.permissions())
        .map_err(|error| InstallError::io("set permissions", destination, error))?;
    *files += 1;
    Ok(())
}

/// The `meta.json` copy git holds for a Mission whose working-tree copy is gone
/// or identity-less (`mission_metadata.read_tracked_meta`).
///
/// The tracked content is `HEAD`'s blob for the Mission's path — exactly what
/// `git checkout -- <path>` restores. An index-only copy (staged, never
/// committed) is *not* history's identity, and a committed copy whose index
/// blob differs (a staged edit) may not be overwritten by `HEAD`'s bytes: both
/// are reported `unavailable`, so the caller writes nothing rather than creating
/// an identity over a copy git does hold.
pub fn read_tracked_meta(feature_dir: &Path) -> TrackedMeta {
    let Some(prefix) = checkout_prefix(feature_dir) else {
        return TrackedMeta::default();
    };
    let relative = format!("kitty-specs/{}/meta.json", feature_dir_name(feature_dir));
    let path = format!("{prefix}{relative}");
    let committed = crate::migrations::git::run(feature_dir, &["show", &format!("HEAD:{path}")]);
    let committed = match committed {
        Ok(output) if output.status.success() => output.stdout,
        Ok(_) => {
            // HEAD has no blob: an index-only copy cannot be history's identity.
            return if index_tracks_meta(feature_dir, &path) {
                TrackedMeta {
                    meta: None,
                    unavailable: true,
                }
            } else {
                TrackedMeta::default()
            };
        }
        // git ran and refused (dubious ownership, an unreadable `.git`): the
        // repository state is unknown, so refuse instead of minting an identity.
        Err(_) => {
            return TrackedMeta {
                meta: None,
                unavailable: true,
            };
        }
    };
    if index_meta_bytes(feature_dir, &path).is_some_and(|staged| staged != committed) {
        return TrackedMeta {
            meta: None,
            unavailable: true,
        };
    }
    match serde_json::from_slice::<serde_json::Value>(&committed) {
        Ok(value) if value.is_object() => TrackedMeta {
            meta: Some(value),
            unavailable: false,
        },
        Ok(_) => TrackedMeta {
            meta: None,
            unavailable: true,
        },
        Err(_) => TrackedMeta {
            meta: None,
            unavailable: true,
        },
    }
}

/// The tracked copy of one Mission's `meta.json`, when git holds a usable one.
#[derive(Clone, Debug, Default)]
pub struct TrackedMeta {
    /// The tracked copy, when git returned a JSON object.
    pub meta: Option<serde_json::Value>,
    /// Git knows the path but its content could not be used.
    pub unavailable: bool,
}

fn feature_dir_name(feature_dir: &Path) -> String {
    feature_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `rev-parse --is-inside-work-tree --show-prefix` keyed on the Mission folder.
fn checkout_prefix(feature_dir: &Path) -> Option<String> {
    match crate::migrations::git::run(
        feature_dir,
        &["rev-parse", "--is-inside-work-tree", "--show-prefix"],
    ) {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            let mut lines = text.split('\n');
            let inside = lines.next().unwrap_or_default().trim() == "true";
            if inside {
                Some(lines.next().unwrap_or_default().to_string())
            } else {
                None
            }
        }
        // Not a repository, or git is not installed: nothing is tracked here.
        _ => None,
    }
}

fn index_meta_bytes(feature_dir: &Path, path: &str) -> Option<Vec<u8>> {
    let output = crate::migrations::git::run(feature_dir, &["show", &format!(":{path}")]).ok()?;
    output.status.success().then_some(output.stdout)
}

fn index_tracks_meta(feature_dir: &Path, path: &str) -> bool {
    crate::migrations::git::run(feature_dir, &["ls-files", "--error-unmatch", "--", path])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Whether `path` lives inside the project (a destination may never escape it).
pub fn inside(project_root: &Path, path: &Path) -> bool {
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let resolved = match std::fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(_) => {
            let Some(parent) = path.parent() else {
                return false;
            };
            let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            parent.join(path.file_name().unwrap_or_default())
        }
    };
    resolved.starts_with(&root)
}
