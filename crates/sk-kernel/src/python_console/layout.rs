//! The reference's **Rich console text layout** for plain human text.
//!
//! Rich15 MIT notice/provenance: `docs/rust/dependencies/rich-console-layout-reference.md`.
//!
//! `console.print(text, markup=False)` does not write `text`: the pinned Rich
//! 15.0.0 renders a `Text` first and the *layout* phase decides the lines that
//! reach stdout, before any encoding happens. This module reproduces that phase
//! for the plain-text case the CLI uses, exactly:
//!
//! * the console width (`Console.width`) bounds every line;
//! * a tab expands to the next multiple of the console `tab_size` (8) measured
//!   in **cells**, not characters;
//! * each authored line is word-wrapped (`Text.wrap` → `_wrap.divide_line`),
//!   over-long words are folded (`rich.cells.chop_cells`, grapheme-aware), the
//!   trailing whitespace of an over-wide piece is trimmed (`Text.rstrip_end`),
//!   and each piece is cropped to the width (`Text.set_cell_size`);
//! * words keep their trailing whitespace, so a wrap point legitimately leaves
//!   a trailing space on the previous line, exactly like the reference;
//! * a trailing newline in the input produces an extra blank line
//!   (`Text.split(allow_blank=True)`), and the final line is followed by the
//!   caller's one `end="\n"`;
//! * the five code points `Text.__init__` strips (`rich.control.
//!   STRIP_CONTROL_CODES` = BEL, BS, VT, FF, CR) never reach the layout, while
//!   NUL, the other C0 units and DEL are kept at width zero.
//!
//! Everything here is measured in the reference's own cell terms through
//! [`super::codepoint_cell_len`] / [`super::codepoint_cell_size`] and the
//! pinned width table — there is no second width authority, no second
//! Unicode table and no Rust `char`-count approximation. The input is exact
//! Python code units (the [`PythonString`]), so a lone surrogate, a private-use
//! code point, a noncharacter, a combining mark, a wide CJK code point or a
//! ZWJ sequence measures and survives exactly as the reference measures it.
//!
//! Deliberately **not** here: the console's stream codec (owned by the CLI's
//! `python_output`), terminal control sanitising and emoji-code substitution
//! (the CLI console's own `render_str` policy), colour, and any renderable
//! other than plain `Text`. Layout only.

use crate::PythonString;
use crate::python_text::is_whitespace;

use super::cell_width_data::{SINGLE_CELL_RANGES, is_narrow_to_wide};
use super::{codepoint_cell_len, codepoint_cell_size};

/// The reference console's default `tab_size` (`Console(tab_size=8)`).
const TAB_SIZE: usize = 8;
const TAB: u32 = 0x09;
const NEWLINE: u32 = 0x0A;
const SPACE: u32 = 0x20;
const ZERO_WIDTH_JOINER: u32 = 0x200D;
const VARIATION_SELECTOR_16: u32 = 0xFE0F;

/// `rich.control.STRIP_CONTROL_CODES`: BEL, BS, VT, FF, CR — removed by
/// `Text.__init__` before any measuring or wrapping happens.
const STRIP_CONTROL_CODES: [u32; 5] = [0x07, 0x08, 0x0B, 0x0C, 0x0D];

/// `console.print(text, markup=False)` for a console of `width` **cells**.
///
/// Returns the text the reference's layout phase produces, *without* the one
/// trailing newline `console.print` appends (the caller writes that, and with
/// it the reference's stream codec). `width` must be at least 1: Rich renders
/// nothing at all when `ConsoleOptions.max_width < 1`, which is a caller
/// decision, not a layout result.
pub fn layout_console_text(text: &PythonString, width: usize) -> PythonString {
    debug_assert!(width >= 1, "the reference renders nothing below one cell");
    let stripped: Vec<u32> = text
        .codepoints()
        .iter()
        .copied()
        .filter(|code| !STRIP_CONTROL_CODES.contains(code))
        .collect();
    let lines = wrap_lines(&stripped, width);
    let mut joined: Vec<u32> = Vec::with_capacity(text.codepoints().len() + 8);
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            joined.push(NEWLINE);
        }
        joined.extend_from_slice(line);
    }
    // `Console.print(crop=True)` ends with
    // `Segment.split_and_crop_lines(..., self.width, pad=False)`: every line is
    // cropped to the console width and never padded.
    let cropped = crop_lines(&joined, width);
    PythonString::from_codepoints(&cropped).expect("console layout preserves code units")
}

