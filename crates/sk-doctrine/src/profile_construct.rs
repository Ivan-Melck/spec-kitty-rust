//! Compatibility adapter: the construction phase now lives in the Kernel floor.
//!
//! Source-exact construction of YAML core tags is shared by both Source dialects
//! (PyYAML's YAML 1.1 for `mission.yaml`, ruamel's YAML 1.2 for profiles), so the
//! mechanism is in `sk_kernel::python_yaml` and this module binds it to the
//! **ruamel** dialect. Every accessor and tag name is re-exported unchanged, so
//! the Doctrine model layer (`profile.rs`, `profile_schema.rs`, `org_charter.rs`,
//! `offering/*`) reads exactly what it read before.

use std::collections::BTreeMap;

use serde_yaml_ng::Value;

pub(crate) use sk_kernel::python_yaml::RuntimeCause;
pub(crate) use sk_kernel::python_yaml::YamlError as ProfileTextError;
pub(crate) use sk_kernel::python_yaml::{
    big_integer_value, constructed_big_integer, constructed_bytes, constructed_mapping,
    constructed_omap, constructed_pairs, constructed_sequence, constructed_set,
};

/// Construct every tagged node exactly as `ruamel.yaml`'s safe constructor does.
pub(crate) fn construct(
    value: &mut Value,
    markers: &BTreeMap<String, sk_kernel::PythonString>,
) -> Result<(), ProfileTextError> {
    sk_kernel::python_yaml::construct(
        value,
        markers,
        sk_kernel::python_yaml::Dialect::RuamelSafe12,
    )
}
