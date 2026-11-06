//! Canonical YAML serialization — the native port of the pinned
//! `charter.offering.yaml_utils.canonical_yaml` writer.
//!
//! Source contract (`charter/offering/yaml_utils.py`, `canonical_yaml`):
//!
//! ```python
//! yaml = YAML()                      # ruamel round-trip, default settings
//! yaml.default_flow_style = False
//! yaml.explicit_start = False
//! sorted_body = _sort_keys(dict(body))       # keys sorted at every level
//! buf = io.BytesIO(); yaml.dump(sorted_body, buf)
//! ```
//!
//! The bytes are *load-bearing*: the charter synthesis manifest self-hash,
//! the pack manifest hash, the v2 sidecar/repair hashes and the synthesis
//! `artifact_content_hash` are all `SHA-256(canonical_yaml(payload))`. A
//! native writer that merely "produces valid YAML" therefore cannot be
//! substituted — the Source verifier recomputes the digest from the exact
//! bytes, so this module reproduces ruamel's `RoundTripEmitter` output for
//! the scalar/collection shapes the charter payloads use:
//!
//! * block mappings with sorted keys, two-space map/sequence indent and
//!   *indentless* block sequences (a `-` sits under its key);
//! * plain / single-quoted / double-quoted scalar selection, including
//!   ruamel's YAML-1.2 resolver gate (`"123"`, `"true"`, `"null"`, dates and
//!   `0x…` are quoted; `yes`/`on` stay plain in 1.2);
//! * the emitter's 80-column wrapping for plain and quoted scalars;
//! * `[]` / `{}` flow style for empty collections and `key:` (empty) for a
//!   nested null.
//!
//! This module is a *writer*, not a second reader authority: callers parse
//! with the existing YAML reader (or the round-trip CST editor) and hand the
//! value tree here only when Source itself calls `canonical_yaml`.
//!
//! No aliases/anchors are ever produced (the input is pure data), tags are
//! never emitted for the supported value classes (every scalar's own tag is
//! the one implicit in its plain form), and the encoding is UTF-8 without a
//! BOM — all three match the Source function's documented rules.

use std::fmt::Write as _;

use serde_yaml_ng::Value;
use sk_kernel::PythonString;

/// Serialize `body` to the canonical YAML byte form used by every charter
/// hash in the pinned Source.
///
/// The top-level value is normally a mapping (Source does `dict(body)`);
/// scalars and sequences are accepted too so callers can hash sub-payloads
/// without a second writer.
pub fn canonical_yaml(body: &Value) -> Vec<u8> {
    let mut emitter = Emitter::default();
    emitter.emit_document(body);
    emitter.out.into_bytes()
}

/// Serialize `body` in AUTHORED key order with the same emitter.
///
/// The pinned Source writes two different YAML documents: hashed payloads
/// (`canonical_yaml`, keys sorted at every level) and *authored* documents a
/// model dumps in field order (`charter.yaml`'s composed bundle, a rewritten
/// `config.yaml`). Both go through one emitter so quoting, indentation and
/// line-wrapping rules cannot drift between them; only the key order differs.
pub fn ordered_yaml(body: &Value) -> Vec<u8> {
    ordered_yaml_width(body, BEST_WIDTH)
}

/// Serialize authored string-list fields without projecting their values to UTF-8.
///
/// This is the exact input surface for activation stems. Mapping/sequence layout,
/// scalar style, escaping and folding all use the same emitter as [`ordered_yaml`].
/// Keys are scalar field names; values retain every Python code point, including
/// lone surrogates. This does not perform ledger admission or publish a file.
pub fn ordered_yaml_string_lists(fields: &[(String, Vec<PythonString>)]) -> Vec<u8> {
    let mut emitter = Emitter {
        preserve_order: true,
        root_context: true,
        ..Emitter::default()
    };
    if fields.is_empty() {
        emitter.expect_flow_mapping();
    } else {
        emitter.expect_mapping_entries(fields, |emitter, (key, items)| {
            emitter.expect_block_mapping_key(&Value::String(key.clone()));
            emitter.write_indicator(":", false, false, false);
            emitter.root_context = false;
            emitter.sequence_context = false;
            emitter.mapping_context = true;
            emitter.simple_key_context = false;
            if items.is_empty() {
                emitter.expect_flow_sequence();
            } else {
                emitter.expect_block_sequence(items, |emitter, text| {
                    emitter.root_context = false;
                    emitter.sequence_context = true;
                    emitter.mapping_context = false;
                    emitter.simple_key_context = false;
                    emitter.increase_indent(true, None, false);
                    match text.to_scalar_string() {
                        Ok(text) => emitter.process_scalar(&Value::String(text)),
                        Err(_) => {
                            // A surrogate makes Source's scalar analysis select
                            // double quotes. Keep the original units as data.
                            emitter.write_indent();
                            emitter.write_double_quoted(text.codepoints(), true);
                        }
                    }
                    emitter.indent = emitter.indents.pop().map(|entry| entry.0).unwrap_or(None);
                });
            }
        });
    }
    emitter.write_indent();
    emitter.out.into_bytes()
}

