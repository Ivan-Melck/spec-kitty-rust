//! The ONE canonical raw `org-charter.yaml` reader, `extends:` walk and
//! per-field projections.
//!
//! Mirrors `src/charter/activation/org_pack_discovery.py`:
//!
//! * the raw document read (`_load_org_charter_doc`) and its catch phases;
//! * the declared-pack/`extends:` walk (`_iter_org_charter_docs`,
//!   `_chain_members`) including the **declared-alone** lenient fallback;
//! * the per-requested-field floor projection (`_required_values`);
//! * the coercing token readers (`read_org_required_ids`,
//!   `_read_org_required_selections`, `union_required_tokens`) which are
//!   deliberately a *different* API from the strict per-field floor read;
//! * the org `skill_namespace` fold (`read_org_skill_namespace`,
//!   `last_non_empty_token`).
//!
//! The reference's raw walk never validates the whole `OrgCharterPolicy`
//! schema: a malformed, irrelevant field (an ignored `schema_version`, an
//! `interview_defaults` of the wrong shape, an unknown key) must not erase the
//! pack's `required_<kind>` lists or its `extends:` contribution. The typed,
//! whole-policy capability is a separate Native contract
//! ([`super::load_org_charter_policy`]); this module is the raw one.
//!
//! **One read.** Every document is read exactly once, through the crate's single
//! lossless transport + constructor ([`ArtifactDocument`]): the same constructed
//! value tree, object graph and marker table the identity/offering readers use.
//! Projections then come from the Kernel graph authority (`str()`, truthiness,
//! `list` shape), never from a private walker or a re-parse.
//!
//! **Caught versus escaping.** The reference catches
//! `(OSError, YAMLError, ValueError)` per document and lets everything else
//! escape. [`read_raw`] preserves that split: a caught failure is `Unreadable` /
//! `NotMapping` (skipped when lenient, a refusal when strict), while an uncaught
//! constructor failure (`KeyError`, `TypeError`, `AssertionError`) becomes
//! [`DoctrineError::OrgCharterReadAborted`] and is never reclassified as a skip.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;

use crate::error::DoctrineError;
use crate::offering::value::ArtifactDocument;
use crate::org_charter::{
    ORG_CHARTER, ORG_CHARTER_SCHEMA_VERSION, OrgCharterDocument, org_requirable_kinds,
    required_key, strict_error, strip_token,
};

/// One raw `org-charter.yaml` document of the walk.
///
/// `member` is the pack whose own document this is; `declared` is the configured
/// pack whose `extends:` chain reached it (`member == declared` for the pack's
/// own document).
#[derive(Clone, Debug)]
pub struct OrgCharterRawDocument {
    declared: String,
    member: String,
    path: PathBuf,
    pack_root: PathBuf,
    document: ArtifactDocument,
}

impl OrgCharterRawDocument {
    /// The configured (declared) pack whose chain produced this document.
    pub fn declared(&self) -> &str {
        &self.declared
    }

    /// The chain member that authored this document.
    pub fn member(&self) -> &str {
        &self.member
    }

    /// The document file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pack root the document lives in.
    pub fn pack_root(&self) -> &Path {
        &self.pack_root
    }

    /// The `required_<plural>` ids of `kind`, exactly, per **requested field**.
    ///
    /// This is `_required_values(raw, field, member, strict)`:
    ///
    /// * an absent or null field is empty;
    /// * the field must be a list of strings — one non-string member invalidates
    ///   that whole field only (never the document, never another kind);
    /// * every item is `str.strip()`ed and empty items are dropped;
    /// * `strict` turns a malformed field into the reference's
    ///   `ActiveCharterConfigError`-shaped refusal.
    pub fn requirement_tokens(
        &self,
        kind: ArtifactKind,
        strict: bool,
    ) -> Result<Vec<PythonString>, DoctrineError> {
        let field = required_key(kind);
        let root = self.document.root();
        let Some(value) = root.lookup(&field) else {
            return Ok(Vec::new());
        };
        if value.is_null() {
            return Ok(Vec::new());
        }
        let Some(items) = value.as_sequence() else {
            return self.malformed_field(&field, strict);
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            let item = root.view(*item);
            let Some(text) = item.as_str() else {
                return self.malformed_field(&field, strict);
            };
            let token = strip_token(text);
            if !token.is_empty() {
                out.push(token);
            }
        }
        Ok(out)
    }

