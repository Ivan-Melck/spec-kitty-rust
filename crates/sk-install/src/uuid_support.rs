//! Minimal RFC 4122 UUID support: v4 minting and v5 name-based derivation.
//!
//! Identity must be byte-compatible with upstream (`uuid.uuid4()` and
//! `uuid.uuid5`), and the workspace cannot depend on the `uuid` crate offline,
//! so this module implements the two versions the identity path needs:
//!
//! - **v4** from the operating-system CSPRNG (`getrandom`);
//! - **v5** from SHA-1 over `namespace bytes || name bytes`, with the RFC 4122
//!   version/variant bits set.
//!
//! Parsing is strict: 32 hex digits in the canonical 8-4-4-4-12 grouping. A
//! recorded identity that does not parse is a typed fault (never re-minted
//! over), because re-minting a UUID silently orphans every ledger/journal row
//! keyed on the old value.

use std::fmt;

use crate::error::InstallError;

/// The RFC 4122 URL namespace (`6ba7b811-9dad-11d1-80b4-00c04fd430c8`).
pub const NAMESPACE_URL: Uuid = Uuid([
    0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

/// A 128-bit UUID.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Uuid([u8; 16]);

impl Uuid {
    /// Parse the canonical hyphenated form (strict).
    pub fn parse(text: &str) -> Result<Self, InstallError> {
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return Err(identity_error(text, "expected 36 characters"));
        }
        let mut raw = [0_u8; 16];
        let mut nibble = 0_usize;
        for (index, character) in bytes.iter().enumerate() {
            match index {
                8 | 13 | 18 | 23 => {
                    if *character != b'-' {
                        return Err(identity_error(text, "missing hyphen"));
                    }
                }
                _ => {
                    let digit = match character {
                        b'0'..=b'9' => character - b'0',
                        b'a'..=b'f' => character - b'a' + 10,
                        b'A'..=b'F' => character - b'A' + 10,
                        _ => return Err(identity_error(text, "non-hex character")),
                    };
                    raw[nibble / 2] = (raw[nibble / 2] << 4) | digit;
                    nibble += 1;
                }
            }
        }
        if nibble != 32 {
            return Err(identity_error(text, "wrong digit count"));
        }
        Ok(Self(raw))
    }

    /// Mint a random v4 UUID.
    pub fn new_v4() -> Result<Self, InstallError> {
        let mut raw = [0_u8; 16];
        getrandom::fill(&mut raw).map_err(|error| InstallError::IdentityInvalid {
            path: std::path::PathBuf::from("<random>"),
            detail: format!("cannot read the system random source: {error}"),
        })?;
        raw[6] = (raw[6] & 0x0f) | 0x40;
        raw[8] = (raw[8] & 0x3f) | 0x80;
        Ok(Self(raw))
    }

    /// Derive a v5 (SHA-1, name-based) UUID in `namespace`.
    pub fn new_v5(namespace: Uuid, name: &str) -> Self {
        let mut input = Vec::with_capacity(16 + name.len());
        input.extend_from_slice(&namespace.0);
        input.extend_from_slice(name.as_bytes());
        let digest = sha1(&input);
        let mut raw = [0_u8; 16];
        raw.copy_from_slice(&digest[..16]);
        raw[6] = (raw[6] & 0x0f) | 0x50;
        raw[8] = (raw[8] & 0x3f) | 0x80;
        Self(raw)
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let raw = &self.0;
        write!(
            formatter,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            raw[0],
            raw[1],
            raw[2],
            raw[3],
            raw[4],
            raw[5],
            raw[6],
            raw[7],
            raw[8],
            raw[9],
            raw[10],
            raw[11],
            raw[12],
            raw[13],
            raw[14],
            raw[15]
        )
    }
}

fn identity_error(value: &str, detail: &str) -> InstallError {
    InstallError::IdentityInvalid {
        path: std::path::PathBuf::from(value),
        detail: detail.to_string(),
    }
}

/// SHA-1 (FIPS 180-1). Required only for UUIDv5 compatibility with upstream.
pub fn sha1(input: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut words = [0_u32; 80];
        for (index, word) in chunk.chunks(4).filter(|word| word.len() == 4).enumerate() {
            words[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (index, word) in words.iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999_u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temporary = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temporary;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut digest = [0_u8; 20];
    for (index, word) in h.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_the_standard_vectors() {
        let hex = |bytes: [u8; 20]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }

    #[test]
    fn uuid_v5_matches_python_uuid5_for_the_identity_build_id() {
        let namespace = Uuid::new_v5(NAMESPACE_URL, "spec-kitty:identity:build_id");
        assert_eq!(
            namespace.to_string(),
            "18c0fef1-8e60-5d6b-89d9-e2f1ccb9cc03"
        );
        let project = Uuid::parse("48fe8ecb-fbf7-4893-90f5-714b9987ac60").expect("uuid");
        let build = Uuid::new_v5(namespace, &format!("{project}:28d3810d268d"));
        assert_eq!(build.to_string(), "c4685d25-fbd9-5632-a6cb-1822523869dc");
    }

    #[test]
    fn v4_sets_version_and_variant_bits() {
        let uuid = Uuid::new_v4().expect("uuid");
        let text = uuid.to_string();
        assert_eq!(text.len(), 36);
        assert_eq!(&text[14..15], "4");
        assert!(matches!(&text[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(uuid, Uuid::new_v4().expect("uuid"));
    }

    #[test]
    fn parsing_is_strict() {
        assert!(Uuid::parse("48fe8ecb-fbf7-4893-90f5-714b9987ac60").is_ok());
        for bad in [
            "48fe8ecbfbf7489390f5714b9987ac60",
            "48fe8ecb-fbf7-4893-90f5-714b9987ac6",
            "zzfe8ecb-fbf7-4893-90f5-714b9987ac60",
            "48fe8ecb_fbf7_4893_90f5_714b9987ac60",
        ] {
            assert!(Uuid::parse(bad).is_err(), "{bad:?}");
        }
    }
}
