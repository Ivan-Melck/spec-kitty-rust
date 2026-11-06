"""Generate `crates/sk-kernel/src/python_console/cell_width_data.rs`.

The console cell width is the reference's, not Rust's: `rich.cells.cell_len`
sums `get_character_cell_size` over the code points, using the `rich._unicode_data`
table selected by `unicode_version="auto"`. Rich 15.0.0 ships its own table
(Unicode 17.0.0) even though the pinned CPython 3.11.15 interpreter's
`unicodedata` is 14.0.0, so neither `unicodedata.east_asian_width` nor a Rust
width table is the authority. This dumps the reference's own table from the
pinned oracle and writes the compressed non-1 ranges.

Usage (from the repository root):

    .venv-rust-oracle/bin/python \
        crates/sk-kernel/src/python_console/generate_cell_widths.py
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[4]
ORACLE = REPO / ".venv-rust-oracle/bin/python"
OUT = REPO / "crates/sk-kernel/src/python_console/cell_width_data.rs"

SCRIPT = r"""
import json
import rich
from rich import cells
from rich import _unicode_data

table = cells.load_cell_table("auto")
print(json.dumps({
    "rich": getattr(rich, "VERSION", None) or "15.0.0",
    "table_unicode": table.unicode_version,
    "ranges": [list(row) for row in table.widths],
    "narrow_to_wide": sorted(ord(character) for character in table.narrow_to_wide),
}))
"""

# `rich.cells._SINGLE_CELL_UNICODE_RANGES`, verbatim from the locked source.
SINGLE_CELL_RANGES = [
    ("Latin (excluding non-printable)", 0x20, 0x7E),
    ("Latin-1 punctuation", 0xA0, 0xAC),
    ("Latin extended", 0xAE, 0x2FF),
    ("Greek / Cyrillic", 0x370, 0x482),
    ("Box drawing, box elements, geometric shapes", 0x2500, 0x25FC),
    ("Braille", 0x2800, 0x28FF),
]


def main() -> int:
    proc = subprocess.run(
        [str(ORACLE), "-c", SCRIPT],
        env={"PATH": "/usr/local/bin:/usr/bin:/bin"},
        capture_output=True,
        check=False,
    )
    assert proc.returncode == 0, proc.stderr.decode()
    facts = json.loads(proc.stdout)
    ranges = [(int(a), int(b), int(w)) for a, b, w in facts["ranges"]]
    narrow = [int(code) for code in facts["narrow_to_wide"]]

    # Source-first assertions on the harvested facts before writing anything.
    assert ranges == sorted(ranges), "the reference table is ordered"
    for start, end, width in ranges:
        assert 0 <= start <= end <= 0x10FFFF, (start, end)
        assert width in (0, 2), (start, end, width)
    zero = sum(1 for _, _, width in ranges if width == 0)
    wide = sum(1 for _, _, width in ranges if width == 2)
    print(
        json.dumps(
            {
                "rich": facts["rich"],
                "table_unicode": facts["table_unicode"],
                "ranges": len(ranges),
                "zero_width_ranges": zero,
                "double_width_ranges": wide,
                "narrow_to_wide": len(narrow),
            }
        )
    )

    lines: list[str] = []
    add = lines.append
    add("//! Generated console cell-width data — DO NOT EDIT BY HAND.")
    add("//!")
    add("//! Source: the locked reference's own table, dumped from the pinned oracle.")
    add(
        f"//! Rich {facts['rich']}, `rich._unicode_data` version "
        f"{facts['table_unicode']}"
    )
    add(
        "//! (the pinned CPython 3.11.15 interpreter's `unicodedata` is 14.0.0; Rich"
    )
    add(
        "//! ships its own table, so the interpreter's East-Asian width is NOT the"
    )
    add("//! authority).")
    add("//!")
    add(
        f"//! `NON_SINGLE_CELL_RANGES`: {len(ranges)} `(first, last, width)` ranges "
        f"whose width is not 1"
    )
    add(
        f"//! ({zero} zero-width, {wide} double-width). `NARROW_TO_WIDE`: the "
        f"{len(narrow)} code points"
    )
    add("//! the reference widens when they are followed by `U+FE0F`.")
    add("//!")
    add("//! Generator: `crates/sk-kernel/src/python_console/generate_cell_widths.py`.")
    add("")
    add("/// The Rich release the table was dumped from.")
    add(f'pub const RICH_VERSION: &str = "{facts["rich"]}";')
    add("/// The Unicode version of Rich's own width table.")
    add(f'pub const RICH_UNICODE_VERSION: &str = "{facts["table_unicode"]}";')
    add("")
    add("/// `(first, last, width)` for every range whose width is not 1.")
    add("pub(crate) const NON_SINGLE_CELL_RANGES: &[(u32, u32, u8)] = &[")
    for start, end, width in ranges:
        add(f"    (0x{start:04X}, 0x{end:04X}, {width}),")
    add("];")
    add("")
    add("/// `rich.cells._SINGLE_CELL_UNICODE_RANGES`, verbatim.")
    add("pub(crate) const SINGLE_CELL_RANGES: &[(u32, u32)] = &[")
    for label, start, end in SINGLE_CELL_RANGES:
        add(f"    // {label}")
        add(f"    (0x{start:04X}, 0x{end:04X}),")
    add("];")
    add("")
    add("/// Code points the reference widens when followed by `U+FE0F`/`U+200D`.")
    add("pub(crate) const NARROW_TO_WIDE: &[u32] = &[")
    row: list[str] = []
    for code in narrow:
        row.append(f"0x{code:04X},")
        if len(row) == 12:
            add("    " + " ".join(row))
            row = []
    if row:
        add("    " + " ".join(row))
    add("];")
    add("")
    add("/// `rich.cells.get_character_cell_size` for one code point.")
    add("pub(crate) fn get_character_cell_size(code: u32) -> usize {")
    add("    // `if codepoint and codepoint < 32 or 0x07F <= codepoint < 0x0A0: return 0`")
    add("    if (code != 0 && code < 32) || (0x7F..0xA0).contains(&code) {")
    add("        return 0;")
    add("    }")
    add("    match NON_SINGLE_CELL_RANGES.binary_search_by(|&(start, end, _)| {")
    add("        if code < start {")
    add("            std::cmp::Ordering::Greater")
    add("        } else if code > end {")
    add("            std::cmp::Ordering::Less")
    add("        } else {")
    add("            std::cmp::Ordering::Equal")
    add("        }")
    add("    }) {")
    add("        Ok(index) => usize::from(NON_SINGLE_CELL_RANGES[index].2),")
    add("        Err(_) => 1,")
    add("    }")
    add("}")
    add("")
    add("/// `rich.cells._is_single_cell_widths`: every character in the six ranges.")
    add("pub(crate) fn is_single_cell_widths(text: &str) -> bool {")
    add("    text.chars().all(|character| {")
    add("        let code = u32::from(character);")
    add("        SINGLE_CELL_RANGES")
    add("            .iter()")
    add("            .any(|(start, end)| (*start..=*end).contains(&code))")
    add("    })")
    add("}")
    add("")
    add("/// Whether a code point is in the reference's `narrow_to_wide` set.")
    add("pub(crate) fn is_narrow_to_wide(code: u32) -> bool {")
    add("    NARROW_TO_WIDE.binary_search(&code).is_ok()")
    add("}")
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({len(lines)} lines)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
