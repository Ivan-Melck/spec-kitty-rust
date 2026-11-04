#!/usr/bin/env python3
"""Generate the verified CPython 3.11.15 / Unicode 14.0.0 facts for ActionRouter.

Development only: nothing in the Rust product imports this script, and the
product never runs Python. The generated table in
``crates/sk-invocation/src/router_unicode_data.rs`` is derived from the pinned
development interpreter after ``scripts/rust/verify_oracle.py`` proved that
interpreter is the lock-exact one.

Why a separate table: the router's token normalization is
``re.split(r"[\\s\\W]+", text.lower())``. That needs (a) Python's ``\\w`` word
membership -- ``str.isalnum()`` plus ``_`` -- and (b) Python's ``str.lower()``,
which is *not* Rust's ``char::to_lowercase()`` (different Unicode version) and
*not* ``casefold``. A capital sigma lowercases contextually
(``Final_Sigma``), and ``U+0130`` lowercases to two code points.

The ``Final_Sigma`` context follows the standard Unicode rule, verified against
the pinned interpreter rather than assumed. Both scans skip *case-ignorable* code
points, stop at a *cased* one and stop at a *non-ignorable, non-cased* one:

* ``"ΑΣ5"``-style suffixes: ``"ΑΣ5".lower()`` keeps the final
  sigma (``5`` is not case-ignorable, so the suffix cannot be
  ``case-ignorable* cased``); ``"ΑΣ_Α".lower()`` is final for the
  same reason while ``"ΑΣ.Α".lower()`` is not (``.`` is
  case-ignorable, so the following ``Α`` vetoes it),
* before the sigma: ``"Α5Σ".lower()`` is not final (``5`` is not
  case-ignorable) while ``"Α:Σ".lower()`` is final. A leading combining
  acute followed by sigma has no preceding cased letter and is not final.

So two real ``str.lower()`` probes classify every code point without guessing a
Unicode data file:

* ``"X\u03a3".lower()[-1] == "\u03c2"`` -> ``X`` is decisively cased
  (Cased and not Case_Ignorable),
* otherwise ``"\u0391X\u03a3".lower()[-1] == "\u03c2"`` -> ``X`` is
  case-ignorable (including the 200 points that also have Unicode Cased).

These are effective scan classes, not disjoint Unicode Cased/Case_Ignorable
properties: U+0345 belongs to both properties and both scans skip it.

Two earlier readings of these scans were rejected by the live oracle: one that
skipped every non-cased code point in the after-scan, and one that treated the
end of the after-scan as "final" without running the before-clause. The composed
oracle cases pin both clauses.

Usage (from the repository root, with the pinned oracle):

    .venv-rust-oracle/bin/python scripts/rust/generate_router_unicode.py
    .venv-rust-oracle/bin/python scripts/rust/generate_router_unicode.py --check

``--check`` fails (exit 1) when the committed table differs from a fresh
generation. Writing refuses to replace a different existing file unless
``--force`` is given, so the frozen table cannot be silently overwritten. A
missing or drifted interpreter is a hard failure, never a skipped or empty
comparison.
"""

from __future__ import annotations

import argparse
import hashlib
import re
import subprocess
import sys
import tempfile
import unicodedata
from pathlib import Path

# The pinned reference interpreter (CPython 3.11.15); ``verify_oracle.py`` checks
# the runtime closure, these guard the exact bug-fix release and Unicode version
# the facts came from.
EXPECTED_VERSION = (3, 11, 15)
EXPECTED_IMPLEMENTATION = "cpython"
EXPECTED_UNIDATA = "14.0.0"
EXPECTED_REQUIREMENTS = 53
LAST_CODE_POINT = 0x110000
OUTPUT = Path("crates/sk-invocation/src/router_unicode_data.rs")

WORD_RE = re.compile(r"\w")
CAPITAL_SIGMA = "\u03a3"
SMALL_FINAL_SIGMA = "\u03c2"
CAPITAL_ALPHA = "\u0391"


def merge_ranges(codes) -> list[tuple[int, int]]:
    ranges: list[tuple[int, int]] = []
    for code in codes:
        if ranges and ranges[-1][1] + 1 == code:
            ranges[-1] = (ranges[-1][0], code)
        else:
            ranges.append((code, code))
    return ranges


