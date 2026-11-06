//! Read-only diagnostics for the two project registries `doctor` validates.
//!
//! * `spec-kitty doctor shim-registry` — [`shim`]: the compatibility-shim
//!   registry (`docs/migrations/shim-registry.yaml`) compared against the
//!   project's own `pyproject.toml` version.
//! * `spec-kitty doctor contracts` — [`contract`]: the Contract Registry
//!   (`docs/contracts/contract-registry.yaml`) validated for well-formedness,
//!   including the DIR-041 / NFR-003 ban on positional `file:line` anchors.
//!
//! # Source
//!
//! The reference implementation is, at the pinned baseline:
//!
//! | Surface | Reference |
//! | --- | --- |
//! | shim registry model + validation | `src/specify_cli/compat/registry.py` |
//! | shim status classification + project version | `src/specify_cli/compat/doctor.py` |
//! | contract registry model + validation | `src/specify_cli/contracts/registry.py` |
//! | positional-anchor predicate | `src/specify_cli/contracts/anchoring.py` |
//! | command shells, JSON documents, exit codes | `cli/commands/doctor.py` (`shim_registry`, `contracts`) |
//!
//! # Reused authorities (no second parser, model, version or reader)
//!
//! * **YAML** — both registries are read by `ruamel.yaml.YAML(typ="safe")` in
//!   Source, so both are decoded here through the ONE canonical kernel reader
//!   in its ruamel/YAML-1.2 dialect
//!   ([`sk_kernel::python_yaml::Dialect::RuamelSafe12`]), producing the shared
//!   [`sk_kernel::python_yaml::GraphDocument`]. There is no private YAML
//!   parser, value model or writer in this module.
//! * **Versions** — the release comparisons reuse this crate's canonical
//!   PEP 440 [`crate::version::Version`] (the same authority `upgrade`,
//!   `ledger` and `requires_core` use), which is what `packaging.version.Version`
//!   is in Source.
//! * **Character classes** — the `\w`, `\d` and whitespace rules of the
//!   Source regular expressions are the shared Kernel Python-text primitives
//!   ([`sk_kernel::python_text`]), never Rust's Unicode classes.
//! * **Strings** — authored values stay [`sk_kernel::PythonString`] (exact code
//!   points, including lone surrogates) until a JSON document needs scalar
//!   text; that conversion is fallible and never lossy.
//!
//! # Read-only contract
//!
//! Nothing here creates, rewrites, locks or deletes anything. The only
//! filesystem effects are `stat` (existence probes for shim modules) and reads.
//! A repair/extension of a registry is an operator action in the authored YAML.
//!
//! # Deliberate gaps (not claimed)
//!
//! * JSON object **key order**. Source builds its document with a Python dict
//!   and `json.dumps`, so its keys are emitted in declaration order. This
//!   workspace's `serde_json` is built without `preserve_order` (a manifest
//!   decision owned by the coordinator), so documents here carry the same keys
//!   and values in alphabetical order. JSON objects are unordered; the key set
//!   and every value are asserted.
//! * A project version that Source would reject with an uncaught
//!   `packaging.version.InvalidVersion` (or `KeyError` on a malformed
//!   `pyproject.toml`) is a typed
//!   [`RegistryDiagnosticsError::ProjectVersionInvalid`] /
//!   [`RegistryDiagnosticsError::ProjectKeyMissing`] here, reported as a
//!   configuration error by the command adapter. Source raises a traceback;
//!   no caller could read that as a passing check, and no exit-0 path exists
//!   for it here either.
//! * `is_file_line_anchor` is the Contract Registry's own DIR-041 predicate.
//!   It has no other native consumer yet; if another lane needs it (the
//!   architectural positional-anchor ratchets), it belongs in a shared home,
//!   not in a second copy — see the lane report's API request.

use std::fmt;
use std::path::{Path, PathBuf};

use sk_kernel::PythonString;
use sk_kernel::python_text::{is_decimal, is_whitespace, is_word};
use sk_kernel::python_yaml::{Dialect, GraphDocument, decode_graph, read_document_text};

pub mod contract;
pub mod shim;

/// The shim registry's canonical home inside a project (`ruamel`-read).
pub const SHIM_REGISTRY_LABEL: &str = "Shim registry";
/// The contract registry's canonical home inside a project (`ruamel`-read).
pub const CONTRACT_REGISTRY_LABEL: &str = "Contract registry";
/// The project metadata file the shim registry is compared against.
pub const PYPROJECT_FILE: &str = "pyproject.toml";

