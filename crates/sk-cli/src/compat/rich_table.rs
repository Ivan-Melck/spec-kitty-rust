//! The reference's **Rich table layout**, for the human console views.
//!
//! `profiles list` renders a `rich.table.Table` (Rich 15.0.0) with
//! `Table(title="Agent Profiles")`, `box.HEAVY_HEAD`, the default
//! `padding=(0, 1, 0, 1)`, `show_edge=True`, and one `Column` per field:
//! the first is `no_wrap=True, overflow="fold"`, the rest use the
//! `add_column` defaults (`justify="left"`, `overflow="ellipsis"`).
//! `console.print` then writes the rendered lines.
//!
//! This module reproduces that layout: the column-width algorithm
//! (`_calculate_column_widths` + `_collapse_widths` + `ratio_reduce`), the
//! per-cell word wrap / truncation (`Text.wrap` with `divide_line(fold=…)` and
//! `Text.truncate`), left justification, the title centered over the table, and
//! the box drawing. It measures in **cells** through the one canonical width
//! primitive ([`sk_kernel::python_console::cell_len`]) — there is no second
//! width table and no second formatter here; only the layout lives in this
//! adapter.
//!
//! The console writes only plain text for these views (the reference's styles
//! are colour-only, and a non-terminal has no colour system), so no ANSI
//! sequences are produced. A row whose text cannot be encoded as UTF-8 is
//! refused by the caller *before* this renderer runs, exactly like the
//! reference's single `UnicodeEncodeError` write.

use sk_kernel::python_console::cell_len;

/// How a cell that does not fit is finished (`Column.overflow`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Overflow {
    /// `"ellipsis"` (the `add_column` default): truncate and append `…`.
    Ellipsis,
    /// `"fold"`: hard-split the text at the column edge.
    Fold,
}

/// One column's rendering contract.
#[derive(Clone, Debug)]
pub(crate) struct TableColumn {
    /// The header text.
    pub header: String,
    /// `no_wrap=True`: a single line, hard-cut instead of wrapped.
    pub no_wrap: bool,
    /// How an over-long cell is finished.
    pub overflow: Overflow,
}

impl TableColumn {
    /// The reference's profile-id column: `no_wrap=True, overflow="fold"`.
    pub fn unwrapped(header: &str) -> Self {
        Self {
            header: header.to_owned(),
            no_wrap: true,
            overflow: Overflow::Fold,
        }
    }

    /// An `add_column` default column: wrapping, ellipsis overflow.
    pub fn wrapped(header: &str) -> Self {
        Self {
            header: header.to_owned(),
            no_wrap: false,
            overflow: Overflow::Ellipsis,
        }
    }
}

/// A table rendered exactly like `rich.table.Table` with `box.HEAVY_HEAD`.
#[derive(Clone, Debug)]
pub(crate) struct RichTable {
    title: String,
    columns: Vec<TableColumn>,
    rows: Vec<Vec<String>>,
}

impl RichTable {
    /// A titled table with these columns.
    pub fn new(title: &str, columns: Vec<TableColumn>) -> Self {
        Self {
            title: title.to_owned(),
            columns,
            rows: Vec::new(),
        }
    }

    /// Append one row (one cell per column).
    pub fn push_row(&mut self, cells: Vec<String>) {
        debug_assert_eq!(cells.len(), self.columns.len());
        self.rows.push(cells);
    }

    /// The rendered lines (no trailing newline; `console.print` adds one per
    /// line).
    pub fn render(&self, max_width: usize) -> Vec<String> {
        if self.columns.is_empty() {
            return vec![String::new()];
        }
        // `_extra_width`: two edges plus one divider per column boundary.
        let extra_width = 2 + self.columns.len() - 1;
        let content_max = max_width.saturating_sub(extra_width).max(1);
        let widths = self.column_widths(content_max);
        let table_width = widths.iter().sum::<usize>() + extra_width;

        let mut lines = Vec::with_capacity(self.rows.len() + 4);
        if !self.title.is_empty() {
            lines.push(center(&self.title, table_width));
        }
        lines.push(border("┏", "━", "┳", "┓", &widths));
        lines.extend(self.render_row(&widths, &self.header_cells(), true));
        lines.push(border("┡", "━", "╇", "┩", &widths));
        for row in &self.rows {
            lines.extend(self.render_row(&widths, row, false));
        }
        lines.push(border("└", "─", "┴", "┘", &widths));
        lines
    }

