//! ONE canonical Python object graph for the Source-compatible YAML loader.
//!
//! The reference loaders produce Python **objects with identity**: anchors share
//! one object, aliases refer to it, `a: &x [*x]` is a genuinely cyclic list, and
//! a `dict` keeps two `NaN` keys when the two floats are distinct objects. A
//! tree of owned values cannot express any of that, so this module owns the
//! construction result: [`ObjectGraph`] is a frozen arena of boxed objects with
//! typed [`NodeId`] links.
//!
//! Binding invariants (see `docs/rust/reviews/canonical-yaml-graph-decision.md`):
//!
//! * **Links are structural.** A [`NodeId`] names a node inside one graph. It is
//!   never a business identifier, an escaped spelling or a pseudo-value.
//! * **Sharing and cycles survive.** Aliases and cycle edges are node links; no
//!   recursive owned tree, alias expansion, scalar drop or formatted-string
//!   substitute is produced. Clone handles ([`ObjectGraph::clone`]) share the
//!   same arena, so they keep the same object identities.
//! * **Drop is flat.** A node holds [`NodeId`]s, never nested boxed objects, so
//!   dropping a graph never recurses through the document.
//! * **Objects have real addresses.** `None` and `float('nan')` hash from
//!   CPython's `_Py_HashPointer` over the *actual* boxed object address
//!   ([`ObjectGraph::hash`]), with the genuine rotate-right-by-four and the
//!   `-1 -> -2` remap. Two distinct `NaN` objects do not share a fabricated
//!   sentinel, and the loader's `.nan` singleton is one node, so two `.nan` keys
//!   are one object while two explicit `!!float nan` keys are two. `None` and the
//!   loader's `.nan` hash from **process-level** boxed singletons, so their hash is
//!   identical across every live document — Source's `None`/`nan_value` are
//!   per-process objects too, not per-document ones.
//! * **One text authority.** `repr()` of containers is derived from this
//!   structure, including Python's recursion markers (`[1, [...]]`, `{'a': {...}}`).
//!
//! The graph is built from the *already constructed* semantic document: the
//! dialect's implicit-scalar resolver, tags, duplicate policy, merge keys and
//! every scalar constructor stay the one existing authority; this module adds the
//! object/identity layer and the anchor/alias links the semantic tree cannot
//! carry. `python_yaml::transport::splice_identity_tags` marks the anchor and
//! alias sites in the marked text, so the semantic reader never has to expand an
//! alias or refuse a cycle.
//!
//! Not yet migrated: the model-facing consumers still project through
//! [`ObjectGraph::to_python_value`] (acyclic documents only) via
//! `python_yaml::decode_text`. A cyclic document is accepted by
//! [`super::decode_graph`] and still refused by the tree projection — a named
//! open case, not a parity claim.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::PythonString;
use crate::python_int::{IntDecimalError, UNLIMITED_INTEGER_DIGITS, format_decimal};
use crate::python_value::PythonValue;
use serde_yaml_ng::Value;
use serde_yaml_ng::value::Tag;

use super::Dialect;
use super::construct;
use super::error::{RuntimeCause, YamlError};
use super::set_order;

/// Tag prefix marking an anchored node: `!sk-canonical-anchor-<ordinal>`.
pub const ANCHOR_TAG_PREFIX: &str = "!sk-canonical-anchor-";
/// Tag prefix marking an anchor on an empty (null) node.
pub const ANCHOR_NULL_TAG_PREFIX: &str = "!sk-canonical-anchor-null-";
/// Tag prefix marking an alias site: `!sk-canonical-alias-<ordinal>`.
pub const ALIAS_TAG_PREFIX: &str = "!sk-canonical-alias-";
/// Tag of a `!!float nan` scalar, which constructs a **fresh** float object.
///
/// PyYAML's `construct_yaml_float` returns `self.nan_value` for the `.nan`
/// spelling and `math.nan` (a new object) for a bare `nan` payload; ruamel
/// behaves the same way (probed: `{!!float nan: a, !!float nan: b}` is two keys
/// in both dialects, `{!!float .nan: a, !!float .nan: b}` is one). The semantic
/// constructor rejects the tag; this layer is the one that owns the identity.
pub const FRESH_NAN_TAG_PREFIX: &str = "!sk-canonical-fresh-nan-";

/// Whether a tag is one this module's transport splice produced.
pub fn is_graph_tag(tag: &Tag) -> bool {
    let text = tag.to_string();
    text.starts_with(ANCHOR_TAG_PREFIX)
        || text.starts_with(ANCHOR_NULL_TAG_PREFIX)
        || text.starts_with(ALIAS_TAG_PREFIX)
        || text.starts_with(FRESH_NAN_TAG_PREFIX)
}

/// Whether a tag marks a fresh `!!float nan` object.
///
/// Each occurrence carries its own ordinal: two fresh NaN objects are distinct
/// keys, and the semantic reader must not fold two identical spellings into one
/// mapping entry before the graph sees them.
pub fn is_fresh_nan_tag(tag: &Tag) -> bool {
    tag.to_string().starts_with(FRESH_NAN_TAG_PREFIX)
}

/// The graph tag a semantic node carries, when it carries one.
pub fn graph_tag_name(value: &Value) -> Option<String> {
    let Value::Tagged(tagged) = value else {
        return None;
    };
    is_graph_tag(&tagged.tag).then(|| tagged.tag.to_string())
}

/// A typed link to one object inside one [`ObjectGraph`].
///
/// Structural only: an index into the graph's object storage.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(u32);

