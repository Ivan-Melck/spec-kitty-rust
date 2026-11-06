//! One graph-read surface for authored profile documents (and their layers).
//!
//! The profile loader, the JSON-schema diagnostics and the Pydantic-equivalent
//! model all read the **canonical Kernel object graph** through this module.
//! There is no second value model here: [`Values`] is a borrowed stack of
//! [`NodeRef`]s, and every accessor delegates to the graph authority
//! (`NodeRef::as_str`, `Object`, `repr`, `is_falsy`, …). Nothing is projected to
//! a tree, so a Source-valid cycle in a field the model ignores is never turned
//! into a refusal, and a field's exact code points are read as they are.
//!
//! # Why a stack
//!
//! The reference deep-merges a higher layer's profile document into its built-in
//! *model dump* (`_merge_profiles`) before validating: nested mappings merge
//! recursively, every other value (a list included) is replaced. That merge is a
//! read-time view here, not a materialised document: [`Values::field`] returns
//! the highest layer's value, and a mapping present in several layers becomes a
//! merged stack, which recurses naturally through nested lookups. A value that is
//! a mapping in a higher layer replaces a scalar below it, exactly like
//! `deep_merge`.
//!
//! # Coercions
//!
//! `lax` interpretations (Pydantic's `str`/`bool`/`int`) belong to the model
//! layer; this module only reports the constructed Python kind.

use sk_kernel::PythonString;
use sk_kernel::python_yaml::{NodeRef, Object};

/// A mapping field, possibly merged from several profile layers.
///
/// Layer order is highest priority first: `layers[0]` is the overlay (or the only
/// document) and every later layer is a lower-priority base.
#[derive(Clone, Debug)]
pub(crate) struct Values<'g> {
    layers: Vec<NodeRef<'g>>,
}

impl<'g> Values<'g> {
    /// One node, no layering.
    pub(crate) fn single(node: NodeRef<'g>) -> Self {
        Self { layers: vec![node] }
    }

    /// A merged stack of mapping nodes (highest priority first).
    ///
    /// `None` when no layer holds a mapping — a caller that requires an object
    /// then reports its own diagnostic instead of inventing an empty one.
    pub(crate) fn merged(layers: Vec<NodeRef<'g>>) -> Option<Self> {
        let layers: Vec<NodeRef<'g>> = layers
            .into_iter()
            .filter(|layer| layer.as_mapping().is_some())
            .collect();
        if layers.is_empty() {
            return None;
        }
        Some(Self { layers })
    }

    /// The node this value reads from (the highest-priority layer).
    pub(crate) fn node(&self) -> NodeRef<'g> {
        self.layers[0]
    }

    /// The Python type name of the value.
    pub(crate) fn type_name(&self) -> &'static str {
        self.node().type_name()
    }

    /// Python `repr()` of the value, from the graph authority.
    pub(crate) fn repr(&self) -> String {
        self.node().repr()
    }

    /// Whether the value is `None`.
    pub(crate) fn is_null(&self) -> bool {
        self.node().is_null()
    }

    /// Python truthiness.
    pub(crate) fn is_falsy(&self) -> bool {
        self.node().is_falsy()
    }

    /// Whether the value is a Python mapping (`dict`/`ordereddict`).
    pub(crate) fn is_mapping(&self) -> bool {
        self.node().as_mapping().is_some()
    }

    /// The exact code points of a Python `str`.
    pub(crate) fn as_str(&self) -> Option<&'g PythonString> {
        self.node().as_str()
    }

    /// The bytes of a Python `bytes`.
    pub(crate) fn as_bytes(&self) -> Option<&'g [u8]> {
        self.node().as_bytes()
    }

    /// The value of a Python `bool`.
    pub(crate) fn as_bool(&self) -> Option<bool> {
        self.node().as_bool()
    }

    /// The canonical decimal text of a Python `int`.
    pub(crate) fn as_int_text(&self) -> Option<&'g str> {
        self.node().as_int_text()
    }

    /// The value of a Python `float`.
    pub(crate) fn as_float(&self) -> Option<f64> {
        self.node().as_float()
    }

    /// Whether the value is a constructed `date`/`datetime`.
    pub(crate) fn is_timestamp(&self) -> bool {
        matches!(self.node().object(), Object::Date(_) | Object::DateTime(_))
    }

    /// A field by exact key, with the layer merge applied.
    pub(crate) fn field(&self, key: &str) -> Option<Values<'g>> {
        for (index, layer) in self.layers.iter().enumerate() {
            let Some(value) = layer.lookup(key) else {
                continue;
            };
            if value.as_mapping().is_none() {
                return Some(Self::single(value));
            }
            // The reference recurses only when *both* sides are mappings; a
            // mapping in the higher layer replaces anything a lower layer holds,
            // so only lower mapping layers contribute.
            let mut layers = vec![value];
            for lower in &self.layers[index + 1..] {
                if let Some(candidate) = lower.lookup(key)
                    && candidate.as_mapping().is_some()
                {
                    layers.push(candidate);
                }
            }
            return Some(Self { layers });
        }
        None
    }

    /// Whether a field is present (in any layer).
    pub(crate) fn contains_key(&self, key: &str) -> bool {
        self.field(key).is_some()
    }

    /// The authored keys, in order, joined across layers without duplicates.
    pub(crate) fn keys(&self) -> Vec<PythonString> {
        let mut keys: Vec<PythonString> = Vec::new();
        for layer in &self.layers {
            for key in string_keys(*layer) {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        keys
    }

    /// Whether a mapping key is not a Python `str`.
    pub(crate) fn has_non_string_key(&self) -> bool {
        self.layers.iter().any(|layer| {
            layer.as_mapping().is_some_and(|entries| {
                entries
                    .iter()
                    .any(|(key, _)| layer.view(*key).as_str().is_none())
            })
        })
    }

    /// The members of a Python `list` (or a constructed `!!set`).
    ///
    /// Pydantic's lax `list[...]` accepts any iterable, and the reference's
    /// ruamel loader hands it a real `set` for `!!set`, so both are list-like
    /// here. `!!pairs` is a list of tuples in Source; the graph keeps its entries
    /// as links, so it is reported as not-list-like rather than fabricated into
    /// a second value form (a known wording gap for a document that is refused
    /// under both implementations).
    pub(crate) fn sequence(&self) -> Option<Vec<Values<'g>>> {
        let node = self.node();
        let items = match node.object() {
            Object::Seq(items) | Object::Set(items) => items.clone(),
            _ => return None,
        };
        Some(
            items
                .into_iter()
                .map(|id| Self::single(node.member(id)))
                .collect(),
        )
    }
}