/// `Text.wrap(console, width, overflow="fold")`: the layout's line list.
fn wrap_lines(text: &[u32], width: usize) -> Vec<Vec<u32>> {
    let mut lines: Vec<Vec<u32>> = Vec::new();
    // `Text.split(allow_blank=True)`: the trailing empty line of a text that
    // ends with a newline is kept (Python's `str.split("\n")` behaviour).
    for authored in text.split(|code| *code == NEWLINE) {
        let line = if authored.contains(&TAB) {
            expand_tabs(authored)
        } else {
            authored.to_vec()
        };
        let offsets = divide_line(&line, width);
        let mut pieces = divide(&line, &offsets);
        // `for line in new_lines: line.rstrip_end(width)`
        for piece in pieces.iter_mut() {
            rstrip_end(piece, width);
        }
        // `lines.justify(..., justify="default")` matches none of the
        // reference's left/center/right/full branches, so it changes nothing.
        // `for line in new_lines: line.truncate(width, overflow="fold")`
        for piece in pieces.iter_mut() {
            crop_to_cells(piece, width);
        }
        lines.append(&mut pieces);
    }
    lines
}

/// `Text.expand_tabs(tab_size)`: tabs become spaces up to the next tab stop,
/// counted in cells (so `日本\tx` pads four spaces at size 8).
fn expand_tabs(line: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(line.len() + TAB_SIZE);
    let mut cell_position = 0usize;
    let mut part_start = 0usize;
    for (index, code) in line.iter().enumerate() {
        if *code != TAB {
            continue;
        }
        // The part is `line[part_start..=index]` with its tab replaced by one
        // space; `Text.expand_tabs` counts that space, then pads to the stop.
        let mut part = line[part_start..index].to_vec();
        part.push(SPACE);
        cell_position += codepoint_cell_len(&part);
        let remainder = cell_position % TAB_SIZE;
        if remainder != 0 {
            let spaces = TAB_SIZE - remainder;
            part.extend(std::iter::repeat_n(SPACE, spaces));
            cell_position += spaces;
        }
        out.extend_from_slice(&part);
        part_start = index + 1;
    }
    // The reference counts the final part's cells too, but nothing reads that
    // count afterwards, so it is not carried here.
    out.extend_from_slice(&line[part_start..]);
    out
}

/// `rich._wrap.divide_line(text, width, fold=True)`: the cell offsets to break
/// an authored line at. Ported statement for statement, including the
/// `word.rstrip()` fit test and the `fold` branch's break positions.
fn divide_line(text: &[u32], width: usize) -> Vec<usize> {
    let mut breaks: Vec<usize> = Vec::new();
    let mut cell_offset: usize = 0;
    for (word_start, word_end) in words(text) {
        let mut start = word_start;
        let word = &text[word_start..word_end];
        let word_length = codepoint_cell_len(trim_end_whitespace(word));
        if cell_offset <= width && width - cell_offset >= word_length {
            cell_offset += codepoint_cell_len(word);
            continue;
        }
        if word_length > width {
            // Longer than any line: fold it across lines (`fold=True`).
            let folded = chop_cells(word, width);
            let count = folded.len();
            for (index, piece) in folded.iter().enumerate() {
                if start != 0 {
                    breaks.push(start);
                }
                if index + 1 == count {
                    cell_offset = codepoint_cell_len(piece);
                } else {
                    start += piece.len();
                }
            }
        } else if cell_offset != 0 && start != 0 {
            // It fits on the next (empty) line.
            breaks.push(start);
            cell_offset = codepoint_cell_len(word);
        }
    }
    breaks
}

/// `rich._wrap.words`: `re_word = re.compile(r"\s*\S+\s*")` matches yielded as
/// `(start, end)` code-point spans, scanning from the previous match end.
fn words(text: &[u32]) -> Vec<(usize, usize)> {
    let mut matches: Vec<(usize, usize)> = Vec::new();
    let mut position = 0usize;
    while position < text.len() {
        let mut index = position;
        while index < text.len() && is_whitespace(text[index]) {
            index += 1;
        }
        let word_start = index;
        while index < text.len() && !is_whitespace(text[index]) {
            index += 1;
        }
        if index == word_start {
            // `\S+` cannot match even after the greedy `\s*` backtracks, so the
            // regex has no match here and therefore none later either.
            break;
        }
        while index < text.len() && is_whitespace(text[index]) {
            index += 1;
        }
        matches.push((position, index));
        position = index;
    }
    matches
}

