//! Project-governance resolution (`charter.activation.resolver`).
//!
//! One owner for the reference's `resolve_project_governance`:
//!
//! * the canonical charter read root (governance/directives sections) stays
//!   distinct from the activation repository root (ledger, identity, org floor);
//! * paradigms and directives use the three-state activation list (absent /
//!   explicit empty / explicit ids), union the charter's `selected_*` layer in
//!   first-occurrence order and drop unknown selections with a typed diagnostic;
//! * tools are the sorted union of the runtime registry baseline and the
//!   charter's `available_tools` (an empty/absent registry means the defaults);
//! * the template set is validated only when the catalogue's `template_sets`
//!   domain is present, otherwise the reference's fallback string applies;
//! * the effective deny applies after every union.
//!
//! The charter-config reader and the directive selection/diagnostic policy are
//! shared with [`crate::context`]'s action build — this module is the single
//! owner, not a second directive/catalogue authority. The compact summary is a
//! caller-specific projection of this result ([`compact_governance_summary`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;
use sk_kernel::PythonString;

use crate::activation::ActivationState;
use crate::context::{Diagnostic, DiagnosticCode};
use crate::context_render::CompactGovernance;
use crate::identity::IdentityIndex;
use crate::ledger::{ActivationLedger, DenySet, ExactDenySet, LedgerToken, load_activation_ledger};
use crate::org::normalize_directive_id;
use crate::org_charter::required_floor;

/// The reference `DEFAULT_TEMPLATE_SET`.
pub const DEFAULT_TEMPLATE_SET: &str = "software-dev-default";

/// The reference `DEFAULT_TOOL_REGISTRY` (`frozenset({"spec-kitty", "git"})`).
pub const DEFAULT_TOOL_REGISTRY: [&str; 2] = ["spec-kitty", "git"];

/// The migration runbook named by a retired pack-field rejection.
const RETIRED_FIELD_RUNBOOK: &str = "docs/migrations/charter-pack-cutover.md";

/// The reference `UNKNOWN_SELECTION_DIAGNOSTIC` message template.
fn unknown_selection_message(kind: &str, token: &str) -> String {
    format!(
        "Unknown selected {kind} '{token}' dropped: not found in built-in, org packs \
         (charter_packs.org.packs) or .kittify/charter-packs/; see 'spec-kitty charter list'."
    )
}

/// The reference `_TEMPLATE_SET_REMEDIATION` sentence.
const TEMPLATE_SET_REMEDIATION: &str = "Update charter template_set to one of the available template sets, \
     or provide it through a project override (see 'spec-kitty charter list --show-available').";

/// The reference `GovernanceResolutionError`.
///
/// The display text is the reference's `ValueError` message: the ordered issues,
/// each prefixed with `- `, under `Governance resolution failed:`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernanceResolutionError {
    issues: Vec<String>,
}

impl GovernanceResolutionError {
    /// Build the refusal from its ordered issues.
    #[must_use]
    pub fn new(issues: Vec<String>) -> Self {
        Self { issues }
    }

    /// The ordered issue lines.
    #[must_use]
    pub fn issues(&self) -> &[String] {
        &self.issues
    }
}

impl std::fmt::Display for GovernanceResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Governance resolution failed:\n- {}",
            self.issues.join("\n- ")
        )
    }
}

impl std::error::Error for GovernanceResolutionError {}

/// A typed project-governance failure.
///
/// Only [`Self::Resolution`] is the reference's `GovernanceResolutionError`.
/// The other variants are the reference's separate exception families (charter
/// config reads, retired pack fields, pack-root discovery), kept distinct so a
/// caller cannot turn a transport/config failure into a healthy resolution and
/// the compact summary cannot treat every direct-resolver error as "unresolved".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectGovernanceError {
    /// The reference `GovernanceResolutionError` (for example an unavailable
    /// template set).
    Resolution(GovernanceResolutionError),
    /// The retired `governance.doctrine` selection key (`ActiveCharterConfigError`).
    RetiredGovernanceSelection {
        /// The authored charter file.
        path: PathBuf,
    },
    /// A retired pack field (`RetiredPackFieldError`); the compact summary
    /// re-raises this instead of degrading.
    RetiredPackField {
        /// The file that carries the retired field.
        path: PathBuf,
        /// The retired field name.
        field: String,
        /// Its replacement.
        replacement: String,
    },
    /// A charter-config read/validation failure (the reference's broad
    /// config/pydantic exception family).
    Config {
        /// The reference-shaped cause.
        message: String,
    },
    /// A catalogue/root discovery failure (pack root or missions root).
    Catalogue {
        /// The reference-shaped cause.
        message: String,
    },
}

impl std::fmt::Display for ProjectGovernanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolution(error) => write!(formatter, "{error}"),
            Self::RetiredGovernanceSelection { path } => write!(
                formatter,
                "{}: `governance.doctrine` is the retired selection key and is no longer read. \
                 Run `spec-kitty upgrade` to rewrite it as `governance.charter`. \
                 See {RETIRED_FIELD_RUNBOOK}.",
                path.display()
            ),
            Self::RetiredPackField {
                path,
                field,
                replacement,
            } => write!(
                formatter,
                "{}: field '{field}' was removed. Replacement: {replacement}. See {RETIRED_FIELD_RUNBOOK}.",
                path.display()
            ),
            Self::Config { message } | Self::Catalogue { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ProjectGovernanceError {}

impl From<GovernanceResolutionError> for ProjectGovernanceError {
    fn from(error: GovernanceResolutionError) -> Self {
        Self::Resolution(error)
    }
}

fn config_error(error: impl std::fmt::Display) -> ProjectGovernanceError {
    ProjectGovernanceError::Config {
        message: error.to_string(),
    }
}

/// The reference `GovernanceResolution` value (project + org selection surface).
///
/// `tactics`/`styleguides`/`toolguides`/`procedures`/`assets` are always empty
/// for the project function, and `profile_id`/`role` are `None`, exactly as the
/// reference constructs them here. Metadata keeps the reference's insertion
/// order (`tools_source`, `directives_source`, `template_set_source`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GovernanceResolution {
    /// Sorted resolved paradigms (activation base + charter selection, denied last).
    pub paradigms: Vec<String>,
    /// Resolved directive ids in first-occurrence order (base, charter selection,
    /// project-local), denied last.
    pub directives: Vec<String>,
    /// Sorted tool union.
    pub tools: Vec<String>,
    /// The resolved template set or the fallback string.
    pub template_set: String,
    /// Ordered provenance map (`tools_source`, `directives_source`,
    /// `template_set_source`).
    pub metadata: Vec<(String, String)>,
    /// Empty in the project function.
    pub tactics: Vec<String>,
    /// Empty in the project function.
    pub styleguides: Vec<String>,
    /// Empty in the project function.
    pub toolguides: Vec<String>,
    /// Empty in the project function.
    pub procedures: Vec<String>,
    /// Empty in the project function.
    pub assets: Vec<String>,
    /// `None` in the project function.
    pub profile_id: Option<String>,
    /// `None` in the project function.
    pub role: Option<String>,
    /// Typed diagnostics in producer order.
    pub diagnostics: Vec<Diagnostic>,
}

