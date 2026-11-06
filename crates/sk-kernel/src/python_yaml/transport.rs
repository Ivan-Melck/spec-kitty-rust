//! Lossless transport for authored YAML text into exact Python code points.
//!
//! Shared by both Source dialects (`PyYamlSafe11` for `mission.yaml`,
//! `RuamelSafe12` for profiles/offerings/project config): the syntax/transport
//! mechanism is one; only the implicit-scalar *resolver* it re-spells and the
//! construction/duplicate policy differ, and both come from [`super::Dialect`].
//!
//! The reference loads profiles with `ruamel.yaml`'s safe reader, so an
//! authored `"author\ud800"` is a Python `str` holding code point `U+D800`.
//! `serde_yaml_ng` (unsafe-libyaml) refuses that whole *document* at the
//! reader, and `yaml_edit`'s own value decoder falls back to the literal text
//! `\ud800`, which is not the authored value. Neither can be used as the data
//! authority for a lossy scalar, so this module transports the authored token
//! exactly across the existing parser:
//!
//! 1. the authored text is parsed into `yaml_edit`'s lossless syntax tree,
//!    which keeps every scalar token verbatim;
//! 2. a string containing surrogate units OR shaped like a transport marker is
//!    encoded injectively into a marker of the same YAML type; and
//! 3. the marked text is parsed by `serde_yaml_ng`, the single semantic
//!    authority for structure, keys, anchors, `<<` merge keys, numbers, bools
//!    and null.
//!
//! The marker is an injective function of the exact code points
//! (`U+FDD0` + the lowercase hex code units joined by `.` + `U+FDEF`). Equal
//! authored strings therefore produce equal markers and different strings
//! produce different markers, so equality, typing, `Mapping` key identity,
//! de-duplication and every merge rule keep their exact meaning *before* the
//! value is ever restored. Compare this with a fixed `\u0000` shadow, where
//! `"author\ud800"` and an authored NUL collide: that is why a private
//! constant substitution is not usable here.
//!
//! Authored marker-shaped strings are ordinary valid data, not reserved input.
//! They are encoded too. Thus encoded outputs cannot collide with any unchanged
//! string, and literal markers remain distinct from surrogate-bearing strings,
//! including when bodies from different files are merged.
//!
//! Only double-quoted scalars can be lossy: a raw surrogate code unit cannot
//! exist in the UTF-8 file, single-quoted and block scalars never unescape, and
//! plain scalars keep a backslash literally. `\xNN` cannot reach the surrogate
//! range either. No surrogate pair is ever combined: the reference preserves
//! `"\uD83D\uDE00"` as two code units, and so does this decoder.

use std::collections::BTreeMap;

use crate::PythonString;
use serde_yaml_ng::{Mapping, Value};
use yaml_edit::visitor::{YamlAccept, YamlVisitor};
use yaml_edit::{Document, Scalar, YamlNode};

/// First code unit of a transport marker (`U+FDD0`, a Unicode noncharacter).
const MARKER_START: char = '\u{fdd0}';
/// Last code unit of a transport marker (`U+FDEF`, a Unicode noncharacter).
const MARKER_END: char = '\u{fdef}';

/// The shadow spelling of a genuine YAML merge key.
///
/// Authored strings with the marker shape are themselves transported, so no
/// authored key or value can ever equal this spelling. A plain `<<` is a merge
/// key only when the YAML resolver gives it `tag:yaml.org,2002:merge`; a quoted
/// `'<<'`, a `!!str <<` and any other explicitly string-tagged key stay literal
/// data (the reference's `flatten_mapping` keys off the node tag).
const MERGE_KEY_PREFIX: &str = "\u{fdd0}merge";

/// The marker spelling of the `n`-th genuine merge key in the document.
///
/// One marker per merge key (not one shared marker) because PyYAML accepts a
/// mapping with two `<<` keys and applies both; a shared spelling would look
/// like a duplicate key to the semantic reader.
fn merge_marker(index: usize) -> String {
    format!("{MERGE_KEY_PREFIX}.{index}{MARKER_END}")
}

/// Whether a synthesized key is a merge-key marker (never authored data, since
/// marker-shaped authored strings are themselves transported).
fn is_merge_marker(value: &str) -> bool {
    value.starts_with(MERGE_KEY_PREFIX) && value.ends_with(MARKER_END)
}

/// The YAML merge tag the reference's resolver assigns to a plain `<<`.
const MERGE_TAG: &str = "tag:yaml.org,2002:merge";
/// The YAML string tag: an explicit `!!str` overrides the merge resolution.
const STRING_TAG: &str = "tag:yaml.org,2002:str";

use super::Dialect;
use super::error::YamlError;
use super::graph::{ALIAS_TAG_PREFIX, ANCHOR_TAG_PREFIX, FRESH_NAN_TAG_PREFIX};
use yaml_edit::anchor_resolution::AnchorRegistry;

/// Authored text plus the marked form the semantic reader consumes.
#[derive(Clone, Debug, Default)]
pub struct LosslessText {
    shadow: String,
    markers: BTreeMap<String, PythonString>,
    duplicate_keys: BTreeMap<String, (&'static str, String)>,
}

impl LosslessText {
    /// The text handed to the semantic reader (markers in place of lossy tokens).
    pub fn shadow(&self) -> &str {
        &self.shadow
    }

    /// Exact authored values recovered by this transport, keyed by marker.
    pub fn markers(&self) -> &BTreeMap<String, PythonString> {
        &self.markers
    }

