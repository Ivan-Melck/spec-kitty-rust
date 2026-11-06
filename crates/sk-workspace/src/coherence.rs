//! Canonical self-bookkeeping leg of coordination coherence.
//! This is NOT the topology-dependent coordination-residue union. Callers
//! must apply that separate authority only under their established topology.

pub mod done;

pub use done::{CoordDoneReadError, coord_incoherent_done_wps};

/// Source `coordination.coherence.is_self_bookkeeping_churn`, read-only.
///
/// Use exact Python code points (`u32`) or native ASCII-compatible filesystem
/// units (`u8`). Rules inspect ASCII delimiters/literals and nonempty segments,
/// never manufacture a path identity or convert invalid bytes lossily. The
/// predicate intentionally retains Source's regex `$` final-LF behavior,
/// dossier fnmatch wildcards, and encoding-provenance suffix behavior.
pub fn is_self_bookkeeping_churn<T: Copy + Eq + From<u8>>(path: &[T]) -> bool {
    let mut normalized = sk_kernel::paths::to_posix_units(path);
    while normalized.last() == Some(&T::from(b'/')) {
        normalized.pop();
    }
    if ends_with(&normalized, b".kittify/encoding-provenance/global.jsonl")
        || sk_status::preflight::is_dossier_snapshot(&normalized)
        || regex_legs(&normalized)
    {
        return true;
    }
    // Python re `$` may match before ONE final LF. This alternative applies
    // only to regex legs, not endswith or fnmatch's strict end anchor.
    normalized.last() == Some(&T::from(b'\n')) && regex_legs(&normalized[..normalized.len() - 1])
}

fn regex_legs<T: Copy + Eq + From<u8>>(path: &[T]) -> bool {
    let parts: Vec<&[T]> = path.split(|unit| *unit == T::from(b'/')).collect();
    if parts
        .windows(2)
        .any(|pair| eq(pair[0], b".kittify") && eq(pair[1], b"mission-state-audit"))
    {
        return true;
    }
    let n = parts.len();
    if n >= 2 && eq(parts[n - 2], b".kittify") && eq(parts[n - 1], b"meta.json") {
        return true;
    }
    if n >= 3
        && eq(parts[n - 3], b"kitty-specs")
        && !parts[n - 2].is_empty()
        && eq(parts[n - 1], b"meta.json")
    {
        return true;
    }
    if n >= 4
        && !parts[n - 2].is_empty()
        && !parts[n - 1].is_empty()
        && eq(parts[n - 4], b".kittify")
        && eq(parts[n - 3], b"derived")
    {
        return true;
    }
    if n >= 4
        && eq(parts[n - 4], b"kitty-specs")
        && !parts[n - 3].is_empty()
        && eq(parts[n - 2], b"decisions")
        && eq(parts[n - 1], b"index.json.lock")
    {
        return true;
    }
    if n >= 2 && eq(parts[n - 2], b"kitty-ops") && ends_with(parts[n - 1], b".jsonl") {
        let stem = &parts[n - 1][..parts[n - 1].len() - 6];
        let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        return stem.len() == 26
            && stem
                .iter()
                .all(|unit| alphabet.iter().any(|b| *unit == T::from(*b)));
    }
    false
}
fn eq<T: Copy + Eq + From<u8>>(units: &[T], literal: &[u8]) -> bool {
    units.len() == literal.len() && units.iter().zip(literal).all(|(u, b)| *u == T::from(*b))
}
fn ends_with<T: Copy + Eq + From<u8>>(units: &[T], literal: &[u8]) -> bool {
    units.len() >= literal.len() && eq(&units[units.len() - literal.len()..], literal)
}
