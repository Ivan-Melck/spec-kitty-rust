//! Canonical mission-qualified template catalog and the ONE six-tier template
//! resolver.
//!
//! The reference surface is `charter.offering.template_catalog` (discovery plus
//! `resolve_template_by_id`) delegating to the private six-tier
//! `charter.offering.resolver._resolve_asset` chain. This module is the single
//! Native owner of both halves:
//!
//! - [`discover_templates`] enumerates templates across caller-supplied tier
//!   roots, annotating each with its source tier. Template identity is the
//!   mission-qualified `<mission>/<name>` pair, so the same file name under two
//!   missions stays two distinct references, while the same pair in two tiers is
//!   deduplicated to the first supplied (highest-precedence) tier.
//! - [`resolve_template`] / [`resolve_command`] / [`resolve_template_by_id`]
//!   perform the reference's one six-tier content/command resolution:
//!   `override` (mission-scoped, then the legacy global override) → `legacy` →
//!   `org` (declared packs, first match in declaration order) → `global_mission`
//!   → `global` → `package_default`.
//!
//! This is a resolution chain, not an overlay merge: the first tier that has
//! the file wins, and there is no later-wins artifact merging. Org configuration
//! is read **lazily** — only after the override and legacy tiers miss — and an
//! unavailable home directory skips the two global tiers without failing the
//! lookup, exactly like the reference's `RuntimeError` guard.
//!
//! # Provenance, not guessing
//!
//! [`ResolutionResult`] carries the winning path, its [`TemplateTier`] and the
//! mission. [`TemplateTier::as_str`] returns the reference's enum *value*
//! spelling (`global_mission`, `package_default`), which is what
//! `charter context --include` prints; no hyphenated wire label is invented.
//! [`render_template_include`] renders the reference's
//! `Template <id> (tier: <tier>):` heading plus the template text with trailing
//! newlines stripped (`rstrip("\n")`).
//!
//! # Authorities this module reuses
//!
//! Project org roots and `.kittify/config.yaml` come from [`crate::config`]; the
//! user-global `.kittify` home and the installed package missions root come from
//! `sk-kernel`'s path authorities; bytes and UTF-8 decoding come from the
//! kernel's Source-policy reader and CPython-exact strict decoder; newline
//! translation comes from the kernel's `TextIOWrapper(newline=None)` primitive.
//! Nothing here re-implements a path resolver, a YAML parser or a decoder.
//!
//! # Known gaps (explicit, not papered over)
//!
//! * **Caller bases differ.** [`render_source_template_include`] preserves the
//!   reference include's project-LAYER directory selection (or absence), even
//!   though its docstring claims a repository root. Compatibility callers use
//!   that facade. [`render_template_include`] and the explicit-root resolver
//!   remain separate data/Native capabilities; their extra reachable project
//!   tiers are not silently adopted by Source compatibility.
//! * The reference's `logging.warning` / `DeprecationWarning` stderr wire for a
//!   legacy-tier hit is not reproduced; the tier is reported in
//!   [`TemplateTier::Legacy`] and the invocation context owns any user-facing
//!   nudge. The reference's Python-formatted OSError text
//!   (`[Errno N] strerror: '<path>'`) is likewise not reconstructed for a read
//!   failure — this module reports the underlying I/O error text, and the
//!   reference's Rich word-wrap of *text-mode* output is not applied.
//! * Discovery refuses a mission/name directory entry that is not valid UTF-8
//!   ([`TemplateError::NonUtf8Name`]) because the workspace has no canonical
//!   lossless Python `str` path decoding; it never substitutes a lossy name.
//! * DRG node minting for discovered templates (`template:<mission>/<name>` URNs
//!   for `charter list --all`) is not part of this module: the graph authority
//!   lives in `sk-doctrine`, which depends on this crate, never the other way
//!   round. [`template_urn`] provides the URN spelling without minting a node.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sk_kernel::python_text::universal_newlines;
use sk_kernel::python_utf8::decode_utf8;
use sk_kernel::{PythonString, read_source_bytes};

