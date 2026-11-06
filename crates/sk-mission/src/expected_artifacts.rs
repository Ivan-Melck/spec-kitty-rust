//! Canonical `expected-artifacts.yaml` reading and artifact-name projection.
//!
//! Source's acceptance path never hardcodes `spec.md` / `plan.md` / `tasks.md`.
//! It resolves each name from the mission type's `expected-artifacts.yaml`
//! through one loader authority
//! (`charter.activation.manifest_loader.load_manifest` →
//! `charter.offering.missions.step_projection.project_artifact_name_set`):
//!
//! * an org-pack `<org_root>/missions/<type>/expected-artifacts.yaml` wins
//!   **whole-file** (never field-merged) over the built-in file, and among
//!   several org roots the *last existing match* wins;
//! * the built-in file is `<packaged missions root>/<type>/expected-artifacts.yaml`;
//! * genuine absence on every consulted tier is `None` (custom mission types
//!   degrade gracefully);
//! * a *found* file that is unreadable, undecodable or not valid YAML raises the
//!   "malformed" failure; a *found* file that parses but violates the schema
//!   (`extra="forbid"`, missing `mission_type`, a bad `artifact_class`, a
//!   non-string pattern) raises the distinct "schema" failure. Both are typed
//!   here so a caller can never mistake a broken manifest for "no manifest".
//!
//! This module owns the *reading*; it does not decide which tiers exist. Org
//! roots and the packaged missions tree come from the caller's provider ports
//! (Source's own layering: the manifest loader asks the charter/activation
//! surface, which this crate has no business depending on).
//!
//! # Projection
//!
//! [`ExpectedArtifactManifest::artifact_name`] reproduces
//! `project_artifact_name_set`: `required_always`, then every
//! `required_by_step` list in authored step order, then `optional_always`, with
//! the last `artifact_key` occurrence winning. Source reads the acceptance trio
//! as `input.spec.main` / `output.plan.main` / `output.tasks.list`
//! (`resolve_configured_artifact_name`), which is why key projection — not "all
//! blocking rows" — is the authority used here.

use std::path::{Path, PathBuf};

use sk_kernel::PythonString;
use sk_kernel::python_yaml::{Dialect, GraphDocument, NodeRef};

use crate::definition::dialect::SafeDocumentDecoder;

/// The manifest's file name inside a mission-type directory.
pub const EXPECTED_ARTIFACTS_FILE_NAME: &str = "expected-artifacts.yaml";

/// Upper bound for one manifest read.
///
/// Source reads the file unbounded; the bound (16 MiB, the same budget
/// `mission.yaml` uses) exists so a hostile or accidental huge file is refused
/// instead of loaded.
pub const DEFAULT_EXPECTED_ARTIFACTS_BYTE_LIMIT: u64 = 16 * 1024 * 1024;

/// One `artifact_class` value (Source `ArtifactClassEnum`).
pub const ARTIFACT_CLASSES: [&str; 6] = [
    "input", "workflow", "output", "evidence", "policy", "runtime",
];

/// The acceptance trio's manifest keys, in `_missing_artifacts` order.
pub const ACCEPTANCE_ARTIFACT_KEYS: [(&str, &str); 3] = [
    ("input.spec.main", "spec"),
    ("output.plan.main", "plan"),
    ("output.tasks.list", "tasks"),
];

/// One `ExpectedArtifactSpec` row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedArtifactSpec {
    /// Stable key, e.g. `input.spec.main`.
    pub artifact_key: String,
    /// `input` | `workflow` | `output` | `evidence` | `policy` | `runtime`.
    pub artifact_class: String,
    /// Path pattern relative to the mission directory.
    pub path_pattern: String,
    /// Whether a missing artifact blocks mission completeness.
    pub blocking: bool,
}

/// A validated `expected-artifacts.yaml`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedArtifactManifest {
    /// Declared schema version (default `1.0`).
    pub schema_version: String,
    /// The mission type this manifest declares.
    pub mission_type: String,
    /// Declared manifest data version (default `1`).
    pub manifest_version: String,
    /// Artifacts required regardless of step.
    pub required_always: Vec<ExpectedArtifactSpec>,
    /// Step id → required artifacts, in authored order.
    pub required_by_step: Vec<(String, Vec<ExpectedArtifactSpec>)>,
    /// Artifacts optional throughout the mission.
    pub optional_always: Vec<ExpectedArtifactSpec>,
}

impl ExpectedArtifactManifest {
    /// Step ids declared in `required_by_step`, in authored order.
    pub fn step_ids(&self) -> Vec<&str> {
        self.required_by_step
            .iter()
            .map(|(step, _)| step.as_str())
            .collect()
    }

