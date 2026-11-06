//! Strict CPython UTF-8 codec outcomes over the canonical Rust UTF-8 decoder.
//! Byte decoding and encoding are separate from TextIO universal-newline
//! translation and JSON/YAML parsing; callers must not erase those failure-phase
//! distinctions. The encoder is the one strict `str.encode('utf-8')` authority
//! for writer policies (no private per-consumer encoder, no lossy replacement).

/// CPython `UnicodeDecodeError` data for strict `utf-8` decoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonUtf8DecodeError {
    object: Vec<u8>,
    /// First byte of the rejected sequence (inclusive).
    pub start: usize,
    /// End of the rejected sequence (exclusive).
    pub end: usize,
    /// CPython's decoding reason, not Rust's diagnostic text.
    pub reason: &'static str,
}
impl PythonUtf8DecodeError {
    /// The bytes CPython attached to the exception. For `utf-8-sig`, an initial
    /// BOM has already been removed by that codec's delegating decoder.
    pub fn object(&self) -> &[u8] {
        &self.object
    }
    /// The underlying decoder name (also `utf-8` for a sig decoder error).
    pub const fn encoding(&self) -> &'static str {
        "utf-8"
    }
}
impl std::fmt::Display for PythonUtf8DecodeError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.end == self.start + 1 {
            write!(
                out,
                "'utf-8' codec can't decode byte 0x{:02x} in position {}: {}",
                self.object[self.start], self.start, self.reason
            )
        } else {
            write!(
                out,
                "'utf-8' codec can't decode bytes in position {}-{}: {}",
                self.start,
                self.end - 1,
                self.reason
            )
        }
    }
}
impl std::error::Error for PythonUtf8DecodeError {}

/// Strict `bytes.decode('utf-8')`, preserving the exact failure object/range.
/// Successful decoding borrows the original bytes; it never drops/replaces a
/// scalar, silently strips a BOM, or translates newlines.
pub fn decode_utf8(bytes: &[u8]) -> Result<&str, PythonUtf8DecodeError> {
    std::str::from_utf8(bytes).map_err(|error| {
        let start = error.valid_up_to();
        let (end, reason) = match error.error_len() {
            None => (bytes.len(), "unexpected end of data"),
            Some(length) => (
                start + length,
                if (0xc2..=0xf4).contains(&bytes[start]) {
                    "invalid continuation byte"
                } else {
                    "invalid start byte"
                },
            ),
        };
        PythonUtf8DecodeError {
            object: bytes.to_vec(),
            start,
            end,
            reason,
        }
    })
}

/// Strict `bytes.decode('utf-8-sig')`: strip exactly one initial UTF-8 BOM,
/// then delegate to the same strict decoder/error authority.
pub fn decode_utf8_sig(bytes: &[u8]) -> Result<&str, PythonUtf8DecodeError> {
    decode_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
}

/// CPython `UnicodeEncodeError` data for strict `utf-8` encoding.
///
/// The object is the code-unit sequence CPython attached to the exception (a
/// Python `str` is code units, not bytes), so a lone surrogate that has no UTF-8
/// form is retained verbatim instead of being replaced. This is the encode-side
/// sibling of [`PythonUtf8DecodeError`]; both keep the reference's
/// `start`/`end`/`reason` and its exact message text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PythonUtf8EncodeError {
    object: Vec<u32>,
    /// First code unit of the rejected sequence (inclusive).
    pub start: usize,
    /// End of the rejected sequence (exclusive).
    pub end: usize,
    /// CPython's encoding reason, not Rust's diagnostic text.
    pub reason: &'static str,
}
impl PythonUtf8EncodeError {
    /// The code units CPython attached to the exception.
    pub fn object(&self) -> &[u32] {
        &self.object
    }
    /// The encoder name.
    pub const fn encoding(&self) -> &'static str {
        "utf-8"
    }
}
impl std::fmt::Display for PythonUtf8EncodeError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.end == self.start + 1 {
            write!(
                out,
                "'utf-8' codec can't encode character '\\u{:04x}' in position {}: {}",
                self.object[self.start], self.start, self.reason
            )
        } else {
            write!(
                out,
                "'utf-8' codec can't encode characters in position {}-{}: {}",
                self.start,
                self.end - 1,
                self.reason
            )
        }
    }
}
impl std::error::Error for PythonUtf8EncodeError {}