/// The Python `str` keys of one mapping node, in authored order.
fn string_keys(node: NodeRef<'_>) -> Vec<PythonString> {
    node.as_mapping()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(key, _)| node.view(*key).as_str().cloned())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether an authored key is the ASCII spelling `ascii`.
///
/// A key carrying an unpaired surrogate has no scalar spelling and cannot equal
/// an ASCII field name; this is the same exact-code-point comparison the graph's
/// own `lookup` uses, not a lossy projection.
pub(crate) fn key_is(key: &PythonString, ascii: &str) -> bool {
    key.codepoints().len() == ascii.chars().count()
        && key
            .codepoints()
            .iter()
            .zip(ascii.chars())
            .all(|(unit, character)| *unit == u32::from(character))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sk_kernel::python_yaml::{Dialect, GraphDocument, decode_graph};

    fn document(text: &str) -> GraphDocument {
        GraphDocument::new(decode_graph(text, Dialect::RuamelSafe12).expect("decode"))
    }

    #[test]
    fn a_cycle_survives_and_stays_readable() {
        let document = document("profile-id: p\nx: &c [*c]\n");
        let root = Values::single(document.root());
        let cycle = root.field("x").expect("x");
        assert!(cycle.sequence().is_some(), "the cycle is a real list");
        assert!(document.has_cycle());
        // Reading an unrelated field never sees the cycle.
        assert_eq!(
            root.field("profile-id").and_then(|v| v.as_str().cloned()),
            Some(PythonString::from_scalars("p"))
        );
    }

    #[test]
    fn layering_merges_nested_mappings_and_replaces_other_values() {
        let overlay = document("m:\n  b: 2\nlist: [overlay]\n");
        let base = document("m:\n  a: 1\n  b: 9\nlist: [base]\nscalar: kept\n");
        let merged = Values::merged(vec![overlay.root(), base.root()]).expect("merged");
        let m = merged.field("m").expect("m");
        assert_eq!(
            m.field("a")
                .and_then(|v| v.as_int_text().map(str::to_owned)),
            Some("1".to_owned())
        );
        assert_eq!(
            m.field("b")
                .and_then(|v| v.as_int_text().map(str::to_owned)),
            Some("2".to_owned())
        );
        let list = merged.field("list").expect("list");
        let items = list.sequence().expect("list");
        assert_eq!(items.len(), 1, "a list is replaced, never unioned");
        assert!(merged.field("scalar").is_some(), "lower layers still show");
    }

    #[test]
    fn a_mapping_in_the_overlay_replaces_a_scalar_below_it() {
        let overlay = document("m: {a: 1}\n");
        let base = document("m: scalar\n");
        let merged = Values::merged(vec![overlay.root(), base.root()]).expect("merged");
        let m = merged.field("m").expect("m");
        assert_eq!(
            m.field("a")
                .and_then(|v| v.as_int_text().map(str::to_owned)),
            Some("1".to_owned())
        );
    }
}
