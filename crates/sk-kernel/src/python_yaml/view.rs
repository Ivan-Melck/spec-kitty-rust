//! Borrowed views over the ONE canonical YAML object graph.
//!
//! [`super::graph::ObjectGraph`] is the construction/identity/equality/hash/repr
//! authority. Typed consumers (Mission definitions, path conventions, Doctrine
//! raw documents) must *read that graph*, not a separately projected acyclic
//! tree: projecting the whole root before validation would falsely refuse a
//! Source-valid cycle that lives in a field the model ignores.
//!
//! [`NodeRef`] is the borrowed handle those consumers use. It adds **no** second
//! authority: every method delegates to the graph's own object data, identity,
//! `equal`, `hash`, `repr` and `str`. Field-scoped projection
//! ([`NodeRef::to_python_value`]) is available for the one case where a consumer
//! has already decided — with Source's own field policy — that a *subtree* must be
//! materialised; it never widens to the document root on its own.
//!
//! # One coercion authority
//!
//! `lax_str`, `lax_bool` and `falsy` are the single definitions of the
//! Python/pydantic scalar interpretations. Both [`NodeRef`] and
//! [`crate::python_value::PythonValue`] (the legacy within-build alias surface)
//! delegate here, so a consumer cannot drift by carrying a private copy.

use crate::PythonString;
use crate::python_value::PythonValue;

use super::Dialect;
use super::error::YamlError;
use super::graph::{NodeId, Object, ObjectGraph};

/// A decoded document held as the canonical object graph.
///
/// The graph is the authority; the root view is the entry point a typed consumer
/// reads. Cloning is cheap and shares the same arena (and therefore the same
/// object identities).
#[derive(Clone, Debug)]
pub struct GraphDocument {
    graph: ObjectGraph,
}

impl GraphDocument {
    /// Wrap one constructed graph as a document.
    pub fn new(graph: ObjectGraph) -> Self {
        Self { graph }
    }

    /// The graph authority behind this document.
    pub fn graph(&self) -> &ObjectGraph {
        &self.graph
    }

    /// The document root as a borrowed view.
    pub fn root(&self) -> NodeRef<'_> {
        self.view(self.graph.root())
    }

    /// One node by structural link.
    pub fn view(&self, id: NodeId) -> NodeRef<'_> {
        NodeRef {
            graph: &self.graph,
            id,
        }
    }

    /// Whether any object in the document is cyclic (structural, non-recursive).
    pub fn has_cycle(&self) -> bool {
        self.graph.has_cycle()
    }

    /// Consume the document, yielding the graph.
    pub fn into_graph(self) -> ObjectGraph {
        self.graph
    }

    /// The legacy whole-document acyclic projection.
    ///
    /// Only for the pre-migration tree callers and tests; a Source-compatible
    /// loader must read views instead (see the module docs).
    pub fn to_python_value(&self) -> Result<PythonValue, YamlError> {
        self.graph.to_python_value()
    }
}

/// A view over any graph, so a consumer that owns an [`ObjectGraph`] directly
/// (Doctrine's artifact documents) names the very same nodes.
impl ObjectGraph {
    /// One node by structural link.
    pub fn view(&self, id: NodeId) -> NodeRef<'_> {
        NodeRef { graph: self, id }
    }

    /// The root object as a borrowed view.
    pub fn root_view(&self) -> NodeRef<'_> {
        self.view(self.root())
    }
}

/// A borrowed handle to one object inside a [`GraphDocument`] or an
/// [`ObjectGraph`].
///
/// Structural only: it names a node of the graph, never a business identifier or
/// an escaped spelling. It cannot outlive the graph.
#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'g> {
    graph: &'g ObjectGraph,
    id: NodeId,
}

impl<'g> PartialEq for NodeRef<'g> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && std::ptr::eq(self.graph, other.graph)
    }
}

impl<'g> Eq for NodeRef<'g> {}