    fn malformed_field(
        &self,
        field: &str,
        strict: bool,
    ) -> Result<Vec<PythonString>, DoctrineError> {
        if strict {
            return Err(strict_error(format!(
                "`{field}` in the org-charter.yaml of org pack {:?} must be a list of ids.",
                self.member
            )));
        }
        Ok(Vec::new())
    }

    /// The `required_<plural>` ids of `kind` **coerced through Python `str()`**.
    ///
    /// This is `read_org_required_ids`'s law (`union_required_tokens`): any
    /// `list` value contributes `str(item).strip()` for every member, so
    /// `[1, true, null]` yields `["1", "True", "None"]`. A non-list value
    /// contributes nothing. It is deliberately NOT the strict per-field read
    /// above — the two APIs answer different questions and must not be merged.
    pub fn coerced_requirement_tokens(&self, kind: ArtifactKind) -> Vec<PythonString> {
        let root = self.document.root();
        let Some(value) = root.lookup(&required_key(kind)) else {
            return Vec::new();
        };
        let Some(items) = value.as_sequence() else {
            return Vec::new();
        };
        let mut out: Vec<PythonString> = Vec::with_capacity(items.len());
        for item in items {
            let token = strip_token(&root.view(*item).python_str());
            if !token.is_empty() && !out.contains(&token) {
                out.push(token);
            }
        }
        out
    }

    /// This document's `skill_namespace` (`last_non_empty_token`'s per-document
    /// step): a string only, stripped, empty ignored. No coercion, exactly like
    /// the reference's `isinstance(candidate, str)` guard.
    pub fn skill_namespace(&self) -> Option<PythonString> {
        stripped_string(&self.document, "skill_namespace")
    }

    /// This document's `org_name` (string only, stripped, empty ignored).
    pub fn org_name(&self) -> Option<PythonString> {
        stripped_string(&self.document, "org_name")
    }

    /// The `extends:` edge of this document: `str(raw["extends"])` when the
    /// authored value is **truthy**, else `None`.
    ///
    /// The reference stringifies whatever it finds (`5` -> `"5"`, `true` ->
    /// `"True"`, `[a]` -> `"['a']"`) and treats every falsy value (`0`, `""`,
    /// `false`, `[]`, `{}`, `null`) as "no base". Both halves are the
    /// reference's, not a schema opinion.
    pub fn extends(&self) -> Option<PythonString> {
        extends_edge(&self.document)
    }

    /// The authored `schema_version`, best effort.
    ///
    /// The raw readers never consult it (the reference's raw walk ignores it
    /// entirely); it exists for the projected document view only.
    pub fn schema_version(&self) -> i64 {
        let Some(value) = self.document.root().lookup("schema_version") else {
            return ORG_CHARTER_SCHEMA_VERSION;
        };
        value
            .as_int_text()
            .and_then(|text| text.parse::<i64>().ok())
            .or_else(|| {
                value
                    .as_str()
                    .and_then(|text| text.to_scalar_string().ok())
                    .and_then(|text| text.parse::<i64>().ok())
            })
            .unwrap_or(ORG_CHARTER_SCHEMA_VERSION)
    }

