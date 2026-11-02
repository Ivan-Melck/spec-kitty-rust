//! Comment- and byte-preserving additive YAML editing.
//!
//! Upstream stamps schema/identity fields into operator-authored files with
//! `ruamel.yaml` round-trip mode: existing keys, values, comments, quoting and
//! line endings survive byte-for-byte, and only missing fields are inserted.
//! `serde_yaml_ng` cannot round-trip comments, so this module never
//! reserializes a document. It edits *lines* and then proves the edit
//! semantically:
//!
//! 1. parse the original document;
//! 2. compute the expected mapping (original + the requested additions);
//! 3. insert/replace only the lines that carry those fields;
//! 4. re-parse and require the new document to equal the expected mapping.
//!
//! Step 4 is what makes the byte-level edit safe: a misplaced insertion, a
//! duplicate key or an unexpected quoting change is a typed refusal, not a
//! corrupt config. Non-mapping roots, non-mapping parents and attempts to
//! replace a mapping/sequence with a scalar are refused for the same reason.
//!
//! The editor is intentionally narrow: top-level mapping entries and one
//! nested mapping level. That is the shape of `.kittify/config.yaml`
//! (`project:`), `.kittify/metadata.yaml` (`spec_kitty:`) and
//! `.kittify/charter/activation.yaml` (a top-level list), the files the
//! initialization and upgrade paths stamp. It is not a general YAML library.

use serde_yaml_ng::{Mapping, Value};

use crate::error::InstallError;

/// A scalar value the editor may insert or replace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Scalar {
    /// An explicit YAML null (`key:`).
    Null,
    /// A boolean.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A string scalar.
    Str(String),
}

impl Scalar {
    fn to_value(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(*value),
            Self::Int(value) => Value::Number((*value).into()),
            Self::Str(value) => Value::String(value.clone()),
        }
    }

    fn rendered(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::Str(value) => render_string(value),
        }
    }
}

/// Where a missing top-level mapping is inserted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertPosition {
    /// After any leading comment/blank header lines, before the first entry.
    Top,
    /// After the last entry in the document.
    Bottom,
}

/// The result of an additive edit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditOutcome {
    /// The new document text (byte-identical outside the inserted/replaced lines).
    pub text: String,
    /// Whether anything changed.
    pub changed: bool,
    /// The fields that were inserted (not those merely confirmed present).
    pub inserted: Vec<String>,
}

/// A nested mapping entry (`schema_capabilities:` and its children).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MappingEntry {
    /// The key under the parent mapping.
    pub key: String,
    /// The children of a nested mapping. Empty means a scalar-bearing key is
    /// not allowed; use [`YamlEditor::insert_scalars`] for scalars.
    pub children: Vec<(String, Scalar)>,
}

/// A comment- and byte-preserving YAML editor over one document.
#[derive(Clone, Debug)]
pub struct YamlEditor {
    path: std::path::PathBuf,
    original: String,
}