/// Field names that express a positional `file:line` anchor.
///
/// Any of these appearing anywhere in a Contract Record is rejected
/// (NFR-003 / DIR-041): the whole point of the registry is to anchor on
/// content, never on a line number that a benign edit moves.
pub const FORBIDDEN_POSITIONAL_FIELDS: [&str; 6] =
    ["file", "line", "lineno", "line_no", "file_line", "fileline"];

/// Which registry a failure refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryKind {
    /// `docs/migrations/shim-registry.yaml`
    Shim,
    /// `docs/contracts/contract-registry.yaml`
    Contract,
}

impl RegistryKind {
    /// The label Source uses in its not-found message.
    pub fn label(self) -> &'static str {
        match self {
            Self::Shim => SHIM_REGISTRY_LABEL,
            Self::Contract => CONTRACT_REGISTRY_LABEL,
        }
    }

    /// Wrap collected validation errors in this registry's schema failure.
    pub fn schema_failure(self, errors: Vec<String>) -> RegistryDiagnosticsError {
        match self {
            Self::Shim => RegistryDiagnosticsError::ShimSchema { errors },
            Self::Contract => RegistryDiagnosticsError::ContractSchema { errors },
        }
    }
}

/// A read-only registry/configuration failure.
///
/// Every variant keeps the identity of what failed (path, key, or the
/// collected validation messages), so a caller can branch without parsing a
/// message. The collected-error variants mirror Source's "validate the whole
/// document, then raise once" strategy: one malformed record never hides the
/// next.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryDiagnosticsError {
    /// The registry file does not exist at its canonical home.
    NotFound {
        /// Human label for the registry (`"Shim registry"`, …).
        label: &'static str,
        /// The canonical (missing) path.
        path: PathBuf,
    },
    /// `pyproject.toml` does not exist at the project root.
    PyprojectNotFound {
        /// The expected path.
        path: PathBuf,
    },
    /// `pyproject.toml` parsed but a required table/key is absent.
    ProjectKeyMissing {
        /// `"project"` or `"version"`.
        key: &'static str,
    },
    /// `pyproject.toml` is not valid TOML, or `project.version` is not a
    /// usable PEP 440 version string.
    ProjectVersionInvalid {
        /// What was observed.
        detail: String,
    },
    /// The shim registry failed schema validation.
    ShimSchema {
        /// Every collected violation, in Source's order.
        errors: Vec<String>,
    },
    /// The contract registry failed schema validation.
    ContractSchema {
        /// Every collected violation, in Source's order.
        errors: Vec<String>,
    },
    /// A file existed but could not be read.
    Unreadable {
        /// The path that failed.
        path: PathBuf,
        /// The operating system's message.
        detail: String,
    },
    /// Authored text or a path cannot be represented as JSON scalar text
    /// without inventing or replacing code points.
    Unencodable {
        /// Python `repr` of the offending value (exact, escaped).
        repr: String,
        /// What could not be encoded.
        reason: &'static str,
    },
}

impl RegistryDiagnosticsError {
    /// The stable machine-readable code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "registry_not_found",
            Self::PyprojectNotFound { .. } => "pyproject_not_found",
            Self::ProjectKeyMissing { .. } => "pyproject_key_missing",
            Self::ProjectVersionInvalid { .. } => "project_version_invalid",
            Self::ShimSchema { .. } => "shim_registry_schema",
            Self::ContractSchema { .. } => "contract_registry_schema",
            Self::Unreadable { .. } => "registry_unreadable",
            Self::Unencodable { .. } => "registry_text_unencodable",
        }
    }

    /// The collected schema violations, when this is a schema failure.
    pub fn schema_errors(&self) -> Option<&[String]> {
        match self {
            Self::ShimSchema { errors } | Self::ContractSchema { errors } => Some(errors),
            _ => None,
        }
    }
}

impl fmt::Display for RegistryDiagnosticsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { label, path } => {
                write!(formatter, "{label} not found at {}", path.display())
            }
            Self::PyprojectNotFound { path } => {
                write!(formatter, "pyproject.toml not found at {}", path.display())
            }
            // Source interpolates the `KeyError` argument, i.e. the repr of the
            // missing key: `missing key 'project' in pyproject.toml`.
            Self::ProjectKeyMissing { key } => {
                write!(formatter, "missing key '{key}' in pyproject.toml")
            }
            Self::ProjectVersionInvalid { detail } => formatter.write_str(detail),
            Self::ShimSchema { errors } | Self::ContractSchema { errors } => {
                formatter.write_str(&errors.join("\n"))
            }
            Self::Unreadable { path, detail } => {
                write!(formatter, "{} could not be read: {detail}", path.display())
            }
            Self::Unencodable { repr, reason } => {
                write!(formatter, "{reason}: {repr}")
            }
        }
    }
}

