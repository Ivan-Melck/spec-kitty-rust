//! The one constructed-YAML read/text authority for offering artifacts.
//!
//! Every artifact document (identity index, offering repositories, mission-type
//! roster) is read here, through the crate's single lossless transport
//! ([`crate::profile_text`]) and single construction phase
//! ([`crate::profile_construct`]). That is the reference's own shape:
//! `pack_manager._declared_id`/`kind_vocabulary._read_id` and
//! `BaseArtifactRepository._load` all run the same `ruamel.yaml` safe loader
//! before they look at a field.
//!
//! # Read outcomes, not one `None`
//!
//! The reference distinguishes two failures, and so does this module:
//!
//! * **skip** — an unreadable file, a YAML syntax error, a refused/unknown tag
//!   (`ConstructorError`, a `YAMLError`), a non-UTF-8 body that
//!   `ruamel` reports as a `YAMLError`-adjacent read failure: the repositories
//!   catch these per file and warn.
//! * **fatal** — a Python exception the reference's `except (YAMLError,
//!   ValidationError, OSError)` does **not** catch: the bare `KeyError` from an
//!   invalid `!!bool` spelling, a `ValueError` from `int()`/`float()`/date
//!   construction, the `TypeError` from an unhashable `!!set` key, the
//!   `AssertionError` from a duplicate `!!omap` key. These abort the whole
//!   repository load; [`ArtifactReadError::Fatal`] carries the exception name
//!   so a caller can reproduce the phase instead of pretending the file was
//!   skipped.
//!
//! An invalid UTF-8 body is the one case where the reference raises
//! `UnicodeDecodeError` (a `ValueError`) inside the `read_text` call, so it is
//! modelled as fatal too.
//!
//! # `str()` / `repr()` over the constructed domain
//!
//! [`ArtifactDocument::python_str`] and [`ArtifactDocument::python_repr`] are the
//! one Python `str()`/`repr()` implementation for constructed YAML values, used
//! by the identity index (`pack_manager._declared_id`) and by the offering
//! loader. They cover the whole domain the constructor can produce: exact
//! (marker-carried) strings, `bytes`, dates/datetimes, arbitrary integers,
//! floats (through the shared kernel codec), `null`/bools, lists, dicts, sets,
//! `!!omap`, `!!pairs` and nested combinations.
//!
//! Container rendering is `repr()` of the *element*, exactly like CPython:
//! `str([1, 2])` is `"[1, 2]"` and `str([["a"]])` is `"[['a']]"`.

use std::collections::BTreeMap;
use std::path::Path;

use serde_yaml_ng::Value;
use sk_kernel::PythonString;
use sk_kernel::python_int::IntDecimalError;
use sk_kernel::python_yaml::{NodeRef, ObjectGraph, build_from_value_with_markers};

use crate::profile_text::ProfileTextError;

/// A reason a document is *skipped* (the reference's per-file catch).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SkipReason {
    /// The file could not be read (`OSError`).
    Unreadable(String),
    /// The document is not YAML (`YAMLError`).
    Syntax(String),
    /// The safe constructor refused the document (`ConstructorError`).
    Construct(String),
    /// The lossless transport could not carry the text.
    Transport(String),
}

/// Why a document could not be read at all.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ArtifactReadError {
    /// The reference skips this file and keeps loading.
    Skip(SkipReason),
    /// The reference's loader does not catch this: the whole load aborts.
    Fatal {
        /// The Python exception name (`KeyError`, `ValueError`, `TypeError`,
        /// `AssertionError`).
        python: &'static str,
        /// Its message.
        message: String,
    },
}

impl ArtifactReadError {
    /// The fatal exception name and message, when this aborts the load.
    pub(crate) fn fatal(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::Fatal { python, message } => Some((python, message)),
            Self::Skip(_) => None,
        }
    }
}

impl std::fmt::Display for ArtifactReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skip(reason) => match reason {
                SkipReason::Unreadable(message) => write!(formatter, "unreadable: {message}"),
                SkipReason::Syntax(message) => write!(formatter, "invalid YAML: {message}"),
                SkipReason::Construct(message) => formatter.write_str(message),
                SkipReason::Transport(message) => formatter.write_str(message),
            },
            Self::Fatal { python, message } => write!(formatter, "{python}: {message}"),
        }
    }
}