    /// Every row, in Source's `project_artifact_name_set` order.
    fn rows_in_projection_order(&self) -> impl Iterator<Item = &ExpectedArtifactSpec> {
        self.required_always
            .iter()
            .chain(
                self.required_by_step
                    .iter()
                    .flat_map(|(_, specs)| specs.iter()),
            )
            .chain(self.optional_always.iter())
    }

    /// The projected `artifact_key -> path_pattern` mapping (last key wins).
    pub fn artifact_names(&self) -> Vec<(String, String)> {
        let mut names: Vec<(String, String)> = Vec::new();
        for spec in self.rows_in_projection_order() {
            match names.iter_mut().find(|(key, _)| key == &spec.artifact_key) {
                Some(entry) => entry.1.clone_from(&spec.path_pattern),
                None => names.push((spec.artifact_key.clone(), spec.path_pattern.clone())),
            }
        }
        names
    }

    /// The configured path pattern for `artifact_key`, when projected.
    pub fn artifact_name(&self, artifact_key: &str) -> Option<&str> {
        self.rows_in_projection_order()
            .filter(|spec| spec.artifact_key == artifact_key)
            .last()
            .map(|spec| spec.path_pattern.as_str())
    }

    /// The blocking artifacts required at `step` (present-or-not is the
    /// caller's question): `required_always` then `required_by_step[step]`.
    pub fn required_artifacts_for_step(&self, step: &str) -> Vec<&ExpectedArtifactSpec> {
        let mut rows: Vec<&ExpectedArtifactSpec> = self
            .required_always
            .iter()
            .filter(|spec| spec.blocking)
            .collect();
        if let Some((_, specs)) = self.required_by_step.iter().find(|(name, _)| name == step) {
            rows.extend(specs.iter().filter(|spec| spec.blocking));
        }
        rows
    }
}

/// Where a manifest may be read from.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExpectedArtifactRoots<'a> {
    /// Org-pack roots in declared precedence order. Source keeps the *last*
    /// existing match (`resolve_org_expected_artifacts`).
    pub org_roots: &'a [PathBuf],
    /// The packaged missions tree (`<root>/<mission_type>/expected-artifacts.yaml`).
    pub packaged_missions_root: Option<&'a Path>,
}

/// A typed `expected-artifacts.yaml` failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedArtifactsError {
    /// The mission type is not a safe single path segment.
    UnsafeMissionType {
        /// The refused mission type.
        mission_type: String,
        /// Why it was refused.
        detail: String,
    },
    /// A found manifest could not be read or decoded (Source
    /// `MalformedManifestError`).
    Malformed {
        /// The file that was found.
        path: PathBuf,
        /// The read/decode failure.
        detail: String,
    },
    /// A found, parseable manifest violated the schema (Source
    /// `ManifestSchemaError`).
    Schema {
        /// The mission type the manifest was resolved for.
        mission_type: String,
        /// The source of the manifest (a file path, or an org-tier label).
        origin: String,
        /// The failing field and why.
        detail: String,
    },
    /// A projected key is missing (Source `ArtifactNameConfigurationError`).
    MissingArtifactKey {
        /// The mission type the manifest was resolved for.
        mission_type: String,
        /// The missing key.
        artifact_key: String,
        /// Where the manifest came from, for the operator.
        origin: String,
    },
}

impl std::fmt::Display for ExpectedArtifactsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafeMissionType {
                mission_type,
                detail,
            } => write!(
                formatter,
                "mission type {mission_type:?} is not a safe path segment: {detail}"
            ),
            Self::Malformed { path, detail } => write!(
                formatter,
                "{}{EXPECTED_ARTIFACTS_FILE_NAME} is unreadable or malformed: {detail}",
                path.display()
            ),
            Self::Schema {
                mission_type,
                origin,
                detail,
            } => write!(
                formatter,
                "expected-artifacts.yaml schema-invalid for mission type {mission_type:?} ({origin}): {detail}"
            ),
            Self::MissingArtifactKey {
                mission_type,
                artifact_key,
                origin,
            } => write!(
                formatter,
                "expected-artifacts.yaml for mission type {mission_type:?} ({origin}) is missing the requested mapping key {artifact_key:?}"
            ),
        }
    }
}

impl std::error::Error for ExpectedArtifactsError {}

