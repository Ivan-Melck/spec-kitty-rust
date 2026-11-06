//! Kernel-owned POSIX filesystem-encoding and native-locale authority.
//!
//! This module is the single owner of the startup filesystem codec and of the
//! exact `os.fsdecode` / `os.fsencode` behaviour it implies. The implementation
//! was promoted from `sk-invocation` with its notices, algorithms and measured
//! Source observations intact; `sk-invocation` now re-exports these same types
//! and functions instead of carrying a second codec. The reference composes the
//! Op commit message as a Python `str` and only
//! encodes it when `subprocess` builds the `git commit -m` argv. CPython uses
//! `sys.getfilesystemencoding()` with the `surrogateescape` error handler
//! there, determined once at interpreter startup by:
//!
//! * `PYTHONUTF8` (explicit UTF-8 mode on/off), which wins before the locale,
//! * otherwise the locale's `LC_ALL` > `LC_CTYPE` > `LANG` chain, where a
//!   `C`/`POSIX` locale enables the automatic UTF-8 mode (PEP 540),
//! * and, when UTF-8 mode is off, either the locale's charset or the PEP 538
//!   `C.UTF-8` coercion, which is skipped when `LC_ALL` is set and can be
//!   disabled with `PYTHONCOERCECLOCALE=0`.
//!
//! The charset is **not** inferred from the locale name's suffix. The one
//! native authority for whether a locale is installed and which codeset it
//! actually carries is [`crate::os_locale`], which reads the host's real
//! `LC_CTYPE` data (or reports a truthful uncertainty). Measured against the
//! pinned CPython 3.11.15 (see `OP-BYTE-BRIDGE-REPORT.md` and
//! `OP-LOCALE-REMEDIATION-REPORT.md`): `os.fsencode` maps `\udc80..\udcff` to
//! the single bytes `0x80..0xff`, encodes ordinary scalars as UTF-8 in a UTF-8
//! context, refuses non-ASCII scalars in the forced-ASCII context, encodes
//! `U+0000..=U+00FF` as their Latin-1 bytes in an actual ISO-8859-1 context,
//! and refuses every other surrogate with `UnicodeEncodeError`.
//!
//! The same captured context now also implements exact `os.fsdecode` for raw
//! filesystem bytes: UTF-8/ASCII use CPython's `surrogateescape`, actual
//! ISO-8859-1/ISO-8859-15 decode their complete byte range to the code points
//! Source produces, and an unimplemented legacy charset or non-Unix platform is
//! an explicit typed Native gap rather than `to_string_lossy`, U+FFFD, a fake
//! Source `UnicodeDecodeError`, or an empty/null path. `os.fsdecode(str)` is
//! identity and is not routed through the byte decoder.
//!
//! Implemented filesystem charsets:
//!
//! * UTF-8 and ASCII, with `surrogateescape`.
//! * Strict ISO-8859-1 and ISO-8859-15 (Latin-9), selected only from actual
//!   codeset data. Latin-9 has eight scalar replacements, not a Latin-1 fallback.
//!   Escaped bytes stay raw; unrepresentable scalars are refused truthfully.
//! * A bounded list of real legacy charsets that are ASCII-transparent (for
//!   example EUC-JP): an all-ASCII/escaped message stays publishable because
//!   its bytes are the same there, while a non-ASCII scalar is an honest
//!   native codec gap rather than a fabricated source refusal. A charset
//!   outside that proof refuses the whole message.
//!
//! Non-Unix platforms keep the existing UTF-8 context (a documented unverified
//! gap); non-glibc Unix and locale archives report native uncertainty and are
//! refused, never silently treated as a C fallback. The context is captured
//! once per service from the process environment; no environment variable is
//! mutated and no Python interpreter is executed. Git, not this module, owns
//! `i18n.commitEncoding` and any commit-object encoding header: the returned
//! bytes are the exact pre-conversion payload.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::PythonString;
use crate::os_locale::{self, LocaleFact, canonical_codeset};

/// The filesystem charset this adapter implements.
#[derive(Clone, Debug, Eq, PartialEq)]
enum FilesystemCodec {
    /// UTF-8 with `surrogateescape` (CPython `utf-8` filesystem encoding).
    Utf8,
    /// ASCII with `surrogateescape` (CPython `ascii` filesystem encoding).
    Ascii,
    /// ISO-8859-1 with `surrogateescape` (CPython `latin-1`).
    Latin1,
    /// ISO-8859-15 (Latin-9), including its eight replacements for Latin-1.
    Latin9,
    /// A real legacy charset whose ASCII bytes are identity-mapped.
    ///
    /// Only ASCII scalars and escaped bytes are implemented; a non-ASCII
    /// scalar needs the unimplemented codec.
    AsciiTransparent { codeset: String },
}

/// The startup context captured once for one service binding.
#[derive(Clone, Debug)]
pub struct OsEncodingContext {
    codec: Result<FilesystemCodec, String>,
}

/// Why the exact message code points cannot become a commit payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OsEncodeError {
    /// A code point the source's filesystem encoding cannot represent.
    ///
    /// `position` is the code-point index in the composed message, so the
    /// report is actionable without inventing a Python exception `repr`.
    Unencodable {
        /// The offending Python code unit.
        code: u32,
        /// Its index in the composed message.
        position: usize,
    },
    /// A source-valid startup context this adapter does not implement.
    UnsupportedContext {
        /// The measured context and why it is not implemented.
        detail: String,
    },
}

