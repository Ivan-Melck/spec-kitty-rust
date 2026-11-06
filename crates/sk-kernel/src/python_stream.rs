//! One composition of the reference's **physical stdio** codec and error policy.
//!
//! The reference prints with CPython's `sys.stdout` / `sys.stderr`: a
//! `TextIOWrapper` whose encoding is decided once at interpreter startup, from
//! `PYTHONIOENCODING`, the UTF-8 mode and the real locale, and whose
//! `str` → bytes step is a *separate, late* phase from rendering. Exact code
//! points in memory therefore do not imply correct stream bytes:
//!
//! * with the default UTF-8 mode, `sys.stdout.errors` is `surrogateescape`, so
//!   an authored `\udc80..\udcff` reaches stdout as the single byte `0x80..0xff`
//!   (and every other lone surrogate still fails);
//! * `sys.stderr.errors` is **always** `backslashreplace` in the pinned
//!   interpreter: measured with `PYTHONIOENCODING` unset, `utf-8`,
//!   `ascii`, `latin-1`, `utf-8:strict`, `ascii:replace`, `ascii:ignore`,
//!   `latin-1:xmlcharrefreplace`, `ascii:namereplace` and
//!   `utf-8:surrogateescape` — every one reports `sys.stderr.errors ==
//!   'backslashreplace'`, so an unencodable unit is the ASCII `\uXXXX`/`\xNN`
//!   escape and never a raw surrogateescape byte;
//! * `PYTHONIOENCODING` overrides the *encoding* for both streams, so
//!   `utf-8:backslashreplace` prints `\ud800`, `ascii:replace` prints `?` on
//!   stdout, and `ascii` makes either stream spell a non-ASCII unit as its
//!   escape;
//! * `json.dumps(..., ensure_ascii=True)` only decides the *spelling* of the
//!   JSON document — the physical stream can still be `latin-1` or `utf-16`;
//! * `utf-16`/`utf-32` streams carry **no BOM** on either stream (measured: two
//!   `stderr.write` calls under `PYTHONIOENCODING=utf-16`/`utf-32` produce the
//!   native-endian units only, and the existing stdout composition already
//!   models that), so no per-stream BOM state exists to carry;
//! * an unencodable unit aborts the whole `write()` (no partial bytes), and the
//!   reference adds its own `PYTHONIOENCODING=utf-8` hint to stdout's error text;
//! * a closed stdout pipe is not a success: Rich's console exits 1 (it redirects
//!   fd 1 to `/dev/null`), while a write/flush failure that is still pending at
//!   interpreter shutdown — the builtin `print()` JSON arm, or any other
//!   `OSError` from the stream — ends as CPython's exit 120. A closed stderr
//!   pipe is a raised `BrokenPipeError` (exit 1) and a full stderr device ends
//!   as the shutdown-flush exit 120 (both measured).
//!
//! This module is the ONE owner of that composition, shared by the CLI's stdout
//! emitters and the Doctrine activation-reader warnings. It composes existing
//! authorities instead of inventing a second Unicode/locale model:
//!
//! * [`crate::python_utf8`] owns strict UTF-8 encoding, the exact
//!   `UnicodeEncodeError` data and the codec-independent error-handler
//!   replacements;
//! * [`crate::python_os_encoding`] owns the real startup locale request/codeset,
//!   the `PYTHONUTF8` startup value and the ISO-8859-15 single-byte mapping.
//!
//! The filesystem encoding is deliberately **not** reused by name: it is a
//! different startup decision with a different error default, and the stream
//! classifies the locale codeset for itself.
//!
//! Supported policy scope (honest, not a Source-exception list): the UTF-8,
//! ASCII, ISO-8859-1, ISO-8859-15, UTF-16/UTF-16-LE/UTF-16-BE and
//! UTF-32/UTF-32-LE/UTF-32-BE codecs, with the `strict`, `ignore`, `replace`,
//! `xmlcharrefreplace`, `backslashreplace`, `surrogateescape` and
//! `surrogatepass` handlers (`namereplace` only where the reference itself
//! falls back to `\uXXXX`). Any other Source-valid codec or handler is a
//! precise refusal and remains a recorded native gap — never a silent
//! substitution and never claimed as a Source exception.

use std::ffi::OsString;
use std::io::{self, Write};

use crate::PythonString;
use crate::python_os_encoding::{
    LocaleCodeset, latin9_byte, locale_codeset, locale_request, utf8_mode_setting,
};
use crate::python_utf8::{
    PythonEncodeErrorHandler, PythonUtf8HandlerError, encode_handler_replacement,
    encode_python_string, encode_python_string_with,
};

/// The environment variable that overrides the startup stream codec.
const IO_ENCODING_VAR: &str = "PYTHONIOENCODING";

/// Which reference stream produced (or failed to receive) the bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamKind {
    /// `sys.stdout`.
    Stdout,
    /// `sys.stderr`.
    Stderr,
}

impl StreamKind {
    /// The pinned interpreter's error handler for this stream.
    ///
    /// `stderr` is measured as `backslashreplace` under every probed
    /// `PYTHONIOENCODING` setting, including explicit handlers; `stdout` keeps
    /// the configured handler (the startup default is `surrogateescape` in
    /// UTF-8 mode and the C locale family, `strict` in a real locale).
    const fn configured_errors(self) -> Option<&'static str> {
        match self {
            Self::Stdout => None,
            Self::Stderr => Some("backslashreplace"),
        }
    }
}

/// Which reference write class produced the bytes.
///
/// The rendered bytes are identical; the classes differ only in their failure
/// phase, which the reference really does implement separately per stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteClass {
    /// Rich's `Console.print` (the human text arm): a broken pipe becomes the
    /// console's `SystemExit(1)` with stdout redirected to `/dev/null`.
    RichConsolePrint,
    /// Builtin `print()` (the `--json` arm): a broken pipe is reported as the
    /// ignored flush failure at interpreter shutdown, exit 120.
    BuiltinPrint,
    /// A `print(..., file=sys.stderr)` line: a broken pipe raises
    /// `BrokenPipeError` (exit 1) and a full device ends at the shutdown flush
    /// (exit 120), both measured.
    Stderr,
}

impl WriteClass {
    /// The reference's exit code for a broken pipe in this class.
    pub const fn broken_pipe_exit(self) -> u8 {
        match self {
            Self::RichConsolePrint | Self::Stderr => 1,
            Self::BuiltinPrint => 120,
        }
    }
}