use crate::config::{PathExpansion, load_org_pack_registry};
use crate::limits::Limits;

/// The content-template subdirectory name.
pub const CONTENT_SUBDIR: &str = "templates";

/// The command-template subdirectory name.
pub const COMMAND_SUBDIR: &str = "command-templates";

/// The per-mission template subdirectories discovery scans.
///
/// Both content templates (`templates/spec-template.md`) and command templates
/// (`command-templates/implement.md`) are mission-scoped in the reference.
pub const TEMPLATE_SUBDIRS: [&str; 2] = [CONTENT_SUBDIR, COMMAND_SUBDIR];

/// Files that live in template directories but are not themselves templates.
pub const NON_TEMPLATE_FILES: [&str; 1] = ["README.md"];

/// The reference's fallback project root when no tier root supplies one.
const NO_PROJECT_DIR: &str = "/nonexistent-spec-kitty-project";

/// One template resolution tier, in the reference's precedence order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TemplateTier {
    /// `.kittify/overrides/missions/<mission>/{templates,command-templates}/`
    /// (mission-scoped), falling back to the global
    /// `.kittify/overrides/{templates,command-templates}/`.
    Override,
    /// `.kittify/{templates,command-templates}/` (deprecated).
    Legacy,
    /// `<org_root>/missions/<mission>/{templates,command-templates}/` for each
    /// declared org pack, in declaration order.
    Org,
    /// `~/.kittify/missions/<mission>/{templates,command-templates}/`.
    GlobalMission,
    /// `~/.kittify/{templates,command-templates}/`.
    Global,
    /// The installed `packs/built-in/missions/<mission>/…` default.
    PackageDefault,
}

impl TemplateTier {
    /// Every tier, highest precedence first.
    pub const ORDER: [Self; 6] = [
        Self::Override,
        Self::Legacy,
        Self::Org,
        Self::GlobalMission,
        Self::Global,
        Self::PackageDefault,
    ];

    /// The reference's `ResolutionTier.value` spelling (underscored, never hyphenated).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Legacy => "legacy",
            Self::Org => "org",
            Self::GlobalMission => "global_mission",
            Self::Global => "global",
            Self::PackageDefault => "package_default",
        }
    }
}

impl fmt::Display for TemplateTier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A resolution-tier root supplied to discovery as data.
///
/// Each root points at a *missions root*: a directory whose immediate children
/// are per-mission directories that may contain `templates/` and
/// `command-templates/` subdirectories. The caller resolves these paths; this
/// module never reaches into `.kittify` or `~/.kittify` on its own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TierRoot {
    /// The resolution tier this root represents.
    pub tier: TemplateTier,
    /// Directory containing per-mission template trees.
    pub missions_root: PathBuf,
    /// Optional project root (the directory containing `.kittify/`), which is
    /// what makes the project-scoped override/legacy tiers resolvable.
    pub project_dir: Option<PathBuf>,
}

impl TierRoot {
    /// A tier root for `tier` at `missions_root`.
    pub fn new(tier: TemplateTier, missions_root: impl Into<PathBuf>) -> Self {
        Self {
            tier,
            missions_root: missions_root.into(),
            project_dir: None,
        }
    }

    /// Attach the project root that carries `.kittify/`.
    #[must_use]
    pub fn with_project_dir(mut self, project_dir: impl Into<PathBuf>) -> Self {
        self.project_dir = Some(project_dir.into());
        self
    }
}

/// One discovered template, mission-qualified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateRef {
    /// Mission-qualified identity `<mission>/<name>`.
    pub template_id: String,
    /// Mission name (for example `software-dev`).
    pub mission: String,
    /// Template file name, including any extension.
    pub name: String,
    /// Source resolution tier.
    pub tier: TemplateTier,
    /// Path to the template file on disk.
    pub path: PathBuf,
}

/// The winning tier, path and mission of one template lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolutionResult {
    /// The resolved path.
    pub path: PathBuf,
    /// The tier that produced it.
    pub tier: TemplateTier,
    /// The mission the lookup named, when it named one.
    pub mission: Option<String>,
}