/// Source `assert_safe_path_segment`: one non-empty path segment.
fn safe_mission_type(mission_type: &str) -> Result<(), ExpectedArtifactsError> {
    let refused = |detail: &str| {
        Err(ExpectedArtifactsError::UnsafeMissionType {
            mission_type: mission_type.to_owned(),
            detail: detail.to_owned(),
        })
    };
    if mission_type.is_empty() {
        return refused("it is empty");
    }
    if mission_type.starts_with('.') {
        return refused("it starts with a dot");
    }
    for character in mission_type.chars() {
        let allowed = character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.');
        if !allowed {
            return refused(if character.is_ascii_whitespace() {
                "it contains whitespace"
            } else if character.is_ascii() {
                "it contains a reserved character"
            } else {
                "it contains a non-ASCII character"
            });
        }
    }
    Ok(())
}

/// Resolve the manifest file the tiers select, if any.
///
/// Org tier first (last existing match wins), then the packaged built-in tree.
fn manifest_path(
    mission_type: &str,
    roots: ExpectedArtifactRoots<'_>,
) -> Option<(PathBuf, String)> {
    let mut org_hit: Option<PathBuf> = None;
    for root in roots.org_roots {
        let candidate = root
            .join("missions")
            .join(mission_type)
            .join(EXPECTED_ARTIFACTS_FILE_NAME);
        if candidate.is_file() {
            org_hit = Some(candidate);
        }
    }
    if let Some(path) = org_hit {
        let origin = format!(
            "org-tier expected-artifacts.yaml for mission type {mission_type:?} (checked org roots: {})",
            roots
                .org_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Some((path, origin));
    }
    roots.packaged_missions_root.map(|root| {
        let path = root.join(mission_type).join(EXPECTED_ARTIFACTS_FILE_NAME);
        let origin = path.display().to_string();
        (path, origin)
    })
}

/// Load `mission_type`'s manifest, when any consulted tier has one.
///
/// # Errors
///
/// See [`ExpectedArtifactsError`]: an unsafe mission type, a found-but-malformed
/// file, or a found-but-schema-invalid file. Absence is `Ok(None)`.
pub fn load_expected_artifacts(
    mission_type: &str,
    roots: ExpectedArtifactRoots<'_>,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<Option<ExpectedArtifactManifest>, ExpectedArtifactsError> {
    Ok(
        load_expected_artifacts_with_origin(mission_type, roots, decoder)?
            .map(|loaded| loaded.manifest),
    )
}

/// A loaded manifest together with the provenance of the file it was read from.
///
/// The origin is the label [`manifest_path`] already builds for its own schema
/// errors (an org-tier description, or the packaged file's path). Callers that
/// must disclose WHICH manifest governed a decision use this instead of inferring
/// it from their own branch logic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedExpectedArtifacts {
    /// The validated manifest.
    pub manifest: ExpectedArtifactManifest,
    /// Source-style origin label: the file path, or the org-tier description.
    pub origin: String,
    /// The file the manifest was read from.
    pub path: std::path::PathBuf,
}

/// [`load_expected_artifacts`] that also returns where the manifest came from.
///
/// # Errors
///
/// As [`load_expected_artifacts`].
pub fn load_expected_artifacts_with_origin(
    mission_type: &str,
    roots: ExpectedArtifactRoots<'_>,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<Option<LoadedExpectedArtifacts>, ExpectedArtifactsError> {
    safe_mission_type(mission_type)?;
    let Some((path, origin)) = manifest_path(mission_type, roots) else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = sk_kernel::read_bounded(&path, DEFAULT_EXPECTED_ARTIFACTS_BYTE_LIMIT).map_err(
        |source| ExpectedArtifactsError::Malformed {
            path: path.clone(),
            detail: source.to_string(),
        },
    )?;
    let text = sk_kernel::python_yaml::read_document_text(&bytes, Dialect::PyYamlSafe11).map_err(
        |error| ExpectedArtifactsError::Malformed {
            path: path.clone(),
            detail: error.to_string(),
        },
    )?;
    let document =
        decoder
            .decode(&path, &text)
            .map_err(|detail| ExpectedArtifactsError::Malformed {
                path: path.clone(),
                detail,
            })?;
    let manifest = parse_manifest(&document, mission_type, &origin)?;
    Ok(Some(LoadedExpectedArtifacts {
        manifest,
        origin,
        path,
    }))
}