/// One stdio codec this composition implements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamCodec {
    /// CPython `utf-8`.
    Utf8,
    /// CPython `ascii`.
    Ascii,
    /// CPython `latin-1` (`sys.stdout.encoding` spells it `iso8859-1`).
    Latin1,
    /// CPython `iso8859-15` (Latin-9), reported as the `charmap` codec.
    Latin9,
    /// CPython `utf-16` (native endianness, no BOM on the stream).
    Utf16,
    /// CPython `utf-16-le`.
    Utf16Le,
    /// CPython `utf-16-be`.
    Utf16Be,
    /// CPython `utf-32` (native endianness, no BOM on the stream).
    Utf32,
    /// CPython `utf-32-le`.
    Utf32Le,
    /// CPython `utf-32-be`.
    Utf32Be,
}

impl StreamCodec {
    /// `sys.stdout.encoding` for this codec.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Ascii => "ascii",
            Self::Latin1 => "iso8859-1",
            Self::Latin9 => "iso8859-15",
            Self::Utf16 => "utf-16",
            Self::Utf16Le => "utf-16-le",
            Self::Utf16Be => "utf-16-be",
            Self::Utf32 => "utf-32",
            Self::Utf32Le => "utf-32-le",
            Self::Utf32Be => "utf-32-be",
        }
    }

    /// The codec name CPython uses in `UnicodeEncodeError` text.
    const fn error_name(self) -> &'static str {
        match self {
            // The reference's fast single-byte encoders report these names,
            // not the `sys.stdout.encoding` spellings.
            Self::Latin1 => "latin-1",
            Self::Latin9 => "charmap",
            other => other.name(),
        }
    }

    /// The reason CPython reports for one unencodable unit.
    const fn reason(self) -> &'static str {
        match self {
            Self::Utf8
            | Self::Utf16
            | Self::Utf16Le
            | Self::Utf16Be
            | Self::Utf32
            | Self::Utf32Le
            | Self::Utf32Be => "surrogates not allowed",
            Self::Ascii => "ordinal not in range(128)",
            Self::Latin1 => "ordinal not in range(256)",
            Self::Latin9 => "character maps to <undefined>",
        }
    }

    /// Byte-oriented codecs (where `surrogateescape` yields one raw byte).
    const fn is_byte_oriented(self) -> bool {
        matches!(self, Self::Utf8 | Self::Ascii | Self::Latin1 | Self::Latin9)
    }

    /// UTF codecs, whose `surrogatepass` writes the unit itself.
    const fn is_utf(self) -> bool {
        !matches!(self, Self::Ascii | Self::Latin1 | Self::Latin9)
    }

    /// A charmap codec wraps an unknown handler in the codec's own `LookupError`.
    const fn is_charmap(self) -> bool {
        matches!(self, Self::Latin9)
    }
}

/// The startup stream policy CPython would give `sys.stdout` or `sys.stderr`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamPolicy {
    kind: StreamKind,
    codec: StreamCodec,
    errors: String,
}

impl StreamPolicy {
    /// Capture the current process's startup stdout policy.
    ///
    /// The reference decides this once per interpreter; the native process
    /// reads the same environment and real locale facts (no mutation, no
    /// Python process) at the emit boundary.
    pub fn from_environment() -> Result<Self, StreamError> {
        Self::from_environment_for(StreamKind::Stdout)
    }

    /// Resolve the policy from an injected environment lookup.
    ///
    /// This is the exact precedence the pinned interpreter uses: an explicit
    /// `PYTHONIOENCODING` wins (codec and handler), otherwise the UTF-8 mode
    /// decides the codec (`utf-8`) or the real locale codeset is classified;
    /// the default error handler is `surrogateescape` in UTF-8 mode and in the
    /// C locale family, and `strict` in a real locale.
    pub fn resolve(lookup: &impl Fn(&str) -> Option<OsString>) -> Result<Self, StreamError> {
        Self::resolve_for(StreamKind::Stdout, lookup)
    }

    /// Capture the current process's startup policy for *kind*.
    pub fn from_environment_for(kind: StreamKind) -> Result<Self, StreamError> {
        Self::resolve_for(kind, &|name| std::env::var_os(name))
    }

    /// The pinned `sys.stderr` policy: the same startup encoding resolution as
    /// stdout, with the measured `backslashreplace` handler.
    pub fn for_stderr() -> Result<Self, StreamError> {
        Self::from_environment_for(StreamKind::Stderr)
    }

    /// Resolve the policy for *kind* from an injected environment lookup.
    pub fn resolve_for(
        kind: StreamKind,
        lookup: &impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, StreamError> {
        let utf8_setting =
            utf8_mode_setting(lookup).map_err(|detail| StreamError::InvalidStartup {
                detail: format!("Fatal Python error: {detail}"),
            })?;
        let request = locale_request(lookup).map_err(|detail| StreamError::InvalidStartup {
            detail: format!("the startup locale request is not decodable: {detail}"),
        })?;
        let auto_utf8 = match utf8_setting {
            Some(value) => value,
            None => matches!(request.as_deref(), None | Some("C") | Some("POSIX")),
        };
        if let Some(setting) = non_empty_value(lookup, IO_ENCODING_VAR)? {
            let (encoding, handler) = match setting.split_once(':') {
                Some((encoding, handler)) => (encoding, Some(handler)),
                None => (setting.as_str(), None),
            };
            // `PYTHONIOENCODING=:` (both parts empty) is measured as "ignored"
            // by the pinned interpreter; `utf-8:` keeps `strict`, and an empty
            // codec part keeps the locale/UTF-8-mode codec.
            if !(encoding.is_empty() && handler.is_some_and(str::is_empty)) {
                let codec = if encoding.is_empty() {
                    default_codec(lookup, auto_utf8)?
                } else {
                    parse_codec(encoding)?
                };
                let errors = match kind.configured_errors() {
                    Some(fixed) => fixed.to_owned(),
                    None => match handler {
                        Some(handler) if !handler.is_empty() => handler.to_owned(),
                        _ => "strict".to_owned(),
                    },
                };
                return Ok(Self {
                    kind,
                    codec,
                    errors,
                });
            }
        }
        let codec = default_codec(lookup, auto_utf8)?;
        let errors = match kind.configured_errors() {
            Some(fixed) => fixed.to_owned(),
            None if auto_utf8 || c_locale_family(&request, codec) => "surrogateescape".to_owned(),
            None => "strict".to_owned(),
        };
        Ok(Self {
            kind,
            codec,
            errors,
        })
    }

    /// Which stream this policy was resolved for.
    pub const fn kind(&self) -> StreamKind {
        self.kind
    }

    /// The resolved encoding (`sys.stdout.encoding`).
    pub const fn codec(&self) -> StreamCodec {
        self.codec
    }

    /// The resolved encoding label (`sys.stdout.encoding`).
    pub const fn encoding(&self) -> &'static str {
        self.codec.name()
    }