impl NodeId {
    /// The storage index (traversal-local; never an identity outside the graph).
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Why a `NaN` object exists in the graph.
///
/// Source's loaders own one `nan_value` singleton per loader; an explicit
/// `!!float nan` constructs a fresh `math.nan` instead. Identity, not value,
/// decides a `dict` key match, so the provenance is part of the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FloatProvenance {
    /// The loader's `.nan` singleton (also `!!float .nan`).
    LoaderSingleton,
    /// A distinct float object constructed by `!!float nan`.
    Fresh,
}

/// One Python object in the arena.
#[derive(Debug)]
pub enum Object {
    /// `None`
    Null,
    /// `bool`
    Bool(bool),
    /// `int` — canonical decimal text, so arbitrary precision survives.
    Int(String),
    /// `float` with its construction provenance.
    Float {
        /// The IEEE-754 value.
        value: f64,
        /// Which Python object this is (`nan` identity depends on it).
        provenance: FloatProvenance,
    },
    /// `str` — exact code points, including unpaired surrogates.
    Str(PythonString),
    /// `bytes`
    Bytes(Vec<u8>),
    /// `datetime.date`, spelled as CPython's `str()`.
    Date(String),
    /// `datetime.datetime`, spelled as CPython's `str()`.
    DateTime(String),
    /// `list` — ordered links (may contain cycles).
    Seq(Vec<NodeId>),
    /// `tuple` — ordered links.
    Tuple(Vec<NodeId>),
    /// `set` — members in CPython hash-table iteration order.
    Set(Vec<NodeId>),
    /// `dict` — authored insertion order, first key position kept.
    Map(Vec<(NodeId, NodeId)>),
    /// `list` of `(key, value)` tuples (`!!pairs`, PyYAML's `!!omap`).
    Pairs(Vec<(NodeId, NodeId)>),
    /// `ordereddict` (ruamel's `!!omap`).
    OrderedDict(Vec<(NodeId, NodeId)>),
}

impl Object {
    /// The Python type name validation diagnostics use.
    pub fn python_type_name(&self) -> &'static str {
        match self {
            Self::Null => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float { .. } => "float",
            Self::Str(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::Date(_) => "date",
            Self::DateTime(_) => "datetime",
            Self::Seq(_) => "list",
            Self::Tuple(_) => "tuple",
            Self::Set(_) => "set",
            Self::Map(_) | Self::OrderedDict(_) => "dict",
            Self::Pairs(_) => "list",
        }
    }

    /// Whether Python would call this object hashable (`hash()` may still raise
    /// for a tuple holding an unhashable member — that failure belongs to
    /// [`ObjectGraph::hash`]).
    pub fn is_hashable_kind(&self) -> bool {
        !matches!(
            self,
            Self::Seq(_) | Self::Map(_) | Self::Set(_) | Self::Pairs(_) | Self::OrderedDict(_)
        )
    }