    /// PyYAML duplicate keys renamed aside: marker -> (resolved kind, payload).
    ///
    /// Ruamel never produces these (a duplicate key is refused); PyYAML keeps
    /// the last value in the first key's position, so the later key is renamed
    /// here and collapsed after construction (`resolve_duplicate_keys`).
    pub fn duplicate_keys(&self) -> &BTreeMap<String, (&'static str, String)> {
        &self.duplicate_keys
    }
}

/// Whether the stream contains no document at all (only comments/blank lines).
fn is_blank_document(text: &str) -> bool {
    text.lines().all(|line| {
        let trimmed = line.trim_start();
        trimmed.is_empty() || trimmed.starts_with('#')
    })
}

/// Whether `value` has the marker shape. Authored strings with this shape must
/// be encoded as data too, so they cannot collide with transport outputs.
fn is_marker(value: &str) -> bool {
    value.starts_with(MARKER_START) && value.ends_with(MARKER_END)
}

/// The injective marker for an exact code-point string.
fn marker_for(codepoints: &[u32]) -> String {
    let mut marker = String::with_capacity(codepoints.len() * 5 + 2);
    marker.push(MARKER_START);
    for (position, code) in codepoints.iter().enumerate() {
        if position > 0 {
            marker.push('.');
        }
        marker.push_str(&format!("{code:x}"));
    }
    marker.push(MARKER_END);
    marker
}

/// The double-quoted YAML token that parses back to `marker`.
fn marker_token(marker: &str) -> String {
    let mut token = String::with_capacity(marker.len() + 16);
    token.push('"');
    for character in marker.chars() {
        match character {
            MARKER_START => token.push_str("\\uFDD0"),
            MARKER_END => token.push_str("\\uFDEF"),
            other => token.push(other),
        }
    }
    token.push('"');
    token
}

/// Transport `text` into the marked form plus its exact values.
///
/// The authored bytes are never rewritten; the shadow is a separate string.
/// `dialect` selects the implicit-scalar resolver that decides each plain
/// scalar's tag, and the duplicate-key policy of the loader it belongs to.
pub fn transport(text: &str, dialect: Dialect) -> Result<LosslessText, YamlError> {
    if is_blank_document(text) {
        // A stream with no document (empty, or only comments/blank lines) is
        // `None` in both Source loaders. The lossless syntax tree refuses a
        // comment-only stream, so the empty document is emitted directly.
        return Ok(LosslessText {
            shadow: String::new(),
            markers: BTreeMap::new(),
            duplicate_keys: BTreeMap::new(),
        });
    }
    let document: Document = text
        .parse()
        .map_err(|error: yaml_edit::YamlError| YamlError::Syntax(error.to_string()))?;
    let mut collector = LossyTokens {
        markers: BTreeMap::new(),
        replacements: Vec::new(),
    };
    document.accept(&mut collector);
    // A genuine merge key is rewritten to an unreachable marker key so the
    // semantic reader can tell it apart from a quoted literal `'<<'`. The
    // rewrite covers the whole key node (including an explicit `!!merge` tag),
    // so a lossy token inside that key is subsumed by this replacement.
    let mut merge_spans = Vec::new();
    collect_merge_spans(&document, &mut merge_spans);
    collector.replacements.retain(|(start, end, _)| {
        !merge_spans
            .iter()
            .any(|(merge_start, merge_end)| *start < *merge_end && *merge_start < *end)
    });
    for (index, (start, end)) in merge_spans.iter().enumerate() {
        let marker = merge_marker(index);
        collector
            .replacements
            .push((*start, *end, marker_token(&marker)));
    }
    // Every authored plain scalar is implicitly typed by the loader's own
    // resolver (YAML 1.1 for PyYAML, YAML 1.2 for ruamel), and every authored
    // core tag (`!!binary` or its verbatim/`%TAG` spellings) is resolved by the
    // semantic reader as a *scalar type*, dropping the tag entirely
    // (`!!binary SGVsbG8=` reads as the string `SGVsbG8=`). Keep the
    // reference's construction phase in charge: each plain scalar and each
    // core-tagged scalar is re-spelled as the equivalent local tag with a
    // quoted payload, which the reader preserves. An authored tag outside the
    // core vocabulary is the reference's constructor error, refused here
    // (before any model validation).
    let mut rewrite = Rewrite::new(text, dialect);
    rewrite.document(&document)?;
    let tag_spans = std::mem::take(&mut rewrite.spans);
    for (marker, exact) in rewrite.markers {
        collector.markers.insert(marker, exact);
    }
    let duplicate_keys = rewrite.duplicate_keys;
    collector.replacements.retain(|(start, end, _)| {
        !tag_spans
            .iter()
            .any(|(tag_start, tag_end, _)| *start < *tag_end && *tag_start < *end)
    });
    collector.replacements.extend(tag_spans);
    if collector.replacements.is_empty() {
        return Ok(LosslessText {
            shadow: text.to_owned(),
            markers: BTreeMap::new(),
            duplicate_keys,
        });
    }
    collector
        .replacements
        .sort_by_key(|(start, end, _)| (*start, *end));
    let mut shadow = String::with_capacity(text.len() + collector.replacements.len() * 8);
    let mut cursor = 0usize;
    for (start, end, replacement) in &collector.replacements {
        if *start < cursor || *end > text.len() {
            return Err(YamlError::Syntax(
                "the lossless syntax tree reported overlapping scalar spans".to_owned(),
            ));
        }
        shadow.push_str(&text[cursor..*start]);
        shadow.push_str(replacement);
        cursor = *end;
    }
    shadow.push_str(&text[cursor..]);
    Ok(LosslessText {
        shadow,
        markers: collector.markers,
        duplicate_keys,
    })
}

/// Walk the lossless syntax tree, recording the span of every genuine merge
/// key. The document root itself may be tagged.
fn collect_merge_spans(document: &Document, spans: &mut Vec<(usize, usize)>) {
    if let Some(mapping) = document.as_mapping() {
        collect_mapping_merge_spans(&mapping, spans);
    } else if let Some(sequence) = document.as_sequence() {
        collect_sequence_merge_spans(&sequence, spans);
    } else if let Some(tagged) = document.as_tagged() {
        if let Some(mapping) = tagged.as_mapping() {
            collect_mapping_merge_spans(&mapping, spans);
        } else if let Some(sequence) = tagged.as_sequence() {
            collect_sequence_merge_spans(&sequence, spans);
        }
    }
}

fn collect_mapping_merge_spans(mapping: &yaml_edit::Mapping, spans: &mut Vec<(usize, usize)>) {
    for (key, value) in mapping.iter() {
        if is_merge_key(&key) {
            let range = key.byte_range();
            spans.push((range.start as usize, range.end as usize));
        }
        collect_value_merge_spans(&value, spans);
    }
}

fn collect_sequence_merge_spans(sequence: &yaml_edit::Sequence, spans: &mut Vec<(usize, usize)>) {
    for value in sequence.values() {
        collect_value_merge_spans(&value, spans);
    }
}

fn collect_value_merge_spans(node: &YamlNode, spans: &mut Vec<(usize, usize)>) {
    match node {
        YamlNode::Mapping(mapping) => collect_mapping_merge_spans(mapping, spans),
        YamlNode::Sequence(sequence) => collect_sequence_merge_spans(sequence, spans),
        YamlNode::TaggedNode(tagged) => {
            if let Some(mapping) = tagged.as_mapping() {
                collect_mapping_merge_spans(&mapping, spans);
            } else if let Some(sequence) = tagged.as_sequence() {
                collect_sequence_merge_spans(&sequence, spans);
            }
        }
        YamlNode::Scalar(_) | YamlNode::Alias(_) => {}
    }
}

/// The core tags the reference's safe constructor implements, with whether the
/// tag attaches to a scalar.
fn core_kind(resolved: &str) -> Option<(&'static str, bool)> {
    for (name, scalar) in [
        ("null", true),
        ("bool", true),
        ("int", true),
        ("float", true),
        ("binary", true),
        ("timestamp", true),
        ("str", true),
        ("omap", false),
        ("pairs", false),
        ("set", false),
        ("seq", false),
        ("map", false),
    ] {
        if resolved == format!("!!{name}") {
            return Some((name, scalar));
        }
    }
    None
}

/// The reference's constructor error for a tag it cannot construct.
fn unknown_tag_error(resolved: &str) -> YamlError {
    let spelling = resolved
        .strip_prefix("!<")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(resolved);
    YamlError::Construct(format!(
        "could not determine a constructor for the tag {}",
        PythonString::from_scalars(spelling).repr()
    ))
}

/// Rewrite state for one authored document.
///
/// One pass over the lossless syntax tree does three things the semantic reader
/// cannot do for itself: re-spell every authored scalar with the *dialect's*
/// resolved tag, keep lossy strings exact, and apply the dialect's duplicate-key
/// policy (ruamel refuses; PyYAML keeps the last value in the first position,
/// which requires renaming the later key aside because the semantic reader
/// rejects a repeated key outright).
struct Rewrite<'a> {
    text: &'a str,
    dialect: Dialect,
    spans: Vec<(usize, usize, String)>,
    markers: BTreeMap<String, PythonString>,
    duplicate_keys: BTreeMap<String, (&'static str, String)>,
    /// Each `!!float nan` object needs its own marker so two of them stay two
    /// distinct mapping keys in the semantic document.
    fresh_nan_ordinal: u32,
}

impl<'a> Rewrite<'a> {
    fn new(text: &'a str, dialect: Dialect) -> Self {
        Self {
            text,
            dialect,
            spans: Vec::new(),
            markers: BTreeMap::new(),
            duplicate_keys: BTreeMap::new(),
            fresh_nan_ordinal: 0,
        }
    }

