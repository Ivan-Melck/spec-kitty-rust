//! ONE provable text decoder and the separate newline normaliser
//! (Source `kernel.text_decode`, #4940/#4998).
//!
//! The pinned reference lives in the zero-dependency `kernel` root so every
//! layer can reach it, and it imports only the standard library. This module is
//! that authority's Rust home: no second decoder, no private per-consumer copy,
//! and no dependency on `charset_normalizer`-style guessing.
//!
//! **Provable encodings only.** [`decode_unambiguous`] never guesses. It
//! succeeds only when a byte-order mark or strict UTF-8 *proves* the encoding.
//! Everything else (single-byte code pages, ambiguous candidates, heuristic
//! detection) is out of scope here — that guessing layer belongs one level up in
//! the reference's `charter.encoding_recovery.recover`, which asks
//! `charset_normalizer`. User-owned text this kernel touches directly is either
//! decoded with proof or left alone.
//!
//! BOMs are checked longest/most-specific first: the UTF-32-LE BOM
//! (`FF FE 00 00`) begins with the UTF-16-LE BOM (`FF FE`), so UTF-32 must be
//! tried before UTF-16 or a UTF-32-LE file is silently mis-decoded as UTF-16-LE
//! with embedded NUL bytes. That ordering is the historical bug the reference
//! module documents, and it is asserted here.
//!
//! The UTF-8 branch reuses [`crate::python_utf8::decode_utf8`] (the canonical
//! strict-CPython decoder with its exact failure object/range/message) instead of
//! inventing a second diagnostic authority. UTF-16/UTF-32 reject truncated
//! units, unpaired surrogates, surrogate code points and values above
//! U+10FFFF, exactly as the strict CPython codecs do — a rejected payload makes
//! the whole decode unprovable (`None`); it never falls through to another
//! codec.
//!
//! Newline translation is deliberately a separate function
//! ([`normalize_newlines`]): decoding and universal-newline handling have
//! distinct failure phases, and callers that need a BOM strip or whitespace
//! trim do that themselves.
//!
//! ## Explicit file-read / provenance adapter
//!
//! [`read_text_with_provenance`] is the reference's `charter.activation._io`
//! read composition: read the bytes, decode with proof, then append the
//! encoding-provenance record — in that order, with the reference's phase
//! boundaries. It is an *adapter*: the three pure codecs above stay pure and
//! I/O-free, and the provenance lifecycle itself lives in
//! [`crate::encoding_provenance`]. A read that cannot be proven raises instead of
//! appending a record (the reference's `CHARTER_ENCODING_AMBIGUOUS` ordering),
//! and a caught provenance-write failure never changes the returned text.

use std::path::{Path, PathBuf};

use crate::KernelError;
use crate::PythonString;
use crate::encoding_provenance::{
    AMBIGUOUS_CODE, EncodingProvenanceJournal, EncodingProvenanceWarning, ReadEncodingFacts,
};
use crate::python_utf8::{decode_utf8, decode_utf8_sig};

/// The reported source encoding of an input that is **not** BOM-marked.
pub const UTF8: &str = "utf-8";

/// Which BOM shape was recognised (private; the public facts are the two
/// codec names).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BomShape {
    Utf32BigEndian,
    Utf32LittleEndian,
    Utf8Sig,
    Utf16LittleEndian,
    Utf16BigEndian,
}

/// A recognised byte-order mark: the decoding codec and the reported source
/// encoding (the reference's `(codec, source_encoding)` pair).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bom {
    shape: BomShape,
    codec: &'static str,
    source_encoding: &'static str,
}

impl Bom {
    /// The codec name the reference decodes with (`utf-32`, `utf-16`,
    /// `utf-8-sig`). `utf-16`/`utf-32` are BOM-driven in CPython.
    pub const fn codec(&self) -> &'static str {
        self.codec
    }

    /// The concrete encoding reported to callers (`utf-32-be`, `utf-16-le`,
    /// `utf-8-sig`, …). Distinct from [`codec`](Self::codec): the reference
    /// records *this* value when it needs to know whether the source was plain
    /// UTF-8.
    pub const fn source_encoding(&self) -> &'static str {
        self.source_encoding
    }
}

/// The recognised BOMs, longest/most-specific first.
///
/// Do not reorder: `FF FE 00 00` is both the UTF-32-LE BOM and a UTF-16-LE BOM
/// followed by a NUL unit.
const BOM_TABLE: &[(&[u8], BomShape, &str, &str)] = &[
    (
        b"\x00\x00\xfe\xff",
        BomShape::Utf32BigEndian,
        "utf-32",
        "utf-32-be",
    ),
    (
        b"\xff\xfe\x00\x00",
        BomShape::Utf32LittleEndian,
        "utf-32",
        "utf-32-le",
    ),
    (b"\xef\xbb\xbf", BomShape::Utf8Sig, "utf-8-sig", "utf-8-sig"),
    (
        b"\xff\xfe",
        BomShape::Utf16LittleEndian,
        "utf-16",
        "utf-16-le",
    ),
    (b"\xfe\xff", BomShape::Utf16BigEndian, "utf-16", "utf-16-be"),
];