def word_ranges() -> list[tuple[int, int]]:
    word: list[int] = []
    for code in range(LAST_CODE_POINT):
        character = chr(code)
        matches = WORD_RE.fullmatch(character) is not None
        # A second, independent reading of the same predicate: Python's ``\\w``
        # for ``str`` patterns is ``str.isalnum()`` plus the underscore.
        expected = code == 0x5F or character.isalnum()
        if matches != expected:
            raise SystemExit(
                f"refusing to generate: \\w and isalnum disagree at U+{code:04X}"
            )
        if matches:
            word.append(code)
    return merge_ranges(word)


def lower_facts() -> tuple[list[tuple[int, int]], list[tuple[int, list[int]]], int]:
    singles: list[tuple[int, int]] = []
    expansions: list[tuple[int, list[int]]] = []
    identity = 0
    for code in range(LAST_CODE_POINT):
        mapped = [ord(character) for character in chr(code).lower()]
        if mapped == [code]:
            identity += 1
        elif len(mapped) == 1:
            singles.append((code, mapped[0]))
        else:
            expansions.append((code, mapped))
    if identity + len(singles) + len(expansions) != LAST_CODE_POINT:
        raise SystemExit("refusing to generate: lower mappings do not cover Unicode")
    return singles, expansions, identity


def context_facts() -> tuple[list[tuple[int, int]], list[tuple[int, int]], int]:
    """Classify every code point for the interpreter's ``Final_Sigma`` scans."""
    cased: list[int] = []
    ignorable: list[int] = []
    neither: list[int] = []
    for code in range(LAST_CODE_POINT):
        character = chr(code)
        if (character + CAPITAL_SIGMA).lower()[-1] == SMALL_FINAL_SIGMA:
            cased.append(code)
        elif (CAPITAL_ALPHA + character + CAPITAL_SIGMA).lower()[-1] == SMALL_FINAL_SIGMA:
            ignorable.append(code)
        else:
            neither.append(code)
    total = len(cased) + len(ignorable) + len(neither)
    if total != LAST_CODE_POINT:
        raise SystemExit(f"refusing to generate: classified {total} of {LAST_CODE_POINT}")
    return merge_ranges(cased), merge_ranges(ignorable), len(neither)


def verify_ranges(name: str, ranges: list[tuple[int, int]]) -> None:
    previous = -1
    for start, end in ranges:
        if start > end:
            raise SystemExit(f"refusing to generate: {name} range {start:#x}..{end:#x} is inverted")
        if start <= previous:
            raise SystemExit(f"refusing to generate: {name} ranges are not sorted/disjoint")
        if end >= LAST_CODE_POINT:
            raise SystemExit(f"refusing to generate: {name} range {start:#x}..{end:#x} leaves Unicode")
        previous = end


def verify_mapping(name: str, mapping: list[tuple[int, int]]) -> None:
    previous = -1
    for source, target in mapping:
        if source <= previous:
            raise SystemExit(f"refusing to generate: {name} mapping is not sorted/unique")
        if target >= LAST_CODE_POINT:
            raise SystemExit(f"refusing to generate: {name} target {target:#x} leaves Unicode")
        if target == source:
            raise SystemExit(f"refusing to generate: {name} identity mapping {source:#x} is not omitted")
        previous = source


def range_digest(name: str, ranges: list[tuple[int, int]]) -> str:
    text = name + ":" + " ".join(f"{start:06x}-{end:06x}" for start, end in ranges)
    return hashlib.sha256(text.encode("ascii")).hexdigest()


def lower_digest(singles: list[tuple[int, int]], expansions: list[tuple[int, list[int]]]) -> str:
    parts = ["lower:singles"]
    parts.extend(f"{code:06x}>{mapped:06x}" for code, mapped in singles)
    parts.append("expansions")
    parts.extend(
        f"{code:06x}>{','.join(f'{item:06x}' for item in mapped)}"
        for code, mapped in expansions
    )
    return hashlib.sha256(" ".join(parts).encode("ascii")).hexdigest()