/// Why a template lookup or catalog scan failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateError {
    /// The template id is not `<mission>/<name>` (the reference's `ValueError`).
    MalformedId {
        /// The id exactly as the operator spelled it.
        template_id: String,
    },
    /// A declared org pack root could not be computed (the reference's
    /// `ValueError` family: an unset environment token, an escaping `subdir`).
    OrgConfig {
        /// The underlying message.
        detail: String,
    },
    /// No tier holds the requested asset (the reference's `FileNotFoundError`).
    NotFound {
        /// The requested file name.
        name: String,
        /// `templates` or `command-templates`.
        subdir: String,
        /// The mission the lookup named.
        mission: String,
        /// The project root the lookup used.
        project: PathBuf,
    },
    /// The installed package missions root cannot be resolved. The reference
    /// does not catch this exception class, so it is reported as its own
    /// failure rather than folded into "not found".
    PackageRoot {
        /// The underlying message.
        detail: String,
    },
    /// A discovered mission or template name is not valid UTF-8, so no exact
    /// Python `str` identity can be minted.
    NonUtf8Name {
        /// The directory entry that could not be named.
        path: PathBuf,
    },
    /// A catalog scan could not read a directory.
    Scan {
        /// The directory that failed.
        path: PathBuf,
        /// The operating-system detail.
        detail: String,
    },
}