    /// The structural links this object holds, in authored order.
    pub fn links(&self) -> Vec<NodeId> {
        match self {
            Self::Seq(items) | Self::Tuple(items) | Self::Set(items) => items.clone(),
            Self::Map(entries) | Self::Pairs(entries) | Self::OrderedDict(entries) => entries
                .iter()
                .flat_map(|(key, value)| [*key, *value])
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// A frozen Python object graph: one document's constructed objects.
///
/// The arena is immutable after construction, so a node's address is stable for
/// the lifetime of the graph and can back `_Py_HashPointer`.
#[derive(Clone)]
pub struct ObjectGraph {
    // The box is load-bearing: `None` and `nan` hash from their object's real
    // address, so each object needs its own stable allocation that survives the
    // vector's growth. A `Vec<Object>` would move them on reallocation.
    #[allow(clippy::vec_box)]
    nodes: Arc<Vec<Box<Object>>>,
    root: NodeId,
}

impl std::fmt::Debug for ObjectGraph {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObjectGraph")
            .field("nodes", &self.nodes.len())
            .field("root", &self.root)
            .finish()
    }
}

impl ObjectGraph {
    /// The document's root object.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// How many objects the graph holds (cycle/anchor-expansion free count).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph holds no object (never true; a document always has a
    /// root).
    pub fn is_empty(&self) -> bool {
        false
    }

    /// One object by link.
    pub fn object(&self, id: NodeId) -> &Object {
        &self.nodes[id.index()]
    }

    /// The Python type name of one object.
    pub fn type_name(&self, id: NodeId) -> &'static str {
        self.object(id).python_type_name()
    }

    /// Whether two links name the **same** object (Python `is`).
    pub fn is_same_object(&self, left: NodeId, right: NodeId) -> bool {
        left == right
    }

    /// The address of one object's storage, for pointer-hash derivation.
    ///
    /// `None` and the loader's `.nan` come from **process-level** modeled
    /// singletons (a real boxed [`Object`] allocated once, exactly like CPython's
    /// `None` and PyYAML/ruamel's `nan_value`), so their pointer hash is the same
    /// across every live document in this process. A fresh `!!float nan` and every
    /// other object keep their own node address.
    pub fn object_address(&self, id: NodeId) -> usize {
        match self.object(id) {
            Object::Null => std::ptr::from_ref(null_singleton()) as usize,
            Object::Float {
                value,
                provenance: FloatProvenance::LoaderSingleton,
            } if value.is_nan() => std::ptr::from_ref(loader_nan_singleton()) as usize,
            reference => std::ptr::from_ref(reference) as usize,
        }
    }

    /// Python `==` for two objects, with Python's identity shortcut first.
    ///
    /// A scalar `NaN` is never equal to a different `NaN` object; the *same*
    /// object compares equal through the identity shortcut, which is exactly why
    /// two `.nan` keys collapse and two `!!float nan` keys do not.
    pub fn equal(&self, left: NodeId, right: NodeId) -> bool {
        let mut seen: BTreeSet<(NodeId, NodeId)> = BTreeSet::new();
        let mut pending = vec![(left, right)];
        while let Some((left, right)) = pending.pop() {
            if left == right {
                continue;
            }
            if !seen.insert((left, right)) {
                // Already proven equal on this walk (a cycle we are re-entering).
                continue;
            }
            let (a, b) = (self.object(left), self.object(right));
            match (a, b) {
                (Object::Null, Object::Null) => {}
                (Object::Bool(a), Object::Bool(b)) if a == b => {}
                (Object::Int(a), Object::Int(b)) if a == b => {}
                (Object::Float { value: a, .. }, Object::Float { value: b, .. }) => {
                    // `nan != nan` unless it is the same object, which the
                    // identity shortcut above already handled.
                    if !(a == b) {
                        return false;
                    }
                }
                (Object::Str(a), Object::Str(b)) if a == b => {}
                (Object::Bytes(a), Object::Bytes(b)) if a == b => {}
                (Object::Date(a), Object::Date(b)) if a == b => {}
                (Object::DateTime(a), Object::DateTime(b)) if a == b => {}
                (Object::Seq(a), Object::Seq(b)) | (Object::Tuple(a), Object::Tuple(b))
                    if a.len() == b.len() =>
                {
                    pending.extend(a.iter().copied().zip(b.iter().copied()));
                }
                (Object::Set(a), Object::Set(b)) if a.len() == b.len() => {
                    if !self.sets_are_equal(a, b) {
                        return false;
                    }
                }
                (Object::Map(a), Object::Map(b))
                | (Object::OrderedDict(a), Object::OrderedDict(b))
                    if a.len() == b.len() =>
                {
                    if !self.entries_are_equal(a, b) {
                        return false;
                    }
                }
                (Object::Pairs(a), Object::Pairs(b)) if a.len() == b.len() => {
                    for ((key_a, value_a), (key_b, value_b)) in a.iter().zip(b.iter()) {
                        pending.push((*key_a, *key_b));
                        pending.push((*value_a, *value_b));
                    }
                }
                _ => return false,
            }
        }
        true
    }

    fn sets_are_equal(&self, left: &[NodeId], right: &[NodeId]) -> bool {
        left.iter()
            .all(|item| right.iter().any(|other| self.equal(*item, *other)))
    }

    fn entries_are_equal(&self, left: &[(NodeId, NodeId)], right: &[(NodeId, NodeId)]) -> bool {
        left.iter().all(|(key, value)| {
            right.iter().any(|(other_key, other)| {
                self.equal(*key, *other_key) && self.equal(*value, *other)
            })
        })
    }

    /// Python `hash()` of one object, in this process's own terms.
    ///
    /// `None` and a `NaN` float hash from their **real** object address through
    /// CPython's `_Py_HashPointer`; the other scalar kinds reuse the existing
    /// canonical hashing authority (`set_order`). An unhashable object (a list,
    /// dict, set, or a tuple holding one) raises the bare `TypeError` the
    /// reference raises in the same phase.
    pub fn hash(&self, id: NodeId) -> Result<i64, YamlError> {
        let secret = set_order::hash_secret()?;
        self.hash_with_secret(id, secret)
    }

    fn hash_with_secret(&self, id: NodeId, secret: (u64, u64)) -> Result<i64, YamlError> {
        let object = self.object(id);
        let hash = match object {
            Object::Null => set_order::pointer_hash_of(self.object_address(id)),
            Object::Bool(flag) => i64::from(*flag),
            Object::Int(digits) => set_order::decimal_hash(digits),
            Object::Float { value, .. } => {
                if value.is_nan() {
                    // `hash(float('nan'))` is the pointer hash of *that* object;
                    // distinct NaN objects therefore land in different slots.
                    set_order::pointer_hash_of(self.object_address(id))
                } else {
                    set_order::float_hash(*value)
                }
            }
            Object::Str(text) => set_order::string_hash_with_secret(text.codepoints(), secret)?,
            Object::Bytes(bytes) => set_order::bytes_hash_with_secret(bytes, secret)?,
            Object::Date(_) | Object::DateTime(_) => {
                let text = match object {
                    Object::Date(text) | Object::DateTime(text) => text,
                    _ => unreachable!("the match arm proved a timestamp"),
                };
                set_order::timestamp_hash(text, secret)?
            }
            Object::Tuple(items) => {
                let mut hashes = Vec::with_capacity(items.len());
                for item in items {
                    hashes.push(self.hash_with_secret(*item, secret)?);
                }
                set_order::xx_hash(&hashes)
            }
            other => return Err(unhashable(other.python_type_name())),
        };
        Ok(if hash == -1 { -2 } else { hash })
    }

    /// Python `repr()` of one object, with Python's recursion markers.
    ///
    /// **Inspection facade, not the Source business path.** It renders under the
    /// explicit unlimited integer policy
    /// ([`UNLIMITED_INTEGER_DIGITS`](crate::python_int::UNLIMITED_INTEGER_DIGITS))
    /// through the SAME renderer as [`Self::try_repr`], so this convenience cannot
    /// drift from it; it does not claim the reference's decimal ceiling. A caller
    /// observing Source's eager `str()`/`repr()` phase must call [`Self::try_repr`]
    /// (or [`Self::try_python_str`]) with the interpreter's configured limit.
    pub fn repr(&self, id: NodeId) -> String {
        self.try_repr(id, UNLIMITED_INTEGER_DIGITS)
            .expect("the unlimited integer policy cannot refuse a render")
    }

    /// Python `repr()` of one object under the interpreter's integer ceiling.
    ///
    /// The traversal, alias/cycle short-circuit and authored ordering are the same
    /// as [`Self::repr`]; the only fallible step is decimal integer formatting, which
    /// propagates from nested values and mapping keys in demand order.
    ///
    /// # Errors
    ///
    /// [`IntDecimalError::DigitLimit`] when an `int` in the demanded subtree would
    /// exceed the configured decimal conversion ceiling.
    pub fn try_repr(&self, id: NodeId, digit_limit: usize) -> Result<String, IntDecimalError> {
        let mut rendered = String::new();
        self.write_repr(id, digit_limit, &mut Vec::new(), &mut rendered)?;
        Ok(rendered)
    }

    /// The one `repr()` traversal, shared by both entry points.
    fn write_repr(
        &self,
        id: NodeId,
        digit_limit: usize,
        path: &mut Vec<NodeId>,
        out: &mut String,
    ) -> Result<(), IntDecimalError> {
        match self.object(id) {
            Object::Null => out.push_str("None"),
            Object::Bool(true) => out.push_str("True"),
            Object::Bool(false) => out.push_str("False"),
            Object::Int(digits) => out.push_str(&format_decimal(digits, digit_limit)?),
            Object::Float { value, .. } => {
                out.push_str(&crate::python_json::python_float_repr(*value))
            }
            Object::Str(text) => out.push_str(&text.repr()),
            Object::Bytes(bytes) => out.push_str(&bytes_repr(bytes)),
            Object::Date(text) | Object::DateTime(text) => out.push_str(&timestamp_repr(text)),
            Object::Seq(items) => {
                if path.contains(&id) {
                    out.push_str("[...]");
                    return Ok(());
                }
                path.push(id);
                out.push('[');
                for (position, item) in items.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    self.write_repr(*item, digit_limit, path, out)?;
                }
                out.push(']');
                path.pop();
            }
            Object::Tuple(items) => {
                if path.contains(&id) {
                    out.push_str("(...)");
                    return Ok(());
                }
                path.push(id);
                out.push('(');
                for (position, item) in items.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    self.write_repr(*item, digit_limit, path, out)?;
                }
                if items.len() == 1 {
                    out.push(',');
                }
                out.push(')');
                path.pop();
            }
            Object::Map(entries) => {
                if path.contains(&id) {
                    out.push_str("{...}");
                    return Ok(());
                }
                path.push(id);
                out.push('{');
                for (position, (key, value)) in entries.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    self.write_repr(*key, digit_limit, path, out)?;
                    out.push_str(": ");
                    self.write_repr(*value, digit_limit, path, out)?;
                }
                out.push('}');
                path.pop();
            }
            Object::OrderedDict(entries) => {
                // Source (ruamel safe `!!omap`) prints the `ordereddict` call
                // form, not a dict literal.
                if path.contains(&id) {
                    out.push_str("ordereddict([...])");
                    return Ok(());
                }
                path.push(id);
                out.push_str("ordereddict([");
                for (position, (key, value)) in entries.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    out.push('(');
                    self.write_repr(*key, digit_limit, path, out)?;
                    out.push_str(", ");
                    self.write_repr(*value, digit_limit, path, out)?;
                    out.push(')');
                }
                out.push_str("])");
                path.pop();
            }
            Object::Set(items) => {
                // CPython prints an empty set as the call form, not `{set()}`.
                if items.is_empty() {
                    out.push_str("set()");
                    return Ok(());
                }
                if path.contains(&id) {
                    out.push_str("{...}");
                    return Ok(());
                }
                path.push(id);
                out.push('{');
                for (position, item) in items.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    self.write_repr(*item, digit_limit, path, out)?;
                }
                out.push('}');
                path.pop();
            }
            Object::Pairs(entries) => {
                if path.contains(&id) {
                    out.push_str("[...]");
                    return Ok(());
                }
                path.push(id);
                out.push('[');
                for (position, (key, value)) in entries.iter().enumerate() {
                    if position > 0 {
                        out.push_str(", ");
                    }
                    // Python's `repr` of a `!!pairs` entry is its real tuple.
                    out.push('(');
                    self.write_repr(*key, digit_limit, path, out)?;
                    out.push_str(", ");
                    self.write_repr(*value, digit_limit, path, out)?;
                    out.push(')');
                }
                out.push(']');
                path.pop();
            }
        }
        Ok(())
    }

    /// Project the whole graph into the acyclic [`PythonValue`] tree.
    ///
    /// A **legacy** whole-document projection: [`super::decode_text`] uses it for
    /// the pre-migration tree callers and tests. Source-compatible consumers must
    /// **not** call it on the root before validating — a Source-valid cycle in a
    /// field the model ignores would be falsely refused there. Use
    /// [`Self::to_python_value_at`] for a field-scoped projection instead.
    ///
    /// Shared objects are copied and a **cycle is a typed error**
    /// ([`YamlError::Construct`]) rather than a silent truncation.
    pub fn to_python_value(&self) -> Result<PythonValue, YamlError> {
        self.to_python_value_at(self.root)
    }

    /// Project the subtree rooted at `id` into the acyclic [`PythonValue`] tree.
    ///
    /// This is the field-scoped projection a typed consumer uses after it has
    /// decided — with Source's own field policy — that this one subtree must be a
    /// materialised value. A cycle inside the subtree is still the same typed
    /// error; a cycle elsewhere in the document is irrelevant.
    pub fn to_python_value_at(&self, id: NodeId) -> Result<PythonValue, YamlError> {
        let mut path: Vec<NodeId> = Vec::new();
        self.project(id, &mut path)
    }

    /// Python `str()` of one object, as exact code points.
    ///
    /// A `str` is itself (no quotes, unpaired surrogates preserved); a
    /// `date`/`datetime` renders its ISO `str()` spelling; every other kind uses
    /// [`Self::repr`] (containers' `str` **is** their `repr` in CPython, so a
    /// cycle prints its structural `[...]`/`{...}` marker). This is the ONE
    /// `str()` authority consumers share; none of them carries a private walker.
    ///
    /// Like [`Self::repr`], this is the explicit **unlimited** inspection facade.
    /// The Source eager `str()` phase must call [`Self::try_python_str`] with the
    /// interpreter's configured ceiling.
    pub fn python_str(&self, id: NodeId) -> PythonString {
        self.try_python_str(id, UNLIMITED_INTEGER_DIGITS)
            .expect("the unlimited integer policy cannot refuse a render")
    }

    /// Python `str()` of one object under the interpreter's integer ceiling.
    ///
    /// `str` and `date`/`datetime` spellings never consult the ceiling; every other
    /// kind renders through the one [`Self::try_repr`] traversal, so a nested `int`
    /// propagates [`IntDecimalError`] from its demand position.
    ///
    /// # Errors
    ///
    /// [`IntDecimalError::DigitLimit`] when a demanded `int` would exceed the
    /// configured decimal conversion ceiling.
    pub fn try_python_str(
        &self,
        id: NodeId,
        digit_limit: usize,
    ) -> Result<PythonString, IntDecimalError> {
        match self.object(id) {
            Object::Str(text) => Ok(text.clone()),
            Object::Date(text) | Object::DateTime(text) => Ok(PythonString::from_scalars(text)),
            _ => Ok(PythonString::from_scalars(&self.try_repr(id, digit_limit)?)),
        }
    }

    fn project(&self, id: NodeId, path: &mut Vec<NodeId>) -> Result<PythonValue, YamlError> {
        match self.object(id) {
            Object::Null => Ok(PythonValue::Null),
            Object::Bool(flag) => Ok(PythonValue::Bool(*flag)),
            Object::Int(digits) => Ok(PythonValue::Int(digits.clone())),
            Object::Float { value, .. } => Ok(PythonValue::Float(*value)),
            Object::Str(text) => Ok(PythonValue::Str(text.clone())),
            Object::Bytes(bytes) => Ok(PythonValue::Bytes(bytes.clone())),
            Object::Date(text) => Ok(PythonValue::Date(text.clone())),
            Object::DateTime(text) => Ok(PythonValue::DateTime(text.clone())),
            Object::Seq(_)
            | Object::Tuple(_)
            | Object::Set(_)
            | Object::Map(_)
            | Object::Pairs(_)
            | Object::OrderedDict(_) => {
                if path.contains(&id) {
                    return Err(cycle_projection_error());
                }
                path.push(id);
                let projected = match self.object(id) {
                    Object::Seq(items) => PythonValue::Seq(self.project_all(items, path)?),
                    Object::Tuple(items) => PythonValue::Tuple(self.project_all(items, path)?),
                    Object::Set(items) => PythonValue::Set(self.project_all(items, path)?),
                    Object::Map(entries) => PythonValue::Map(self.project_entries(entries, path)?),
                    Object::OrderedDict(entries) => {
                        PythonValue::OrderedDict(self.project_entries(entries, path)?)
                    }
                    Object::Pairs(entries) => {
                        PythonValue::Pairs(self.project_entries(entries, path)?)
                    }
                    _ => unreachable!("the match arm proved a container"),
                };
                path.pop();
                Ok(projected)
            }
        }
    }

    fn project_all(
        &self,
        items: &[NodeId],
        path: &mut Vec<NodeId>,
    ) -> Result<Vec<PythonValue>, YamlError> {
        items.iter().map(|item| self.project(*item, path)).collect()
    }

    fn project_entries(
        &self,
        entries: &[(NodeId, NodeId)],
        path: &mut Vec<NodeId>,
    ) -> Result<Vec<(PythonValue, PythonValue)>, YamlError> {
        entries
            .iter()
            .map(|(key, value)| Ok((self.project(*key, path)?, self.project(*value, path)?)))
            .collect()
    }

    /// Whether any object in the graph is part of a cycle (reachable from
    /// itself). Structural, non-recursive (Kahn-style peeling).
    pub fn has_cycle(&self) -> bool {
        let mut settled = vec![false; self.nodes.len()];
        // A node is "safe" when every path from it terminates. Peel nodes with
        // no outgoing link to an unsettled node until nothing changes.
        loop {
            let mut changed = false;
            for (index, object) in self.nodes.iter().enumerate() {
                if settled[index] {
                    continue;
                }
                if object
                    .links()
                    .iter()
                    .all(|link| settled[link.index()] && link.index() != index)
                {
                    settled[index] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        settled.iter().any(|value| !value)
    }
}

/// CPython's `None` is one object for the whole process; so is this.
///
/// A real boxed object (not a fabricated constant or a node index) so the pointer
/// hash derives from genuine storage and stays identical across every live
/// document, which is what Source's per-process `None` singleton does.
fn null_singleton() -> &'static Object {
    static NULL: std::sync::OnceLock<Box<Object>> = std::sync::OnceLock::new();
    NULL.get_or_init(|| Box::new(Object::Null))
}

/// Each Source loader owns one `.nan` value for the whole process; so does this.
///
/// A fresh `!!float nan` is a different object every time (see
/// [`FloatProvenance::Fresh`]), so it does **not** use this storage.
fn loader_nan_singleton() -> &'static Object {
    static NAN: std::sync::OnceLock<Box<Object>> = std::sync::OnceLock::new();
    NAN.get_or_init(|| {
        Box::new(Object::Float {
            value: f64::NAN,
            provenance: FloatProvenance::LoaderSingleton,
        })
    })
}