    fn document(&mut self, document: &Document) -> Result<(), YamlError> {
        if let Some(tagged) = document.as_tagged() {
            let node = YamlNode::TaggedNode(tagged.clone());
            return self.tagged(&node, &tagged, false);
        }
        if let Some(mapping) = document.as_mapping() {
            return self.mapping(&mapping);
        }
        if let Some(sequence) = document.as_sequence() {
            return self.sequence(&sequence);
        }
        if let Some(scalar) = document.as_scalar() {
            return self.scalar(&scalar, false);
        }
        Ok(())
    }

    fn node(&mut self, node: &YamlNode, is_key: bool) -> Result<(), YamlError> {
        match node {
            YamlNode::Mapping(mapping) => self.mapping(mapping),
            YamlNode::Sequence(sequence) => self.sequence(sequence),
            YamlNode::Scalar(scalar) => self.scalar(scalar, is_key),
            YamlNode::Alias(_) => Ok(()),
            YamlNode::TaggedNode(tagged) => self.tagged(node, tagged, is_key),
        }
    }

    fn mapping(&mut self, mapping: &yaml_edit::Mapping) -> Result<(), YamlError> {
        let mut seen: Vec<String> = Vec::new();
        for (key, value) in mapping.iter() {
            if is_merge_key(&key) {
                // The merge key's own span is owned by `collect_merge_spans`.
                self.node(&value, false)?;
                continue;
            }
            self.key(&key, &mut seen)?;
            self.node(&value, false)?;
        }
        Ok(())
    }

    fn sequence(&mut self, sequence: &yaml_edit::Sequence) -> Result<(), YamlError> {
        for value in sequence.values() {
            self.node(&value, false)?;
        }
        Ok(())
    }

    /// Rewrite one mapping key and apply the dialect's duplicate-key policy.
    fn key(&mut self, node: &YamlNode, seen: &mut Vec<String>) -> Result<(), YamlError> {
        if let Some(signature) = key_signature(node, self.dialect) {
            if seen.contains(&signature) {
                if self.dialect.refuses_duplicate_keys() {
                    return Err(YamlError::Construct(format!(
                        "found duplicate key {}",
                        key_spelling(node)
                    )));
                }
                if let Some((kind, payload)) = key_scalar(node, self.dialect) {
                    let marker = format!(
                        "{MARKER_START}dup.{}{MARKER_END}",
                        self.duplicate_keys.len()
                    );
                    self.duplicate_keys.insert(marker.clone(), (kind, payload));
                    self.markers
                        .insert(marker.clone(), PythonString::from_scalars(&marker));
                    let range = node.byte_range();
                    self.spans.push((
                        range.start as usize,
                        range.end as usize,
                        marker_token(&marker),
                    ));
                    return Ok(());
                }
            } else {
                seen.push(signature);
            }
        }
        self.node(node, true)
    }

    fn tagged(
        &mut self,
        node: &YamlNode,
        tagged: &yaml_edit::TaggedNode,
        is_key: bool,
    ) -> Result<(), YamlError> {
        let resolved = tagged.resolved_tag().unwrap_or_default();
        if resolved == "!!merge" {
            if is_key {
                // The merge-key span owns this whole key node.
                return Ok(());
            }
            return Err(unknown_tag_error(&resolved));
        }
        if resolved == "!!value" {
            if !is_key {
                return Err(unknown_tag_error(&resolved));
            }
            // The reference converts a `value`-tagged key to a string key.
            if let Some((tag_start, tag_end)) = self.tag_token_range(node, tagged) {
                self.spans.push((tag_start, tag_end, "!str".to_owned()));
            }
            return Ok(());
        }
        let Some((name, scalar_tag)) = core_kind(&resolved) else {
            return Err(unknown_tag_error(&resolved));
        };
        let Some((tag_start, tag_end)) = self.tag_token_range(node, tagged) else {
            return Err(YamlError::Syntax(
                "a tagged node has no tag token".to_owned(),
            ));
        };
        let local = if scalar_tag && fresh_nan_scalar(node).is_some() {
            // `!!float nan` constructs a *fresh* float object (Source: two keys),
            // unlike `.nan`'s loader singleton. The marker carries the object's
            // ordinal, so the semantic reader keeps both keys.
            let ordinal = self.fresh_nan_ordinal;
            self.fresh_nan_ordinal += 1;
            format!("{FRESH_NAN_TAG_PREFIX}{ordinal} ")
        } else {
            format!("!{name}")
        };
        if scalar_tag {
            if let Some(scalar) = tagged.value() {
                if let Some((payload, exact)) = scalar_payload(&scalar, &mut self.markers) {
                    if let Some((marker, exact)) = exact {
                        self.markers.insert(marker, exact);
                    }
                    let mut replacement = quote_scalar_text(&payload);
                    let raw = scalar.value();
                    if raw.ends_with("\r\n") {
                        replacement.push_str("\r\n");
                    } else if raw.ends_with('\n') {
                        replacement.push('\n');
                    } else if raw.ends_with('\r') {
                        replacement.push('\r');
                    }
                    let range = scalar.byte_range();
                    self.spans
                        .push((range.start as usize, range.end as usize, replacement));
                }
                self.spans.push((tag_start, tag_end, local));
                return Ok(());
            }
            // A scalar tag on a collection: keep the tag and let the
            // construction phase report the reference's node-kind error.
            self.spans.push((tag_start, tag_end, local));
            return Ok(());
        }
        self.spans.push((tag_start, tag_end, local));
        if let Some(mapping) = tagged.as_mapping() {
            return self.mapping(&mapping);
        }
        if let Some(sequence) = tagged.as_sequence() {
            return self.sequence(&sequence);
        }
        Ok(())
    }

    /// Rewrite one authored plain scalar as the dialect's resolver types it.
    ///
    /// Both value *and* key positions are resolved: PyYAML's loader resolves a
    /// key too (`yes: 1` is `{True: 1}`, `017: 1` is `{15: 1}`), and a mapping's
    /// Python key identity is exactly what the model's diagnostics and the
    /// duplicate-key policy depend on. A quoted or block scalar is already a
    /// string; an empty token is an omitted value (null) and has no span.
    fn scalar(&mut self, scalar: &Scalar, is_key: bool) -> Result<(), YamlError> {
        if scalar.is_quoted() || scalar.is_block() {
            return Ok(());
        }
        let token = scalar.value();
        if token.is_empty() {
            return Ok(());
        }
        if token == "<<" {
            // The merge tag has no safe-constructor entry as a *value*.
            return if is_key {
                Ok(())
            } else {
                Err(unknown_tag_error(MERGE_TAG))
            };
        }
        if token == "=" {
            return if is_key {
                Ok(())
            } else {
                Err(unknown_tag_error("tag:yaml.org,2002:value"))
            };
        }
        let Some((payload, exact)) = scalar_payload(scalar, &mut self.markers) else {
            return Ok(());
        };
        let carried = exact.is_some();
        if let Some((marker, exact)) = exact {
            self.markers.insert(marker, exact);
        }
        let kind = super::dialect::implicit_scalar_kind(&payload, self.dialect);
        if kind == "str" && !carried && !reads_as_typed(&payload) {
            // When the dialect resolves a scalar to a plain `str` *and* the
            // semantic reader would also read it as a string, the authored token
            // can stay as-is. A typed scalar needs the explicit local tag, and a
            // token the 1.2-core reader would type itself (`1e5` is a float
            // there but a string in PyYAML) needs the tag to *hold* it a string.
            return Ok(());
        }
        let replacement = format!("!{kind} {}", quote_scalar_text(&payload));
        let range = scalar.byte_range();
        self.spans
            .push((range.start as usize, range.end as usize, replacement));
        Ok(())
    }