    /// The lenient per-field projection of this document into the shared
    /// `OrgCharterPolicy` shape.
    ///
    /// Every field is projected with the *lenient* rule, because the reference's
    /// raw walk validates nothing while walking: `required_<kind>` through
    /// [`Self::requirement_tokens`] (lenient), `extends` through the `str()`
    /// coercion, `org_name`/`skill_namespace` as strings. `interview_defaults`,
    /// `governance_policies` and `activations` are the typed loader's fields and
    /// stay empty here — the raw readers never read them.
    pub(crate) fn to_policy(&self) -> Result<super::OrgCharterPolicy, DoctrineError> {
        let mut required = BTreeMap::new();
        for kind in org_requirable_kinds() {
            required.insert(kind, self.requirement_tokens(kind, false)?);
        }
        Ok(super::OrgCharterPolicy {
            path: self.path.clone(),
            pack_root: self.pack_root.clone(),
            schema_version: super::PythonInt::from_i64(self.schema_version()),
            extends: self
                .extends()
                .and_then(|token| token.to_scalar_string().ok()),
            org_name: self.org_name(),
            interview_defaults: super::AuthoredDefaults::new(),
            required,
            skill_namespace: self.skill_namespace(),
            governance_policies: Vec::new(),
            activations: Vec::new(),
        })
    }
}

fn stripped_string(document: &ArtifactDocument, key: &str) -> Option<PythonString> {
    let text = document.root().lookup(key)?.as_str()?.clone();
    let token = strip_token(&text);
    (!token.is_empty()).then_some(token)
}

fn extends_edge(document: &ArtifactDocument) -> Option<PythonString> {
    let value = document.root().lookup("extends")?;
    if value.is_falsy() {
        return None;
    }
    Some(value.python_str())
}

/// The outcome of one raw document read.
enum RawRead {
    /// No `org-charter.yaml` in the pack root (`charter_path.exists()` false).
    Absent,
    /// Present but not a YAML mapping (an empty/whitespace/`null`/list document).
    NotMapping,
    /// Present but unreadable/refused by a **caught** phase.
    Unreadable(String),
    /// The constructed document.
    Mapping(ArtifactDocument),
}

/// The reference's per-document catch: `except (OSError, YAMLError, ValueError)`.
///
/// `ArtifactDocument` classifies the same phases: `Skip` is OSError /
/// `YAMLError` / an unknown tag, and a Python `ValueError` (a non-UTF-8 body, a
/// refused `int`/`float`/date) is caught too. `KeyError`, `TypeError` and
/// `AssertionError` are **not** caught by the reference and must escape.
fn is_caught(error: &crate::offering::value::ArtifactReadError) -> bool {
    match error.fatal() {
        None => true,
        Some(("ValueError", _)) => true,
        Some(_) => false,
    }
}

fn aborted(path: &Path, error: crate::offering::value::ArtifactReadError) -> DoctrineError {
    let (exception, message) = error
        .fatal()
        .map(|(python, message)| (python.to_owned(), message.to_owned()))
        .unwrap_or_else(|| ("Exception".to_owned(), error.to_string()));
    DoctrineError::OrgCharterReadAborted {
        path: path.to_path_buf(),
        exception,
        message,
    }
}

/// Read one pack's `org-charter.yaml` raw.
///
/// Absent/empty/non-mapping and caught failures are observations; an uncaught
/// constructor failure is an error that escapes every posture.
fn read_raw(path: &Path) -> Result<RawRead, DoctrineError> {
    if !sk_kernel::paths::source_path_exists(path)
        .map_err(|source| DoctrineError::io("inspect org charter", path, source))?
    {
        return Ok(RawRead::Absent);
    }
    let document = match ArtifactDocument::read(path) {
        Ok(document) => document,
        Err(error) if is_caught(&error) => return Ok(RawRead::Unreadable(error.to_string())),
        Err(error) => return Err(aborted(path, error)),
    };
    if !document.value.is_mapping() {
        return Ok(RawRead::NotMapping);
    }
    Ok(RawRead::Mapping(document))
}

