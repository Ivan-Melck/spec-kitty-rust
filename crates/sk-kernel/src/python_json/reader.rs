//! The canonical strict CPython `json.loads` reader.
//!
//! This is the workspace's one JSON read authority. It is the **existing**
//! Invocation legacy reader (grammar, Python dict duplicate/order rules,
//! integer-digit ceiling, recursive-descent scanner) relocated here, not a
//! second parser: the Op adapter and every general caller share this module.
//!
//! Behaviour, measured against the pinned CPython 3.11.15 oracle:
//!
//! * Input is exact Python code points ([`PythonString`] or a UTF-8 [`str`]
//!   convenience). Lone surrogate units are valid Python string content, so
//!   they are preserved as values **and** object keys. A raw NUL or any other
//!   code point below `U+0020` inside a string is an invalid control character;
//!   the escaped `\u0000` form is accepted.
//! * The whole document must be exactly one JSON value surrounded by optional
//!   JSON whitespace (space, tab, LF, CR). A tolerant first *value* is not a
//!   trailing-junk licence: leftover data is `Extra data`.
//! * `NaN`, `Infinity` and `-Infinity` are accepted as bare constants
//!   (case-sensitive; `+Infinity`, `nan`, `Inf`, `-NaN` are not).
//! * Object members keep Python dict order; a duplicate key keeps its **first
//!   insertion position** and takes the **last** value, including nested maps.
//! * Integer tokens are exact (`-0` decodes to `0`) under the interpreter's
//!   startup digit ceiling; a token with `.`, `e` or `E` is a CPython `float`,
//!   so overflow is `±inf`, underflow is signed zero and rounding is
//!   correctly-rounded binary64. A bare `.`/exponent that is not followed by a
//!   digit ends the number token, exactly like the C scanner (`1.` is the
//!   integer `1` followed by `Extra data`).
//! * [`PythonJsonError`] carries the Source message and the Source position in
//!   **code points** (`char`), with `line`/`column` computed the way
//!   `json.JSONDecodeError` does. The one non-`JSONDecodeError` Source failure,
//!   the integer digit ceiling, keeps CPython's `ValueError` text and has no
//!   position.
//!
//! Deliberate, documented gaps (not Source-valid refusals on the paths that
//! matter, and never a silent drop):
//!
//! * A native operational budget of [`SOURCE_DEFAULT_RECURSION_BUDGET`] value
//!   frames bounds process stack use. CPython's own acceptance ceiling in the
//!   released caller contexts is caller-frame dependent and measured at
//!   992/991/990/989 for top-level/one/two/three Python frames, i.e. **below**
//!   this operational budget; the budget is not a JSON-format limit and not a
//!   claim of exact emulation of a customized interpreter recursion limit.
//! * `json.loads(bytes)` decodes UTF-8/UTF-16/UTF-32 with
//!   `errors="surrogatepass"` before scanning; that bytes-to-`str` phase is
//!   caller-specific (the Op reader uses UTF-8 `read_text` semantics), so this
//!   reader starts at the `str` phase. It does not invent a new UTF-8 refusal:
//!   callers that already hold a [`PythonString`] keep every code unit.

use super::{PythonJsonDict, PythonJsonValue};
use crate::PythonString;

/// Operational value-frame budget for a recursive scan.
///
/// The reference interpreter's default recursion budget is 1000 and its
/// measured acceptance ceiling in released caller contexts is ~989–994, so this
/// guard never refuses an input the pinned Source accepts there. It is not a
/// JSON-format depth rule and not an exact emulation of customized interpreter
/// recursion settings.
pub const SOURCE_DEFAULT_RECURSION_BUDGET: usize = 1000;

/// CPython's default `sys.get_int_max_str_digits()` value.
pub const SOURCE_DEFAULT_INTEGER_DIGITS: usize = 4300;