    /// The byte range of the authored tag token inside a tagged node.
    fn tag_token_range(
        &self,
        node: &YamlNode,
        tagged: &yaml_edit::TaggedNode,
    ) -> Option<(usize, usize)> {
        let raw = tagged.tag()?;
        let range = node.byte_range();
        let start = range.start as usize;
        let end = range.end as usize;
        let slice = self.text.get(start..end)?;
        let offset = slice.find(&raw)?;
        Some((start + offset, start + offset + raw.len()))
    }
}

/// Whether YAML 1.2's core schema would type this plain scalar as non-string.
///
/// Conservative on purpose: a false positive only costs an explicit `!str` tag.
fn reads_as_typed(text: &str) -> bool {
    let body = text
        .strip_prefix('-')
        .or_else(|| text.strip_prefix('+'))
        .unwrap_or(text);
    body.starts_with(|character: char| character.is_ascii_digit() || character == '.')
        || matches!(
            text,
            "~" | "null"
                | "Null"
                | "NULL"
                | "true"
                | "True"
                | "TRUE"
                | "false"
                | "False"
                | "FALSE"
                | ""
        )
}

/// The dialect-resolved identity of one scalar mapping key.
///
/// A Python `dict` has one entry per *equal* key, so `yes`/`true` (both `True`),
/// `1`/`01` and `017`/`15` (both fifteen) are the same key. The signature is the
/// constructed scalar rendered canonically. A key that contains a collection is
/// not compared here — see [`super::OPEN_CASES`].
fn key_signature(node: &YamlNode, dialect: Dialect) -> Option<String> {
    if fresh_nan_scalar(node).is_some() {
        // `!!float nan` constructs a **fresh** float object, so two such keys are
        // two distinct Python keys (probed: both loaders keep two entries). It is
        // not a duplicate and has no value-derived signature.
        return None;
    }
    let (kind, payload) = key_scalar(node, dialect)?;
    let value = super::construct::construct_scalar(kind, &payload, dialect)?;
    canonical_key(&value)
}

/// The scalar of an explicit `!!float nan` key/value, which is a fresh object.
///
/// PyYAML's `construct_yaml_float` returns the loader's `nan_value` for the
/// `.nan` spelling and `math.nan` (a new float) for a bare `nan` payload; ruamel
/// behaves the same way. The `.nan` singleton is not fresh, so it is not matched
/// here.
fn fresh_nan_scalar(node: &YamlNode) -> Option<Scalar> {
    let YamlNode::TaggedNode(tagged) = node else {
        return None;
    };
    let tag = tagged.resolved_tag()?;
    if !matches!(
        tag.as_str(),
        "tag:yaml.org,2002:float" | "!!float" | "!float"
    ) {
        return None;
    }
    let scalar = tagged.value()?;
    scalar
        .as_string()
        .trim()
        .eq_ignore_ascii_case("nan")
        .then_some(scalar)
}

/// The resolved scalar spelling of a mapping key node.
fn key_scalar(node: &YamlNode, dialect: Dialect) -> Option<(&'static str, String)> {
    match node {
        YamlNode::Scalar(scalar) => {
            if scalar.is_quoted() || scalar.is_block() {
                return Some(("str", scalar.as_string()));
            }
            let token = scalar.value().to_owned();
            if token.is_empty() {
                return Some(("null", token));
            }
            Some((super::dialect::implicit_scalar_kind(&token, dialect), token))
        }
        YamlNode::TaggedNode(tagged) => {
            let resolved = tagged.resolved_tag().unwrap_or_default();
            let (name, _) = core_kind(&resolved)?;
            let payload = tagged.value()?.value().to_owned();
            Some((name, payload))
        }
        _ => None,
    }
}

/// Identity markers spliced into the marked text for the canonical object graph.
///
/// The semantic reader expands an alias into a copy and cannot represent a
/// cycle, so the graph layer marks the sites instead: an alias becomes a tagged
/// empty scalar carrying the anchor's ordinal, and the anchor's node gets the
/// matching tag. `super::graph::build` turns those markers into real
/// [`super::graph::NodeId`] links, so sharing and cycles survive without a
/// second parser: the dialect resolver, tag constructors, merge keys and
/// duplicate policy stay exactly the ones the semantic document already used.
///
/// The markers are not data. Authored text cannot produce them: the transport
/// already refuses an authored tag outside the core vocabulary, and these are
/// synthesized after that pass.
///
/// An anchored node that already carries an authored tag (`&x !!set {…}`) is
/// left unspliced, because two tags on one node are not valid YAML; aliases to
/// it keep the reader's own alias expansion. That residual is named in the
/// graph module's documentation rather than silently mis-linked.
pub fn splice_identity_tags(shadow: &str) -> Result<String, YamlError> {
    let Ok(document) = shadow.parse::<Document>() else {
        // Let the semantic reader report the syntax error it already reports.
        return Ok(shadow.to_owned());
    };
    let registry = AnchorRegistry::from_document(&document);
    let Some(root) = document_root(&document) else {
        return Ok(shadow.to_owned());
    };
    let mut walk = IdentityWalk {
        text: shadow,
        registry: &registry,
        ordinals: BTreeMap::new(),
        skipped: std::collections::BTreeSet::new(),
        anchored: std::collections::BTreeSet::new(),
        edits: Vec::new(),
        next_ordinal: 0,
    };
    walk.node(&root);
    if walk.edits.is_empty() {
        return Ok(shadow.to_owned());
    }
    walk.edits.sort_by_key(|(start, end, _)| (*start, *end));
    let mut spliced = String::with_capacity(shadow.len() + walk.edits.len() * 24);
    let mut cursor = 0usize;
    for (start, end, replacement) in &walk.edits {
        if *start < cursor || *end > shadow.len() {
            return Err(YamlError::Syntax(
                "the lossless syntax tree reported overlapping identity spans".to_owned(),
            ));
        }
        spliced.push_str(&shadow[cursor..*start]);
        spliced.push_str(replacement);
        cursor = *end;
    }
    spliced.push_str(&shadow[cursor..]);
    Ok(spliced)
}

/// The document's value node, whichever wrapper the syntax tree puts it in.
fn document_root(document: &Document) -> Option<YamlNode> {
    let node = yaml_edit::AsYaml::as_node(document)?;
    if let Some(value) = YamlNode::from_syntax(node.clone()) {
        return Some(value);
    }
    node.children().find_map(YamlNode::from_syntax)
}

struct IdentityWalk<'a> {
    text: &'a str,
    registry: &'a AnchorRegistry,
    /// Anchor name -> the ordinal the markers carry.
    ordinals: BTreeMap<String, u32>,
    /// Names whose target node cannot carry a second tag.
    skipped: std::collections::BTreeSet<String>,
    /// Ordinals whose target tag has already been spliced.
    anchored: std::collections::BTreeSet<u32>,
    edits: Vec<(usize, usize, String)>,
    next_ordinal: u32,
}

