//! Canonical read-only dossier-snapshot ownership classification.
//! Source `status.preflight.is_dossier_snapshot`; no dirty-gate-specific list.

/// Whether a path matches Source's dossier-snapshot globs (Linux fnmatch).
///
/// Accepts exact Python code points (`u32`) or ASCII-compatible filesystem
/// units (`u8`). The wildcard deliberately spans separators and may be empty;
/// a filename merely below `dossiers` is not otherwise exempt. No UTF-8/lossy
/// bridge is involved. Separator normalization is the shared kernel primitive.
pub fn is_dossier_snapshot<T: Copy + Eq + From<u8>>(path: &[T]) -> bool {
    let normalized = sk_kernel::paths::to_posix_units(path);
    let path = if starts_with(&normalized, b"./") {
        &normalized[2..]
    } else {
        &normalized
    };
    let prefix = b".kittify/dossiers/";
    let suffix = b"/snapshot-latest.json";
    if !ends_with(path, suffix) {
        return false;
    }
    (0..path.len()).any(|start| {
        (start == 0 || path[start - 1] == T::from(b'/'))
            && path.len() - start >= prefix.len() + suffix.len()
            && starts_with(&path[start..], prefix)
    })
}

fn starts_with<T: Copy + Eq + From<u8>>(units: &[T], literal: &[u8]) -> bool {
    units.len() >= literal.len() && units.iter().zip(literal).all(|(u, b)| *u == T::from(*b))
}
fn ends_with<T: Copy + Eq + From<u8>>(units: &[T], literal: &[u8]) -> bool {
    units.len() >= literal.len() && starts_with(&units[units.len() - literal.len()..], literal)
}