    /// The resolved error handler name (`sys.stdout.errors`), verbatim.
    pub fn errors(&self) -> &str {
        &self.errors
    }

    /// Encode exact code points for this stream, in one `write()` call.
    ///
    /// The error handler is applied to the maximal run of unencodable units, and
    /// the reference's `UnicodeEncodeError` text/positions are preserved. A
    /// handler the pinned interpreter would reject only fails when an
    /// unencodable unit actually needs it.
    pub fn encode(&self, value: &PythonString) -> Result<Vec<u8>, StreamError> {
        let handler = PythonEncodeErrorHandler::from_name(&self.errors);
        if self.codec == StreamCodec::Utf8 {
            return self.encode_utf8(value, handler);
        }
        let units = value.codepoints();
        let mut out = Vec::with_capacity(units.len());
        let mut position = 0usize;
        while position < units.len() {
            if let Some(bytes) = self.codec.encode_unit(units[position]) {
                out.extend_from_slice(&bytes);
                position += 1;
                continue;
            }
            let mut end = position + 1;
            while end < units.len() && self.codec.encode_unit(units[end]).is_none() {
                end += 1;
            }
            let Some(handler) = handler else {
                return Err(self.unknown_handler());
            };
            match handler {
                PythonEncodeErrorHandler::Strict => {
                    return Err(self.encode_error(units, position, end));
                }
                PythonEncodeErrorHandler::SurrogateEscape if self.codec.is_byte_oriented() => {
                    for (offset, &unit) in units[position..end].iter().enumerate() {
                        match escaped_byte(unit) {
                            Some(byte) => out.push(byte),
                            None => {
                                return Err(self.encode_error(
                                    units,
                                    position + offset,
                                    position + offset + 1,
                                ));
                            }
                        }
                    }
                }
                PythonEncodeErrorHandler::SurrogatePass if self.codec.is_utf() => {
                    for &unit in &units[position..end] {
                        out.extend_from_slice(&self.codec.encode_surrogate_unit(unit));
                    }
                }
                PythonEncodeErrorHandler::SurrogateEscape
                | PythonEncodeErrorHandler::SurrogatePass => {
                    // `surrogateescape` is a byte-oriented handler and
                    // `surrogatepass` a UTF-codec one; elsewhere the reference
                    // raises the codec's own `UnicodeEncodeError`.
                    return Err(self.encode_error(units, position, end));
                }
                other => {
                    for &unit in &units[position..end] {
                        let Some(replacement) = encode_handler_replacement(other, unit) else {
                            return Err(StreamError::UnsupportedHandler { name: other.name() });
                        };
                        out.extend_from_slice(&self.codec.encode_replacement(&replacement));
                    }
                }
            }
            position = end;
        }
        Ok(out)
    }

    /// The UTF-8 arm delegates to the one Kernel encoder authority.
    fn encode_utf8(
        &self,
        value: &PythonString,
        handler: Option<PythonEncodeErrorHandler>,
    ) -> Result<Vec<u8>, StreamError> {
        let Some(handler) = handler else {
            return match encode_python_string(value) {
                Ok(bytes) => Ok(bytes),
                Err(_) => Err(self.unknown_handler()),
            };
        };
        encode_python_string_with(value, handler).map_err(|error| match error {
            PythonUtf8HandlerError::Encode(error) => StreamError::UnicodeEncode {
                message: error.to_string(),
            },
            PythonUtf8HandlerError::UnknownHandler { name } => StreamError::UnknownHandler {
                name,
                charmap: false,
            },
            PythonUtf8HandlerError::UnsupportedHandler { name } => {
                StreamError::UnsupportedHandler { name }
            }
        })
    }

    fn unknown_handler(&self) -> StreamError {
        StreamError::UnknownHandler {
            name: self.errors.clone(),
            charmap: self.codec.is_charmap(),
        }
    }

    fn encode_error(&self, units: &[u32], start: usize, end: usize) -> StreamError {
        StreamError::UnicodeEncode {
            message: unicode_encode_message(self.codec, units, start, end),
        }
    }
}

impl StreamCodec {
    /// The bytes of one code unit, or `None` when this codec cannot encode it.
    fn encode_unit(self, code: u32) -> Option<Vec<u8>> {
        match self {
            Self::Utf8 => {
                let character = char::from_u32(code)?;
                let mut buffer = [0u8; 4];
                Some(character.encode_utf8(&mut buffer).as_bytes().to_vec())
            }
            Self::Ascii => (code < 0x80).then(|| vec![code as u8]),
            Self::Latin1 => (code <= 0xFF).then(|| vec![code as u8]),
            Self::Latin9 => latin9_byte(code).map(|byte| vec![byte]),
            Self::Utf16 => {
                if is_surrogate(code) {
                    return None;
                }
                Some(if cfg!(target_endian = "little") {
                    utf16_le(code)
                } else {
                    utf16_be(code)
                })
            }
            Self::Utf16Le => {
                if is_surrogate(code) {
                    return None;
                }
                Some(utf16_le(code))
            }
            Self::Utf16Be => {
                if is_surrogate(code) {
                    return None;
                }
                Some(utf16_be(code))
            }
            Self::Utf32 | Self::Utf32Le | Self::Utf32Be => {
                if is_surrogate(code) {
                    return None;
                }
                Some(utf32_units(code, self))
            }
        }
    }

    /// A UTF codec's `surrogatepass` bytes for one surrogate unit.
    fn encode_surrogate_unit(self, code: u32) -> Vec<u8> {
        match self {
            Self::Utf8 => vec![
                0xE0 | (code >> 12) as u8,
                0x80 | ((code >> 6) & 0x3F) as u8,
                0x80 | (code & 0x3F) as u8,
            ],
            Self::Utf16 => {
                if cfg!(target_endian = "little") {
                    (code as u16).to_le_bytes().to_vec()
                } else {
                    (code as u16).to_be_bytes().to_vec()
                }
            }
            Self::Utf16Le => (code as u16).to_le_bytes().to_vec(),
            Self::Utf16Be => (code as u16).to_be_bytes().to_vec(),
            Self::Utf32 => {
                if cfg!(target_endian = "little") {
                    code.to_le_bytes().to_vec()
                } else {
                    code.to_be_bytes().to_vec()
                }
            }
            Self::Utf32Le => code.to_le_bytes().to_vec(),
            Self::Utf32Be => code.to_be_bytes().to_vec(),
            Self::Ascii | Self::Latin1 | Self::Latin9 => Vec::new(),
        }
    }