/// Resolve one configured artifact name through the manifest projection.
///
/// # Errors
///
/// As [`load_expected_artifacts`], plus
/// [`ExpectedArtifactsError::MissingArtifactKey`] when the manifest exists but
/// does not project `artifact_key`.
pub fn configured_artifact_name(
    mission_type: &str,
    artifact_key: &str,
    roots: ExpectedArtifactRoots<'_>,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<Option<String>, ExpectedArtifactsError> {
    let Some(manifest) = load_expected_artifacts(mission_type, roots, decoder)? else {
        return Ok(None);
    };
    match manifest.artifact_name(artifact_key) {
        Some(name) => Ok(Some(name.to_owned())),
        None => Err(ExpectedArtifactsError::MissingArtifactKey {
            mission_type: mission_type.to_owned(),
            artifact_key: artifact_key.to_owned(),
            origin: manifest.mission_type.clone(),
        }),
    }
}

/// The `_missing_artifacts` required-name resolution Source performs, in order.
///
/// Returns `Ok(None)` when no manifest is available on any tier, so the caller
/// can fall back to the documented pinned default rather than guessing a name.
pub fn acceptance_required_artifact_names(
    roots: ExpectedArtifactRoots<'_>,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<Option<Vec<String>>, ExpectedArtifactsError> {
    Ok(
        acceptance_required_artifact_names_with_origin(roots, decoder)?
            .map(|(names, _origin)| names),
    )
}

/// [`acceptance_required_artifact_names`] that also returns the origin of the
/// manifest the trio was projected from.
///
/// # Errors
///
/// As [`load_expected_artifacts`], plus
/// [`ExpectedArtifactsError::MissingArtifactKey`] for a manifest that does not
/// project one of the three acceptance keys.
pub fn acceptance_required_artifact_names_with_origin(
    roots: ExpectedArtifactRoots<'_>,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<Option<(Vec<String>, String)>, ExpectedArtifactsError> {
    let Some(loaded) = load_expected_artifacts_with_origin("software-dev", roots, decoder)? else {
        return Ok(None);
    };
    let mut names = Vec::new();
    for (artifact_key, label) in ACCEPTANCE_ARTIFACT_KEYS {
        match loaded.manifest.artifact_name(artifact_key) {
            Some(name) => names.push(name.to_owned()),
            None => {
                return Err(ExpectedArtifactsError::MissingArtifactKey {
                    mission_type: "software-dev".to_owned(),
                    artifact_key: format!("{artifact_key} ({label})"),
                    origin: loaded.manifest.mission_type.clone(),
                });
            }
        }
    }
    Ok(Some((names, loaded.origin)))
}

