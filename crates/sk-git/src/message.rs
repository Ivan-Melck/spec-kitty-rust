//! Opaque exact-byte commit messages.
//!
//! A commit message is a byte payload. The reference implementation carries a
//! Python `str` and lets the `os.fsencode`/argv boundary — and then Git — decide
//! what the bytes mean, so a `\udc80` code point reaches Git as the single byte
//! `0x80` and Git, not Spec Kitty, applies `i18n.commitEncoding`. This type is
//! the native equivalent of "exact bytes, no re-interpretation":
//!
//! * construction only captures bytes and provenance and cannot fail, so the
//!   legacy planner keeps its exact error position;
//! * [`CommitMessage::validate`] is the **one** validation authority (non-empty,
//!   at most [`MAX_COMMIT_MESSAGE_LEN`](crate::MAX_COMMIT_MESSAGE_LEN) bytes, no
//!   NUL) for both the legacy `String` entry point and a raw byte payload;
//! * [`CommitMessage::as_utf8`] is an *inspection* helper. Nothing here decodes
//!   lossily, guesses a legacy encoding, or materialises a fake `&str`, and
//!   [`Debug`] is an escaped rendering for diagnostics only — never wire,
//!   audit, or reflog data.

use std::ffi::OsString;
use std::fmt;

use crate::error::GitError;
use crate::safe_commit::MAX_COMMIT_MESSAGE_LEN;

/// Where a [`CommitMessage`]'s bytes came from.
///
/// Provenance only: the planner, the message file, hooks, and the commit object
/// treat both origins identically. It exists so a caller or diagnostic can tell
/// a native string payload from OS/argv bytes without re-decoding the payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageOrigin {
    /// Captured from a Rust `&str` (necessarily valid UTF-8).
    Utf8,
    /// Captured from raw bytes or an OS string; not necessarily UTF-8.
    OsBytes,
}

/// One commit-message payload: exact bytes, never re-encoded.
///
/// The module documentation describes the boundary this type draws.
#[derive(Clone, PartialEq, Eq)]
pub struct CommitMessage {
    bytes: Vec<u8>,
    origin: MessageOrigin,
}

impl CommitMessage {
    /// Capture a Rust string exactly.
    pub fn from_text(message: &str) -> Self {
        Self {
            bytes: message.as_bytes().to_vec(),
            origin: MessageOrigin::Utf8,
        }
    }

    /// Capture raw bytes exactly, without validation or decoding.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
            origin: MessageOrigin::OsBytes,
        }
    }

    /// Capture an OS string's exact bytes.
    ///
    /// This is the native form of an argv/filesystem payload. On Unix every
    /// byte sequence is accepted; on other platforms only a Unicode OS string
    /// is accepted, because the wide-character representation is not a byte
    /// payload this crate claims to carry — the refusal is typed rather than an
    /// untested encoding guess.
    #[cfg(unix)]
    pub fn from_os_string(message: impl Into<OsString>) -> Result<Self, GitError> {
        use std::os::unix::ffi::OsStrExt;
        let message = message.into();
        Ok(Self {
            bytes: message.as_os_str().as_bytes().to_vec(),
            origin: MessageOrigin::OsBytes,
        })
    }

    /// Capture an OS string's exact bytes.
    ///
    /// Non-Unix platforms accept a Unicode OS string only; anything else is a
    /// typed refusal instead of an untested wide-argv claim.
    #[cfg(not(unix))]
    pub fn from_os_string(message: impl Into<OsString>) -> Result<Self, GitError> {
        let message = message.into();
        match message.into_string() {
            Ok(text) => Ok(Self::from_bytes(text.into_bytes())),
            Err(_) => Err(GitError::Unsupported {
                feature: "a commit message from a non-Unicode OS string on this platform",
            }),
        }
    }

    /// The payload as exact bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The payload as text, only when it is valid UTF-8.
    ///
    /// This is an inspection helper: it never repairs, replaces, or guesses.
    pub fn as_utf8(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// Where the bytes came from.
    pub fn origin(&self) -> MessageOrigin {
        self.origin
    }

    /// Apply the one message validation the planner uses.
    ///
    /// The rules and their exact error values are the legacy ones: empty, more
    /// than [`MAX_COMMIT_MESSAGE_LEN`](crate::MAX_COMMIT_MESSAGE_LEN) bytes, or
    /// containing NUL. The reference's argv boundary accepts an empty message
    /// (the commit itself classifies it); the crate-private argv validator is
    /// the execution-time rule set. This is the prepared-construction contract
    /// and is unchanged.
    pub fn validate(&self) -> Result<(), GitError> {
        validate_message_bytes(&self.bytes)
    }
}