impl GovernanceResolution {
    /// The provenance value for *key*, if present.
    #[must_use]
    pub fn metadata_value(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The diagnostic messages in producer order (the compact summary projection).
    #[must_use]
    pub fn diagnostic_messages(&self) -> Vec<String> {
        self.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect()
    }
}

/// The catalogue facts the resolver validates against.
///
/// The reference's `OfferingCatalog` is narrowed to the three domains this
/// resolver reads: paradigms, directives and template sets, each with its
/// `domains_present` flag. Built-in declared ids are read through the canonical
/// identity scan; template sets are the reference's mission directories that
/// contain `mission.yaml`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GovernanceCatalogue {
    /// Built-in paradigm declared ids (file stem fallback included).
    pub paradigms: BTreeSet<String>,
    /// Built-in directive declared ids (not yet normalised).
    pub directives: BTreeSet<String>,
    /// `<mission>-default` for every mission directory carrying `mission.yaml`.
    pub template_sets: BTreeSet<String>,
    /// The reference's `domains_present` set.
    pub domains_present: BTreeSet<String>,
}

impl GovernanceCatalogue {
    /// Assemble catalogue facts directly (tests and callers that already read them).
    #[must_use]
    pub fn new(
        paradigms: BTreeSet<String>,
        directives: BTreeSet<String>,
        template_sets: BTreeSet<String>,
        domains_present: BTreeSet<String>,
    ) -> Self {
        Self {
            paradigms,
            directives,
            template_sets,
            domains_present,
        }
    }

    /// Read the built-in catalogue from the canonical pack root.
    ///
    /// # Errors
    ///
    /// A missing built-in pack root or missions root (the reference raises
    /// `PackRootNotFound` / `MissionsRootNotFound`; the compact summary treats
    /// that as "governance unavailable", never as a resolved empty catalogue).
    pub fn load(repo_root: &Path) -> Result<Self, ProjectGovernanceError> {
        let built_in = crate::org_charter::built_in_search(repo_root)
            .find_root()
            .map_err(|error| ProjectGovernanceError::Catalogue {
                message: format!("no pack root could be resolved for built-in: {error}"),
            })?;
        let identity = IdentityIndex::from_roots(&built_in, &[], None);
        let paradigms = built_in_ids(&identity, ArtifactKind::Paradigm);
        let directives = built_in_ids(&identity, ArtifactKind::Directive);
        let missions = sk_kernel::get_package_asset_root().map_err(|error| {
            ProjectGovernanceError::Catalogue {
                message: error.to_string(),
            }
        })?;
        let template_sets = mission_template_sets(&missions)?;
        let mut domains_present = BTreeSet::new();
        if built_in.join("paradigms").is_dir() {
            domains_present.insert("paradigms".to_owned());
        }
        if built_in.join("directives").is_dir() {
            domains_present.insert("directives".to_owned());
        }
        // The reference's `_load_template_sets_with_presence`: the missions root
        // must exist (already proven by `get_package_asset_root`), so the
        // template-sets domain is present even when it is empty.
        domains_present.insert("template_sets".to_owned());
        Ok(Self::new(
            paradigms,
            directives,
            template_sets,
            domains_present,
        ))
    }

    fn has_domain(&self, domain: &str) -> bool {
        self.domains_present.contains(domain)
    }
}

/// The built-in declared ids of one kind, through the canonical identity scan.
fn built_in_ids(identity: &IdentityIndex, kind: ArtifactKind) -> BTreeSet<String> {
    identity
        .stems(kind)
        .into_iter()
        .filter_map(|stem| identity.declared_id(kind, &stem))
        .collect()
}

/// The reference `MissionTemplateRepository.list_missions()` projection:
/// sorted child directory names that contain a `mission.yaml`.
fn mission_template_sets(missions_root: &Path) -> Result<BTreeSet<String>, ProjectGovernanceError> {
    let entries =
        std::fs::read_dir(missions_root).map_err(|error| ProjectGovernanceError::Catalogue {
            message: format!("{}: {error}", missions_root.display()),
        })?;
    let mut names = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| ProjectGovernanceError::Catalogue {
            message: format!("{}: {error}", missions_root.display()),
        })?;
        let path = entry.path();
        if !path.is_dir() || !path.join("mission.yaml").exists() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        names.insert(format!("{name}-default"));
    }
    Ok(names)
}

/// Options for [`resolve_project_governance_with`].
///
/// Defaults mirror the reference's keyword defaults: no tool registry override,
/// `software-dev-default`, no injected identity/catalogue and no explicit read
/// scope (the canonical Git root).
#[derive(Clone, Debug, Default)]
pub struct ProjectGovernanceOptions {
    /// The caller's runtime tool registry (empty is treated like absent).
    pub tool_registry: Option<BTreeSet<String>>,
    /// The fallback template set; `None` uses [`DEFAULT_TEMPLATE_SET`].
    pub fallback_template_set: Option<String>,
    /// An already-built identity index.
    pub identity: Option<IdentityIndex>,
    /// An already-read catalogue.
    pub catalogue: Option<GovernanceCatalogue>,
    /// An explicitly resolved charter read root (the reference's `scope.root`).
    pub read_root: Option<PathBuf>,
}

/// Resolve project + org governance selections (reference defaults).
///
/// # Errors
///
/// See [`ProjectGovernanceError`].
pub fn resolve_project_governance(
    repo_root: &Path,
) -> Result<GovernanceResolution, ProjectGovernanceError> {
    resolve_project_governance_with(repo_root, &ProjectGovernanceOptions::default())
}