/// Strict `str.encode('utf-8')` over exact CPython code units.
///
/// Python string code units are validated by [`crate::PythonString`]
/// (`0..=U+10FFFF`). Only a surrogate unit has no UTF-8 form; it is refused with
/// CPython's `surrogates not allowed` reason and its exact position range.
/// Nothing is replaced, truncated or silently dropped, and no BOM/newline
/// translation is applied (that belongs to TextIO, not to this encoder).
///
/// The reported range is CPython's, not the first offending unit alone: the
/// encoder refuses the maximal run of consecutive surrogate units, so
/// three consecutive surrogates report `start=0, end=3` ("can't encode
/// characters in position 0-2") while a lone surrogate between encodable units
/// reports `start=position, end=position+1` ("can't encode character
/// '\\ud800' in position N").
pub fn encode_python_string(value: &crate::PythonString) -> Result<Vec<u8>, PythonUtf8EncodeError> {
    let units = value.codepoints();
    let mut out = Vec::with_capacity(units.len());
    for (position, &unit) in units.iter().enumerate() {
        match char::from_u32(unit) {
            Some(character) => {
                let mut buffer = [0u8; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            None => {
                // CPython reports the whole run of consecutive unencodable units.
                let mut end = position + 1;
                while end < units.len() && char::from_u32(units[end]).is_none() {
                    end += 1;
                }
                return Err(PythonUtf8EncodeError {
                    object: units.to_vec(),
                    start: position,
                    end,
                    reason: "surrogates not allowed",
                });
            }
        }
    }
    Ok(out)
}

/// Strict `str.encode('utf-8')` over Rust text.
///
/// Every Unicode scalar has a UTF-8 form, so this cannot fail; it is the one
/// encoder every text input of the writer policies goes through, rather than a
/// private `as_bytes`/lossy shortcut per consumer.
pub fn encode_utf8_scalars(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for character in text.chars() {
        let mut buffer = [0u8; 4];
        out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    }
    out
}

/// One CPython `errors` handler in the **encode** direction.
///
/// These are the exact registered handler names the pinned interpreter accepts.
/// CPython does not normalise an `errors` value: `REPLACE`, `Strict` or
/// `backslash-replace` are *unknown* handler names, so [`Self::from_name`]
/// matches byte-for-byte and never guesses. The stream policy stores an unknown
/// name verbatim and only fails when an unencodable unit actually needs it,
/// exactly like `TextIOWrapper`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PythonEncodeErrorHandler {
    /// `strict` — raise `UnicodeEncodeError`.
    Strict,
    /// `ignore` — drop the unencodable unit.
    Ignore,
    /// `replace` — write the codec's replacement character (`?`).
    Replace,
    /// `xmlcharrefreplace` — write `&#NNNN;`.
    XmlCharRefReplace,
    /// `backslashreplace` — write `\uXXXX` / `\UXXXXXXXX`.
    BackslashReplace,
    /// `surrogateescape` — `\udc80..\udcff` become the single byte `0x80..0xff`.
    SurrogateEscape,
    /// `surrogatepass` — a UTF codec writes the surrogate unit itself.
    SurrogatePass,
    /// `namereplace` — `\N{NAME}`, with the `\uXXXX` form for unnamed units.
    NameReplace,
}

impl PythonEncodeErrorHandler {
    /// The exact CPython handler name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Ignore => "ignore",
            Self::Replace => "replace",
            Self::XmlCharRefReplace => "xmlcharrefreplace",
            Self::BackslashReplace => "backslashreplace",
            Self::SurrogateEscape => "surrogateescape",
            Self::SurrogatePass => "surrogatepass",
            Self::NameReplace => "namereplace",
        }
    }

    /// Resolve an exact CPython `errors` name, or `None` for an unknown name.
    ///
    /// The comparison is exact (CPython's registry is case- and
    /// hyphen-sensitive), which is why an unrecognised name is preserved by the
    /// caller instead of being coerced to `strict`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "strict" => Some(Self::Strict),
            "ignore" => Some(Self::Ignore),
            "replace" => Some(Self::Replace),
            "xmlcharrefreplace" => Some(Self::XmlCharRefReplace),
            "backslashreplace" => Some(Self::BackslashReplace),
            "surrogateescape" => Some(Self::SurrogateEscape),
            "surrogatepass" => Some(Self::SurrogatePass),
            "namereplace" => Some(Self::NameReplace),
            _ => None,
        }
    }
}

