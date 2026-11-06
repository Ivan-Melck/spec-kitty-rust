//! Compatibility adapter: the lossless transport now lives in the Kernel floor.
//!
//! The authored-text carrier (lossless CST → injective markers → one semantic
//! parse) is a *shared* Python-safe YAML authority, not a profile-private one:
//! `mission.yaml` needs the same mechanism with the PyYAML dialect. It lives in
//! `sk_kernel::python_yaml`, which owns both dialects; this module keeps the
//! `crate::profile_text` call surface every Doctrine consumer already uses and
//! binds it to the **ruamel** dialect (`YAML(typ="safe")`, YAML 1.2), which is
//! what profiles, offerings and activation artifacts are read with.
//!
//! Behavior is unchanged: the same marker machinery, the same double-quoted
//! decoder, the same merge-key rules, the same duplicate-key refusal and the same
//! error texts. The unit tests for that machinery moved with it.

use std::collections::BTreeMap;

use serde_yaml_ng::Value;

pub(crate) use sk_kernel::python_yaml::LosslessText;
pub(crate) use sk_kernel::python_yaml::YamlError as ProfileTextError;
pub use sk_kernel::python_yaml::{is_marker_text, marker_for_exact};

/// The dialect every Doctrine artifact is authored in.
pub(crate) const DIALECT: sk_kernel::python_yaml::Dialect =
    sk_kernel::python_yaml::Dialect::RuamelSafe12;

/// Transport `text` into the marked form plus its exact values.
pub(crate) fn transport(text: &str) -> Result<LosslessText, ProfileTextError> {
    sk_kernel::python_yaml::transport(text, DIALECT)
}

/// Check the transport invariant after semantic parsing.
pub(crate) fn verify_markers(
    value: &Value,
    markers: &BTreeMap<String, sk_kernel::PythonString>,
) -> Result<(), ProfileTextError> {
    sk_kernel::python_yaml::verify_markers(value, markers)
}

/// Resolve YAML merge keys on an already-parsed document.
pub(crate) fn resolve_merge_keys(value: &mut Value) -> Result<(), ProfileTextError> {
    sk_kernel::python_yaml::resolve_merge_keys(value, DIALECT)
}

#[cfg(test)]
mod adapter_tests {
    use super::*;

    /// The adapter is a *type alias*, not a second error model: a value of the
    /// Kernel error type is exactly the Doctrine error type (same TypeId), so no
    /// consumer can hold two incompatible error models.
    #[test]
    fn the_error_type_is_the_kernel_type() {
        fn same_type(error: sk_kernel::python_yaml::YamlError) -> ProfileTextError {
            error
        }
        let error = sk_kernel::python_yaml::YamlError::Construct("x".to_owned());
        assert_eq!(same_type(error.clone()), error);
    }

    /// The dialect this adapter binds is ruamel's (YAML 1.2), never PyYAML's.
    #[test]
    fn the_adapter_binds_the_ruamel_dialect() {
        assert_eq!(DIALECT, sk_kernel::python_yaml::Dialect::RuamelSafe12);
        // ruamel refuses a duplicate key (PyYAML's dialect would not).
        assert!(DIALECT.refuses_duplicate_keys());
    }
}