def verify(root: Path) -> None:
    if sys.implementation.name != EXPECTED_IMPLEMENTATION:
        raise SystemExit(f"refusing to generate: requires {EXPECTED_IMPLEMENTATION}")
    if unicodedata.unidata_version != EXPECTED_UNIDATA:
        raise SystemExit(f"refusing to generate: requires Unicode {EXPECTED_UNIDATA}")
    version = tuple(sys.version_info[:3])
    if version != EXPECTED_VERSION:
        raise SystemExit(
            f"refusing to generate: interpreter {version} is not the pinned {EXPECTED_VERSION}"
        )
    verifier = root / "scripts/rust/verify_oracle.py"
    if not verifier.is_file():
        raise SystemExit(f"refusing to generate: missing oracle verifier {verifier}")
    with tempfile.TemporaryDirectory(prefix="sk-router-oracle-") as cwd:
        result = subprocess.run(
            [sys.executable, str(verifier), "--root", str(root)],
            capture_output=True,
            text=True,
            check=False,
            cwd=cwd,
        )
    if result.returncode != 0:
        raise SystemExit(
            "refusing to generate: the development oracle is not lock-exact\n"
            f"{result.stdout}{result.stderr}"
        )
    payload = result.stdout.strip()
    if f'"checked_runtime_requirements": {EXPECTED_REQUIREMENTS}' not in payload:
        raise SystemExit(
            "refusing to generate: the oracle did not report the pinned "
            f"{EXPECTED_REQUIREMENTS} runtime requirements\n{payload}"
        )
    if '"python": "' + ".".join(str(part) for part in EXPECTED_VERSION) + '"' not in payload:
        raise SystemExit(
            "refusing to generate: the oracle did not report the pinned interpreter\n"
            f"{payload}"
        )


