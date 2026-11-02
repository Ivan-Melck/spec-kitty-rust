//! Canonical content digests.
//!
//! The `sha256:<hex>` provenance format used by the skills manifest and pack
//! source hashes lives here, together with the bare hex forms used where a
//! filename fragment is wanted. Both digest exactly the bytes they are given;
//! no line-ending, whitespace, or path normalisation happens. The charter
//! freshness hash is a different algorithm and is deliberately not this
//! module.
//!
//! SHA-1 is provided **only** for the Mission checkout-claim lock filename
//! discriminator. Upstream `kernel.locks._anchor_digest` uses
//! `hashlib.sha1(..., usedforsecurity=False)` there because that name has been
//! stable since #5819 and renaming it would move every live lock. It is not a
//! provenance or content-integrity digest and must not be used as one.

use sha2::{Digest, Sha256};

/// The provenance prefix of [`sha256_digest`].
pub const SHA256_PREFIX: &str = "sha256:";

/// Longest hex prefix an anchored lock filename may request.
pub const MAX_DIGEST_HEX_LENGTH: usize = 64;

/// Return `sha256:<hex>` over `data` (exact bytes, no normalisation).
pub fn sha256_digest(data: &[u8]) -> String {
    format!("{SHA256_PREFIX}{}", sha256_hex(data))
}

/// Return the bare lower-case hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Digest a path identity to `hex_length` lower-case hex characters.
///
/// Kept crate-private: lock filenames are a consumer concern, not a general
/// digest contract. A length outside `1..=64` is refused rather than silently
/// clamped, so a caller can always reproduce the name it asked for.
pub(crate) fn sha256_prefix(data: &[u8], hex_length: usize) -> Result<String, crate::KernelError> {
    validate_hex_length(hex_length)?;
    let mut digest = sha256_hex(data);
    digest.truncate(hex_length);
    Ok(digest)
}

/// Return the bare lower-case hex SHA-1 of `data`.
///
/// This exists solely to reproduce upstream's checkout-claim lock filename
/// (see the module documentation). It is a **name discriminator**, never a
/// security primitive.
pub fn sha1_hex(data: &[u8]) -> String {
    sha1_digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Digest a lock-identity to `hex_length` lower-case SHA-1 hex characters.
///
/// As with [`sha256_prefix`], the length is validated rather than silently
/// clamped. SHA-1 produces 40 hex characters, so requests above 40 yield the
/// full digest (upstream's `hexdigest()[:64]` behaves the same way).
pub(crate) fn sha1_prefix(data: &[u8], hex_length: usize) -> Result<String, crate::KernelError> {
    validate_hex_length(hex_length)?;
    let mut digest = sha1_hex(data);
    digest.truncate(hex_length);
    Ok(digest)
}

fn validate_hex_length(hex_length: usize) -> Result<(), crate::KernelError> {
    if !(1..=MAX_DIGEST_HEX_LENGTH).contains(&hex_length) {
        return Err(crate::KernelError::InvalidDigestLength { length: hex_length });
    }
    Ok(())
}

/// A small, dependency-free SHA-1 implementation (FIPS 180-1).
///
/// The workspace cannot add the `sha1` crate offline, and the algorithm is
/// needed for one stable lock filename. Kept private so no consumer mistakes
/// it for an integrity digest.
fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let mut chunk = [0_u8; 64];
    for block in message.as_chunks::<64>().0 {
        chunk.copy_from_slice(block);
        let mut words = [0_u32; 80];
        for (index, word) in words.iter_mut().take(16).enumerate() {
            let start = index * 4;
            *word = u32::from_be_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (index, word) in words.iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
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

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
    }

    let mut digest = [0_u8; 20];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_the_fips_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha1_prefix_truncates_and_never_exceeds_the_digest() {
        assert_eq!(sha1_prefix(b"abc", 16).unwrap().len(), 16);
        assert_eq!(sha1_prefix(b"abc", 40).unwrap().len(), 40);
        assert_eq!(sha1_prefix(b"abc", 64).unwrap().len(), 40);
        assert!(sha1_prefix(b"abc", 0).is_err());
        assert!(sha1_prefix(b"abc", 65).is_err());
    }
}
