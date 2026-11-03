//! Read-only YAML semantics for language inputs; never rewrites source bytes.

use serde_yaml_ng::Value;

/// Strip only the leading UTF-8 BOM before the semantic parser sees it.
/// serde_yaml_ng otherwise misreads some BOM-prefixed multi-key documents.
pub(crate) fn parse(text: &str) -> Option<Value> {
    serde_yaml_ng::from_str(text.strip_prefix('\u{feff}').unwrap_or(text)).ok()
}

/// The reference's safe-loader reads skip the entire document on an unknown
/// tag, not just the tagged item. Compiled charter reads use `parse` instead:
/// their reference accessor has a different round-trip-loader policy.
pub(crate) fn parse_safe(text: &str) -> Option<Value> {
    let value = parse(text)?;
    (!contains_tag(&value)).then_some(value)
}

fn contains_tag(value: &Value) -> bool {
    match value {
        Value::Tagged(_) => true,
        Value::Sequence(items) => items.iter().any(contains_tag),
        Value::Mapping(items) => items
            .iter()
            .any(|(key, value)| contains_tag(key) || contains_tag(value)),
        _ => false,
    }
}