/// Render one Python value the way pydantic names it in a schema error.
fn python_type(node: NodeRef<'_>) -> &'static str {
    node.python_type_name()
}

fn field_name(node: NodeRef<'_>) -> String {
    node.as_str()
        .and_then(|text| text.to_scalar_string().ok())
        .unwrap_or_else(|| node.repr())
}

/// Parse and validate one document into the typed manifest.
fn parse_manifest(
    document: &GraphDocument,
    mission_type: &str,
    origin: &str,
) -> Result<ExpectedArtifactManifest, ExpectedArtifactsError> {
    let fail = |detail: String| ExpectedArtifactsError::Schema {
        mission_type: mission_type.to_owned(),
        origin: origin.to_owned(),
        detail,
    };
    if document.has_cycle() {
        return Err(fail("the document contains a reference cycle".to_owned()));
    }
    let root = document.root();
    let Some(entries) = root.as_mapping() else {
        return Err(fail(format!(
            "expected a mapping at the document root, found {}",
            python_type(root)
        )));
    };

    let allowed = [
        "schema_version",
        "mission_type",
        "manifest_version",
        "required_always",
        "required_by_step",
        "optional_always",
    ];
    for (key, _) in entries {
        let key_view = document.view(*key);
        let name = field_name(key_view);
        if !key_view
            .as_str()
            .and_then(|text| text.to_scalar_string().ok())
            .is_some_and(|name| allowed.contains(&name.as_str()))
        {
            return Err(fail(format!("Extra inputs are not permitted: {name}")));
        }
    }

    let lookup = |name: &str| root.lookup(name);
    let text_field =
        |name: &str, default: Option<&str>| -> Result<String, ExpectedArtifactsError> {
            match lookup(name) {
                None => default
                    .map(str::to_owned)
                    .ok_or_else(|| ExpectedArtifactsError::Schema {
                        mission_type: mission_type.to_owned(),
                        origin: origin.to_owned(),
                        detail: format!("{name}: Field required"),
                    }),
                Some(node) => match node.as_str().and_then(|text| text.to_scalar_string().ok()) {
                    Some(text) => Ok(text),
                    None => Err(fail(format!(
                        "{name}: Input should be a valid string, found {}",
                        python_type(node)
                    ))),
                },
            }
        };

    let declared_type = text_field("mission_type", None)?;
    let schema_version = text_field("schema_version", Some("1.0"))?;
    let manifest_version = text_field("manifest_version", Some("1"))?;

    let spec_rows = |label: &str,
                     node: NodeRef<'_>|
     -> Result<Vec<ExpectedArtifactSpec>, ExpectedArtifactsError> {
        let Some(rows) = node.as_sequence() else {
            return Err(fail(format!(
                "{label}: Input should be a valid list, found {}",
                python_type(node)
            )));
        };
        rows.iter()
            .enumerate()
            .map(|(index, row)| {
                parse_spec(document, document.view(*row), &format!("{label}[{index}]"))
                    .map_err(fail)
            })
            .collect()
    };

    let mut required_always = Vec::new();
    if let Some(node) = lookup("required_always") {
        required_always = spec_rows("required_always", node)?;
    }
    let mut optional_always = Vec::new();
    if let Some(node) = lookup("optional_always") {
        optional_always = spec_rows("optional_always", node)?;
    }
    let mut required_by_step: Vec<(String, Vec<ExpectedArtifactSpec>)> = Vec::new();
    if let Some(node) = lookup("required_by_step") {
        let Some(entries) = node.as_mapping() else {
            return Err(fail(format!(
                "required_by_step: Input should be a valid dictionary, found {}",
                python_type(node)
            )));
        };
        for (key, value) in entries {
            let step = document.view(*key);
            let Some(step_name) = step.as_str().and_then(|text| text.to_scalar_string().ok())
            else {
                return Err(fail(format!(
                    "required_by_step: Input should be a valid dictionary key, found {}",
                    python_type(step)
                )));
            };
            let rows = spec_rows(
                &format!("required_by_step[{step_name}]"),
                document.view(*value),
            )?;
            match required_by_step
                .iter_mut()
                .find(|(name, _)| name == &step_name)
            {
                Some(entry) => entry.1 = rows,
                None => required_by_step.push((step_name, rows)),
            }
        }
    }

    Ok(ExpectedArtifactManifest {
        schema_version,
        mission_type: declared_type,
        manifest_version,
        required_always,
        required_by_step,
        optional_always,
    })
}

/// Validate one spec row; the message names the field exactly as pydantic does.
fn parse_spec(
    document: &GraphDocument,
    node: NodeRef<'_>,
    label: &str,
) -> Result<ExpectedArtifactSpec, String> {
    let Some(entries) = node.as_mapping() else {
        return Err(format!(
            "{label}: Input should be a valid dictionary, found {}",
            python_type(node)
        ));
    };
    let allowed = ["artifact_key", "artifact_class", "path_pattern", "blocking"];
    for (key, _) in entries {
        let key_view = document.view(*key);
        let name = field_name(key_view);
        if !allowed.contains(&name.as_str()) {
            return Err(format!("{label}.{name}: Extra inputs are not permitted"));
        }
    }
    let required_text = |name: &str, non_empty: bool| -> Result<String, String> {
        match node.lookup(name) {
            None => Err(format!("{label}.{name}: Field required")),
            Some(field) => match field.as_str().and_then(|text| text.to_scalar_string().ok()) {
                Some(text) if !non_empty || !text.is_empty() => Ok(text),
                Some(_) => Err(format!(
                    "{label}.{name}: String should have at least 1 character"
                )),
                None => Err(format!(
                    "{label}.{name}: Input should be a valid string, found {}",
                    python_type(field)
                )),
            },
        }
    };
    let artifact_key = required_text("artifact_key", true)?;
    let path_pattern = required_text("path_pattern", true)?;
    let artifact_class = required_text("artifact_class", false)?;
    if !ARTIFACT_CLASSES.contains(&artifact_class.as_str()) {
        return Err(format!(
            "{label}.artifact_class: Input should be one of {}",
            ARTIFACT_CLASSES.join(", ")
        ));
    }
    let blocking = match node.lookup("blocking") {
        None => false,
        Some(field) => match field.as_bool() {
            Some(value) => value,
            None => {
                return Err(format!(
                    "{label}.blocking: Input should be a valid boolean, found {}",
                    python_type(field)
                ));
            }
        },
    };
    Ok(ExpectedArtifactSpec {
        artifact_key,
        artifact_class,
        path_pattern,
        blocking,
    })
}

/// The authored text of a manifest scalar, when it is a Python `str`.
///
/// Exposed for callers that need to compare a manifest value to a fixed token
/// without re-decoding the graph.
pub fn authored_text(value: &PythonString) -> Option<String> {
    value.to_scalar_string().ok()
}