/// Return the recognised [`Bom`] for `data`, or `None`.
///
/// Pure detection: this never decodes `data`, never validates the payload and
/// never falls through to a later entry once a prefix matched.
pub fn detect_bom(data: &[u8]) -> Option<Bom> {
    BOM_TABLE
        .iter()
        .find_map(|(prefix, shape, codec, encoding)| {
            data.starts_with(prefix).then_some(Bom {
                shape: *shape,
                codec,
                source_encoding: encoding,
            })
        })
}

/// Decode `data` only when a BOM or strict UTF-8 proves the encoding.
///
/// Returns `(text, source_encoding)` on success and `None` when the encoding
/// cannot be proven. Never guesses and never calls a charset detector.
///
/// - A recognised BOM is tried first. If the bytes following it fail to decode
///   under that codec, that is **not** proof of anything else, so this returns
///   `None` (it does not fall through to strict UTF-8).
/// - Otherwise strict `UTF-8` is tried. Success proves `utf-8`; failure returns
///   `None`.
/// - Empty bytes decode to `("", "utf-8")`.
/// - A BOM-only input decodes to `("", <its encoding>)`, as the CPython codecs
///   do.
///
/// Decoded text keeps every code point exactly — NUL, noncharacters such as
/// U+FFFE/U+FFFF, and any second U+FEFF that was authored after the BOM. A BOM
/// that belongs to the selected codec is consumed by that codec, never kept as
/// a stray character.
pub fn decode_unambiguous(data: &[u8]) -> Option<(String, &'static str)> {
    match detect_bom(data) {
        Some(bom) => decode_with_bom(data, bom).map(|text| (text, bom.source_encoding())),
        None => decode_utf8(data).ok().map(|text| (text.to_owned(), UTF8)),
    }
}

fn decode_with_bom(data: &[u8], bom: Bom) -> Option<String> {
    match bom.shape {
        BomShape::Utf8Sig => decode_utf8_sig(data).ok().map(str::to_owned),
        BomShape::Utf16LittleEndian => decode_utf16(&data[b"\xff\xfe".len()..], false),
        BomShape::Utf16BigEndian => decode_utf16(&data[b"\xfe\xff".len()..], true),
        BomShape::Utf32LittleEndian => decode_utf32(&data[b"\xff\xfe\x00\x00".len()..], false),
        BomShape::Utf32BigEndian => decode_utf32(&data[b"\x00\x00\xfe\xff".len()..], true),
    }
}

/// Strict UTF-16 payload decoding (the BOM is already consumed).
///
/// Rejects an odd trailing byte and any unpaired surrogate, like
/// `bytes.decode("utf-16")`.
fn decode_utf16(bytes: &[u8], big_endian: bool) -> Option<String> {
    let (units, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let units = units.iter().map(|pair| {
        if big_endian {
            u16::from_be_bytes(*pair)
        } else {
            u16::from_le_bytes(*pair)
        }
    });
    let mut text = String::with_capacity(bytes.len() / 2);
    for unit in char::decode_utf16(units) {
        text.push(unit.ok()?);
    }
    Some(text)
}

/// Strict UTF-32 payload decoding (the BOM is already consumed).
///
/// Rejects a truncated unit, a surrogate code point and any value above
/// U+10FFFF, like `bytes.decode("utf-32")`.
fn decode_utf32(bytes: &[u8], big_endian: bool) -> Option<String> {
    let (units, remainder) = bytes.as_chunks::<4>();
    if !remainder.is_empty() {
        return None;
    }
    let mut text = String::with_capacity(units.len());
    for unit in units {
        let scalar = if big_endian {
            u32::from_be_bytes(*unit)
        } else {
            u32::from_le_bytes(*unit)
        };
        text.push(char::from_u32(scalar)?);
    }
    Some(text)
}

/// Convert CRLF and lone CR to LF. Nothing else.
///
/// No BOM stripping and no whitespace trimming — callers that need those do
/// them separately around this call, exactly like the reference.
pub fn normalize_newlines(text: &str) -> String {
    crate::python_text::universal_newlines(text).into_owned()
}

/// One proven file read plus the provenance record it produced.
///
/// `text` is exact code points: the BOM of the selected codec is consumed, and
/// every other code point (NUL, noncharacters, a second authored U+FEFF) is
/// preserved verbatim. `warnings` carries a *caught* provenance-write failure
/// (the reference's `except OSError: logger.warning(...)`) — the read itself
/// still succeeded.
#[derive(Debug)]
pub struct ProvenancedText {
    text: PythonString,
    source_encoding: &'static str,
    confidence: f64,
    normalization_applied: bool,
    bypass_used: bool,
    source_path: PathBuf,
    log_path: Option<PathBuf>,
    warnings: Vec<EncodingProvenanceWarning>,
}

impl ProvenancedText {
    /// The decoded text as exact code points.
    pub fn text(&self) -> &PythonString {
        &self.text
    }

    /// The proven source encoding name (`utf-8`, `utf-8-sig`, `utf-16-le`, …).
    pub fn source_encoding(&self) -> &'static str {
        self.source_encoding
    }

    /// The detection confidence the decode reported.
    pub fn confidence(&self) -> f64 {
        self.confidence
    }

    /// Whether the bytes were re-encoded relative to plain UTF-8.
    pub fn normalization_applied(&self) -> bool {
        self.normalization_applied
    }

    /// Whether an operator override produced the text (never for this edge).
    pub fn bypass_used(&self) -> bool {
        self.bypass_used
    }

    /// The path that was read.
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// The log the record was appended to, when an append was attempted.
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    /// Caught provenance-write failures, in the reference's warning wording.
    pub fn warnings(&self) -> &[EncodingProvenanceWarning] {
        &self.warnings
    }

    /// The text as a Rust `str`, refusing a code point `str` cannot hold.
    ///
    /// A BOM/strict-UTF-8 decode never produces a surrogate, so this is total
    /// for every value this adapter returns; the `Result` exists so a caller
    /// never silently substitutes U+FFFD.
    pub fn to_scalar_string(&self) -> Result<String, u32> {
        self.text.to_scalar_string()
    }
}