impl TemplateError {
    /// Whether the reference raises a `ValueError` for this failure.
    ///
    /// The reference's include renderer catches `ValueError` and re-labels it
    /// `Malformed template selector …`; `FileNotFoundError` and the packaged
    /// root failures take other paths.
    #[must_use]
    pub fn is_reference_value_error(&self) -> bool {
        matches!(self, Self::MalformedId { .. } | Self::OrgConfig { .. })
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedId { template_id } => write!(
                formatter,
                "template_id {} is not of the form '<mission>/<name>'",
                PythonString::from_scalars(template_id).repr()
            ),
            Self::OrgConfig { detail } | Self::PackageRoot { detail } => {
                formatter.write_str(detail)
            }
            Self::NotFound {
                name,
                subdir,
                mission,
                project,
            } => write!(
                formatter,
                "Asset '{name}' not found in any resolution tier (subdir={}, mission={}, project={})",
                PythonString::from_scalars(subdir).repr(),
                PythonString::from_scalars(mission).repr(),
                project.display()
            ),
            Self::NonUtf8Name { path } => write!(
                formatter,
                "native template catalog cannot name directory entry {}: it is not valid UTF-8 \
                 and this workspace has no lossless Python `str` path decoding yet",
                path.display()
            ),
            Self::Scan { path, detail } => {
                write!(
                    formatter,
                    "cannot read template directory {}: {detail}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for TemplateError {}

/// Where the resolver's non-project roots come from.
///
/// The live implementation reads the canonical kernel/`sk-charter` authorities.
/// Tests inject their own data instead, so the six-tier order is provable
/// without mutating process environment.
pub trait TemplateRoots {
    /// `~/.kittify`, or `None` when no home directory can be determined.
    ///
    /// The reference catches the home-directory `RuntimeError` and simply skips
    /// the two global tiers, so this is an availability fact, not an error.
    fn kittify_home(&self) -> Option<PathBuf>;

    /// The installed package missions root (`packs/built-in/missions`).
    fn package_missions_root(&self) -> Result<PathBuf, TemplateError>;

    /// Declared org pack roots in declaration order.
    ///
    /// Called only after the override and legacy tiers miss, which is what keeps
    /// org configuration lazy.
    fn org_roots(&self, project_dir: &Path) -> Result<Vec<PathBuf>, TemplateError>;
}

/// The live [`TemplateRoots`] implementation: kernel paths and real org config.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiveTemplateRoots;

impl TemplateRoots for LiveTemplateRoots {
    fn kittify_home(&self) -> Option<PathBuf> {
        sk_kernel::get_kittify_home().ok()
    }

    fn package_missions_root(&self) -> Result<PathBuf, TemplateError> {
        sk_kernel::get_package_asset_root().map_err(|error| TemplateError::PackageRoot {
            detail: error.to_string(),
        })
    }

    fn org_roots(&self, project_dir: &Path) -> Result<Vec<PathBuf>, TemplateError> {
        // The reference reads the org registry through the lenient
        // `load_pack_registry(quiet=True)` path: an unreadable or invalid
        // registry contributes no org layer and is not an error here. Path
        // expansion/subdir containment of a *declared* pack stays fail-closed
        // (the reference's `ValueError` family) and is raised below.
        let Ok(packs) = load_org_pack_registry(project_dir, &Limits::default()) else {
            return Ok(Vec::new());
        };
        let expansion = PathExpansion::from_env();
        packs
            .iter()
            .map(|pack| {
                pack.effective_root(project_dir, &expansion)
                    .map_err(|error| TemplateError::OrgConfig {
                        detail: error.to_string(),
                    })
            })
            .collect()
    }
}

/// Return the mission-qualified template id `<mission>/<name>`.
#[must_use]
pub fn template_id_for(mission: &str, name: &str) -> String {
    format!("{mission}/{name}")
}

/// Return the DRG URN spelling for a template id: `template:<mission>/<name>`.
#[must_use]
pub fn template_urn(template_id: &str) -> String {
    format!("template:{template_id}")
}

/// Enumerate templates across the supplied tier roots.
///
/// Every root is walked for per-mission `templates/` and `command-templates/`
/// directories; every regular file found becomes a [`TemplateRef`] annotated
/// with its source tier. Directories are walked recursively (a
/// `documentation/templates/divio/…` template is still found), and
/// [`NON_TEMPLATE_FILES`] is filtered out. The same `<mission>/<name>` in
/// several tiers is deduplicated to the **first supplied** root, mirroring the
/// resolution order.
///
/// # Errors
///
/// [`TemplateError::NonUtf8Name`] for a directory entry that cannot be minted as
/// an exact template identity, and [`TemplateError::Scan`] when a directory
/// cannot be read.
pub fn discover_templates(tier_roots: &[TierRoot]) -> Result<Vec<TemplateRef>, TemplateError> {
    let mut seen: BTreeMap<String, TemplateRef> = BTreeMap::new();
    for root in tier_roots {
        for (mission, mission_dir) in mission_dirs(&root.missions_root)? {
            for path in template_files(&mission_dir)? {
                let name = entry_name(&path)?;
                let template_id = template_id_for(&mission, &name);
                if seen.contains_key(&template_id) {
                    continue;
                }
                seen.insert(
                    template_id.clone(),
                    TemplateRef {
                        template_id,
                        mission: mission.clone(),
                        name,
                        tier: root.tier,
                        path,
                    },
                );
            }
        }
    }
    let mut refs: Vec<TemplateRef> = seen.into_values().collect();
    refs.sort_by(|left, right| {
        left.template_id
            .cmp(&right.template_id)
            .then_with(|| left.tier.as_str().cmp(right.tier.as_str()))
    });
    Ok(refs)
}

/// `(mission_name, mission_dir)` pairs under a missions root, sorted by name.
///
/// A missing or non-directory root yields nothing, so a caller may pass a tier
/// root that does not exist on this machine.
fn mission_dirs(missions_root: &Path) -> Result<Vec<(String, PathBuf)>, TemplateError> {
    if !missions_root.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(missions_root).map_err(|error| TemplateError::Scan {
        path: missions_root.to_path_buf(),
        detail: error.to_string(),
    })?;
    let mut missions: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| TemplateError::Scan {
            path: missions_root.to_path_buf(),
            detail: error.to_string(),
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        missions.push((entry_name(&path)?, path));
    }
    missions.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(missions)
}

/// Template files under one mission directory's two template subdirectories.
fn template_files(mission_dir: &Path) -> Result<Vec<PathBuf>, TemplateError> {
    let mut files: Vec<PathBuf> = Vec::new();
    for subdir in TEMPLATE_SUBDIRS {
        let template_dir = mission_dir.join(subdir);
        if !template_dir.is_dir() {
            continue;
        }
        let mut found: Vec<PathBuf> = Vec::new();
        collect_files(&template_dir, &mut found)?;
        // The reference sorts the whole match set by path text before filtering.
        found.sort();
        for path in found {
            let name = entry_name(&path)?;
            if NON_TEMPLATE_FILES.contains(&name.as_str()) {
                continue;
            }
            files.push(path);
        }
    }
    Ok(files)
}

/// Collect regular files under `directory`, recursing into real subdirectories.
///
/// A symlinked directory is not descended into (the reference's recursive glob
/// does not follow directory symlinks), while a symlink to a regular file is
/// still a template.
fn collect_files(directory: &Path, out: &mut Vec<PathBuf>) -> Result<(), TemplateError> {
    let entries = std::fs::read_dir(directory).map_err(|error| TemplateError::Scan {
        path: directory.to_path_buf(),
        detail: error.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| TemplateError::Scan {
            path: directory.to_path_buf(),
            detail: error.to_string(),
        })?;
        let path = entry.path();
        let is_real_directory = std::fs::symlink_metadata(&path)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if is_real_directory {
            collect_files(&path, out)?;
            continue;
        }
        if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// The final path component as an exact string, or a typed refusal.
fn entry_name(path: &Path) -> Result<String, TemplateError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| TemplateError::NonUtf8Name {
            path: path.to_path_buf(),
        })
}

/// Resolve a content template through the six-tier chain (live roots).
///
/// # Errors
///
/// [`TemplateError`] for a miss, a malformed org declaration or an unavailable
/// packaged missions root.
pub fn resolve_template(
    name: &str,
    project_dir: &Path,
    mission: &str,
) -> Result<ResolutionResult, TemplateError> {
    resolve_template_with_roots(name, project_dir, mission, &LiveTemplateRoots)
}

/// Resolve a content template through the six-tier chain with supplied roots.
///
/// # Errors
///
/// See [`resolve_template`].
pub fn resolve_template_with_roots(
    name: &str,
    project_dir: &Path,
    mission: &str,
    roots: &dyn TemplateRoots,
) -> Result<ResolutionResult, TemplateError> {
    resolve_asset(name, CONTENT_SUBDIR, project_dir, mission, roots)
}

/// Resolve a command template through the six-tier chain (live roots).
///
/// # Errors
///
/// See [`resolve_template`].
pub fn resolve_command(
    name: &str,
    project_dir: &Path,
    mission: &str,
) -> Result<ResolutionResult, TemplateError> {
    resolve_command_with_roots(name, project_dir, mission, &LiveTemplateRoots)
}

/// Resolve a command template through the six-tier chain with supplied roots.
///
/// # Errors
///
/// See [`resolve_template`].
pub fn resolve_command_with_roots(
    name: &str,
    project_dir: &Path,
    mission: &str,
    roots: &dyn TemplateRoots,
) -> Result<ResolutionResult, TemplateError> {
    resolve_asset(name, COMMAND_SUBDIR, project_dir, mission, roots)
}

/// The reference's six-tier probe order for one asset.
///
/// Tiers are probed in order and the first existing file wins. Org configuration
/// is consulted only after the override and legacy tiers miss, and an
/// unavailable home directory skips the two global tiers.
fn resolve_asset(
    name: &str,
    subdir: &str,
    project_dir: &Path,
    mission: &str,
    roots: &dyn TemplateRoots,
) -> Result<ResolutionResult, TemplateError> {
    let kittify = project_dir.join(".kittify");

    // Tier 1 — override, mission-scoped first, then the backward-compatible
    // global override shape.
    let mission_override = kittify
        .join("overrides")
        .join("missions")
        .join(mission)
        .join(subdir)
        .join(name);
    if mission_override.is_file() {
        return Ok(resolved(mission_override, TemplateTier::Override, mission));
    }
    let override_path = kittify.join("overrides").join(subdir).join(name);
    if override_path.is_file() {
        return Ok(resolved(override_path, TemplateTier::Override, mission));
    }

    // Tier 2 — legacy.
    let legacy = kittify.join(subdir).join(name);
    if legacy.is_file() {
        return Ok(resolved(legacy, TemplateTier::Legacy, mission));
    }

    // Tier 3 — declared org packs, first match in declaration order.
    for org_root in roots.org_roots(project_dir)? {
        let org_path = org_root
            .join("missions")
            .join(mission)
            .join(subdir)
            .join(name);
        if org_path.is_file() {
            return Ok(resolved(org_path, TemplateTier::Org, mission));
        }
    }

    // Tiers 4 and 5 — the user-global home. An unavailable home skips both.
    if let Some(global_home) = roots.kittify_home() {
        let global_mission = global_home
            .join("missions")
            .join(mission)
            .join(subdir)
            .join(name);
        if global_mission.is_file() {
            return Ok(resolved(
                global_mission,
                TemplateTier::GlobalMission,
                mission,
            ));
        }
        let global = global_home.join(subdir).join(name);
        if global.is_file() {
            return Ok(resolved(global, TemplateTier::Global, mission));
        }
    }

    // Tier 6 — the packaged default through the canonical repository paths.
    let package_root = roots.package_missions_root()?;
    let package_path = match subdir {
        COMMAND_SUBDIR => command_template_path(&package_root, mission, strip_md(name)),
        CONTENT_SUBDIR => content_template_path(&package_root, mission, name),
        other => {
            let path = package_root.join(mission).join(other).join(name);
            path.is_file().then_some(path)
        }
    };
    if let Some(path) = package_path {
        return Ok(resolved(path, TemplateTier::PackageDefault, mission));
    }

    Err(TemplateError::NotFound {
        name: name.to_owned(),
        subdir: subdir.to_owned(),
        mission: mission.to_owned(),
        project: project_dir.to_path_buf(),
    })
}

/// One resolved lookup.
fn resolved(path: PathBuf, tier: TemplateTier, mission: &str) -> ResolutionResult {
    ResolutionResult {
        path,
        tier,
        mission: Some(mission.to_owned()),
    }
}

/// Python `str.removesuffix(".md")`.
fn strip_md(name: &str) -> &str {
    name.strip_suffix(".md").unwrap_or(name)
}

/// `<missions_root>/<mission>/templates/<name>`, when it is a file.
fn content_template_path(missions_root: &Path, mission: &str, name: &str) -> Option<PathBuf> {
    let path = missions_root.join(mission).join("templates").join(name);
    path.is_file().then_some(path)
}

/// `<missions_root>/<mission>/command-templates/<stem>.md`, else the canonical
/// mission-step prompt `<missions_root>/mission-steps/<mission>/<stem>/prompt.md`.
fn command_template_path(missions_root: &Path, mission: &str, stem: &str) -> Option<PathBuf> {
    let legacy = missions_root
        .join(mission)
        .join("command-templates")
        .join(format!("{stem}.md"));
    if legacy.is_file() {
        return Some(legacy);
    }
    let step_prompt = missions_root
        .join("mission-steps")
        .join(mission)
        .join(stem)
        .join("prompt.md");
    step_prompt.is_file().then_some(step_prompt)
}

/// Resolve a `<mission>/<name>` template id through the six-tier chain.
///
/// The project root (the directory containing `.kittify/`) is taken from the
/// first tier root that carries one, exactly like the reference; without one the
/// project-scoped tiers simply miss and the global/package tiers still resolve.
/// The id is split exactly once on `/` and both halves must be non-empty; the
/// reference adds no further path grammar.
///
/// # Errors
///
/// [`TemplateError::MalformedId`] for an id without that shape, plus every
/// failure [`resolve_template`] reports.
pub fn resolve_template_by_id(
    template_id: &str,
    tier_roots: &[TierRoot],
) -> Result<ResolutionResult, TemplateError> {
    resolve_template_by_id_with_roots(template_id, tier_roots, &LiveTemplateRoots)
}

/// [`resolve_template_by_id`] with supplied roots.
///
/// # Errors
///
/// See [`resolve_template_by_id`].
pub fn resolve_template_by_id_with_roots(
    template_id: &str,
    tier_roots: &[TierRoot],
    roots: &dyn TemplateRoots,
) -> Result<ResolutionResult, TemplateError> {
    let (mission, name) = split_template_id(template_id)?;
    let project_dir = tier_roots
        .iter()
        .find_map(|root| root.project_dir.clone())
        .unwrap_or_else(|| PathBuf::from(NO_PROJECT_DIR));
    resolve_template_with_roots(name, &project_dir, mission, roots)
}

/// Split `<mission>/<name>` exactly once, requiring both halves.
fn split_template_id(template_id: &str) -> Result<(&str, &str), TemplateError> {
    match template_id.split_once('/') {
        Some((mission, name)) if !mission.is_empty() && !name.is_empty() => Ok((mission, name)),
        _ => Err(TemplateError::MalformedId {
            template_id: template_id.to_owned(),
        }),
    }
}

/// One packaged mission root plus an explicitly supplied resolver project base.
///
/// This data/Native capability can reach repository project tiers. Source's
/// include facade below replaces this base with its actual project layer.
///
/// # Errors
///
/// [`TemplateError::PackageRoot`] when the installed missions root is missing.
pub fn live_tier_roots(project_dir: &Path) -> Result<Vec<TierRoot>, TemplateError> {
    Ok(vec![
        TierRoot::new(
            TemplateTier::PackageDefault,
            LiveTemplateRoots.package_missions_root()?,
        )
        .with_project_dir(project_dir),
    ])
}

/// A rendered `template:<mission>/<name>` include.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemplateInclude {
    /// The reference's rendered text: heading, newline, template text with
    /// trailing newlines stripped.
    pub text: String,
    /// The tier the template resolved from.
    pub tier: TemplateTier,
    /// The resolved path.
    pub path: PathBuf,
}

/// Why a template include could not be rendered.
///
/// The variants exist so a caller can distinguish the reference's failure phases
/// (a `ValueError` from resolution, a `FileNotFoundError`, an `OSError` while
/// reading, and the exceptions the reference does not catch).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateIncludeError {
    /// The reference's `ValueError` while resolving, re-labelled exactly as its
    /// renderer does.
    Malformed {
        /// The selector as the operator spelled it.
        selector: String,
        /// The underlying detail.
        detail: String,
    },
    /// The reference's `FileNotFoundError` while resolving.
    NotFound {
        /// The selector as the operator spelled it.
        selector: String,
    },
    /// The reference's `OSError` while reading the resolved template.
    Unreadable {
        /// The selector as the operator spelled it.
        selector: String,
        /// The operating-system detail.
        detail: String,
    },
    /// A failure the reference's renderer does not catch (a packaged-root
    /// failure or a strict UTF-8 decode failure).
    Uncaught {
        /// The underlying detail.
        detail: String,
    },
}

impl fmt::Display for TemplateIncludeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { selector, detail } => {
                write!(
                    formatter,
                    "Malformed template selector '{selector}': {detail}"
                )
            }
            Self::NotFound { selector } => {
                write!(formatter, "No template found for selector '{selector}'.")
            }
            Self::Unreadable { selector, detail } => write!(
                formatter,
                "Template for selector '{selector}' could not be read: {detail}"
            ),
            Self::Uncaught { detail } => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for TemplateIncludeError {}

/// Render a `template:<mission>/<name>` include with the live roots.
///
/// The heading is `Template <id> (tier: <Source spelling>):` followed by the
/// template text with every trailing newline removed (`rstrip("\n")`). Reading
/// follows the reference: the file is read as bytes through the kernel's
/// Source-policy reader, decoded strictly as UTF-8 (a BOM is preserved), and
/// newline-translated exactly like `TextIOWrapper(newline=None)`.
///
/// `project_root` is an explicit resolver base. This is the data/Native
/// capability; compatibility callers use [`render_source_template_include`]
/// instead of silently correcting the reference include's project-layer base.
///
/// # Errors
///
/// [`TemplateIncludeError`] for a miss, a malformed id/org declaration, an
/// unreadable file or a failure the reference does not catch.
pub fn render_template_include(
    project_root: &Path,
    template_id: &str,
    selector: &str,
) -> Result<TemplateInclude, TemplateIncludeError> {
    // The reference builds these tier roots data before resolving, so a missing
    // packaged missions root fails before any tier can win.
    let tier_roots =
        live_tier_roots(project_root).map_err(|error| TemplateIncludeError::Uncaught {
            detail: error.to_string(),
        })?;
    render_template_from_tier_roots(template_id, selector, &tier_roots)
}

/// Source's include entry: the selected project layer, not the repository root.
///
/// An empty layer directory still wins; absent candidates leave `project_dir`
/// absent so the canonical resolver applies its existing Source fallback.
pub fn render_source_template_include(
    repo_root: &Path,
    template_id: &str,
    selector: &str,
) -> Result<TemplateInclude, TemplateIncludeError> {
    let project_layer = crate::resolve_source_project_layer(repo_root);
    let mut tier_roots =
        live_tier_roots(repo_root).map_err(|error| TemplateIncludeError::Uncaught {
            detail: error.to_string(),
        })?;
    tier_roots[0].project_dir = project_layer;
    render_template_from_tier_roots(template_id, selector, &tier_roots)
}

/// One renderer/reader and one tier resolver serve both explicit and Source bases.
fn render_template_from_tier_roots(
    template_id: &str,
    selector: &str,
    tier_roots: &[TierRoot],
) -> Result<TemplateInclude, TemplateIncludeError> {
    let resolution = resolve_template_by_id_with_roots(template_id, tier_roots, &LiveTemplateRoots)
        .map_err(|error| match error {
            TemplateError::NotFound { .. } => TemplateIncludeError::NotFound {
                selector: selector.to_owned(),
            },
            error if error.is_reference_value_error() => TemplateIncludeError::Malformed {
                selector: selector.to_owned(),
                detail: error.to_string(),
            },
            error => TemplateIncludeError::Uncaught {
                detail: error.to_string(),
            },
        })?;

    let content = read_template_text(&resolution.path).map_err(|failure| match failure {
        ReadFailure::Io(detail) => TemplateIncludeError::Unreadable {
            selector: selector.to_owned(),
            detail,
        },
        ReadFailure::NonUtf8(detail) => TemplateIncludeError::Uncaught { detail },
    })?;

    Ok(TemplateInclude {
        text: format!(
            "Template {template_id} (tier: {}):\n{}",
            resolution.tier.as_str(),
            content.trim_end_matches('\n')
        ),
        tier: resolution.tier,
        path: resolution.path,
    })
}

/// How reading a resolved template failed.
enum ReadFailure {
    /// The read itself failed.
    Io(String),
    /// The bytes are not valid UTF-8.
    NonUtf8(String),
}

/// Read one template exactly as the reference's `Path.read_text("utf-8")` does.
fn read_template_text(path: &Path) -> Result<String, ReadFailure> {
    let bytes = read_source_bytes(path).map_err(|error| ReadFailure::Io(io_detail(&error)))?;
    let text = decode_utf8(&bytes).map_err(|error| ReadFailure::NonUtf8(error.to_string()))?;
    Ok(universal_newlines(text).into_owned())
}

/// The exception detail the reference's `{exc}` interpolation would show.
///
/// The reference renders Python's `OSError` text; this reports the underlying
/// operating-system error instead of inventing a Python-formatted string.
fn io_detail(error: &sk_kernel::KernelError) -> String {
    match error {
        sk_kernel::KernelError::Io { source, .. } => source.to_string(),
        other => other.to_string(),
    }
}
