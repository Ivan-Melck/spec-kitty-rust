//! Active project languages: the doctrine-derived vocabulary, the declared and
//! configured authorities, and the interview inference tiers.
//!
//! Mirrors `src/charter/activation/language_scope.py`,
//! `src/charter/activation/language_authority.py`,
//! `src/charter/offering/shared/language_catalog.py` and
//! `src/charter/offering/shared/scoping.py` for the language signal only.
//!
//! [`LanguageScopeService::infer`] is the single authority for the signal's
//! three load-bearing result states:
//!
//! * [`InferredLanguages::NoSignal`] — no charter/interview signal (or an answer
//!   that declares nothing: empty, placeholder, the shipped default text). Every
//!   scoped artifact is admitted.
//! * [`InferredLanguages::Empty`] — an explicit empty compiled answer
//!   (`catalog.languages: []`). Nothing scoped is admitted.
//! * [`InferredLanguages::Declared`] — a recognised list, e.g. `["python"]`, or
//!   `["unknown"]` for a real declaration no doctrine artifact is scoped to
//!   (#5284: `unknown` means "no specialist guidance exists", never "bad
//!   language").
//!
//! Tier order: (0) the operator-confirmed `languages` list of
//! `.kittify/config.yaml`, which wins over everything below including
//! `prefer_interview` (FR-020); (1) the compiled `catalog.languages` in
//! `.kittify/charter/charter.yaml` — authoritative once present, **even when
//! empty**; (2) the interview (`interview` when supplied, else the on-disk
//! transcript). `prefer_interview` (charter regeneration) resolves from the
//! interview alone and may return no signal. There is no `charter.md` fallback.
//!
//! A real `languages_frameworks` declaration decides alone (operator decision
//! D1): built-in detector hits in that answer UNION doctrine-vocabulary
//! whole-word hits in that answer, else `["unknown"]`. Other answers are only
//! scanned (built-in detector only) when that answer is absent, a placeholder or
//! the shipped default; nothing found is no signal.
//!
//! Declared representation limits (no false equality claimed): Python
//! `str.casefold()` is approximated with `to_lowercase()` (identical for ASCII,
//! may differ for `ß`/final sigma), Python `\w` with
//! `char::is_alphanumeric() || '_'`, and the `ast.literal_eval` subset below
//! covers flat/nested list, tuple and scalar shapes (float and exotic reprs are
//! rendered best-effort; a shape it cannot parse falls back exactly where the
//! reference falls back — to the raw answer text).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;

use crate::error::DoctrineError;
use crate::language_vocabulary::LanguageVocabularyCache;

/// Reserved language value Spec Kitty writes for an unrecognised project.
///
/// Artifacts cannot target it (the `charter validate` guard rejects it) and it
/// is stripped from both sides of the scoping match. It is deliberately not a
/// sentinel.
pub const UNKNOWN_LANGUAGE: &str = "unknown";

/// Tokens that are never real language identifiers for artifact scoping.
pub const RESERVED_LANGUAGE_TOKENS: [&str; 1] = [UNKNOWN_LANGUAGE];

/// Sentinel strings that should never appear as real language tokens.
pub const SENTINEL_LANGUAGE_TOKENS: [&str; 2] = ["any", "all"];

/// The interview question that declares the project's languages.
pub const LANGUAGES_ANSWER_KEY: &str = "languages_frameworks";

/// The family `java` and `kotlin` both imply.
pub const JVM_FAMILY: &str = "jvm";

/// The top-level `.kittify/config.yaml` key holding the operator-confirmed list.
pub const CONFIG_LANGUAGES_KEY: &str = "languages";

/// The operator-confirmed config path, relative to the repository root.
pub const CONFIG_RELATIVE_PATH: &str = ".kittify/config.yaml";

/// The on-disk interview transcript, relative to the repository root.
pub const INTERVIEW_ANSWERS_RELATIVE_PATH: &str = ".kittify/charter/interview/answers.yaml";

/// A token preceded by `word-` is part of a longer compound name
/// (`librespot-java`), not a declaration of that language (#4613). Trailing
/// compounds such as `Java-based` still match.
///
/// Recorded as the reference's regular-expression prefix; the native detector
/// applies the same rule directly without a regex engine.
pub const COMPOUND_PREFIX_GUARD: &str = r"(?<!\w-)";

/// The shipped `defaults.yaml` answer for [`LANGUAGES_ANSWER_KEY`].
pub const SHIPPED_DEFAULT_LANGUAGES_ANSWER: &str =
    "Use only the languages, frameworks, and tools explicitly declared by this project.";

/// The shipped `defaults.yaml` answer texts, key for key.
///
/// A supplied interview whose every answer is exactly one of these carries no
/// user answers, so it must not mask the on-disk transcript (compile and
/// runtime would otherwise diverge, F5).
pub const SHIPPED_DEFAULT_ANSWERS: [(&str, &str); 11] = [
    (
        "project_intent",
        "Deliver predictable changes with clear reviewability.",
    ),
    (
        "languages_frameworks",
        "Use only the languages, frameworks, and tools explicitly declared by this project.",
    ),
    (
        "testing_requirements",
        "Use the project's declared test approach. If none is declared, mark it as NEEDS CLARIFICATION.",
    ),
    (
        "quality_gates",
        "Only require the quality gates explicitly declared by this project before merge.",
    ),
    (
        "review_policy",
        "At least one focused reviewer approves before merge.",
    ),
    (
        "performance_targets",
        "CLI operations should complete quickly (typically under 2 seconds).",
    ),
    (
        "deployment_constraints",
        "Must run on macOS and Linux developer environments.",
    ),
    (
        "documentation_policy",
        "Update docs whenever command behavior or workflow semantics change.",
    ),
    (
        "risk_boundaries",
        "Do not relax quality/security standards without explicit maintainer approval.",
    ),
    (
        "amendment_process",
        "Changes are proposed via PR and reviewed before adoption.",
    ),
    (
        "exception_policy",
        "Exceptions must be documented in PR notes with scope and sunset criteria.",
    ),
];

/// Normalised answers that declare nothing: not a language, and never a signal
/// that the language is *unknown*.
const PLACEHOLDER_ANSWERS: [&str; 10] = [
    "",
    "[]",
    "-",
    "n/a",
    "na",
    "none",
    "null",
    "tbd",
    "language-agnostic",
    "language agnostic",
];

const NEEDS_CLARIFICATION_PREFIX: &str = "[needs clarification";

