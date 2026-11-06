//! The YAML dialect seam for Mission definitions.
//!
//! Source reads `mission.yaml` with **PyYAML `safe_load`** and `.kittify/config.yaml`
//! with **ruamel's safe loader** (YAML 1.2 in the pinned Source). PyYAML uses
//! YAML 1.1 resolution and its own constructors, and those policies must stay
//! distinct even though both construct Python objects. Mission validation judges
//! those values. A YAML 1.2-core reader is *not* the same dialect, and the
//! differences are observable in this schema (probed against the pinned oracle):
//!
//! | document | Source (`safe_load`) | YAML 1.2 core (`serde_yaml_ng`) |
//! |---|---|---|
//! | `name: yes` | `bool True` → `Input should be a valid string` | `str "yes"` → accepted |
//! | `workspace: no` | `bool False` → invalid | `str "no"` |
//! | `version: 017` | `int 15` | `int 17` |
//! | `version: 0o17` | `str "0o17"` | `int 15` |
//! | `version: 1:30` | `int 90` (`str` field → refused) | `str "1:30"` |
//! | `workspace: 2026-11-05` | `date` → invalid | `str` |
//! | `agent_context: !!binary aGk=` | `bytes` → decoded `"hi"` | tag refused |
//! | `paths: {<<: *base}` | merge key applied | `"<<"` treated as a key |
//! | duplicate keys | last wins, accepted | refused |
//! | `!!set` | `set` | tag refused |
//!
//! The canonical loader therefore takes a [`SafeDocumentDecoder`] and never
//! guesses. Two real decoders ship:
//!
//! * [`PyYamlSafe`] — `mission.yaml`'s own loader, implemented by the shared
//!   Kernel decoder in the PyYAML/YAML 1.1 dialect
//!   ([`sk_kernel::python_yaml::Dialect::PyYamlSafe11`]);
//! * [`RuamelSafe`] — the YAML 1.2 dialect used by `.kittify/config.yaml`,
//!   offerings and profiles, for readers that need the *project* override
//!   (`path_conventions`), never for `mission.yaml`.
//!
//! [`CoreSchemaYaml`] remains only as an explicitly-distinct YAML 1.2-core
//! adapter (useful for cross-checks); it is **not** Source-faithful and must
//! never be used as a fallback for either Source loader.

use std::path::Path;

use sk_kernel::python_yaml::{Dialect, GraphDocument, OPEN_CASES, OpenCase, build_from_value};

use crate::definition::error::DefinitionError;

/// How closely a decoder's dialect matches a Source loader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialectFidelity {
    /// One of Source's own loaders, with the named constructions still open.
    ///
    /// `open` is never empty for the shared reader: a decoder that reproduces
    /// the exercised constructions can still be unfaithful elsewhere, so the
    /// unclosed cases travel with the verdict instead of being implied away.
    SourceDialect {
        /// Which Source loader this decoder models.
        dialect: Dialect,
        /// The Source constructs this decoder does not reproduce yet.
        open: &'static [OpenCase],
    },
    /// A different dialect: constructs in [`CORE_SCHEMA_DIVERGENCES`] decode
    /// differently.
    CoreSchemaOnly,
}

/// A YAML document decoder for Mission configuration.
///
/// `decode` receives the file's text (a leading UTF-8 BOM already stripped, as
/// PyYAML does) and returns the document as the ONE canonical object graph, or
/// the loader's message as the error detail. The graph — not a whole-root
/// acyclic projection — is what the typed readers consume, so a Source-valid
/// cycle in a field the model ignores is not falsely refused.
pub trait SafeDocumentDecoder {
    /// Decode one document into the canonical object graph.
    fn decode(&self, path: &Path, text: &str) -> Result<GraphDocument, String>;

    /// The decoder's dialect fidelity.
    fn fidelity(&self) -> DialectFidelity;
}

/// `mission.yaml`'s loader: PyYAML `safe_load` (YAML 1.1).
#[derive(Clone, Copy, Debug, Default)]
pub struct PyYamlSafe;