    fn header_cells(&self) -> Vec<String> {
        self.columns
            .iter()
            .map(|column| column.header.clone())
            .collect()
    }

    /// `_calculate_column_widths` for `content_max` (the width available to the
    /// columns, padding included, borders excluded).
    fn column_widths(&self, content_max: usize) -> Vec<usize> {
        let mut widths: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| self.measure_column(index, column, content_max).max(1))
            .collect();
        if widths.iter().sum::<usize>() > content_max {
            widths = collapse_widths(&widths, &self.wrapable(), content_max);
            // `_collapse_widths` cannot always fit the no-wrap columns. Rich's
            // last resort also shrinks those columns before re-measuring.
            let table_width = widths.iter().sum::<usize>();
            if table_width > content_max {
                let maximums: Vec<i64> = widths.iter().map(|width| *width as i64).collect();
                widths = ratio_reduce(
                    (table_width - content_max) as i64,
                    &vec![1; widths.len()],
                    &maximums,
                    &widths,
                );
            }
            // The reference re-measures every column at its collapsed width.
            widths = self
                .columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    let width = widths[index];
                    self.measure_column(index, column, width)
                })
                .collect();
        }
        widths
    }

    fn wrapable(&self) -> Vec<bool> {
        self.columns.iter().map(|column| !column.no_wrap).collect()
    }

    /// `_measure_column`: `(min, max)` collapsed to the maximum, padding
    /// included, never above `options_max`.
    fn measure_column(&self, index: usize, column: &TableColumn, options_max: usize) -> usize {
        if options_max < 1 {
            return 0;
        }
        let padding = 2;
        let inner = options_max.saturating_sub(padding);
        let mut maximum = 0usize;
        let mut cells: Vec<&str> = Vec::with_capacity(self.rows.len() + 1);
        cells.push(column.header.as_str());
        for row in &self.rows {
            if let Some(cell) = row.get(index) {
                cells.push(cell.as_str());
            }
        }
        for cell in cells {
            let measured = cell_len(cell).min(inner) + padding;
            maximum = maximum.max(measured.min(options_max));
        }
        maximum
    }

    /// Render one logical row into its visual lines (all lines have the same
    /// height; `vertical="bottom"` for the header, `"top"` for data rows).
    fn render_row(&self, widths: &[usize], cells: &[String], header: bool) -> Vec<String> {
        let rendered: Vec<Vec<String>> = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let width = widths[index].saturating_sub(2);
                render_cell(
                    cells.get(index).map(String::as_str).unwrap_or_default(),
                    width,
                    column,
                )
            })
            .collect();
        let height = rendered.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let divider = if header { '┃' } else { '│' };
        let mut lines = Vec::with_capacity(height);
        for line in 0..height {
            let mut text = String::new();
            text.push(divider);
            for (index, cell_lines) in rendered.iter().enumerate() {
                let content = if header {
                    // `vertical="bottom"`: the header block is padded above.
                    let offset = height - cell_lines.len();
                    line.checked_sub(offset).and_then(|row| cell_lines.get(row))
                } else {
                    cell_lines.get(line)
                };
                let content = content.map(String::as_str).unwrap_or_default();
                let width = widths[index].saturating_sub(2);
                if widths[index] > 0 {
                    text.push(' ');
                }
                text.push_str(content);
                let used = cell_len(content);
                for _ in used..width {
                    text.push(' ');
                }
                if widths[index] > 1 {
                    text.push(' ');
                }
                if index + 1 < widths.len() {
                    text.push(divider);
                }
            }
            text.push(divider);
            lines.push(text);
        }
        lines
    }
}

/// A horizontal box row, e.g. `┏━━━┳━━┓`.
fn border(left: &str, horizontal: &str, cross: &str, right: &str, widths: &[usize]) -> String {
    let mut text = String::new();
    text.push_str(left);
    for (index, width) in widths.iter().enumerate() {
        text.push_str(&horizontal.repeat(*width));
        if index + 1 < widths.len() {
            text.push_str(cross);
        }
    }
    text.push_str(right);
    text
}

