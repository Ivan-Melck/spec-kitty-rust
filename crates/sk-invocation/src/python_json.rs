//! Compatibility names for the single lower-layer Python JSON authority.
//!
//! Kernel owns the existing implementation: the ordered value tree, the exact
//! code-point reader and typed Source errors, the object builder, the string
//! encoder and the float formatter. These are exact re-exports, not wrappers or
//! a second parser/formatter. Legacy parser/claim behavior is unchanged; the
//! legacy Op adapter (`crate::legacy_json`) keeps its names on top of these
//! same items.

pub(crate) use sk_kernel::python_json::python_float_repr;
pub use sk_kernel::python_json::{
    PythonJsonDict, PythonJsonError, PythonJsonErrorKind, PythonJsonObject,
    PythonJsonRenderOptions, PythonJsonValue, SOURCE_DEFAULT_INTEGER_DIGITS,
    SOURCE_DEFAULT_RECURSION_BUDGET, environment_integer_digits, is_falsy, parse,
    parse_python_string, parse_python_string_with_integer_digits, parse_str_with_integer_digits,
    push_python_string, python_repr, python_repr_json, python_repr_value, python_seconds,
    source_integer_digit_limit,
};

#[cfg(test)]
mod tests {
    use super::{PythonJsonObject, PythonJsonValue};

    #[test]
    fn both_legacy_json_types_are_same_build_kernel_aliases() {
        use std::any::TypeId;
        assert_eq!(
            TypeId::of::<PythonJsonObject>(),
            TypeId::of::<sk_kernel::python_json::PythonJsonObject>()
        );
        assert_eq!(
            TypeId::of::<PythonJsonValue>(),
            TypeId::of::<sk_kernel::python_json::PythonJsonValue>()
        );
        let object: PythonJsonObject = sk_kernel::python_json::PythonJsonObject::new();
        let back: sk_kernel::python_json::PythonJsonObject = object;
        assert_eq!(back.finish(), "{}");
        let value: PythonJsonValue = sk_kernel::python_json::PythonJsonValue::scalar("x");
        let back: sk_kernel::python_json::PythonJsonValue = value;
        assert_eq!(back.render(None), "\"x\"");
    }

    #[test]
    fn read_and_render_authorities_are_same_build_kernel_items() {
        use std::any::TypeId;
        assert_eq!(
            TypeId::of::<super::PythonJsonDict>(),
            TypeId::of::<sk_kernel::python_json::PythonJsonDict>()
        );
        assert_eq!(
            TypeId::of::<super::PythonJsonError>(),
            TypeId::of::<sk_kernel::python_json::PythonJsonError>()
        );
        assert_eq!(
            TypeId::of::<super::PythonJsonRenderOptions>(),
            TypeId::of::<sk_kernel::python_json::PythonJsonRenderOptions>()
        );
        let parsed = super::parse(r#"{"a": 1}"#).expect("canonical reader alias");
        assert_eq!(
            sk_kernel::python_json::parse(r#"{"a": 1}"#).expect("kernel reader"),
            parsed
        );
        assert_eq!(
            super::python_repr_value(&parsed),
            sk_kernel::python_json::python_repr_value(&parsed)
        );
    }
}
