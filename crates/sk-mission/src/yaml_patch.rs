//! Textual, format-preserving edits to a top-level YAML frontmatter field.
//!
//! The read path never reserializes user-authored YAML. The write path here
//! follows the same rule: only the named field's value bytes change. Comments,
//! key order, indentation, line endings, and the Markdown body stay exactly as
//! the author wrote them. After editing, the result is decoded again and the
//! complete mapping is compared with the expected mapping, so a patch that
//! would change any other field fails closed instead of shipping.

use std::fmt;

use serde_yaml_ng::{Mapping, Value};

use crate::frontmatter::{FrontmatterError, split_frontmatter_spans};

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum PatchError {
    MissingFrontmatter,
    Unterminated,
    InvalidYaml(String),
    DuplicateField(String),
    UnsafeRewrite(String),
}

impl fmt::Display for PatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => formatter.write_str("missing frontmatter"),
            Self::Unterminated => formatter.write_str("unterminated frontmatter"),
            Self::InvalidYaml(detail) => write!(formatter, "invalid YAML: {detail}"),
            Self::DuplicateField(field) => {
                write!(formatter, "frontmatter declares {field} more than once")
            }
            Self::UnsafeRewrite(detail) => write!(formatter, "unsafe YAML rewrite: {detail}"),
        }
    }
}

impl std::error::Error for PatchError {}

impl From<FrontmatterError> for PatchError {
    fn from(error: FrontmatterError) -> Self {
        match error {
            FrontmatterError::MissingOpeningDelimiter => Self::MissingFrontmatter,
            FrontmatterError::Unterminated => Self::Unterminated,
        }
    }
}

/// Return `source` with the top-level sequence `field` set to `values`.
///
/// Returns `Ok(None)` when the field already has exactly those values in that
/// order. The Markdown body and every other YAML byte are preserved.
pub(crate) fn set_top_level_sequence(
    source: &str,
    field: &str,
    values: &[String],
) -> Result<Option<String>, PatchError> {
    let with_bom = source.starts_with('\u{feff}');
    let text = source.strip_prefix('\u{feff}').unwrap_or(source);
    let spans = split_frontmatter_spans(text)?;
    let yaml = &text[spans.yaml_start..spans.yaml_end];

    let existing = decode_mapping(yaml)?;
    let current = sequence_of(&existing, field)?;
    if current.as_deref() == Some(values) {
        return Ok(None);
    }

    let new_yaml = rewrite_yaml(yaml, field, values)?;
    let decoded = decode_mapping(&new_yaml)?;
    let mut expected = existing.clone();
    expected.insert(
        Value::String(field.to_owned()),
        Value::Sequence(values.iter().cloned().map(Value::String).collect()),
    );
    // serde_yaml_ng mappings preserve insertion order, so compare by key/value
    // rather than by position: only the field's VALUE is allowed to differ.
    if mapping_semantics(&decoded) != mapping_semantics(&expected) {
        return Err(PatchError::UnsafeRewrite(format!(
            "rewriting {field} would change another field"
        )));
    }

    let mut output = String::with_capacity(text.len() + values.len() * 8);
    if with_bom {
        output.push('\u{feff}');
    }
    output.push_str(&text[..spans.yaml_start]);
    output.push_str(&new_yaml);
    output.push_str(&text[spans.yaml_end..]);
    Ok(Some(output))
}

/// Decode a frontmatter YAML payload into a mapping.
pub(crate) fn decode_mapping(yaml: &str) -> Result<Mapping, PatchError> {
    let value: Value = serde_yaml_ng::from_str(yaml)
        .map_err(|error| PatchError::InvalidYaml(error.to_string()))?;
    match value {
        Value::Mapping(mapping) => Ok(mapping),
        Value::Null => Ok(Mapping::new()),
        other => Err(PatchError::InvalidYaml(format!(
            "frontmatter is {}, not a mapping",
            kind(&other)
        ))),
    }
}