/// Center `text` in `width` cells (the reference's `justify="center"`).
fn center(text: &str, width: usize) -> String {
    let used = cell_len(text);
    if used >= width {
        return text.to_owned();
    }
    let left = (width - used) / 2;
    let mut line = " ".repeat(left);
    line.push_str(text);
    line.push_str(&" ".repeat(width - used - left));
    line
}

/// `Text.wrap` + `Text.truncate` + left justification for one cell.
fn render_cell(text: &str, width: usize, column: &TableColumn) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if column.no_wrap {
        return vec![pad_right(&truncate(text, width, column.overflow), width)];
    }
    let fold = column.overflow == Overflow::Fold;
    let mut lines = Vec::new();
    for logical in text.split('\n') {
        for line in wrap_line(logical, width, fold) {
            let line = truncate(&line, width, column.overflow);
            lines.push(pad_right(&line, width));
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Greedy word wrap at `width` cells (`Text.divide_line`).
fn wrap_line(text: &str, width: usize, fold: bool) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split(' ').filter(|word| !word.is_empty()) {
        if current.is_empty() {
            if cell_len(word) > width && fold {
                let (head, tail) = split_cells(word, width);
                lines.push(head);
                let mut rest = tail;
                while cell_len(&rest) > width {
                    let (head, tail) = split_cells(&rest, width);
                    lines.push(head);
                    rest = tail;
                }
                current = rest;
            } else {
                current = word.to_owned();
            }
            continue;
        }
        if cell_len(&current) + 1 + cell_len(word) <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            if cell_len(word) > width && fold {
                let (head, tail) = split_cells(word, width);
                lines.push(head);
                let mut rest = tail;
                while cell_len(&rest) > width {
                    let (head, tail) = split_cells(&rest, width);
                    lines.push(head);
                    rest = tail;
                }
                current = rest;
            } else {
                current = word.to_owned();
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Split `text` after at most `width` cells, returning `(head, tail)`.
fn split_cells(text: &str, width: usize) -> (String, String) {
    let mut used = 0usize;
    let mut split = 0usize;
    for (offset, character) in text.char_indices() {
        let mut single = [0u8; 4];
        let cells = cell_len(character.encode_utf8(&mut single));
        if used + cells > width {
            break;
        }
        used += cells;
        split = offset + character.len_utf8();
    }
    (text[..split].to_owned(), text[split..].to_owned())
}

/// `Text.truncate(width, overflow=…)`: cut to `width` cells, with `…` for the
/// ellipsis overflow.
fn truncate(text: &str, width: usize, overflow: Overflow) -> String {
    if cell_len(text) <= width {
        return text.to_owned();
    }
    match overflow {
        Overflow::Fold => split_cells(text, width).0,
        Overflow::Ellipsis => {
            let target = width.saturating_sub(1);
            let mut head = split_cells(text, target).0;
            head.push('…');
            head
        }
    }
}

/// `justify="left"`: pad on the right to the column width.
fn pad_right(text: &str, width: usize) -> String {
    let used = cell_len(text);
    if used >= width {
        return text.to_owned();
    }
    let mut line = text.to_owned();
    line.push_str(&" ".repeat(width - used));
    line
}

/// `_collapse_widths`: shrink the widest wrapable column until the table fits.
fn collapse_widths(widths: &[usize], wrapable: &[bool], max_width: usize) -> Vec<usize> {
    let mut widths = widths.to_vec();
    let mut total: usize = widths.iter().sum();
    let mut excess = total as i64 - max_width as i64;
    if !wrapable.iter().any(|allow| *allow) {
        return widths;
    }
    while total != 0 && excess > 0 {
        let max_column = widths
            .iter()
            .zip(wrapable)
            .filter(|(_, allow)| **allow)
            .map(|(width, _)| *width)
            .max()
            .unwrap_or(0);
        let second_max = widths
            .iter()
            .zip(wrapable)
            .map(|(width, allow)| {
                if *allow && *width != max_column {
                    *width
                } else {
                    0
                }
            })
            .max()
            .unwrap_or(0);
        let difference = max_column.saturating_sub(second_max);
        let ratios: Vec<i64> = widths
            .iter()
            .zip(wrapable)
            .map(|(width, allow)| i64::from(*allow && *width == max_column))
            .collect();
        if !ratios.iter().any(|ratio| *ratio != 0) || difference == 0 {
            break;
        }
        let max_reduce: Vec<i64> = vec![excess.min(difference as i64); widths.len()];
        widths = ratio_reduce(excess, &ratios, &max_reduce, &widths);
        total = widths.iter().sum();
        excess = total as i64 - max_width as i64;
    }
    widths
}

/// `rich._ratio.ratio_reduce`.
fn ratio_reduce(total: i64, ratios: &[i64], maximums: &[i64], values: &[usize]) -> Vec<usize> {
    let ratios: Vec<i64> = ratios
        .iter()
        .zip(maximums)
        .map(|(ratio, maximum)| if *maximum != 0 { *ratio } else { 0 })
        .collect();
    let mut total_ratio: i64 = ratios.iter().sum();
    if total_ratio == 0 {
        return values.to_vec();
    }
    let mut total_remaining = total;
    let mut result = Vec::with_capacity(values.len());
    for ((ratio, maximum), value) in ratios.iter().zip(maximums).zip(values) {
        if *ratio != 0 && total_ratio > 0 {
            let share = *ratio as f64 * total_remaining as f64 / total_ratio as f64;
            let distributed = (*maximum).min(round_half_even(share));
            result.push((*value as i64 - distributed).max(0) as usize);
            total_remaining -= distributed;
            total_ratio -= *ratio;
        } else {
            result.push(*value);
        }
    }
    result
}

/// Python's `round()` on a float: round half to even.
fn round_half_even(value: f64) -> i64 {
    let floor = value.floor();
    let fraction = value - floor;
    let rounded = if (fraction - 0.5).abs() < f64::EPSILON {
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        value.round()
    };
    rounded as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> RichTable {
        let mut table = RichTable::new(
            "Agent Profiles",
            vec![
                TableColumn::unwrapped("Profile ID"),
                TableColumn::wrapped("Friendly Name"),
                TableColumn::wrapped("Role"),
                TableColumn::wrapped("Source"),
            ],
        );
        table.push_row(vec![
            "analyst-annie".to_owned(),
            "Analyst Annie".to_owned(),
            "analyst".to_owned(),
            "built-in".to_owned(),
        ]);
        table.push_row(vec![
            "retrospective-facilitator".to_owned(),
            "Retrospective Facilitator".to_owned(),
            "architecture-scout".to_owned(),
            "built-in".to_owned(),
        ]);
        table
    }

    #[test]
    fn wide_layout_matches_the_reference_bytes() {
        let lines = table().render(200);
        assert_eq!(
            lines,
            vec![
                "                                     Agent Profiles                                      ",
                "┏━━━━━━━━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━━━━━━━━━━━━┳━━━━━━━━━━┓",
                "┃ Profile ID                ┃ Friendly Name             ┃ Role               ┃ Source   ┃",
                "┡━━━━━━━━━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━━━━━━━━━━━━╇━━━━━━━━━━┩",
                "│ analyst-annie             │ Analyst Annie             │ analyst            │ built-in │",
                "│ retrospective-facilitator │ Retrospective Facilitator │ architecture-scout │ built-in │",
                "└───────────────────────────┴───────────────────────────┴────────────────────┴──────────┘",
            ]
        );
    }

    #[test]
    fn narrow_layout_wraps_and_ellipsizes_like_the_reference() {
        let lines = table().render(80);
        for line in &lines {
            assert_eq!(
                cell_len(line),
                80,
                "every line fills the console width: {line}"
            );
        }
        assert!(lines.iter().any(|line| line.contains("architecture-sco…")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Retrospective") && !line.contains("Facilitator")),
            "{lines:#?}"
        );
        assert_eq!(lines[0].len(), 80, "the title is centered over the table");
    }
}