/// A read that produced no provenance (the reference raises before writing).
#[derive(Debug, thiserror::Error)]
pub enum ProvenanceReadError {
    /// The bytes could not be read. The reference lets this `OSError` escape.
    #[error("failed to read {path}: {source}")]
    Read {
        /// The path that was read.
        path: PathBuf,
        /// The underlying read failure.
        #[source]
        source: KernelError,
    },
    /// The encoding is not proven (no BOM, not strict UTF-8). The reference
    /// raises `CHARTER_ENCODING_AMBIGUOUS` here and appends nothing.
    ///
    /// The reference's single-byte (cp1252) tie-break and its `unsafe` lossless
    /// bypass need a code-page authority this workspace does not carry yet, so
    /// this edge refuses instead of guessing. No record is written.
    #[error("ERROR: {code}\n  File: {path}\n  {detail}")]
    Unprovable {
        /// The fail-closed diagnostic code.
        code: &'static str,
        /// The path that was read.
        path: PathBuf,
        /// Which proof was missing.
        detail: String,
    },
    /// The record could not be minted. The reference builds the record outside
    /// its `try:` block, so this escapes there too; nothing is written.
    #[error("cannot record encoding provenance for {path}: {source}")]
    Record {
        /// The path that was read.
        path: PathBuf,
        /// The mint/routing failure.
        #[source]
        source: crate::encoding_provenance::EncodingProvenanceError,
    },
}

/// Read `path` and append its encoding-provenance record.
///
/// This is the one explicit file-read/provenance adapter over the pure codecs in
/// this module. The reference's phase order is preserved exactly:
///
/// 1. read the bytes (an unreadable file escapes, no record);
/// 2. prove the encoding (BOM first, then strict UTF-8) — an unproven encoding
///    refuses with [`ProvenanceReadError::Unprovable`] and appends nothing;
/// 3. mint and append the record through
///    [`EncodingProvenanceJournal::record_read`], whose append failure is caught
///    into [`ProvenancedText::warnings`];
/// 4. hand the text back.
///
/// `normalization_applied` is the reference's rule for this proven subset: a
/// BOM decode (or any non-`utf-8` source encoding) was re-encoded, plain UTF-8
/// was not.
pub fn read_text_with_provenance(
    path: &Path,
    journal: &EncodingProvenanceJournal,
) -> Result<ProvenancedText, ProvenanceReadError> {
    let bytes = crate::read_source_bytes(path).map_err(|source| ProvenanceReadError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let (text, source_encoding) = decode_unambiguous(&bytes).ok_or_else(|| {
        ProvenanceReadError::Unprovable {
            code: AMBIGUOUS_CODE,
            path: path.to_path_buf(),
            detail: format!(
                "the byte sequence cannot be proven as UTF-8 (with or without a BOM); {} bytes read",
                bytes.len()
            ),
        }
    })?;
    let normalization_applied = source_encoding != UTF8;
    let recorded = journal
        .record_read(
            Some(path),
            ReadEncodingFacts {
                source_encoding,
                confidence: 1.0,
                normalization_applied,
                bypass_used: false,
            },
        )
        .map_err(|source| ProvenanceReadError::Record {
            path: path.to_path_buf(),
            source,
        })?;
    let (record, warning) = recorded.into_parts();
    Ok(ProvenancedText {
        text: PythonString::from_scalars(&text),
        source_encoding,
        confidence: 1.0,
        normalization_applied,
        bypass_used: false,
        source_path: path.to_path_buf(),
        log_path: Some(record.log_path().to_path_buf()),
        warnings: warning.into_iter().collect(),
    })
}