impl OsEncodingContext {
    /// Capture the current process's startup filesystem-encoding context.
    ///
    /// Called once when the Op service binds or a provenance journal is
    /// constructed; the value is stable for the handle's lifetime, while the
    /// reference interpreter's decision is made at its own startup. The
    /// environment is only read.
    pub fn from_environment() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// Resolve a context from an explicit, read-only environment lookup.
    ///
    /// This is the same startup rule as [`from_environment`](Self::from_environment),
    /// exposed so a caller (or a child test) can bind the exact locale variables
    /// without mutating process-global state.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        Self {
            codec: resolve(&lookup, &|name| os_locale::resolve(&lookup, name)),
        }
    }

    /// A short description of the captured context, for truthful refusals.
    pub fn label(&self) -> String {
        match &self.codec {
            Ok(FilesystemCodec::Utf8) => "utf-8, surrogateescape".to_owned(),
            Ok(FilesystemCodec::Ascii) => "ascii, surrogateescape".to_owned(),
            Ok(FilesystemCodec::Latin1) => "iso-8859-1, surrogateescape".to_owned(),
            Ok(FilesystemCodec::Latin9) => "iso-8859-15, surrogateescape".to_owned(),
            Ok(FilesystemCodec::AsciiTransparent { codeset }) => {
                format!("{codeset}, ASCII and escaped bytes only")
            }
            Err(detail) => format!("unsupported context: {detail}"),
        }
    }

    /// Encode the composed message's exact code points, late.
    ///
    /// `\udc80..\udcff` become the single bytes `0x80..0xff` (surrogateescape)
    /// in every implemented codec; every other surrogate is unencodable, as is
    /// any non-ASCII scalar in the ASCII codec. ISO-8859-1 maps
    /// `U+0000..=U+00FF` to its own byte. No replacement, lossy decode or
    /// Latin-1 reinterpretation happens outside an actual ISO-8859-1 context.
    pub fn encode(&self, codepoints: &[u32]) -> Result<Vec<u8>, OsEncodeError> {
        match &self.codec {
            Ok(codec) => codec.encode(codepoints),
            Err(detail) => Err(OsEncodeError::UnsupportedContext {
                detail: detail.clone(),
            }),
        }
    }

    /// Decode raw filesystem bytes exactly as Source `os.fsdecode(bytes)` does.
    ///
    /// UTF-8/ASCII use `surrogateescape`; ISO-8859-1/ISO-8859-15 decode every
    /// byte through their real mapping. A legacy charset this adapter cannot
    /// decode, an undecidable startup context, or a non-Unix platform is an
    /// explicit [`OsDecodeError`], never lossy text or a fabricated Source
    /// exception.
    pub fn decode(&self, bytes: &[u8]) -> Result<PythonString, OsDecodeError> {
        #[cfg(not(unix))]
        {
            let _ = (self, bytes);
            return Err(OsDecodeError::PlatformUnsupported {
                detail: "exact native filesystem bytes are not implemented on this platform"
                    .to_owned(),
            });
        }
        #[cfg(unix)]
        {
            match &self.codec {
                Ok(codec) => codec.decode(bytes),
                Err(detail) => Err(OsDecodeError::UnsupportedContext {
                    detail: detail.clone(),
                }),
            }
        }
    }

    /// Decode one native `OsStr` exactly as Source spells it after `os.fsdecode`.
    #[cfg(unix)]
    pub fn decode_os_str(&self, value: &OsStr) -> Result<PythonString, OsDecodeError> {
        use std::os::unix::ffi::OsStrExt;
        self.decode(value.as_bytes())
    }

    /// Decode one native `OsStr`; non-Unix wide-character paths are an explicit gap.
    #[cfg(not(unix))]
    pub fn decode_os_str(&self, value: &OsStr) -> Result<PythonString, OsDecodeError> {
        let _ = (self, value);
        Err(OsDecodeError::PlatformUnsupported {
            detail: "native wide-character paths are not implemented by this adapter".to_owned(),
        })
    }

    /// Source `str(Path(...))`: pathlib construction spelling precedes filename
    /// decoding. Unlike raw `decode(bytes)`, this drops redundant separators/dot
    /// components, but never resolves symlinks or lexical parent components.
    pub fn decode_path(&self, path: &Path) -> Result<PythonString, OsDecodeError> {
        #[cfg(unix)]
        let spelled = crate::paths::source_posix_path_spelling(path);
        #[cfg(unix)]
        let path = spelled.as_path();
        self.decode_os_str(path.as_os_str())
    }
}

impl FilesystemCodec {
    fn encode(&self, codepoints: &[u32]) -> Result<Vec<u8>, OsEncodeError> {
        let mut bytes = Vec::with_capacity(codepoints.len());
        for (position, &code) in codepoints.iter().enumerate() {
            if (0xDC80..=0xDCFF).contains(&code) {
                // CPython's `surrogateescape` maps escaped bytes back exactly.
                bytes.push((code - 0xDC00) as u8);
                continue;
            }
            match (self, char::from_u32(code)) {
                (FilesystemCodec::Utf8, Some(character)) => {
                    let mut buffer = [0u8; 4];
                    bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                }
                (FilesystemCodec::Ascii, Some(character)) if code < 0x80 => {
                    bytes.push(character as u8);
                }
                (FilesystemCodec::Latin1, Some(character)) if code <= 0xFF => {
                    bytes.push(character as u8);
                }
                (FilesystemCodec::Latin9, Some(_)) => match latin9_byte(code) {
                    Some(byte) => bytes.push(byte),
                    None => return Err(OsEncodeError::Unencodable { code, position }),
                },
                (FilesystemCodec::AsciiTransparent { .. }, Some(_)) if code < 0x80 => {
                    bytes.push(code as u8);
                }
                (FilesystemCodec::AsciiTransparent { codeset }, Some(_)) => {
                    return Err(OsEncodeError::UnsupportedContext {
                        detail: format!(
                            "the resolved locale charset `{codeset}` is implemented only for \
                             ASCII and escaped bytes; code point U+{code:04X} at index \
                             {position} needs that codec"
                        ),
                    });
                }
                _ => return Err(OsEncodeError::Unencodable { code, position }),
            }
        }
        Ok(bytes)
    }

    /// Decode raw bytes exactly as CPython's `os.fsdecode` does in this codec.
    fn decode(&self, bytes: &[u8]) -> Result<PythonString, OsDecodeError> {
        let mut units: Vec<u32> = Vec::with_capacity(bytes.len());
        match self {
            FilesystemCodec::Utf8 => return decode_utf8_surrogateescape(bytes),
            FilesystemCodec::Ascii => {
                for &byte in bytes {
                    units.push(if byte < 0x80 {
                        u32::from(byte)
                    } else {
                        0xDC00 + u32::from(byte)
                    });
                }
            }
            FilesystemCodec::Latin1 => {
                units.extend(bytes.iter().map(|byte| u32::from(*byte)));
            }
            FilesystemCodec::Latin9 => {
                units.extend(bytes.iter().map(|byte| latin9_scalar(*byte)));
            }
            FilesystemCodec::AsciiTransparent { codeset } => {
                for (position, &byte) in bytes.iter().enumerate() {
                    if byte >= 0x80 {
                        return Err(OsDecodeError::UnsupportedCodec {
                            codeset: codeset.clone(),
                            byte,
                            position,
                        });
                    }
                    units.push(u32::from(byte));
                }
            }
        }
        Ok(PythonString::from_units(units)
            .expect("filesystem codecs produce only valid Python string units"))
    }
}

/// Strict ISO-8859-15: the eight replaced Latin-1 scalars are unencodable,
/// while the replacement scalars map to their actual single bytes. Git still
/// owns any subsequent interpretation of the resulting payload.
///
/// Public as a **pure codec accessor** so another stream owner (for example the
/// CLI's stdout composition) borrows this one mapping instead of copying it.
pub fn latin9_byte(code: u32) -> Option<u8> {
    match code {
        0x20ac => Some(0xa4),
        0x0160 => Some(0xa6),
        0x0161 => Some(0xa8),
        0x017d => Some(0xb4),
        0x017e => Some(0xb8),
        0x0152 => Some(0xbc),
        0x0153 => Some(0xbd),
        0x0178 => Some(0xbe),
        0x00a4 | 0x00a6 | 0x00a8 | 0x00b4 | 0x00b8 | 0x00bc | 0x00bd | 0x00be => None,
        0..=0xff => Some(code as u8),
        _ => None,
    }
}

/// The inverse ISO-8859-15 byte-to-scalar mapping used by filesystem decoding.
fn latin9_scalar(byte: u8) -> u32 {
    match byte {
        0xa4 => 0x20ac,
        0xa6 => 0x0160,
        0xa8 => 0x0161,
        0xb4 => 0x017d,
        0xb8 => 0x017e,
        0xbc => 0x0152,
        0xbd => 0x0153,
        0xbe => 0x0178,
        other => u32::from(other),
    }
}