/// [`ordered_yaml`] with an explicit folding width.
///
/// The pinned Source writes two round-trip configurations: ruamel's default
/// (`width` unset, 80 columns) and the charter-pack cutover's
/// `YAML(); yaml.preserve_quotes = True; yaml.width = 4096`, whose dumps must not
/// be folded at 80. One emitter, one width parameter — never a second writer.
pub fn ordered_yaml_width(body: &Value, width: usize) -> Vec<u8> {
    let mut emitter = Emitter {
        preserve_order: true,
        best_width: width,
        ..Emitter::default()
    };
    emitter.emit_document(body);
    emitter.out.into_bytes()
}

/// Ordered YAML plus Source kernel's guarded trailing-whitespace normalization.
///
/// This is the execution-writer flavour used by retrospective/review records.
/// Existing hash and lossless rendering APIs are deliberately unchanged.
/// Spaces, tabs and CR at line ends are removed only when both byte streams
/// re-parse to equal values; an uncertain safety check retains the original.
pub fn ordered_yaml_width_normalized(body: &Value, width: usize) -> Vec<u8> {
    let original = ordered_yaml_width(body, width);
    let mut normalized = Vec::with_capacity(original.len());
    for (index, line) in original.split(|byte| *byte == b'\n').enumerate() {
        if index != 0 {
            normalized.push(b'\n');
        }
        let end = line
            .iter()
            .rposition(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
            .map_or(0, |index| index + 1);
        normalized.extend_from_slice(&line[..end]);
    }
    if normalized == original {
        return original;
    }
    match (
        serde_yaml_ng::from_slice::<Value>(&original),
        serde_yaml_ng::from_slice::<Value>(&normalized),
    ) {
        (Ok(before), Ok(after)) if before == after => normalized,
        _ => original,
    }
}

/// `SHA-256(canonical_yaml(body))` in lowercase hex — the single manifest /
/// sidecar hash recipe (`charter.offering.packs.hashing.hash_manifest_payload`).
pub fn canonical_yaml_sha256(body: &Value) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(canonical_yaml(body));
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Hash a mapping payload after dropping `exclude_keys`, exactly as
/// `hash_manifest_payload(data, exclude_keys=…)` does.
pub fn canonical_yaml_sha256_without(body: &Value, exclude_keys: &[&str]) -> String {
    match body {
        Value::Mapping(map) => {
            let mut filtered = serde_yaml_ng::Mapping::new();
            for (key, value) in map {
                let skip = key
                    .as_str()
                    .is_some_and(|name| exclude_keys.contains(&name));
                if !skip {
                    filtered.insert(key.clone(), value.clone());
                }
            }
            canonical_yaml_sha256(&Value::Mapping(filtered))
        }
        other => canonical_yaml_sha256(other),
    }
}

const REQUESTED_INDENT: usize = 2;
const BEST_WIDTH: usize = 80;
const MAX_SIMPLE_KEY_LENGTH: usize = 128;

/// Scalar style chosen by ruamel's `choose_scalar_style` for a plain value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Style {
    Plain,
    Single,
    Double,
}

/// Scalar analysis result (`ruamel.yaml.emitter.Emitter.analyze_scalar`).
#[derive(Clone, Debug, Default)]
struct Analysis {
    empty: bool,
    multiline: bool,
    allow_block_plain: bool,
    allow_single_quoted: bool,
}