/// The category of a refused document, each with the reference message text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PythonJsonErrorKind {
    /// `Expecting value`
    ExpectingValue,
    /// `Expecting ',' delimiter`
    ExpectingDelimiter,
    /// `Expecting ':' delimiter`
    ExpectingColon,
    /// `Expecting property name enclosed in double quotes`
    ExpectingPropertyName,
    /// `Extra data`
    ExtraData,
    /// `Unterminated string starting at`
    UnterminatedString,
    /// `Invalid \escape`
    InvalidEscape,
    /// `Invalid \uXXXX escape`
    InvalidUnicodeEscape,
    /// `Invalid control character at`
    InvalidControlCharacter,
    /// `Unexpected UTF-8 BOM (decode using utf-8-sig)`
    UnexpectedBom,
    /// The interpreter's integer conversion ceiling (`ValueError`, no position).
    IntegerDigitLimit,
    /// The startup `PYTHONINTMAXSTRDIGITS` value could not configure an
    /// interpreter that would have started (no Source JSON phase is reached).
    IntegerDigitLimitSetting,
    /// The native operational frame budget was exhausted (never a JSON-format
    /// claim; CPython reports `RecursionError` here).
    RecursionBudget,
    /// A grammar-valid float token that the platform float parser refused.
    NumberConversion,
}

impl PythonJsonErrorKind {
    /// The reference message for this refusal.
    pub fn message(self) -> &'static str {
        match self {
            Self::ExpectingValue => "Expecting value",
            Self::ExpectingDelimiter => "Expecting ',' delimiter",
            Self::ExpectingColon => "Expecting ':' delimiter",
            Self::ExpectingPropertyName => "Expecting property name enclosed in double quotes",
            Self::ExtraData => "Extra data",
            Self::UnterminatedString => "Unterminated string starting at",
            Self::InvalidEscape => "Invalid \\escape",
            Self::InvalidUnicodeEscape => "Invalid \\uXXXX escape",
            Self::InvalidControlCharacter => "Invalid control character at",
            Self::UnexpectedBom => "Unexpected UTF-8 BOM (decode using utf-8-sig)",
            Self::IntegerDigitLimit => {
                "Exceeds the limit for integer string conversion: use sys.set_int_max_str_digits() to increase the limit"
            }
            Self::IntegerDigitLimitSetting => {
                "invalid PYTHONINTMAXSTRDIGITS startup setting (must be 0 or at least 640)"
            }
            Self::RecursionBudget => {
                "native operational JSON frame budget exhausted (not a Source format rule)"
            }
            Self::NumberConversion => "could not convert a JSON number token",
        }
    }
}

/// A refused document with the reference message and position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PythonJsonError {
    kind: PythonJsonErrorKind,
    pos: Option<usize>,
    line: Option<usize>,
    column: Option<usize>,
    digits: usize,
    limit: usize,
}

impl PythonJsonError {
    fn at(kind: PythonJsonErrorKind, units: &[u32], pos: usize) -> Self {
        let (line, column) = source_line_column(units, pos);
        Self {
            kind,
            pos: Some(pos),
            line: Some(line),
            column: Some(column),
            digits: 0,
            limit: 0,
        }
    }

    fn integer_digit_limit(limit: usize, digits: usize) -> Self {
        Self {
            kind: PythonJsonErrorKind::IntegerDigitLimit,
            pos: None,
            line: None,
            column: None,
            digits,
            limit,
        }
    }

    fn setting() -> Self {
        Self {
            kind: PythonJsonErrorKind::IntegerDigitLimitSetting,
            pos: None,
            line: None,
            column: None,
            digits: 0,
            limit: 0,
        }
    }

    /// The refusal category.
    pub fn kind(&self) -> PythonJsonErrorKind {
        self.kind
    }

    /// The Source position in code points (`JSONDecodeError.pos`), when the
    /// failure has one.
    pub fn pos(&self) -> Option<usize> {
        self.pos
    }

    /// The Source 1-based line (`JSONDecodeError.lineno`).
    pub fn line(&self) -> Option<usize> {
        self.line
    }

    /// The Source 1-based column (`JSONDecodeError.colno`).
    pub fn column(&self) -> Option<usize> {
        self.column
    }

    /// The integer digit ceiling and the observed digit count, for
    /// [`PythonJsonErrorKind::IntegerDigitLimit`].
    pub fn integer_digits(&self) -> Option<(usize, usize)> {
        (self.kind == PythonJsonErrorKind::IntegerDigitLimit).then_some((self.limit, self.digits))
    }

    /// The reference message text (dynamic only for the digit ceiling).
    pub fn message(&self) -> String {
        if self.kind == PythonJsonErrorKind::IntegerDigitLimit {
            format!(
                "Exceeds the limit ({} digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
                self.limit, self.digits
            )
        } else {
            self.kind.message().to_owned()
        }
    }
}