    /// Encode an ASCII replacement (`?`, `\uXXXX`, `&#N;`) through this codec.
    fn encode_replacement(self, replacement: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(replacement.len() * 4);
        for &byte in replacement {
            let bytes = self
                .encode_unit(u32::from(byte))
                .expect("every supported stream codec encodes ASCII");
            out.extend_from_slice(&bytes);
        }
        out
    }
}

/// The reference's `UnicodeEncodeError` message for one maximal unencodable run.
fn unicode_encode_message(codec: StreamCodec, units: &[u32], start: usize, end: usize) -> String {
    let name = codec.error_name();
    let reason = codec.reason();
    if end == start + 1 {
        let unit = escape_unit(units[start]);
        format!("'{name}' codec can't encode character '{unit}' in position {start}: {reason}")
    } else {
        format!(
            "'{name}' codec can't encode characters in position {}-{}: {reason}",
            start,
            end - 1
        )
    }
}

/// CPython's spelling of one unencodable code unit.
fn escape_unit(code: u32) -> String {
    if code < 0x100 {
        format!("\\x{code:02x}")
    } else if code < 0x10000 {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

fn is_surrogate(code: u32) -> bool {
    (0xD800..=0xDFFF).contains(&code)
}

/// `surrogateescape` for a byte-oriented codec.
fn escaped_byte(code: u32) -> Option<u8> {
    (0xDC80..=0xDCFF)
        .contains(&code)
        .then(|| (code - 0xDC00) as u8)
}

/// UTF-16 little-endian bytes for one non-surrogate code point.
fn utf16_le(code: u32) -> Vec<u8> {
    utf16_codepoint(code)
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// UTF-16 big-endian bytes for one non-surrogate code point.
fn utf16_be(code: u32) -> Vec<u8> {
    utf16_codepoint(code)
        .into_iter()
        .flat_map(u16::to_be_bytes)
        .collect()
}

/// The one or two UTF-16 units of a non-surrogate code point.
fn utf16_codepoint(code: u32) -> Vec<u16> {
    if code <= 0xFFFF {
        return vec![code as u16];
    }
    let value = code - 0x10000;
    vec![
        0xD800 + ((value >> 10) as u16),
        0xDC00 + ((value & 0x3FF) as u16),
    ]
}

/// UTF-32 bytes for one code point (native endianness for the plain codec).
fn utf32_units(code: u32, codec: StreamCodec) -> Vec<u8> {
    let little = matches!(codec, StreamCodec::Utf32Le)
        || (codec == StreamCodec::Utf32 && cfg!(target_endian = "little"));
    if little {
        code.to_le_bytes().to_vec()
    } else {
        code.to_be_bytes().to_vec()
    }
}

/// The stream codec when `PYTHONIOENCODING` does not name one.
fn default_codec(
    lookup: &impl Fn(&str) -> Option<OsString>,
    auto_utf8: bool,
) -> Result<StreamCodec, StreamError> {
    if auto_utf8 {
        return Ok(StreamCodec::Utf8);
    }
    match locale_codeset(lookup) {
        LocaleCodeset::Resolved { codeset } => parse_codec(&codeset),
        LocaleCodeset::CLocale => Ok(StreamCodec::Ascii),
        LocaleCodeset::Uncertain { detail } => Err(StreamError::UnsupportedEncoding {
            name: "the startup locale codeset".to_owned(),
            detail,
        }),
    }
}

/// Is this the C locale family whose stdout default is `surrogateescape`?
///
/// Measured against the pinned interpreter: the C/POSIX locale (ASCII codeset),
/// the exact PEP 538 coercion target names `C.UTF-8`/`C.utf8`/`c.utf8`, and a
/// coerced request use `surrogateescape`; a real locale (`en_US.UTF-8`,
/// `C.UTF8`, `C.UTF-8@euro`) uses `strict`. The comparison is case-sensitive on
/// purpose: `C.UTF8` is a real locale, not the coercion target. The codeset
/// itself still comes from the real locale data.
fn c_locale_family(request: &Option<String>, codec: StreamCodec) -> bool {
    if codec == StreamCodec::Ascii {
        return true;
    }
    matches!(
        request.as_deref(),
        Some("C" | "POSIX" | "C.UTF-8" | "C.utf8" | "c.utf8")
    )
}

/// A non-empty environment value; uninterpretable bytes are an honest refusal.
fn non_empty_value(
    lookup: &impl Fn(&str) -> Option<OsString>,
    name: &str,
) -> Result<Option<String>, StreamError> {
    match lookup(name) {
        None => Ok(None),
        Some(raw) => match raw.into_string() {
            Ok(text) => Ok((!text.is_empty()).then_some(text)),
            Err(_) => Err(StreamError::InvalidStartup {
                detail: format!("{name} is not a text value this native adapter can interpret"),
            }),
        },
    }
}

/// Resolve one codec spelling (an `encodings` name, alias or a host codeset).
///
/// The `encodings` normalisation is the reference's: case-fold, keep ASCII
/// alphanumerics and `.`, collapse every other run to `_`, trim `_`. Only the
/// supported codec set resolves; every other Source-valid name is an explicit
/// native gap.
pub fn parse_codec(name: &str) -> Result<StreamCodec, StreamError> {
    let normalized = normalize_codec_name(name);
    let codec = match normalized.as_str() {
        "utf_8" | "utf8" | "u8" | "utf" | "cp65001" => StreamCodec::Utf8,
        "ascii" | "us_ascii" | "us" | "646" | "ansi_x3.4.1968" | "ansi_x3.4.1986"
        | "ansi_x3.4_1968" | "iso646_us" | "cp367" | "ibm367" => StreamCodec::Ascii,
        "latin_1" | "latin1" | "latin" | "iso_8859_1" | "iso8859_1" | "iso88591" | "8859"
        | "cp819" | "ibm819" | "l1" => StreamCodec::Latin1,
        "iso_8859_15" | "iso8859_15" | "iso885915" | "latin_9" | "latin9" | "l9" => {
            StreamCodec::Latin9
        }
        "utf_16" | "utf16" | "u16" => StreamCodec::Utf16,
        "utf_16_le" | "utf_16le" | "utf16le" | "unicodelittleunmarked" => StreamCodec::Utf16Le,
        "utf_16_be" | "utf_16be" | "utf16be" | "unicodebigunmarked" => StreamCodec::Utf16Be,
        "utf_32" | "utf32" | "u32" => StreamCodec::Utf32,
        "utf_32_le" | "utf_32le" | "utf32le" => StreamCodec::Utf32Le,
        "utf_32_be" | "utf_32be" | "utf32be" => StreamCodec::Utf32Be,
        _ => {
            return Err(StreamError::UnsupportedEncoding {
                name: name.to_owned(),
                detail:
                    "the pinned interpreter accepts this codec spelling, but this native stream \
                         composition implements only utf-8, ascii, iso-8859-1, iso-8859-15, \
                         utf-16/-le/-be and utf-32/-le/-be"
                        .to_owned(),
            });
        }
    };
    Ok(codec)
}

/// The `encodings` module's name normalisation.
fn normalize_codec_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_separator = false;
    for character in name.chars() {
        let lower = character.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '.' {
            if pending_separator && !out.is_empty() {
                out.push('_');
            }
            pending_separator = false;
            out.push(lower);
        } else {
            pending_separator = true;
        }
    }
    out
}

/// Why the physical stream step could not deliver the rendered text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamError {
    /// A Source-valid codec/codeset this composition does not implement.
    UnsupportedEncoding {
        /// The codec spelling or the measured context.
        name: String,
        /// The precise native gap.
        detail: String,
    },
    /// An unsupported startup value (`PYTHONUTF8`, an undecodable variable).
    InvalidStartup {
        /// The precise startup failure.
        detail: String,
    },
    /// A CPython `UnicodeEncodeError` (codec name, unit, position, reason).
    UnicodeEncode {
        /// The exact reference message.
        message: String,
    },
    /// `LookupError: unknown error handler name '<name>'`.
    UnknownHandler {
        /// The handler exactly as the stream policy stored it.
        name: String,
        /// A charmap codec wraps the `LookupError` in the codec's own text.
        charmap: bool,
    },
    /// A handler this native encoder cannot carry (a named `namereplace`).
    UnsupportedHandler {
        /// The handler name.
        name: &'static str,
    },
    /// The downstream reader closed the pipe.
    BrokenPipe {
        /// The reference write class whose phase failed.
        class: WriteClass,
    },
    /// Stdout is not an open descriptor at all.
    ///
    /// CPython leaves `sys.stdout` as `None` in that case, so the reference
    /// fails at the attribute access (exit 1) and never reaches a shutdown
    /// flush. This is distinct from a stream that exists but cannot be written.
    Unavailable {
        /// The OS error text.
        detail: String,
    },
    /// Any other write/flush failure (for example a full device).
    Io {
        /// The reference write class whose phase failed.
        class: WriteClass,
        /// The OS error text.
        detail: String,
    },
}

impl StreamError {
    /// The process exit code the reference produces for this phase.
    ///
    /// CPython reports a *failed flush at interpreter shutdown* as exit 120, so
    /// any write/flush failure that is still unresolved when the process ends
    /// lands on 120 (measured: `/dev/full` gives 120 on the stdout text arm, the
    /// builtin-`print` JSON arm and the stderr line). The Rich text arm's broken
    /// pipe is the one exception — the reference redirects fd 1 to `/dev/null`
    /// and exits 1 — and a broken stderr pipe raises `BrokenPipeError`, also
    /// exit 1. Startup and encoding refusals happen before that shutdown flush
    /// and stay 1.
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::BrokenPipe { class } => class.broken_pipe_exit(),
            Self::Io { .. } => 120,
            Self::Unavailable { .. } => 1,
            _ => 1,
        }
    }

    /// True when the failure is the reference's physical-encoding phase.
    pub const fn is_encode_failure(&self) -> bool {
        matches!(
            self,
            Self::UnicodeEncode { .. }
                | Self::UnknownHandler { .. }
                | Self::UnsupportedHandler { .. }
        )
    }

    /// True when the reference could not have started with this stream at all.
    ///
    /// CPython resolves `sys.stderr`'s codec, `PYTHONIOENCODING` and its error
    /// handler at interpreter start-up and dies with a `Fatal Python error` when the
    /// start-up configuration itself is unusable, so no user code (and therefore no
    /// `logging` call) ever runs. The physical write/flush classes — and a runtime
    /// `UnicodeEncodeError` from `stream.write` — are the opposite: the reference's
    /// `Handler.emit` catches them and `handleError` swallows them.
    ///
    /// This class is deliberately **narrow**: an `UnsupportedEncoding` or
    /// `UnsupportedHandler` names a codec/error handler the pinned interpreter
    /// accepts but this composition does not carry, which is an explicit native
    /// capability gap ([`Self::is_native_capability_gap`]), never a Source-invalid
    /// label. `UnknownHandler` is ambiguous between an invalid start-up value and a
    /// handler the native encoder lacks at write time; it is classified as a
    /// capability gap rather than asserting an unproven Source-invalidity. Exact
    /// process-start capture and a full codec/handler registry remain required.
    pub const fn is_stream_startup_failure(&self) -> bool {
        matches!(self, Self::InvalidStartup { .. })
    }

    /// A Source-valid backend this composition does not implement.
    ///
    /// This is an explicit, unaccepted functionality gap: it is never a semantic
    /// exception, never Source-invalid input, and never a healthy handled outcome.
    pub const fn is_native_capability_gap(&self) -> bool {
        matches!(
            self,
            Self::UnsupportedEncoding { .. }
                | Self::UnknownHandler { .. }
                | Self::UnsupportedHandler { .. }
        )
    }

    /// Whether the default warning transport cannot deliver anything at all.
    ///
    /// Both a real start-up refusal and a native capability gap stop delivery.
    /// Neither may be reported as the Source's handled-runtime-error arm, because
    /// native has no bytes to claim.
    pub const fn stops_warning_delivery(&self) -> bool {
        self.is_stream_startup_failure() || self.is_native_capability_gap()
    }
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedEncoding { name, detail } => write!(
                out,
                "native stdio codec {name} is not implemented by this stream composition\n{detail}"
            ),
            Self::InvalidStartup { detail } => out.write_str(detail),
            Self::UnicodeEncode { message } => out.write_str(message),
            Self::UnknownHandler {
                name,
                charmap: true,
            } => write!(
                out,
                "encoding with the stdout charmap codec failed (LookupError: unknown error \
                 handler name '{name}')"
            ),
            Self::UnknownHandler {
                name,
                charmap: false,
            } => {
                write!(out, "unknown error handler name '{name}'")
            }
            Self::UnsupportedHandler { name } => write!(
                out,
                "the `{name}` error handler needs the pinned Unicode name table, which this \
                 native encoder does not carry"
            ),
            Self::BrokenPipe { class } => match class {
                WriteClass::RichConsolePrint => {
                    out.write_str("BrokenPipeError: [Errno 32] Broken pipe")
                }
                WriteClass::BuiltinPrint => write!(
                    out,
                    "Exception ignored in: <_io.TextIOWrapper name='<stdout>' mode='w' \
                     encoding='utf-8'>\nBrokenPipeError: [Errno 32] Broken pipe"
                ),
                WriteClass::Stderr => write!(
                    out,
                    "Exception ignored in: <_io.TextIOWrapper name='<stderr>' mode='w' \
                     encoding='utf-8'>\nBrokenPipeError: [Errno 32] Broken pipe"
                ),
            },
            Self::Unavailable { detail } | Self::Io { detail, .. } => out.write_str(detail),
        }
    }
}