/// UTF-8 decode with CPython's `surrogateescape` error handler.
///
/// Source-probed rule: the valid prefix is kept as scalar code points and then
/// **one** offending byte becomes `U+DC00 + byte`, after which decoding resumes
/// at the following byte. `b"\xe2\x82"` therefore yields two escaped units, not
/// one range.
fn decode_utf8_surrogateescape(bytes: &[u8]) -> Result<PythonString, OsDecodeError> {
    let mut units: Vec<u32> = Vec::with_capacity(bytes.len());
    let mut offset = 0usize;
    while offset < bytes.len() {
        match crate::python_utf8::decode_utf8(&bytes[offset..]) {
            Ok(text) => {
                units.extend(text.chars().map(u32::from));
                break;
            }
            Err(error) => {
                if error.start > 0
                    && let Ok(text) =
                        crate::python_utf8::decode_utf8(&bytes[offset..offset + error.start])
                {
                    units.extend(text.chars().map(u32::from));
                }
                units.push(0xDC00 + u32::from(bytes[offset + error.start]));
                offset += error.start + 1;
            }
        }
    }
    Ok(
        PythonString::from_units(units)
            .expect("surrogateescape code points are always valid units"),
    )
}

/// Why raw filesystem bytes cannot be spelled as Source text.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OsDecodeError {
    /// The startup context itself is unsupported (bad `PYTHONUTF8`, unreadable
    /// native locale data, or an unimplemented charset).
    #[error("unsupported filesystem-decoding context: {detail}")]
    UnsupportedContext {
        /// The measured context and why it is not implemented.
        detail: String,
    },
    /// A real legacy charset this adapter does not decode; no byte is guessed.
    #[error(
        "the resolved locale charset `{codeset}` is not implemented for native filesystem decoding (byte 0x{byte:02X} at offset {position})"
    )]
    UnsupportedCodec {
        /// The actual codeset reported by the host locale data.
        codeset: String,
        /// The first byte whose decoding needs the unimplemented codec.
        byte: u8,
        /// Its byte offset in the native filename.
        position: usize,
    },
    /// Non-Unix paths are wide characters, not raw filesystem bytes; no lossy
    /// projection is offered as parity.
    #[error("native filesystem decoding is not implemented on this platform: {detail}")]
    PlatformUnsupported {
        /// Why the platform is outside this adapter's verified surface.
        detail: String,
    },
}

/// The startup locale state after the POSIX precedence chain and the PEP 540
/// preconfiguration, before any filesystem/stream classification.
///
/// Read-only: it captures the same environment values [`OsEncodingContext`]
/// consults, so a stream policy and the filesystem policy cannot disagree about
/// *which* locale was requested or whether UTF-8 mode is on.
struct StartupLocale {
    /// The `LC_ALL` value when it pins the locale (`PEP 538` coercion is skipped).
    lc_all: Option<String>,
    /// The effective request: first non-empty of `LC_ALL`, `LC_CTYPE`, `LANG`.
    requested: Option<String>,
    /// `PYTHONCOERCECLOCALE` is exactly `0`.
    coercion_disabled: bool,
    /// `PYTHONUTF8` as the pinned interpreter's startup accepts it.
    utf8_mode: Option<bool>,
}

fn startup_locale(lookup: &impl Fn(&str) -> Option<OsString>) -> Result<StartupLocale, String> {
    let utf8_mode = pythonutf8(lookup)?;
    let lc_all = value(lookup, "LC_ALL")?;
    let requested = match lc_all.clone() {
        Some(locale) => Some(locale),
        None => match value(lookup, "LC_CTYPE")? {
            Some(locale) => Some(locale),
            None => value(lookup, "LANG")?,
        },
    };
    // CPython compares this getenv flag byte-for-byte with "0"; other bytes
    // enable coercion, and resolved locales never need it.
    let coercion_disabled = lookup("PYTHONCOERCECLOCALE")
        .is_some_and(|raw| raw.as_os_str() == std::ffi::OsStr::new("0"));
    Ok(StartupLocale {
        lc_all,
        requested,
        coercion_disabled,
        utf8_mode,
    })
}

/// The effective startup locale request (first non-empty `LC_ALL`, `LC_CTYPE`,
/// `LANG`), or `None` for the C locale.
///
/// Pure: it reads the injected lookup only and applies no policy. A stream owner
/// uses it to decide whether the interpreter's automatic UTF-8 mode applies;
/// the real codeset still comes from [`locale_codeset`].
pub fn locale_request(
    lookup: &impl Fn(&str) -> Option<OsString>,
) -> Result<Option<String>, String> {
    Ok(startup_locale(lookup)?.requested)
}

/// `PYTHONUTF8` exactly as the pinned interpreter's startup accepts it.
///
/// `None` means unset (automatic UTF-8 mode is then decided by the locale);
/// `Some(true)`/`Some(false)` is the explicit mode. Any other value is an
/// unsupported startup value, never silently treated as unset.
pub fn utf8_mode_setting(
    lookup: &impl Fn(&str) -> Option<OsString>,
) -> Result<Option<bool>, String> {
    pythonutf8(lookup)
}

/// The real startup locale codeset, or the measured C-locale fact.
///
/// This is the locale fact only: it says nothing about the filesystem encoding,
/// and nothing about which codecs this adapter implements. A stream owner must
/// classify it itself rather than assume the filesystem encoding by name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocaleCodeset {
    /// libc is really in the C/POSIX locale (no installed codeset, or coercion
    /// was skipped/disabled).
    CLocale,
    /// A real locale resolved to this codeset (for example `UTF-8`).
    Resolved {
        /// The codeset name reported by the host locale data.
        codeset: String,
    },
    /// The native authority could not decide (unreadable locale archive, a
    /// path-like locale name), so nothing may be guessed.
    Uncertain {
        /// Why the fact is undecidable.
        detail: String,
    },
}

/// Resolve the startup locale to its real codeset through the same authority
/// the filesystem encoder uses.
///
/// It applies the same POSIX precedence and PEP 538/540 rules: an absent
/// explicit `PYTHONUTF8` in the C locale means the automatic UTF-8 mode (whose
/// stream encoding is UTF-8), and with UTF-8 mode explicitly off the first
/// actually installed coercion target (`C.UTF-8`, `C.utf8`, `UTF-8`) decides.
/// Pure read-only access: no environment mutation, no filesystem policy.
pub fn locale_codeset(lookup: &impl Fn(&str) -> Option<OsString>) -> LocaleCodeset {
    locale_codeset_with(lookup, &|name| os_locale::resolve(lookup, name))
}