impl std::fmt::Display for PythonJsonError {
    /// `str(JSONDecodeError)`: `msg: line L column C (char P)`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.pos, self.line, self.column) {
            (Some(pos), Some(line), Some(column)) => write!(
                formatter,
                "{}: line {line} column {column} (char {pos})",
                self.message()
            ),
            _ => formatter.write_str(&self.message()),
        }
    }
}

impl std::error::Error for PythonJsonError {}

/// Parse one complete document (UTF-8 text convenience).
///
/// Reads `PYTHONINTMAXSTRDIGITS` the way interpreter startup does; use
/// [`parse_str_with_integer_digits`] for an explicit ceiling.
pub fn parse(text: &str) -> Result<PythonJsonValue, PythonJsonError> {
    parse_str_with_integer_digits(text, environment_integer_digits()?)
}

/// Parse one complete document (exact Python code points).
///
/// The exact-input path: code points that a Rust `String` cannot carry (lone
/// surrogates) are valid here, because `json.loads` accepts them in a `str`.
pub fn parse_python_string(text: &PythonString) -> Result<PythonJsonValue, PythonJsonError> {
    parse_python_string_with_integer_digits(text, environment_integer_digits()?)
}

/// Parse one complete UTF-8 document with an explicit integer digit ceiling.
pub fn parse_str_with_integer_digits(
    text: &str,
    integer_digits: usize,
) -> Result<PythonJsonValue, PythonJsonError> {
    let units: Vec<u32> = text.chars().map(u32::from).collect();
    parse_units(&units, integer_digits)
}

/// Parse one complete exact-code-point document with an explicit integer ceiling.
pub fn parse_python_string_with_integer_digits(
    text: &PythonString,
    integer_digits: usize,
) -> Result<PythonJsonValue, PythonJsonError> {
    parse_units(text.codepoints(), integer_digits)
}

/// The startup-equivalent integer conversion ceiling.
///
/// `None` (unset) is CPython's default 4300, empty means default, `0` means
/// unlimited and any other value must be at least 640. Leading ASCII whitespace
/// is accepted and trailing data is not, matching interpreter startup.
pub fn source_integer_digit_limit(setting: Option<&str>) -> Result<usize, PythonJsonError> {
    let Some(setting) = setting.filter(|value| !value.is_empty()) else {
        return Ok(SOURCE_DEFAULT_INTEGER_DIGITS);
    };
    let value = setting.trim_start_matches([' ', '\t', '\n', '\r', '\u{0b}', '\u{0c}']);
    let limit = value
        .parse::<i32>()
        .map_err(|_| PythonJsonError::setting())?;
    if limit == 0 || limit >= 640 {
        usize::try_from(limit).map_err(|_| PythonJsonError::setting())
    } else {
        Err(PythonJsonError::setting())
    }
}

/// The ceiling configured for this process, or the startup failure.
pub fn environment_integer_digits() -> Result<usize, PythonJsonError> {
    let setting = match std::env::var("PYTHONINTMAXSTRDIGITS") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => return Err(PythonJsonError::setting()),
    };
    source_integer_digit_limit(setting.as_deref())
}

fn parse_units(units: &[u32], integer_digits: usize) -> Result<PythonJsonValue, PythonJsonError> {
    // `json.loads(str)` refuses a leading BOM before scanning.
    if units.first() == Some(&u32::from('\u{feff}')) {
        return Err(PythonJsonError::at(
            PythonJsonErrorKind::UnexpectedBom,
            units,
            0,
        ));
    }
    let mut reader = Reader {
        text: units,
        index: 0,
        depth: 0,
        integer_digits,
    };
    let value = reader.value()?;
    reader.skip_whitespace();
    if reader.index != units.len() {
        return Err(reader.error(PythonJsonErrorKind::ExtraData));
    }
    Ok(value)
}

/// `JSONDecodeError.lineno`/`.colno` from a code-point position.
fn source_line_column(units: &[u32], pos: usize) -> (usize, usize) {
    let newline = u32::from('\n');
    let line = units[..pos].iter().filter(|&&unit| unit == newline).count() + 1;
    let column = match units[..pos].iter().rposition(|&unit| unit == newline) {
        Some(last) => pos - last,
        None => pos + 1,
    };
    (line, column)
}