impl fmt::Debug for CommitMessage {
    /// Escaped, length-annotated diagnostics. Never parse this back.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CommitMessage { len: ")?;
        write!(
            formatter,
            "{}, origin: {:?}, bytes: \"",
            self.bytes.len(),
            self.origin
        )?;
        for &byte in &self.bytes {
            match byte {
                b' '..=b'~' if byte != b'\\' && byte != b'"' => {
                    formatter.write_str(&char::from(byte).to_string())?;
                }
                b'\\' => formatter.write_str("\\\\")?,
                b'"' => formatter.write_str("\\\"")?,
                _ => write!(formatter, "\\x{byte:02x}")?,
            }
        }
        formatter.write_str("\" }")
    }
}

/// The single byte-level message validation authority.
pub(crate) fn validate_message_bytes(bytes: &[u8]) -> Result<(), GitError> {
    match message_defect(bytes) {
        None => Ok(()),
        Some(defect) => Err(defect.into_error()),
    }
}

/// Why a payload would be refused, as a typed reason rather than error text.
///
/// This is the one inspection every message entry point shares; the public
/// errors below are the exact legacy values, so a caller can tell *which* rule
/// refused the payload without parsing a `Display` string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageDefect {
    /// No bytes at all.
    Empty,
    /// More than [`MAX_COMMIT_MESSAGE_LEN`] bytes.
    TooLong,
    /// The bytes contain a NUL.
    Nul,
}

impl MessageDefect {
    fn into_error(self) -> GitError {
        match self {
            Self::Empty => GitError::SafeCommitInvalidRequest {
                detail: "a safe commit needs a non-empty message".to_string(),
            },
            Self::TooLong => GitError::SafeCommitInvalidRequest {
                detail: format!(
                    "the commit message exceeds the {MAX_COMMIT_MESSAGE_LEN}-byte limit"
                ),
            },
            Self::Nul => GitError::SafeCommitInvalidRequest {
                detail: "the commit message must not contain NUL".to_string(),
            },
        }
    }
}

/// The one byte-level rule set, in the legacy order (empty, length, NUL).
fn message_defect(bytes: &[u8]) -> Option<MessageDefect> {
    if bytes.is_empty() {
        return Some(MessageDefect::Empty);
    }
    if bytes.len() > MAX_COMMIT_MESSAGE_LEN {
        return Some(MessageDefect::TooLong);
    }
    if bytes.contains(&0) {
        return Some(MessageDefect::Nul);
    }
    None
}

