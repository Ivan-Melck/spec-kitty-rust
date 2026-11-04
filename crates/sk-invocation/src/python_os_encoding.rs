//! Application-owned POSIX filesystem-encoding authority for the Op message.
//!
//! The reference composes the Op commit message as a Python `str` and only
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
//! Implemented filesystem charsets:
//!
//! * UTF-8 and ASCII, with `surrogateescape`.
//! * ISO-8859-1, strictly (no Git-body re-interpretation): a scalar in
//!   `0x00..=0xFF` becomes its single byte, `\udc80..\udcff` stay raw bytes,
//!   and a higher scalar is truthfully unencodable.
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

use std::ffi::OsString;

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
    /// A real legacy charset whose ASCII bytes are identity-mapped.
    ///
    /// Only ASCII scalars and escaped bytes are implemented; a non-ASCII
    /// scalar needs the unimplemented codec.
    AsciiTransparent { codeset: String },
}

/// The startup context captured once for one service binding.
#[derive(Clone, Debug)]
pub(crate) struct OsEncodingContext {
    codec: Result<FilesystemCodec, String>,
}

/// Why the exact message code points cannot become a commit payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OsEncodeError {
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
    /// Called once when the Op service binds; the value is stable for the
    /// handle's lifetime, while the reference interpreter's decision is made
    /// at its own startup. The environment is only read.
    pub(crate) fn from_environment() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// Resolve a context from an environment lookup (the unit-test seam).
    fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        Self {
            codec: resolve(&lookup, &|name| os_locale::resolve(&lookup, name)),
        }
    }

    /// A short description of the captured context, for truthful refusals.
    pub(crate) fn label(&self) -> String {
        match &self.codec {
            Ok(FilesystemCodec::Utf8) => "utf-8, surrogateescape".to_owned(),
            Ok(FilesystemCodec::Ascii) => "ascii, surrogateescape".to_owned(),
            Ok(FilesystemCodec::Latin1) => "iso-8859-1, surrogateescape".to_owned(),
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
    pub(crate) fn encode(&self, codepoints: &[u32]) -> Result<Vec<u8>, OsEncodeError> {
        match &self.codec {
            Ok(codec) => codec.encode(codepoints),
            Err(detail) => Err(OsEncodeError::UnsupportedContext {
                detail: detail.clone(),
            }),
        }
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
    let pythonutf8 = pythonutf8(lookup)?;
    // `PYTHONUTF8=1` takes precedence and must not be pre-empted by an
    // irrelevant locale value that this adapter cannot interpret.
    if pythonutf8 == Some(true) {
        return Ok(FilesystemCodec::Utf8);
    }
    // POSIX precedence: the first non-empty of LC_ALL, LC_CTYPE, LANG. No
    // value means the C locale, as in libc.
    let lc_all = value(lookup, "LC_ALL")?;
    let locale = match lc_all.clone() {
        Some(locale) => Some(locale),
        None => match value(lookup, "LC_CTYPE")? {
            Some(locale) => Some(locale),
            None => value(lookup, "LANG")?,
        },
    };
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
            // CPython compares this getenv flag byte-for-byte with "0";
            // other bytes enable coercion, and resolved locales never need it.
            let coercion_disabled = lookup("PYTHONCOERCECLOCALE")
                .is_some_and(|raw| raw.as_os_str() == std::ffi::OsStr::new("0"));
            if lc_all.is_some() || coercion_disabled {
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
}