/// Resolve project + org governance selections with explicit inputs.
///
/// The reference order is preserved: canonical charter root, `governance:`
/// section, `directives:` section, offering catalogue, activation ledger,
/// identity index, org-required floor and effective deny. The activation root is
/// `repo_root` even when an explicit `read_root` selects a different charter.
///
/// # Errors
///
/// See [`ProjectGovernanceError`].
pub fn resolve_project_governance_with(
    repo_root: &Path,
    options: &ProjectGovernanceOptions,
) -> Result<GovernanceResolution, ProjectGovernanceError> {
    let read_root = match &options.read_root {
        Some(root) => root.clone(),
        None => crate::charter_sync::charter_read_root(repo_root, None).map_err(config_error)?,
    };
    let sections = read_charter_sections(&read_root)?;
    let charter_path = sections.path.clone();
    if !sections.governance_valid {
        return Err(ProjectGovernanceError::Config {
            message: format!(
                "{}: invalid governance section for GovernanceConfig",
                charter_path.display()
            ),
        });
    }
    if !sections.directives_valid {
        return Err(ProjectGovernanceError::Config {
            message: format!(
                "{}: invalid directives section for DirectivesConfig",
                charter_path.display()
            ),
        });
    }
    let catalogue = match &options.catalogue {
        Some(catalogue) => catalogue.clone(),
        None => GovernanceCatalogue::load(repo_root)?,
    };
    let ledger = load_activation_ledger(repo_root).map_err(config_error)?;
    let identity = match &options.identity {
        Some(identity) => identity.clone(),
        None => IdentityIndex::from_project(repo_root).map_err(config_error)?,
    };
    let floor = required_floor(repo_root, &identity)
        .map_err(config_error)?
        .into_map();
    let state = ActivationState::from_ledger(&ledger, &identity, &floor).map_err(config_error)?;

    let charter_config = governance_charter_config(&sections.charter);
    let mut diagnostics = Vec::new();

    let selected_paradigms_raw = resolve_selected_paradigms(
        &charter_config.scalar_selected_paradigms(),
        &catalogue,
        &identity,
        &mut diagnostics,
    );
    let (paradigm_base, paradigm_base_source) = resolve_paradigm_base(
        &catalogue,
        &ledger,
        &identity,
        state.deny_for(ArtifactKind::Paradigm),
        &mut diagnostics,
    );
    report_charter_selection(
        "paradigm",
        &selected_paradigms_raw,
        &paradigm_base,
        &paradigm_base_source,
        &mut diagnostics,
    );
    let mut selected_paradigms = paradigm_base;
    for id in selected_paradigms_raw {
        if !selected_paradigms.contains(&id) {
            selected_paradigms.push(id);
        }
    }

    let (tools, tools_source) = resolve_tools_selection(
        &charter_config.available_tools,
        options.tool_registry.as_ref(),
        &mut diagnostics,
    );

    let catalog_ids: BTreeSet<String> = catalogue
        .directives
        .iter()
        .map(|id| normalize_directive_id(id))
        .collect();
    let (directives, directives_source) = match resolve_directive_selection(
        &sections,
        &catalog_ids,
        &identity,
        &ledger,
        state.deny_for(ArtifactKind::Directive),
    ) {
        DirectiveSelectionOutcome::Resolved {
            ids,
            source,
            diagnostics: mut selection_diagnostics,
        } => {
            diagnostics.append(&mut selection_diagnostics);
            (ids, source)
        }
        DirectiveSelectionOutcome::Degraded { .. } => {
            return Err(ProjectGovernanceError::Config {
                message: format!(
                    "{}: invalid governance or directives section",
                    charter_path.display()
                ),
            });
        }
    };

    let fallback_template_set = options
        .fallback_template_set
        .clone()
        .unwrap_or_else(|| DEFAULT_TEMPLATE_SET.to_owned());
    let (template_set, template_set_source) = resolve_template_set_selection(
        charter_config.template_set.as_deref(),
        &catalogue,
        &fallback_template_set,
        &mut diagnostics,
    )?;

    // The effective deny applies after every union; it never holds a required id.
    if let Some(deny) = state.deny_for(ArtifactKind::Paradigm) {
        selected_paradigms.retain(|id| deny.admits(id));
    }

    Ok(GovernanceResolution {
        paradigms: selected_paradigms,
        directives,
        tools,
        template_set,
        metadata: vec![
            ("tools_source".to_owned(), tools_source),
            ("directives_source".to_owned(), directives_source),
            ("template_set_source".to_owned(), template_set_source),
        ],
        tactics: Vec::new(),
        styleguides: Vec::new(),
        toolguides: Vec::new(),
        procedures: Vec::new(),
        assets: Vec::new(),
        profile_id: None,
        role: None,
        diagnostics,
    })
}

/// The compact caller's projection of [`GovernanceResolution`].
///
/// The reference's `_resolve_governance_summary` catch policy:
///
/// * `GovernanceResolutionError` -> the `(none)` labels plus a
///   `governance unresolved (…)` diagnostic;
/// * `RetiredPackFieldError` -> re-raised (returned as `Err` here);
/// * every other reference exception -> the `(none)` labels plus a
///   `governance unavailable (…)` diagnostic.
///
/// The direct resolver never applies these catches.
///
/// # Errors
///
/// Only [`ProjectGovernanceError::RetiredPackField`]: every other failure is
/// folded into the degraded summary, exactly as the reference does.
pub fn compact_governance_summary(
    repo_root: &Path,
) -> Result<CompactGovernance, ProjectGovernanceError> {
    compact_governance_summary_with(repo_root, None)
}

/// [`compact_governance_summary`] with an optional explicit read root.
///
/// # Errors
///
/// Only [`ProjectGovernanceError::RetiredPackField`].
pub fn compact_governance_summary_with(
    repo_root: &Path,
    read_root: Option<&Path>,
) -> Result<CompactGovernance, ProjectGovernanceError> {
    let options = ProjectGovernanceOptions {
        read_root: read_root.map(Path::to_path_buf),
        ..ProjectGovernanceOptions::default()
    };
    match resolve_project_governance_with(repo_root, &options) {
        Ok(resolution) => {
            let diagnostics = resolution.diagnostic_messages();
            Ok(CompactGovernance {
                template_set: Some(resolution.template_set),
                paradigms: resolution.paradigms,
                tools: resolution.tools,
                diagnostics,
                resolver_directive_ids: resolution.directives,
            })
        }
        Err(error @ ProjectGovernanceError::RetiredPackField { .. }) => Err(error),
        Err(ProjectGovernanceError::Resolution(error)) => {
            Ok(degraded_summary(format!("governance unresolved ({error})")))
        }
        Err(error) => Ok(degraded_summary(format!(
            "governance unavailable ({error})"
        ))),
    }
}