impl YamlEditor {
    /// Start editing `text`, attributed to `path` in errors.
    pub fn new(path: impl Into<std::path::PathBuf>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            original: text.into(),
        }
    }

    /// The original text.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Insert scalar fields into a one-level `parent` mapping if absent.
    ///
    /// A present key is never modified: even `key: null` is an authored value.
    pub fn insert_scalars(
        &self,
        parent: &str,
        entries: &[(String, Scalar)],
        position: InsertPosition,
    ) -> Result<EditOutcome, InstallError> {
        let original = parse_mapping(&self.path, &self.original)?;
        let mut expected = original.clone();
        let parent_value = expected
            .entry(Value::String(parent.to_string()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
        if parent_value.is_null() {
            *parent_value = Value::Mapping(Mapping::new());
        } else if !parent_value.is_mapping() {
            return Err(yaml_error(
                &self.path,
                format!("{parent:?} exists but is not a mapping"),
            ));
        }
        let parent_mapping = parent_value.as_mapping_mut().expect("checked mapping");
        let mut inserted = Vec::new();
        for (key, scalar) in entries {
            let key_value = Value::String(key.clone());
            if !parent_mapping.contains_key(&key_value) {
                parent_mapping.insert(key_value, scalar.to_value());
                inserted.push(key.clone());
            }
        }
        if inserted.is_empty() {
            return Ok(EditOutcome {
                text: self.original.clone(),
                changed: false,
                inserted,
            });
        }
        let text = self.insert_scalar_lines(parent, entries, position)?;
        verify(&self.path, &self.original, &text, &expected)?;
        Ok(EditOutcome {
            text,
            changed: true,
            inserted,
        })
    }

    /// Insert a nested mapping (for example `schema_capabilities:`) if absent.
    pub fn insert_mapping(
        &self,
        parent: &str,
        entry: &MappingEntry,
        position: InsertPosition,
    ) -> Result<EditOutcome, InstallError> {
        let original = parse_mapping(&self.path, &self.original)?;
        let mut expected = original.clone();
        let parent_value = expected
            .entry(Value::String(parent.to_string()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
        if parent_value.is_null() {
            *parent_value = Value::Mapping(Mapping::new());
        } else if !parent_value.is_mapping() {
            return Err(yaml_error(
                &self.path,
                format!("{parent:?} exists but is not a mapping"),
            ));
        }
        let parent_mapping = parent_value.as_mapping_mut().expect("checked mapping");
        let key_value = Value::String(entry.key.clone());
        if parent_mapping.contains_key(&key_value) {
            return Ok(EditOutcome {
                text: self.original.clone(),
                changed: false,
                inserted: Vec::new(),
            });
        }
        let mut mapping = Mapping::new();
        for (child, scalar) in &entry.children {
            mapping.insert(Value::String(child.clone()), scalar.to_value());
        }
        parent_mapping.insert(key_value, Value::Mapping(mapping));
        let text = self.insert_nested_lines(parent, entry, position)?;
        verify(&self.path, &self.original, &text, &expected)?;
        Ok(EditOutcome {
            text,
            changed: true,
            inserted: vec![entry.key.clone()],
        })
    }

    /// Replace the scalar values of existing keys, inserting absent ones.
    ///
    /// An existing mapping/sequence value is refused: overwriting it would
    /// destroy authored structure, which this editor never does.
    pub fn upsert_scalars(
        &self,
        parent: &str,
        entries: &[(String, Scalar)],
        position: InsertPosition,
    ) -> Result<EditOutcome, InstallError> {
        let original = parse_mapping(&self.path, &self.original)?;
        let mut expected = original.clone();
        let parent_value = expected
            .entry(Value::String(parent.to_string()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
        if parent_value.is_null() {
            *parent_value = Value::Mapping(Mapping::new());
        } else if !parent_value.is_mapping() {
            return Err(yaml_error(
                &self.path,
                format!("{parent:?} exists but is not a mapping"),
            ));
        }
        let parent_mapping = parent_value.as_mapping_mut().expect("checked mapping");
        let mut inserted = Vec::new();
        for (key, scalar) in entries {
            let key_value = Value::String(key.clone());
            match parent_mapping.get_mut(&key_value) {
                Some(existing) if existing.is_mapping() || existing.is_sequence() => {
                    return Err(yaml_error(
                        &self.path,
                        format!(
                            "{parent}.{key} is structured; refusing to replace it with a scalar"
                        ),
                    ));
                }
                Some(existing) => {
                    let replacement = scalar.to_value();
                    if *existing != replacement {
                        *existing = replacement;
                        inserted.push(key.clone());
                    }
                }
                None => {
                    parent_mapping.insert(key_value, scalar.to_value());
                    inserted.push(key.clone());
                }
            }
        }
        if inserted.is_empty() {
            return Ok(EditOutcome {
                text: self.original.clone(),
                changed: false,
                inserted,
            });
        }
        let text = self.replace_or_insert_lines(parent, entries, position)?;
        verify(&self.path, &self.original, &text, &expected)?;
        Ok(EditOutcome {
            text,
            changed: true,
            inserted,
        })
    }

    fn insert_scalar_lines(
        &self,
        parent: &str,
        entries: &[(String, Scalar)],
        position: InsertPosition,
    ) -> Result<String, InstallError> {
        let document = Lines::parse(&self.path, &self.original)?;
        let keys: Vec<&str> = if parent.is_empty() {
            Vec::new()
        } else {
            vec![parent]
        };
        let block = document.block(&self.path, &keys)?;
        let mut rendered = Vec::new();
        if let Some(parent) = &block.missing_parent {
            rendered.push(format!("{parent}:"));
        }
        for (key, scalar) in entries {
            if block.has_child(key) {
                continue;
            }
            rendered.push(entry_line(block.child_indent(), key, scalar));
        }
        if rendered.is_empty() {
            return Ok(self.original.clone());
        }
        Ok(document.with_lines(&self.path, &block, rendered, position))
    }

    fn insert_nested_lines(
        &self,
        parent: &str,
        entry: &MappingEntry,
        position: InsertPosition,
    ) -> Result<String, InstallError> {
        let document = Lines::parse(&self.path, &self.original)?;
        let keys: Vec<&str> = if parent.is_empty() {
            Vec::new()
        } else {
            vec![parent]
        };
        let block = document.block(&self.path, &keys)?;
        if block.has_child(&entry.key) {
            return Ok(self.original.clone());
        }
        let child_indent = block.child_indent();
        let mut rendered = Vec::new();
        if let Some(parent) = &block.missing_parent {
            rendered.push(format!("{parent}:"));
        }
        rendered.push(format!("{child_indent}{}:", entry.key));
        let nested_indent = format!("{child_indent}  ");
        for (key, scalar) in &entry.children {
            rendered.push(entry_line(&nested_indent, key, scalar));
        }
        Ok(document.with_lines(&self.path, &block, rendered, position))
    }

    fn replace_or_insert_lines(
        &self,
        parent: &str,
        entries: &[(String, Scalar)],
        position: InsertPosition,
    ) -> Result<String, InstallError> {
        let document = Lines::parse(&self.path, &self.original)?;
        let keys: Vec<&str> = if parent.is_empty() {
            Vec::new()
        } else {
            vec![parent]
        };
        let block = document.block(&self.path, &keys)?;
        replace_in_block(&document, &block, entries, position)
    }

    /// Insert a nested mapping at the document root (for example `environment:`).
    pub fn insert_root_mapping(
        &self,
        entry: &MappingEntry,
        position: InsertPosition,
    ) -> Result<EditOutcome, InstallError> {
        let original = parse_mapping(&self.path, &self.original)?;
        let mut expected = original.clone();
        let key_value = Value::String(entry.key.clone());
        if expected.contains_key(&key_value) {
            return Ok(EditOutcome {
                text: self.original.clone(),
                changed: false,
                inserted: Vec::new(),
            });
        }
        let mut mapping = Mapping::new();
        for (child, scalar) in &entry.children {
            mapping.insert(Value::String(child.clone()), scalar.to_value());
        }
        expected.insert(key_value, Value::Mapping(mapping));
        let document = Lines::parse(&self.path, &self.original)?;
        let block = document.block(&self.path, &[])?;
        let child_indent = block.child_indent();
        let mut rendered = vec![format!("{child_indent}{}:", entry.key)];
        let nested_indent = format!("{child_indent}  ");
        for (key, scalar) in &entry.children {
            rendered.push(entry_line(&nested_indent, key, scalar));
        }
        let text = document.with_lines(&self.path, &block, rendered, position);
        verify(&self.path, &self.original, &text, &expected)?;
        Ok(EditOutcome {
            text,
            changed: true,
            inserted: vec![entry.key.clone()],
        })
    }

    /// Insert a string-list entry at the document root if absent.
    pub fn insert_root_list(
        &self,
        key: &str,
        values: &[String],
        position: InsertPosition,
    ) -> Result<EditOutcome, InstallError> {
        let original = parse_mapping(&self.path, &self.original)?;
        let mut expected = original.clone();
        let key_value = Value::String(key.to_string());
        if expected.contains_key(&key_value) {
            return Ok(EditOutcome {
                text: self.original.clone(),
                changed: false,
                inserted: Vec::new(),
            });
        }
        expected.insert(
            key_value,
            Value::Sequence(values.iter().cloned().map(Value::String).collect()),
        );
        let document = Lines::parse(&self.path, &self.original)?;
        let block = document.block(&self.path, &[])?;
        let indent = block.child_indent();
        let mut rendered = vec![format!("{indent}{key}:")];
        for value in values {
            rendered.push(format!(
                "{indent}- {}",
                Scalar::Str(value.clone()).rendered()
            ));
        }
        let text = document.with_lines(&self.path, &block, rendered, position);
        verify(&self.path, &self.original, &text, &expected)?;
        Ok(EditOutcome {
            text,
            changed: true,
            inserted: vec![key.to_string()],
        })
    }

    /// Upsert scalar children of a one-level nested mapping
    /// (`environment` under the root, `identity` under `project`).
    pub fn upsert_nested_scalars(
        &self,
        parent: &str,
        child: &str,
        entries: &[(String, Scalar)],
        position: InsertPosition,
    ) -> Result<EditOutcome, InstallError> {
        let original = parse_mapping(&self.path, &self.original)?;
        let mut expected = original.clone();
        let parent_value = expected
            .entry(Value::String(parent.to_string()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
        if parent_value.is_null() {
            *parent_value = Value::Mapping(Mapping::new());
        } else if !parent_value.is_mapping() {
            return Err(yaml_error(
                &self.path,
                format!("{parent:?} exists but is not a mapping"),
            ));
        }
        let parent_mapping = parent_value.as_mapping_mut().expect("checked mapping");
        let child_value = parent_mapping
            .entry(Value::String(child.to_string()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
        if child_value.is_null() {
            *child_value = Value::Mapping(Mapping::new());
        }
        if !child_value.is_mapping() {
            return Err(yaml_error(
                &self.path,
                format!("{parent}.{child} exists but is not a mapping"),
            ));
        }
        let child_mapping = child_value.as_mapping_mut().expect("checked mapping");
        let mut changed = Vec::new();
        for (key, scalar) in entries {
            let key_value = Value::String(key.clone());
            match child_mapping.get_mut(&key_value) {
                Some(existing) if existing.is_mapping() || existing.is_sequence() => {
                    return Err(yaml_error(
                        &self.path,
                        format!("{parent}.{child}.{key} is structured; refusing to replace it"),
                    ));
                }
                Some(existing) => {
                    let replacement = scalar.to_value();
                    if *existing != replacement {
                        *existing = replacement;
                        changed.push(key.clone());
                    }
                }
                None => {
                    child_mapping.insert(key_value, scalar.to_value());
                    changed.push(key.clone());
                }
            }
        }
        if changed.is_empty() {
            return Ok(EditOutcome {
                text: self.original.clone(),
                changed: false,
                inserted: changed,
            });
        }
        let document = Lines::parse(&self.path, &self.original)?;
        let block = document.block(&self.path, &[parent, child])?;
        let text = replace_in_block(&document, &block, entries, position)?;
        verify(&self.path, &self.original, &text, &expected)?;
        Ok(EditOutcome {
            text,
            changed: true,
            inserted: changed,
        })
    }
}

fn entry_line(indent: &str, key: &str, scalar: &Scalar) -> String {
    let rendered = scalar.rendered();
    if rendered.is_empty() {
        format!("{indent}{key}:")
    } else {
        format!("{indent}{key}: {rendered}")
    }
}

fn replace_in_block(
    document: &Lines,
    block: &Block,
    entries: &[(String, Scalar)],
    position: InsertPosition,
) -> Result<String, InstallError> {
    let mut lines = document.lines.clone();
    let mut rendered = Vec::new();
    if let Some(parent) = &block.missing_parent {
        rendered.push(format!("{parent}:"));
    }
    for (key, scalar) in entries {
        if let Some(index) = block.child_line(key) {
            lines[index].content = replace_scalar(&lines[index].content, &scalar.rendered())?;
        } else {
            rendered.push(entry_line(block.child_indent(), key, scalar));
        }
    }
    if rendered.is_empty() {
        return Ok(Lines {
            bom: document.bom,
            lines,
        }
        .render());
    }
    let document = Lines {
        bom: document.bom,
        lines,
    };
    Ok(document.with_lines(std::path::Path::new("<edited>"), block, rendered, position))
}

/// Parse a document into a mapping (`{}` for an empty document).
pub fn parse_mapping(path: &std::path::Path, text: &str) -> Result<Mapping, InstallError> {
    let trimmed = text.trim_start_matches('\u{feff}');
    if trimmed.trim().is_empty() {
        return Ok(Mapping::new());
    }
    let value: Value = serde_yaml_ng::from_str(trimmed)
        .map_err(|error| yaml_error(path, format!("cannot parse YAML: {error}")))?;
    match value {
        Value::Mapping(mapping) => Ok(mapping),
        Value::Null => Ok(Mapping::new()),
        _ => Err(yaml_error(
            path,
            "document root is not a mapping; refusing to edit it".to_string(),
        )),
    }
}

fn verify(
    path: &std::path::Path,
    original: &str,
    edited: &str,
    expected: &Mapping,
) -> Result<(), InstallError> {
    let actual = parse_mapping(path, edited)?;
    if normalized(&actual) != normalized(expected) {
        return Err(yaml_error(
            path,
            "byte-preserving edit changed the document beyond the requested fields".to_string(),
        ));
    }
    // A no-op edit must not have rewritten the document at all.
    if original == edited {
        return Err(yaml_error(
            path,
            "edit reported a change but produced no bytes".to_string(),
        ));
    }
    Ok(())
}

fn normalized(mapping: &Mapping) -> serde_json::Value {
    let value = Value::Mapping(mapping.clone());
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

fn yaml_error(path: &std::path::Path, detail: String) -> InstallError {
    InstallError::YamlEdit {
        path: path.to_path_buf(),
        detail,
    }
}

fn render_string(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value
            .chars()
            .next()
            .is_some_and(|character| "!&*[]{}#,|>@`\"'%-?:".contains(character))
        || value.starts_with(char::is_whitespace)
        || value.ends_with(char::is_whitespace)
        || value.parse::<i64>().is_ok()
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "null" | "~" | "on" | "off"
        )
        || looks_like_timestamp(value);
    if needs_quotes {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        value.to_string()
    }
}

fn looks_like_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

#[derive(Clone, Debug)]
struct Line {
    content: String,
    newline: String,
}

struct Lines {
    bom: bool,
    lines: Vec<Line>,
}

impl Lines {
    fn parse(path: &std::path::Path, text: &str) -> Result<Self, InstallError> {
        let bom = text.starts_with('\u{feff}');
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut lines = Vec::new();
        let mut start = 0;
        while start < body.len() {
            let rest = &body[start..];
            let (content, newline, consumed) = match rest.find('\n') {
                Some(index) => {
                    let raw = &rest[..index];
                    let content = raw.strip_suffix('\r').unwrap_or(raw);
                    let newline = if raw.ends_with('\r') { "\r\n" } else { "\n" };
                    (content.to_string(), newline.to_string(), index + 1)
                }
                None => (rest.to_string(), String::new(), rest.len()),
            };
            if content.contains('\t') && content.trim_start().starts_with('\t') {
                return Err(yaml_error(
                    path,
                    "tab indentation is not valid YAML; refusing to edit".to_string(),
                ));
            }
            lines.push(Line { content, newline });
            start += consumed;
        }
        Ok(Self { bom, lines })
    }

    fn render(&self) -> String {
        let mut text = String::new();
        if self.bom {
            text.push('\u{feff}');
        }
        for line in &self.lines {
            text.push_str(&line.content);
            text.push_str(&line.newline);
        }
        text
    }

    fn block(&self, path: &std::path::Path, keys: &[&str]) -> Result<Block, InstallError> {
        if keys.is_empty() {
            let insert_at = self.first_entry_index().unwrap_or(self.lines.len());
            return Ok(Block {
                start: insert_at,
                insert_at: self.lines.len(),
                child_indent: String::new(),
                existing_children: self.children(0, self.lines.len(), 0),
                missing_parent: None,
                root: true,
            });
        }
        let mut start = 0;
        let mut indent = 0;
        for (depth, key) in keys.iter().enumerate() {
            let found = (start..self.lines.len()).find(|index| {
                matches!(
                    key_of(&self.lines[*index].content),
                    Some((line_indent, line_key))
                        if line_indent == indent && line_key == *key
                )
            });
            match found {
                Some(index) => {
                    if depth + 1 == keys.len() {
                        let end = self.block_end(index, indent);
                        return Ok(Block {
                            start: index,
                            insert_at: self.last_content(start, index, end) + 1,
                            child_indent: " ".repeat(indent + 2),
                            existing_children: self.children(index, end, indent + 2),
                            missing_parent: None,
                            root: false,
                        });
                    }
                    start = index + 1;
                    indent += 2;
                }
                None => {
                    // A missing parent: insert a fresh top-level block.
                    if depth != 0 {
                        return Err(yaml_error(
                            path,
                            format!("parent path {keys:?} is not a contiguous mapping"),
                        ));
                    }
                    let insert_at = self.first_entry_index().unwrap_or(self.lines.len());
                    return Ok(Block {
                        start: insert_at,
                        insert_at,
                        child_indent: "  ".to_string(),
                        existing_children: Vec::new(),
                        missing_parent: Some((*key).to_string()),
                        root: false,
                    });
                }
            }
        }
        Err(yaml_error(
            path,
            format!("parent path {keys:?} is not a mapping"),
        ))
    }

    fn first_entry_index(&self) -> Option<usize> {
        self.lines
            .iter()
            .position(|line| !is_blank_or_comment(&line.content) && key_of(&line.content).is_some())
    }

    /// The end of the block opened at `start` (exclusive), at `indent`.
    fn block_end(&self, start: usize, indent: usize) -> usize {
        let mut end = start + 1;
        while end < self.lines.len() {
            let line = &self.lines[end];
            if is_blank_or_comment(&line.content) {
                end += 1;
                continue;
            }
            let line_indent = indent_of(&line.content);
            if line_indent <= indent {
                break;
            }
            end += 1;
        }
        end
    }

    /// The last line of the block's own content (skipping trailing blanks/comments).
    fn last_content(&self, start: usize, index: usize, end: usize) -> usize {
        let mut last = index;
        for cursor in index + 1..end {
            let line = &self.lines[cursor];
            if !is_blank_or_comment(&line.content) {
                last = cursor;
            }
        }
        let _ = start;
        last
    }

    fn children(&self, start: usize, end: usize, child_indent: usize) -> Vec<(usize, String)> {
        let mut children = Vec::new();
        for index in start + 1..end {
            let line = &self.lines[index];
            if is_blank_or_comment(&line.content) {
                continue;
            }
            if let Some((indent, key)) = key_of(&line.content)
                && indent == child_indent
            {
                children.push((index, key));
            }
        }
        children
    }

    fn with_lines(
        &self,
        path: &std::path::Path,
        block: &Block,
        rendered: Vec<String>,
        position: InsertPosition,
    ) -> String {
        if rendered.is_empty() {
            return self.render();
        }
        let newline = self
            .lines
            .iter()
            .find(|line| !line.newline.is_empty())
            .map(|line| line.newline.clone())
            .unwrap_or_else(|| "\n".to_string());
        let _ = path;
        let mut lines = self.lines.clone();
        let index = match position {
            InsertPosition::Top if block.root => block.start.min(lines.len()),
            InsertPosition::Top if block.missing_parent.is_none() => {
                (block.start + 1).min(lines.len())
            }
            InsertPosition::Bottom if block.missing_parent.is_some() || block.root => lines.len(),
            _ => block.insert_at.min(lines.len()),
        };
        let needs_trailing_newline =
            index == lines.len() && lines.last().is_some_and(|line| line.newline.is_empty());
        if needs_trailing_newline && let Some(last) = lines.last_mut() {
            last.newline = newline.clone();
        }
        let mut inserted: Vec<Line> = rendered
            .into_iter()
            .map(|content| Line {
                content,
                newline: newline.clone(),
            })
            .collect();
        if index == lines.len()
            && let Some(last) = inserted.last_mut()
            && self
                .lines
                .last()
                .is_some_and(|line| line.newline.is_empty())
        {
            last.newline = String::new();
        }
        lines.splice(index..index, inserted);
        Self {
            bom: self.bom,
            lines,
        }
        .render()
    }
}

struct Block {
    start: usize,
    insert_at: usize,
    child_indent: String,
    existing_children: Vec<(usize, String)>,
    missing_parent: Option<String>,
    root: bool,
}

impl Block {
    fn has_child(&self, key: &str) -> bool {
        self.existing_children
            .iter()
            .any(|(_, existing)| existing == key)
    }

    fn child_line(&self, key: &str) -> Option<usize> {
        self.existing_children
            .iter()
            .find(|(_, existing)| existing == key)
            .map(|(index, _)| *index)
    }

    fn child_indent(&self) -> &str {
        &self.child_indent
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

/// Return `(indent, key)` for a simple `key:` line, `None` for other lines.
fn key_of(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
        return None;
    }
    let indent = indent_of(line);
    // Split at the first mapping colon that is not inside quotes.
    let mut quote: Option<char> = None;
    let mut previous = '\0';
    let mut colon = None;
    for (index, character) in trimmed.char_indices() {
        match quote {
            Some(active) if character == active && previous != '\\' => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character == '#' && previous == ' ' => break,
            None if character == ':' => {
                let after = trimmed[index + 1..].chars().next();
                if after.is_none() || after.is_some_and(char::is_whitespace) {
                    colon = Some(index);
                    break;
                }
            }
            None => {}
        }
        previous = character;
    }
    let colon = colon?;
    let raw_key = trimmed[..colon].trim();
    if raw_key.is_empty() {
        return None;
    }
    let key = match serde_yaml_ng::from_str::<Value>(&format!("{raw_key}: 0")) {
        Ok(Value::Mapping(mapping)) => mapping
            .into_iter()
            .next()
            .and_then(|(key, _)| key.as_str().map(str::to_string))?,
        _ => return None,
    };
    Some((indent, key))
}

/// Replace the value of a `key: value` line, keeping its key text and any
/// trailing comment.
fn replace_scalar(line: &str, rendered: &str) -> Result<String, InstallError> {
    let colon = line.find(':').ok_or_else(|| InstallError::YamlEdit {
        path: std::path::PathBuf::from("<memory>"),
        detail: "line has no mapping colon".to_string(),
    })?;
    let after = &line[colon + 1..];
    let value_trimmed = after.trim_start();
    let value_offset = after.len() - value_trimmed.len();
    let value_start = colon + 1 + value_offset;
    // Find an unquoted trailing comment.
    let mut quote: Option<char> = None;
    let mut previous = '\0';
    let mut comment = None;
    let bytes = line.as_bytes();
    let mut index = value_start;
    while index < line.len() {
        let character = line[index..].chars().next().expect("in bounds");
        match quote {
            Some(active) if character == active && previous != '\\' => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character == '#' && index > 0 && bytes[index - 1] == b' ' => {
                comment = Some(index);
                break;
            }
            None => {}
        }
        previous = character;
        index += character.len_utf8();
    }
    let raw_end = comment.unwrap_or(line.len());
    let mut value_end = raw_end;
    while value_end > value_start && line[..value_end].ends_with(' ') {
        value_end -= 1;
    }
    let mut output = String::from(&line[..value_start]);
    if output.ends_with(':') {
        output.push(' ');
    }
    output.push_str(rendered);
    output.push_str(&line[value_end..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(
        editor: &YamlEditor,
        parent: &str,
        entries: &[(&str, Scalar)],
    ) -> Result<EditOutcome, InstallError> {
        let owned: Vec<(String, Scalar)> = entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect();
        editor.insert_scalars(parent, &owned, InsertPosition::Top)
    }

    #[test]
    fn inserts_missing_keys_and_preserves_operator_content() {
        let text = "# operator header\nspec_kitty:\n  # keep this comment\n  version: '1.2.3'  # inline\n\noperator:\n  kept: true\n";
        let editor = YamlEditor::new("metadata.yaml", text);
        let outcome =
            insert(&editor, "spec_kitty", &[("schema_version", Scalar::Int(3))]).expect("insert");
        assert!(outcome.changed);
        assert_eq!(outcome.inserted, vec!["schema_version"]);
        assert!(outcome.text.contains("# keep this comment"));
        assert!(outcome.text.contains("version: '1.2.3'  # inline"));
        assert!(outcome.text.contains("operator:\n  kept: true"));
        assert!(outcome.text.contains("  schema_version: 3"));
        // The insertion sits inside spec_kitty, before the blank line, at the
        // top of the mapping (the requested InsertPosition).
        let schema = outcome.text.find("schema_version").unwrap();
        let operator = outcome.text.find("operator:").unwrap();
        let version = outcome.text.find("version:").unwrap();
        assert!(schema < version && version < operator);
    }

    #[test]
    fn existing_key_is_never_replaced_by_insert() {
        let text = "spec_kitty:\n  schema_version: 9\n";
        let editor = YamlEditor::new("metadata.yaml", text);
        let outcome =
            insert(&editor, "spec_kitty", &[("schema_version", Scalar::Int(3))]).expect("no-op");
        assert!(!outcome.changed);
        assert_eq!(outcome.text, text);
    }

    #[test]
    fn inserts_a_missing_parent_mapping_at_the_top() {
        let text = "# header\n\nvcs:\n  type: git\n";
        let editor = YamlEditor::new("config.yaml", text);
        let outcome =
            insert(&editor, "project", &[("slug", Scalar::Str("demo".into()))]).expect("insert");
        assert!(outcome.text.starts_with("# header\n"));
        assert!(outcome.text.contains("project:\n  slug: demo\n"));
        assert!(outcome.text.contains("vcs:\n  type: git"));
    }

    #[test]
    fn upsert_replaces_only_the_value_and_keeps_inline_comments() {
        let text = "project:\n  uuid: old-id  # identity\n  slug: demo\n";
        let editor = YamlEditor::new("config.yaml", text);
        let outcome = editor
            .upsert_scalars(
                "project",
                &[("uuid".to_string(), Scalar::Str("new-id".to_string()))],
                InsertPosition::Top,
            )
            .expect("upsert");
        assert!(outcome.changed);
        assert!(
            outcome.text.contains("uuid: new-id  # identity"),
            "{}",
            outcome.text
        );
        assert!(outcome.text.contains("slug: demo"));
    }

    #[test]
    fn refuses_to_replace_structured_values() {
        let text = "project:\n  uuid:\n    nested: true\n";
        let editor = YamlEditor::new("config.yaml", text);
        let error = editor
            .upsert_scalars(
                "project",
                &[("uuid".to_string(), Scalar::Str("new".to_string()))],
                InsertPosition::Top,
            )
            .expect_err("refused");
        assert_eq!(error.code(), "yaml_not_editable");
    }

    #[test]
    fn refuses_non_mapping_roots_and_duplicate_proof_failures() {
        let editor = YamlEditor::new("config.yaml", "- a\n- b\n");
        assert_eq!(
            insert(&editor, "project", &[("slug", Scalar::Str("x".into()))])
                .expect_err("refused")
                .code(),
            "yaml_not_editable"
        );
    }

    #[test]
    fn preserves_crlf_and_absent_trailing_newline() {
        let text = "spec_kitty:\r\n  version: 1";
        let editor = YamlEditor::new("metadata.yaml", text);
        let outcome =
            insert(&editor, "spec_kitty", &[("schema_version", Scalar::Int(3))]).expect("insert");
        assert!(
            outcome
                .text
                .contains("spec_kitty:\r\n  schema_version: 3\r\n  version: 1")
        );
        assert!(!outcome.text.ends_with("\r\n"));
    }

    #[test]
    fn quotes_timestamp_like_strings() {
        let editor = YamlEditor::new("metadata.yaml", "spec_kitty:\n");
        let outcome = insert(
            &editor,
            "spec_kitty",
            &[(
                "initialized_at",
                Scalar::Str("2026-11-02T19:52:04.299247+00:00".into()),
            )],
        )
        .expect("insert");
        assert!(
            outcome
                .text
                .contains("initialized_at: '2026-11-02T19:52:04.299247+00:00'")
        );
    }

    #[test]
    fn inserts_a_nested_capabilities_mapping() {
        let editor = YamlEditor::new("metadata.yaml", "spec_kitty:\n  version: 1\n");
        let outcome = editor
            .insert_mapping(
                "spec_kitty",
                &MappingEntry {
                    key: "schema_capabilities".to_string(),
                    children: vec![
                        ("canonical_context".to_string(), Scalar::Bool(true)),
                        ("thin_shims".to_string(), Scalar::Bool(true)),
                    ],
                },
                InsertPosition::Bottom,
            )
            .expect("insert");
        assert!(outcome.changed);
        assert!(outcome.text.contains(
            "  schema_capabilities:\n    canonical_context: true\n    thin_shims: true\n"
        ));
    }
}