impl std::error::Error for RegistryDiagnosticsError {}

// ---------------------------------------------------------------------------
// Shared reads
// ---------------------------------------------------------------------------

/// Read and decode one registry document with the canonical ruamel-safe reader.
///
/// `path.exists()` is checked first so a missing registry raises the
/// forward-correct `NotFound` naming the canonical home, exactly as Source's
/// `resolve_*_path` + `exists()` pair does.
pub(crate) fn load_document(
    kind: RegistryKind,
    path: &Path,
) -> Result<GraphDocument, RegistryDiagnosticsError> {
    if !path.exists() {
        return Err(RegistryDiagnosticsError::NotFound {
            label: kind.label(),
            path: path.to_path_buf(),
        });
    }
    let bytes = std::fs::read(path).map_err(|error| RegistryDiagnosticsError::Unreadable {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;
    let decode = |detail: String| -> RegistryDiagnosticsError {
        kind.schema_failure(vec![format!("YAML parse error: {detail}")])
    };
    let text = read_document_text(&bytes, Dialect::RuamelSafe12)
        .map_err(|error| decode(error.to_string()))?;
    let graph =
        decode_graph(&text, Dialect::RuamelSafe12).map_err(|error| decode(error.to_string()))?;
    Ok(GraphDocument::new(graph))
}

/// Read `[project] version` from the project's `pyproject.toml`.
///
/// Source loads the whole file with `tomllib` and indexes
/// `["project"]["version"]`; a missing file is `FileNotFoundError` and a
/// missing key is `KeyError`. Both are typed here. The value is returned as the
/// authored string (Source emits it verbatim in its JSON document and parses it
/// separately for the comparison).
pub fn read_project_version(root: &Path) -> Result<String, RegistryDiagnosticsError> {
    let path = root.join(PYPROJECT_FILE);
    if !path.exists() {
        return Err(RegistryDiagnosticsError::PyprojectNotFound { path });
    }
    let bytes = std::fs::read(&path).map_err(|error| RegistryDiagnosticsError::Unreadable {
        path: path.clone(),
        detail: error.to_string(),
    })?;
    let text = String::from_utf8(bytes).map_err(|error| {
        RegistryDiagnosticsError::ProjectVersionInvalid {
            detail: format!("pyproject.toml is not valid UTF-8: {error}"),
        }
    })?;
    let document = text.parse::<toml_edit::DocumentMut>().map_err(|error| {
        RegistryDiagnosticsError::ProjectVersionInvalid {
            detail: format!("pyproject.toml is not valid TOML: {error}"),
        }
    })?;
    let project = document
        .get("project")
        .ok_or(RegistryDiagnosticsError::ProjectKeyMissing { key: "project" })?;
    let version = project
        .get("version")
        .ok_or(RegistryDiagnosticsError::ProjectKeyMissing { key: "version" })?;
    let version =
        version
            .as_str()
            .ok_or_else(|| RegistryDiagnosticsError::ProjectVersionInvalid {
                detail: "project.version in pyproject.toml is not a string".to_owned(),
            })?;
    Ok(version.to_owned())
}

/// Parse a PEP 440 version through the canonical crate authority.
pub(crate) fn parse_version(text: &str) -> Option<crate::version::Version> {
    crate::version::Version::parse(text).ok()
}

/// The exact code points of an authored string, as scalar text.
///
/// Fails (never substitutes) when the string carries a lone surrogate: a
/// `String` cannot hold one, and replacing it would silently rewrite authored
/// data.
pub(crate) fn scalar_text(
    text: &PythonString,
    reason: &'static str,
) -> Result<String, RegistryDiagnosticsError> {
    text.to_scalar_string()
        .map_err(|_| RegistryDiagnosticsError::Unencodable {
            repr: text.repr(),
            reason,
        })
}

/// The same, for a filesystem path in a JSON document.
pub(crate) fn path_text(path: &Path) -> Result<String, RegistryDiagnosticsError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| RegistryDiagnosticsError::Unencodable {
            repr: path.display().to_string(),
            reason: "a filesystem path is not valid UTF-8",
        })
}