/// Read one top-level field as a sequence of strings.
///
/// `None` means the field is absent or null. A scalar string is refused except
/// for the legacy quoted `"[]"` spelling, which the Python finalizer accepts.
pub(crate) fn sequence_of(
    mapping: &Mapping,
    field: &str,
) -> Result<Option<Vec<String>>, PatchError> {
    match mapping.get(Value::String(field.to_owned())) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Sequence(items)) => {
            let mut values = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let Some(value) = item.as_str() else {
                    return Err(PatchError::InvalidYaml(format!(
                        "{field}[{index}] is {}; expected a string",
                        kind(item)
                    )));
                };
                values.push(value.to_owned());
            }
            Ok(Some(values))
        }
        Some(Value::String(value)) if value.trim() == "[]" => Ok(Some(Vec::new())),
        Some(other) => Err(PatchError::InvalidYaml(format!(
            "{field} is {}; expected a sequence of strings",
            kind(other)
        ))),
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a sequence",
        Value::Mapping(_) => "a mapping",
        Value::Tagged(_) => "a tagged value",
    }
}

fn mapping_semantics(mapping: &Mapping) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = mapping
        .iter()
        .map(|(key, value)| {
            (
                serde_yaml_ng::to_string(&key)
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                serde_yaml_ng::to_string(&value)
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
            )
        })
        .collect();
    entries.sort();
    entries
}

struct Line<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

fn lines_with_offsets(yaml: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for raw in yaml.split_inclusive('\n') {
        let start = offset;
        offset += raw.len();
        lines.push(Line {
            start,
            end: offset,
            text: raw,
        });
    }
    if yaml.is_empty() {
        return lines;
    }
    if !yaml.ends_with('\n') {
        // split_inclusive already covered the final partial line.
    }
    lines
}

fn without_line_ending(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

/// Count top-level declarations of `field` (used to refuse duplicate YAML keys).
pub(crate) fn count_top_level_field(yaml: &str, field: &str) -> usize {
    lines_with_offsets(yaml)
        .iter()
        .filter(|line| top_level_key(line.text).as_deref() == Some(field))
        .count()
}

fn top_level_key(line: &str) -> Option<String> {
    let line = without_line_ending(line);
    if line.is_empty() {
        return None;
    }
    let first = line.as_bytes()[0];
    if matches!(first, b' ' | b'\t' | b'#') {
        return None;
    }
    let colon = line.find(':')?;
    let mut key = line[..colon].trim();
    if (key.starts_with('"') && key.ends_with('"') && key.len() >= 2)
        || (key.starts_with('\'') && key.ends_with('\'') && key.len() >= 2)
    {
        key = &key[1..key.len() - 1];
    }
    (!key.is_empty()).then(|| key.to_owned())
}

fn newline_of(line: &str) -> &'static str {
    if line.ends_with("\r\n") { "\r\n" } else { "\n" }
}