/// The codec-independent replacement bytes for one unencodable code unit.
///
/// `None` means the handler's behaviour is codec-specific (or needs data this
/// authority does not own), so the calling encoder must own it:
/// `strict`/`surrogateescape`/`surrogatepass` classify per codec, and
/// `namereplace` for a *named* character needs a Unicode name table this crate
/// does not carry (it returns the `\uXXXX` form only for an unnamed unit, which
/// includes every surrogate). `Some(vec![])` is the `ignore` drop.
pub fn encode_handler_replacement(handler: PythonEncodeErrorHandler, code: u32) -> Option<Vec<u8>> {
    match handler {
        PythonEncodeErrorHandler::Ignore => Some(Vec::new()),
        PythonEncodeErrorHandler::Replace => Some(vec![b'?']),
        PythonEncodeErrorHandler::XmlCharRefReplace => Some(format!("&#{code};").into_bytes()),
        PythonEncodeErrorHandler::BackslashReplace => Some(backslash_escape(code).into_bytes()),
        PythonEncodeErrorHandler::NameReplace if (0xD800..=0xDFFF).contains(&code) => {
            Some(backslash_escape(code).into_bytes())
        }
        PythonEncodeErrorHandler::Strict
        | PythonEncodeErrorHandler::SurrogateEscape
        | PythonEncodeErrorHandler::SurrogatePass
        | PythonEncodeErrorHandler::NameReplace => None,
    }
}

/// CPython's `backslashreplace` spelling of one code unit.
///
/// The width follows CPython's own escape rule: `\xNN` below `U+0100`,
/// `\uNNNN` below `U+10000`, `\UNNNNNNNN` above it (measured against the
/// pinned interpreter, which writes `h\xe9llo` for `ascii:backslashreplace`).
fn backslash_escape(code: u32) -> String {
    if code < 0x100 {
        format!("\\x{code:02x}")
    } else if code < 0x10000 {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

/// Why a handler-aware `str.encode` could not produce bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonUtf8HandlerError {
    /// A `UnicodeEncodeError` from `strict` (or from `surrogateescape` for a
    /// unit outside the escaped range).
    Encode(PythonUtf8EncodeError),
    /// `LookupError: unknown error handler name '<name>'`.
    UnknownHandler {
        /// The handler name exactly as the stream policy stored it.
        name: String,
    },
    /// `namereplace` (a named character) needs the Unicode name table, which
    /// this authority does not own: an honest native gap, not a substitution.
    UnsupportedHandler {
        /// The handler name (`namereplace`).
        name: &'static str,
    },
}

impl std::fmt::Display for PythonUtf8HandlerError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(error) => write!(out, "{error}"),
            Self::UnknownHandler { name } => {
                write!(out, "unknown error handler name '{name}'")
            }
            Self::UnsupportedHandler { name } => write!(
                out,
                "the `{name}` error handler needs the pinned Unicode name table, which this \
                 native encoder does not carry"
            ),
        }
    }
}
impl std::error::Error for PythonUtf8HandlerError {}

