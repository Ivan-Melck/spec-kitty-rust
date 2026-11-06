//! The charter bundle content-identity hash (`charter/bundle.py`,
//! `charter/hasher.py`).
//!
//! This is the ONE write-side hashing recipe for the compiled charter bundle and
//! the synthesis manifest's `bundle_content_hash` stamp: every writer and the
//! freshness reader route through it, so a stamp can never drift from the bytes
//! it describes.
//!
//! Recipe (pinned Source `hash_content`): drop a leading BOM, normalise line
//! endings (CRLF and lone CR → LF), strip outer whitespace, then SHA-256 the
//! UTF-8 bytes and render `sha256:<hex>`.
//!
//! The bundle digest hashes each file in [`BUNDLE_CONTENT_HASH_FILES`]
//! independently (declared order), appends each present member of
//! [`BUNDLE_OPTIONAL_CONTENT_HASH_FILES`], and combines them through the same
//! [`hash_content`] over their newline-joined digests. Per-file hashing is
//! required: a BOM on a non-first file would otherwise survive.

use std::path::Path;

use sk_kernel::content_digest::sha256_hex;

/// Required content-hash members, relative to `.kittify/charter/`.
pub const BUNDLE_CONTENT_HASH_FILES: [&str; 1] = ["charter.yaml"];

/// Optional content-hash members (the activation ledger), hashed only when present.
pub const BUNDLE_OPTIONAL_CONTENT_HASH_FILES: [&str; 1] = ["activation.yaml"];

/// `hash_content`: the BOM/newline/whitespace-normalised `sha256:` digest of one text.
pub fn hash_content(content: &str) -> String {
    let normalised: String = content
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    format!("sha256:{}", sha256_hex(normalised.trim().as_bytes()))
}

fn file_digest(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some(hash_content(&text))
}

/// `compute_bundle_content_hash`: the content-identity digest of the synced bundle.
///
/// `None` — fail-safe, never panicking — when `.kittify/charter/` is missing, or
/// when any required/optional-present member is missing or unreadable.
pub fn compute_bundle_content_hash(project_root: &Path) -> Option<String> {
    let charter_dir = project_root.join(".kittify").join("charter");
    if !charter_dir.is_dir() {
        return None;
    }
    let mut required: Vec<String> = Vec::new();
    for name in BUNDLE_CONTENT_HASH_FILES {
        required.push(file_digest(&charter_dir.join(name))?);
    }
    let mut optional: Vec<String> = Vec::new();
    for name in BUNDLE_OPTIONAL_CONTENT_HASH_FILES {
        let path = charter_dir.join(name);
        if !path.exists() {
            continue;
        }
        optional.push(file_digest(&path)?);
    }
    Some(combine_bundle_digests(&required, &optional))
}

/// The bundle stamp over SUPPLIED member texts, through the same recipe.
///
/// A caller that must stamp prospective bytes it has not written yet (the
/// activation-ledger migration's preview) passes the required member first and
/// the optional activation member when it will exist. This is the canonical
/// seam for that case: no fake filesystem, no second hashing recipe.
pub fn bundle_content_hash_of(required: &[&str], optional: &[&str]) -> String {
    let required: Vec<String> = required.iter().map(|text| hash_content(text)).collect();
    let optional: Vec<String> = optional.iter().map(|text| hash_content(text)).collect();
    combine_bundle_digests(&required, &optional)
}

fn combine_bundle_digests(required: &[String], optional: &[String]) -> String {
    let mut digests: Vec<String> = required.to_vec();
    digests.extend(optional.iter().cloned());
    hash_content(&digests.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_normalises_bom_newlines_and_outer_whitespace() {
        assert_eq!(hash_content("abc"), hash_content("  abc\n"));
        assert_eq!(hash_content("a\r\nb"), hash_content("a\nb"));
        assert_eq!(hash_content("\u{feff}abc"), hash_content("abc"));
        assert!(hash_content("abc").starts_with("sha256:"));
    }

    #[test]
    fn supplied_member_texts_hash_through_the_same_recipe() {
        assert_eq!(
            bundle_content_hash_of(&["catalog: {}\n"], &["activated_kinds: []\n"]),
            crate::hash_content(&format!(
                "{}\n{}",
                crate::hash_content("catalog: {}\n"),
                crate::hash_content("activated_kinds: []\n")
            ))
        );
    }

    #[test]
    fn an_absent_bundle_has_no_digest() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(compute_bundle_content_hash(directory.path()), None);
    }

    #[test]
    fn a_present_ledger_changes_the_digest_and_its_absence_does_not() {
        let directory = tempfile::tempdir().unwrap();
        let charter = directory.path().join(".kittify/charter");
        std::fs::create_dir_all(&charter).unwrap();
        std::fs::write(charter.join("charter.yaml"), "catalog: {}\n").unwrap();
        let without = compute_bundle_content_hash(directory.path()).unwrap();
        std::fs::write(charter.join("activation.yaml"), "activated_kinds: []\n").unwrap();
        let with = compute_bundle_content_hash(directory.path()).unwrap();
        assert_ne!(without, with);
        std::fs::remove_file(charter.join("activation.yaml")).unwrap();
        assert_eq!(
            compute_bundle_content_hash(directory.path()).unwrap(),
            without
        );
    }
}