/// Python's `str.rstrip()` for a slice already ending at a non-whitespace unit
/// is the slice itself; the helper keeps the intent explicit at the call site.
fn trim_end_whitespace(text: &[u32]) -> &[u32] {
    let mut end = text.len();
    while end > 0 && is_whitespace(text[end - 1]) {
        end -= 1;
    }
    &text[..end]
}

/// `rich.cells.chop_cells`: split `text` so every piece fits `width` cells,
/// breaking between graphemes and never inside one.
fn chop_cells(text: &[u32], width: usize) -> Vec<Vec<u32>> {
    if width == 0 {
        // The reference raises here (`range() arg 3 must not be zero`); the
        // console never renders below one cell, so this is unreachable.
        return Vec::new();
    }
    if is_single_cell(text) {
        return text.chunks(width).map(<[u32]>::to_vec).collect();
    }
    let (spans, _) = split_graphemes(text);
    let mut lines: Vec<Vec<u32>> = Vec::new();
    let mut line_size = 0usize;
    let mut line_offset = 0usize;
    for span in &spans {
        if line_size + span.cells > width {
            lines.push(text[line_offset..span.start].to_vec());
            line_offset = span.start;
            line_size = 0;
        }
        line_size += span.cells;
    }
    if line_size > 0 {
        lines.push(text[line_offset..].to_vec());
    }
    lines
}

/// `Text.divide(offsets)`: the slices between consecutive break offsets.
fn divide(text: &[u32], offsets: &[usize]) -> Vec<Vec<u32>> {
    let mut bounds: Vec<usize> = Vec::with_capacity(offsets.len() + 2);
    bounds.push(0);
    bounds.extend_from_slice(offsets);
    bounds.push(text.len());
    bounds
        .windows(2)
        .map(|pair| text[pair[0]..pair[1]].to_vec())
        .collect()
}

/// `Text.rstrip_end(size)`: trim trailing whitespace when the line is longer
/// than `size` — the reference compares the *code-point* length with the cell
/// width here, and removes at most the excess.
fn rstrip_end(line: &mut Vec<u32>, size: usize) {
    let text_length = line.len();
    if text_length <= size {
        return;
    }
    let excess = text_length - size;
    let mut whitespace = 0usize;
    while whitespace < line.len() && is_whitespace(line[line.len() - 1 - whitespace]) {
        whitespace += 1;
    }
    if whitespace == 0 {
        return;
    }
    let remove = whitespace.min(excess);
    line.truncate(text_length - remove);
}

/// `Text.truncate(width, overflow="fold")` for one line.
fn crop_to_cells(line: &mut Vec<u32>, width: usize) {
    if codepoint_cell_len(line) > width {
        *line = set_cell_size(line, width);
    }
}

/// `rich.cells.set_cell_size`: crop or pad to exactly `total` cells.
fn set_cell_size(text: &[u32], total: usize) -> Vec<u32> {
    if is_single_cell(text) {
        if text.len() < total {
            let mut padded = text.to_vec();
            padded.extend(std::iter::repeat_n(SPACE, total - text.len()));
            return padded;
        }
        return text[..total.min(text.len())].to_vec();
    }
    if total == 0 {
        return Vec::new();
    }
    let cell_size = codepoint_cell_len(text);
    if cell_size == total {
        return text.to_vec();
    }
    if cell_size < total {
        let mut padded = text.to_vec();
        padded.extend(std::iter::repeat_n(SPACE, total - cell_size));
        return padded;
    }
    split_text(text, total).0
}

