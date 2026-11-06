//! Canonical Mission definition resolution: the resolver tiers, the built-in
//! global-cache shadow guard, the legacy packaged-config distinction, and the
//! registered sparse mission-type identity.
//!
//! Source's authority is `specify_cli.mission.get_mission_by_name`, which layers
//! its own rules on `specify_cli.runtime.resolver.resolve_mission`:
//!
//! 1. `<project>/.kittify/overrides/missions/<name>/mission.yaml` — `OVERRIDE`
//! 2. `<project>/.kittify/missions/<name>/mission.yaml` — `LEGACY` (+ nudge)
//! 3. `<org pack root>/missions/<name>/mission.yaml`, in declared order — `ORG`
//! 4. `<kittify home>/missions/<name>/mission.yaml` — `GLOBAL_MISSION`, **except**
//!    that for a name the packaged tree also carries, a global hit is an artifact
//!    of the user's runtime cache and is pinned back to the packaged built-in
//! 5. the packaged missions tree — `PACKAGE_DEFAULT`, resolved to the **legacy**
//!    `specify_cli/missions/<name>` source, not to `packs/built-in/missions/`
//!
//! Nothing here invents a tier, substitutes `software-dev`, or treats a
//! `meta.json` field as configuration. A name that resolves nowhere but is a
//! registered (activated) sparse mission type gets the neutral identity Source
//! builds for it; everything else is a typed not-found error.
//!
//! The org-pack chain, the packaged tree location, the kittify home and the
//! registered-type roster are **provider ports**: they are charter/pack,
//! packaging and activation reads owned by other surfaces, and duplicating them
//! here would create a second authority (and a Mission → Doctrine cycle).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_kernel::PythonString;

use crate::definition::dialect::SafeDocumentDecoder;
use crate::definition::error::DefinitionError;
use crate::definition::model::{MissionDefinition, MissionDomain, MissionWorkflow, WorkflowPhase};
use crate::definition::value::OrderedMap;

/// The Source resolver tier a definition came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolutionTier {
    /// Project mission override (`#4088`).
    Override,
    /// Legacy project mission directory.
    Legacy,
    /// Org charter pack mission.
    Org,
    /// Global runtime cache (`~/.kittify/missions`).
    GlobalMission,
    /// Packaged built-in tree.
    PackageDefault,
}

impl ResolutionTier {
    /// The tier token.
    pub fn as_str(self) -> &'static str {
        match self {
            ResolutionTier::Override => "override",
            ResolutionTier::Legacy => "legacy",
            ResolutionTier::Org => "org",
            ResolutionTier::GlobalMission => "global-mission",
            ResolutionTier::PackageDefault => "package-default",
        }
    }
}

/// Where a resolved definition came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionOrigin {
    /// A `mission.yaml` at [`ResolutionTier`].
    MissionYaml {
        /// The tier that supplied it.
        tier: ResolutionTier,
    },
    /// The neutral in-memory identity Source builds for an activated sparse
    /// `mission_types/<id>.yaml` registration that has no `mission.yaml` at any
    /// tier. No file was read for it.
    RegisteredMissionType {
        /// The registered type id.
        id: PythonString,
    },
}

/// A registered mission type's authored facts, as the canonical
/// charter/offering surface knows them. This is the only shape the neutral
/// identity needs; it is deliberately not a second mission-type model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredMissionType {
    /// The registered id (must equal the name being resolved).
    pub id: PythonString,
    /// Human display name.
    pub display_name: PythonString,
    /// Declared action sequence, when the registration carries one.
    pub action_sequence: Option<Vec<PythonString>>,
    /// Declared path conventions, when the registration carries any.
    pub path_conventions: Option<OrderedMap<PythonString>>,
}

/// Cross-surface reads the resolver needs and does not own.
pub trait DefinitionProviders {
    /// Org pack roots in declared precedence order, already existing-filtered
    /// (Source `resolve_pack_chain(strict=False)`). Absent roots are dropped, not
    /// invented.
    fn org_pack_roots(&self, project_root: &Path) -> Vec<PathBuf>;

    /// The registered mission type for `name`, or `None` when the project has not
    /// activated it / no layer has a loadable registration.
    fn registered_mission_type(
        &self,
        project_root: &Path,
        name: &str,
    ) -> Option<RegisteredMissionType>;

    /// The packaged missions tree (Source `_packaged_missions_dir()`), when this
    /// installation has one.
    fn packaged_missions_root(&self, project_root: &Path) -> Option<PathBuf>;