fn unhashable(type_name: &'static str) -> YamlError {
    YamlError::Runtime {
        cause: RuntimeCause::Type,
        message: format!("unhashable type: '{type_name}'"),
    }
}

fn cycle_projection_error() -> YamlError {
    YamlError::Construct(
        "a cyclic alias cannot be projected into an acyclic value tree by this consumer".to_owned(),
    )
}

/// Python `repr()` of a `date`/`datetime` object.
///
/// CPython's `repr` is the constructor form (`datetime.date(2026, 11, 5)`), while
/// `str()` is the ISO spelling the value carries. Trailing zero components are
/// omitted; `hour` and `minute` are always present on a datetime.
fn timestamp_repr(text: &str) -> String {
    let numbers = |spelling: &str| -> Vec<u32> {
        spelling
            .split('-')
            .filter_map(|part| part.parse::<u32>().ok())
            .collect()
    };
    let Some((day, rest)) = text.split_once(' ') else {
        return format!(
            "datetime.date({})",
            numbers(text)
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    let date = numbers(day);
    let (clock, offset) = match rest.find(['+', '-']) {
        Some(index) => (&rest[..index], Some(&rest[index..])),
        None => (rest, None),
    };
    let (clock, micro) = match clock.split_once('.') {
        Some((time, micro)) => (time, micro.parse::<u32>().unwrap_or(0)),
        None => (clock, 0),
    };
    let time: Vec<u32> = clock
        .split(':')
        .filter_map(|part| part.parse::<u32>().ok())
        .collect();
    let hour = time.first().copied().unwrap_or(0);
    let minute = time.get(1).copied().unwrap_or(0);
    let second = time.get(2).copied().unwrap_or(0);
    let mut arguments: Vec<String> = date.iter().map(|value| value.to_string()).collect();
    arguments.push(hour.to_string());
    arguments.push(minute.to_string());
    if second != 0 || micro != 0 {
        arguments.push(second.to_string());
    }
    if micro != 0 {
        arguments.push(micro.to_string());
    }
    if let Some(offset) = offset {
        let seconds = parse_offset_seconds(offset);
        arguments.push(if seconds == 0 {
            "tzinfo=datetime.timezone.utc".to_owned()
        } else {
            format!("tzinfo=datetime.timezone(datetime.timedelta(seconds={seconds}))")
        });
    }
    format!("datetime.datetime({})", arguments.join(", "))
}

fn parse_offset_seconds(offset: &str) -> i64 {
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let body = offset.trim_start_matches(['+', '-']);
    let mut parts = body.split(':');
    let hours: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let minutes: i64 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    sign * (hours * 3600 + minutes * 60)
}

/// Python `bytes` `repr()`.
///
/// CPython prefers single quotes, switches to double quotes when the payload has
/// a `'` and no `"`, and uses the short escapes `\n`, `\r`, `\t` before the
/// generic `\xNN` form. Getting this wrong is observable anywhere a `bytes` value
/// is printed, so the rule lives once, here.
fn bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut rendered = String::with_capacity(bytes.len() + 3);
    rendered.push('b');
    rendered.push(quote as char);
    for byte in bytes {
        match *byte {
            b'\\' => rendered.push_str("\\\\"),
            b'\n' => rendered.push_str("\\n"),
            b'\r' => rendered.push_str("\\r"),
            b'\t' => rendered.push_str("\\t"),
            byte if byte == quote => {
                rendered.push('\\');
                rendered.push(byte as char);
            }
            0x20..=0x7e => rendered.push(*byte as char),
            other => rendered.push_str(&format!("\\x{other:02x}")),
        }
    }
    rendered.push(quote as char);
    rendered
}

/// Build the graph for one constructed semantic document.
///
/// `markers` recovers exact code points for transported scalars; `dialect`
/// decides the mapping-key policy for the identity-sensitive entries (PyYAML
/// keeps the first key position and the last value, ruamel refuses a duplicate).
pub fn build(
    value: &Value,
    markers: &BTreeMap<String, PythonString>,
    dialect: Dialect,
) -> Result<ObjectGraph, YamlError> {
    let mut builder = Builder {
        nodes: Vec::new(),
        anchors: BTreeMap::new(),
    };
    let root = builder.build(value, markers, dialect)?;
    Ok(ObjectGraph {
        nodes: Arc::new(builder.nodes),
        root,
    })
}

struct Builder {
    #[allow(clippy::vec_box)]
    nodes: Vec<Box<Object>>,
    anchors: BTreeMap<u32, NodeId>,
}

impl Builder {
    fn reserve(&mut self) -> NodeId {
        let id = NodeId(u32::try_from(self.nodes.len()).expect("object count fits in u32"));
        self.nodes.push(Box::new(Object::Null));
        id
    }

    fn store(&mut self, id: NodeId, object: Object) {
        // Replace in place: the box's allocation (and therefore its address) is
        // the object's identity for pointer hashing.
        *self.nodes[id.index()] = object;
    }

    fn push(&mut self, object: Object) -> NodeId {
        let id = self.reserve();
        self.store(id, object);
        id
    }

    fn build(
        &mut self,
        value: &Value,
        markers: &BTreeMap<String, PythonString>,
        dialect: Dialect,
    ) -> Result<NodeId, YamlError> {
        match value {
            Value::Null => Ok(self.push(Object::Null)),
            Value::Bool(flag) => Ok(self.push(Object::Bool(*flag))),
            Value::Number(number) => Ok(self.push(number_object(number))),
            Value::String(text) => Ok(self.push(Object::Str(exact_string(text, markers)))),
            Value::Sequence(items) => {
                let id = self.reserve();
                let mut links = Vec::with_capacity(items.len());
                for item in items {
                    links.push(self.build(item, markers, dialect)?);
                }
                self.store(id, Object::Seq(links));
                Ok(id)
            }
            Value::Mapping(mapping) => {
                let id = self.reserve();
                let mut entries: Vec<(NodeId, NodeId)> = Vec::with_capacity(mapping.len());
                for (key, value) in mapping.iter() {
                    let key = self.build(key, markers, dialect)?;
                    let value = self.build(value, markers, dialect)?;
                    insert_entry(&mut entries, key, value, self, markers, dialect)?;
                }
                self.store(id, Object::Map(entries));
                Ok(id)
            }
            Value::Tagged(_) => self.build_tagged(value, markers, dialect),
        }
    }

    fn build_tagged(
        &mut self,
        value: &Value,
        markers: &BTreeMap<String, PythonString>,
        dialect: Dialect,
    ) -> Result<NodeId, YamlError> {
        let Value::Tagged(tagged) = value else {
            unreachable!("the caller proved a tagged value");
        };
        let tag = tagged.tag.to_string();
        if let Some(ordinal) = tag.strip_prefix(ANCHOR_NULL_TAG_PREFIX) {
            let ordinal = parse_ordinal(ordinal)?;
            let id = self.push(Object::Null);
            self.anchors.insert(ordinal, id);
            return Ok(id);
        }
        if let Some(ordinal) = tag.strip_prefix(ANCHOR_TAG_PREFIX) {
            let ordinal = parse_ordinal(ordinal)?;
            // Reserve the anchor's own object **before** descending, so an alias
            // inside the anchored node (a self-reference) links to it.
            let id = self.reserve();
            self.anchors.insert(ordinal, id);
            let built = self.build(&tagged.value, markers, dialect)?;
            let object = std::mem::replace(&mut *self.nodes[built.index()], Object::Null);
            self.store(id, object);
            return Ok(id);
        }
        if let Some(ordinal) = tag.strip_prefix(ALIAS_TAG_PREFIX) {
            let ordinal = parse_ordinal(ordinal)?;
            return self.anchors.get(&ordinal).copied().ok_or_else(|| {
                YamlError::Construct(format!(
                    "found undefined alias (graph link {ordinal} has no anchor)"
                ))
            });
        }
        if tag.starts_with(FRESH_NAN_TAG_PREFIX) {
            return Ok(self.push(Object::Float {
                value: f64::NAN,
                provenance: FloatProvenance::Fresh,
            }));
        }
        let inner = Value::Tagged(tagged.clone());
        if let Some(bytes) = construct::constructed_bytes(&inner) {
            return Ok(self.push(Object::Bytes(bytes)));
        }
        if let Some(digits) = construct::constructed_big_integer(&inner) {
            return Ok(self.push(Object::Int(digits.to_owned())));
        }
        if construct::is_constructed_timestamp(&inner) {
            let text = construct::constructed_timestamp_text(&inner).unwrap_or_default();
            let object = if text.contains(' ') && text.contains(':') {
                Object::DateTime(text)
            } else {
                Object::Date(text)
            };
            return Ok(self.push(object));
        }
        if let Some(items) = construct::constructed_tuple(&inner) {
            let id = self.reserve();
            let mut links = Vec::with_capacity(items.len());
            for item in items {
                links.push(self.build(item, markers, dialect)?);
            }
            self.store(id, Object::Tuple(links));
            return Ok(id);
        }
        if let Some(members) = construct::constructed_set(&inner) {
            let id = self.reserve();
            let mut links: Vec<NodeId> = Vec::with_capacity(members.len());
            for member in members {
                let member = self.build(member, markers, dialect)?;
                if !links.contains(&member)
                    && !links
                        .iter()
                        .any(|other| self_graph_equal(self, *other, member))
                {
                    links.push(member);
                }
            }
            self.store(id, Object::Set(links));
            return Ok(id);
        }
        if let Some(entries) = construct::constructed_omap(&inner) {
            let id = self.reserve();
            let mut links: Vec<(NodeId, NodeId)> = Vec::with_capacity(entries.len());
            for (key, value) in entries.iter() {
                let key = self.build(key, markers, dialect)?;
                let value = self.build(value, markers, dialect)?;
                insert_entry(&mut links, key, value, self, markers, dialect)?;
            }
            self.store(id, Object::OrderedDict(links));
            return Ok(id);
        }
        if let Some(items) = construct::constructed_pairs(&inner) {
            let id = self.reserve();
            let mut links: Vec<(NodeId, NodeId)> = Vec::with_capacity(items.len());
            for item in items {
                // `!!pairs` stores each entry as a real `(key, value)` tuple; a
                // PyYAML `!!omap` keeps single-entry mappings.
                let (key, value) = match construct::constructed_tuple(item) {
                    Some(members) if members.len() == 2 => (members[0].clone(), members[1].clone()),
                    _ => single_entry(item)?,
                };
                let key = self.build(&key, markers, dialect)?;
                let value = self.build(&value, markers, dialect)?;
                links.push((key, value));
            }
            self.store(id, Object::Pairs(links));
            return Ok(id);
        }
        // Any other tag was already refused by the semantic constructor.
        Err(YamlError::Construct(format!(
            "could not determine a constructor for the tag '{tag}'"
        )))
    }
}

/// A `serde_yaml_ng` number as its Python object.
fn number_object(number: &serde_yaml_ng::Number) -> Object {
    if let Some(integer) = number.as_i64() {
        return Object::Int(integer.to_string());
    }
    if let Some(unsigned) = number.as_u64() {
        return Object::Int(unsigned.to_string());
    }
    Object::Float {
        value: number.as_f64().unwrap_or(f64::NAN),
        provenance: FloatProvenance::LoaderSingleton,
    }
}

fn parse_ordinal(text: &str) -> Result<u32, YamlError> {
    text.parse::<u32>().map_err(|_| {
        YamlError::Construct("a graph identity tag lost its anchor ordinal".to_owned())
    })
}

fn exact_string(text: &str, markers: &BTreeMap<String, PythonString>) -> PythonString {
    match markers.get(text) {
        Some(exact) => exact.clone(),
        None => PythonString::from_scalars(text),
    }
}

/// The one `(key, value)` of a `!!pairs`/`!!omap` list entry.
fn single_entry(item: &Value) -> Result<(Value, Value), YamlError> {
    let Value::Mapping(mapping) = item else {
        return Err(YamlError::Construct(
            "expected a mapping of a single (key, value) pair".to_owned(),
        ));
    };
    let mut entries = mapping.iter();
    let (Some((key, value)), None) = (entries.next(), entries.next()) else {
        return Err(YamlError::Construct(
            "expected a mapping of a single (key, value) pair".to_owned(),
        ));
    };
    Ok((key.clone(), value.clone()))
}

/// Python `dict[key] = value` over object identity, then equality.
fn insert_entry(
    entries: &mut Vec<(NodeId, NodeId)>,
    key: NodeId,
    value: NodeId,
    builder: &Builder,
    _markers: &BTreeMap<String, PythonString>,
    dialect: Dialect,
) -> Result<(), YamlError> {
    let position = entries
        .iter()
        .position(|(existing, _)| *existing == key || self_graph_equal(builder, *existing, key));
    if let Some(position) = position {
        if dialect.refuses_duplicate_keys() {
            return Err(YamlError::Construct(format!(
                "found duplicate key {}",
                builder
                    .nodes
                    .get(key.index())
                    .map(|object| render_partial(object))
                    .unwrap_or_else(|| "<unhashable>".to_owned())
            )));
        }
        entries[position].1 = value;
        return Ok(());
    }
    entries.push((key, value));
    Ok(())
}

/// Structural equality over a graph that is still being built.
fn self_graph_equal(builder: &Builder, left: NodeId, right: NodeId) -> bool {
    let mut seen: BTreeSet<(NodeId, NodeId)> = BTreeSet::new();
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        if left == right {
            continue;
        }
        if !seen.insert((left, right)) {
            continue;
        }
        let (Some(a), Some(b)) = (
            builder.nodes.get(left.index()),
            builder.nodes.get(right.index()),
        ) else {
            return false;
        };
        match (a.as_ref(), b.as_ref()) {
            (Object::Null, Object::Null) => {}
            (Object::Bool(a), Object::Bool(b)) if a == b => {}
            (Object::Int(a), Object::Int(b)) if a == b => {}
            (Object::Float { value: a, .. }, Object::Float { value: b, .. }) => {
                if !(a == b) {
                    return false;
                }
            }
            (Object::Str(a), Object::Str(b)) if a == b => {}
            (Object::Bytes(a), Object::Bytes(b)) if a == b => {}
            (Object::Date(a), Object::Date(b)) if a == b => {}
            (Object::DateTime(a), Object::DateTime(b)) if a == b => {}
            (Object::Seq(a), Object::Seq(b)) | (Object::Tuple(a), Object::Tuple(b))
                if a.len() == b.len() =>
            {
                pending.extend(a.iter().copied().zip(b.iter().copied()));
            }
            (Object::Set(a), Object::Set(b)) if a.len() == b.len() => {
                for item in a {
                    if !b
                        .iter()
                        .any(|other| *item == *other || self_graph_equal(builder, *item, *other))
                    {
                        return false;
                    }
                }
            }
            (Object::Map(a), Object::Map(b)) | (Object::OrderedDict(a), Object::OrderedDict(b))
                if a.len() == b.len() =>
            {
                for (key, value) in a {
                    let matched = b.iter().any(|(other_key, other)| {
                        (*key == *other_key || self_graph_equal(builder, *key, *other_key))
                            && (*value == *other || self_graph_equal(builder, *value, *other))
                    });
                    if !matched {
                        return false;
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

fn render_partial(object: &Object) -> String {
    match object {
        Object::Null => "None".to_owned(),
        Object::Bool(true) => "True".to_owned(),
        Object::Bool(false) => "False".to_owned(),
        Object::Int(digits) => digits.clone(),
        Object::Float { value, .. } => crate::python_json::python_float_repr(*value),
        Object::Str(text) => text.repr().to_string(),
        Object::Bytes(bytes) => bytes_repr(bytes),
        Object::Date(text) | Object::DateTime(text) => timestamp_repr(text),
        Object::Seq(_) => "[]".to_owned(),
        Object::Tuple(_) => "()".to_owned(),
        Object::Set(_) => "set()".to_owned(),
        Object::Map(_) | Object::OrderedDict(_) => "{}".to_owned(),
        Object::Pairs(_) => "[]".to_owned(),
    }
}