fn is_break(ch: char) -> bool {
    matches!(ch, '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// Port of `analyze_scalar` for the fields the style decision consumes.
fn analyze_scalar(chars: &[char]) -> Analysis {
    if chars.is_empty() {
        return Analysis {
            empty: true,
            multiline: false,
            allow_block_plain: true,
            allow_single_quoted: true,
        };
    }

    let n = chars.len();
    let mut block_indicators = false;
    let mut flow_indicators = false;
    let mut line_breaks = false;
    let mut special_characters = false;

    let mut leading_space = false;
    let mut leading_break = false;
    let mut trailing_space = false;
    let mut trailing_break = false;
    let mut break_space = false;
    let mut space_break = false;

    if chars.len() >= 3 && chars[0] == '.' && chars[1] == '.' && chars[2] == '.' {
        block_indicators = true;
        flow_indicators = true;
    }
    if chars.len() >= 3 && chars[0] == '-' && chars[1] == '-' && chars[2] == '-' {
        block_indicators = true;
        flow_indicators = true;
    }

    let mut preceded_by_whitespace = true;
    let mut followed_by_whitespace = n == 1 || matches!(chars[1], '\0' | ' ' | '\t' | '\r' | '\n');
    let mut previous_space = false;
    let mut previous_break = false;

    for (index, &ch) in chars.iter().enumerate() {
        if index == 0 {
            if matches!(
                ch,
                '#' | ','
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '&'
                    | '*'
                    | '!'
                    | '|'
                    | '>'
                    | '\''
                    | '"'
                    | '%'
                    | '@'
                    | '`'
            ) {
                flow_indicators = true;
                block_indicators = true;
            }
            if matches!(ch, '?' | ':') {
                // YAML 1.2 (`serializer.use_version == (1, 2)`): only a
                // single-character scalar is a flow indicator here.
                if n == 1 {
                    flow_indicators = true;
                }
                if followed_by_whitespace {
                    block_indicators = true;
                }
            }
            if ch == '-' && followed_by_whitespace {
                flow_indicators = true;
                block_indicators = true;
            }
        } else {
            if matches!(ch, ',' | '[' | ']' | '{' | '}') {
                flow_indicators = true;
            }
            if ch == ':' && followed_by_whitespace {
                flow_indicators = true;
                block_indicators = true;
            }
            if ch == '#' && preceded_by_whitespace {
                flow_indicators = true;
                block_indicators = true;
            }
        }

        if is_break(ch) {
            line_breaks = true;
        }
        let unicode_ok = ch == '\u{85}'
            || ('\u{A0}'..='\u{D7FF}').contains(&ch)
            || ('\u{E000}'..='\u{FFFD}').contains(&ch)
            || ('\u{10000}'..='\u{10FFFF}').contains(&ch);
        if !(ch == '\n' || ('\x20'..='\x7E').contains(&ch)) {
            if unicode_ok && ch != '\u{FEFF}' {
                // `allow_unicode` is True for the Source YAML() instance.
            } else {
                special_characters = true;
            }
        }

        if ch == ' ' {
            if index == 0 {
                leading_space = true;
            }
            if index == n - 1 {
                trailing_space = true;
            }
            if previous_break {
                break_space = true;
            }
            previous_space = true;
            previous_break = false;
        } else if is_break(ch) {
            if index == 0 {
                leading_break = true;
            }
            if index == n - 1 {
                trailing_break = true;
            }
            if previous_space {
                space_break = true;
            }
            previous_space = false;
            previous_break = true;
        } else {
            previous_space = false;
            previous_break = false;
        }

        preceded_by_whitespace = matches!(
            ch,
            '\0' | ' ' | '\t' | '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}'
        );
        followed_by_whitespace = index + 2 >= n
            || matches!(
                chars[index + 2],
                '\0' | ' ' | '\t' | '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}'
            );
    }

    let mut allow_flow_plain = true;
    let mut allow_block_plain = true;
    let mut allow_single_quoted = true;

    if leading_space || leading_break || trailing_space || trailing_break {
        allow_flow_plain = false;
        allow_block_plain = false;
    }
    if break_space {
        allow_flow_plain = false;
        allow_block_plain = false;
        allow_single_quoted = false;
    }
    if special_characters || space_break {
        // Both classes have the same consequence in the pinned analyzer: only a
        // double-quoted scalar may carry them.
        allow_flow_plain = false;
        allow_block_plain = false;
        allow_single_quoted = false;
    }
    if line_breaks {
        allow_flow_plain = false;
        allow_block_plain = false;
    }
    if flow_indicators {
        allow_flow_plain = false;
    }
    if block_indicators {
        allow_block_plain = false;
    }
    let _ = allow_flow_plain;

    Analysis {
        empty: false,
        multiline: line_breaks,
        allow_block_plain,
        allow_single_quoted,
    }
}

/// True when the YAML-1.2 resolver maps `value` onto one of its implicit
/// non-string tags (so the round-trip emitter must quote it to keep it a
/// string). Mirrors `Resolver.resolve(ScalarNode, value, (True, False))`.
fn resolves_as_non_string(value: &str) -> bool {
    sk_kernel::python_yaml::implicit_scalar_kind(
        value,
        sk_kernel::python_yaml::Dialect::RuamelSafe12,
    ) != "str"
        // These resolver tags are syntax, not constructed scalar value kinds.
        || matches!(value, "<<" | "=")
}

/// Port of `Emitter.choose_scalar_style` for the supported value classes.
fn choose_style(
    chars: &[char],
    analysis: &Analysis,
    string_implicit: bool,
    simple_key_context: bool,
) -> Style {
    if string_implicit
        && !(simple_key_context && (analysis.empty || analysis.multiline))
        && analysis.allow_block_plain
    {
        return Style::Plain;
    }
    let value: String = chars.iter().collect();
    if value.contains('\'') || value.contains('\n') {
        return Style::Double;
    }
    if analysis.allow_single_quoted && !(simple_key_context && analysis.multiline) {
        return Style::Single;
    }
    Style::Double
}

struct Emitter {
    out: String,
    indent: Option<usize>,
    indents: Vec<(Option<usize>, bool)>,
    column: usize,
    whitespace: bool,
    indention: bool,
    no_newline: Option<bool>,
    root_context: bool,
    sequence_context: bool,
    mapping_context: bool,
    simple_key_context: bool,
    best_sequence_indent: usize,
    /// `yaml.width` in ruamel terms: the column beyond which a plain/quoted
    /// scalar is folded. `BEST_WIDTH` is ruamel's default; the cutover's
    /// round-trip dumps ask for `width=4096`.
    best_width: usize,
    best_map_indent: usize,
    sequence_dash_offset: usize,
    requested_indent: Option<usize>,
    open_ended: bool,
    compact_seq_seq: bool,
    compact_seq_map: bool,
    allow_unicode: bool,
    preserve_order: bool,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            out: String::new(),
            indent: None,
            indents: Vec::new(),
            column: 0,
            whitespace: true,
            indention: true,
            no_newline: None,
            root_context: false,
            sequence_context: false,
            mapping_context: false,
            simple_key_context: false,
            best_sequence_indent: REQUESTED_INDENT,
            best_map_indent: REQUESTED_INDENT,
            sequence_dash_offset: 0,
            requested_indent: Some(REQUESTED_INDENT),
            best_width: BEST_WIDTH,
            open_ended: false,
            compact_seq_seq: true,
            compact_seq_map: true,
            allow_unicode: true,
            preserve_order: false,
        }
    }
}