/// Read one `org-charter.yaml` as a constructed mapping plus its exact-scalar
/// table (the typed loader's input).
///
/// `None` for an absent or empty file. A non-mapping document is the typed
/// loader's `InvalidOrgCharter` refusal; a document that makes the reference's
/// loader raise an uncaught Python exception is
/// [`DoctrineError::OrgCharterReadAborted`].
pub(crate) fn read_mapping(path: &Path) -> Result<Option<OrgCharterDocument>, DoctrineError> {
    if !path.is_file() {
        return Ok(None);
    }
    match read_raw(path)? {
        RawRead::Absent => Ok(None),
        RawRead::NotMapping => Err(DoctrineError::InvalidOrgCharter {
            path: path.to_path_buf(),
            message: "the org-charter.yaml root must be a mapping".to_owned(),
        }),
        RawRead::Unreadable(message) => Err(DoctrineError::InvalidOrgCharter {
            path: path.to_path_buf(),
            message,
        }),
        RawRead::Mapping(document) => {
            let mapping = document
                .value
                .as_mapping()
                .cloned()
                .expect("checked is_mapping");
            Ok(Some((mapping, document.markers().clone())))
        }
    }
}

/// The configured org packs, in config declaration order (name, effective root).
///
/// The registry is the canonical `charter_packs.org.packs[]` read
/// ([`sk_charter::load_org_pack_registry`]) and the path authority is
/// [`sk_charter::OrgPackConfig::effective_root`]; nothing else is consulted. In
/// particular the built-in tier is **not** required: an unavailable built-in
/// pack must never erase a readable org floor (the reference's
/// `_enumerate_org_pack_paths` never looks at it).
///
/// Posture (the reference's split):
///
/// * an unreadable/malformed registry is a strict refusal when strict and an
///   empty list when lenient (`load_pack_registry`'s own caught branch);
/// * a `local_path` that cannot be expanded (`${UNSET}`, `~` with no home, a
///   `subdir` escape) is a strict refusal when strict and **escapes** when
///   lenient, because the reference's list comprehension sits outside its `try`.
pub(crate) fn declared_org_packs(
    repo_root: &Path,
    strict: bool,
) -> Result<Vec<(String, PathBuf)>, DoctrineError> {
    let packs = match sk_charter::load_org_pack_registry(repo_root, &sk_charter::Limits::default())
    {
        Ok(packs) => packs,
        Err(error) if strict => {
            return Err(strict_error(format!(
                "The org pack registry in .kittify/config.yaml cannot be read: {error}"
            )));
        }
        Err(_) => return Ok(Vec::new()),
    };
    let expansion = sk_charter::PathExpansion::from_env();
    let mut declared = Vec::with_capacity(packs.len());
    for pack in packs {
        match pack.effective_root(repo_root, &expansion) {
            Ok(root) => declared.push((pack.name, root)),
            Err(error) if strict => {
                return Err(strict_error(format!(
                    "The org pack registry in .kittify/config.yaml cannot be read: {error}"
                )));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(declared)
}

/// Why an `extends:` chain cannot be resolved.
///
/// Shared with the typed pack-context door ([`super::chain`]), which maps it to
/// the reference's typed wording instead of the raw walk's. There is ONE walk.
pub(crate) enum ChainFailure {
    /// A base layer is not among the loaded documents (dangling `extends:`).
    MissingBase {
        missing: PythonString,
        chain: Vec<String>,
    },
    /// A layer already in the chain re-appears (including a self-edge).
    Cycle { cycle: Vec<String> },
}

impl ChainFailure {
    /// The raw walk's `resolve_extends_order` wording.
    fn message(&self) -> String {
        match self {
            Self::MissingBase { missing, chain } => format!(
                "Base layer {} not found. Chain: {}",
                missing.repr(),
                chain.join(" → ")
            ),
            Self::Cycle { cycle } => {
                format!("Cycle detected in extends: chain: {}", cycle.join(" → "))
            }
        }
    }
}

/// `declared` first, then its `extends:` bases nearest first.
///
/// The reference resolves the chain through its canonical
/// `resolve_extends_order` and reverses the base-first order. The topology is
/// validated fail-closed **before** any order is returned, so a strict caller
/// never folds a half-resolved chain.
///
/// This is the crate's ONE `extends:` walk: the raw reader and the typed
/// pack-context door both borrow it (`super::chain` reverses its result).
pub(crate) fn chain_members(
    declared: &str,
    edges: &BTreeMap<String, Option<PythonString>>,
) -> Result<Vec<String>, ChainFailure> {
    let mut chain = vec![declared.to_owned()];
    let mut visited: BTreeSet<String> = BTreeSet::from([declared.to_owned()]);
    let mut current = declared.to_owned();
    loop {
        let Some(base) = edges.get(&current) else {
            return Err(ChainFailure::MissingBase {
                missing: PythonString::from_scalars(&current),
                chain,
            });
        };
        let Some(base_exact) = base.clone() else {
            return Ok(chain);
        };
        // Registry names use the scalar config view; an exact non-scalar edge
        // cannot name one of those loaded keys. It is a dangling edge, not None.
        let Ok(base) = base_exact.to_scalar_string() else {
            return Err(ChainFailure::MissingBase {
                missing: base_exact,
                chain,
            });
        };
        if !edges.contains_key(&base) {
            return Err(ChainFailure::MissingBase {
                missing: base_exact,
                chain,
            });
        }
        if !visited.insert(base.clone()) {
            chain.push(base);
            return Err(ChainFailure::Cycle { cycle: chain });
        }
        chain.push(base.clone());
        current = base;
    }
}

/// The one raw walk: every declared pack and its `extends:` chain.
///
/// Failure order follows the reference exactly — the registry, then each
/// declared pack in declaration order (directory check, then document read),
/// then each declared pack's chain in declaration order. Lenient: an absent /
/// unreadable / non-mapping document is skipped and an invalid chain falls back
/// to the **declared pack alone** (never a partially visited prefix). Strict:
/// every one of those is a typed refusal.
pub(crate) fn walk(
    repo_root: &Path,
    strict: bool,
) -> Result<Vec<OrgCharterRawDocument>, DoctrineError> {
    let declared = declared_org_packs(repo_root, strict)?;
    let mut docs: BTreeMap<String, (PathBuf, ArtifactDocument)> = BTreeMap::new();
    for (name, root) in &declared {
        if strict && !root.is_dir() {
            return Err(strict_error(format!(
                "org pack {name:?} is declared but its path is not an existing directory ({}); \
                 run `spec-kitty charter fetch --pack {name}`.",
                root.display()
            )));
        }
        let path = root.join(ORG_CHARTER);
        match read_raw(&path)? {
            RawRead::Absent => {}
            RawRead::NotMapping => {
                if strict {
                    return Err(strict_error(format!(
                        "{} of org pack {name:?} is not a YAML mapping.",
                        path.display()
                    )));
                }
            }
            RawRead::Unreadable(message) => {
                if strict {
                    return Err(strict_error(format!(
                        "cannot read {} of org pack {name:?}: {message}",
                        path.display()
                    )));
                }
            }
            RawRead::Mapping(document) => {
                docs.entry(name.clone()).or_insert((path, document));
            }
        }
    }

    let roots: BTreeMap<&str, &Path> = declared
        .iter()
        .map(|(name, root)| (name.as_str(), root.as_path()))
        .collect();
    let edges: BTreeMap<String, Option<PythonString>> = docs
        .iter()
        .map(|(name, (_, document))| (name.clone(), extends_edge(document)))
        .collect();

    let mut out = Vec::new();
    for (name, _root) in &declared {
        let Some((path, _)) = docs.get(name) else {
            continue;
        };
        let members = match chain_members(name, &edges) {
            Ok(members) => members,
            Err(failure) if strict => {
                return Err(DoctrineError::OrgCharterExtends {
                    path: path.clone(),
                    message: failure.message(),
                });
            }
            Err(_) => vec![name.clone()],
        };
        for member in members {
            let Some((member_path, member_document)) = docs.get(&member) else {
                continue;
            };
            out.push(OrgCharterRawDocument {
                declared: name.clone(),
                path: member_path.clone(),
                pack_root: roots
                    .get(member.as_str())
                    .map(|root| (*root).to_path_buf())
                    .unwrap_or_else(|| member_path.parent().unwrap_or(member_path).to_path_buf()),
                member,
                document: member_document.clone(),
            });
        }
    }
    Ok(out)
}