impl SafeDocumentDecoder for PyYamlSafe {
    fn decode(&self, _path: &Path, text: &str) -> Result<GraphDocument, String> {
        sk_kernel::python_yaml::decode_graph(text, Dialect::PyYamlSafe11)
            .map(GraphDocument::new)
            .map_err(|error| error.to_string())
    }

    fn fidelity(&self) -> DialectFidelity {
        DialectFidelity::SourceDialect {
            dialect: Dialect::PyYamlSafe11,
            open: OPEN_CASES,
        }
    }
}

/// `.kittify/config.yaml`'s loader: `ruamel.yaml.YAML(typ="safe")` (YAML 1.2).
///
/// Used by the project `path_conventions` reader — **not** for `mission.yaml`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuamelSafe;

impl SafeDocumentDecoder for RuamelSafe {
    fn decode(&self, _path: &Path, text: &str) -> Result<GraphDocument, String> {
        sk_kernel::python_yaml::decode_graph(text, Dialect::RuamelSafe12)
            .map(GraphDocument::new)
            .map_err(|error| error.to_string())
    }

    fn fidelity(&self) -> DialectFidelity {
        DialectFidelity::SourceDialect {
            dialect: Dialect::RuamelSafe12,
            open: OPEN_CASES,
        }
    }
}

/// The YAML 1.1 constructs where a 1.2-core reader provably diverges from
/// Source's safe loader. Documented as data so a caller (and the parent review)
/// can see exactly what [`CoreSchemaYaml`] does not reproduce.
pub const CORE_SCHEMA_DIVERGENCES: &[&str] = &[
    "plain scalars yes/no/on/off (YAML 1.1 booleans)",
    "leading-zero octal integers (017)",
    "0o-prefixed integers and sexagesimal integers (1:30)",
    "implicit !!timestamp values",
    "!!binary (bytes) and !!set tags",
    "mapping merge keys (<<)",
    "duplicate mapping keys (Source keeps the last value)",
];

/// `serde_yaml_ng`'s YAML 1.2-core reader, adapted onto the ONE canonical object
/// graph.
///
/// Use it only where a 1.2-core document is expected, or where the caller has
/// accepted the divergences above. It is never the canonical default and never a
/// fallback for a Source loader. Its own document is read by `serde_yaml_ng` on
/// purpose (that *is* the distinct dialect being modelled); the object model it
/// hands to callers is still the one canonical graph, built by the shared
/// builder, so consumers never see a second object authority.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoreSchemaYaml;

impl SafeDocumentDecoder for CoreSchemaYaml {
    fn decode(&self, _path: &Path, text: &str) -> Result<GraphDocument, String> {
        let value: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(text).map_err(|error| error.to_string())?;
        build_from_value(&value, Dialect::PyYamlSafe11)
            .map(GraphDocument::new)
            .map_err(|error| error.to_string())
    }

    fn fidelity(&self) -> DialectFidelity {
        DialectFidelity::CoreSchemaOnly
    }
}

/// Refuse to proceed without one of Source's own loader dialects.
///
/// The canonical entry points accept any [`SafeDocumentDecoder`]; this helper is
/// what a caller uses when it specifically requires the Source dialect, so a
/// 1.2-core read cannot silently stand in for a Mission's PyYAML gate. The
/// returned slice is the decoder's named open cases — never an empty promise.
pub fn require_source_faithful(
    decoder: &dyn SafeDocumentDecoder,
) -> Result<&'static [OpenCase], DefinitionError> {
    match decoder.fidelity() {
        DialectFidelity::SourceDialect { open, .. } => Ok(open),
        DialectFidelity::CoreSchemaOnly => Err(DefinitionError::DialectUnavailable {
            detail: "a YAML 1.2-core decoder cannot reproduce PyYAML safe_load; missing \
                     shared PyYAML-safe primitive (see CORE_SCHEMA_DIVERGENCES)"
                .to_owned(),
        }),
    }
}