const fn ascii(character: u8) -> u32 {
    character as u32
}

fn is_digit(unit: u32) -> bool {
    (ascii(b'0')..=ascii(b'9')).contains(&unit)
}

fn hex_digit(unit: u32) -> Option<u32> {
    match unit {
        unit if (ascii(b'0')..=ascii(b'9')).contains(&unit) => Some(unit - ascii(b'0')),
        unit if (ascii(b'a')..=ascii(b'f')).contains(&unit) => Some(unit - ascii(b'a') + 10),
        unit if (ascii(b'A')..=ascii(b'F')).contains(&unit) => Some(unit - ascii(b'A') + 10),
        _ => None,
    }
}

/// The recursive-descent CPython `json.loads` lexical scanner.
struct Reader<'a> {
    text: &'a [u32],
    index: usize,
    depth: usize,
    integer_digits: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u32> {
        self.text.get(self.index).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u32> {
        self.text.get(self.index + offset).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(
            self.peek(),
            Some(unit)
                if unit == ascii(b' ') || unit == ascii(b'\t') || unit == ascii(b'\n') || unit == ascii(b'\r')
        ) {
            self.index += 1;
        }
    }

    fn error(&self, kind: PythonJsonErrorKind) -> PythonJsonError {
        PythonJsonError::at(kind, self.text, self.index)
    }

    fn matches_at(&self, word: &str) -> bool {
        let remaining = self.text.len().saturating_sub(self.index);
        remaining >= word.len()
            && word
                .chars()
                .zip(&self.text[self.index..])
                .all(|(expected, &actual)| u32::from(expected) == actual)
    }

    /// One value, enforcing the native operational frame budget.
    fn value(&mut self) -> Result<PythonJsonValue, PythonJsonError> {
        self.skip_whitespace();
        if self.depth >= SOURCE_DEFAULT_RECURSION_BUDGET {
            return Err(self.error(PythonJsonErrorKind::RecursionBudget));
        }
        self.depth += 1;
        // Any error discards the whole document, so only successful returns
        // need the counter restored for a sibling value.
        let result = self.value_inner();
        self.depth -= 1;
        result
    }

    fn value_inner(&mut self) -> Result<PythonJsonValue, PythonJsonError> {
        match self.peek() {
            None => Err(self.error(PythonJsonErrorKind::ExpectingValue)),
            Some(unit) if unit == ascii(b'n') => self.literal("null", PythonJsonValue::Null),
            Some(unit) if unit == ascii(b't') => self.literal("true", PythonJsonValue::Bool(true)),
            Some(unit) if unit == ascii(b'f') => {
                self.literal("false", PythonJsonValue::Bool(false))
            }
            Some(unit) if unit == ascii(b'N') => {
                self.constant("NaN", PythonJsonValue::Float(f64::NAN))
            }
            Some(unit) if unit == ascii(b'I') => {
                self.constant("Infinity", PythonJsonValue::Float(f64::INFINITY))
            }
            Some(unit) if unit == ascii(b'-') && self.matches_at("-Infinity") => {
                self.index += "-Infinity".len();
                Ok(PythonJsonValue::Float(f64::NEG_INFINITY))
            }
            Some(unit) if unit == ascii(b'-') || is_digit(unit) => self.number(),
            Some(unit) if unit == ascii(b'[') => self.array(),
            Some(unit) if unit == ascii(b'{') => self.object(),
            Some(unit) if unit == ascii(b'"') => self.string().map(PythonJsonValue::String),
            Some(_) => Err(self.error(PythonJsonErrorKind::ExpectingValue)),
        }
    }

    /// `null`/`true`/`false`: the C scanner requires every character to be
    /// present, otherwise the value never starts (`Expecting value` at start).
    fn literal(
        &mut self,
        word: &str,
        value: PythonJsonValue,
    ) -> Result<PythonJsonValue, PythonJsonError> {
        if !self.matches_at(word) {
            return Err(self.error(PythonJsonErrorKind::ExpectingValue));
        }
        self.index += word.len();
        Ok(value)
    }

    /// `NaN`/`Infinity`: the `parse_constant` default accepts them verbatim.
    fn constant(
        &mut self,
        word: &str,
        value: PythonJsonValue,
    ) -> Result<PythonJsonValue, PythonJsonError> {
        self.literal(word, value)
    }

    fn number(&mut self) -> Result<PythonJsonValue, PythonJsonError> {
        let start = self.index;
        if self.peek() == Some(ascii(b'-')) {
            self.index += 1;
        }
        match self.peek() {
            Some(unit) if unit == ascii(b'0') => self.index += 1,
            Some(unit) if (ascii(b'1')..=ascii(b'9')).contains(&unit) => {
                self.index += 1;
                while self.peek().is_some_and(is_digit) {
                    self.index += 1;
                }
            }
            _ => {
                // The C scanner reports the value start, not the byte after '-'.
                self.index = start;
                return Err(self.error(PythonJsonErrorKind::ExpectingValue));
            }
        }
        let mut is_float = false;
        // The C scanner only consumes `.` when a digit follows it.
        if self.peek() == Some(ascii(b'.')) && self.peek_at(1).is_some_and(is_digit) {
            is_float = true;
            self.index += 2;
            while self.peek().is_some_and(is_digit) {
                self.index += 1;
            }
        }
        // A bare exponent is not consumed either (`1e` is the integer 1 plus
        // `Extra data` at the `e`).
        if matches!(self.peek(), Some(unit) if unit == ascii(b'e') || unit == ascii(b'E')) {
            let exponent = self.index;
            self.index += 1;
            if matches!(self.peek(), Some(unit) if unit == ascii(b'+') || unit == ascii(b'-')) {
                self.index += 1;
            }
            if self.peek().is_some_and(is_digit) {
                is_float = true;
                while self.peek().is_some_and(is_digit) {
                    self.index += 1;
                }
            } else {
                self.index = exponent;
            }
        }
        let token = self.ascii_token(start, self.index);
        if is_float {
            // CPython `float` and Rust `f64` share the JSON number grammar and
            // correctly-rounded parsing: overflow is `±inf`, underflow keeps a
            // signed zero. Integer tokens never pass through `f64`.
            return token
                .parse::<f64>()
                .map(PythonJsonValue::Float)
                .map_err(|_| self.error(PythonJsonErrorKind::NumberConversion));
        }
        let digits = token.strip_prefix('-').unwrap_or(&token).len();
        if self.integer_digits != 0 && digits > self.integer_digits {
            // CPython's ceiling counts digits, not the sign; float
            // mantissas/exponents do not have this integer-only ceiling.
            return Err(PythonJsonError::integer_digit_limit(
                self.integer_digits,
                digits,
            ));
        }
        if token == "-0" {
            Ok(PythonJsonValue::Integer("0".to_owned()))
        } else {
            Ok(PythonJsonValue::Integer(token))
        }
    }

    /// A grammar-validated ASCII number token as Rust text.
    fn ascii_token(&self, start: usize, end: usize) -> String {
        self.text[start..end]
            .iter()
            .map(|&unit| char::from_u32(unit).expect("number tokens are ASCII"))
            .collect()
    }

    fn string(&mut self) -> Result<PythonString, PythonJsonError> {
        let opening = self.index;
        self.index += 1;
        let mut out = PythonString::default();
        loop {
            let Some(unit) = self.peek() else {
                return Err(PythonJsonError::at(
                    PythonJsonErrorKind::UnterminatedString,
                    self.text,
                    opening,
                ));
            };
            if unit == ascii(b'"') {
                self.index += 1;
                return Ok(out);
            }
            if unit == ascii(b'\\') {
                self.index += 1;
                self.escape(&mut out, opening)?;
                continue;
            }
            if unit < ascii(b' ') {
                return Err(self.error(PythonJsonErrorKind::InvalidControlCharacter));
            }
            out.push_codepoint(unit)
                .expect("input code units are valid Python code points");
            self.index += 1;
        }
    }

    fn escape(&mut self, out: &mut PythonString, opening: usize) -> Result<(), PythonJsonError> {
        let backslash = self.index - 1;
        let Some(unit) = self.peek() else {
            // `json.loads('"\\')` reports the string as unterminated.
            return Err(PythonJsonError::at(
                PythonJsonErrorKind::UnterminatedString,
                self.text,
                opening,
            ));
        };
        let escaped = match unit {
            unit if unit == ascii(b'"') => 0x22,
            unit if unit == ascii(b'\\') => 0x5c,
            unit if unit == ascii(b'/') => 0x2f,
            unit if unit == ascii(b'b') => 0x08,
            unit if unit == ascii(b'f') => 0x0c,
            unit if unit == ascii(b'n') => 0x0a,
            unit if unit == ascii(b'r') => 0x0d,
            unit if unit == ascii(b't') => 0x09,
            unit if unit == ascii(b'u') => return self.unicode_escape(out),
            _ => {
                return Err(PythonJsonError::at(
                    PythonJsonErrorKind::InvalidEscape,
                    self.text,
                    backslash,
                ));
            }
        };
        self.index += 1;
        push_unit(out, escaped);
        Ok(())
    }

    /// One `\uXXXX` escape with CPython's exact surrogate rule.
    ///
    /// A high surrogate combines with an immediately following low-surrogate
    /// escape into one astral scalar. A malformed second escape is still an
    /// "Invalid \uXXXX escape", but a *valid* non-low second unit is not
    /// consumed: the lone high surrogate is kept and the following escape is
    /// scanned normally, so `"\uD800\u0041"` is two code points. Low surrogates
    /// are always kept as lone units, and pairing is greedy from left to right
    /// (`"\uD800\uD800\uDC00"` is `[U+D800, U+10000]`).
    fn unicode_escape(&mut self, out: &mut PythonString) -> Result<(), PythonJsonError> {
        // `self.index` still points at the `u`; the C scanner reports escape
        // failures at the `u`, not at the backslash or the bad digit.
        let first_u = self.index;
        let first = self.hex4(first_u)?;
        self.index = first_u + 5;
        if (0xd800..=0xdbff).contains(&first)
            && self.peek() == Some(ascii(b'\\'))
            && self.peek_at(1) == Some(ascii(b'u'))
        {
            let second_u = self.index + 1;
            let second = self.hex4(second_u)?;
            if (0xdc00..=0xdfff).contains(&second) {
                self.index = second_u + 5;
                push_unit(out, 0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00));
                return Ok(());
            }
        }
        push_unit(out, first);
        Ok(())
    }

    /// Four hex digits encoding one code unit, or the Source escape error.
    fn hex4(&self, unit_index: usize) -> Result<u32, PythonJsonError> {
        let mut value = 0u32;
        for offset in 1..=4 {
            let digit = self
                .text
                .get(unit_index + offset)
                .copied()
                .and_then(hex_digit)
                .ok_or_else(|| {
                    PythonJsonError::at(
                        PythonJsonErrorKind::InvalidUnicodeEscape,
                        self.text,
                        unit_index,
                    )
                })?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn array(&mut self) -> Result<PythonJsonValue, PythonJsonError> {
        self.index += 1;
        self.skip_whitespace();
        let mut items = Vec::new();
        if self.peek() == Some(ascii(b']')) {
            self.index += 1;
            return Ok(PythonJsonValue::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(unit) if unit == ascii(b',') => self.index += 1,
                Some(unit) if unit == ascii(b']') => {
                    self.index += 1;
                    return Ok(PythonJsonValue::Array(items));
                }
                _ => return Err(self.error(PythonJsonErrorKind::ExpectingDelimiter)),
            }
        }
    }

    fn object(&mut self) -> Result<PythonJsonValue, PythonJsonError> {
        self.index += 1;
        self.skip_whitespace();
        let mut object = PythonJsonDict::new();
        if self.peek() == Some(ascii(b'}')) {
            self.index += 1;
            return Ok(PythonJsonValue::ExactObject(object));
        }
        loop {
            self.skip_whitespace();
            // JSON object keys are strings; Python rejects anything else.
            if self.peek() != Some(ascii(b'"')) {
                return Err(self.error(PythonJsonErrorKind::ExpectingPropertyName));
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(ascii(b':')) {
                return Err(self.error(PythonJsonErrorKind::ExpectingColon));
            }
            self.index += 1;
            let value = self.value()?;
            object.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(unit) if unit == ascii(b',') => self.index += 1,
                Some(unit) if unit == ascii(b'}') => {
                    self.index += 1;
                    return Ok(PythonJsonValue::ExactObject(object));
                }
                _ => return Err(self.error(PythonJsonErrorKind::ExpectingDelimiter)),
            }
        }
    }
}

/// Append one decoded parser unit; parser input is already in range.
fn push_unit(out: &mut PythonString, code: u32) {
    out.push_codepoint(code)
        .expect("decoded JSON string units are valid Python code points");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(codepoints: &[u32]) -> PythonString {
        PythonString::from_codepoints(codepoints).expect("fixture code points")
    }

    #[test]
    fn integer_startup_configuration_matches_pinned_cpython_facts() {
        assert_eq!(source_integer_digit_limit(None), Ok(4300));
        for value in ["", "0", "640", "+640", "-0", " 640", "000640"] {
            let expected = if value.is_empty() {
                4300
            } else if value == "0" || value == "-0" {
                0
            } else {
                640
            };
            assert_eq!(
                source_integer_digit_limit(Some(value)),
                Ok(expected),
                "{value:?}"
            );
        }
        for value in ["640 ", "639", "bad", "2147483648"] {
            assert!(
                source_integer_digit_limit(Some(value)).is_err(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn duplicate_keys_keep_the_first_position_and_the_last_value() {
        let value = parse_python_string_with_integer_digits(
            &text(&r#"{"p": {"a": 1, "b": 2, "a": 3}}"#.chars().map(u32::from).collect::<Vec<_>>()),
            4300,
        )
        .expect("valid object");
        let PythonJsonValue::ExactObject(object) = value else {
            panic!("object expected");
        };
        let Some(PythonJsonValue::ExactObject(nested)) = object.get("p") else {
            panic!("nested object expected");
        };
        assert_eq!(
            nested
                .keys()
                .map(|key| key.to_scalar_string().expect("scalar key"))
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(
            nested.get("a"),
            Some(&PythonJsonValue::Integer("3".to_owned()))
        );
    }

    #[test]
    fn any_root_value_is_accepted_but_trailing_data_is_not() {
        for document in ["null", "true", "false", "[]", "{}", "1", "-0.5", "\"x\""] {
            assert!(parse(document).is_ok(), "{document}");
            assert!(parse(&format!(" {document} \t\r\n")).is_ok(), "{document}");
        }
        for document in ["1 2", "{}[]", "null x", "\"a\"\"b\""] {
            let error = parse(document).expect_err(document);
            assert_eq!(error.kind(), PythonJsonErrorKind::ExtraData, "{document}");
            assert_eq!(
                error.to_string(),
                format!(
                    "Extra data: line 1 column {} (char {})",
                    error.pos().unwrap() + 1,
                    error.pos().unwrap()
                )
            );
        }
    }

    #[test]
    fn positions_are_code_points_not_utf8_bytes() {
        let error = parse("\"𝄞\" x").expect_err("extra data");
        assert_eq!(error.pos(), Some(4));
        assert_eq!(error.line(), Some(1));
        assert_eq!(error.column(), Some(5));
        let error = parse("\"é\"\t\n  y").expect_err("extra data");
        assert_eq!(
            (error.pos(), error.line(), error.column()),
            (Some(7), Some(2), Some(3))
        );
    }

    #[test]
    fn surrogate_escapes_pair_and_lone_units_survive() {
        let document = text(&"[\"\\ud800\", \"\\udc80\", \"\\ud800\\u0041\", \"\\ud800\\ud800\\udc00\", \"\\ud83d\\ude00\"]".chars().map(u32::from).collect::<Vec<_>>());
        let PythonJsonValue::Array(items) =
            parse_python_string_with_integer_digits(&document, 4300).expect("valid")
        else {
            panic!("array expected");
        };
        let units: Vec<Vec<u32>> = items
            .iter()
            .map(|item| match item {
                PythonJsonValue::String(text) => text.codepoints().to_vec(),
                other => panic!("string expected: {other:?}"),
            })
            .collect();
        assert_eq!(
            units,
            vec![
                vec![0xd800],
                vec![0xdc80],
                vec![0xd800, 0x41],
                vec![0xd800, 0x10000],
                vec![0x1f600]
            ]
        );
    }

    #[test]
    fn raw_surrogate_input_units_round_trip_through_the_exact_path() {
        let raw = text(&[0x22, 0xd800, 0x22]);
        let PythonJsonValue::String(value) =
            parse_python_string_with_integer_digits(&raw, 4300).expect("Source accepts raw units")
        else {
            panic!("string expected");
        };
        assert_eq!(value.codepoints(), &[0xd800]);
    }

    #[test]
    fn integer_ceiling_reports_the_source_value_error_text() {
        let digits = "1".repeat(4301);
        let error = parse(&digits).expect_err("ceiling");
        assert_eq!(error.kind(), PythonJsonErrorKind::IntegerDigitLimit);
        assert_eq!(error.pos(), None);
        assert_eq!(
            error.message(),
            "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit"
        );
        assert_eq!(error.integer_digits(), Some((4300, 4301)));
        assert!(parse(&format!("-{}", "1".repeat(4300))).is_ok());
        assert!(
            parse_str_with_integer_digits(&digits, 0).is_ok(),
            "0 is unlimited"
        );
        assert!(parse_str_with_integer_digits(&"1".repeat(640), 640).is_ok());
        assert!(parse_str_with_integer_digits(&"1".repeat(641), 640).is_err());
    }

    #[test]
    fn bom_is_a_source_error_only_at_the_document_start() {
        let error = parse("\u{feff}{}").expect_err("bom");
        assert_eq!(error.kind(), PythonJsonErrorKind::UnexpectedBom);
        assert_eq!(
            error.to_string(),
            "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)"
        );
        assert!(parse("{}\u{feff}").is_err());
    }

    #[test]
    fn error_categories_and_positions_match_the_measured_source_scanner() {
        for (document, kind, pos) in [
            ("", PythonJsonErrorKind::ExpectingValue, 0),
            (" ", PythonJsonErrorKind::ExpectingValue, 1),
            ("x", PythonJsonErrorKind::ExpectingValue, 0),
            ("tru", PythonJsonErrorKind::ExpectingValue, 0),
            ("truex", PythonJsonErrorKind::ExtraData, 4),
            ("-", PythonJsonErrorKind::ExpectingValue, 0),
            ("- ", PythonJsonErrorKind::ExpectingValue, 0),
            (".5", PythonJsonErrorKind::ExpectingValue, 0),
            ("1.", PythonJsonErrorKind::ExtraData, 1),
            ("1e", PythonJsonErrorKind::ExtraData, 1),
            ("1e+", PythonJsonErrorKind::ExtraData, 1),
            ("01", PythonJsonErrorKind::ExtraData, 1),
            ("-01", PythonJsonErrorKind::ExtraData, 2),
            ("[1 2]", PythonJsonErrorKind::ExpectingDelimiter, 3),
            ("[1,]", PythonJsonErrorKind::ExpectingValue, 3),
            ("{\"a\": 1,}", PythonJsonErrorKind::ExpectingPropertyName, 8),
            ("{\"a\" 1}", PythonJsonErrorKind::ExpectingColon, 5),
            ("{", PythonJsonErrorKind::ExpectingPropertyName, 1),
            ("[", PythonJsonErrorKind::ExpectingValue, 1),
            ("\"abc", PythonJsonErrorKind::UnterminatedString, 0),
            ("\"a\\qb\"", PythonJsonErrorKind::InvalidEscape, 2),
            ("\"a\nb\"", PythonJsonErrorKind::InvalidControlCharacter, 2),
            ("\"\u{0}\"", PythonJsonErrorKind::InvalidControlCharacter, 1),
            ("\"\\uZZZZ\"", PythonJsonErrorKind::InvalidUnicodeEscape, 2),
            (
                "\"\\ud800\\uZZZZ\"",
                PythonJsonErrorKind::InvalidUnicodeEscape,
                8,
            ),
            ("NaNx", PythonJsonErrorKind::ExtraData, 3),
            ("+Infinity", PythonJsonErrorKind::ExpectingValue, 0),
            ("-NaN", PythonJsonErrorKind::ExpectingValue, 0),
            ("nan", PythonJsonErrorKind::ExpectingValue, 0),
        ] {
            let error = parse(document).expect_err(document);
            assert_eq!(error.kind(), kind, "{document:?}");
            assert_eq!(error.pos(), Some(pos), "{document:?}");
        }
    }
}