impl<'g> NodeRef<'g> {
    /// The graph authority.
    pub fn graph(self) -> &'g ObjectGraph {
        self.graph
    }

    /// The structural link (never an identity outside this graph).
    pub fn id(self) -> NodeId {
        self.id
    }

    /// The underlying object.
    pub fn object(self) -> &'g Object {
        self.graph.object(self.id)
    }

    /// The Python type name (`"dict"`, `"list"`, `"str"`, …).
    pub fn type_name(self) -> &'static str {
        self.object().python_type_name()
    }

    /// The Python type name, named as the value domain spells it.
    pub fn python_type_name(self) -> &'static str {
        self.type_name()
    }

    /// Whether the object is `None`.
    pub fn is_null(self) -> bool {
        matches!(self.object(), Object::Null)
    }

    /// The `bool` payload.
    pub fn as_bool(self) -> Option<bool> {
        match self.object() {
            Object::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    /// The canonical decimal text of an `int`.
    pub fn as_int_text(self) -> Option<&'g str> {
        match self.object() {
            Object::Int(digits) => Some(digits),
            _ => None,
        }
    }

    /// The IEEE-754 value of a `float`.
    pub fn as_float(self) -> Option<f64> {
        match self.object() {
            Object::Float { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// The exact code points of a `str`.
    pub fn as_str(self) -> Option<&'g PythonString> {
        match self.object() {
            Object::Str(text) => Some(text),
            _ => None,
        }
    }

    /// The bytes of a `bytes` object.
    pub fn as_bytes(self) -> Option<&'g [u8]> {
        match self.object() {
            Object::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The `str()` spelling of a `date`.
    pub fn as_date(self) -> Option<&'g str> {
        match self.object() {
            Object::Date(text) => Some(text),
            _ => None,
        }
    }

    /// The `str()` spelling of a `datetime`.
    pub fn as_datetime(self) -> Option<&'g str> {
        match self.object() {
            Object::DateTime(text) => Some(text),
            _ => None,
        }
    }

    /// The ordered members of a `list`.
    pub fn as_sequence(self) -> Option<&'g [NodeId]> {
        match self.object() {
            Object::Seq(items) => Some(items),
            _ => None,
        }
    }

    /// The ordered members of a `tuple`.
    pub fn as_tuple(self) -> Option<&'g [NodeId]> {
        match self.object() {
            Object::Tuple(items) => Some(items),
            _ => None,
        }
    }

    /// The members of a `set`, in CPython table order.
    pub fn as_set(self) -> Option<&'g [NodeId]> {
        match self.object() {
            Object::Set(items) => Some(items),
            _ => None,
        }
    }

    /// The entries of a `dict` (or ruamel `ordereddict`), authored order.
    pub fn as_mapping(self) -> Option<&'g [(NodeId, NodeId)]> {
        match self.object() {
            Object::Map(entries) | Object::OrderedDict(entries) => Some(entries),
            _ => None,
        }
    }

    /// The entries of a ruamel `ordereddict`.
    pub fn as_ordered_dict(self) -> Option<&'g [(NodeId, NodeId)]> {
        match self.object() {
            Object::OrderedDict(entries) => Some(entries),
            _ => None,
        }
    }

    /// The entries of an `!!pairs` list (each a real `(key, value)` tuple).
    pub fn as_pairs(self) -> Option<&'g [(NodeId, NodeId)]> {
        match self.object() {
            Object::Pairs(entries) => Some(entries),
            _ => None,
        }
    }

    /// How many members/entries the container holds (`None` for a scalar).
    pub fn member_count(self) -> Option<usize> {
        match self.object() {
            Object::Seq(items) | Object::Tuple(items) | Object::Set(items) => Some(items.len()),
            Object::Map(entries) | Object::OrderedDict(entries) | Object::Pairs(entries) => {
                Some(entries.len())
            }
            _ => None,
        }
    }

    /// One node by structural link.
    pub fn view(self, id: NodeId) -> NodeRef<'g> {
        NodeRef {
            graph: self.graph,
            id,
        }
    }

    /// Turn a member link into a view.
    pub fn member(self, id: NodeId) -> NodeRef<'g> {
        self.view(id)
    }

    /// The last entry for a string key (Python `dict` semantics).
    ///
    /// Authored duplicate keys have already collapsed to one entry with the last
    /// value and the first position, so "last entry" is the surviving value.
    pub fn lookup(self, key: &str) -> Option<NodeRef<'g>> {
        self.as_mapping()?
            .iter()
            .rev()
            .find(|(candidate, _)| match self.graph.object(*candidate) {
                Object::Str(text) => str_key_matches(text, key),
                _ => false,
            })
            .map(|(_, value)| self.view(*value))
    }

    /// Whether the mapping has an entry for `key`.
    pub fn contains_key(self, key: &str) -> bool {
        self.lookup(key).is_some()
    }

    /// The structural links this object holds, in authored order.
    pub fn links(self) -> Vec<NodeId> {
        self.object().links()
    }

    /// Python `is` — the same object.
    pub fn is_same_object(self, other: NodeRef<'g>) -> bool {
        self.graph.is_same_object(self.id, other.id)
    }

    /// Python `==` with the identity shortcut (see [`ObjectGraph::equal`]).
    pub fn equal(self, other: NodeRef<'g>) -> bool {
        self.graph.equal(self.id, other.id)
    }

    /// Python `hash()`.
    pub fn hash(self) -> Result<i64, YamlError> {
        self.graph.hash(self.id)
    }

    /// Python `repr()` with structural recursion markers.
    pub fn repr(self) -> String {
        self.graph.repr(self.id)
    }

    /// Python `str()`, as exact code points.
    pub fn python_str(self) -> PythonString {
        self.graph.python_str(self.id)
    }

    /// Project just this subtree into the acyclic legacy tree.
    ///
    /// Cycles inside the subtree stay a typed error; cycles elsewhere are
    /// irrelevant, which is the whole point of a field-scoped projection.
    pub fn to_python_value(self) -> Result<PythonValue, YamlError> {
        self.graph.to_python_value_at(self.id)
    }

    /// Python truthiness (`bool(value)`).
    pub fn is_falsy(self) -> bool {
        falsy(Truthiness::of(self))
    }

    /// pydantic's lax `str` (a `str`, or UTF-8 `bytes`).
    pub fn as_lax_str(self) -> Option<PythonString> {
        lax_str(self.as_str(), self.as_bytes())
    }

    /// pydantic's lax `bool`.
    pub fn as_bool_lax(self) -> Option<bool> {
        lax_bool(
            self.as_bool(),
            self.as_int_text(),
            self.as_float(),
            self.as_str()
                .map(PythonString::to_scalar_string)
                .transpose()
                .ok()
                .flatten(),
        )
    }
}