// ---------------------------------------------------------------------------
// The language catalog
// ---------------------------------------------------------------------------

/// One catalog language: its token, declared-output rank, declared patterns and
/// the families it implies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageSpec {
    /// The canonical token.
    pub token: &'static str,
    /// Position in declared-language output (the order persisted upstream).
    pub declared_rank: u8,
    /// Whole-word declaration patterns (`\b<needle>\b`, compound-guarded).
    pub declared_patterns: &'static [&'static str],
    /// Families this language implies (expanded at match time, never persisted).
    pub families: &'static [&'static str],
}

/// A build or test tool whose declaration names only a family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuildToolSpec {
    /// The tool token as authored in prose.
    pub token: &'static str,
    /// Whole-word declaration patterns.
    pub declared_patterns: &'static [&'static str],
    /// The family it names when no language of that family is declared.
    pub family: &'static str,
    /// The declared-output rank of the family it contributes.
    pub declared_rank: u8,
}

/// The one language catalog (mirror of `language_catalog.py`).
pub const LANGUAGES: [LanguageSpec; 10] = [
    LanguageSpec {
        token: "python",
        declared_rank: 0,
        declared_patterns: &["python", "pytest", "mypy", "ruff"],
        families: &[],
    },
    LanguageSpec {
        token: "go",
        declared_rank: 9,
        declared_patterns: &[],
        families: &[],
    },
    LanguageSpec {
        token: "rust",
        declared_rank: 3,
        declared_patterns: &["rust", "cargo", "rustc"],
        families: &[],
    },
    LanguageSpec {
        token: "java",
        declared_rank: 4,
        declared_patterns: &["java"],
        families: &[JVM_FAMILY],
    },
    LanguageSpec {
        token: "kotlin",
        declared_rank: 5,
        declared_patterns: &["kotlin"],
        families: &[JVM_FAMILY],
    },
    LanguageSpec {
        token: "ruby",
        declared_rank: 7,
        declared_patterns: &["ruby", "rspec", "rails"],
        families: &[],
    },
    LanguageSpec {
        token: "php",
        declared_rank: 8,
        declared_patterns: &["php", "phpunit"],
        families: &[],
    },
    LanguageSpec {
        token: "javascript",
        declared_rank: 2,
        // The reference pattern is `\bnode(?:\.js)?\b`; the alternation is
        // equivalent to these two boundary-checked needles for matching.
        declared_patterns: &[
            "javascript",
            "jest",
            "node",
            "node.js",
            "npm",
            "pnpm",
            "yarn",
        ],
        families: &[],
    },
    LanguageSpec {
        token: "typescript",
        declared_rank: 1,
        declared_patterns: &["typescript", "tsc"],
        families: &[],
    },
    LanguageSpec {
        token: "swift",
        declared_rank: 6,
        declared_patterns: &["swift", "xctest"],
        families: &[],
    },
];

/// Build and test tools: declared, they add only their family, and only when no
/// language of that family is declared (the build-token rule, FR-013).
pub const BUILD_TOOLS: [BuildToolSpec; 3] = [
    BuildToolSpec {
        token: "gradle",
        declared_patterns: &["gradle"],
        family: JVM_FAMILY,
        declared_rank: 4,
    },
    BuildToolSpec {
        token: "maven",
        declared_patterns: &["maven"],
        family: JVM_FAMILY,
        declared_rank: 4,
    },
    BuildToolSpec {
        token: "junit",
        declared_patterns: &["junit"],
        family: JVM_FAMILY,
        declared_rank: 4,
    },
];

/// The public token list (the seam exports this).
pub fn language_tokens() -> Vec<&'static str> {
    LANGUAGES.iter().map(|spec| spec.token).collect()
}

/// Whether `token` is a catalog language (a family such as `jvm` is not).
pub fn is_catalog_language(token: &str) -> bool {
    let token = token.trim().to_lowercase();
    LANGUAGES.iter().any(|spec| spec.token == token)
}

/// Whether `token` names a family such as `jvm`, not a language.
pub fn is_language_family(token: &str) -> bool {
    let token = token.trim().to_lowercase();
    LANGUAGES
        .iter()
        .any(|spec| spec.families.contains(&token.as_str()))
}

/// Return `tokens` plus the family each language implies (stable order, no
/// duplicates).
///
/// `["kotlin"]` becomes `["kotlin", "jvm"]`; a family token never implies its
/// members, so `["jvm"]` stays `["jvm"]`. Expansion is applied at match time
/// only and is never persisted.
pub fn expand_language_families<I, S>(tokens: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut expanded: Vec<String> = Vec::new();
    for raw in tokens {
        let token = raw.as_ref().trim().to_lowercase();
        let families: &[&str] = LANGUAGES
            .iter()
            .find(|spec| spec.token == token)
            .map(|spec| spec.families)
            .unwrap_or(&[]);
        for item in std::iter::once(token.as_str()).chain(families.iter().copied()) {
            if !item.is_empty() && !expanded.iter().any(|existing| existing == item) {
                expanded.push(item.to_owned());
            }
        }
    }
    expanded
}

/// Python `\w` for `str` patterns: alphanumerics plus underscore.
fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Whether `needle` occurs in `haystack` as a whole word, with the reference's
/// `(?<!\w-)` compound guard.
///
/// Every shipped pattern is `\b<literal>\b` (plus the `node(?:\.js)?`
/// alternation, expanded into two needles above), so the regex reduces to this
/// scan. `haystack` is already lowercased.
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let index = from + offset;
        let end = index + needle.len();
        let before = haystack[..index].chars().next_back();
        let before_before = haystack[..index].chars().rev().nth(1);
        let after = haystack[end..].chars().next();
        let boundary_before = before.is_none_or(|previous| !is_word_char(previous));
        let compound_guard = !(before == Some('-') && before_before.is_some_and(is_word_char));
        let boundary_after = after.is_none_or(|next| !is_word_char(next));
        if boundary_before && compound_guard && boundary_after {
            return true;
        }
        from = index + 1;
    }
    false
}