/// A message-safe rendering of an authored string.
///
/// Diagnostic text must carry the exact code points; when they are not scalar
/// text (a lone surrogate), the Python `repr` (an escape sequence) is used
/// rather than a replacement character.
pub(crate) fn message_text(text: &PythonString) -> String {
    text.to_scalar_string().unwrap_or_else(|_| text.repr())
}

/// The Python final-newline semantics of the `$` anchor used by Source's
/// regular expressions: `$` matches at the end of the string *or* just before a
/// single trailing `\n`. Every "full match" predicate below therefore strips at
/// most one final `\n` before matching.
fn without_final_newline(codepoints: &[u32]) -> &[u32] {
    match codepoints.last() {
        Some(&last) if last == u32::from(b'\n') => &codepoints[..codepoints.len() - 1],
        _ => codepoints,
    }
}

fn is_ascii_letter(code: u32) -> bool {
    (u32::from(b'A')..=u32::from(b'Z')).contains(&code)
        || (u32::from(b'a')..=u32::from(b'z')).contains(&code)
}

fn is_ascii_alnum(code: u32) -> bool {
    is_ascii_letter(code) || (u32::from(b'0')..=u32::from(b'9')).contains(&code)
}

/// `^[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*$` (Python `\w`).
pub fn is_dotted_identifier(text: &PythonString) -> bool {
    let codepoints = without_final_newline(text.codepoints());
    let mut rest = codepoints;
    loop {
        let Some((&first, tail)) = rest.split_first() else {
            return false;
        };
        if !(is_ascii_letter(first) || first == u32::from(b'_')) {
            return false;
        }
        let end = tail
            .iter()
            .position(|code| !is_word(*code))
            .unwrap_or(tail.len());
        rest = &tail[end..];
        if rest.is_empty() {
            return true;
        }
        match rest.split_first() {
            Some((&dot, after)) if dot == u32::from(b'.') => rest = after,
            _ => return false,
        }
    }
}

/// `^\d+\.\d+\.\d+(?:[a-z]\d+)?$` (Python `\d`), Source's "semver-shaped"
/// release field gate. A shaped value may still fail PEP 440; that is the
/// separate `InvalidVersion` branch.
pub fn is_release_field(text: &PythonString) -> bool {
    let codepoints = without_final_newline(text.codepoints());
    let mut index = 0;
    // Three dot-separated runs of decimal digits.
    for segment in 0..3 {
        let start = index;
        while index < codepoints.len() && is_decimal(codepoints[index]) {
            index += 1;
        }
        if index == start {
            return false;
        }
        let needs_dot = segment < 2;
        if needs_dot && codepoints.get(index) != Some(&u32::from(b'.')) {
            return false;
        }
        if needs_dot {
            index += 1;
        }
    }
    let rest = &codepoints[index..];
    if rest.is_empty() {
        return true;
    }
    let Some((&letter, after)) = rest.split_first() else {
        return false;
    };
    if !(u32::from(b'a')..=u32::from(b'z')).contains(&letter) {
        return false;
    }
    !after.is_empty() && after.iter().all(|code| is_decimal(*code))
}

/// `^(#\d+|https?://.+)$` — a tracker reference is `#123` or an `http(s)` URL.
pub fn is_tracker_reference(text: &PythonString) -> bool {
    let codepoints = without_final_newline(text.codepoints());
    if let Some((&hash, rest)) = codepoints.split_first()
        && hash == u32::from(b'#')
        && !rest.is_empty()
        && rest.iter().all(|code| is_decimal(*code))
    {
        return true;
    }
    for prefix in ["http://", "https://"] {
        let prefix: Vec<u32> = prefix.chars().map(|character| character as u32).collect();
        if let Some(rest) = codepoints.strip_prefix(prefix.as_slice())
            && !rest.is_empty()
            && !rest.contains(&u32::from(b'\n'))
        {
            return true;
        }
    }
    false
}

/// Source's `_TRAILING_LINE_RE = r":(\d+)$"` and
/// `_PATHISH_PREFIX_RE = r"\.[A-Za-z0-9]+$"`, together with the "contains a
/// path separator" test: `contracts.anchoring.is_file_line_anchor`.
///
/// `path/to/file.py:42` and `pkg/mod:42` are positional anchors; a dotted
/// symbol, a `~/.kittify` path, a semver or a tracker ref are not.
pub fn is_file_line_anchor(text: &PythonString) -> bool {
    let codepoints = text.codepoints();
    let start = codepoints
        .iter()
        .position(|code| !is_whitespace(*code))
        .unwrap_or(codepoints.len());
    let end = codepoints
        .iter()
        .rposition(|code| !is_whitespace(*code))
        .map(|index| index + 1)
        .unwrap_or(start);
    let stripped = &codepoints[start..end];

    let digits = stripped
        .iter()
        .rev()
        .take_while(|code| is_decimal(**code))
        .count();
    if digits == 0 || digits >= stripped.len() {
        return false;
    }
    let colon = stripped.len() - digits - 1;
    if stripped[colon] != u32::from(b':') {
        return false;
    }
    let prefix = &stripped[..colon];
    if prefix.is_empty() {
        return false;
    }
    prefix.contains(&u32::from(b'/'))
        || prefix.contains(&u32::from(b'\\'))
        || has_file_extension(prefix)
}