fn locale_codeset_with(
    lookup: &impl Fn(&str) -> Option<OsString>,
    facts: &impl Fn(Option<&str>) -> LocaleFact,
) -> LocaleCodeset {
    // `PYTHONUTF8=1` decides the stream before any locale value is read (the
    // same precedence the filesystem resolver uses), so an uninterpretable
    // locale value cannot pre-empt it.
    match pythonutf8(lookup) {
        Ok(Some(true)) => {
            return LocaleCodeset::Resolved {
                codeset: "UTF-8".to_owned(),
            };
        }
        Ok(_) => {}
        Err(detail) => return LocaleCodeset::Uncertain { detail },
    }
    let startup = match startup_locale(lookup) {
        Ok(startup) => startup,
        Err(detail) => return LocaleCodeset::Uncertain { detail },
    };
    match facts(startup.requested.as_deref()) {
        // A failed lookup leaves libc in the C locale, exactly like an explicit
        // `LC_ALL=C`.
        LocaleFact::C | LocaleFact::Unavailable => {
            if startup.utf8_mode.is_none() {
                // PEP 540 automatic UTF-8 mode: the actual stream encoding is
                // UTF-8 even though libc is in the C locale.
                return LocaleCodeset::Resolved {
                    codeset: "UTF-8".to_owned(),
                };
            }
            if startup.lc_all.is_some() || startup.coercion_disabled {
                return LocaleCodeset::CLocale;
            }
            match coercion_codeset(facts) {
                Ok(Some(codeset)) => LocaleCodeset::Resolved { codeset },
                Ok(None) => LocaleCodeset::CLocale,
                Err(detail) => LocaleCodeset::Uncertain { detail },
            }
        }
        LocaleFact::Resolved { codeset } => LocaleCodeset::Resolved { codeset },
        LocaleFact::Uncertain { detail } => LocaleCodeset::Uncertain { detail },
    }
}

/// The codeset of the first PEP 538 coercion target that actually resolves.
///
/// `Ok(None)` means no target is installed (libc stays in the C locale).
fn coercion_codeset(facts: &impl Fn(Option<&str>) -> LocaleFact) -> Result<Option<String>, String> {
    for name in ["C.UTF-8", "C.utf8", "UTF-8"] {
        match facts(Some(name)) {
            LocaleFact::Resolved { codeset } => return Ok(Some(codeset)),
            LocaleFact::C | LocaleFact::Unavailable => {}
            LocaleFact::Uncertain { detail } => return Err(detail),
        }
    }
    Ok(None)
}

/// Resolve a startup context from an injected environment and locale facts.
///
/// Non-Unix platforms keep the existing UTF-8 context: wide-character argv is
/// not a byte-payload representation this adapter claims, and inventing a
/// refusal for a platform that is not verified here would be worse than the
/// documented gap.
#[cfg(not(unix))]
fn resolve(
    lookup: &impl Fn(&str) -> Option<OsString>,
    facts: &impl Fn(Option<&str>) -> LocaleFact,
) -> Result<FilesystemCodec, String> {
    let _ = (lookup, facts);
    Ok(FilesystemCodec::Utf8)
}

/// Resolve a startup context from an injected environment and locale facts.
#[cfg(unix)]
fn resolve(
    lookup: &impl Fn(&str) -> Option<OsString>,
    facts: &impl Fn(Option<&str>) -> LocaleFact,
) -> Result<FilesystemCodec, String> {
    // `PYTHONUTF8=1` takes precedence and must not be pre-empted by an
    // irrelevant locale value that this adapter cannot interpret; it is
    // therefore read before any locale value is decoded.
    if pythonutf8(lookup)? == Some(true) {
        return Ok(FilesystemCodec::Utf8);
    }
    let startup = startup_locale(lookup)?;
    let pythonutf8 = startup.utf8_mode;
    // POSIX precedence: the first non-empty of LC_ALL, LC_CTYPE, LANG. No
    // value means the C locale, as in libc.
    let lc_all = startup.lc_all.clone();
    let locale = startup.requested.clone();
    match facts(locale.as_deref()) {
        // A failed lookup leaves libc in the C locale, exactly like an explicit
        // `LC_ALL=C`; automatic UTF-8 mode depends on that actual state, not
        // on the requested text.
        LocaleFact::C | LocaleFact::Unavailable => {
            if pythonutf8.is_none() {
                // PEP 540 automatic UTF-8 mode for the actual C/POSIX locale.
                return Ok(FilesystemCodec::Utf8);
            }
            // UTF-8 mode is explicitly off. PEP 538 coercion runs only when
            // `LC_ALL` did not pin the locale and it is not disabled.
            if lc_all.is_some() || startup.coercion_disabled {
                return Ok(FilesystemCodec::Ascii);
            }
            coercion_target(facts)
        }
        LocaleFact::Resolved { codeset } => classify_codeset(&codeset),
        LocaleFact::Uncertain { detail } => Err(detail),
    }
}

/// The first coercion target that actually exists decides the charset.
///
/// CPython tries `C.UTF-8`, `C.utf8`, then `UTF-8`; when none is installed it
/// stays in the C locale. The bytes are only used when a target really
/// resolved, so a host without the target gets ASCII with UTF-8 mode off.
fn coercion_target(facts: &impl Fn(Option<&str>) -> LocaleFact) -> Result<FilesystemCodec, String> {
    for name in ["C.UTF-8", "C.utf8", "UTF-8"] {
        match facts(Some(name)) {
            LocaleFact::Resolved { codeset } => {
                return classify_codeset(&codeset).map_err(|detail| {
                    format!("the PEP 538 coercion target `{name}` resolved, but {detail}")
                });
            }
            LocaleFact::C | LocaleFact::Unavailable => {}
            LocaleFact::Uncertain { detail } => return Err(detail),
        }
    }
    Ok(FilesystemCodec::Ascii)
}

/// Map a real codeset name to the codec this adapter implements.
fn classify_codeset(codeset: &str) -> Result<FilesystemCodec, String> {
    match canonical_codeset(codeset).as_deref() {
        Some("utf-8") => Ok(FilesystemCodec::Utf8),
        Some("ascii") => Ok(FilesystemCodec::Ascii),
        Some("iso-8859-1") => Ok(FilesystemCodec::Latin1),
        Some("iso-8859-15") => Ok(FilesystemCodec::Latin9),
        Some(other) if other.starts_with("iso-8859-") => Ok(FilesystemCodec::AsciiTransparent {
            codeset: codeset.to_owned(),
        }),
        _ if ascii_transparent(codeset) => Ok(FilesystemCodec::AsciiTransparent {
            codeset: codeset.to_owned(),
        }),
        _ => Err(format!(
            "the resolved locale charset `{codeset}` is not implemented by this native adapter"
        )),
    }
}

/// Real legacy charsets whose ASCII bytes are the ASCII code points.
///
/// This is a bounded, named list, not a claim about every legacy charset: an
/// unknown charset is refused instead of assumed ASCII-compatible.
fn ascii_transparent(codeset: &str) -> bool {
    let normalized: String = codeset
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    matches!(
        normalized.as_str(),
        "eucjp"
            | "euckr"
            | "sjis"
            | "shiftjis"
            | "cp932"
            | "big5"
            | "big5hkscs"
            | "gbk"
            | "gb18030"
            | "gb2312"
            | "koi8r"
            | "koi8u"
            | "tis620"
            | "cp874"
            | "cp1250"
            | "cp1251"
            | "cp1252"
            | "cp1253"
            | "cp1254"
            | "cp1255"
            | "cp1256"
            | "cp1257"
            | "cp1258"
            | "iso2022jp"
            | "iso2022kr"
            | "iso2022cn"
            | "macroman"
            | "cp437"
            | "cp850"
            | "cp866"
            | "armscii8"
            | "georgianps"
    )
}