/// Canonical language identifiers a free-form declaration names, in
/// `declared_rank` order.
///
/// Build-token rule (FR-013): a declared build or test tool (gradle, maven,
/// junit) adds its family (`jvm`) only when no language of that family is
/// declared, so `"Gradle"` alone is `["jvm"]` while `"Kotlin, Gradle"` is
/// `["kotlin"]`.
pub fn declared_languages(text: &str) -> Vec<String> {
    let haystack = text.to_lowercase();
    let mut matches: Vec<(u8, &'static str)> = Vec::new();
    let mut families: BTreeSet<&'static str> = BTreeSet::new();
    for spec in LANGUAGES {
        if spec
            .declared_patterns
            .iter()
            .any(|needle| contains_word(&haystack, needle))
        {
            matches.push((spec.declared_rank, spec.token));
            families.extend(spec.families.iter().copied());
        }
    }
    for tool in BUILD_TOOLS {
        if !families.contains(tool.family)
            && tool
                .declared_patterns
                .iter()
                .any(|needle| contains_word(&haystack, needle))
        {
            matches.push((tool.declared_rank, tool.family));
            families.insert(tool.family);
        }
    }
    matches.sort_unstable();
    let tokens: Vec<String> = matches
        .into_iter()
        .map(|(_, token)| token.to_owned())
        .collect();
    crate::profile::normalize_languages(&tokens)
}

/// Canonical language identifiers mentioned in free-form text.
///
/// The build-token rule is included: `"Gradle"` alone is `["jvm"]`.
pub fn extract_declared_languages(text: &str) -> Vec<String> {
    declared_languages(text)
}

// ---------------------------------------------------------------------------
// Python value rendering (the reference's `str(value)` conversion)
// ---------------------------------------------------------------------------

/// A YAML value rendered as Python's `str(value)` would render it.
///
/// Interview answers and compiled list items are `str()`-converted by the
/// reference before any language parsing, so a list answer arrives as its Python
/// list representation (`"['zig', 'c']"`) and is re-parsed by `_answer_items`.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => python_repr(other),
    }
}

/// The reference `repr(value)` projection of one YAML value.
///
/// Public so the Source-first control matrix (`tests/python_repr_oracle.rs`) can
/// diff it against the pinned interpreter without a second copy of the rules.
/// Strings render through the canonical kernel formatter.
pub fn python_value_repr(value: &Value) -> String {
    python_repr(value)
}

fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_owned(),
        Value::Number(number) => python_number_repr(number),
        Value::String(text) => python_string_repr(text),
        Value::Sequence(items) => format!(
            "[{}]",
            items.iter().map(python_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Mapping(mapping) => format!(
            "{{{}}}",
            mapping
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr(key), python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Tagged(tagged) => python_repr(&tagged.value),
    }
}

fn python_number_repr(number: &serde_yaml_ng::Number) -> String {
    if let Some(value) = number.as_i64() {
        return value.to_string();
    }
    if let Some(value) = number.as_u64() {
        return value.to_string();
    }
    number
        .as_f64()
        .map(python_float_repr)
        .unwrap_or_else(|| number.to_string())
}

/// `repr` of a float: integral values keep a `.0`.
///
/// Declared edge: the reference's shortest-repr exponent spelling (`1e+20`) is
/// not reproduced; only plain and integral floats are byte-identical.
fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-inf".to_owned()
        } else {
            "inf".to_owned()
        };
    }
    if value == value.trunc() && value.abs() < 1e16 {
        return format!("{value:.1}");
    }
    format!("{value}")
}

/// `repr` of a string through the workspace's one canonical CPython formatter.
///
/// This used to be a local approximation (quote, backslash and the three common
/// control escapes; every other non-printable code point emitted raw). That
/// diverged from the reference for exactly the answers this module renders — an
/// interview answer holding a NUL, a private-use code point or any other
/// non-printable — so the formatter is now `sk_kernel::python_repr`, whose
/// `str.isprintable()` table is generated from the pinned CPython 3.11.15
/// interpreter and shared with the Op reader. There is no second `repr`
/// authority in Doctrine; see `tests/python_repr_oracle.rs` for the Source-first
/// control matrix.
fn python_string_repr(text: &str) -> String {
    sk_kernel::python_repr(text)
}

/// The previous local renderer, kept only as the differential control's
/// reference point (it must never be called by production code).
#[cfg(test)]
fn legacy_python_string_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other == quote => {
                out.push('\\');
                out.push(other);
            }
            other => out.push(other),
        }
    }
    out.push(quote);
    out
}

// ---------------------------------------------------------------------------
// Python literal subset (the reference's `ast.literal_eval`)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum PyLiteral {
    Str(String),
    Int(i128),
    Float(f64),
    Bool(bool),
    None,
    List(Vec<PyLiteral>),
    Tuple(Vec<PyLiteral>),
    Dict(Vec<(PyLiteral, PyLiteral)>),
}

fn py_literal_str(literal: &PyLiteral) -> String {
    match literal {
        PyLiteral::Str(text) => text.clone(),
        other => py_literal_repr(other),
    }
}