fn degraded_summary(diagnostic: String) -> CompactGovernance {
    CompactGovernance {
        template_set: None,
        paradigms: Vec::new(),
        tools: Vec::new(),
        diagnostics: vec![diagnostic],
        resolver_directive_ids: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Charter-config read (shared with the action build)
// ---------------------------------------------------------------------------

/// The authored charter document, distinguishing "absent" from "unparseable".
#[derive(Clone, Debug)]
pub(crate) enum CharterDocument {
    /// No charter file (or the path could not be resolved inside the root).
    Absent,
    /// A parsed document (any YAML shape; non-mapping roots read as no section).
    Parsed(Value),
    /// The file exists but could not be read or parsed.
    Unreadable {
        /// The read/parse failure text.
        message: String,
    },
}

/// The authored charter sections the reference's charter-config readers see.
#[derive(Clone, Debug)]
pub(crate) struct CharterConfigSections {
    /// The parsed `charter.yaml` document, when a read succeeded.
    pub(crate) charter: Option<Value>,
    /// Whether `governance:` passes the reference `GovernanceConfig` shape.
    pub(crate) governance_valid: bool,
    /// Whether `directives:` passes the reference `DirectivesConfig` shape.
    pub(crate) directives_valid: bool,
    /// The resolved (or intended) charter file path, for error messages.
    pub(crate) path: PathBuf,
}

impl CharterConfigSections {
    /// The directly-loaded local directives, when that read succeeded.
    pub(crate) fn local_directives(&self) -> Vec<LocalDirective> {
        if !self.directives_valid {
            return Vec::new();
        }
        local_directives(&self.charter)
    }
}

/// A read failure the callers classify differently.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CharterSectionsError {
    /// The file exists but could not be read or parsed (the action build
    /// degrades; the project resolver and compact summary do not pretend a
    /// resolved catalogue).
    Unreadable {
        /// The resolved path.
        path: PathBuf,
        /// The cause.
        message: String,
    },
    /// The retired `governance.doctrine` selection key (both callers refuse).
    RetiredGovernanceSelection {
        /// The authored charter file.
        path: PathBuf,
    },
    /// A retired pack field (the project resolver refuses; the compact summary
    /// re-raises; the action build's broad catch degrades).
    RetiredPackField {
        /// The file that carries the retired field.
        path: PathBuf,
        /// The retired field name.
        field: String,
        /// Its replacement.
        replacement: String,
    },
}

impl From<CharterSectionsError> for ProjectGovernanceError {
    fn from(error: CharterSectionsError) -> Self {
        match error {
            CharterSectionsError::Unreadable { path, message } => Self::Config {
                message: format!("{}: {message}", path.display()),
            },
            CharterSectionsError::RetiredGovernanceSelection { path } => {
                Self::RetiredGovernanceSelection { path }
            }
            CharterSectionsError::RetiredPackField {
                path,
                field,
                replacement,
            } => Self::RetiredPackField {
                path,
                field,
                replacement,
            },
        }
    }
}

/// Read the authored charter sections for a root.
pub(crate) fn read_charter_sections(
    root: &Path,
) -> Result<CharterConfigSections, CharterSectionsError> {
    let (document, path) = read_charter_document(root);
    let charter = match document {
        CharterDocument::Absent => None,
        CharterDocument::Parsed(document) => Some(document),
        CharterDocument::Unreadable { message } => {
            return Err(CharterSectionsError::Unreadable { path, message });
        }
    };
    require_canonical_governance(&charter, &path)?;
    reject_retired_activation_fields(&charter, &path)?;
    Ok(CharterConfigSections {
        governance_valid: governance_section_valid(&charter),
        directives_valid: directives_section_valid(&charter),
        charter,
        path,
    })
}

/// The authored charter document plus the path the read targeted.
pub(crate) fn read_charter_document(root: &Path) -> (CharterDocument, PathBuf) {
    // Source probes exists then opens the lexical path ordinarily. It follows
    // external symlinks, and a directory/unignored stat error isn't absence.
    let path = root.join(sk_charter::CHARTER_YAML);
    match sk_kernel::paths::source_path_exists(&path) {
        Ok(false) => return (CharterDocument::Absent, path),
        Err(error) => {
            return (
                CharterDocument::Unreadable {
                    message: error.to_string(),
                },
                path,
            );
        }
        Ok(true) => {}
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return (
                CharterDocument::Unreadable {
                    message: error.to_string(),
                },
                path,
            );
        }
    };
    let text = sk_kernel::python_text::universal_newlines(&text);
    match serde_yaml_ng::from_str::<Value>(&text) {
        Ok(document) => (CharterDocument::Parsed(document), path),
        Err(error) => (
            CharterDocument::Unreadable {
                message: format!("YAML parse error: {error}"),
            },
            path,
        ),
    }
}