impl std::error::Error for StreamError {}

/// Write one reference `print(..., file=sys.stderr)` line.
///
/// The value is encoded exactly (the measured `backslashreplace` handler applies)
/// and delivered in one write with the reference's trailing newline. Encoding
/// happens before any byte is written, so an unencodable unit leaves stderr
/// untouched, exactly like the reference's single `str` write.
pub fn write_stderr_line(value: &PythonString) -> Result<(), StreamError> {
    let policy = StreamPolicy::for_stderr()?;
    let mut line = value.clone();
    line.push_scalars("\n");
    let bytes = policy.encode(&line)?;
    let mut stderr = io::stderr().lock();
    match stderr.write_all(&bytes) {
        Ok(()) => match stderr.flush() {
            Ok(()) => Ok(()),
            Err(error) => Err(stderr_write_error(&error)),
        },
        Err(error) => Err(stderr_write_error(&error)),
    }
}

/// Classify one failed stderr write with the measured reference phase.
fn stderr_write_error(error: &io::Error) -> StreamError {
    if error.kind() == io::ErrorKind::BrokenPipe || error.raw_os_error() == Some(32) {
        StreamError::BrokenPipe {
            class: WriteClass::Stderr,
        }
    } else {
        StreamError::Io {
            class: WriteClass::Stderr,
            detail: stderr_error_text(error),
        }
    }
}