impl Emitter {
    fn emit_document(&mut self, body: &Value) {
        self.emit_node(body, true, false, false, false);
        // `expect_document_end`: write_indent(); no explicit end marker.
        self.write_indent();
        // `expect_stream_start` end: an open-ended plain root scalar gets the
        // explicit end indicator so a re-read does not swallow a following
        // document.
        if self.open_ended {
            self.write_indicator("...", true, false, false);
            self.write_indent();
        }
    }

    fn emit_node(
        &mut self,
        value: &Value,
        root: bool,
        sequence: bool,
        mapping: bool,
        simple_key: bool,
    ) {
        self.root_context = root;
        self.sequence_context = sequence;
        self.mapping_context = mapping;
        self.simple_key_context = simple_key;
        match value {
            Value::Sequence(items) => {
                if items.is_empty() {
                    self.expect_flow_sequence();
                } else {
                    self.expect_block_sequence(items, |emitter, item| {
                        emitter.emit_node(item, false, true, false, false);
                    });
                }
            }
            Value::Mapping(map) => {
                if map.is_empty() {
                    self.expect_flow_mapping();
                } else {
                    self.expect_block_mapping(map);
                }
            }
            scalar => self.expect_scalar(scalar),
        }
    }

    fn expect_scalar(&mut self, value: &Value) {
        self.increase_indent(true, None, false);
        self.process_scalar(value);
        self.indent = self.indents.pop().map(|entry| entry.0).unwrap_or(None);
    }

    fn expect_flow_sequence(&mut self) {
        // `seq_flow_align` returns 0 for the non-canonical emitter.
        self.write_indicator("[", true, true, false);
        self.increase_indent(true, Some(true), false);
        self.indent = self.indents.pop().map(|entry| entry.0).unwrap_or(None);
        self.write_indicator("]", false, false, false);
        self.write_line_break(None);
    }