/// `str.encode('utf-8', errors)` over exact CPython code units.
///
/// This is the same strict encoder as [`encode_python_string`] plus the named
/// CPython handler applied to the maximal run of unencodable units (only
/// surrogate units are unencodable in UTF-8). It never pairs surrogates, never
/// substitutes U+FFFD and never applies TextIO newline translation; reporting
/// the exact `start`/`end`/`reason` keeps the handler phase distinct.
pub fn encode_python_string_with(
    value: &crate::PythonString,
    handler: PythonEncodeErrorHandler,
) -> Result<Vec<u8>, PythonUtf8HandlerError> {
    let units = value.codepoints();
    let mut out = Vec::with_capacity(units.len());
    let mut position = 0usize;
    while position < units.len() {
        let unit = units[position];
        if let Some(character) = char::from_u32(unit) {
            let mut buffer = [0u8; 4];
            out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            position += 1;
            continue;
        }
        let mut end = position + 1;
        while end < units.len() && char::from_u32(units[end]).is_none() {
            end += 1;
        }
        match handler {
            PythonEncodeErrorHandler::Strict => {
                return Err(PythonUtf8HandlerError::Encode(utf8_encode_error(
                    units, position, end,
                )));
            }
            PythonEncodeErrorHandler::SurrogateEscape => {
                for (offset, &escaped) in units[position..end].iter().enumerate() {
                    if (0xDC80..=0xDCFF).contains(&escaped) {
                        out.push((escaped - 0xDC00) as u8);
                    } else {
                        return Err(PythonUtf8HandlerError::Encode(utf8_encode_error(
                            units,
                            position + offset,
                            position + offset + 1,
                        )));
                    }
                }
            }
            PythonEncodeErrorHandler::SurrogatePass => {
                for &unit in &units[position..end] {
                    // CPython's `surrogatepass` writes the UTF-8 form of the
                    // surrogate code point itself (three bytes, CESU-style).
                    out.push(0xE0 | (unit >> 12) as u8);
                    out.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                    out.push(0x80 | (unit & 0x3F) as u8);
                }
            }
            PythonEncodeErrorHandler::NameReplace => {
                // The reference falls back to `\uXXXX` for a unit without a
                // Unicode name (every surrogate); a named unit needs the name
                // table this authority does not carry, so that is an honest
                // native gap rather than an invented spelling.
                for &unit in &units[position..end] {
                    match encode_handler_replacement(handler, unit) {
                        Some(replacement) => out.extend_from_slice(&replacement),
                        None => {
                            return Err(PythonUtf8HandlerError::UnsupportedHandler {
                                name: "namereplace",
                            });
                        }
                    }
                }
            }
            PythonEncodeErrorHandler::Ignore
            | PythonEncodeErrorHandler::Replace
            | PythonEncodeErrorHandler::XmlCharRefReplace
            | PythonEncodeErrorHandler::BackslashReplace => {
                for &unit in &units[position..end] {
                    let replacement = encode_handler_replacement(handler, unit)
                        .expect("codec-independent handler");
                    out.extend_from_slice(&replacement);
                }
            }
        }
        position = end;
    }
    Ok(out)
}