/// `rich.cells._split_text`: split at a cell position. A position inside a
/// double-width grapheme becomes a space on each side, exactly like the
/// reference.
fn split_text(text: &[u32], cell_position: usize) -> (Vec<u32>, Vec<u32>) {
    if cell_position == 0 {
        return (Vec::new(), text.to_vec());
    }
    let (spans, cell_length) = split_graphemes(text);
    if cell_length == 0 || spans.is_empty() {
        return (text.to_vec(), Vec::new());
    }
    // The reference guesses `int((cell_position / cell_length) * len(spans))`
    // in Python float arithmetic, then corrects the guess in the loop.
    let mut offset = ((cell_position as f64 / cell_length as f64) * spans.len() as f64) as usize;
    offset = offset.min(spans.len());
    let mut left_size: usize = spans[..offset].iter().map(|span| span.cells).sum();
    loop {
        if left_size == cell_position {
            if offset >= spans.len() {
                return (text.to_vec(), Vec::new());
            }
            let split_index = spans[offset].start;
            return (text[..split_index].to_vec(), text[split_index..].to_vec());
        }
        if left_size < cell_position {
            if offset >= spans.len() {
                // Unreachable for a valid guess; refusing to index past the end
                // is still better than a panic.
                return (text.to_vec(), Vec::new());
            }
            let span = &spans[offset];
            if left_size + span.cells > cell_position {
                let mut left = text[..span.start].to_vec();
                left.push(SPACE);
                let mut right = vec![SPACE];
                right.extend_from_slice(&text[span.end..]);
                return (left, right);
            }
            offset += 1;
            left_size += span.cells;
        } else {
            let span = &spans[offset - 1];
            if left_size - span.cells < cell_position {
                let mut left = text[..span.start].to_vec();
                left.push(SPACE);
                let mut right = vec![SPACE];
                right.extend_from_slice(&text[span.end..]);
                return (left, right);
            }
            offset -= 1;
            left_size -= span.cells;
        }
    }
}

/// One grapheme: `(start, end, cells)` over code-point indices.
#[derive(Clone, Copy, Debug)]
struct GraphemeSpan {
    start: usize,
    end: usize,
    cells: usize,
}

/// `rich.cells.split_graphemes`: spans covering every index, a ZWJ consuming
/// the following code point and `U+FE0F` widening a preceding
/// `narrow_to_wide` character.
fn split_graphemes(text: &[u32]) -> (Vec<GraphemeSpan>, usize) {
    let mut spans: Vec<GraphemeSpan> = Vec::new();
    let mut total = 0usize;
    let mut last_measured: Option<u32> = None;
    let mut index = 0usize;
    while index < text.len() {
        let character = text[index];
        if character == ZERO_WIDTH_JOINER || character == VARIATION_SELECTOR_16 {
            if spans.is_empty() {
                spans.push(GraphemeSpan {
                    start: index,
                    end: index + 1,
                    cells: 0,
                });
                index += 1;
                continue;
            }
            if character == ZERO_WIDTH_JOINER {
                index += if index + 1 < text.len() { 2 } else { 1 };
                if let Some(last) = spans.last_mut() {
                    last.end = index;
                }
            } else {
                index += 1;
                if let Some(previous) = last_measured
                    && is_narrow_to_wide(previous)
                {
                    last_measured = None;
                    total += 1;
                    if let Some(last) = spans.last_mut() {
                        last.cells += 1;
                        last.end = index;
                    }
                    continue;
                }
                if let Some(last) = spans.last_mut() {
                    last.end = index;
                }
            }
            continue;
        }
        let width = codepoint_cell_size(character);
        if width != 0 {
            last_measured = Some(character);
            spans.push(GraphemeSpan {
                start: index,
                end: index + 1,
                cells: width,
            });
            index += 1;
            total += width;
        } else if spans.is_empty() {
            spans.push(GraphemeSpan {
                start: index,
                end: index + 1,
                cells: 0,
            });
            index += 1;
        } else {
            if let Some(last) = spans.last_mut() {
                last.end = index + 1;
            }
            index += 1;
        }
    }
    (spans, total)
}

/// `rich.cells._is_single_cell_widths`: every code unit is inside one of the
/// six single-cell ranges, so the reference's ASCII fast paths apply.
fn is_single_cell(text: &[u32]) -> bool {
    text.iter().all(|code| {
        SINGLE_CELL_RANGES
            .iter()
            .any(|(start, end)| (*start..=*end).contains(code))
    })
}

/// `Segment.split_and_crop_lines(..., width, pad=False)` for one text run.
fn crop_lines(text: &[u32], width: usize) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(text.len());
    let mut start = 0usize;
    for (index, code) in text.iter().enumerate() {
        if *code != NEWLINE {
            continue;
        }
        out.extend_from_slice(&adjust_line_length(&text[start..index], width));
        out.push(NEWLINE);
        start = index + 1;
    }
    if start < text.len() {
        out.extend_from_slice(&adjust_line_length(&text[start..], width));
    }
    out
}

/// `Segment.adjust_line_length(line, width, pad=False)`.
fn adjust_line_length(line: &[u32], width: usize) -> Vec<u32> {
    if codepoint_cell_len(line) > width {
        set_cell_size(line, width)
    } else {
        line.to_vec()
    }
}

#[cfg(test)]
mod tests;