fn py_literal_repr(literal: &PyLiteral) -> String {
    match literal {
        PyLiteral::Str(text) => python_string_repr(text),
        PyLiteral::Int(value) => value.to_string(),
        PyLiteral::Float(value) => python_float_repr(*value),
        PyLiteral::Bool(flag) => if *flag { "True" } else { "False" }.to_owned(),
        PyLiteral::None => "None".to_owned(),
        PyLiteral::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(py_literal_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        PyLiteral::Tuple(items) => {
            if items.len() == 1 {
                format!("({},)", py_literal_repr(&items[0]))
            } else {
                format!(
                    "({})",
                    items
                        .iter()
                        .map(py_literal_repr)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        PyLiteral::Dict(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(key, value)| format!("{}: {}", py_literal_repr(key), py_literal_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

struct LiteralParser {
    chars: Vec<char>,
    index: usize,
}

impl LiteralParser {
    fn parse(text: &str) -> Option<PyLiteral> {
        let mut parser = Self {
            chars: text.chars().collect(),
            index: 0,
        };
        let value = parser.value()?;
        parser.skip_whitespace();
        (parser.index == parser.chars.len()).then_some(value)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(is_python_whitespace) {
            self.index += 1;
        }
    }

    fn value(&mut self) -> Option<PyLiteral> {
        self.skip_whitespace();
        match self.peek()? {
            '[' => self.list(),
            '(' => self.tuple(),
            '{' => self.dict(),
            '\'' | '"' => self.string().map(PyLiteral::Str),
            _ => {
                if self.string_prefix_ahead() {
                    self.string().map(PyLiteral::Str)
                } else {
                    self.keyword_or_number()
                }
            }
        }
    }

    /// Whether the next characters are a Python string prefix then a quote.
    fn string_prefix_ahead(&self) -> bool {
        let mut index = self.index;
        let mut prefixes = 0;
        while prefixes < 2
            && self
                .chars
                .get(index)
                .is_some_and(|character| matches!(character, 'r' | 'R' | 'b' | 'B' | 'u' | 'U'))
        {
            index += 1;
            prefixes += 1;
        }
        prefixes > 0 && matches!(self.chars.get(index), Some('\'' | '"'))
    }

    fn list(&mut self) -> Option<PyLiteral> {
        self.index += 1;
        let mut items = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek() == Some(']') {
                self.index += 1;
                return Some(PyLiteral::List(items));
            }
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek()? {
                ',' => self.index += 1,
                ']' => {
                    self.index += 1;
                    return Some(PyLiteral::List(items));
                }
                _ => return None,
            }
        }
    }

    fn tuple(&mut self) -> Option<PyLiteral> {
        self.index += 1;
        self.skip_whitespace();
        if self.peek() == Some(')') {
            self.index += 1;
            return Some(PyLiteral::Tuple(Vec::new()));
        }
        let first = self.value()?;
        self.skip_whitespace();
        if self.peek() != Some(',') {
            // `(value)` is just `value` in Python.
            return (self.peek() == Some(')')).then(|| {
                self.index += 1;
                first
            });
        }
        let mut items = vec![first];
        while self.peek() == Some(',') {
            self.index += 1;
            self.skip_whitespace();
            if self.peek() == Some(')') {
                self.index += 1;
                return Some(PyLiteral::Tuple(items));
            }
            items.push(self.value()?);
            self.skip_whitespace();
        }
        if self.peek() == Some(')') {
            self.index += 1;
            return Some(PyLiteral::Tuple(items));
        }
        None
    }

    fn dict(&mut self) -> Option<PyLiteral> {
        self.index += 1;
        let mut entries = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek() == Some('}') {
                self.index += 1;
                return Some(PyLiteral::Dict(entries));
            }
            let key = self.value()?;
            self.skip_whitespace();
            // A set literal (`{1, 2}`) is not modelled; the reference's caller
            // falls back to the raw text for any non-list/tuple shape.
            if self.peek() != Some(':') {
                return None;
            }
            self.index += 1;
            let value = self.value()?;
            entries.push((key, value));
            self.skip_whitespace();
            match self.peek()? {
                ',' => self.index += 1,
                '}' => {
                    self.index += 1;
                    return Some(PyLiteral::Dict(entries));
                }
                _ => return None,
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        let mut raw = false;
        let mut prefixes = 0;
        while prefixes < 2 {
            match self.peek() {
                Some('r' | 'R') => {
                    raw = true;
                    self.index += 1;
                    prefixes += 1;
                }
                Some('b' | 'B' | 'u' | 'U') => {
                    self.index += 1;
                    prefixes += 1;
                }
                _ => break,
            }
        }
        let quote = self.peek()?;
        if quote != '\'' && quote != '"' {
            return None;
        }
        self.index += 1;
        let mut out = String::new();
        loop {
            let character = self.peek()?;
            self.index += 1;
            if character == quote {
                return Some(out);
            }
            if character != '\\' || raw {
                out.push(character);
                continue;
            }
            let escape = self.peek()?;
            self.index += 1;
            match escape {
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'v' => out.push('\u{b}'),
                '0' => out.push('\0'),
                'x' => out.push(self.hex_escape(2)?),
                'u' => out.push(self.hex_escape(4)?),
                'U' => out.push(self.hex_escape(8)?),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
        }
    }

    fn hex_escape(&mut self, digits: usize) -> Option<char> {
        let mut value: u32 = 0;
        for _ in 0..digits {
            let character = self.peek()?;
            let digit = character.to_digit(16)?;
            value = value * 16 + digit;
            self.index += 1;
        }
        char::from_u32(value)
    }

    fn keyword_or_number(&mut self) -> Option<PyLiteral> {
        let start = self.index;
        while self.peek().is_some_and(is_word_char) {
            self.index += 1;
        }
        let word: String = self.chars[start..self.index].iter().collect();
        match word.as_str() {
            "True" => return Some(PyLiteral::Bool(true)),
            "False" => return Some(PyLiteral::Bool(false)),
            "None" => return Some(PyLiteral::None),
            _ => self.index = start,
        }
        self.number()
    }

    fn number(&mut self) -> Option<PyLiteral> {
        let start = self.index;
        let mut negative = false;
        if matches!(self.peek(), Some('+' | '-')) {
            negative = self.peek() == Some('-');
            self.index += 1;
        }
        let mut radix = 10;
        if self.peek() == Some('0') {
            match self.chars.get(self.index + 1) {
                Some('x' | 'X') => {
                    radix = 16;
                    self.index += 2;
                }
                Some('o' | 'O') => {
                    radix = 8;
                    self.index += 2;
                }
                Some('b' | 'B') => {
                    radix = 2;
                    self.index += 2;
                }
                _ => {}
            }
        }
        let mut text = String::new();
        let mut is_float = false;
        while let Some(character) = self.peek() {
            if character == '_' {
                self.index += 1;
                continue;
            }
            if character.is_digit(radix) {
                text.push(character);
                self.index += 1;
                continue;
            }
            if radix == 10 && character == '.' && !is_float {
                is_float = true;
                text.push('.');
                self.index += 1;
                continue;
            }
            if radix == 10 && matches!(character, 'e' | 'E') {
                is_float = true;
                text.push('e');
                self.index += 1;
                if matches!(self.peek(), Some('+' | '-')) {
                    if self.peek() == Some('-') {
                        text.push('-');
                    }
                    self.index += 1;
                }
                continue;
            }
            break;
        }
        if text.is_empty() {
            self.index = start;
            return None;
        }
        if is_float {
            let parsed: f64 = text.parse().ok()?;
            return Some(PyLiteral::Float(if negative { -parsed } else { parsed }));
        }
        let value = i128::from_str_radix(&text, radix).ok()?;
        Some(PyLiteral::Int(if negative { -value } else { value }))
    }
}

fn parse_python_literal(text: &str) -> Option<PyLiteral> {
    LiteralParser::parse(text)
}

/// Python `str.split()` whitespace includes the C0 information separators, which
/// the Unicode `White_Space` property does not.
fn is_python_whitespace(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

fn py_trim(text: &str) -> &str {
    text.trim_matches(is_python_whitespace)
}

// ---------------------------------------------------------------------------
// Interview answers
// ---------------------------------------------------------------------------

/// The persisted interview answers relevant to language resolution, in file
/// order, with every value already `str()`-converted exactly as the reference's
/// `CharterInterview.from_dict` does it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InterviewAnswers {
    entries: Vec<(String, String)>,
}

impl InterviewAnswers {
    /// Build from already-converted `(key, value)` pairs.
    pub fn new<I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut answers = Self::default();
        for (key, value) in entries {
            answers.insert(key, value);
        }
        answers
    }

    /// Build from the YAML `answers` mapping of an interview document.
    pub fn from_yaml_answers(answers: &Value) -> Self {
        let mut interview = Self::default();
        if let Some(mapping) = answers.as_mapping() {
            for (key, value) in mapping {
                interview.insert(python_str(key), python_str(value));
            }
        }
        interview
    }

    fn insert(&mut self, key: String, value: String) {
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| *existing == key)
        {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    /// One answer by question key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value.as_str())
    }

    /// The answers in file order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// The answer values in file order.
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(_, value)| value.as_str())
    }

    /// The number of answers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no answers.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Read `.kittify/charter/interview/answers.yaml`.
///
/// A missing, unreadable, non-UTF-8, malformed, non-mapping or non-`answers`
/// document reads as "no usable answers" — the reference's documented degrade
/// contract for this file (its encoding-ambiguity refusal is not modelled).
pub fn read_interview_answers(path: &Path) -> Option<InterviewAnswers> {
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let value = crate::language_yaml::parse_safe(&text)?;
    if value.is_null() {
        return Some(InterviewAnswers::default());
    }
    let mapping = value.as_mapping()?;
    Some(match mapping.get("answers") {
        Some(Value::Mapping(answers)) => {
            InterviewAnswers::from_yaml_answers(&Value::Mapping(answers.clone()))
        }
        _ => InterviewAnswers::default(),
    })
}

fn read_disk_interview(repo_root: Option<&Path>) -> Option<InterviewAnswers> {
    let repo_root = repo_root?;
    read_interview_answers(&repo_root.join(INTERVIEW_ANSWERS_RELATIVE_PATH))
}

/// The shipped `defaults.yaml` answer for `key` (`None` when the key is absent).
pub fn shipped_default_answer(key: &str) -> Option<&'static str> {
    SHIPPED_DEFAULT_ANSWERS
        .iter()
        .find(|(question, _)| *question == key)
        .map(|(_, answer)| *answer)
}

/// Whether every answer is the shipped `defaults.yaml` text.
fn is_packaged_default_interview(interview: &InterviewAnswers) -> bool {
    interview
        .iter()
        .all(|(key, value)| shipped_default_answer(key) == Some(value))
}

// ---------------------------------------------------------------------------
// Placeholders and the declared answer
// ---------------------------------------------------------------------------

/// Casefold-and-collapse normalisation of an answer value.
///
/// Declared edge: `to_lowercase()` stands in for Python `str.casefold()`; the
/// two are identical for ASCII and may differ for `ß` / final sigma.
fn normalize_answer(value: &str) -> String {
    value
        .to_lowercase()
        .split(is_python_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `value` declares no language: a placeholder, a sentinel, `unknown`,
/// a `[NEEDS CLARIFICATION` marker or the shipped default text.
pub fn is_placeholder_language_answer(value: &str) -> bool {
    let normalized = normalize_answer(value);
    PLACEHOLDER_ANSWERS.contains(&normalized.as_str())
        || SENTINEL_LANGUAGE_TOKENS.contains(&normalized.as_str())
        || RESERVED_LANGUAGE_TOKENS.contains(&normalized.as_str())
        || normalized.starts_with(NEEDS_CLARIFICATION_PREFIX)
        || normalized == normalize_answer(SHIPPED_DEFAULT_LANGUAGES_ANSWER)
}

/// Split a stringified answer: a list repr (`"['Zig', 'C']"`) becomes its
/// members.
fn answer_items(raw: &str) -> Vec<String> {
    let text = py_trim(raw);
    if text.starts_with('[') && text.ends_with(']') {
        match parse_python_literal(text) {
            Some(PyLiteral::List(items)) | Some(PyLiteral::Tuple(items)) => {
                return items.iter().map(py_literal_str).collect();
            }
            _ => return vec![text.to_owned()],
        }
    }
    vec![text.to_owned()]
}

/// Split a scalar answer on ``,``/``;``/` / `/``/`` and drop placeholder parts.
///
/// A whole item or comma/semicolon piece is tested before slash-splitting so
/// `n/a` stays one placeholder rather than becoming `n` and `a`.
fn real_parts(item: &str) -> Vec<String> {
    if is_placeholder_language_answer(item) {
        return Vec::new();
    }
    let mut parts = Vec::new();
    for piece in split_list_separators(item) {
        if is_placeholder_language_answer(&piece) {
            continue;
        }
        for sub in piece.split('/') {
            if !is_placeholder_language_answer(sub) {
                parts.push(py_trim(sub).to_owned());
            }
        }
    }
    parts
}

/// Split on `,`, `;` or whitespace-`/`-whitespace.
fn split_list_separators(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character == ',' || character == ';' {
            pieces.push(std::mem::take(&mut current));
            index += 1;
            continue;
        }
        if is_python_whitespace(character)
            && chars.get(index + 1) == Some(&'/')
            && chars
                .get(index + 2)
                .is_some_and(|next| is_python_whitespace(*next))
        {
            pieces.push(std::mem::take(&mut current));
            index += 3;
            continue;
        }
        current.push(character);
        index += 1;
    }
    pieces.push(current);
    pieces
}

/// The real `languages_frameworks` declaration, or `None` when it declares
/// nothing.
fn declared_language_answer(interview: &InterviewAnswers) -> Option<String> {
    let raw = interview.get(LANGUAGES_ANSWER_KEY)?;
    let real: Vec<String> = answer_items(raw)
        .into_iter()
        .flat_map(|item| real_parts(&item))
        .collect();
    let joined = real.join(", ");
    (!joined.is_empty()).then_some(joined)
}

/// Whole-word `\w` tokens of an answer, honouring the compound guard and the
/// `C#`/`C++` exclusion.
fn word_tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if !is_word_char(chars[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len() && is_word_char(chars[index]) {
            index += 1;
        }
        let end = index;
        // `(?<!\w-)`: a token preceded by `word-` is part of a longer compound.
        if start >= 2 && chars[start - 1] == '-' && is_word_char(chars[start - 2]) {
            continue;
        }
        // `(?![#+])`: `C#` / `C++`-style tokens are not words.
        if matches!(chars.get(end), Some(&'#') | Some(&'+')) {
            continue;
        }
        tokens.push(chars[start..end].iter().collect());
    }
    tokens
}

// ---------------------------------------------------------------------------
// Configured and compiled authorities
// ---------------------------------------------------------------------------

/// The operator-confirmed language list of `.kittify/config.yaml`, or `None`
/// when unset.
///
/// A pure read. Entries are lower-cased and de-duplicated; the first entry is
/// the primary language. An entry that is neither a catalog language nor a
/// family (which a declared "Gradle" resolves to) is dropped. When valid
/// entries remain they alone are the list; when none remain (or the list is
/// empty, not a list, or unreadable) the key reads as absent and detection
/// applies.
pub fn configured_languages(repo_root: &Path) -> Option<Vec<String>> {
    let path = repo_root.join(CONFIG_RELATIVE_PATH);
    if !path.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let document = crate::language_yaml::parse_safe(&text)?;
    let raw = document.as_mapping()?.get(CONFIG_LANGUAGES_KEY)?;
    let items = raw.as_sequence()?;
    if items.is_empty() {
        return None;
    }
    let mut valid: Vec<String> = Vec::new();
    for item in items {
        let token = item
            .as_str()
            .map(|text| text.trim().to_lowercase())
            .unwrap_or_default();
        if token.is_empty() {
            continue;
        }
        if (is_catalog_language(&token) || is_language_family(&token)) && !valid.contains(&token) {
            valid.push(token);
        }
    }
    if valid.is_empty() { None } else { Some(valid) }
}

/// The structured `languages` field persisted at compile time.
///
/// `None` when `charter.yaml` does not exist, is unparseable, or its `catalog`
/// section does not carry the field (a charter compiled before the field
/// existed). An empty list is `Some(vec![])` — a legitimate compiled answer, not
/// an absence signal. Items are `str()`-converted and normalised, never
/// filtered.
fn read_compiled_languages(repo_root: &Path) -> Option<Vec<String>> {
    let path = repo_root.join(sk_charter::CHARTER_YAML);
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let document = crate::language_yaml::parse(&text)?;
    let catalog = document.as_mapping()?.get("catalog")?;
    let catalog = catalog.as_mapping()?;
    let languages = catalog.get("languages")?;
    let items = languages.as_sequence()?;
    let converted: Vec<String> = items.iter().map(python_str).collect();
    Some(crate::profile::normalize_languages(&converted))
}

/// The language signal a project resolves to.
///
/// The three states are load-bearing for scoped-artifact matching; collapsing
/// them loses the difference between "no signal" (admit everything) and
/// "explicitly empty" (admit nothing scoped).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InferredLanguages {
    /// `None` — no charter/interview signal, or an answer that declares nothing.
    NoSignal,
    /// `[]` — an explicit empty compiled answer.
    Empty,
    /// A recognised list (`["python"]`, or `["unknown"]`).
    Declared(Vec<String>),
}

impl InferredLanguages {
    /// The tokens, empty for the two absence states.
    pub fn as_slice(&self) -> &[String] {
        match self {
            Self::Declared(tokens) => tokens,
            Self::NoSignal | Self::Empty => &[],
        }
    }

    /// The reference's `list[str] | None` projection.
    pub fn into_option(self) -> Option<Vec<String>> {
        match self {
            Self::NoSignal => None,
            Self::Empty => Some(Vec::new()),
            Self::Declared(tokens) => Some(tokens),
        }
    }

    /// Whether this is the no-signal state.
    pub fn is_no_signal(&self) -> bool {
        matches!(self, Self::NoSignal)
    }
}

/// One inference request.
///
/// `packs_root` is the native read seam (`SPEC_KITTY_PACKS_ROOT`, parent or
/// built-in leaf spelling); the reference reads the environment instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct InferRequest<'a> {
    /// The repository root; `None` for an in-memory compile.
    pub repo_root: Option<&'a Path>,
    /// A supplied interview (else the on-disk transcript).
    pub interview: Option<&'a InterviewAnswers>,
    /// Resolve from the interview alone (charter regeneration).
    pub prefer_interview: bool,
    /// The built-in packs root override.
    pub packs_root: Option<&'a Path>,
}

impl<'a> InferRequest<'a> {
    /// A request for `repo_root` with no supplied interview.
    pub fn new(repo_root: Option<&'a Path>) -> Self {
        Self {
            repo_root,
            ..Self::default()
        }
    }

    /// Supply an in-memory interview.
    pub fn with_interview(mut self, interview: &'a InterviewAnswers) -> Self {
        self.interview = Some(interview);
        self
    }

    /// Prefer the interview over the compiled tier.
    pub fn prefer_interview(mut self) -> Self {
        self.prefer_interview = true;
        self
    }

    /// Read the built-in tier from this packs root.
    pub fn with_packs_root(mut self, packs_root: &'a Path) -> Self {
        self.packs_root = Some(packs_root);
        self
    }
}

/// The read-only language-scope service: inference plus the memoised scoped
/// vocabulary.
#[derive(Clone, Debug, Default)]
pub struct LanguageScopeService {
    vocabulary: LanguageVocabularyCache,
}

impl LanguageScopeService {
    /// A service with the default bounded vocabulary cache.
    pub fn new() -> Self {
        Self {
            vocabulary: LanguageVocabularyCache::new(),
        }
    }

    /// A service over an explicit cache (its capacity, contents and lifetime).
    pub fn with_vocabulary_cache(cache: LanguageVocabularyCache) -> Self {
        Self { vocabulary: cache }
    }

    /// The cache behind this service.
    pub fn vocabulary_cache(&self) -> &LanguageVocabularyCache {
        &self.vocabulary
    }

    /// The cache behind this service, for an explicit invalidation.
    pub fn vocabulary_cache_mut(&mut self) -> &mut LanguageVocabularyCache {
        &mut self.vocabulary
    }

    /// Drop every memoised root (the reference's `cache_clear()`).
    pub fn clear_vocabulary_cache(&mut self) {
        self.vocabulary.clear();
    }

    /// The memoised scoped vocabulary for one root pair.
    pub fn scoped_language_vocabulary(
        &mut self,
        repo_root: Option<&Path>,
        packs_root: Option<&Path>,
    ) -> Result<Arc<BTreeSet<String>>, DoctrineError> {
        self.vocabulary.vocabulary(repo_root, packs_root)
    }

    /// Whole-word tokens of `answer` that some doctrine artifact is scoped to.
    fn vocabulary_languages(
        &mut self,
        answer: &str,
        repo_root: Option<&Path>,
        packs_root: Option<&Path>,
    ) -> Result<Vec<String>, DoctrineError> {
        let vocabulary = self.vocabulary.vocabulary(repo_root, packs_root)?;
        let matched: BTreeSet<String> = word_tokens(&answer.to_lowercase())
            .into_iter()
            .filter(|token| vocabulary.contains(token))
            .collect();
        Ok(matched.into_iter().collect())
    }

    /// Tier 2: resolve languages from the interview, the declared answer first.
    fn languages_from_interview(
        &mut self,
        repo_root: Option<&Path>,
        packs_root: Option<&Path>,
        interview: &InterviewAnswers,
    ) -> Result<InferredLanguages, DoctrineError> {
        if let Some(answer) = declared_language_answer(interview) {
            let mut recognized: BTreeSet<String> =
                extract_declared_languages(&answer).into_iter().collect();
            recognized.extend(self.vocabulary_languages(&answer, repo_root, packs_root)?);
            let mut tokens: Vec<String> = recognized.into_iter().collect();
            if tokens.is_empty() {
                tokens.push(UNKNOWN_LANGUAGE.to_owned());
            }
            return Ok(InferredLanguages::Declared(tokens));
        }
        let combined = interview.values().collect::<Vec<_>>().join("\n");
        let found = extract_declared_languages(&combined);
        Ok(if found.is_empty() {
            InferredLanguages::NoSignal
        } else {
            InferredLanguages::Declared(found)
        })
    }

    /// Infer the active project languages — the single authority for this
    /// signal.
    ///
    /// See the module documentation for the tier order and the three-state
    /// result contract. `prefer_interview` resolves from the interview alone
    /// when one is supplied or on disk, and may return
    /// [`InferredLanguages::NoSignal`].
    pub fn infer(
        &mut self,
        request: &InferRequest<'_>,
    ) -> Result<InferredLanguages, DoctrineError> {
        let repo_root = request.repo_root;
        if let Some(root) = repo_root
            && let Some(configured) = configured_languages(root)
        {
            return Ok(InferredLanguages::Declared(configured));
        }

        let mut resolved_interview = request.interview.cloned();
        if request.prefer_interview && resolved_interview.is_none() {
            resolved_interview = read_disk_interview(repo_root);
        }

        let preferred = request.prefer_interview && resolved_interview.is_some();
        if !preferred {
            let compiled = repo_root.and_then(read_compiled_languages);
            if let Some(compiled) = compiled {
                return Ok(if compiled.is_empty() {
                    InferredLanguages::Empty
                } else {
                    InferredLanguages::Declared(compiled)
                });
            }
            let needs_disk = match &resolved_interview {
                None => true,
                Some(interview) => is_packaged_default_interview(interview),
            };
            if needs_disk {
                // A supplied interview that is only the shipped default carries
                // no user answers, so it must not mask the on-disk transcript.
                resolved_interview = read_disk_interview(repo_root).or(resolved_interview);
            }
        }

        let Some(interview) = resolved_interview else {
            return Ok(InferredLanguages::NoSignal);
        };
        self.languages_from_interview(repo_root, request.packs_root, &interview)
    }

    /// Whether no installed doctrine is scoped to any of `languages`
    /// (operator decision D2).
    ///
    /// `None` (no signal) and `[]` (explicit empty) are never advisory
    /// candidates. Otherwise reserved tokens are stripped and the remainder is
    /// compared with the scoped vocabulary: `["unknown"]` and `["rust"]` (no
    /// shipped rust doctrine) lack guidance, `["python"]` does not. Families are
    /// expanded before intersecting (a `jvm`-scoped artifact is guidance for
    /// `kotlin` and `java`).
    pub fn lacks_specialist_guidance(
        &mut self,
        languages: Option<&[String]>,
        repo_root: Option<&Path>,
        packs_root: Option<&Path>,
    ) -> Result<bool, DoctrineError> {
        let Some(languages) = languages else {
            return Ok(false);
        };
        let normalized = crate::profile::normalize_languages(languages);
        if normalized.is_empty() {
            return Ok(false);
        }
        let active: BTreeSet<String> =
            expand_language_families(normalized.iter().map(String::as_str))
                .into_iter()
                .filter(|token| !RESERVED_LANGUAGE_TOKENS.contains(&token.as_str()))
                .collect();
        let vocabulary = self.vocabulary.vocabulary(repo_root, packs_root)?;
        Ok(active.is_disjoint(&vocabulary))
    }
}

/// [`LanguageScopeService::infer`] with a one-shot (uncached) service.
pub fn infer_repo_languages(
    request: &InferRequest<'_>,
) -> Result<InferredLanguages, DoctrineError> {
    LanguageScopeService::new().infer(request)
}

/// [`LanguageScopeService::lacks_specialist_guidance`] with a one-shot service.
pub fn lacks_specialist_guidance(
    languages: Option<&[String]>,
    repo_root: Option<&Path>,
    packs_root: Option<&Path>,
) -> Result<bool, DoctrineError> {
    LanguageScopeService::new().lacks_specialist_guidance(languages, repo_root, packs_root)
}

/// The five artifact kinds whose artifacts carry `applies_to_languages`.
pub fn language_scoped_kinds() -> [ArtifactKind; 5] {
    crate::language_vocabulary::LANGUAGE_SCOPED_KINDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_languages_follow_catalog_rank_and_the_build_token_rule() {
        assert_eq!(
            extract_declared_languages(
                "Python services with pytest and ruff. TypeScript with tsc."
            ),
            vec!["python", "typescript"]
        );
        assert_eq!(extract_declared_languages("Gradle"), vec![JVM_FAMILY]);
        assert_eq!(
            extract_declared_languages("Kotlin with Gradle"),
            vec!["kotlin"]
        );
        assert_eq!(
            extract_declared_languages("Java 21 with Gradle"),
            vec!["java"]
        );
        assert_eq!(
            extract_declared_languages("maven and rust"),
            vec!["rust", "jvm"]
        );
        assert!(extract_declared_languages("librespot-java").is_empty());
        assert!(extract_declared_languages("my-python-thing").is_empty());
        assert!(extract_declared_languages("React-TypeScript").is_empty());
        assert_eq!(
            extract_declared_languages("Java-based backend"),
            vec!["java"]
        );
        assert_eq!(extract_declared_languages("node.jsx"), vec!["javascript"]);
        assert_eq!(
            extract_declared_languages("x"),
            crate::profile::normalize_languages(&[])
        );
    }

    #[test]
    fn families_expand_one_way_only() {
        assert_eq!(
            expand_language_families(["kotlin"]),
            vec!["kotlin".to_owned(), JVM_FAMILY.to_owned()]
        );
        assert_eq!(
            expand_language_families([JVM_FAMILY]),
            vec![JVM_FAMILY.to_owned()]
        );
        assert_eq!(
            expand_language_families(["python", " Kotlin "]),
            vec![
                "python".to_owned(),
                "kotlin".to_owned(),
                JVM_FAMILY.to_owned()
            ]
        );
        assert!(is_language_family("JVM"));
        assert!(!is_catalog_language("jvm"));
        assert!(is_catalog_language(" TypeScript "));
    }

    #[test]
    fn placeholders_cover_the_reference_set() {
        for value in [
            "",
            "  ",
            "[]",
            "-",
            "N/A",
            "na",
            "none",
            "null",
            "TBD",
            "any",
            "ALL",
            "unknown",
            "language-agnostic",
            "language agnostic",
            "[NEEDS CLARIFICATION: which?]",
            SHIPPED_DEFAULT_LANGUAGES_ANSWER,
        ] {
            assert!(
                is_placeholder_language_answer(value),
                "{value:?} must be a placeholder"
            );
        }
        for value in ["zig", "python", "n/a, zig", "UNKNOWNISH"] {
            assert!(
                !is_placeholder_language_answer(value),
                "{value:?} must not be a placeholder"
            );
        }
    }

    #[test]
    fn answer_items_parse_a_python_list_repr() {
        assert_eq!(answer_items("['Zig', 'C']"), vec!["Zig", "C"]);
        // Only a bracketed repr is parsed; a bare tuple stays raw text.
        assert_eq!(answer_items("('Zig',)"), vec!["('Zig',)"]);
        assert_eq!(answer_items("[('Zig',)]"), vec!["('Zig',)"]);
        assert_eq!(answer_items("[1, None, True]"), vec!["1", "None", "True"]);
        // A malformed or non-list literal falls back to the raw text.
        assert_eq!(answer_items("[unterminated"), vec!["[unterminated"]);
        assert_eq!(answer_items("{'a': 1}"), vec!["{'a': 1}"]);
    }

    #[test]
    fn real_parts_drop_placeholders_and_split_on_separators() {
        assert_eq!(real_parts("n/a"), Vec::<String>::new());
        assert_eq!(real_parts("n/a, zig"), vec!["zig"]);
        assert_eq!(real_parts("python / zig"), vec!["python", "zig"]);
        assert_eq!(real_parts("python/zig"), vec!["python", "zig"]);
        assert_eq!(real_parts("none; tbd"), Vec::<String>::new());
        assert_eq!(real_parts("unknown, n/a"), Vec::<String>::new());
    }

    #[test]
    fn word_tokens_apply_the_compound_guard_and_hash_plus_exclusion() {
        assert_eq!(word_tokens("zig"), vec!["zig"]);
        assert_eq!(word_tokens("c# and c++"), vec!["and"]);
        assert_eq!(word_tokens("librespot-java"), vec!["librespot"]);
        // The compound guard drops the tail word too, exactly as `(?<!\\w-)` does.
        assert_eq!(word_tokens("java-based"), vec!["java"]);
        assert_eq!(word_tokens("日本語 2"), vec!["日本語", "2"]);
    }

    #[test]
    fn python_value_rendering_matches_str_for_the_shapes_that_matter() {
        assert_eq!(python_str(&Value::Null), "None");
        assert_eq!(python_str(&Value::Bool(false)), "False");
        assert_eq!(python_str(&Value::Number(3.into())), "3");
        assert_eq!(python_str(&Value::String("Zig".to_owned())), "Zig");
        let sequence = Value::Sequence(vec![
            Value::String("Zig".to_owned()),
            Value::Number(1.into()),
        ]);
        assert_eq!(python_str(&sequence), "['Zig', 1]");
    }

    #[test]
    fn string_repr_uses_the_canonical_kernel_formatter() {
        // The previous local approximation emitted every non-printable code point
        // raw and only escaped quote/backslash/`\n`/`\r`/`\t`; the canonical
        // formatter escapes exactly like the pinned interpreter. The full
        // Source-first matrix lives in `tests/python_repr_oracle.rs`.
        assert_eq!(python_string_repr("a\u{0}b"), "'a\\x00b'");
        assert_eq!(python_string_repr("a\u{7f}b"), "'a\\x7fb'");
        assert_eq!(python_string_repr("a\u{e000}b"), "'a\\ue000b'");
        assert_eq!(python_string_repr("a\u{1f600}b"), "'a\u{1f600}b'");
        assert_eq!(python_string_repr("a'b"), "\"a'b\"");
        assert_eq!(python_string_repr("a\\b"), "'a\\\\b'");
        assert_ne!(
            python_string_repr("a\u{0}b"),
            legacy_python_string_repr("a\u{0}b")
        );
    }

    #[test]
    fn packaged_default_interview_is_detected() {
        let shipped = InterviewAnswers::new(
            SHIPPED_DEFAULT_ANSWERS
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        assert!(is_packaged_default_interview(&shipped));
        assert!(is_packaged_default_interview(&InterviewAnswers::default()));
        let edited = InterviewAnswers::new([(LANGUAGES_ANSWER_KEY.to_owned(), "Zig".to_owned())]);
        assert!(!is_packaged_default_interview(&edited));
        // An answer for a question the shipped defaults never ask also breaks it.
        let extra = InterviewAnswers::new([("no_such_question".to_owned(), "Zig".to_owned())]);
        assert!(!is_packaged_default_interview(&extra));
    }

    #[test]
    fn shipped_defaults_match_the_reference_file() {
        assert_eq!(
            shipped_default_answer(LANGUAGES_ANSWER_KEY),
            Some(SHIPPED_DEFAULT_LANGUAGES_ANSWER)
        );
        assert_eq!(shipped_default_answer("no_such_question"), None);
        assert_eq!(SHIPPED_DEFAULT_ANSWERS.len(), 11);
    }
}