/// The reference `OSError` text for one failed stderr write.
fn stderr_error_text(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("OSError: [Errno {code}] {error}"),
        None => format!("OSError: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| OsString::from(value.clone()))
        }
    }

    fn text(codepoints: &[u32]) -> PythonString {
        PythonString::from_codepoints(codepoints).expect("valid code units")
    }

    fn policy(pairs: &[(&str, &str)]) -> StreamPolicy {
        StreamPolicy::resolve(&lookup(pairs)).expect("resolvable policy")
    }

    fn stderr_policy(pairs: &[(&str, &str)]) -> StreamPolicy {
        StreamPolicy::resolve_for(StreamKind::Stderr, &lookup(pairs))
            .expect("resolvable stderr policy")
    }

    const LOW: u32 = 0xDC80;
    const LOW_END: u32 = 0xDCFF;
    const HIGH: u32 = 0xD800;
    const E_ACUTE: u32 = 0x00E9;
    const SNOWMAN: u32 = 0x2603;
    const EMOJI: u32 = 0x1F600;
    const PUA: u32 = 0xE000;
    const NONCHARACTER: u32 = 0xFFFF;

    #[test]
    fn the_c_locale_family_default_is_utf8_surrogateescape() {
        let resolved = policy(&[("LC_ALL", "C.UTF-8")]);
        assert_eq!(resolved.encoding(), "utf-8");
        assert_eq!(resolved.errors(), "surrogateescape");
        assert_eq!(
            resolved.encode(&text(&[b'a' as u32, LOW, LOW_END, b'b' as u32])),
            Ok(b"a\x80\xffb".to_vec())
        );
    }

    #[test]
    fn an_explicit_utf8_handler_keeps_the_reference_bytes() {
        let strict = policy(&[("PYTHONIOENCODING", "utf-8:strict")]);
        assert_eq!(strict.errors(), "strict");
        assert!(matches!(
            strict.encode(&text(&[LOW])),
            Err(StreamError::UnicodeEncode { message })
                if message == "'utf-8' codec can't encode character '\\udc80' in position 0: surrogates not allowed"
        ));
        let escaped = policy(&[("PYTHONIOENCODING", "utf-8:surrogateescape")]);
        assert_eq!(escaped.encode(&text(&[LOW])), Ok(vec![0x80]));
        assert!(matches!(
            escaped.encode(&text(&[HIGH])),
            Err(StreamError::UnicodeEncode { .. })
        ));
        let backslash = policy(&[("PYTHONIOENCODING", "utf-8:backslashreplace")]);
        assert_eq!(
            backslash.encode(&text(&[LOW, HIGH])),
            Ok(b"\\udc80\\ud800".to_vec())
        );
        assert_eq!(
            backslash.encode(&text(&[EMOJI])),
            Ok("\u{1f600}".as_bytes().to_vec())
        );
        let replace = policy(&[("PYTHONIOENCODING", "utf-8:replace")]);
        assert_eq!(replace.encode(&text(&[LOW, HIGH])), Ok(b"??".to_vec()));
        let ignore = policy(&[("PYTHONIOENCODING", "utf-8:ignore")]);
        assert_eq!(ignore.encode(&text(&[b'a' as u32, LOW])), Ok(b"a".to_vec()));
        let xml = policy(&[("PYTHONIOENCODING", "utf-8:xmlcharrefreplace")]);
        assert_eq!(xml.encode(&text(&[LOW])), Ok(b"&#56448;".to_vec()));
        let pass = policy(&[("PYTHONIOENCODING", "utf-8:surrogatepass")]);
        assert_eq!(pass.encode(&text(&[LOW])), Ok(vec![0xED, 0xB2, 0x80]));
    }

    #[test]
    fn ascii_and_latin_codecs_use_the_reference_reasons() {
        let ascii = policy(&[("PYTHONIOENCODING", "ascii:strict")]);
        assert_eq!(ascii.encoding(), "ascii");
        assert!(matches!(
            ascii.encode(&text(&[E_ACUTE])),
            Err(StreamError::UnicodeEncode { message })
                if message == "'ascii' codec can't encode character '\\xe9' in position 0: ordinal not in range(128)"
        ));
        let replaced = policy(&[("PYTHONIOENCODING", "ascii:replace")]);
        assert_eq!(
            replaced.encode(&text(&[E_ACUTE, SNOWMAN, EMOJI])),
            Ok(b"???".to_vec())
        );
        let escaped = policy(&[("PYTHONIOENCODING", "ascii:surrogateescape")]);
        assert_eq!(escaped.encode(&text(&[LOW])), Ok(vec![0x80]));
        assert!(matches!(
            escaped.encode(&text(&[E_ACUTE])),
            Err(StreamError::UnicodeEncode { .. })
        ));
        let latin1 = policy(&[("PYTHONIOENCODING", "latin-1:strict")]);
        assert_eq!(latin1.encoding(), "iso8859-1");
        assert_eq!(latin1.encode(&text(&[E_ACUTE])), Ok(vec![0xE9]));
        assert!(matches!(
            latin1.encode(&text(&[SNOWMAN])),
            Err(StreamError::UnicodeEncode { message })
                if message == "'latin-1' codec can't encode character '\\u2603' in position 0: ordinal not in range(256)"
        ));
        let latin9 = policy(&[("PYTHONIOENCODING", "iso8859-15:strict")]);
        assert_eq!(latin9.encoding(), "iso8859-15");
        assert_eq!(latin9.encode(&text(&[0x20AC])), Ok(vec![0xA4]));
        assert!(matches!(
            latin9.encode(&text(&[0x00A4])),
            Err(StreamError::UnicodeEncode { message })
                if message == "'charmap' codec can't encode character '\\xa4' in position 0: character maps to <undefined>"
        ));
    }

    #[test]
    fn utf16_and_utf32_write_no_bom_and_apply_handlers_through_the_codec() {
        let utf16 = policy(&[("PYTHONIOENCODING", "utf-16")]);
        assert_eq!(
            utf16.encode(&text(&[b'h' as u32, b'i' as u32])),
            Ok(vec![0x68, 0x00, 0x69, 0x00])
        );
        let replaced = policy(&[("PYTHONIOENCODING", "utf-16:replace")]);
        assert_eq!(replaced.encode(&text(&[HIGH])), Ok(vec![0x3F, 0x00]));
        assert!(matches!(
            replaced.encode(&text(&[HIGH])),
            Ok(bytes) if bytes == vec![0x3F, 0x00]
        ));
        let escaped = policy(&[("PYTHONIOENCODING", "utf-16:surrogateescape")]);
        assert!(matches!(
            escaped.encode(&text(&[LOW])),
            Err(StreamError::UnicodeEncode { .. })
        ));
        let be = policy(&[("PYTHONIOENCODING", "utf-16-be")]);
        assert_eq!(be.encode(&text(&[b'h' as u32])), Ok(vec![0x00, 0x68]));
        let astral = policy(&[("PYTHONIOENCODING", "utf-16-le")]);
        assert_eq!(
            astral.encode(&text(&[EMOJI])),
            Ok(vec![0x3D, 0xD8, 0x00, 0xDE])
        );
        let utf32 = policy(&[("PYTHONIOENCODING", "utf-32-le")]);
        assert_eq!(
            utf32.encode(&text(&[b'h' as u32])),
            Ok(vec![0x68, 0x00, 0x00, 0x00])
        );
    }

    #[test]
    fn a_handler_is_resolved_only_when_it_is_needed() {
        let unused = policy(&[("PYTHONIOENCODING", "ascii:no-such-handler")]);
        assert_eq!(unused.errors(), "no-such-handler");
        assert_eq!(unused.encode(&text(&[b'o' as u32])), Ok(b"o".to_vec()));
        assert!(matches!(
            unused.encode(&text(&[E_ACUTE])),
            Err(StreamError::UnknownHandler { name, charmap: false }) if name == "no-such-handler"
        ));
        let charmap = policy(&[("PYTHONIOENCODING", "iso8859-15:no-such-handler")]);
        assert!(matches!(
            charmap.encode(&text(&[SNOWMAN])),
            Err(StreamError::UnknownHandler { charmap: true, .. })
        ));
        // Names are not normalised: the pinned interpreter rejects these.
        for name in ["REPLACE", "Strict", "backslash-replace", "strict "] {
            let setting = format!("utf-8:{name}");
            let resolved = policy(&[("PYTHONIOENCODING", &setting)]);
            assert!(matches!(
                resolved.encode(&text(&[LOW])),
                Err(StreamError::UnknownHandler { .. })
            ));
        }
    }

    #[test]
    fn namereplace_is_an_honest_gap_only_for_named_characters() {
        let named = policy(&[("PYTHONIOENCODING", "ascii:namereplace")]);
        assert_eq!(
            named.encode(&text(&[HIGH])),
            Ok(b"\\ud800".to_vec()),
            "the reference itself falls back to \\uXXXX for an unnamed unit"
        );
        assert!(matches!(
            named.encode(&text(&[SNOWMAN])),
            Err(StreamError::UnsupportedHandler {
                name: "namereplace"
            })
        ));
    }

    #[test]
    fn pythonioencoding_parsing_follows_the_measured_reference() {
        // Being explicit about the codec keeps `strict` when no handler is given.
        assert_eq!(policy(&[("PYTHONIOENCODING", "utf-8")]).errors(), "strict");
        assert_eq!(policy(&[("PYTHONIOENCODING", "utf-8:")]).errors(), "strict");
        // An empty codec part keeps the default codec but takes the handler.
        let kept = policy(&[
            ("LC_ALL", "C.UTF-8"),
            ("PYTHONIOENCODING", ":surrogateescape"),
        ]);
        assert_eq!(kept.encoding(), "utf-8");
        assert_eq!(kept.errors(), "surrogateescape");
        // Both parts empty is measured as "ignored".
        let ignored = policy(&[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", ":")]);
        assert_eq!(ignored.encoding(), "utf-8");
        assert_eq!(ignored.errors(), "surrogateescape");
        // An empty variable is absent.
        let empty = policy(&[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", "")]);
        assert_eq!(empty.errors(), "surrogateescape");
    }

    /// The measured stderr facts: same encoding resolution as stdout, but
    /// `backslashreplace` under EVERY probed setting, and no BOM.
    #[test]
    fn stderr_is_always_backslashreplace_and_never_carries_a_bom() {
        let settings = [
            ("LC_ALL", "C.UTF-8"),
            ("PYTHONIOENCODING", "utf-8"),
            ("PYTHONIOENCODING", "utf-8:strict"),
            ("PYTHONIOENCODING", "ascii"),
            ("PYTHONIOENCODING", "latin-1"),
            ("PYTHONIOENCODING", "ascii:replace"),
            ("PYTHONIOENCODING", "ascii:ignore"),
            ("PYTHONIOENCODING", "latin-1:xmlcharrefreplace"),
            ("PYTHONIOENCODING", "ascii:namereplace"),
            ("PYTHONIOENCODING", "utf-8:surrogateescape"),
            ("PYTHONIOENCODING", "utf-16"),
            ("PYTHONIOENCODING", "utf-32"),
        ];
        for pairs in settings {
            let resolved = stderr_policy(&[pairs]);
            assert_eq!(resolved.errors(), "backslashreplace", "{pairs:?}");
        }
        // UTF-8: a surrogate escapes, a PUA/noncharacter stays UTF-8.
        let utf8 = stderr_policy(&[("LC_ALL", "C.UTF-8")]);
        assert_eq!(utf8.encoding(), "utf-8");
        assert_eq!(
            utf8.encode(&text(&[LOW, PUA, NONCHARACTER])),
            Ok(b"\\udc80\xee\x80\x80\xef\xbf\xbf".to_vec())
        );
        // ASCII: every non-ASCII unit is escaped.
        let ascii = stderr_policy(&[("PYTHONIOENCODING", "ascii")]);
        assert_eq!(
            ascii.encode(&text(&[LOW, PUA, NONCHARACTER, E_ACUTE])),
            Ok(b"\\udc80\\ue000\\uffff\\xe9".to_vec())
        );
        // Latin-1 keeps the byte and escapes the rest; Latin-9 is a charmap.
        let latin1 = stderr_policy(&[("PYTHONIOENCODING", "latin-1")]);
        assert_eq!(
            latin1.encode(&text(&[E_ACUTE, PUA])),
            Ok(vec![0xE9, b'\\', b'u', b'e', b'0', b'0', b'0'])
        );
        let latin9 = stderr_policy(&[("PYTHONIOENCODING", "iso8859-15")]);
        assert_eq!(latin9.encode(&text(&[0x20AC])), Ok(vec![0xA4]));
        // UTF-16/32 carry no BOM, so two writes are the plain units.
        let utf16 = stderr_policy(&[("PYTHONIOENCODING", "utf-16")]);
        assert_eq!(utf16.encode(&text(&[b'W' as u32])), Ok(vec![0x57, 0x00]));
        let utf32 = stderr_policy(&[("PYTHONIOENCODING", "utf-32")]);
        assert_eq!(
            utf32.encode(&text(&[b'W' as u32])),
            Ok(vec![0x57, 0x00, 0x00, 0x00])
        );
        // The stderr policy keeps its kind and never resolves a surrogateescape.
        assert_eq!(utf8.kind(), StreamKind::Stderr);
        assert!(matches!(
            utf8.encode(&text(&[E_ACUTE])),
            Ok(bytes) if bytes == "\u{e9}".as_bytes().to_vec()
        ));
    }

    #[test]
    fn codec_names_normalise_like_encodings_and_refuse_the_rest() {
        for name in ["utf-8", "UTF8", "utf_8", " utf-8", "cp65001"] {
            assert_eq!(parse_codec(name).expect("utf-8 alias"), StreamCodec::Utf8);
        }
        assert_eq!(
            parse_codec("ISO-8859-15").expect("latin9"),
            StreamCodec::Latin9
        );
        assert_eq!(parse_codec("latin-1").expect("latin1"), StreamCodec::Latin1);
        assert_eq!(
            parse_codec("UTF-16-BE").expect("utf16be"),
            StreamCodec::Utf16Be
        );
        assert!(matches!(
            parse_codec("euc-jp"),
            Err(StreamError::UnsupportedEncoding { .. })
        ));
    }

    #[test]
    fn utf8_mode_and_the_c_locale_family_set_the_default_handler() {
        let explicit = policy(&[("LC_ALL", "en_US.UTF-8"), ("PYTHONUTF8", "1")]);
        assert_eq!(explicit.encoding(), "utf-8");
        assert_eq!(explicit.errors(), "surrogateescape");
        // A real locale with UTF-8 mode off is `strict`.
        let real = policy(&[("LC_ALL", "C.UTF8")]);
        assert_eq!(real.encoding(), "utf-8");
        assert_eq!(real.errors(), "strict");
        // The exact coercion-target spellings are the C family.
        for name in ["C.UTF-8", "C.utf8", "c.utf8"] {
            let family = policy(&[("LC_ALL", name)]);
            assert_eq!(family.encoding(), "utf-8", "{name}");
            assert_eq!(family.errors(), "surrogateescape", "{name}");
        }
        // The explicit C locale without UTF-8 mode is ASCII.
        let ascii = policy(&[
            ("LC_ALL", "C"),
            ("PYTHONUTF8", "0"),
            ("PYTHONCOERCECLOCALE", "0"),
        ]);
        assert_eq!(ascii.encoding(), "ascii");
        assert_eq!(ascii.errors(), "surrogateescape");
        assert_eq!(ascii.encode(&text(&[LOW])), Ok(vec![0x80]));
    }

    #[test]
    fn startup_failures_are_reported_before_any_write() {
        assert!(matches!(
            StreamPolicy::resolve(&lookup(&[("PYTHONIOENCODING", "no-such-codec")])),
            Err(StreamError::UnsupportedEncoding { .. })
        ));
        assert!(matches!(
            StreamPolicy::resolve(&lookup(&[("PYTHONUTF8", "abc")])),
            Err(StreamError::InvalidStartup { .. })
        ));
        assert!(matches!(
            StreamPolicy::resolve_for(
                StreamKind::Stderr,
                &lookup(&[("PYTHONIOENCODING", "no-such-codec")])
            ),
            Err(StreamError::UnsupportedEncoding { .. })
        ));
    }

    #[test]
    fn the_broken_pipe_exit_class_matches_the_write_class() {
        assert_eq!(
            StreamError::BrokenPipe {
                class: WriteClass::RichConsolePrint
            }
            .exit_code(),
            1
        );
        assert_eq!(
            StreamError::BrokenPipe {
                class: WriteClass::BuiltinPrint
            }
            .exit_code(),
            120
        );
        assert_eq!(
            StreamError::BrokenPipe {
                class: WriteClass::Stderr
            }
            .exit_code(),
            1
        );
        assert_eq!(
            StreamError::UnicodeEncode {
                message: "x".to_owned()
            }
            .exit_code(),
            1
        );
        assert_eq!(
            StreamError::Unavailable {
                detail: "OSError: [Errno 9] Bad file descriptor".to_owned()
            }
            .exit_code(),
            1
        );
        // A failed flush still pending at shutdown is CPython's exit 120.
        assert_eq!(
            StreamError::Io {
                class: WriteClass::RichConsolePrint,
                detail: "OSError: [Errno 28] No space left on device".to_owned()
            }
            .exit_code(),
            120
        );
        assert_eq!(
            StreamError::Io {
                class: WriteClass::Stderr,
                detail: "OSError: [Errno 28] No space left on device".to_owned()
            }
            .exit_code(),
            120
        );
        assert!(
            StreamError::UnicodeEncode {
                message: "x".to_owned()
            }
            .is_encode_failure()
        );
        assert!(
            !StreamError::Io {
                class: WriteClass::Stderr,
                detail: String::new()
            }
            .is_encode_failure()
        );
    }
}
