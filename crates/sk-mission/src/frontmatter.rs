use thiserror::Error;

/// Why a Markdown document does not carry a well-formed frontmatter block.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum FrontmatterError {
    /// The document does not begin with a `---` delimiter line.
    #[error("the document does not begin with a '---' frontmatter delimiter")]
    MissingOpeningDelimiter,

    /// The opening delimiter is never closed by `---` or `...`.
    #[error("the frontmatter block is not closed by a '---' or '...' line")]
    Unterminated,
}

/// A borrowed split of a Markdown document into its frontmatter and body.
///
/// Both slices point into the original source; nothing is copied, normalized,
/// or reserialized.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Frontmatter<'a> {
    /// The YAML text between the opening and closing delimiter lines.
    pub yaml: &'a str,
    /// Everything after the closing delimiter line, including its line ending.
    pub body: &'a str,
}

/// Split a Markdown document into YAML frontmatter and body.
///
/// Accepted shape (the shape every real `kitty-specs/*/tasks/WP*.md` uses):
///
/// ```text
/// ---
/// work_package_id: WP01
/// ---
/// body
/// ```
///
/// A leading UTF-8 BOM is tolerated. Delimiter lines may carry trailing
/// whitespace and CRLF endings. The body starts at the first byte after the
/// closing delimiter's line ending, so callers can inspect the prompt text
/// without the source ever being rewritten.
pub fn split_frontmatter(source: &str) -> Result<Frontmatter<'_>, FrontmatterError> {
    let source_without_bom = source.strip_prefix('\u{feff}').unwrap_or(source);

    let mut offset = 0usize;
    let mut opening_closed = false;
    let mut yaml_start = 0usize;

    for line in source_without_bom.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let trimmed = line
            .trim_end_matches(['\r', '\n'])
            .trim_end_matches([' ', '\t']);

        if !opening_closed {
            if line_start != 0 || trimmed != "---" {
                return Err(FrontmatterError::MissingOpeningDelimiter);
            }
            opening_closed = true;
            yaml_start = offset;
            continue;
        }

        if trimmed == "---" || trimmed == "..." {
            return Ok(Frontmatter {
                yaml: &source_without_bom[yaml_start..line_start],
                body: &source_without_bom[offset..],
            });
        }
    }

    if !opening_closed {
        return Err(FrontmatterError::MissingOpeningDelimiter);
    }
    Err(FrontmatterError::Unterminated)
}