fn render_scalar(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if plain {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn flow(values: &[String]) -> String {
    let rendered: Vec<String> = values.iter().map(|value| render_scalar(value)).collect();
    format!("[{}]", rendered.join(", "))
}

fn rewrite_yaml(yaml: &str, field: &str, values: &[String]) -> Result<String, PatchError> {
    let lines = lines_with_offsets(yaml);
    let mut key_index = None;
    for (index, line) in lines.iter().enumerate() {
        if top_level_key(line.text).as_deref() == Some(field) {
            if key_index.is_some() {
                return Err(PatchError::DuplicateField(field.to_owned()));
            }
            key_index = Some(index);
        }
    }

    let Some(key_index) = key_index else {
        return Ok(insert_field(yaml, field, values));
    };

    let key_line = &lines[key_index];
    let newline = newline_of(key_line.text);
    let next_start = key_line.end;

    let mut block_end = yaml.len();
    let mut first_value_line = None;
    for line in lines.iter().skip(key_index + 1) {
        let content = without_line_ending(line.text);
        if content.trim().is_empty() {
            continue;
        }
        if content.starts_with(' ') || content.starts_with('\t') {
            if first_value_line.is_none() {
                first_value_line = Some(line);
            }
            continue;
        }
        block_end = line.start;
        break;
    }

    // Keep blank separator lines that precede the next top-level field.
    let mut replacement_end = block_end;
    for line in lines.iter().skip(key_index + 1) {
        if line.start >= block_end {
            break;
        }
        if !without_line_ending(line.text).trim().is_empty() {
            replacement_end = line.end;
        }
    }
    if replacement_end == next_start && block_end > next_start {
        // A block of only blank lines: preserve it untouched.
        replacement_end = next_start;
    }

    let key_content = without_line_ending(key_line.text);
    let colon = key_content.find(':').unwrap_or(key_content.len());
    let after_colon = &key_content[colon + 1..];
    let inline_comment = after_colon
        .find('#')
        .map(|hash| after_colon[hash..].trim_end().to_owned());
    let same_line_value = after_colon.split('#').next().unwrap_or_default().trim();

    let block_style = same_line_value.is_empty() && first_value_line.is_some();
    let indent = first_value_line
        .map(|line| {
            let content = without_line_ending(line.text);
            content.len() - content.trim_start_matches([' ', '\t']).len()
        })
        .unwrap_or(2)
        .max(1);

    let comment_suffix = inline_comment
        .as_ref()
        .map(|comment| format!("  {comment}"))
        .unwrap_or_default();
    let rendered = if values.is_empty() {
        format!("{field}: []{comment_suffix}")
    } else if block_style {
        let mut rendered = format!("{field}:{comment_suffix}");
        for value in values {
            rendered.push_str(newline);
            rendered.push_str(&" ".repeat(indent));
            rendered.push_str("- ");
            rendered.push_str(&render_scalar(value));
        }
        rendered
    } else {
        format!("{field}: {}{comment_suffix}", flow(values))
    };

    let mut output = String::with_capacity(yaml.len() + 64);
    output.push_str(&yaml[..key_line.start]);
    output.push_str(&rendered);
    if key_line.text.ends_with('\n') {
        output.push_str(newline);
    }
    output.push_str(&yaml[replacement_end..]);
    Ok(output)
}

fn insert_field(yaml: &str, field: &str, values: &[String]) -> String {
    let lines = lines_with_offsets(yaml);
    let newline = lines
        .first()
        .map(|line| newline_of(line.text))
        .unwrap_or("\n");
    let insertion = format!("{field}: {}{newline}", flow(values));
    let anchor = lines
        .iter()
        .find(|line| top_level_key(line.text).as_deref() == Some("work_package_id"));
    let offset = anchor.map(|line| line.end).unwrap_or(yaml.len());
    let mut output = String::with_capacity(yaml.len() + insertion.len());
    output.push_str(&yaml[..offset]);
    output.push_str(&insertion);
    output.push_str(&yaml[offset..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_comments_and_body() {
        let source = "---\n# note\nwork_package_id: WP02\ntitle: \"Keep\"  # inline\ndependencies: []\n---\n\nBody # not a comment\n";
        let updated = set_top_level_sequence(
            source,
            "dependencies",
            &["WP01".to_owned(), "WP02".to_owned()],
        )
        .expect("patch")
        .expect("changed");
        assert_eq!(
            updated,
            "---\n# note\nwork_package_id: WP02\ntitle: \"Keep\"  # inline\ndependencies: [WP01, WP02]\n---\n\nBody # not a comment\n"
        );
    }

    #[test]
    fn keeps_block_style_and_blank_separators() {
        let source =
            "---\nwork_package_id: WP02\ndependencies:\n  - WP09\n\nphase: one\n---\nbody\n";
        let updated = set_top_level_sequence(source, "dependencies", &["WP01".to_owned()])
            .expect("patch")
            .expect("changed");
        assert_eq!(
            updated,
            "---\nwork_package_id: WP02\ndependencies:\n  - WP01\n\nphase: one\n---\nbody\n"
        );
    }

    #[test]
    fn inserts_after_work_package_id_when_missing() {
        let source = "---\ntitle: T\nwork_package_id: WP01\nphase: P\n---\nbody\n";
        let updated = set_top_level_sequence(source, "dependencies", &["WP02".to_owned()])
            .expect("patch")
            .expect("changed");
        assert_eq!(
            updated,
            "---\ntitle: T\nwork_package_id: WP01\ndependencies: [WP02]\nphase: P\n---\nbody\n"
        );
    }
}
