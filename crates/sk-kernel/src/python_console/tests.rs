//! Published reference facts for the console cell-width primitive.
//!
//! Every expectation here was read off the pinned Rich 15.0.0 / CPython 3.11.15
//! oracle (`rich.cells.get_character_cell_size` / `cell_len`); the exhaustive
//! comparison lives in `tests/python_console_widths_oracle.rs`.

use super::*;

#[test]
fn control_and_c1_code_points_are_zero_cells() {
    assert_eq!(codepoint_cell_size(0), 0, "NUL");
    assert_eq!(codepoint_cell_size(1), 0);
    assert_eq!(codepoint_cell_size(31), 0);
    assert_eq!(
        codepoint_cell_size(32),
        1,
        "space starts the printable range"
    );
    assert_eq!(codepoint_cell_size(0x7e), 1);
    assert_eq!(codepoint_cell_size(0x7f), 0);
    assert_eq!(codepoint_cell_size(0x9b), 0);
    assert_eq!(codepoint_cell_size(0x9f), 0);
    assert_eq!(codepoint_cell_size(0xa0), 1);
}

#[test]
fn printable_code_points_use_the_reference_table() {
    assert_eq!(codepoint_cell_size(u32::from('A')), 1);
    assert_eq!(codepoint_cell_size(0x00ad), 1, "soft hyphen is one cell");
    assert_eq!(codepoint_cell_size(0x0301), 0, "combining acute");
    assert_eq!(codepoint_cell_size(0x4e2d), 2, "CJK ideograph");
    assert_eq!(codepoint_cell_size(0x1f600), 2, "astral emoji");
    assert_eq!(codepoint_cell_size(0xe000), 1, "private use");
    assert_eq!(codepoint_cell_size(0xfdd0), 1, "noncharacter");
    assert_eq!(codepoint_cell_size(0xd800), 1, "lone surrogate unit");
    assert_eq!(codepoint_cell_size(0x10ffff), 1, "last code point");
}

#[test]
fn cell_len_uses_the_ascii_fast_path_then_the_per_code_point_sum() {
    assert_eq!(cell_len("Hello"), 5);
    assert_eq!(cell_len(""), 0);
    assert_eq!(cell_len("\t"), 0, "a tab is a C0 control: zero cells");
    assert_eq!(cell_len("a\u{0301}"), 1, "combining mark adds no cell");
    assert_eq!(cell_len("中a"), 3);
    assert_eq!(cell_len("naïve"), 5);
}

#[test]
fn cell_len_handles_the_zwj_and_variation_selector_loop() {
    // A VS16 widens a preceding `narrow_to_wide` character by one cell.
    assert_eq!(cell_len("\u{2764}\u{fe0f}"), 2, "heavy heart + VS16");
    assert_eq!(cell_len("\u{2764}"), 1, "heavy heart alone");
    // A ZWJ also consumes the character after it.
    assert_eq!(cell_len("\u{1f469}\u{200d}\u{1f4bb}"), 2);
    assert_eq!(cell_len("\u{200d}"), 0);
    assert_eq!(cell_len("\u{fe0f}"), 0);
}

#[test]
fn exact_code_points_measure_like_the_reference() {
    // A lone surrogate cannot be a Rust `str`, but it is a Python code unit and
    // measures one cell.
    assert_eq!(codepoint_cell_len(&[0xd800]), 1);
    assert_eq!(codepoint_cell_len(&[0x61, 0xd800, 0x62]), 3);
    assert_eq!(codepoint_cell_len(&[0]), 0);
    assert_eq!(codepoint_cell_len(&[]), 0);
    assert_eq!(cell_len("abc"), codepoint_cell_len(&[0x61, 0x62, 0x63]));
}