    /// The global kittify home (Source `get_kittify_home()`), when configured.
    fn kittify_home(&self, project_root: &Path) -> Option<PathBuf>;
}

/// A provider set that resolves only what exists inside the project itself.
///
/// Production composition supplies the charter-pack, packaging and activation
/// providers; this is the honest empty composition (no org packs, no packaged
/// tree, no global home, no registered types).
#[derive(Clone, Copy, Debug, Default)]
pub struct ProjectOnlyProviders;

impl DefinitionProviders for ProjectOnlyProviders {
    fn org_pack_roots(&self, _project_root: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    fn registered_mission_type(
        &self,
        _project_root: &Path,
        _name: &str,
    ) -> Option<RegisteredMissionType> {
        None
    }

    fn packaged_missions_root(&self, _project_root: &Path) -> Option<PathBuf> {
        None
    }

    fn kittify_home(&self, _project_root: &Path) -> Option<PathBuf> {
        None
    }
}

/// A provider set for a source checkout: the packaged tree Source reads is
/// `<repo>/src/specify_cli/missions`, and the global home is `$HOME/.kittify`.
///
/// The packaged root is a *checkout* location, so this composition is for the
/// development/test harness and for a caller that knows it runs from a checkout.
/// Org packs and registered types stay unwired here.
#[derive(Clone, Debug)]
pub struct CheckoutProviders {
    repo_root: PathBuf,
    home: Option<PathBuf>,
}

impl CheckoutProviders {
    /// Providers anchored at `repo_root`, reading `HOME` for the kittify home.
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }

    /// Providers with an explicit global home (`None` disables the global tier).
    pub fn with_home(repo_root: impl Into<PathBuf>, home: Option<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            home,
        }
    }
}

impl DefinitionProviders for CheckoutProviders {
    fn org_pack_roots(&self, _project_root: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    fn registered_mission_type(
        &self,
        _project_root: &Path,
        _name: &str,
    ) -> Option<RegisteredMissionType> {
        None
    }

    fn packaged_missions_root(&self, _project_root: &Path) -> Option<PathBuf> {
        let root = self
            .repo_root
            .join("src")
            .join("specify_cli")
            .join("missions");
        root.is_dir().then_some(root)
    }

    fn kittify_home(&self, _project_root: &Path) -> Option<PathBuf> {
        self.home.as_ref().map(|home| home.join(".kittify"))
    }
}

/// A resolved Mission definition with its provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedMissionDefinition {
    /// The validated definition.
    pub definition: MissionDefinition,
    /// Where it came from.
    pub origin: DefinitionOrigin,
    /// The tier, when a `mission.yaml` was read.
    pub tier: Option<ResolutionTier>,
    /// The mission directory (`<...>/missions/<name>`); a descriptive marker for
    /// a registered-type identity, exactly as Source treats it.
    pub mission_dir: PathBuf,
    /// The `mission.yaml` that was read, if any.
    pub config_path: Option<PathBuf>,
    /// Warnings Source emitted while resolving/validating (legacy nudge,
    /// unknown path keys).
    pub warnings: Vec<PythonString>,
}

impl ResolvedMissionDefinition {
    /// The mission-type slug Source derives for the project-override route:
    /// the last segment of a `<...>/missions/<type>` path, else `None`.
    pub fn mission_type_slug(&self) -> Option<&str> {
        if self.mission_dir.parent()?.file_name()? != "missions" {
            return None;
        }
        self.mission_dir.file_name()?.to_str()
    }
}

/// Upper bound for a `mission.yaml` read.
///
/// Source reads the file unbounded; the bound exists so a hostile or accidental
/// huge file is refused instead of loaded, and is far above any real definition.
pub const DEFAULT_MISSION_CONFIG_BYTE_LIMIT: u64 = 16 * 1024 * 1024;