/// One artifact document: its constructed value tree, the ONE canonical object
/// graph of that same construction (the identity/text authority), and the
/// transport markers that carry lossy (surrogate-bearing) strings exactly.
#[derive(Clone, Debug)]
pub(crate) struct ArtifactDocument {
    pub(crate) value: Value,
    graph: ObjectGraph,
    markers: BTreeMap<String, PythonString>,
}

impl ArtifactDocument {
    /// Read one artifact file through the one lossless transport + constructor.
    ///
    /// # Errors
    ///
    /// [`ArtifactReadError::Skip`] for a per-file failure, [`ArtifactReadError::Fatal`]
    /// for the Python exceptions the reference's loader does not catch.
    pub(crate) fn read(path: &Path) -> Result<Self, ArtifactReadError> {
        let bytes = std::fs::read(path)
            .map_err(|error| ArtifactReadError::Skip(SkipReason::Unreadable(error.to_string())))?;
        let text = String::from_utf8(bytes).map_err(|error| ArtifactReadError::Fatal {
            python: "ValueError",
            message: format!("utf-8 decode failed: {error}"),
        })?;
        Self::from_text(&text)
    }

    /// Build one artifact document from text that was already read and decoded.
    ///
    /// This is the SAME transport/parse/construct/graph pipeline [`Self::read`] uses,
    /// exposed so a caller that already holds the decoded text (the identity index's
    /// one ordinary read) hands that text to the canonical parser instead of reopening
    /// the file or stringifying a second parse. No parser, grammar or codec is
    /// duplicated, and no YAML dialect or tag behaviour changes.
    ///
    /// # Errors
    ///
    /// [`ArtifactReadError::Skip`] for a per-file failure, [`ArtifactReadError::Fatal`]
    /// for the Python exceptions the reference's loader does not catch.
    pub(crate) fn from_text(text: &str) -> Result<Self, ArtifactReadError> {
        let lossless = crate::profile_text::transport(text).map_err(classify)?;
        let mut value: Value = serde_yaml_ng::from_str(lossless.shadow())
            .map_err(|error| ArtifactReadError::Skip(SkipReason::Syntax(error.to_string())))?;
        crate::profile_text::verify_markers(&value, lossless.markers())
            .map_err(|error| ArtifactReadError::Skip(SkipReason::Transport(error.to_string())))?;
        crate::profile_text::resolve_merge_keys(&mut value)
            .map_err(|error| ArtifactReadError::Skip(SkipReason::Construct(error.to_string())))?;
        crate::profile_construct::construct(&mut value, lossless.markers()).map_err(classify)?;
        // ONE object model: the same construction is also the canonical graph, so
        // `str()`/`repr()` come from the Kernel authority instead of a private
        // walker here. (The tree stays as the schema validator's input; projection
        // is never the authority.)
        let graph =
            build_from_value_with_markers(&value, lossless.markers(), crate::profile_text::DIALECT)
                .map_err(|error| {
                    ArtifactReadError::Skip(SkipReason::Construct(error.to_string()))
                })?;
        Ok(Self {
            value,
            graph,
            markers: lossless.markers().clone(),
        })
    }