/// The `_PATHISH_PREFIX_RE` half: a trailing `.` followed by ASCII alphanumerics.
fn has_file_extension(prefix: &[u32]) -> bool {
    let Some(dot) = prefix.iter().rposition(|code| *code == u32::from(b'.')) else {
        return false;
    };
    let extension = &prefix[dot + 1..];
    !extension.is_empty() && extension.iter().all(|code| is_ascii_alnum(*code))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> PythonString {
        PythonString::from_scalars(value)
    }

    #[test]
    fn dotted_identifier_matches_python_word_semantics() {
        assert!(is_dotted_identifier(&text("specify_cli.compat.registry")));
        assert!(is_dotted_identifier(&text("new_module")));
        assert!(is_dotted_identifier(&text("_private.module2")));
        assert!(is_dotted_identifier(&text("módulo.público")));
        assert!(!is_dotted_identifier(&text("123-not-a-dotted-name")));
        assert!(!is_dotted_identifier(&text("a..b")));
        assert!(!is_dotted_identifier(&text("a.")));
        assert!(!is_dotted_identifier(&text("")));
        assert!(!is_dotted_identifier(&text("a-b")));
        // Python's `$` also matches before one trailing newline.
        assert!(is_dotted_identifier(&text("a.b\n")));
        assert!(!is_dotted_identifier(&text("a.b\n\n")));
    }

    #[test]
    fn release_field_is_the_source_semver_shape() {
        assert!(is_release_field(&text("1.2.3")));
        assert!(is_release_field(&text("3.2.0a1")));
        assert!(is_release_field(&text("1.2.3z1")));
        assert!(!is_release_field(&text("1.2")));
        assert!(!is_release_field(&text("v1.2.3")));
        assert!(!is_release_field(&text("1.2.3.4")));
        assert!(!is_release_field(&text("1.2.3a")));
        assert!(!is_release_field(&text("not-a-version")));
        // Source's pre-release suffix is ONE ASCII letter plus digits, so the
        // `rc` spelling a *project version* uses is not a registry release field.
        assert!(!is_release_field(&text("4.0.0rc8")));
    }

    #[test]
    fn tracker_reference_accepts_hash_and_urls() {
        assert!(is_tracker_reference(&text("#615")));
        assert!(is_tracker_reference(&text("#999")));
        assert!(is_tracker_reference(&text(
            "http://jira.example.com/PROJ-42"
        )));
        assert!(is_tracker_reference(&text(
            "https://github.com/o/r/issues/1"
        )));
        assert!(!is_tracker_reference(&text("#")));
        assert!(!is_tracker_reference(&text("615")));
        assert!(!is_tracker_reference(&text("ftp://example.com")));
        assert!(!is_tracker_reference(&text("http://")));
    }

    #[test]
    fn positional_anchor_detector_matches_source() {
        assert!(is_file_line_anchor(&text("src/specify_cli/foo.py:42")));
        assert!(is_file_line_anchor(&text(
            "tests/architectural/_ratchet_keys.py:229"
        )));
        assert!(is_file_line_anchor(&text("docs/status-model.md:7")));
        assert!(is_file_line_anchor(&text("pkg/mod:42")));
        assert!(is_file_line_anchor(&text("  src/a.py:1  ")));
        assert!(!is_file_line_anchor(&text("~/.kittify")));
        assert!(!is_file_line_anchor(&text("~/.spec-kitty")));
        assert!(!is_file_line_anchor(&text(
            "specify_cli.status.emit.emit_status_transition"
        )));
        assert!(!is_file_line_anchor(&text("3.4.0")));
        assert!(!is_file_line_anchor(&text("#2077")));
        assert!(!is_file_line_anchor(&text("status commit")));
        assert!(!is_file_line_anchor(&text(":42")));
        assert!(!is_file_line_anchor(&text("a:1:2:3")));
    }
}
