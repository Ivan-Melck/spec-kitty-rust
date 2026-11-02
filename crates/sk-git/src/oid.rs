//! A validated, full Git object id.
//!
//! Git accepts abbreviations and revspec suffixes almost everywhere. This crate
//! does not: an [`Oid`] is a complete 40-hex SHA-1 or 64-hex SHA-256 id, so an
//! object id can never carry `^{commit}`, `~1`, a pathspec, or a leading `-`
//! into a command that reads it as syntax.

use std::fmt;

use crate::error::GitError;

/// A complete Git object id (40-hex SHA-1 or 64-hex SHA-256).
///
/// The value is normalised to lowercase. Construction validates hex digits and
/// length only; it makes no claim that the object exists in any repository.
///
/// # Example
///
/// ```
/// # fn main() -> Result<(), sk_git::GitError> {
/// let oid = sk_git::Oid::parse("0123456789abcdef0123456789abcdef01234567")?;
/// assert_eq!(oid.as_str(), "0123456789abcdef0123456789abcdef01234567");
/// assert!(!oid.is_zero());
/// assert!(sk_git::Oid::parse("HEAD~1").is_err());
/// # Ok(())
/// # }
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Oid(String);

impl Oid {
    /// Parse a full object id.
    ///
    /// Uppercase hex is accepted and stored lowercase. Abbreviated ids,
    /// revspec suffixes, whitespace, and anything that is not 40 or 64 hex
    /// digits are refused.
    pub fn parse(value: &str) -> Result<Self, GitError> {
        if value.len() != 40 && value.len() != 64 {
            return Err(GitError::InvalidOid {
                value: value.to_string(),
                detail: "expected exactly 40 or 64 hexadecimal digits",
            });
        }
        if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(GitError::InvalidOid {
                value: value.to_string(),
                detail: "contains a non-hexadecimal character",
            });
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    /// The all-zero id of the same width as `self`.
    ///
    /// `git update-ref <ref> <new> <zero>` is the compare-and-swap form of
    /// "this ref must not exist yet".
    pub fn zero_like(&self) -> Self {
        Self("0".repeat(self.0.len()))
    }

    /// The SHA-1 all-zero id, used when no width is available.
    pub fn zero_sha1() -> Self {
        Self("0".repeat(40))
    }

    /// Whether every digit is zero.
    pub fn is_zero(&self) -> bool {
        self.0.bytes().all(|byte| byte == b'0')
    }

    /// The full lowercase id.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 12 characters, for diagnostics only.
    pub fn short(&self) -> &str {
        &self.0[..12.min(self.0.len())]
    }

    /// The id's hexadecimal width (40 or 64).
    pub fn hex_len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Oid({})", self.0)
    }
}

impl std::str::FromStr for Oid {
    type Err = GitError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_sha1_and_sha256_and_normalises_case() {
        let sha1 = Oid::parse("0123456789ABCDEF0123456789abcdef01234567").unwrap();
        assert_eq!(sha1.as_str(), "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(sha1.hex_len(), 40);

        let sha256 = Oid::parse(&"a".repeat(64)).unwrap();
        assert_eq!(sha256.hex_len(), 64);
        assert_eq!(sha256.zero_like().as_str(), "0".repeat(64));
    }

    #[test]
    fn refuses_abbreviated_revspec_and_option_shaped_values() {
        for value in [
            "abc1234",
            "HEAD~1",
            "-0123456789abcdef0123456789abcdef0123456",
            "0123456789abcdef0123456789abcdef0123456^{commit}",
            "0123456789abcdef0123456789abcdef0123456 ",
            "0123456789abcdef0123456789abcdef012345g",
        ] {
            assert!(
                matches!(Oid::parse(value), Err(GitError::InvalidOid { .. })),
                "{value:?} should be refused"
            );
        }
    }

    #[test]
    fn zero_detection_uses_every_digit() {
        assert!(Oid::zero_sha1().is_zero());
        assert!(
            !Oid::parse("0000000000000000000000000000000000000001")
                .unwrap()
                .is_zero()
        );
    }
}