    fn expect_flow_mapping(&mut self) {
        self.write_indicator("{", true, true, false);
        self.increase_indent(true, Some(false), false);
        self.indent = self.indents.pop().map(|entry| entry.0).unwrap_or(None);
        self.write_indicator("}", false, false, false);
        self.write_line_break(None);
    }

    fn expect_block_mapping(&mut self, map: &serde_yaml_ng::Mapping) {
        let entries: Vec<(&Value, &Value)> = if self.preserve_order {
            map.iter().collect()
        } else {
            sorted_mapping_entries(map)
        };
        self.expect_mapping_entries(&entries, |emitter, (key, value)| {
            emitter.expect_block_mapping_key(key);
            // `expect_block_mapping_simple_value`
            emitter.write_indicator(":", false, false, false);
            emitter.emit_node(value, false, false, true, false);
        });
    }

    fn expect_mapping_entries<T>(
        &mut self,
        entries: &[T],
        mut emit_entry: impl FnMut(&mut Self, &T),
    ) {
        if !self.mapping_context && !(self.compact_seq_map || self.column == 0) {
            self.write_line_break(None);
        }
        self.increase_indent(false, Some(false), false);
        for entry in entries {
            emit_entry(self, entry);
        }
        self.indent = self.indents.pop().map(|entry| entry.0).unwrap_or(None);
    }

    fn expect_block_mapping_key(&mut self, key: &Value) {
        self.write_indent();
        if self.check_simple_key(key) {
            self.emit_node(key, false, false, true, true);
        } else {
            self.write_indicator("?", true, false, true);
            self.emit_node(key, false, false, true, false);
            self.write_indent();
            self.write_indicator(":", true, false, true);
        }
    }