impl IdentityWalk<'_> {
    fn node(&mut self, node: &YamlNode) {
        match node {
            YamlNode::Scalar(_) => {}
            YamlNode::Alias(alias) => self.alias(node, &alias.name()),
            YamlNode::Sequence(sequence) => {
                for item in sequence.values() {
                    self.node(&item);
                }
            }
            YamlNode::Mapping(mapping) => {
                for (key, value) in mapping.iter() {
                    self.node(&key);
                    self.node(&value);
                }
            }
            YamlNode::TaggedNode(tagged) => {
                if let Some(mapping) = tagged.as_mapping() {
                    self.node(&YamlNode::Mapping(mapping));
                } else if let Some(sequence) = tagged.as_sequence() {
                    self.node(&YamlNode::Sequence(sequence));
                }
            }
        }
    }

    fn alias(&mut self, node: &YamlNode, name: &str) {
        if self.skipped.contains(name) {
            return;
        }
        let Some(target) = self.registry.resolve(name) else {
            // An undefined or forward alias: the reader keeps its own refusal.
            return;
        };
        let Some(target_node) = YamlNode::from_syntax(target.clone()) else {
            return;
        };
        if matches!(target_node, YamlNode::TaggedNode(_)) {
            self.skipped.insert(name.to_owned());
            return;
        }
        let ordinal = match self.ordinals.get(name) {
            Some(ordinal) => *ordinal,
            None => {
                let ordinal = self.next_ordinal;
                self.next_ordinal += 1;
                self.ordinals.insert(name.to_owned(), ordinal);
                ordinal
            }
        };
        if self.anchored.insert(ordinal) {
            let start = target_node.byte_range().start as usize;
            let replacement = if self.at_line_start(start) {
                let indent = self.indent_at(start);
                format!("{ANCHOR_TAG_PREFIX}{ordinal}\n{indent}")
            } else {
                format!("{ANCHOR_TAG_PREFIX}{ordinal} ")
            };
            self.edits.push((start, start, replacement));
        }
        let range = node.byte_range();
        self.edits.push((
            range.start as usize,
            range.end as usize,
            // A tag needs a following space: `!t]` is not a tag token.
            format!("{ALIAS_TAG_PREFIX}{ordinal} "),
        ));
    }

    /// Whether only spaces precede `start` on its line.
    fn at_line_start(&self, start: usize) -> bool {
        self.indent_at(start).len() == start - self.line_start(start)
    }

    fn line_start(&self, start: usize) -> usize {
        self.text[..start]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0)
    }

    fn indent_at(&self, start: usize) -> &str {
        let line_start = self.line_start(start);
        let indent: String = self.text[line_start..start]
            .chars()
            .take_while(|character| *character == ' ')
            .collect();
        // The slice must borrow from `self.text`; recompute the length.
        let length = indent.len();
        &self.text[line_start..line_start + length]
    }
}

/// A constructed scalar's canonical spelling, for key-identity comparison.
fn canonical_key(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("null".to_owned()),
        Value::Bool(flag) => Some(format!("bool:{flag}")),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                Some(format!("int:{integer}"))
            } else if let Some(unsigned) = number.as_u64() {
                Some(format!("int:{unsigned}"))
            } else {
                Some(format!("float:{:?}", number.as_f64().map(f64::to_bits)))
            }
        }
        Value::String(text) => Some(format!("str:{text}")),
        Value::Tagged(_) => {
            if let Some(bytes) = super::construct::constructed_bytes(value) {
                Some(format!("bytes:{bytes:?}"))
            } else if let Some(digits) = super::construct::constructed_big_integer(value) {
                Some(format!("int:{digits}"))
            } else if super::construct::is_constructed_timestamp(value) {
                let Value::Tagged(tagged) = value else {
                    return None;
                };
                let text = tagged.value.as_str()?;
                let (is_datetime, rendered) = super::construct::python_timestamp_text(text)?;
                Some(format!(
                    "{}:{rendered}",
                    if is_datetime { "datetime" } else { "date" }
                ))
            } else {
                None
            }
        }
        Value::Sequence(_) | Value::Mapping(_) => None,
    }
}

/// ruamel's `repr`-flavoured key spelling in the duplicate-key message.
fn key_spelling(node: &YamlNode) -> String {
    match node {
        YamlNode::Scalar(scalar) => crate::python_repr::python_repr(&scalar.as_string()),
        _ => "<complex key>".to_owned(),
    }
}

/// The decoded authored text of a scalar, in transport spelling.
///
/// Double-quoted scalars use the exact code-point decoder (surrogates and
/// marker-shaped text become registered transport markers); every other style
/// is decoded by the lossless syntax tree's own unquoting/folding.
fn scalar_payload(
    scalar: &Scalar,
    markers: &mut BTreeMap<String, PythonString>,
) -> Option<(String, Option<(String, PythonString)>)> {
    let token = scalar.value();
    let decoded: String = if token.len() >= 2 && token.starts_with('"') && token.ends_with('"') {
        let codepoints = decode_double_quoted(&token[1..token.len() - 1]).ok()?;
        let has_surrogate = codepoints
            .iter()
            .any(|code| (0xd800..=0xdfff).contains(code));
        let marker_shaped = codepoints.first() == Some(&u32::from(MARKER_START))
            && codepoints.last() == Some(&u32::from(MARKER_END));
        if has_surrogate || marker_shaped {
            let marker = marker_for(&codepoints);
            let exact = PythonString::from_codepoints(&codepoints).ok()?;
            markers.insert(marker.clone(), exact.clone());
            return Some((marker.clone(), Some((marker, exact))));
        }
        codepoints
            .iter()
            .map(|code| char::from_u32(*code).unwrap_or('\u{fffd}'))
            .collect()
    } else {
        scalar.as_string()
    };
    let marker_shaped = decoded.starts_with(MARKER_START) && decoded.ends_with(MARKER_END);
    if marker_shaped {
        let codepoints: Vec<u32> = decoded.chars().map(u32::from).collect();
        let marker = marker_for(&codepoints);
        let exact = PythonString::from_codepoints(&codepoints).ok()?;
        markers.insert(marker.clone(), exact.clone());
        return Some((marker.clone(), Some((marker, exact))));
    }
    Some((decoded, None))
}