/// Load and validate one `mission.yaml` file.
pub fn load_mission_definition(
    config_path: &Path,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<crate::definition::model::LoadedDefinition, DefinitionError> {
    if !config_path.is_file() {
        return Err(DefinitionError::MissionConfigNotFound {
            path: config_path.to_path_buf(),
        });
    }
    let bytes = sk_kernel::read_bounded(config_path, DEFAULT_MISSION_CONFIG_BYTE_LIMIT).map_err(
        |source| DefinitionError::ReadFailed {
            path: config_path.to_path_buf(),
            detail: source.to_string(),
        },
    )?;
    // Source's own read policy for `mission.yaml`: `open(path)` decodes strictly
    // as UTF-8 (so an undecodable byte is CPython's `UnicodeDecodeError`) and
    // translates universal newlines; PyYAML's Reader then strips one leading BOM
    // code point. The shared Kernel codec owns both phases.
    let document_text = sk_kernel::python_yaml::read_document_text(
        &bytes,
        sk_kernel::python_yaml::Dialect::PyYamlSafe11,
    )
    .map_err(|error| DefinitionError::InvalidYaml {
        path: config_path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let document = decoder
        .decode(config_path, &document_text)
        .map_err(|detail| DefinitionError::InvalidYaml {
            path: config_path.to_path_buf(),
            detail,
        })?;
    MissionDefinition::from_document(config_path, &document)
}

/// Resolve a Mission definition by mission-type name.
///
/// See the module documentation for the tier order and the two Source-specific
/// rules (global-cache shadow guard, legacy packaged tree).
pub fn resolve_mission_definition(
    name: &str,
    project_root: &Path,
    providers: &dyn DefinitionProviders,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<ResolvedMissionDefinition, DefinitionError> {
    let kittify = project_root.join(".kittify");

    let override_path = kittify
        .join("overrides")
        .join("missions")
        .join(name)
        .join("mission.yaml");
    if override_path.is_file() {
        return load_at(override_path, ResolutionTier::Override, decoder, Vec::new());
    }

    let legacy_path = kittify.join("missions").join(name).join("mission.yaml");
    if legacy_path.is_file() {
        let mut warnings = Vec::new();
        warnings.push(python_string(&format!(
            "Legacy asset resolved: {} — run 'spec-kitty migrate' to clean up. \
             Legacy resolution will be removed in the next major version.",
            legacy_path.display()
        )));
        return load_at(legacy_path, ResolutionTier::Legacy, decoder, warnings);
    }

    for org_root in providers.org_pack_roots(project_root) {
        let org_path = org_root.join("missions").join(name).join("mission.yaml");
        if org_path.is_file() {
            return load_at(org_path, ResolutionTier::Org, decoder, Vec::new());
        }
    }

    let packaged = providers
        .packaged_missions_root(project_root)
        .filter(|root| mission_dir_is_valid(&root.join(name)));
    let packaged_valid = packaged.is_some();

    if let Some(home) = providers.kittify_home(project_root) {
        let global_path = home.join("missions").join(name).join("mission.yaml");
        if global_path.is_file() {
            // A GLOBAL_MISSION hit for a name the packaged tree also carries is
            // the user's runtime cache shadowing the built-in: pin it back to the
            // packaged built-in (Source's NFR-001 guard).
            if !packaged_valid {
                return load_at(
                    global_path,
                    ResolutionTier::GlobalMission,
                    decoder,
                    Vec::new(),
                );
            }
            let packaged_root = packaged.expect("checked above");
            return load_at(
                packaged_root.join(name).join("mission.yaml"),
                ResolutionTier::PackageDefault,
                decoder,
                Vec::new(),
            );
        }
    }

    if let Some(packaged_root) = packaged {
        return load_at(
            packaged_root.join(name).join("mission.yaml"),
            ResolutionTier::PackageDefault,
            decoder,
            Vec::new(),
        );
    }

    if let Some(registered) = providers.registered_mission_type(project_root, name) {
        return Ok(neutral_registered_definition(name, &registered, &kittify));
    }

    Err(DefinitionError::MissionNotFound {
        name: python_string(name),
        available: available_missions(project_root, providers),
    })
}

/// Resolve the definition for a Mission directory's recorded mission type.
///
/// Source `get_mission_for_feature`: read the canonical `mission_type` field from
/// the Mission's `meta.json`; a typeless / blank / non-string / unreadable meta
/// is the neutral empty key, which template *selection* coalesces to
/// `software-dev` (C-006). The legacy `mission` field is not consulted.
pub fn resolve_mission_definition_for_mission_dir(
    mission_dir: &Path,
    project_root: &Path,
    providers: &dyn DefinitionProviders,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<ResolvedMissionDefinition, DefinitionError> {
    let name = recorded_mission_type(mission_dir).unwrap_or_else(|| "software-dev".to_owned());
    resolve_mission_definition(&name, project_root, providers, decoder)
}

/// The canonical `mission_type` recorded in a Mission's `meta.json`.
///
/// Mirrors `charter.mission_type_key.read_mission_type` + `mission_metadata.load_meta_or_empty`:
/// only a non-empty, trimmed **string** in `mission_type` counts; every read
/// failure degrades to `None` (Source is deliberately silent about a corrupt
/// `meta.json` on this path).
pub fn recorded_mission_type(mission_dir: &Path) -> Option<String> {
    let meta_path = mission_dir.join(crate::meta::META_FILE_NAME);
    let meta =
        crate::meta::load_meta_object(&meta_path, crate::meta::DEFAULT_META_BYTE_LIMIT).ok()?;
    match meta.get("mission_type") {
        Some(serde_json::Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        }
        _ => None,
    }
}

fn load_at(
    config_path: PathBuf,
    tier: ResolutionTier,
    decoder: &dyn SafeDocumentDecoder,
    mut warnings: Vec<PythonString>,
) -> Result<ResolvedMissionDefinition, DefinitionError> {
    let mission_dir = config_path.parent().map(Path::to_path_buf).ok_or_else(|| {
        DefinitionError::MissionConfigNotFound {
            path: config_path.clone(),
        }
    })?;
    let loaded = load_mission_definition(&config_path, decoder)?;
    warnings.extend(loaded.warnings.iter().cloned());
    Ok(ResolvedMissionDefinition {
        definition: loaded.definition,
        origin: DefinitionOrigin::MissionYaml { tier },
        tier: Some(tier),
        mission_dir,
        config_path: Some(config_path),
        warnings,
    })
}

fn mission_dir_is_valid(dir: &Path) -> bool {
    dir.is_dir() && dir.join("mission.yaml").is_file()
}

/// Source's neutral identity for an activated sparse mission-type registration.
///
/// The identity is the type's own (`name` = `display_name`), the conventions are
/// the type's own `path_conventions` slot (empty when it declares none — so path
/// checking no-ops instead of applying software-dev's `src/`/`tests/`), the
/// artifacts are empty, and the workflow is a single phase derived from the
/// declared `action_sequence`. Nothing is substituted from another mission.
fn neutral_registered_definition(
    name: &str,
    registered: &RegisteredMissionType,
    kittify: &Path,
) -> ResolvedMissionDefinition {
    let description = python_string(&format!(
        "Org-registered mission type '{}' -- no mission.yaml exists for it at any resolver tier, \
         so this identity is built from its sparse mission_types/ registration with neutral \
         conventions (no software-dev path/artifact assumptions apply).",
        name
    ));
    let phase_description = match &registered.action_sequence {
        Some(sequence) if !sequence.is_empty() => {
            let mut text = String::from("Derived from the mission type's action_sequence: ");
            for (index, step) in sequence.iter().enumerate() {
                if index > 0 {
                    text.push_str(" -> ");
                }
                text.push_str(&crate::definition::error::escape_codepoints(step));
            }
            text
        }
        _ => "Org mission type declares no action_sequence; no workflow phases are implied."
            .to_owned(),
    };
    let definition = MissionDefinition {
        name: registered.display_name.clone(),
        description,
        version: python_string("0.0.0"),
        domain: MissionDomain::Other,
        workflow: MissionWorkflow {
            phases: vec![WorkflowPhase {
                name: python_string("org-mission-type"),
                description: python_string(&phase_description),
            }],
        },
        artifacts: crate::definition::model::MissionArtifacts::default(),
        paths: registered.path_conventions.clone().unwrap_or_default(),
        validation: crate::definition::model::MissionValidation::default(),
        mcp_tools: None,
        agent_context: None,
        task_metadata: None,
        commands: None,
        task_types: None,
        path_key_warnings: Vec::new(),
    };
    ResolvedMissionDefinition {
        definition,
        origin: DefinitionOrigin::RegisteredMissionType {
            id: registered.id.clone(),
        },
        tier: None,
        mission_dir: kittify.join("missions").join(name),
        config_path: None,
        warnings: Vec::new(),
    }
}

/// The names Source lists in its not-found message: project mission directories
/// and packaged ones that actually contain a `mission.yaml`, deduplicated and
/// sorted by code point.
pub fn available_missions(
    project_root: &Path,
    providers: &dyn DefinitionProviders,
) -> Vec<PythonString> {
    let mut names: BTreeSet<Vec<u32>> = BTreeSet::new();
    let project_missions = project_root.join(".kittify").join("missions");
    collect_mission_names(&project_missions, &mut names);
    if let Some(packaged) = providers.packaged_missions_root(project_root) {
        collect_mission_names(&packaged, &mut names);
    }
    names
        .into_iter()
        .map(|units| PythonString::from_codepoints(&units).expect("valid code points"))
        .collect()
}

fn collect_mission_names(dir: &Path, names: &mut BTreeSet<Vec<u32>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !mission_dir_is_valid(&path) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        names.insert(name.chars().map(u32::from).collect());
    }
}

fn python_string(text: &str) -> PythonString {
    PythonString::from_scalars(text)
}
