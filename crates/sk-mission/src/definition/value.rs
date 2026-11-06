//! PyYAML-`safe_load` document values and authored-order string maps.
//!
//! Source reads `mission.yaml` with `yaml.safe_load` and `.kittify/config.yaml`
//! with `ruamel.yaml`'s safe loader. Both produce Python objects, not JSON, and
//! validation then judges those *Python* values and names their Python type.
//!
//! The value domain is **not** defined here any more: it is the Kernel floor's
//! one canonical Python value type ([`sk_kernel::python_value::PythonValue`]),
//! re-exported under this module's historical names so the definition model, the
//! path-convention reader and the shared decoder share a single within-build
//! type. There is no second DTO and no lossy bridge.
//!
//! Keeping the domain explicit is what lets the definition model reproduce
//! Source acceptance and rejection exactly, instead of narrowing a document to
//! what a Rust struct happens to deserialize.

pub use sk_kernel::python_value::{OrderedMap, PythonValue as SourceValue};