/// The strict `UnicodeEncodeError` for one maximal unencodable run.
fn utf8_encode_error(units: &[u32], start: usize, end: usize) -> PythonUtf8EncodeError {
    PythonUtf8EncodeError {
        object: units.to_vec(),
        start,
        end,
        reason: "surrogates not allowed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PythonString;

    fn text(codepoints: &[u32]) -> PythonString {
        PythonString::from_codepoints(codepoints).expect("valid code units")
    }

    const LOW: u32 = 0xDC80;
    const HIGH: u32 = 0xD800;
    const E_ACUTE: u32 = 0x00E9;
    const EMOJI: u32 = 0x1F600;

    #[test]
    fn handler_names_are_exact_and_complete() {
        for handler in [
            PythonEncodeErrorHandler::Strict,
            PythonEncodeErrorHandler::Ignore,
            PythonEncodeErrorHandler::Replace,
            PythonEncodeErrorHandler::XmlCharRefReplace,
            PythonEncodeErrorHandler::BackslashReplace,
            PythonEncodeErrorHandler::SurrogateEscape,
            PythonEncodeErrorHandler::SurrogatePass,
            PythonEncodeErrorHandler::NameReplace,
        ] {
            assert_eq!(
                PythonEncodeErrorHandler::from_name(handler.name()),
                Some(handler)
            );
        }
        for unknown in ["REPLACE", "Strict", "backslash-replace", "strict ", ""] {
            assert_eq!(PythonEncodeErrorHandler::from_name(unknown), None);
        }
    }

    #[test]
    fn surrogateescape_passes_only_the_escaped_range() {
        assert_eq!(
            encode_python_string_with(
                &text(&[b'a' as u32, LOW, 0xDCFF]),
                PythonEncodeErrorHandler::SurrogateEscape
            ),
            Ok(b"a\x80\xff".to_vec())
        );
        assert!(matches!(
            encode_python_string_with(&text(&[HIGH]), PythonEncodeErrorHandler::SurrogateEscape),
            Err(PythonUtf8HandlerError::Encode(error))
                if error.start == 0 && error.end == 1 && error.reason == "surrogates not allowed"
        ));
        // A run of escaped and unescaped units reports the first real failure.
        assert!(matches!(
            encode_python_string_with(&text(&[LOW, 0xDD00, LOW]), PythonEncodeErrorHandler::SurrogateEscape),
            Err(PythonUtf8HandlerError::Encode(error)) if error.start == 1 && error.end == 2
        ));
    }

    #[test]
    fn surrogatepass_writes_the_unit_itself() {
        assert_eq!(
            encode_python_string_with(&text(&[HIGH]), PythonEncodeErrorHandler::SurrogatePass),
            Ok(vec![0xED, 0xA0, 0x80])
        );
        assert_eq!(
            encode_python_string_with(&text(&[LOW]), PythonEncodeErrorHandler::SurrogatePass),
            Ok(vec![0xED, 0xB2, 0x80])
        );
    }

    #[test]
    fn replacement_handlers_keep_the_reference_spelling() {
        assert_eq!(
            encode_python_string_with(&text(&[LOW]), PythonEncodeErrorHandler::BackslashReplace),
            Ok(b"\\udc80".to_vec())
        );
        assert_eq!(
            encode_python_string_with(&text(&[HIGH]), PythonEncodeErrorHandler::NameReplace),
            Ok(b"\\ud800".to_vec())
        );
        assert_eq!(
            encode_python_string_with(&text(&[LOW]), PythonEncodeErrorHandler::XmlCharRefReplace),
            Ok(b"&#56448;".to_vec())
        );
        assert_eq!(
            encode_python_string_with(&text(&[LOW]), PythonEncodeErrorHandler::Replace),
            Ok(b"?".to_vec())
        );
        assert_eq!(
            encode_python_string_with(
                &text(&[b'a' as u32, HIGH]),
                PythonEncodeErrorHandler::Ignore
            ),
            Ok(b"a".to_vec())
        );
        // Encodable scalars never reach a handler.
        assert_eq!(
            encode_python_string_with(
                &text(&[E_ACUTE, EMOJI]),
                PythonEncodeErrorHandler::BackslashReplace
            ),
            Ok("\u{e9}\u{1f600}".as_bytes().to_vec())
        );
    }

    #[test]
    fn namereplace_for_an_unnamed_unit_uses_the_backslash_form() {
        assert_eq!(
            encode_python_string_with(&text(&[HIGH]), PythonEncodeErrorHandler::NameReplace),
            Ok(b"\\ud800".to_vec()),
            "the reference itself falls back to \\uXXXX for an unnamed unit"
        );
        assert_eq!(
            encode_handler_replacement(PythonEncodeErrorHandler::NameReplace, 0x2603),
            None,
            "a named character cannot be produced without the name table"
        );
        assert_eq!(
            encode_handler_replacement(PythonEncodeErrorHandler::NameReplace, HIGH).as_deref(),
            Some(b"\\ud800".as_slice())
        );
    }

    #[test]
    fn strict_encoding_is_unchanged_by_the_handler_entry_point() {
        assert_eq!(
            encode_python_string_with(
                &text(&[b'o' as u32, b'p' as u32]),
                PythonEncodeErrorHandler::Strict
            ),
            encode_python_string(&text(&[b'o' as u32, b'p' as u32]))
                .map_err(PythonUtf8HandlerError::Encode)
        );
        // The reported range is the maximal run, exactly like `encode_python_string`.
        let unit = text(&[b'x' as u32, HIGH, HIGH, b'y' as u32]);
        let from_handler = encode_python_string_with(&unit, PythonEncodeErrorHandler::Strict)
            .expect_err("surrogates have no UTF-8 form");
        let from_strict = encode_python_string(&unit).expect_err("surrogates have no UTF-8 form");
        assert_eq!(
            from_handler.to_string(),
            PythonUtf8HandlerError::Encode(from_strict).to_string()
        );
    }

    #[test]
    fn replacement_helper_is_codec_independent() {
        assert_eq!(
            encode_handler_replacement(PythonEncodeErrorHandler::Ignore, LOW),
            Some(Vec::new())
        );
        assert_eq!(
            encode_handler_replacement(PythonEncodeErrorHandler::BackslashReplace, EMOJI)
                .as_deref(),
            Some(b"\\U0001f600".as_slice())
        );
        assert_eq!(
            encode_handler_replacement(PythonEncodeErrorHandler::SurrogateEscape, LOW),
            None,
            "codec-specific handlers stay with the encoder"
        );
    }
}