    fn check_simple_key(&self, key: &Value) -> bool {
        match key {
            Value::String(text) => {
                let chars: Vec<char> = text.chars().collect();
                let analysis = analyze_scalar(&chars);
                chars.len() < MAX_SIMPLE_KEY_LENGTH && !analysis.multiline
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => true,
            Value::Sequence(items) => items.is_empty(),
            Value::Mapping(map) => map.is_empty(),
            Value::Tagged(tagged) => self.check_simple_key(&tagged.value),
        }
    }

    fn expect_block_sequence<T>(&mut self, items: &[T], mut emit_item: impl FnMut(&mut Self, &T)) {
        let indentless = if self.mapping_context {
            !self.indention
        } else {
            if !self.compact_seq_seq && self.column != 0 {
                self.write_line_break(None);
            }
            false
        };
        self.increase_indent(false, Some(true), indentless);
        for item in items {
            let nonl = if self.column == 0 {
                self.no_newline
            } else {
                None
            };
            self.write_indent();
            let ind = " ".repeat(self.sequence_dash_offset);
            self.write_indicator(&format!("{ind}-"), true, false, true);
            if nonl == Some(true) || self.sequence_dash_offset + 2 > self.best_sequence_indent {
                self.no_newline = Some(true);
            }
            emit_item(self, item);
        }
        self.indent = self.indents.pop().map(|entry| entry.0).unwrap_or(None);
        self.no_newline = Some(false);
    }

    fn process_scalar(&mut self, value: &Value) {
        let (text, is_string, string_implicit) = scalar_text(value);
        let chars: Vec<char> = text.chars().collect();
        let analysis = analyze_scalar(&chars);
        let style = if is_string {
            choose_style(&chars, &analysis, string_implicit, self.simple_key_context)
        } else {
            Style::Plain
        };
        let split = !self.simple_key_context;
        if self.sequence_context {
            self.write_indent();
        }
        match style {
            Style::Plain => self.write_plain(&chars, split),
            Style::Single => self.write_single_quoted(&chars, split),
            Style::Double => {
                let points: Vec<u32> = chars.iter().copied().map(u32::from).collect();
                self.write_double_quoted(&points, split);
            }
        }
    }

    fn write_indicator(
        &mut self,
        indicator: &str,
        need_whitespace: bool,
        whitespace: bool,
        indention: bool,
    ) {
        let data = if self.whitespace || !need_whitespace {
            indicator.to_string()
        } else {
            format!(" {indicator}")
        };
        self.whitespace = whitespace;
        self.indention = self.indention && indention;
        self.column += data.chars().count();
        self.open_ended = false;
        self.out.push_str(&data);
    }

    fn write_indent(&mut self) {
        let indent = self.indent.unwrap_or(0);
        if !self.indention || self.column > indent || (self.column == indent && !self.whitespace) {
            if self.no_newline.take().unwrap_or(false) {
                // A pending "no newline" request consumes the break.
            } else {
                self.write_line_break(None);
            }
        }
        if self.column < indent {
            self.whitespace = true;
            let pad = indent - self.column;
            self.column = indent;
            self.out.push_str(&" ".repeat(pad));
        }
    }

    fn write_line_break(&mut self, data: Option<char>) {
        self.whitespace = true;
        self.indention = true;
        self.column = 0;
        self.out.push(data.unwrap_or('\n'));
    }

    fn increase_indent(&mut self, flow: bool, sequence: Option<bool>, indentless: bool) {
        self.indents.push((self.indent, sequence.unwrap_or(false)));
        if self.indent.is_none() {
            self.indent = Some(if flow {
                self.requested_indent.unwrap_or(REQUESTED_INDENT)
            } else {
                0
            });
        } else if !indentless {
            let last_sequence = self.indents.last().is_some_and(|entry| entry.1);
            let step = if last_sequence {
                self.best_sequence_indent
            } else {
                self.best_map_indent
            };
            self.indent = Some(self.indent.unwrap_or(0) + step);
        }
    }

    fn write_plain(&mut self, chars: &[char], split: bool) {
        if self.root_context {
            if self.requested_indent.is_some() {
                self.write_line_break(None);
                if self.requested_indent != Some(0) {
                    self.write_indent();
                }
            } else {
                self.open_ended = true;
            }
        }
        if chars.is_empty() {
            return;
        }
        if !self.whitespace {
            self.column += 1;
            self.out.push(' ');
        }
        self.whitespace = false;
        self.indention = false;
        let n = chars.len();
        let mut spaces = false;
        let mut breaks = false;
        let mut start = 0usize;
        let mut end = 0usize;
        while end <= n {
            let ch = if end < n { Some(chars[end]) } else { None };
            if spaces {
                if ch != Some(' ') {
                    if start + 1 == end && self.column >= self.best_width && split {
                        self.write_indent();
                        self.whitespace = false;
                        self.indention = false;
                    } else {
                        let data: String = chars[start..end].iter().collect();
                        self.column += data.chars().count();
                        self.out.push_str(&data);
                    }
                    start = end;
                }
            } else if breaks {
                if !ch.is_some_and(is_break) {
                    if chars[start] == '\n' {
                        self.write_line_break(None);
                    }
                    for ch in &chars[start..end] {
                        if *ch == '\n' {
                            self.write_line_break(None);
                        } else {
                            self.write_line_break(Some(*ch));
                        }
                    }
                    self.write_indent();
                    self.whitespace = false;
                    self.indention = false;
                    start = end;
                }
            } else if ch.is_none() || ch == Some(' ') || ch.is_some_and(is_break) {
                let data: String = chars[start..end].iter().collect();
                if data.chars().count() + self.column > self.best_width
                    && self.indent.is_some()
                    && self.column > self.indent.unwrap_or(0)
                {
                    self.write_indent();
                }
                self.column += data.chars().count();
                self.out.push_str(&data);
                start = end;
            }
            if let Some(ch) = ch {
                spaces = ch == ' ';
                breaks = is_break(ch);
            }
            end += 1;
        }
    }

    fn write_single_quoted(&mut self, chars: &[char], split: bool) {
        if self.root_context && self.requested_indent.is_some() {
            self.write_line_break(None);
            if self.requested_indent != Some(0) {
                self.write_indent();
            }
        }
        self.write_indicator("'", true, false, false);
        let n = chars.len();
        let mut spaces = false;
        let mut breaks = false;
        let mut start = 0usize;
        let mut end = 0usize;
        while end <= n {
            let ch = if end < n { Some(chars[end]) } else { None };
            if spaces {
                if ch.is_none() || ch != Some(' ') {
                    if start + 1 == end
                        && self.column > self.best_width
                        && split
                        && start != 0
                        && end != n
                    {
                        self.write_indent();
                    } else {
                        let data: String = chars[start..end].iter().collect();
                        self.column += data.chars().count();
                        self.out.push_str(&data);
                    }
                    start = end;
                }
            } else if breaks {
                if ch.is_none() || !ch.is_some_and(is_break) {
                    if chars[start] == '\n' {
                        self.write_line_break(None);
                    }
                    for ch in &chars[start..end] {
                        if *ch == '\n' {
                            self.write_line_break(None);
                        } else {
                            self.write_line_break(Some(*ch));
                        }
                    }
                    self.write_indent();
                    start = end;
                }
            } else if start < end
                && (ch.is_none() || ch.is_some_and(is_break) || ch == Some(' ') || ch == Some('\''))
            {
                let data: String = chars[start..end].iter().collect();
                self.column += data.chars().count();
                self.out.push_str(&data);
                start = end;
            }
            if ch == Some('\'') {
                self.out.push_str("''");
                self.column += 2;
                start = end + 1;
            }
            if let Some(ch) = ch {
                spaces = ch == ' ';
                breaks = is_break(ch);
            }
            end += 1;
        }
        self.write_indicator("'", false, false, false);
    }

    fn write_double_quoted(&mut self, chars: &[u32], split: bool) {
        if self.root_context && self.requested_indent.is_some() {
            self.write_line_break(None);
            if self.requested_indent != Some(0) {
                self.write_indent();
            }
        }
        self.write_indicator("\"", true, false, false);
        let n = chars.len();
        let mut start = 0usize;
        let mut end = 0usize;
        while end <= n {
            let ch = if end < n { Some(chars[end]) } else { None };
            let needs_escape = match ch {
                None => true,
                Some(ch) => {
                    matches!(ch, 0x22 | 0x5C | 0x85 | 0x2028 | 0x2029 | 0xFEFF)
                        || !((0x20..=0x7E).contains(&ch)
                            || (self.allow_unicode
                                && ((0xA0..=0xD7FF).contains(&ch)
                                    || (0xE000..=0xFFFD).contains(&ch)
                                    || (0x10000..=0x10FFFF).contains(&ch))))
                }
            };
            if needs_escape {
                if start < end {
                    let data = unescaped_text(&chars[start..end]);
                    self.column += end - start;
                    self.out.push_str(&data);
                    start = end;
                }
                if let Some(ch) = ch {
                    let data = escape_char(ch);
                    self.column += data.chars().count();
                    self.out.push_str(&data);
                    start = end + 1;
                }
            }
            if 0 < end
                && end < n.saturating_sub(1)
                && (ch == Some(0x20) || start >= end)
                && (self.column as isize + (end as isize - start as isize))
                    > self.best_width as isize
                && split
            {
                let mut need_backslash = true;
                if n > end
                    && let Some(space_offset) = chars[end..].iter().position(|ch| *ch == 0x20)
                {
                    let mut space_pos = end + space_offset;
                    if let Some(newline_offset) =
                        chars[end..space_pos].iter().position(|ch| *ch == 0x0A)
                    {
                        space_pos = end + newline_offset;
                    }
                    let next_is_space = chars.get(space_pos + 1) == Some(&0x20);
                    let next_is_newline = chars.get(space_pos + 1) == Some(&0x0A);
                    let doubled_space = end > 0
                        && chars.get(end - 1) == Some(&0x20)
                        && chars.get(end) == Some(&0x20);
                    // Inspect code points directly: this lookahead can include
                    // a surrogate which will be escaped on a later iteration.
                    let inner = &chars[end..space_pos];
                    if chars[space_pos] == 0x0A && !next_is_space {
                        // Keep the backslash.
                    } else if !inner.contains(&0x22)
                        && !inner.contains(&0x27)
                        && !next_is_space
                        && !next_is_newline
                        && !doubled_space
                        && start != end
                    {
                        need_backslash = false;
                    }
                }
                let mut data = if start < end {
                    unescaped_text(&chars[start..end])
                } else {
                    String::new()
                };
                if need_backslash {
                    data.push('\\');
                }
                if start < end {
                    start = end;
                }
                self.column += data.chars().count();
                self.out.push_str(&data);
                self.write_indent();
                self.whitespace = false;
                self.indention = false;
                if chars.get(start) == Some(&0x20) {
                    if !need_backslash {
                        start += 1;
                    }
                    if need_backslash {
                        self.out.push('\\');
                        self.column += 1;
                    }
                }
            }
            end += 1;
        }
        self.write_indicator("\"", false, false, false);
    }
}

/// Only runs between escape boundaries. Every surrogate/control unit has
/// already ended the run, so conversion here cannot reject caller data.
fn unescaped_text(points: &[u32]) -> String {
    points
        .iter()
        .map(|point| char::from_u32(*point).expect("unescaped YAML run contains only scalar units"))
        .collect()
}

fn escape_char(code: u32) -> String {
    let replacement = match code {
        0 => Some('0'),
        0x07 => Some('a'),
        0x08 => Some('b'),
        0x09 => Some('t'),
        0x0A => Some('n'),
        0x0B => Some('v'),
        0x0C => Some('f'),
        0x0D => Some('r'),
        0x1B => Some('e'),
        0x22 => Some('"'),
        0x5C => Some('\\'),
        0x85 => Some('N'),
        0xA0 => Some('_'),
        0x2028 => Some('L'),
        0x2029 => Some('P'),
        _ => None,
    };
    if let Some(replacement) = replacement {
        return format!("\\{replacement}");
    }
    if code <= 0xFF {
        format!("\\x{code:02X}")
    } else if code <= 0xFFFF {
        format!("\\u{code:04X}")
    } else {
        format!("\\U{code:08X}")
    }
}

/// The scalar text and whether the value's own tag is `!!str` (so the
/// resolver's plain implicity applies). Non-string scalars are always plain.
fn scalar_text(value: &Value) -> (String, bool, bool) {
    match value {
        Value::Null => (String::new(), false, false),
        Value::Bool(flag) => (
            if *flag { "true" } else { "false" }.to_string(),
            false,
            false,
        ),
        Value::Number(number) => (number_text(number), false, false),
        Value::String(text) => {
            let implicit = !resolves_as_non_string(text);
            (text.clone(), true, implicit)
        }
        Value::Tagged(tagged) => scalar_text(&tagged.value),
        Value::Sequence(_) | Value::Mapping(_) => (String::new(), false, false),
    }
}

fn number_text(number: &serde_yaml_ng::Number) -> String {
    if let Some(value) = number.as_i64() {
        return value.to_string();
    }
    if let Some(value) = number.as_u64() {
        return value.to_string();
    }
    let value = number.as_f64().unwrap_or(0.0);
    python_float_repr(value)
}

/// Python `repr(float)`-compatible formatting for finite/NaN/inf values.
fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return ".nan".to_string();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-.inf"
        } else {
            ".inf"
        }
        .to_string();
    }
    let mut text = format!("{value}");
    if let Some(index) = text.find(['e', 'E']) {
        let (mantissa, exponent) = text.split_at(index);
        let exponent = &exponent[1..];
        let (sign, digits) = match exponent.chars().next() {
            Some('+') => ('+', &exponent[1..]),
            Some('-') => ('-', &exponent[1..]),
            _ => ('+', exponent),
        };
        let digits = if digits.len() < 2 {
            format!("0{digits}")
        } else {
            digits.to_string()
        };
        text = format!("{mantissa}e{sign}{digits}");
    } else if !text.contains('.') {
        text.push_str(".0");
    }
    text
}