/// The non-empty environment value of `name`.
///
/// An unrepresentable native value (not decodable as text) is an unsupported
/// context rather than silently treated as absent: guessing here would pick a
/// codec the source never chose.
fn value(lookup: &impl Fn(&str) -> Option<OsString>, name: &str) -> Result<Option<String>, String> {
    match lookup(name) {
        None => Ok(None),
        Some(raw) => match raw.into_string() {
            Ok(text) => Ok((!text.is_empty()).then_some(text)),
            Err(_) => Err(format!(
                "{name} is not a text value this native adapter can interpret"
            )),
        },
    }
}

/// `PYTHONUTF8` exactly as the pinned interpreter's startup accepts it.
fn pythonutf8(lookup: &impl Fn(&str) -> Option<OsString>) -> Result<Option<bool>, String> {
    match value(lookup, "PYTHONUTF8")? {
        None => Ok(None),
        Some(text) if text == "0" => Ok(Some(false)),
        Some(text) if text == "1" => Ok(Some(true)),
        Some(text) => Err(format!(
            "PYTHONUTF8={text} is not a startup value the pinned interpreter accepts"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Pairs<'a> = &'a [(&'a str, &'a str)];

    fn lookup(pairs: Pairs<'_>) -> impl Fn(&str) -> Option<OsString> {
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

    fn context_matching(
        pairs: Pairs<'_>,
        facts: impl Fn(Option<&str>) -> LocaleFact,
    ) -> OsEncodingContext {
        let lookup = lookup(pairs);
        OsEncodingContext {
            codec: resolve(&lookup, &facts),
        }
    }

    /// Facts for a host whose C coercion target is installed as UTF-8.
    fn utf8_host(name: Option<&str>) -> LocaleFact {
        match name {
            None | Some("C" | "POSIX") => LocaleFact::C,
            Some(_) => LocaleFact::Resolved {
                codeset: "UTF-8".to_owned(),
            },
        }
    }

    /// Facts for a host with no coercion target and no installed locale.
    fn bare_host(name: Option<&str>) -> LocaleFact {
        match name {
            None | Some("C" | "POSIX") => LocaleFact::C,
            Some(_) => LocaleFact::Unavailable,
        }
    }

    fn scalars(text: &str) -> Vec<u32> {
        text.chars().map(u32::from).collect()
    }

    const DC80: u32 = 0xDC80;
    const DCFF: u32 = 0xDCFF;

    #[test]
    fn utf8_contexts_are_discovered_like_the_pinned_interpreter() {
        for pairs in [
            &[][..],
            &[("LC_CTYPE", "C.UTF-8")],
            &[("LC_ALL", "C.UTF-8"), ("PYTHONUTF8", "0")],
            &[("LC_ALL", "C")],
            &[("LC_ALL", "C"), ("PYTHONCOERCECLOCALE", "0")],
            &[("LC_ALL", "C"), ("PYTHONUTF8", "1")],
            &[("LC_CTYPE", "C"), ("PYTHONUTF8", "0")],
            &[("LC_ALL", "en_US.UTF-8")],
            &[("LANG", "C.UTF-8@euro")],
        ] {
            let captured = context_matching(pairs, utf8_host);
            assert_eq!(
                captured.encode(&scalars("a\u{00E9}")),
                Ok("a\u{00E9}".as_bytes().to_vec()),
                "{pairs:?} is a UTF-8 context"
            );
            assert_eq!(captured.encode(&[DC80]), Ok(vec![0x80]));
        }
    }

    #[test]
    fn forced_ascii_contexts_match_the_pinned_interpreter() {
        for pairs in [
            &[("LC_ALL", "C"), ("PYTHONUTF8", "0")][..],
            &[
                ("LC_ALL", "C"),
                ("PYTHONUTF8", "0"),
                ("PYTHONCOERCECLOCALE", "0"),
            ],
            &[
                ("LC_CTYPE", "C"),
                ("PYTHONUTF8", "0"),
                ("PYTHONCOERCECLOCALE", "0"),
            ],
            // No coercion target is installed: mode 0 stays ASCII.
            &[("LC_CTYPE", "C"), ("PYTHONUTF8", "0")],
        ] {
            let captured = context_matching(pairs, bare_host);
            assert_eq!(captured.encode(&scalars("op(a)")), Ok(b"op(a)".to_vec()));
            assert_eq!(captured.encode(&[0x7F]), Ok(vec![0x7F]));
            assert_eq!(captured.encode(&[DC80, DCFF]), Ok(vec![0x80, 0xFF]));
            assert_eq!(
                captured.encode(&[0x00E9]),
                Err(OsEncodeError::Unencodable {
                    code: 0x00E9,
                    position: 0,
                }),
                "{pairs:?} is an ASCII context"
            );
        }
    }

    #[test]
    fn utf8_mode_override_wins_before_an_undecidable_locale() {
        let captured = context_matching(
            &[("LC_ALL", "xx_YY.BOGUS-8"), ("PYTHONUTF8", "1")],
            |name| match name {
                Some("xx_YY.BOGUS-8") => LocaleFact::Uncertain {
                    detail: "the locale name is a path".to_owned(),
                },
                _ => LocaleFact::C,
            },
        );
        assert_eq!(
            captured.encode(&scalars("a\u{00E9}")),
            Ok("a\u{00E9}".as_bytes().to_vec())
        );
        assert_eq!(captured.label(), "utf-8, surrogateescape");
    }

    #[test]
    fn an_unavailable_locale_is_the_real_c_fallback() {
        // No explicit mode: the actual C locale enables PEP 540 auto UTF-8,
        // even though the requested name says neither C nor UTF-8.
        let auto = context_matching(&[("LC_ALL", "xx_YY.BOGUS-8")], bare_host);
        assert_eq!(
            auto.encode(&scalars("\u{00E9}")),
            Ok("\u{00E9}".as_bytes().to_vec())
        );
        // Mode 0 with LC_ALL pinning: the source really is ASCII.
        let forced = context_matching(
            &[
                ("LC_ALL", "xx_YY.UTF-8"),
                ("PYTHONUTF8", "0"),
                ("PYTHONCOERCECLOCALE", "0"),
            ],
            bare_host,
        );
        assert_eq!(
            forced.encode(&scalars("\u{00E9}")),
            Err(OsEncodeError::Unencodable {
                code: 0x00E9,
                position: 0,
            })
        );
        assert_eq!(forced.encode(&[DC80]), Ok(vec![0x80]));
    }

    #[test]
    fn coercion_uses_only_an_actually_available_target() {
        let enabled = &[("LC_CTYPE", "xx_YY.BOGUS-8"), ("PYTHONUTF8", "0")][..];
        let available = context_matching(enabled, |name| match name {
            Some("C.UTF-8") => LocaleFact::Resolved {
                codeset: "UTF-8".to_owned(),
            },
            Some(_) => LocaleFact::Unavailable,
            None => LocaleFact::C,
        });
        assert_eq!(
            available.encode(&scalars("\u{00E9}")),
            Ok("\u{00E9}".as_bytes().to_vec())
        );
        let unavailable = context_matching(enabled, |name| match name {
            None | Some("C" | "POSIX") => LocaleFact::C,
            Some(_) => LocaleFact::Unavailable,
        });
        assert_eq!(
            unavailable.encode(&scalars("\u{00E9}")),
            Err(OsEncodeError::Unencodable {
                code: 0x00E9,
                position: 0,
            })
        );
        let uncertain = context_matching(enabled, |name| match name {
            Some("C.UTF-8") => LocaleFact::Uncertain {
                detail: "coercion target data is unreadable".to_owned(),
            },
            Some(_) => LocaleFact::Unavailable,
            None => LocaleFact::C,
        });
        assert!(matches!(
            uncertain.encode(&scalars("a")),
            Err(OsEncodeError::UnsupportedContext { detail })
                if detail.contains("unreadable")
        ));
    }

    #[test]
    fn an_actual_latin1_locale_is_strict_iso8859_1() {
        let latin1 = context_matching(&[("LC_ALL", "en_US.ISO-8859-1")], |name| match name {
            Some("en_US.ISO-8859-1") => LocaleFact::Resolved {
                codeset: "ISO-8859-1".to_owned(),
            },
            _ => LocaleFact::C,
        });
        assert_eq!(
            latin1.encode(&[b'o' as u32, 0x00E9, DC80, DCFF]),
            Ok(b"o\xE9\x80\xFF".to_vec())
        );
        assert_eq!(
            latin1.encode(&scalars("implementer")),
            Ok(b"implementer".to_vec())
        );
        assert_eq!(
            latin1.encode(&[0x0100]),
            Err(OsEncodeError::Unencodable {
                code: 0x0100,
                position: 0,
            })
        );
        assert_eq!(latin1.label(), "iso-8859-1, surrogateescape");
    }

    #[test]
    fn latin9_preserves_every_byte_and_rejects_displaced_latin1_scalars() {
        let codec = FilesystemCodec::Latin9;
        for byte in 0u8..=255 {
            let code = match byte {
                0xa4 => 0x20ac,
                0xa6 => 0x0160,
                0xa8 => 0x0161,
                0xb4 => 0x017d,
                0xb8 => 0x017e,
                0xbc => 0x0152,
                0xbd => 0x0153,
                0xbe => 0x0178,
                _ => u32::from(byte),
            };
            assert_eq!(codec.encode(&[code]), Ok(vec![byte]));
        }
        for code in [
            0x00a4, 0x00a6, 0x00a8, 0x00b4, 0x00b8, 0x00bc, 0x00bd, 0x00be, 0x0100, 0xd800, 0xdc7f,
        ] {
            assert_eq!(
                codec.encode(&[code]),
                Err(OsEncodeError::Unencodable { code, position: 0 })
            );
        }
        for code in 0xdc80..=0xdcff {
            assert_eq!(codec.encode(&[code]), Ok(vec![(code - 0xdc00) as u8]));
        }
    }

    #[test]
    fn an_ascii_transparent_charset_keeps_ascii_and_escape_bytes() {
        let eucjp = context_matching(&[("LC_ALL", "ja_JP.EUC-JP")], |name| match name {
            Some("ja_JP.EUC-JP") => LocaleFact::Resolved {
                codeset: "EUC-JP".to_owned(),
            },
            _ => LocaleFact::C,
        });
        assert_eq!(
            eucjp.encode(&[b'o' as u32, b'p' as u32, DC80, DCFF]),
            Ok(b"op\x80\xFF".to_vec())
        );
        assert!(matches!(
            eucjp.encode(&scalars("\u{00E9}")),
            Err(OsEncodeError::UnsupportedContext { detail })
                if detail.contains("EUC-JP") && detail.contains("U+00E9")
        ));
        assert_eq!(eucjp.label(), "EUC-JP, ASCII and escaped bytes only");
    }

    #[test]
    fn an_unimplemented_or_uncertain_native_fact_is_refused_not_guessed() {
        let mystery = context_matching(&[("LC_ALL", "zz_ZZ.MYSTERY-8")], |name| match name {
            Some("zz_ZZ.MYSTERY-8") => LocaleFact::Resolved {
                codeset: "MYSTERY-8".to_owned(),
            },
            _ => LocaleFact::C,
        });
        assert!(matches!(
            mystery.encode(&scalars("op(a)")),
            Err(OsEncodeError::UnsupportedContext { detail })
                if detail.contains("MYSTERY-8")
        ));
        assert!(mystery.label().starts_with("unsupported context:"));
        let uncertain = context_matching(&[("LC_ALL", "en_US.ISO-8859-2")], |name| match name {
            Some("en_US.ISO-8859-2") => LocaleFact::Uncertain {
                detail: "the codeset relation is undecidable".to_owned(),
            },
            _ => LocaleFact::C,
        });
        assert!(matches!(
            uncertain.encode(&scalars("op(a)")),
            Err(OsEncodeError::UnsupportedContext { detail })
                if detail.contains("undecidable")
        ));
    }

    #[test]
    fn surrogateescape_maps_only_the_escaped_range_to_raw_bytes() {
        let captured = context_matching(&[("LC_ALL", "C.UTF-8")], utf8_host);
        assert_eq!(
            captured.encode(&[b'o' as u32, DC80, 0x00E9, 0x1F600, DCFF]),
            Ok(b"o\x80\xc3\xa9\xf0\x9f\x98\x80\xff".to_vec())
        );
        for code in [0xD800, 0xDBFF, 0xDC00, 0xDC7F, 0xDD00, 0xDFFF] {
            assert_eq!(
                captured.encode(&[code]),
                Err(OsEncodeError::Unencodable { code, position: 0 })
            );
        }
        assert_eq!(
            captured.encode(&[0x006F, 0xD800]),
            Err(OsEncodeError::Unencodable {
                code: 0xD800,
                position: 1,
            }),
            "the first failing code point and its position are reported"
        );
        // NUL is a valid byte payload here; the Git payload guard owns it.
        assert_eq!(captured.encode(&[0x0000]), Ok(vec![0x00]));
    }

    #[test]
    fn labels_name_the_measured_codec() {
        assert_eq!(
            context_matching(&[("LC_ALL", "C.UTF-8")], utf8_host).label(),
            "utf-8, surrogateescape"
        );
        assert_eq!(
            context_matching(
                &[
                    ("LC_ALL", "C"),
                    ("PYTHONUTF8", "0"),
                    ("PYTHONCOERCECLOCALE", "0"),
                ],
                utf8_host,
            )
            .label(),
            "ascii, surrogateescape"
        );
        assert!(
            context_matching(&[("LC_ALL", "en_US.ISO-8859-2")], |name| match name {
                Some("en_US.ISO-8859-2") => LocaleFact::Uncertain {
                    detail: "codeset relation undecidable".to_owned(),
                },
                _ => LocaleFact::C,
            })
            .label()
            .starts_with("unsupported context:")
        );
    }

    /// A non-text environment value must not be silently treated as absent.
    #[cfg(unix)]
    #[test]
    fn an_unrepresentable_environment_value_is_unsupported() {
        use std::os::unix::ffi::OsStringExt;
        let captured = OsEncodingContext::from_lookup(|name| {
            (name == "LC_ALL").then(|| OsString::from_vec(b"en_US.\xff8".to_vec()))
        });
        assert!(matches!(
            captured.encode(&scalars("a")),
            Err(OsEncodeError::UnsupportedContext { detail })
                if detail.contains("LC_ALL")
        ));
    }

    /// `PYTHONUTF8=1` must not be pre-empted by an irrelevant locale value
    /// this adapter cannot interpret.
    #[cfg(unix)]
    #[test]
    fn utf8_override_ignores_an_uninterpretable_locale_value() {
        use std::os::unix::ffi::OsStringExt;
        let captured = OsEncodingContext::from_lookup(|name| match name {
            "PYTHONUTF8" => Some(OsString::from("1")),
            "LC_ALL" => Some(OsString::from_vec(b"en_US.\xff8".to_vec())),
            _ => None,
        });
        assert_eq!(
            captured.encode(&scalars("a\u{00E9}")),
            Ok("a\u{00E9}".as_bytes().to_vec())
        );
    }

    #[test]
    fn invalid_startup_values_and_empty_values_stay_truthful() {
        let invalid = context_matching(&[("PYTHONUTF8", "abc")], bare_host);
        assert!(matches!(
            invalid.encode(&[0x0061]),
            Err(OsEncodeError::UnsupportedContext { detail })
                if detail.contains("PYTHONUTF8=abc")
        ));
        // An empty value is absent, exactly like the C environment API.
        let empty = context_matching(
            &[("LC_ALL", ""), ("PYTHONUTF8", ""), ("LC_CTYPE", "en_US")],
            |name| match name {
                Some("en_US") => LocaleFact::Resolved {
                    codeset: "ISO-8859-1".to_owned(),
                },
                None => LocaleFact::C,
                Some(_) => LocaleFact::Unavailable,
            },
        );
        assert_eq!(empty.encode(&[0x00E9]), Ok(vec![0xE9]));
        assert_eq!(empty.label(), "iso-8859-1, surrogateescape");
    }

    #[test]
    fn the_startup_locale_request_follows_posix_precedence() {
        assert_eq!(locale_request(&lookup(&[])), Ok(None));
        assert_eq!(
            locale_request(&lookup(&[("LANG", "en_US.UTF-8")])),
            Ok(Some("en_US.UTF-8".to_owned()))
        );
        assert_eq!(
            locale_request(&lookup(&[("LC_CTYPE", "C.UTF-8"), ("LANG", "x")])),
            Ok(Some("C.UTF-8".to_owned()))
        );
        assert_eq!(
            locale_request(&lookup(&[("LC_ALL", "C"), ("LC_CTYPE", "C.UTF-8")])),
            Ok(Some("C".to_owned()))
        );
        // An empty value is absent, exactly like the C environment API.
        assert_eq!(
            locale_request(&lookup(&[("LC_ALL", ""), ("LC_CTYPE", "C.UTF-8")])),
            Ok(Some("C.UTF-8".to_owned()))
        );
    }

    #[test]
    fn the_utf8_mode_setting_is_the_pinned_startup_value() {
        assert_eq!(utf8_mode_setting(&lookup(&[])), Ok(None));
        assert_eq!(
            utf8_mode_setting(&lookup(&[("PYTHONUTF8", "1")])),
            Ok(Some(true))
        );
        assert_eq!(
            utf8_mode_setting(&lookup(&[("PYTHONUTF8", "0")])),
            Ok(Some(false))
        );
        assert!(matches!(
            utf8_mode_setting(&lookup(&[("PYTHONUTF8", "abc")])),
            Err(detail) if detail.contains("PYTHONUTF8=abc")
        ));
    }

    #[test]
    fn the_locale_codeset_is_a_real_fact_not_a_filesystem_alias() {
        // The C locale with no explicit mode is the automatic UTF-8 mode.
        assert_eq!(
            locale_codeset(&lookup(&[])),
            LocaleCodeset::Resolved {
                codeset: "UTF-8".to_owned(),
            }
        );
        // A real locale reports the codeset the locale data carries.
        let real = |name: Option<&str>| match name {
            Some("en_US.UTF-8") => LocaleFact::Resolved {
                codeset: "UTF-8".to_owned(),
            },
            None | Some("C" | "POSIX") => LocaleFact::C,
            Some(_) => LocaleFact::Unavailable,
        };
        assert_eq!(
            locale_codeset_with(&lookup(&[("LC_ALL", "en_US.UTF-8")]), &real),
            LocaleCodeset::Resolved {
                codeset: "UTF-8".to_owned(),
            }
        );
        // The ASCII-capable legacy locale is a real Latin-9 codeset.
        let latin9 = |name: Option<&str>| match name {
            Some("en_US.iso885915") => LocaleFact::Resolved {
                codeset: "ISO-8859-15".to_owned(),
            },
            None | Some("C" | "POSIX") => LocaleFact::C,
            Some(_) => LocaleFact::Unavailable,
        };
        assert_eq!(
            locale_codeset_with(&lookup(&[("LC_ALL", "en_US.iso885915")]), &latin9),
            LocaleCodeset::Resolved {
                codeset: "ISO-8859-15".to_owned(),
            }
        );
        // UTF-8 mode explicitly off with `LC_ALL` pinning C stays the C locale.
        assert_eq!(
            locale_codeset_with(&lookup(&[("LC_ALL", "C"), ("PYTHONUTF8", "0")]), &utf8_host),
            LocaleCodeset::CLocale
        );
        // ... while the coercion target really decides when `LC_ALL` is unset.
        assert_eq!(
            locale_codeset_with(&lookup(&[("PYTHONUTF8", "0")]), &utf8_host),
            LocaleCodeset::Resolved {
                codeset: "UTF-8".to_owned(),
            }
        );
        assert_eq!(
            locale_codeset_with(&lookup(&[("PYTHONUTF8", "0")]), &bare_host),
            LocaleCodeset::CLocale
        );
        // An undecidable host fact is uncertain, never a C fallback.
        assert!(matches!(
            locale_codeset_with(&lookup(&[("LC_ALL", "zz_ZZ.MYSTERY-8")]), &|name| match name {
                Some("zz_ZZ.MYSTERY-8") => LocaleFact::Uncertain {
                    detail: "undecidable".to_owned(),
                },
                _ => LocaleFact::C,
            }),
            LocaleCodeset::Uncertain { detail } if detail == "undecidable"
        ));
    }

    #[test]
    fn the_locale_codeset_does_not_pre_empt_utf8_mode() {
        use std::os::unix::ffi::OsStringExt;
        let captured = locale_codeset(&|name| match name {
            "PYTHONUTF8" => Some(OsString::from("1")),
            "LC_ALL" => Some(OsString::from_vec(b"en_US.\xff8".to_vec())),
            _ => None,
        });
        assert_eq!(
            captured,
            LocaleCodeset::Resolved {
                codeset: "UTF-8".to_owned(),
            }
        );
    }

    #[test]
    fn the_latin9_mapping_is_borrowable_pure_codec_access() {
        assert_eq!(latin9_byte(0x20ac), Some(0xa4));
        assert_eq!(latin9_byte(0x00e9), Some(0xe9));
        assert_eq!(latin9_byte(0x00a4), None);
        assert_eq!(latin9_byte(0x2603), None);
    }

    fn units(text: &str) -> Vec<u32> {
        text.chars().map(u32::from).collect()
    }

    #[test]
    fn filesystem_decode_utf8_vectors_match_the_pinned_interpreter() {
        let captured = context_matching(&[("LC_ALL", "C.UTF-8")], utf8_host);
        for (bytes, expected) in [
            (&b"caf\xe9"[..], vec![0x63, 0x61, 0x66, 0xdce9]),
            (&b"caf\xff"[..], vec![0x63, 0x61, 0x66, 0xdcff]),
            (&b"\xe2\x82"[..], vec![0xdce2, 0xdc82]),
            (&b"\xf0\x9f\x98"[..], vec![0xdcf0, 0xdc9f, 0xdc98]),
            (&b"\xe2(\xa1"[..], vec![0xdce2, 0x28, 0xdca1]),
            (&b"\xed\xa0\x80"[..], vec![0xdced, 0xdca0, 0xdc80]),
            (&b"a\x00b"[..], vec![0x61, 0x00, 0x62]),
            (&b"\x80"[..], vec![0xdc80]),
            (&b"\x7f"[..], vec![0x7f]),
        ] {
            assert_eq!(
                captured.decode(bytes).expect("decode").codepoints(),
                expected.as_slice(),
                "{bytes:?}"
            );
        }
        assert_eq!(
            captured
                .decode("caf\u{00e9} \u{1f600}".as_bytes())
                .expect("valid UTF-8")
                .codepoints(),
            &units("caf\u{00e9} \u{1f600}")
        );
    }

    #[test]
    fn filesystem_decode_ascii_uses_only_surrogateescape() {
        let captured = context_matching(
            &[
                ("LC_ALL", "C"),
                ("PYTHONUTF8", "0"),
                ("PYTHONCOERCECLOCALE", "0"),
            ],
            bare_host,
        );
        assert_eq!(
            captured.decode(b"caf\xe9").expect("decode").codepoints(),
            &[0x63, 0x61, 0x66, 0xdce9]
        );
        assert_eq!(
            captured.decode(b"\x80").expect("decode").codepoints(),
            &[0xdc80]
        );
        assert_eq!(
            captured.decode(b"op(a)\x7f").expect("decode").codepoints(),
            &[0x6f, 0x70, 0x28, 0x61, 0x29, 0x7f]
        );
    }

    #[test]
    fn filesystem_decode_latin1_and_latin9_are_total_real_mappings() {
        let latin1 = context_matching(&[("LC_ALL", "en_US.ISO-8859-1")], |name| match name {
            Some("en_US.ISO-8859-1") => LocaleFact::Resolved {
                codeset: "ISO-8859-1".to_owned(),
            },
            _ => LocaleFact::C,
        });
        for byte in [0x00u8, 0x7f, 0x80, 0xe9, 0xff] {
            assert_eq!(
                latin1.decode(&[byte]).expect("decode").codepoints(),
                &[u32::from(byte)],
                "ISO-8859-1 byte {byte:#04x}"
            );
        }

        let latin9 = context_matching(&[("LC_ALL", "en_US.iso885915")], |name| match name {
            Some("en_US.iso885915") => LocaleFact::Resolved {
                codeset: "ISO-8859-15".to_owned(),
            },
            _ => LocaleFact::C,
        });
        for (byte, scalar) in [
            (0xa4u8, 0x20acu32),
            (0xa6, 0x0160),
            (0xa8, 0x0161),
            (0xb4, 0x017d),
            (0xb8, 0x017e),
            (0xbc, 0x0152),
            (0xbd, 0x0153),
            (0xbe, 0x0178),
            (0xe9, 0x00e9),
            (0x80, 0x0080),
        ] {
            assert_eq!(
                latin9.decode(&[byte]).expect("decode").codepoints(),
                &[scalar],
                "ISO-8859-15 byte {byte:#04x}"
            );
        }
    }

    #[test]
    fn unimplemented_legacy_and_unsupported_contexts_are_explicit() {
        let eucjp = context_matching(&[("LC_ALL", "ja_JP.eucjp")], |name| match name {
            Some("ja_JP.eucjp") => LocaleFact::Resolved {
                codeset: "EUC-JP".to_owned(),
            },
            _ => LocaleFact::C,
        });
        assert_eq!(
            eucjp.decode(b"op").expect("ASCII prefix").codepoints(),
            &[0x6f, 0x70]
        );
        assert_eq!(
            eucjp.decode(b"op\xe9"),
            Err(OsDecodeError::UnsupportedCodec {
                codeset: "EUC-JP".to_owned(),
                byte: 0xe9,
                position: 2,
            })
        );

        let mystery = context_matching(&[("LC_ALL", "zz_ZZ.MYSTERY-8")], |name| match name {
            Some("zz_ZZ.MYSTERY-8") => LocaleFact::Resolved {
                codeset: "MYSTERY-8".to_owned(),
            },
            _ => LocaleFact::C,
        });
        assert!(matches!(
            mystery.decode(b"op"),
            Err(OsDecodeError::UnsupportedContext { detail }) if detail.contains("MYSTERY-8")
        ));
    }

    #[test]
    fn decode_encode_roundtrips_representative_valid_scalars() {
        let utf8 = context_matching(&[("LC_ALL", "C.UTF-8")], utf8_host);
        let text = "op\u{00e9}\u{1f600}";
        assert_eq!(
            utf8.decode(&utf8.encode(&units(text)).expect("encode"))
                .expect("decode")
                .codepoints(),
            &units(text)
        );
        assert_eq!(
            utf8.decode(&utf8.encode(&[0xdc80]).expect("encode"))
                .expect("decode")
                .codepoints(),
            &[0xdc80]
        );

        let ascii = context_matching(
            &[
                ("LC_ALL", "C"),
                ("PYTHONUTF8", "0"),
                ("PYTHONCOERCECLOCALE", "0"),
            ],
            bare_host,
        );
        assert_eq!(
            ascii
                .decode(&ascii.encode(&[0x6f, 0x70, 0xdc80]).expect("encode"))
                .expect("decode")
                .codepoints(),
            &[0x6f, 0x70, 0xdc80]
        );

        let latin1 = context_matching(&[("LC_ALL", "en_US.ISO-8859-1")], |name| match name {
            Some("en_US.ISO-8859-1") => LocaleFact::Resolved {
                codeset: "ISO-8859-1".to_owned(),
            },
            _ => LocaleFact::C,
        });
        assert_eq!(
            latin1
                .decode(
                    &latin1
                        .encode(&[0x6f, 0x70, 0x00e9, 0x0080])
                        .expect("encode")
                )
                .expect("decode")
                .codepoints(),
            &[0x6f, 0x70, 0x00e9, 0x0080]
        );

        let latin9 = context_matching(&[("LC_ALL", "en_US.iso885915")], |name| match name {
            Some("en_US.iso885915") => LocaleFact::Resolved {
                codeset: "ISO-8859-15".to_owned(),
            },
            _ => LocaleFact::C,
        });
        assert_eq!(
            latin9
                .decode(&latin9.encode(&[0x20ac, 0x00e9]).expect("encode"))
                .expect("decode")
                .codepoints(),
            &[0x20ac, 0x00e9]
        );
    }
}