/// Quote text as a YAML double-quoted scalar that parses back exactly.
fn quote_scalar_text(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\u{0}'..='\u{1f}' | '\u{7f}' => {
                quoted.push_str(&format!("\\x{:02x}", u32::from(character)));
            }
            MARKER_START => quoted.push_str("\\uFDD0"),
            MARKER_END => quoted.push_str("\\uFDEF"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Whether this key node carries the merge tag rather than the string tag.
fn is_merge_key(node: &YamlNode) -> bool {
    match node {
        YamlNode::Scalar(scalar) => !scalar.is_quoted() && scalar.value() == "<<",
        YamlNode::TaggedNode(tagged) => {
            let tag = tagged.resolved_tag().unwrap_or_default();
            is_merge_tag(&tag) && !is_string_tag(&tag)
        }
        _ => false,
    }
}

/// Whether a resolved tag names the YAML merge type.
///
/// `yaml_edit` keeps the shorthand spelling (`!!merge`) for a bare `!!` tag and
/// expands `%TAG` handles only, so accept both spellings and the verbatim URI.
fn is_merge_tag(tag: &str) -> bool {
    matches!(tag, MERGE_TAG | "!!merge" | "!<tag:yaml.org,2002:merge>")
}

/// Whether a resolved tag names the YAML string type.
fn is_string_tag(tag: &str) -> bool {
    matches!(tag, STRING_TAG | "!!str" | "!<tag:yaml.org,2002:str>")
}

/// Whether an exact string has the transport-marker shape.
pub fn is_marker_text(value: &str) -> bool {
    is_marker(value)
}

/// The injective transport spelling for an exact Python string.
pub fn marker_for_exact(value: &PythonString) -> String {
    marker_for(value.codepoints())
}

/// The double-quoted scalars that need a marker, with their source spans.
struct LossyTokens {
    markers: BTreeMap<String, PythonString>,
    replacements: Vec<(usize, usize, String)>,
}

impl YamlVisitor for LossyTokens {
    fn visit_scalar(&mut self, scalar: &Scalar) {
        let text = scalar.value();
        let codepoints = if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
            let Ok(codepoints) = decode_double_quoted(&text[1..text.len() - 1]) else {
                // Do not hide invalid escapes behind a valid marker. The
                // semantic reader will reject the original invalid token.
                return;
            };
            codepoints
        } else {
            // Single-quoted, plain and block scalars cannot contain actual
            // surrogate units, but can author a marker-shaped string. Reuse
            // the existing semantic reader for their scalar decoding.
            let Ok(serde_yaml_ng::Value::String(decoded)) =
                serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
            else {
                return;
            };
            if !is_marker(&decoded) {
                return;
            }
            decoded.chars().map(u32::from).collect()
        };
        let has_surrogate = codepoints
            .iter()
            .any(|code| (0xd800..=0xdfff).contains(code));
        let marker_shaped = codepoints.first() == Some(&u32::from(MARKER_START))
            && codepoints.last() == Some(&u32::from(MARKER_END));
        if !has_surrogate && !marker_shaped {
            return;
        }
        let marker = marker_for(&codepoints);
        let exact = match PythonString::from_codepoints(&codepoints) {
            Ok(exact) => exact,
            Err(error) => {
                // Unreachable: the decoder only emits code points.
                debug_assert!(false, "{error}");
                return;
            }
        };
        let range = scalar.byte_range();
        let mut replacement = marker_token(&marker);
        // A block scalar span owns its structural final line break. Keep that
        // separator when replacing it with a flow scalar, or the next mapping
        // key would be concatenated onto the marker's closing quote.
        if text.ends_with("\r\n") {
            replacement.push_str("\r\n");
        } else if text.ends_with('\n') {
            replacement.push('\n');
        } else if text.ends_with('\r') {
            replacement.push('\r');
        }
        self.replacements
            .push((range.start as usize, range.end as usize, replacement));
        self.markers.insert(marker, exact);
    }
}

/// Decode a double-quoted scalar body into exact Python code points.
///
/// This is YAML 1.2's double-quoted rule as the reference reader applies it:
/// the named escapes, `\xNN`/`\uNNNN`/`\UXXXXXXXX`, escaped line
/// breaks, line folding and indentation stripping of continuation lines. It
/// deliberately does **not** pair surrogate escapes, does preserve `U+0000` and
/// private-use code points, and refuses an unknown or truncated escape (the
/// reference's scanner does the same) rather than falling back to literal text.
pub fn decode_double_quoted(body: &str) -> Result<Vec<u32>, YamlError> {
    let characters: Vec<char> = body.chars().collect();
    let mut index = 0usize;
    let mut out: Vec<u32> = Vec::with_capacity(characters.len());
    // Unescaped white space is dropped when a line break follows it.
    let mut pending_white = String::new();
    while index < characters.len() {
        let character = characters[index];
        index += 1;
        if character == '\n' || character == '\r' {
            if character == '\r' && characters.get(index) == Some(&'\n') {
                index += 1;
            }
            pending_white.clear();
            let empty_lines = skip_line_prefixes(&characters, &mut index);
            if empty_lines == 0 {
                out.push(u32::from(' '));
            } else {
                out.extend(std::iter::repeat_n(u32::from('\n'), empty_lines));
            }
            continue;
        }
        if character == ' ' || character == '\t' {
            pending_white.push(character);
            continue;
        }
        out.extend(pending_white.chars().map(u32::from));
        pending_white.clear();
        if character != '\\' {
            out.push(u32::from(character));
            continue;
        }
        let Some(&escaped) = characters.get(index) else {
            return Err(YamlError::Decode(
                "a quoted scalar ends with a lone backslash".to_owned(),
            ));
        };
        index += 1;
        match escaped {
            'n' => out.push(0x0a),
            't' | '\t' => out.push(0x09),
            'r' => out.push(0x0d),
            'b' => out.push(0x08),
            'f' => out.push(0x0c),
            'a' => out.push(0x07),
            'e' => out.push(0x1b),
            'v' => out.push(0x0b),
            '0' => out.push(0x00),
            '\\' => out.push(u32::from('\\')),
            '"' => out.push(u32::from('"')),
            '/' => out.push(u32::from('/')),
            'N' => out.push(0x85),
            '_' => out.push(0xa0),
            'L' => out.push(0x2028),
            'P' => out.push(0x2029),
            ' ' => out.push(u32::from(' ')),
            '\n' | '\r' => {
                if escaped == '\r' && characters.get(index) == Some(&'\n') {
                    index += 1;
                }
                let empty_lines = skip_line_prefixes(&characters, &mut index);
                out.extend(std::iter::repeat_n(u32::from('\n'), empty_lines));
            }
            'x' => out.push(read_escape(&characters, &mut index, 'x', 2)?),
            'u' => out.push(read_escape(&characters, &mut index, 'u', 4)?),
            'U' => out.push(read_escape(&characters, &mut index, 'U', 8)?),
            other => {
                return Err(YamlError::Decode(format!(
                    "unknown escape sequence \\{other}"
                )));
            }
        }
    }
    out.extend(pending_white.chars().map(u32::from));
    Ok(out)
}

/// Skip the white space and line breaks that continue a folded scalar.
fn skip_line_prefixes(characters: &[char], index: &mut usize) -> usize {
    let mut empty_lines = 0usize;
    loop {
        while matches!(characters.get(*index), Some(' ') | Some('\t')) {
            *index += 1;
        }
        match characters.get(*index) {
            Some('\r') => {
                *index += 1;
                if characters.get(*index) == Some(&'\n') {
                    *index += 1;
                }
                empty_lines += 1;
            }
            Some('\n') => {
                *index += 1;
                empty_lines += 1;
            }
            _ => return empty_lines,
        }
    }
}

/// Read `\xNN`, `\uNNNN` or `\UNNNNNNNN` after the prefix was consumed.
fn read_escape(
    characters: &[char],
    index: &mut usize,
    prefix: char,
    width: usize,
) -> Result<u32, YamlError> {
    let mut digits = String::with_capacity(width);
    for _ in 0..width {
        let Some(&character) = characters.get(*index) else {
            return Err(YamlError::Decode(format!("truncated \\{prefix} escape")));
        };
        if !character.is_ascii_hexdigit() {
            return Err(YamlError::Decode(format!(
                "invalid hex digit {character:?} in \\{prefix} escape"
            )));
        }
        digits.push(character);
        *index += 1;
    }
    let code = u32::from_str_radix(&digits, 16).map_err(|error| {
        YamlError::Decode(format!("invalid \\{prefix}{digits} escape: {error}"))
    })?;
    if code > 0x10ffff {
        return Err(YamlError::Decode(format!(
            "\\{prefix}{digits} is beyond the Unicode range"
        )));
    }
    Ok(code)
}