fn sorted_mapping_entries(map: &serde_yaml_ng::Mapping) -> Vec<(&Value, &Value)> {
    let mut entries: Vec<(&Value, &Value)> = map.iter().collect();
    entries.sort_by(|left, right| match (left.0, right.0) {
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (a, b) => a.as_str().unwrap_or("").cmp(b.as_str().unwrap_or("")),
    });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(text: &str) -> String {
        let value: Value = serde_yaml_ng::from_str(text).expect("parse");
        String::from_utf8(canonical_yaml(&value)).expect("utf8")
    }

    #[test]
    fn sorts_keys_at_every_level() {
        let output = dump("b: 1\na:\n  d: 2\n  c: 3\n");
        assert_eq!(output, "a:\n  c: 3\n  d: 2\nb: 1\n");
    }

    #[test]
    fn indentless_block_sequences() {
        let output = dump("a:\n  - 1\n  - 2\n");
        assert_eq!(output, "a:\n- 1\n- 2\n");
    }

    #[test]
    fn quotes_resolver_visible_strings() {
        let output = dump("a: '123'\nb: 'true'\nc: 'null'\nd: '2026-11-04'\ne: yes\n");
        assert_eq!(
            output,
            "a: '123'\nb: 'true'\nc: 'null'\nd: '2026-11-04'\ne: yes\n"
        );
    }

    #[test]
    fn empty_collections_are_flow_and_nested_null_is_empty() {
        let output = dump("a: []\nb: {}\nc:\n");
        assert_eq!(output, "a: []\nb: {}\nc:\n");
    }
}