/// Compare an authored `PythonString` with an ASCII key by code points.
///
/// Not `to_scalar_string()`: a key that carries an unpaired surrogate is still a
/// legal Python `str` and must be found by exact code points.
fn str_key_matches(value: &PythonString, key: &str) -> bool {
    let units = value.codepoints();
    units.len() == key.chars().count()
        && units
            .iter()
            .zip(key.chars())
            .all(|(unit, character)| *unit == u32::from(character))
}

/// The scalar shape a truthiness decision needs, borrowed from either domain.
///
/// Both [`NodeRef`] and [`crate::python_value::PythonValue`] build one of these
/// and hand it to [`falsy`], so the rule exists exactly once.
pub(crate) enum Truthiness<'a> {
    Null,
    Bool(bool),
    Int(&'a str),
    Float(f64),
    Str(&'a PythonString),
    Bytes(&'a [u8]),
    Sized(usize),
    /// A `date`/`datetime` is always truthy.
    AlwaysTrue,
}

impl<'a> Truthiness<'a> {
    fn of(node: NodeRef<'a>) -> Self {
        match node.object() {
            Object::Null => Self::Null,
            Object::Bool(flag) => Self::Bool(*flag),
            Object::Int(digits) => Self::Int(digits),
            Object::Float { value, .. } => Self::Float(*value),
            Object::Str(text) => Self::Str(text),
            Object::Bytes(bytes) => Self::Bytes(bytes),
            Object::Date(_) | Object::DateTime(_) => Self::AlwaysTrue,
            Object::Seq(items) | Object::Tuple(items) | Object::Set(items) => {
                Self::Sized(items.len())
            }
            Object::Map(entries) | Object::OrderedDict(entries) | Object::Pairs(entries) => {
                Self::Sized(entries.len())
            }
        }
    }
}

/// Python truthiness of one scalar/container shape.
pub(crate) fn falsy(shape: Truthiness<'_>) -> bool {
    match shape {
        Truthiness::Null => true,
        Truthiness::Bool(flag) => !flag,
        Truthiness::Int(text) => {
            let digits = text.trim_start_matches(['+', '-']);
            digits.chars().all(|character| character == '0') || digits.is_empty()
        }
        Truthiness::Float(number) => number == 0.0,
        Truthiness::Str(text) => text.is_empty(),
        Truthiness::Bytes(bytes) => bytes.is_empty(),
        Truthiness::Sized(length) => length == 0,
        Truthiness::AlwaysTrue => false,
    }
}

/// The one pydantic lax `str` rule: a `str`, or strict-UTF-8 `bytes`.
pub(crate) fn lax_str(text: Option<&PythonString>, bytes: Option<&[u8]>) -> Option<PythonString> {
    if let Some(text) = text {
        return Some(text.clone());
    }
    std::str::from_utf8(bytes?)
        .ok()
        .map(PythonString::from_scalars)
}

/// The one pydantic lax `bool` rule.
pub(crate) fn lax_bool(
    flag: Option<bool>,
    int_text: Option<&str>,
    float: Option<f64>,
    scalar_text: Option<String>,
) -> Option<bool> {
    if let Some(flag) = flag {
        return Some(flag);
    }
    if let Some(text) = int_text {
        return match text {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        };
    }
    if let Some(value) = float {
        return if value == 0.0 {
            Some(false)
        } else if value == 1.0 {
            Some(true)
        } else {
            None
        };
    }
    let scalar = scalar_text?;
    match scalar.to_ascii_lowercase().as_str() {
        "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
        "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
        _ => None,
    }
}

/// The canonical builder entry for an adapter that already owns its own parse.
///
/// This is the *one* object-graph constructor: an explicitly-distinct dialect
/// adapter (Mission's YAML-1.2-core cross-check) hands its constructed tree here
/// rather than carrying a private object model. It is not a parser and not a
/// second validator — the caller keeps ownership of its own parse policy.
pub fn build_from_value(
    value: &serde_yaml_ng::Value,
    dialect: Dialect,
) -> Result<ObjectGraph, YamlError> {
    build_from_value_with_markers(value, &std::collections::BTreeMap::new(), dialect)
}

/// The canonical builder entry for an adapter that owns its own parse **and** its
/// transport markers.
///
/// A consumer whose lossless transport carries surrogate-bearing strings as
/// markers must pass them here, or the graph would see the marker spelling
/// instead of the exact code points. `decode_graph` uses the same builder with its
/// own markers; this is that path, not a second one.
pub fn build_from_value_with_markers(
    value: &serde_yaml_ng::Value,
    markers: &std::collections::BTreeMap<String, PythonString>,
    dialect: Dialect,
) -> Result<ObjectGraph, YamlError> {
    super::graph::build(value, markers, dialect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python_yaml::{Dialect, decode_graph};

    fn document(text: &str, dialect: Dialect) -> GraphDocument {
        GraphDocument::new(decode_graph(text, dialect).expect("decode"))
    }

    #[test]
    fn none_and_loader_nan_are_process_singletons_across_live_documents() {
        // Source's `None` and each loader's `.nan` are one object for the whole
        // process, not one per document. Hold two graphs at once and compare the
        // real pointer-derived hashes and storage addresses.
        let first = document("x: null\ny: .nan\n", Dialect::PyYamlSafe11);
        let second = document("p: null\nq: .nan\n", Dialect::RuamelSafe12);
        let first_null = first.root().lookup("x").expect("null");
        let second_null = second.root().lookup("p").expect("null");
        assert_eq!(
            first_null.hash().expect("hash(None)"),
            second_null.hash().expect("hash(None)"),
            "hash(None) must be one process value across live documents"
        );
        assert_eq!(
            first.graph().object_address(first_null.id()),
            second.graph().object_address(second_null.id()),
            "the modeled None storage is process-level, not per document"
        );

        let first_nan = first.root().lookup("y").expect("nan");
        let second_nan = second.root().lookup("q").expect("nan");
        assert_eq!(
            first_nan.hash().expect("hash(nan)"),
            second_nan.hash().expect("hash(nan)"),
            "the loader nan singleton is one process object"
        );
        assert_eq!(
            first.graph().object_address(first_nan.id()),
            second.graph().object_address(second_nan.id())
        );
        assert_ne!(
            first_nan.hash().expect("hash(nan)"),
            first_null.hash().expect("hash(None)")
        );
    }

    #[test]
    fn fresh_nan_objects_stay_distinct_hashable_keys() {
        let graph = document("{!!float nan: a, !!float nan: b}", Dialect::PyYamlSafe11);
        let entries = graph.root().as_mapping().expect("mapping");
        assert_eq!(entries.len(), 2, "two fresh NaN objects are two keys");
        let left = graph.view(entries[0].0);
        let right = graph.view(entries[1].0);
        assert!(!left.is_same_object(right));
        assert_ne!(
            left.hash().expect("hash"),
            right.hash().expect("hash"),
            "distinct NaN objects must not share a pointer hash"
        );
    }

    #[test]
    fn a_cycle_outside_the_projected_subtree_does_not_refuse_it() {
        // The point of a field-scoped projection: Source accepts a cycle in an
        // ignored field, so projecting a different field must not see it.
        let document = document("a: &x [*x]\nb: 1\n", Dialect::PyYamlSafe11);
        assert!(document.has_cycle());
        let flat = document.root().lookup("b").expect("b");
        assert_eq!(
            flat.to_python_value().expect("acyclic subtree"),
            PythonValue::Int("1".to_owned())
        );
        assert!(
            document
                .root()
                .lookup("a")
                .expect("a")
                .to_python_value()
                .is_err(),
            "the cyclic subtree itself is still a typed refusal"
        );
    }

    #[test]
    fn shared_coercions_match_the_legacy_domain() {
        let document = document(
            "flag: 'yes'\ncount: 1\nempty: ''\nbytes: !!binary aGk=\n",
            Dialect::PyYamlSafe11,
        );
        let root = document.root();
        assert_eq!(root.lookup("flag").unwrap().as_bool_lax(), Some(true));
        assert_eq!(root.lookup("count").unwrap().as_bool_lax(), Some(true));
        assert!(root.lookup("empty").unwrap().is_falsy());
        assert_eq!(
            root.lookup("bytes").unwrap().as_lax_str(),
            Some(PythonString::from_scalars("hi"))
        );
        // The legacy domain delegates to the same rule.
        let legacy = PythonValue::Bytes(b"hi".to_vec());
        assert_eq!(legacy.as_lax_str(), Some(PythonString::from_scalars("hi")));
        assert!(PythonValue::Null.is_falsy());
        assert!(!PythonValue::Int("1".to_owned()).is_falsy());
    }

    #[test]
    fn text_authority_prints_python_forms() {
        let document = document(
            "s: !!set {}\nb: !!binary SGVsbG8=\no: !!omap [{a: 1}]\n",
            Dialect::RuamelSafe12,
        );
        let root = document.root();
        assert_eq!(
            root.lookup("s")
                .unwrap()
                .python_str()
                .to_scalar_string()
                .unwrap(),
            "set()"
        );
        assert_eq!(
            root.lookup("b")
                .unwrap()
                .python_str()
                .to_scalar_string()
                .unwrap(),
            "b'Hello'"
        );
        assert_eq!(
            root.lookup("o")
                .unwrap()
                .python_str()
                .to_scalar_string()
                .unwrap(),
            "ordereddict([('a', 1)])"
        );
    }
}