def render(
    word: list[tuple[int, int]],
    singles: list[tuple[int, int]],
    expansions: list[tuple[int, list[int]]],
    cased: list[tuple[int, int]],
    ignorable: list[tuple[int, int]],
    *,
    version: str,
    identity: int,
    neither: int,
) -> str:
    word_count = sum(end - start + 1 for start, end in word)
    cased_count = sum(end - start + 1 for start, end in cased)
    ignorable_count = sum(end - start + 1 for start, end in ignorable)
    if word_count + (LAST_CODE_POINT - word_count) != LAST_CODE_POINT:
        raise SystemExit("refusing to generate: word coverage is inconsistent")
    if cased_count + ignorable_count + neither != LAST_CODE_POINT:
        raise SystemExit("refusing to generate: cased/ignorable coverage is inconsistent")
    if identity + len(singles) + len(expansions) != LAST_CODE_POINT:
        raise SystemExit("refusing to generate: lower coverage is inconsistent")

    def rows(ranges: list[tuple[int, int]]) -> str:
        return "\n".join(f"    (0x{start:04x}, 0x{end:04x})," for start, end in ranges)

    expansion_rows = [
        "(0x{code:04x}, &[{mapped}])".format(
            code=code, mapped=", ".join(f"0x{item:04x}" for item in mapped)
        )
        for code, mapped in expansions
    ]
    if not expansion_rows:
        expansion_table = "&[];"
    elif len(expansion_rows) == 1:
        expansion_table = "&[" + expansion_rows[0] + "];"
    else:
        expansion_table = "&[\n" + "\n".join(
            "    " + row + "," for row in expansion_rows
        ) + "\n];"
    return f'''//! Verified CPython 3.11.15 / Unicode {version} facts for `ActionRouter`.
//!
//! Generated by `scripts/rust/generate_router_unicode.py` from the pinned
//! development oracle (CPython 3.11.15, 53 locked runtime requirements) after
//! `scripts/rust/verify_oracle.py` proved the lock-exact interpreter. Do not
//! edit by hand; regenerate with:
//!
//! ```text
//! .venv-rust-oracle/bin/python scripts/rust/generate_router_unicode.py
//! ```
//!
//! The router normalizes with `re.split(r"[\\s\\W]+", text.lower())`, so it needs
//! the interpreter's own `\\w` membership and `str.lower()` mapping -- not Rust's
//! (newer Unicode) `char` methods and not `casefold`. Word membership is
//! `str.isalnum() || '_'`; `str.lower()` is per-code-point except for the
//! contextual `Final_Sigma` rule. Its disjoint scan classes below are decisive
//! cased (`Cased && !Case_Ignorable`) and all case-ignorable points (including
//! the 200 that also have Unicode Cased), NOT disjoint Unicode properties.
//!
//! * {word_count} word code points in {len(word)} ranges,
//! * {len(singles)} single-code-point lower mappings,
//! * {len(expansions)} multi-code-point lower expansions,
//! * {identity} code points whose lower mapping is identity,
//! * {cased_count} decisive-cased code points in {len(cased)} ranges,
//! * {ignorable_count} case-ignorable code points in {len(ignorable)} ranges.
//!
//! This is tokenization data only: it makes no casefold, number, encoding or
//! doctrine claim, and the product never invokes Python at runtime.

use std::cmp::Ordering;

/// Number of inclusive `\\w` word-code-point ranges.
pub(crate) const WORD_RANGE_COUNT: usize = {len(word)};
/// Sorted, disjoint, inclusive ranges of `str.isalnum() || '_'` code points.
pub(crate) const WORD_RANGES: &[(u32, u32)] = &[
{rows(word)}
];

/// Number of single-code-point `str.lower()` mappings.
pub(crate) const LOWER_SINGLE_COUNT: usize = {len(singles)};
/// Sorted `code point -> lower code point` mappings (identity mappings omitted).
pub(crate) const LOWER_SINGLES: &[(u32, u32)] = &[
{rows([(code, mapped) for code, mapped in singles])}
];
/// Number of multi-code-point `str.lower()` expansions.
pub(crate) const LOWER_EXPANSION_COUNT: usize = {len(expansions)};
/// Sorted `code point -> lower code point sequence` expansions (`U+0130`).
pub(crate) const LOWER_EXPANSIONS: &[(u32, &[u32])] = {expansion_table}

/// Number of cased ranges used by the `Final_Sigma` scans.
pub(crate) const CASED_RANGE_COUNT: usize = {len(cased)};
/// Decisively cased points (`Cased && !Case_Ignorable`), not full Unicode Cased.
pub(crate) const CASED_RANGES: &[(u32, u32)] = &[
{rows(cased)}
];

/// Number of case-ignorable ranges (including also-Cased points).
pub(crate) const CASE_IGNORABLE_RANGE_COUNT: usize = {len(ignorable)};
/// All case-ignorable points, including the overlap with Unicode Cased.
pub(crate) const CASE_IGNORABLE_RANGES: &[(u32, u32)] = &[
{rows(ignorable)}
];

/// Membership test for one of the generated sorted, disjoint range tables.
fn contains(ranges: &[(u32, u32)], code: u32) -> bool {{
    if code >= 0x110000 {{
        return false;
    }}
    ranges
        .binary_search_by(|&(start, end)| {{
            if end < code {{
                Ordering::Less
            }} else if start > code {{
                Ordering::Greater
            }} else {{
                Ordering::Equal
            }}
        }})
        .is_ok()
}}

/// CPython `\\w` for one code point (`str.isalnum() || '_'`).
pub(crate) fn is_word(code: u32) -> bool {{
    debug_assert_eq!(WORD_RANGES.len(), WORD_RANGE_COUNT);
    contains(WORD_RANGES, code)
}}

/// Decisive Cased (excluding Case_Ignorable) for a context scan.
pub(crate) fn is_cased(code: u32) -> bool {{
    debug_assert_eq!(CASED_RANGES.len(), CASED_RANGE_COUNT);
    contains(CASED_RANGES, code)
}}

/// The complete case-ignorable class skipped by both context scans.
pub(crate) fn is_case_ignorable(code: u32) -> bool {{
    debug_assert_eq!(CASE_IGNORABLE_RANGES.len(), CASE_IGNORABLE_RANGE_COUNT);
    contains(CASE_IGNORABLE_RANGES, code)
}}

/// The single-code-point `str.lower()` mapping, when it is not identity.
pub(crate) fn lower_single(code: u32) -> Option<u32> {{
    debug_assert_eq!(LOWER_SINGLES.len(), LOWER_SINGLE_COUNT);
    LOWER_SINGLES
        .binary_search_by(|&(from, _)| from.cmp(&code))
        .ok()
        .map(|index| LOWER_SINGLES[index].1)
}}

/// The multi-code-point `str.lower()` expansion, when there is one.
pub(crate) fn lower_expansion(code: u32) -> Option<&'static [u32]> {{
    debug_assert_eq!(LOWER_EXPANSIONS.len(), LOWER_EXPANSION_COUNT);
    LOWER_EXPANSIONS
        .binary_search_by(|&(from, _)| from.cmp(&code))
        .ok()
        .map(|index| LOWER_EXPANSIONS[index].1)
}}

#[cfg(test)]
mod tests {{
    use super::*;
    use sk_kernel::content_digest::sha256_hex;

    /// Recorded digest of [`WORD_RANGES`] (`word:` + `<start>-<end>` hex pairs).
    const WORD_DIGEST: &str = "{range_digest("word", word)}";
    /// Recorded digest of [`LOWER_SINGLES`] and [`LOWER_EXPANSIONS`].
    const LOWER_DIGEST: &str = "{lower_digest(singles, expansions)}";
    /// Recorded digest of [`CASED_RANGES`].
    const CASED_DIGEST: &str = "{range_digest("cased", cased)}";
    /// Recorded digest of [`CASE_IGNORABLE_RANGES`].
    const IGN_DIGEST: &str = "{range_digest("ignorable", ignorable)}";
    /// Code points whose `str.lower()` output is exactly the input.
    const IDENTITY_COUNT: usize = {identity};
    /// Code points that are neither cased nor case-ignorable.
    const NEITHER_COUNT: usize = {neither};

    fn range_text(name: &str, ranges: &[(u32, u32)]) -> String {{
        let rows: Vec<String> = ranges
            .iter()
            .map(|(start, end)| format!("{{start:06x}}-{{end:06x}}"))
            .collect();
        format!("{{name}}:{{}}", rows.join(" "))
    }}

    #[test]
    fn tables_are_sorted_disjoint_and_counted() {{
        for (name, ranges, count) in [
            ("word", WORD_RANGES, WORD_RANGE_COUNT),
            ("cased", CASED_RANGES, CASED_RANGE_COUNT),
            (
                "ignorable",
                CASE_IGNORABLE_RANGES,
                CASE_IGNORABLE_RANGE_COUNT,
            ),
        ] {{
            assert_eq!(ranges.len(), count, "{{name}} range count");
            let mut previous_end = None;
            let mut total = 0u32;
            for &(start, end) in ranges {{
                assert!(start <= end, "{{name}} range {{start:#06x}}..{{end:#06x}}");
                assert!(end < 0x110000, "{{name}} stays inside Unicode");
                if let Some(previous) = previous_end {{
                    assert!(previous < start, "{{name}} ranges are sorted and disjoint");
                }}
                total += end - start + 1;
                previous_end = Some(end);
            }}
            assert!(total > 0, "{{name}} is never vacuous");
        }}
        assert_eq!(LOWER_SINGLES.len(), LOWER_SINGLE_COUNT);
        assert_eq!(LOWER_EXPANSIONS.len(), LOWER_EXPANSION_COUNT);
        assert_eq!(
            IDENTITY_COUNT + LOWER_SINGLE_COUNT + LOWER_EXPANSION_COUNT,
            0x110000,
            "every code point has exactly one lower mapping"
        );
        let classes = word_count(CASED_RANGES) + word_count(CASE_IGNORABLE_RANGES);
        assert_eq!(NEITHER_COUNT + classes, 0x110000, "coverage is complete");
        for window in LOWER_SINGLES.windows(2) {{
            assert!(window[0].0 < window[1].0, "lower singles are sorted");
        }}
    }}

    fn word_count(ranges: &[(u32, u32)]) -> usize {{
        ranges
            .iter()
            .map(|&(start, end)| usize::try_from(end - start + 1).expect("range fits"))
            .sum()
    }}

    #[test]
    fn recorded_digests_match_the_tables() {{
        for (name, ranges, digest) in [
            ("word", WORD_RANGES, WORD_DIGEST),
            ("cased", CASED_RANGES, CASED_DIGEST),
            ("ignorable", CASE_IGNORABLE_RANGES, IGN_DIGEST),
        ] {{
            let table_digest = sha256_hex(range_text(name, ranges).as_bytes());
            assert_eq!(table_digest, digest, "{{name}}");
        }}
        let mut parts = vec![String::from("lower:singles")];
        for (from, to) in LOWER_SINGLES {{
            parts.push(format!("{{from:06x}}>{{to:06x}}"));
        }}
        parts.push(String::from("expansions"));
        parts.extend(LOWER_EXPANSIONS.iter().map(|(from, mapped)| {{
            let items: Vec<String> = mapped.iter().map(|item| format!("{{item:06x}}")).collect();
            format!("{{from:06x}}>{{}}", items.join(","))
        }}));
        assert_eq!(sha256_hex(parts.join(" ").as_bytes()), LOWER_DIGEST);
    }}

    #[test]
    fn pinned_router_facts_match_cpython() {{
        assert!(is_word(u32::from(b'a')) && is_word(u32::from(b'9')));
        assert!(is_word(0x5f), "underscore is a word code point");
        assert!(is_word(0xe9) && is_word(0x3c9) && is_word(0x31));
        assert!(!is_word(0x20) && !is_word(0x2d) && !is_word(0x301));
        assert!(!is_word(0x1f600), "an emoji is not a word code point");
        assert!(!is_word(0xd800) && !is_word(0x110000));
        assert_eq!(lower_single(0x41), Some(0x61));
        let sigma = lower_single(0x3a3);
        assert_eq!(sigma, Some(0x3c3), "non-final sigma is context-free");
        assert_eq!(lower_single(0xc9), Some(0xe9));
        assert_eq!(lower_single(0x61), None);
        assert_eq!(lower_expansion(0x130), Some(&[0x69, 0x307][..]));
        assert!(is_cased(0x41) && is_cased(0x61) && is_cased(0x391) && is_cased(0x3a3));
        assert!(!is_cased(0x35) && !is_cased(0x301) && !is_cased(0xd800));
        assert!(is_case_ignorable(0x301) && is_case_ignorable(0x27) && is_case_ignorable(0x3a));
        assert!(
            is_case_ignorable(0x345) && !is_cased(0x345),
            "overlap is skipped"
        );
        assert!(!is_case_ignorable(0x35) && !is_case_ignorable(0x41));
        assert!(!is_word(0x378), "unassigned stays non-word");
    }}
}}
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="repository root (default: two levels above this script)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the committed table instead of writing it",
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="replace a committed table that differs from a fresh generation",
    )
    args = parser.parse_args()
    root: Path = args.root.resolve()
    verify(root)
    word = word_ranges()
    singles, expansions, identity = lower_facts()
    cased, ignorable, neither = context_facts()
    for name, ranges in (("word", word), ("cased", cased), ("ignorable", ignorable)):
        verify_ranges(name, ranges)
    verify_mapping("lower", singles)
    for code, mapped in expansions:
        if any(item >= LAST_CODE_POINT for item in mapped) or mapped == [code]:
            raise SystemExit(f"refusing to generate: expansion {code:#x} looks wrong")
    if any(len(mapped) < 2 for _, mapped in expansions):
        raise SystemExit("refusing to generate: an expansion is not multi-code-point")
    text = render(
        word,
        singles,
        expansions,
        cased,
        ignorable,
        version=unicodedata.unidata_version,
        identity=identity,
        neither=neither,
    )
    target = root / OUTPUT
    existing = target.read_text(encoding="utf-8") if target.is_file() else None
    if args.check:
        if existing != text:
            raise SystemExit(f"refusing: {OUTPUT} differs from a fresh verified generation")
        print(
            f"{OUTPUT}: up to date ({len(word)} word ranges, "
            f"{len(singles)} lower singles, {len(expansions)} expansions)"
        )
        return 0
    if existing is not None and existing != text and not args.force:
        raise SystemExit(
            f"refusing to overwrite {OUTPUT}: it differs from a fresh verified "
            "generation (pass --force to replace it)"
        )
    target.write_text(text, encoding="utf-8")
    print(
        f"{OUTPUT}: wrote {len(word)} word ranges, {len(singles)} lower singles, "
        f"{len(expansions)} expansions"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