/// Validate a payload at the reference implementation's argv boundary.
///
/// The reference stages the requested paths first and then passes the message
/// as one `-m` argv element. An **empty** message is a valid argv value: the
/// reference runs its hooks and message artifact; hooks may supply a message,
/// otherwise the commit aborts with an ordinary empty-message failure. It is
/// accepted here and classified by the commit itself. A NUL is refused here
/// (argv cannot carry it), after the stage
/// and before any hook or artifact. The byte-length bound is the crate's own
/// Native budget, not the kernel's `E2BIG` and not a reference-parity claim.
pub(crate) fn validate_argv_message_bytes(bytes: &[u8]) -> Result<(), GitError> {
    match message_defect(bytes) {
        None | Some(MessageDefect::Empty) => Ok(()),
        Some(defect) => Err(defect.into_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_captures_exact_bytes_without_re_encoding() {
        let message = CommitMessage::from_bytes([0x6f, 0x80, 0xff]);
        assert_eq!(message.as_bytes(), &[0x6f, 0x80, 0xff]);
        assert!(message.as_utf8().is_none());
        assert_eq!(message.origin(), MessageOrigin::OsBytes);

        let text = CommitMessage::from_text("op(\u{00a0})");
        assert_eq!(text.as_bytes(), "op(\u{00a0})".as_bytes());
        assert_eq!(text.as_utf8(), Some("op(\u{00a0})"));
        assert_eq!(text.origin(), MessageOrigin::Utf8);
    }

    #[test]
    fn debug_is_escaped_and_never_lossy() {
        let message = CommitMessage::from_bytes(b"a\x80\xff\\\"");
        let rendered = format!("{message:?}");
        assert!(
            rendered.contains("a\\x80\\xff\\\\\\\""),
            "non-ASCII bytes and syntax characters are escaped: {rendered}"
        );
        assert!(
            !rendered.contains('\u{fffd}'),
            "Debug never substitutes U+FFFD: {rendered}"
        );
    }

    #[test]
    fn validation_is_the_legacy_rule_set() {
        assert!(CommitMessage::from_text("ok").validate().is_ok());
        assert!(CommitMessage::from_bytes(b"op(\x80)").validate().is_ok());
        assert!(matches!(
            CommitMessage::from_bytes(Vec::new()).validate(),
            Err(GitError::SafeCommitInvalidRequest { detail })
                if detail == "a safe commit needs a non-empty message"
        ));
        assert!(matches!(
            CommitMessage::from_bytes(b"a\0b").validate(),
            Err(GitError::SafeCommitInvalidRequest { detail })
                if detail == "the commit message must not contain NUL"
        ));
        assert!(matches!(
            CommitMessage::from_bytes(vec![b'x'; MAX_COMMIT_MESSAGE_LEN + 1]).validate(),
            Err(GitError::SafeCommitInvalidRequest { detail })
                if detail == format!("the commit message exceeds the {MAX_COMMIT_MESSAGE_LEN}-byte limit")
        ));
    }

    #[test]
    fn argv_validation_defers_empty_but_refuses_nul_and_overlong() {
        assert!(validate_argv_message_bytes(b"").is_ok());
        assert!(validate_argv_message_bytes(b"ok").is_ok());
        assert!(matches!(
            validate_argv_message_bytes(b"a\0b"),
            Err(GitError::SafeCommitInvalidRequest { detail })
                if detail == "the commit message must not contain NUL"
        ));
        assert!(matches!(
            validate_argv_message_bytes(&vec![b'x'; MAX_COMMIT_MESSAGE_LEN + 1]),
            Err(GitError::SafeCommitInvalidRequest { detail })
                if detail == format!("the commit message exceeds the {MAX_COMMIT_MESSAGE_LEN}-byte limit")
        ));
    }

    #[test]
    fn a_unicode_os_string_keeps_exact_bytes_and_os_provenance() {
        let message = CommitMessage::from_os_string(OsString::from("op(é😀): a")).unwrap();
        assert_eq!(message.as_bytes(), "op(é😀): a".as_bytes());
        assert_eq!(message.origin(), MessageOrigin::OsBytes);
    }

    #[cfg(unix)]
    #[test]
    fn an_os_string_carries_raw_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let message = CommitMessage::from_os_string(OsString::from_vec(b"op(\x80)".to_vec()))
            .expect("unix OS strings carry raw bytes");
        assert_eq!(message.as_bytes(), b"op(\x80)");
        assert_eq!(message.origin(), MessageOrigin::OsBytes);
    }
}