    /// The document root as a borrowed view of the ONE canonical graph.
    pub(crate) fn root(&self) -> NodeRef<'_> {
        self.graph.view(self.graph.root())
    }

    /// The exact code points of one YAML string node (its marker, else its text).
    pub(crate) fn exact_string(&self, text: &str) -> PythonString {
        self.markers
            .get(text)
            .cloned()
            .unwrap_or_else(|| PythonString::from_scalars(text))
    }

    /// The transport markers this document's lossless read recovered.
    ///
    /// A marker is an injective, content-addressed spelling of one exact code-point
    /// string ([`sk_kernel::python_yaml::marker_for_exact`]), so a union of two
    /// documents' marker tables names the same exact strings the two reads
    /// recovered — it cannot alias one document's scalar to another's.
    pub(crate) fn markers(&self) -> &BTreeMap<String, PythonString> {
        &self.markers
    }

    /// Python `str(value)` for one graph node.
    ///
    /// A Python `str` is itself (no quotes, exact code points); `bytes`,
    /// dates/datetimes and every container render their own `str()` — which for
    /// containers *is* their `repr()`, including the structural `[...]`/`{...}`
    /// recursion marker for a cycle. The text is the Kernel graph authority's,
    /// not a private walker here.
    ///
    /// This entry point renders under the explicit **unlimited** integer policy; it
    /// is the inspection/diagnostic facade. A caller observing Source's eager
    /// `str()` phase must use [`Self::try_python_str`] with the interpreter's
    /// configured ceiling.
    pub(crate) fn python_str(&self, node: NodeRef<'_>) -> PythonString {
        self.graph.python_str(node.id())
    }

    /// Python `str(value)` under the interpreter's decimal integer ceiling.
    ///
    /// # Errors
    ///
    /// [`IntDecimalError::DigitLimit`] when a demanded `int` in the subtree would
    /// exceed the configured conversion ceiling.
    pub(crate) fn try_python_str(
        &self,
        node: NodeRef<'_>,
        digit_limit: usize,
    ) -> Result<PythonString, IntDecimalError> {
        self.graph.try_python_str(node.id(), digit_limit)
    }

    /// Python `repr(value)` for one graph node.
    ///
    /// Kept as the shared repr adapter for diagnostics and tests: a caller that
    /// needs `repr()` (rather than `str()`) must come here, not to a local walker.
    #[allow(dead_code)]
    pub(crate) fn python_repr(&self, node: NodeRef<'_>) -> PythonString {
        PythonString::from_scalars(&self.graph.repr(node.id()))
    }

    /// The value as a Python `str` when the node is one (or constructed `bytes`
    /// that decode as UTF-8 — Pydantic's lax `str` coercion accepts `bytes`).
    pub(crate) fn python_string_value(&self, value: &Value) -> Option<PythonString> {
        match value {
            Value::String(text) => Some(self.exact_string(text)),
            Value::Tagged(tagged) => {
                let bytes = crate::profile_construct::constructed_bytes(value)?;
                let _ = tagged;
                std::str::from_utf8(&bytes)
                    .ok()
                    .map(PythonString::from_scalars)
            }
            _ => None,
        }
    }
}

/// The ONE canonical graph of an already-merged mapping.
///
/// The layered repository merges overlay fields onto the built-in **constructed**
/// mapping (raw shallow field replacement, the reference's `_merge`), so the
/// merged record has no graph of its own. This builder hands that merged mapping
/// and the union of the contributing documents' transport markers to the **same**
/// Kernel constructor [`ArtifactDocument::read`] uses — no file is reopened, no
/// text is re-serialised or re-parsed, and no second merger/graph authority
/// appears.
///
/// `markers` must be the union of every contributing layer's
/// [`ArtifactDocument::markers`]. `None` is an explicit capability boundary: the
/// merged mapping is a shape this crate's constructor cannot canonicalise.
pub(crate) fn exact_merged_graph(
    merged: &Value,
    markers: &BTreeMap<String, PythonString>,
) -> Option<ObjectGraph> {
    build_from_value_with_markers(merged, markers, crate::profile_text::DIALECT).ok()
}

/// Classify a transport/constructor failure into the reference's read phases.
fn classify(error: ProfileTextError) -> ArtifactReadError {
    match error {
        ProfileTextError::Syntax(message) => ArtifactReadError::Skip(SkipReason::Syntax(message)),
        ProfileTextError::Construct(message) | ProfileTextError::Merge(message) => {
            ArtifactReadError::Skip(SkipReason::Construct(message))
        }
        ProfileTextError::Decode(message) | ProfileTextError::ForeignMarker(message) => {
            ArtifactReadError::Skip(SkipReason::Transport(message))
        }
        ProfileTextError::Runtime { cause, message } => ArtifactReadError::Fatal {
            python: python_name(cause),
            message,
        },
        ProfileTextError::HashSeed(error) => ArtifactReadError::Fatal {
            python: "RuntimeError",
            message: error.to_string(),
        },
    }
}

fn python_name(cause: crate::profile_construct::RuntimeCause) -> &'static str {
    match cause {
        crate::profile_construct::RuntimeCause::Key => "KeyError",
        crate::profile_construct::RuntimeCause::Value => "ValueError",
        crate::profile_construct::RuntimeCause::Type => "TypeError",
        crate::profile_construct::RuntimeCause::Assertion => "AssertionError",
    }
}