/// Check the transport invariant after semantic parsing.
///
/// Marker-shaped authored strings were themselves encoded, so every marker the
/// semantic reader sees must be registered. This is not a reserved input class.
pub fn verify_markers(
    value: &serde_yaml_ng::Value,
    markers: &BTreeMap<String, PythonString>,
) -> Result<(), YamlError> {
    match value {
        serde_yaml_ng::Value::String(text) => {
            // The merge-key rewrite is the transport's own marker and is
            // resolved immediately after this check; an authored marker-shaped
            // string was itself transported, so it can never spell it.
            if is_marker(text) && !is_merge_marker(text) && !markers.contains_key(text) {
                return Err(YamlError::ForeignMarker(text.clone()));
            }
        }
        serde_yaml_ng::Value::Sequence(items) => {
            for item in items {
                verify_markers(item, markers)?;
            }
        }
        serde_yaml_ng::Value::Mapping(mapping) => {
            for (key, item) in mapping {
                verify_markers(key, markers)?;
                verify_markers(item, markers)?;
            }
        }
        serde_yaml_ng::Value::Tagged(tagged) => verify_markers(&tagged.value, markers)?,
        serde_yaml_ng::Value::Null
        | serde_yaml_ng::Value::Bool(_)
        | serde_yaml_ng::Value::Number(_) => {}
    }
    Ok(())
}

/// Resolve YAML merge keys on an already-parsed document.
///
/// The reference readers flatten merge keys while constructing the mapping.
/// `serde_yaml_ng`'s self-describing `Value` keeps them as ordinary keys, and
/// the transport above has already rewritten the *genuine* merge keys to
/// a merge-key marker so a quoted literal `'<<'` stays data. This applies the
/// reference's exact rules:
///
/// * a mapping with more than one merge key is a `DuplicateKeyError` under
///   ruamel (`dialect.refuses_duplicate_keys()`); PyYAML applies every merge
///   group in authored order instead;
/// * a merge value must be a mapping or a sequence of mappings, otherwise the
///   constructor raises (it is never silently discarded);
/// * for `<<: [*a, *b]`, the sequence is reversed before concatenation, so `a`
///   wins over `b`, and a key's order is the first occurrence (from `b`'s key
///   order, then any `a`-only keys);
/// * an explicitly authored key always wins over a merged one and keeps the
///   merge key's position; and
/// * the merged keys precede the explicit ones, which is the reference's
///   `merge + node.value` order.
pub fn resolve_merge_keys(value: &mut Value, dialect: Dialect) -> Result<(), YamlError> {
    let anchors = collect_graph_anchors(value);
    let mut expanding: Vec<u32> = Vec::new();
    resolve_merge_keys_inner(value, dialect, &anchors, &mut expanding)
}

/// Every anchored node the identity splice marked, by its ordinal.
fn collect_graph_anchors(value: &Value) -> BTreeMap<u32, Value> {
    let mut anchors = BTreeMap::new();
    collect_graph_anchors_into(value, &mut anchors);
    anchors
}

