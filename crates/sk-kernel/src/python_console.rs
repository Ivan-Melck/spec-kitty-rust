//! Pinned Source console cell-width primitives.
//!
//! The reference's human console is `rich.console.Console`; a table's layout is
//! measured in terminal **cells** by `rich.cells.cell_len`, not by Rust's
//! character count and not by the *latest* Unicode width data. The authority is
//! the locked Rich source and its own shipped width table:
//!
//! * `rich.cells.get_character_cell_size(character)` returns `0` for
//!   `0 < codepoint < 32` and `0x7F..0xA0`, otherwise the width from
//!   `rich._unicode_data` (`"auto"`), defaulting to `1` outside the table (so a
//!   lone surrogate unit, a private-use code point and an unassigned plane-16
//!   code point are all one cell);
//! * `rich.cells.cell_len(text)` has an ASCII fast path (the six
//!   `_SINGLE_CELL_UNICODE_RANGES`), a per-code-point sum otherwise, and a
//!   special loop when the text contains `U+200D` (zero-width joiner) or
//!   `U+FE0F` (variation selector 16) that widens a preceding
//!   `narrow_to_wide` character and consumes the character after a ZWJ.
//!
//! Those two functions are reproduced here from the pinned Rich data, so a
//! caller never approximates with Rust's Unicode tables or with
//! `unicodedata.east_asian_width` alone (Rich's table is *not* the
//! interpreter's: Rich 15.0.0 ships Unicode 17.0.0 widths even when the
//! interpreter's `unicodedata` is 14.0.0 — verified by dumping the pinned
//! reference). The generated table carries its provenance; see
//! `python_console/generate_cell_widths.py`.
//!
//! `codepoint_cell_len` takes exact code points (the `PythonString` code units),
//! units), so a table cell whose text holds a lone surrogate measures exactly
//! like the reference instead of failing to encode.
//!
//! This primitive measures **cells only**. Layout (columns, wrapping,
//! justification, box drawing) belongs to the console adapter that consumes it —
//! except for the one plain-text layout the CLI itself needs, which lives in
//! [`layout`] and reuses this table.

mod cell_width_data;
mod layout;
#[cfg(test)]
mod tests;

pub use cell_width_data::{RICH_UNICODE_VERSION, RICH_VERSION};
pub use layout::layout_console_text;

/// `rich.cells.get_character_cell_size` for one Python string code unit.
///
/// `code` is any `0..=U+10FFFF` unit, including a lone surrogate. Values above
/// `U+10FFFF` cannot exist in a Python string and are refused by the
/// constructors that produce code points.
pub fn codepoint_cell_size(code: u32) -> usize {
    cell_width_data::get_character_cell_size(code)
}

/// `rich.cells.cell_len` for Rust text.
pub fn cell_len(text: &str) -> usize {
    if cell_width_data::is_single_cell_widths(text) {
        return text.chars().count();
    }
    codepoint_cell_len(&text.chars().map(u32::from).collect::<Vec<_>>())
}

/// `rich.cells.cell_len` for exact Python code points.
///
/// The same three branches as the reference: the ASCII fast path, the plain
/// per-code-point sum, and the ZWJ/VS16 loop.
pub fn codepoint_cell_len(codepoints: &[u32]) -> usize {
    if let Ok(text) = exact_scalar_text(codepoints)
        && cell_width_data::is_single_cell_widths(&text)
    {
        return codepoints.len();
    }
    let has_special = codepoints
        .iter()
        .any(|code| *code == 0x200d || *code == 0xfe0f);
    if !has_special {
        return codepoints
            .iter()
            .map(|code| codepoint_cell_size(*code))
            .sum();
    }
    // The reference's SPECIAL loop: a ZWJ also consumes the following
    // character; a VS16 widens (by one) a preceding `narrow_to_wide` character.
    let mut total = 0usize;
    let mut last_measured: Option<u32> = None;
    let mut index = 0usize;
    while index < codepoints.len() {
        let character = codepoints[index];
        if character == 0x200d {
            index += 1;
        } else if character == 0xfe0f {
            if let Some(last) = last_measured
                && cell_width_data::is_narrow_to_wide(last)
            {
                total += 1;
            }
            last_measured = None;
        } else {
            let width = codepoint_cell_size(character);
            if width != 0 {
                last_measured = Some(character);
                total += width;
            }
        }
        index += 1;
    }
    total
}

/// The exact text of `codepoints`, when every unit is a Unicode scalar.
fn exact_scalar_text(codepoints: &[u32]) -> Result<String, u32> {
    let mut text = String::with_capacity(codepoints.len());
    for &code in codepoints {
        match char::from_u32(code) {
            Some(character) => text.push(character),
            None => return Err(code),
        }
    }
    Ok(text)
}