/// Refuse the retired `governance.doctrine` selection key.
fn require_canonical_governance(
    charter: &Option<Value>,
    path: &Path,
) -> Result<(), CharterSectionsError> {
    let Some(governance) = charter
        .as_ref()
        .and_then(|charter| charter.get("governance"))
    else {
        return Ok(());
    };
    if governance.as_mapping().is_some() && governance.get("doctrine").is_some() {
        return Err(CharterSectionsError::RetiredGovernanceSelection {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Refuse the retired `activations[*].doctrine_pack_id` field (`#3732`).
fn reject_retired_activation_fields(
    charter: &Option<Value>,
    path: &Path,
) -> Result<(), CharterSectionsError> {
    let Some(activations) = charter
        .as_ref()
        .and_then(|charter| charter.get("governance"))
        .and_then(|governance| governance.get("activations"))
        .and_then(Value::as_sequence)
    else {
        return Ok(());
    };
    for entry in activations {
        if entry.get("doctrine_pack_id").is_some() {
            return Err(CharterSectionsError::RetiredPackField {
                path: path.to_path_buf(),
                field: "doctrine_pack_id".to_owned(),
                replacement: "charter_pack_id".to_owned(),
            });
        }
    }
    Ok(())
}

fn is_string_sequence(value: &Value) -> bool {
    value
        .as_sequence()
        .is_some_and(|items| items.iter().all(Value::is_string))
}

fn is_string_mapping(value: &Value) -> bool {
    value.as_mapping().is_some_and(|mapping| {
        mapping
            .iter()
            .all(|(key, value)| key.is_string() && value.is_string())
    })
}

/// The list fields `GovernanceCharterConfig` validates as `list[str]`.
const GOVERNANCE_STRING_LISTS: [&str; 10] = [
    "selected_paradigms",
    "selected_directives",
    "selected_tactics",
    "selected_styleguides",
    "selected_toolguides",
    "selected_procedures",
    "selected_agent_profiles",
    "selected_mission_step_contracts",
    "selected_glossary_packs",
    "selected_assets",
];

/// Whether `governance:` passes the reference `GovernanceConfig` shape.
fn governance_section_valid(charter: &Option<Value>) -> bool {
    let Some(governance) = charter
        .as_ref()
        .and_then(|charter| charter.get("governance"))
    else {
        // No governance section (or no charter file) is an empty config.
        return true;
    };
    if governance.is_null() {
        // `_load_charter_yaml_section` returns `None` for an explicit null
        // section, which `load_governance_config` treats as absent.
        return true;
    }
    let Some(governance) = governance.as_mapping() else {
        return false;
    };
    for section in [
        "testing",
        "quality",
        "commits",
        "performance",
        "branch_strategy",
    ] {
        if let Some(value) = governance.get(section)
            && !value.is_mapping()
        {
            return false;
        }
    }
    if let Some(value) = governance.get("charter") {
        let Some(charter_block) = value.as_mapping() else {
            return false;
        };
        for key in GOVERNANCE_STRING_LISTS {
            if let Some(value) = charter_block.get(key)
                && !is_string_sequence(value)
            {
                return false;
            }
        }
        for key in [
            "available_tools",
            "authority_paths",
            "governance_references",
        ] {
            if let Some(value) = charter_block.get(key)
                && !is_string_sequence(value)
            {
                return false;
            }
        }
        if let Some(value) = charter_block.get("template_set")
            && !value.is_string()
            && !value.is_null()
        {
            return false;
        }
    }
    if let Some(value) = governance.get("activations") {
        let Some(items) = value.as_sequence() else {
            return false;
        };
        for item in items {
            let Some(entry) = item.as_mapping() else {
                return false;
            };
            if !entry
                .get("activation_context")
                .is_some_and(is_string_mapping)
                || !entry.get("charter_pack_id").is_some_and(Value::is_string)
                || !entry.get("artifact_id").is_some_and(Value::is_string)
                || entry
                    .get("artifact_kind")
                    .is_some_and(|value| !value.is_string() && !value.is_null())
            {
                return false;
            }
        }
    }
    if let Some(value) = governance.get("retrospective")
        && !value.is_null()
        && !value.is_mapping()
    {
        return false;
    }
    true
}

/// Whether the `directives:` section passes the reference `DirectivesConfig` shape.
fn directives_section_valid(charter: &Option<Value>) -> bool {
    let Some(section) = charter
        .as_ref()
        .and_then(|charter| charter.get("directives"))
    else {
        return true;
    };
    if section.is_null() {
        return true;
    }
    let Some(section) = section.as_mapping() else {
        return false;
    };
    let Some(items) = section.get("directives") else {
        return true;
    };
    let Some(items) = items.as_sequence() else {
        return false;
    };
    items.iter().all(|item| {
        let Some(item) = item.as_mapping() else {
            return false;
        };
        if !item.get("id").is_some_and(Value::is_string)
            || !item.get("title").is_some_and(Value::is_string)
        {
            return false;
        }
        for key in ["description", "severity"] {
            if let Some(value) = item.get(key)
                && !value.is_string()
            {
                return false;
            }
        }
        if let Some(value) = item.get("references")
            && !is_string_sequence(value)
        {
            return false;
        }
        true
    })
}

/// A project-local `directives:` entry from `charter.yaml`.
#[derive(Clone, Debug, Default)]
pub(crate) struct LocalDirective {
    pub(crate) id: String,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
}

/// The directly authored directive entries of a valid `directives:` section.
pub(crate) fn local_directives(charter: &Option<Value>) -> Vec<LocalDirective> {
    let Some(charter) = charter else {
        return Vec::new();
    };
    let Some(items) = charter
        .get("directives")
        .and_then(|section| section.get("directives"))
        .and_then(Value::as_sequence)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(Value::as_str)?;
            if id.trim().is_empty() {
                return None;
            }
            Some(LocalDirective {
                id: id.trim().to_owned(),
                title: item.get("title").and_then(Value::as_str).map(str::to_owned),
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// The charter's `governance.charter.selected_directives`, when readable.
pub(crate) fn selected_directives(charter: &Option<Value>) -> Vec<String> {
    let Some(selection) = charter
        .as_ref()
        .and_then(|charter| charter.get("governance"))
        .and_then(|governance| governance.get("charter"))
    else {
        return Vec::new();
    };
    selection
        .get("selected_directives")
        .and_then(Value::as_sequence)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The reference `GovernanceCharterConfig` selection projection.
///
/// This is a **read-only projection of the existing config model**, not a second
/// governance schema: the field names, list validation and defaults are the ones
/// [`governance_section_valid`] already validates, and the values come from the
/// one canonical `charter.yaml` reader. Every `selected_<kind>` list carries the
/// authored token exactly (the reference's `list[str]`), so a renderer can keep
/// an unknown id, a duplicated entry or a non-scalar code point instead of
/// narrowing it.
///
/// The eight selection kinds are the reference `_SELECTION_SECTIONS` roster
/// (paradigm, directive, tactic, styleguide, toolguide, procedure, agent_profile,
/// mission_step_contract). `selected_glossary_packs` / `selected_assets` stay
/// outside this projection because the reference's global-selection block never
/// renders them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GovernanceCharterSelection {
    /// `selected_paradigms`, authored order.
    pub selected_paradigms: Vec<PythonString>,
    /// `selected_directives`, authored order.
    pub selected_directives: Vec<PythonString>,
    /// `selected_tactics`, authored order.
    pub selected_tactics: Vec<PythonString>,
    /// `selected_styleguides`, authored order.
    pub selected_styleguides: Vec<PythonString>,
    /// `selected_toolguides`, authored order.
    pub selected_toolguides: Vec<PythonString>,
    /// `selected_procedures`, authored order.
    pub selected_procedures: Vec<PythonString>,
    /// `selected_agent_profiles`, authored order.
    pub selected_agent_profiles: Vec<PythonString>,
    /// `selected_mission_step_contracts`, authored order.
    pub selected_mission_step_contracts: Vec<PythonString>,
    /// `available_tools` (the resolver's scalar tool registry input).
    pub available_tools: Vec<String>,
    /// `template_set`, when authored.
    pub template_set: Option<String>,
    /// `governance_references`, authored order.
    pub governance_references: Vec<String>,
}

impl GovernanceCharterSelection {
    /// The authored `selected_<kind>` list of one of the eight selection kinds.
    ///
    /// A kind outside the reference roster has no selection field and yields an
    /// empty slice — never a fabricated list.
    #[must_use]
    pub fn selected(&self, kind: ArtifactKind) -> &[PythonString] {
        match kind {
            ArtifactKind::Paradigm => &self.selected_paradigms,
            ArtifactKind::Directive => &self.selected_directives,
            ArtifactKind::Tactic => &self.selected_tactics,
            ArtifactKind::Styleguide => &self.selected_styleguides,
            ArtifactKind::Toolguide => &self.selected_toolguides,
            ArtifactKind::Procedure => &self.selected_procedures,
            ArtifactKind::AgentProfile => &self.selected_agent_profiles,
            ArtifactKind::MissionStepContract => &self.selected_mission_step_contracts,
            _ => &[],
        }
    }

    /// Mutable access to one of the eight selection lists (org overlay + deny).
    fn selected_mut(&mut self, kind: ArtifactKind) -> Option<&mut Vec<PythonString>> {
        Some(match kind {
            ArtifactKind::Paradigm => &mut self.selected_paradigms,
            ArtifactKind::Directive => &mut self.selected_directives,
            ArtifactKind::Tactic => &mut self.selected_tactics,
            ArtifactKind::Styleguide => &mut self.selected_styleguides,
            ArtifactKind::Toolguide => &mut self.selected_toolguides,
            ArtifactKind::Procedure => &mut self.selected_procedures,
            ArtifactKind::AgentProfile => &mut self.selected_agent_profiles,
            ArtifactKind::MissionStepContract => &mut self.selected_mission_step_contracts,
            _ => return None,
        })
    }

    /// The finite scalar view of `selected_paradigms` the existing project
    /// resolver consumes.
    ///
    /// That resolver is `String`-keyed (a pre-existing finite adapter). A token
    /// containing a surrogate unit has no scalar projection and is therefore
    /// invisible to it; the global-selection renderer reads
    /// [`GovernanceCharterSelection::selected`] instead and keeps the exact token.
    #[must_use]
    pub(crate) fn scalar_selected_paradigms(&self) -> Vec<String> {
        self.selected_paradigms
            .iter()
            .filter_map(|token| token.to_scalar_string().ok())
            .collect()
    }
}

/// The reference `GovernanceCharterConfig` projection (list validation already
/// happened in [`governance_section_valid`]).
pub(crate) fn governance_charter_config(charter: &Option<Value>) -> GovernanceCharterSelection {
    let Some(block) = charter
        .as_ref()
        .and_then(|charter| charter.get("governance"))
        .and_then(|governance| governance.get("charter"))
    else {
        return GovernanceCharterSelection::default();
    };
    let strings = |key: &str| -> Vec<String> {
        block
            .get(key)
            .and_then(Value::as_sequence)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let exact = |key: &str| -> Vec<PythonString> {
        block
            .get(key)
            .and_then(Value::as_sequence)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(PythonString::from_scalars)
                    .collect()
            })
            .unwrap_or_default()
    };
    GovernanceCharterSelection {
        selected_paradigms: exact("selected_paradigms"),
        selected_directives: exact("selected_directives"),
        selected_tactics: exact("selected_tactics"),
        selected_styleguides: exact("selected_styleguides"),
        selected_toolguides: exact("selected_toolguides"),
        selected_procedures: exact("selected_procedures"),
        selected_agent_profiles: exact("selected_agent_profiles"),
        selected_mission_step_contracts: exact("selected_mission_step_contracts"),
        available_tools: strings("available_tools"),
        template_set: block
            .get("template_set")
            .and_then(Value::as_str)
            .map(str::to_owned),
        governance_references: strings("governance_references"),
    }
}

/// The reference `_load_governance_charter_config`: the **effective** selection.
///
/// Source's policy, in its own order:
///
/// 1. read the authored `governance.charter` block; any read/validation failure
///    collapses to the default (empty) selection, while the retired-shape errors
///    propagate so their selections are never dropped in silence;
/// 2. union every org pack's `required_<kind>` into the matching `selected_<kind>`
///    list, preserving authored order and appending first-seen requirements;
/// 3. subtract the effective deny (which can never hold an org-required id).
///
/// The org-required read is the canonical raw, lenient reader (one field per
/// requested kind). The deny is the canonical activation-state deny for each
/// kind, built only when the ledger actually carries a deny entry (the
/// reference's `load_denied` NFR-003 arm), so a project without denials reads no
/// identity index and no floor.
///
/// # Errors
///
/// The retired `governance.doctrine` / retired pack field refusals (never
/// swallowed), and a ledger or org-charter read failure the reference raises.
pub fn governance_charter_selection(
    repo_root: &Path,
    identity: Option<&IdentityIndex>,
) -> Result<GovernanceCharterSelection, ProjectGovernanceError> {
    let mut selection = match read_charter_sections(repo_root) {
        Ok(sections) if sections.governance_valid => governance_charter_config(&sections.charter),
        // Source defaults only the config read. Required/deny processing still
        // happens afterwards, including when parsing or validation failed.
        Ok(_) | Err(CharterSectionsError::Unreadable { .. }) => {
            GovernanceCharterSelection::default()
        }
        Err(error) => return Err(error.into()),
    };

    let required =
        crate::org_charter::read_org_required_selections_exact(repo_root).map_err(config_error)?;
    let any_required = required.values().any(|tokens| !tokens.is_empty());

    let deny = effective_deny_for_selection(repo_root, identity)?;
    if !any_required && deny.is_empty() {
        return Ok(selection);
    }

    for kind in crate::org_charter::selection_overlayable_kinds() {
        let Some(field) = selection.selected_mut(*kind) else {
            continue;
        };
        for token in required.get(kind).map(Vec::as_slice).unwrap_or(&[]) {
            if !field.contains(token) {
                field.push(token.clone());
            }
        }
        if let Some(denied) = deny.get(kind) {
            field.retain(|token| denied.admits_exact(token));
        }
    }
    Ok(selection)
}

/// The effective deny of every kind, read at the source reader's own phase.
///
/// Source `_load_governance_charter_config` computes `dict(load_denied(repo_root))`
/// with no `kinds` filter, so the phases are the loader's: the
/// `ledger_may_hold_deny` key probe, the source read, one identity index and the
/// per-pending-kind strict raw floor read. The caller's already-built identity
/// is reused, so the same layers are not scanned twice.
fn effective_deny_for_selection(
    repo_root: &Path,
    _identity: Option<&IdentityIndex>,
) -> Result<BTreeMap<ArtifactKind, ExactDenySet>, ProjectGovernanceError> {
    let denied = crate::source_denied::load_denied(
        repo_root,
        &crate::source_denied::LoadDeniedOptions::default(),
    )
    .map_err(config_error)?;
    crate::source_denied::deny_by_kind(&denied).map_err(config_error)
}

// ---------------------------------------------------------------------------
// Directive selection (shared with the action build)
// ---------------------------------------------------------------------------

/// The shared directive-selection result.
#[derive(Clone, Debug)]
pub(crate) enum DirectiveSelectionOutcome {
    /// A malformed charter section: the reference's action build degrades to the
    /// directly-loaded local directives.
    Degraded {
        /// The directly-loaded local entries.
        local: Vec<LocalDirective>,
    },
    /// The resolved ids, provenance label and diagnostics.
    Resolved {
        /// Resolved ids in first-occurrence order.
        ids: Vec<String>,
        /// The reference `directives_source` label.
        source: String,
        /// Diagnostics in producer order.
        diagnostics: Vec<Diagnostic>,
    },
}

/// Resolve the effective directive list: activation base, charter selection,
/// project-local layer, then the effective deny.
///
/// `catalog_ids` are the normalised built-in directive ids. The action build
/// supplies them from the merged graph's built-in provenance; the project
/// resolver supplies them from the built-in catalogue — the resolution policy
/// is this one function.
pub(crate) fn resolve_directive_selection<T: LedgerToken>(
    sections: &CharterConfigSections,
    catalog_ids: &BTreeSet<String>,
    identity: &IdentityIndex,
    ledger: &ActivationLedger,
    deny: Option<&DenySet<T>>,
) -> DirectiveSelectionOutcome {
    let local = sections.local_directives();
    if !sections.governance_valid || !sections.directives_valid {
        return DirectiveSelectionOutcome::Degraded { local };
    }
    let mut diagnostics = Vec::new();
    let local_ids: BTreeSet<String> = local.iter().map(|entry| entry.id.clone()).collect();
    let charter = &sections.charter;
    let activated = ledger.allow_for(ArtifactKind::Directive);
    let (mut resolved, mut base_source) = match activated {
        None => {
            let base: Vec<String> = catalog_ids.iter().cloned().collect();
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::CatalogDefaultApplied,
                message: format!(
                    "No activated directive set configured; using built-in catalog default ({} directives).",
                    base.len()
                ),
            });
            (base, "catalog_fallback".to_owned())
        }
        Some(tokens) => {
            let mut warnings = Vec::new();
            let mut base: BTreeSet<String> = BTreeSet::new();
            for token in tokens {
                let declared =
                    identity.activation_id(ArtifactKind::Directive, token, &mut warnings);
                let denied = deny.is_some_and(|deny| !deny.admits(&declared));
                if !denied {
                    base.insert(declared);
                }
            }
            // Unknown activation tokens are warnings in the reference, not
            // `UNKNOWN_SELECTION_DROPPED` diagnostics.
            let _ = warnings;
            (base.into_iter().collect(), "activation".to_owned())
        }
    };

    let selected = selected_directives(charter);
    if !selected.is_empty() {
        let mut selection: Vec<String> = Vec::new();
        for token in &selected {
            let declared = identity
                .selection_id(ArtifactKind::Directive, token)
                .or_else(|| {
                    let legacy = normalize_directive_id(token);
                    (local_ids.contains(&legacy) || catalog_ids.contains(&legacy)).then_some(legacy)
                });
            match declared {
                Some(declared) => {
                    if !selection.contains(&declared) {
                        selection.push(declared);
                    }
                }
                None => diagnostics.push(Diagnostic {
                    code: DiagnosticCode::UnknownSelectionDropped,
                    message: unknown_selection_message("directive", token),
                }),
            }
        }
        if !selection.is_empty() {
            let added: Vec<String> = selection
                .iter()
                .filter(|id| !resolved.contains(id))
                .cloned()
                .collect();
            diagnostics.push(charter_selection_merged_diagnostic(
                "directive",
                &selection,
                &added,
                &base_source,
            ));
            for declared in added {
                resolved.push(declared);
            }
            base_source = format!("{base_source}+charter");
        }
    }

    if !local_ids.is_empty() {
        let local_order: Vec<String> = local.iter().map(|entry| entry.id.clone()).collect();
        let added: Vec<String> = local_order
            .iter()
            .filter(|id| !resolved.contains(id))
            .cloned()
            .collect();
        diagnostics.push(project_local_merged_diagnostic(
            &local_order,
            &added,
            &base_source,
        ));
        for entry in &local {
            if !resolved.contains(&entry.id) {
                resolved.push(entry.id.clone());
            }
        }
        base_source = format!("{base_source}+project_local");
    }

    // The effective deny applies after every union; it never holds a
    // required-floor id.
    if let Some(deny) = deny {
        resolved.retain(|id| deny.admits(id));
    }

    DirectiveSelectionOutcome::Resolved {
        ids: resolved,
        source: base_source,
        diagnostics,
    }
}

// ---------------------------------------------------------------------------
// Paradigm, tool and template selection
// ---------------------------------------------------------------------------

fn resolve_selected_paradigms(
    selected: &[String],
    catalogue: &GovernanceCatalogue,
    identity: &IdentityIndex,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let mut resolved = Vec::new();
    let validate = catalogue.has_domain("paradigms");
    for token in selected {
        let declared = identity
            .selection_id(ArtifactKind::Paradigm, token)
            .or_else(|| (catalogue.paradigms.contains(token) || !validate).then(|| token.clone()));
        match declared {
            Some(declared) => {
                if !resolved.contains(&declared) {
                    resolved.push(declared);
                }
            }
            None => diagnostics.push(Diagnostic {
                code: DiagnosticCode::UnknownSelectionDropped,
                message: unknown_selection_message("paradigm", token),
            }),
        }
    }
    resolved
}

fn resolve_paradigm_base<T: LedgerToken>(
    catalogue: &GovernanceCatalogue,
    ledger: &ActivationLedger,
    identity: &IdentityIndex,
    deny: Option<&DenySet<T>>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<String>, String) {
    match ledger.allow_for(ArtifactKind::Paradigm) {
        None => {
            let base: Vec<String> = catalogue.paradigms.iter().cloned().collect();
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::CatalogDefaultApplied,
                message: format!(
                    "No activated paradigm set configured; using built-in catalog default ({} paradigms).",
                    base.len()
                ),
            });
            (base, "catalog_fallback".to_owned())
        }
        Some(tokens) => {
            let mut warnings = Vec::new();
            let mut ids: BTreeSet<String> = BTreeSet::new();
            for token in tokens {
                let declared = identity.activation_id(ArtifactKind::Paradigm, token, &mut warnings);
                // The reference's `PackContext.allow_for` is already netted by
                // the effective deny; preserving that here keeps the
                // selection-merge diagnostic's base fact exact.
                if deny.is_none_or(|deny| deny.admits(&declared)) {
                    ids.insert(declared);
                }
            }
            // Activation-token warnings are logged by the reference, never
            // surfaced as resolution diagnostics.
            let _ = warnings;
            (ids.into_iter().collect(), "activation".to_owned())
        }
    }
}

fn report_charter_selection(
    noun: &str,
    resolved: &[String],
    base: &[String],
    base_source: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if resolved.is_empty() {
        return;
    }
    let base_set: BTreeSet<&String> = base.iter().collect();
    let added: Vec<String> = resolved
        .iter()
        .filter(|id| !base_set.contains(id))
        .cloned()
        .collect();
    diagnostics.push(charter_selection_merged_diagnostic(
        noun,
        resolved,
        &added,
        base_source,
    ));
}

fn charter_selection_merged_diagnostic(
    noun: &str,
    selection: &[String],
    added: &[String],
    base_source: &str,
) -> Diagnostic {
    if !added.is_empty() {
        return Diagnostic {
            code: DiagnosticCode::SelectionMerged,
            message: format!(
                "Charter selected {} {noun}(s) beyond the {base_source} base: {}.",
                added.len(),
                added.join(", ")
            ),
        };
    }
    Diagnostic {
        code: DiagnosticCode::SelectionMerged,
        message: format!(
            "All {} charter-selected {noun}(s) already present in the {base_source} base; none added: {}.",
            selection.len(),
            selection.join(", ")
        ),
    }
}

fn project_local_merged_diagnostic(
    local: &[String],
    added: &[String],
    base_source: &str,
) -> Diagnostic {
    if !added.is_empty() {
        return Diagnostic {
            code: DiagnosticCode::SelectionMerged,
            message: format!(
                "Merged {} project-local directive(s) onto the {base_source} set: {}.",
                added.len(),
                added.join(", ")
            ),
        };
    }
    Diagnostic {
        code: DiagnosticCode::SelectionMerged,
        message: format!(
            "All {} project-local directive(s) already present in the {base_source} set; none added: {}.",
            local.len(),
            local.join(", ")
        ),
    }
}

fn resolve_tools_selection(
    selected_tools: &[String],
    tool_registry: Option<&BTreeSet<String>>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<String>, String) {
    let baseline: BTreeSet<String> = match tool_registry {
        Some(registry) if !registry.is_empty() => registry.clone(),
        // The reference `tool_registry or set(DEFAULT_TOOL_REGISTRY)`: an empty
        // registry falls back to the defaults.
        _ => DEFAULT_TOOL_REGISTRY
            .iter()
            .map(|tool| (*tool).to_owned())
            .collect(),
    };
    if selected_tools.is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::FallbackApplied,
            message: "No available_tools selection provided; using runtime tool registry fallback."
                .to_owned(),
        });
        return (baseline.into_iter().collect(), "registry_only".to_owned());
    }
    let selected: BTreeSet<String> = selected_tools.iter().cloned().collect();
    let unioned: BTreeSet<String> = baseline.union(&selected).cloned().collect();
    let added: Vec<String> = selected.difference(&baseline).cloned().collect();
    if !added.is_empty() {
        diagnostics.push(Diagnostic {
            code: DiagnosticCode::SelectionMerged,
            message: format!(
                "Charter declared additional tool(s) beyond the runtime registry: {}.",
                added.join(", ")
            ),
        });
    }
    (unioned.into_iter().collect(), "charter+registry".to_owned())
}

fn resolve_template_set_selection(
    template_set: Option<&str>,
    catalogue: &GovernanceCatalogue,
    fallback_template_set: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(String, String), ProjectGovernanceError> {
    if let Some(template_set) = template_set.filter(|value| !value.is_empty()) {
        if catalogue.has_domain("template_sets") && !catalogue.template_sets.contains(template_set)
        {
            let available = if catalogue.template_sets.is_empty() {
                "(none)".to_owned()
            } else {
                catalogue
                    .template_sets
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(ProjectGovernanceError::Resolution(
                GovernanceResolutionError::new(vec![
                    format!("Charter selected unavailable template_set: '{template_set}'"),
                    format!("Available template sets: {available}"),
                    TEMPLATE_SET_REMEDIATION.to_owned(),
                ]),
            ));
        }
        return Ok((template_set.to_owned(), "charter".to_owned()));
    }
    diagnostics.push(Diagnostic {
        code: DiagnosticCode::FallbackApplied,
        message: format!(
            "Template set not selected in charter; fallback '{fallback_template_set}' applied."
        ),
    });
    Ok((fallback_template_set.to_owned(), "fallback".to_owned()))
}