fn collect_graph_anchors_into(value: &Value, anchors: &mut BTreeMap<u32, Value>) {
    match value {
        Value::Tagged(tagged) => {
            let tag = tagged.tag.to_string();
            if let Some(ordinal) = tag
                .strip_prefix(ANCHOR_TAG_PREFIX)
                .and_then(|ordinal| ordinal.parse::<u32>().ok())
            {
                anchors.entry(ordinal).or_insert_with(|| value.clone());
            }
            collect_graph_anchors_into(&tagged.value, anchors);
        }
        Value::Sequence(items) => {
            for item in items {
                collect_graph_anchors_into(item, anchors);
            }
        }
        Value::Mapping(mapping) => {
            for (key, value) in mapping.iter() {
                collect_graph_anchors_into(key, anchors);
                collect_graph_anchors_into(value, anchors);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Resolve one merge operand through the identity splice.
///
/// `<<: *defaults` reaches here as an alias marker: the operand is the anchored
/// mapping the marker names, with the anchor marker stripped. A merge that
/// reaches itself is the reference's `found unconstructable recursive node`
/// instead of an unbounded walk.
fn merge_operand(
    value: Value,
    anchors: &BTreeMap<u32, Value>,
    expanding: &mut Vec<u32>,
) -> Result<Value, YamlError> {
    let Value::Tagged(tagged) = &value else {
        return Ok(value);
    };
    let tag = tagged.tag.to_string();
    if let Some(ordinal) = tag.strip_prefix(ALIAS_TAG_PREFIX) {
        let ordinal = ordinal
            .parse::<u32>()
            .map_err(|_| YamlError::Merge("an alias marker lost its anchor ordinal".to_owned()))?;
        if expanding.contains(&ordinal) {
            return Err(YamlError::Merge(
                "found unconstructable recursive node".to_owned(),
            ));
        }
        let Some(target) = anchors.get(&ordinal) else {
            return Err(YamlError::Merge(format!(
                "found undefined alias (merge link {ordinal} has no anchor)"
            )));
        };
        expanding.push(ordinal);
        let resolved = merge_operand(target.clone(), anchors, expanding);
        expanding.pop();
        return resolved;
    }
    if tag.starts_with(ANCHOR_TAG_PREFIX) {
        return Ok(tagged.value.clone());
    }
    Ok(value)
}

fn resolve_merge_keys_inner(
    value: &mut Value,
    dialect: Dialect,
    anchors: &BTreeMap<u32, Value>,
    expanding: &mut Vec<u32>,
) -> Result<(), YamlError> {
    match value {
        Value::Sequence(items) => {
            for item in items {
                resolve_merge_keys_inner(item, dialect, anchors, expanding)?;
            }
            Ok(())
        }
        Value::Mapping(mapping) => {
            let mut merge_groups: Vec<Mapping> = Vec::new();
            let mut explicit: Vec<(Value, Value)> = Vec::new();
            let mut seen_merge = false;
            let entries: Vec<(Value, Value)> = mapping
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            for (key, mut value) in entries {
                resolve_merge_keys_inner(&mut value, dialect, anchors, expanding)?;
                if key.as_str().is_some_and(is_merge_marker) {
                    if seen_merge && dialect.refuses_duplicate_keys() {
                        return Err(YamlError::Merge(
                            "found duplicate merge key \"<<\"".to_owned(),
                        ));
                    }
                    seen_merge = true;
                    match merge_operand(value, anchors, expanding)? {
                        Value::Mapping(source) => merge_groups.push(source),
                        Value::Sequence(sources) => {
                            let mut group: Vec<Mapping> = Vec::with_capacity(sources.len());
                            for source in sources {
                                match merge_operand(source, anchors, expanding)? {
                                    Value::Mapping(source) => group.push(source),
                                    other => {
                                        return Err(YamlError::Merge(format!(
                                            "expected a mapping for merging, but found {}",
                                            node_kind(&other)
                                        )));
                                    }
                                }
                            }
                            group.reverse();
                            merge_groups.extend(group);
                        }
                        other => {
                            return Err(YamlError::Merge(format!(
                                "expected a mapping or list of mappings for merging, but found {}",
                                node_kind(&other)
                            )));
                        }
                    }
                    continue;
                }
                explicit.push((key, value));
            }
            let _ = seen_merge;
            let mut resolved = Mapping::new();
            for group in &merge_groups {
                for (key, value) in group {
                    resolved.insert(key.clone(), value.clone());
                }
            }
            for (key, value) in explicit {
                resolved.insert(key, value);
            }
            *mapping = resolved;
            Ok(())
        }
        Value::Tagged(tagged) => {
            resolve_merge_keys_inner(&mut tagged.value, dialect, anchors, expanding)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

/// The reference constructor's node-id wording for a refused merge value.
fn node_kind(value: &Value) -> &'static str {
    match value {
        Value::Mapping(_) => "mapping",
        Value::Sequence(_) => "sequence",
        Value::Null => "null",
        Value::Bool(_) | Value::Number(_) | Value::String(_) => "scalar",
        Value::Tagged(_) => "tagged",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(text: &str) -> Vec<u32> {
        decode_double_quoted(text).unwrap()
    }

    #[test]
    fn named_escapes_and_folding_match_the_reference() {
        assert_eq!(codes("plain"), vec![112, 108, 97, 105, 110]);
        assert_eq!(
            codes("\\n\\t\\r\\b\\f\\a\\e\\v\\0"),
            vec![0x0a, 0x09, 0x0d, 0x08, 0x0c, 0x07, 0x1b, 0x0b, 0x00]
        );
        assert_eq!(codes("\\\\"), vec![u32::from('\\')]);
        assert_eq!(codes("a\\ b"), vec![97, 32, 98]);
        assert_eq!(codes("\\N\\_\\L\\P"), vec![0x85, 0xa0, 0x2028, 0x2029]);
        assert_eq!(codes("one\n  two"), vec![111, 110, 101, 32, 116, 119, 111]);
        assert_eq!(
            codes("one\n\n  two"),
            vec![111, 110, 101, 10, 116, 119, 111]
        );
        assert_eq!(
            codes("esc\\\n  break"),
            vec![101, 115, 99, 98, 114, 101, 97, 107]
        );
    }

    #[test]
    fn escapes_are_exact_and_never_paired() {
        assert_eq!(codes("\\u0041"), vec![0x41]);
        assert_eq!(codes("\\x41"), vec![0x41]);
        assert_eq!(codes("\\U0001F600"), vec![0x1f600]);
        assert_eq!(codes("\\ud800"), vec![0xd800]);
        assert_eq!(codes("\\uD83D\\uDE00"), vec![0xd83d, 0xde00]);
        assert_eq!(codes("\\uDC00\\uD800"), vec![0xdc00, 0xd800]);
        assert_eq!(codes("\\uE000"), vec![0xe000]);
        assert_eq!(codes("nul-\\u0000-x"), vec![110, 117, 108, 45, 0, 45, 120]);
    }

    #[test]
    fn unknown_and_truncated_escapes_are_refused() {
        assert!(decode_double_quoted("\\q").is_err());
        assert!(decode_double_quoted("\\'").is_err());
        assert!(decode_double_quoted("\\u12").is_err());
        assert!(decode_double_quoted("\\uZZZZ").is_err());
        assert!(decode_double_quoted("\\U00110000").is_err());
        assert!(decode_double_quoted("trailing\\").is_err());
    }

    #[test]
    fn markers_are_injective_and_round_trip() {
        let one = PythonString::from_codepoints(&[0x61, 0xd800]).unwrap();
        let same = PythonString::from_codepoints(&[0x61, 0xd800]).unwrap();
        let other = PythonString::from_codepoints(&[0x61, 0x0]).unwrap();
        let marker = marker_for(one.codepoints());
        assert_eq!(marker, marker_for(same.codepoints()));
        assert_ne!(marker, marker_for(other.codepoints()));
        assert!(is_marker(&marker));
        // The wire token parses back to the exact marker string.
        let decoded = codes(&marker_token(&marker)[1..marker_token(&marker).len() - 1]);
        assert_eq!(decoded, marker.chars().map(u32::from).collect::<Vec<_>>());
    }

    #[test]
    fn transport_carries_only_lossy_double_quoted_tokens() {
        let text = concat!(
            "plain: plain-\\ud800-text\n",
            "single: 'sq-\\ud800'\n",
            "block: |-\n  block-\\ud800\n",
            "quoted: \"author\\ud800\"\n",
            "astral: \"big-\\U0001F600\"\n",
        );
        let lossless = transport(text, Dialect::RuamelSafe12).unwrap();
        assert_eq!(lossless.markers().len(), 1);
        let exact = lossless
            .markers()
            .values()
            .next()
            .expect("the quoted token is carried");
        assert_eq!(exact.codepoints(), &[97, 117, 116, 104, 111, 114, 0xd800]);
        // The shadow keeps every other token verbatim and is parseable.
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(lossless.shadow()).unwrap();
        assert_eq!(
            value["plain"].as_str(),
            Some("plain-\\ud800-text"),
            "a plain scalar keeps the backslash literally"
        );
        assert_eq!(value["single"].as_str(), Some("sq-\\ud800"));
        assert_eq!(value["block"].as_str(), Some("block-\\ud800"));
        assert_eq!(value["astral"].as_str(), Some("big-\u{1f600}"));
        verify_markers(&value, lossless.markers()).unwrap();
    }

    #[test]
    fn anchors_and_merge_keys_survive_the_transport() {
        let text = concat!(
            "name: &shared \"shared\\ud800\"\n",
            "description: *shared\n",
            "base: &ref {code: \"tac-\\ud800\"}\n",
            "refs:\n",
            "  - <<: *ref\n",
            "  - code: tac-2\n",
        );
        let lossless = transport(text, Dialect::RuamelSafe12).unwrap();
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(lossless.shadow()).unwrap();
        let mut value = value;
        resolve_merge_keys(&mut value, Dialect::RuamelSafe12).unwrap();
        let marker = value["name"].as_str().unwrap();
        assert_eq!(
            lossless.markers().get(marker).map(PythonString::codepoints),
            Some(&[115u32, 104, 97, 114, 101, 100, 0xd800][..])
        );
        assert_eq!(value["description"].as_str(), Some(marker));
        let merged_code = value["refs"][0]["code"].as_str().unwrap();
        assert!(lossless.markers().get(merged_code).is_some());
        assert_eq!(value["refs"][1]["code"].as_str(), Some("tac-2"));
    }

    #[test]
    fn an_authored_marker_is_preserved_as_distinct_data() {
        let text = "name: \"\\uFDD0d800\\uFDEF\"\ndescription: \"\\ud800\"\n";
        let lossless = transport(text, Dialect::RuamelSafe12).unwrap();
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(lossless.shadow()).unwrap();
        verify_markers(&value, lossless.markers()).unwrap();
        let name = value["name"].as_str().unwrap();
        let description = value["description"].as_str().unwrap();
        assert_ne!(name, description);
        assert_eq!(
            lossless.markers()[name].codepoints(),
            &[0xfdd0, 0x64, 0x38, 0x30, 0x30, 0xfdef]
        );
        assert_eq!(lossless.markers()[description].codepoints(), &[0xd800]);
    }
}