/// The one text authority for artifact documents is the canonical graph
/// ([`sk_kernel::python_yaml::ObjectGraph::python_str`] / `repr`), reached through
/// [`ArtifactDocument::python_str`] and [`ArtifactDocument::python_repr`]. The
/// per-kind `repr` walkers that used to live here are gone on purpose: a second
/// walker is exactly the fork this migration removes.
#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> ArtifactDocument {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("doc.yaml");
        std::fs::write(&path, text).expect("write");
        ArtifactDocument::read(&path).expect("read")
    }

    /// Render the `id:` value of a one-field document (the position the offering
    /// and identity readers actually consult).
    fn rendered(text: &str) -> String {
        let document = document(&format!("id: {text}"));
        document
            .python_str(document.root().lookup("id").expect("id"))
            .to_scalar_string()
            .expect("scalar")
    }

    #[test]
    fn repr_and_str_come_from_the_one_graph_authority() {
        // No private walker: both texts are the Kernel graph authority's, and a
        // container's `str()` is its `repr()`.
        let document = document("id: {a: [1, 2]}");
        let node = document.root().lookup("id").expect("id");
        let repr = document
            .python_repr(node)
            .to_scalar_string()
            .expect("scalar");
        assert_eq!(repr, "{'a': [1, 2]}");
        let str_text = document
            .python_str(node)
            .to_scalar_string()
            .expect("scalar");
        assert_eq!(str_text, repr);
    }

    #[test]
    fn strings_render_their_own_str_but_containers_repr_their_elements() {
        let document = document("id: \"it's\"");
        assert_eq!(
            document
                .python_str(document.root().lookup("id").expect("id"))
                .to_scalar_string()
                .expect("scalar"),
            "it's"
        );
        assert_eq!(rendered("[1, 2]"), "[1, 2]");
        assert_eq!(
            rendered("[[1, 2], {a: [b, 3]}]"),
            "[[1, 2], {'a': ['b', 3]}]"
        );
        assert_eq!(rendered("{x: 1, y: 2}"), "{'x': 1, 'y': 2}");
        assert_eq!(rendered("[]"), "[]");
        assert_eq!(rendered("{}"), "{}");
    }

    #[test]
    fn constructed_kinds_render_their_python_str() {
        assert_eq!(rendered("!!binary SGVsbG8="), "b'Hello'");
        assert!(
            rendered("!!set {alpha: null}") == "{'alpha'}",
            "{}",
            rendered("!!set {alpha: null}")
        );
        assert_eq!(rendered("!!set {}"), "set()");
        assert_eq!(rendered("!!pairs [{a: b}]"), "[('a', 'b')]");
        assert_eq!(rendered("!!omap [{a: b}]"), "ordereddict([('a', 'b')])");
        assert_eq!(rendered("2020-01-01"), "2020-01-01");
        assert_eq!(rendered("2020-01-01T01:02:03"), "2020-01-01 01:02:03");
        assert_eq!(
            rendered("2020-01-01T01:02:03Z"),
            "2020-01-01 01:02:03+00:00"
        );
        assert_eq!(rendered("1e20"), "1e+20");
        assert_eq!(
            rendered("123456789012345678901234567890"),
            "123456789012345678901234567890"
        );
    }

    #[test]
    fn read_phases_distinguish_skip_from_fatal() {
        let directory = tempfile::tempdir().expect("tempdir");
        let broken = directory.path().join("broken.yaml");
        std::fs::write(&broken, "id: [broken\n").expect("write");
        assert!(matches!(
            ArtifactDocument::read(&broken),
            Err(ArtifactReadError::Skip(_))
        ));
        let badbool = directory.path().join("badbool.yaml");
        std::fs::write(&badbool, "id: !!bool notabool\n").expect("write");
        let error = ArtifactDocument::read(&badbool).expect_err("fatal");
        assert_eq!(error.fatal().map(|(python, _)| python), Some("KeyError"));
        let unknown = directory.path().join("unknown.yaml");
        std::fs::write(&unknown, "id: !custom hello\n").expect("write");
        assert!(matches!(
            ArtifactDocument::read(&unknown),
            Err(ArtifactReadError::Skip(_))
        ));
    }
}
