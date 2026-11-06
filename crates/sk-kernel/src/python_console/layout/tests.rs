//! Measured reference layout facts for the plain-console text layout.
//!
//! Every `(width, input, expected)` row below was read off the pinned oracle
//! (Rich 15.0.0, CPython 3.11.15) through plain `rich.console.Console` —
//! `console.print(payload, markup=False)` with `COLUMNS` set — and then stripped
//! of the one trailing newline that `console.print` appends (`end="\n"`), which
//! the CLI writer adds back after the layout. Probe receipts:
//! `/tmp/sk-cf56-ev/probe/plain_console_matrix.py` (label `cf56-40`).
//!
//! The rows deliberately cover width 80/40/10/5/1, tabs (including a tab after
//! wide cells), wide CJK cells, combining marks, a ZWJ family emoji, VS16, an
//! unabridged long word, a long CJK word (grapheme folding), NBSP as whitespace,
//! NUL/C0/DEL/C1 units, private-use and noncharacter code points, lone
//! surrogates, a carriage return (`Text.__init__` strips it), explicit
//! newlines, a trailing newline and blank lines.

use super::*;

fn layout(text: &str, width: usize) -> String {
    let value = PythonString::from_scalars(text);
    layout_console_text(&value, width)
        .to_scalar_string()
        .expect("the measured rows are scalar text")
}

#[test]
fn measured_reference_layouts_match() {
    for &(width, input, expected) in MEASURED_LAYOUTS {
        assert_eq!(
            layout(input, width),
            expected,
            "width {width}, input {input:?}"
        );
    }
}

#[test]
fn no_laid_out_line_is_wider_than_the_console() {
    for &(width, input, _) in MEASURED_LAYOUTS {
        let laid_out = layout(input, width);
        for line in laid_out.split('\n') {
            let codepoints: Vec<u32> = line.chars().map(u32::from).collect();
            assert!(
                codepoint_cell_len(&codepoints) <= width,
                "width {width}, input {input:?}, line {line:?}"
            );
        }
    }
}

#[test]
fn lone_surrogate_rows_match_and_keep_exact_code_units() {
    // A lone surrogate has no Rust `str` spelling, so the measured rows for it
    // live in their own code-point table. The rows also prove that the layout
    // keeps the exact units: a `to_string_lossy` projection would have inserted
    // U+FFFD and changed what the encoder sees, and a `str` fast path would have
    // refused the input outright.
    for &(width, input, expected) in MEASURED_SURROGATE_LAYOUTS {
        let value = PythonString::from_codepoints(input).expect("valid code units");
        assert_eq!(
            layout_console_text(&value, width).codepoints(),
            expected,
            "width {width}"
        );
    }
}

#[test]
fn a_tab_expands_to_the_next_stop_measured_in_cells() {
    let wide = PythonString::from_scalars("\u{65e5}\u{672c}\tx");
    assert_eq!(
        layout_console_text(&wide, 80).to_scalar_string().unwrap(),
        "\u{65e5}\u{672c}    x"
    );
    let ascii = PythonString::from_scalars("a\tb\tc");
    assert_eq!(
        layout_console_text(&ascii, 80).to_scalar_string().unwrap(),
        "a       b       c"
    );
}

/// The measured rows whose payload holds lone surrogates, as exact code units
/// (`a`, `\udc80`, `b`, `\udfff`, `c`).
const MEASURED_SURROGATE_LAYOUTS: &[(usize, &[u32], &[u32])] = &[
    (80, &UNITS, &UNITS),
    (5, &UNITS, &UNITS),
    (
        1,
        &UNITS,
        &[
            b'a' as u32,
            0x0A,
            0xDC80,
            0x0A,
            b'b' as u32,
            0x0A,
            0xDFFF,
            0x0A,
            b'c' as u32,
        ],
    ),
];

const UNITS: [u32; 5] = [b'a' as u32, 0xDC80, b'b' as u32, 0xDFFF, b'c' as u32];

const MEASURED_LAYOUTS: &[(usize, &str, &str)] = &[
    // (width, input, expected layout) measured from the pinned oracle.
    (80, "", ""),
    (80, "   ", "   "),
    (
        80,
        "one two three four five six seven eight nine ten",
        "one two three four five six seven eight nine ten",
    ),
    (80, "a\nb\n", "a\nb\n"),
    (80, "\n", "\n"),
    (80, "a\n\nb", "a\n\nb"),
    (80, "a\tb\tc", "a       b       c"),
    (80, "\tabc", "        abc"),
    (80, "\u{65e5}\u{672c}\tx", "\u{65e5}\u{672c}    x"),
    (
        80,
        "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} ab",
        "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} ab",
    ),
    (
        80,
        "e\u{301}e\u{301}e\u{301} x",
        "e\u{301}e\u{301}e\u{301} x",
    ),
    (
        80,
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} family",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} family",
    ),
    (80, "\u{2764}\u{fe0f} heart", "\u{2764}\u{fe0f} heart"),
    (
        80,
        "abcdefghijklmnopqrstuvwxyz",
        "abcdefghijklmnopqrstuvwxyz",
    ),
    (
        80,
        "\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}",
        "\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}",
    ),
    (80, "alpha\u{a0}beta gamma", "alpha\u{a0}beta gamma"),
    (
        80,
        "a\u{0}b\u{1}c\u{7f}d\u{85}e\u{9b}f",
        "a\u{0}b\u{1}c\u{7f}d\u{85}e\u{9b}f",
    ),
    (80, "\u{e000}p\u{f8ff}", "\u{e000}p\u{f8ff}"),
    (80, "a\u{fffe}b\u{10ffff}c", "a\u{fffe}b\u{10ffff}c"),
    (80, "a\u{d}b", "ab"),
    (80, "aa\u{a0}bb\u{a0}cc", "aa\u{a0}bb\u{a0}cc"),
    (
        80,
        "\u{1f468}\u{200d}\u{1f469}x",
        "\u{1f468}\u{200d}\u{1f469}x",
    ),
    (40, "", ""),
    (40, "   ", "   "),
    (
        40,
        "one two three four five six seven eight nine ten",
        "one two three four five six seven eight \nnine ten",
    ),
    (40, "a\nb\n", "a\nb\n"),
    (40, "\n", "\n"),
    (40, "a\n\nb", "a\n\nb"),
    (40, "a\tb\tc", "a       b       c"),
    (40, "\tabc", "        abc"),
    (40, "\u{65e5}\u{672c}\tx", "\u{65e5}\u{672c}    x"),
    (
        40,
        "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} ab",
        "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} ab",
    ),
    (
        40,
        "e\u{301}e\u{301}e\u{301} x",
        "e\u{301}e\u{301}e\u{301} x",
    ),
    (
        40,
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} family",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} family",
    ),
    (40, "\u{2764}\u{fe0f} heart", "\u{2764}\u{fe0f} heart"),
    (
        40,
        "abcdefghijklmnopqrstuvwxyz",
        "abcdefghijklmnopqrstuvwxyz",
    ),
    (
        40,
        "\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}",
        "\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}",
    ),
    (40, "alpha\u{a0}beta gamma", "alpha\u{a0}beta gamma"),
    (
        40,
        "a\u{0}b\u{1}c\u{7f}d\u{85}e\u{9b}f",
        "a\u{0}b\u{1}c\u{7f}d\u{85}e\u{9b}f",
    ),
    (40, "\u{e000}p\u{f8ff}", "\u{e000}p\u{f8ff}"),
    (40, "a\u{fffe}b\u{10ffff}c", "a\u{fffe}b\u{10ffff}c"),
    (40, "a\u{d}b", "ab"),
    (40, "aa\u{a0}bb\u{a0}cc", "aa\u{a0}bb\u{a0}cc"),
    (
        40,
        "\u{1f468}\u{200d}\u{1f469}x",
        "\u{1f468}\u{200d}\u{1f469}x",
    ),
    (10, "", ""),
    (10, "   ", "   "),
    (
        10,
        "one two three four five six seven eight nine ten",
        "one two \nthree four\nfive six \nseven \neight nine\nten",
    ),
    (10, "a\nb\n", "a\nb\n"),
    (10, "\n", "\n"),
    (10, "a\n\nb", "a\n\nb"),
    (10, "a\tb\tc", "a       b \nc"),
    (10, "\tabc", "        ab\nc"),
    (10, "\u{65e5}\u{672c}\tx", "\u{65e5}\u{672c}    x"),
    (
        10,
        "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} ab",
        "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\n\u{30b9}\u{30c8} ab",
    ),
    (
        10,
        "e\u{301}e\u{301}e\u{301} x",
        "e\u{301}e\u{301}e\u{301} x",
    ),
    (
        10,
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} family",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} family",
    ),
    (10, "\u{2764}\u{fe0f} heart", "\u{2764}\u{fe0f} heart"),
    (
        10,
        "abcdefghijklmnopqrstuvwxyz",
        "abcdefghij\nklmnopqrst\nuvwxyz",
    ),
    (
        10,
        "\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}",
        "\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\n\u{8a9e}\u{65e5}\u{672c}\u{8a9e}",
    ),
    (10, "alpha\u{a0}beta gamma", "alpha\u{a0}beta\ngamma"),
    (
        10,
        "a\u{0}b\u{1}c\u{7f}d\u{85}e\u{9b}f",
        "a\u{0}b\u{1}c\u{7f}d\u{85}e\u{9b}f",
    ),
    (10, "\u{e000}p\u{f8ff}", "\u{e000}p\u{f8ff}"),
    (10, "a\u{fffe}b\u{10ffff}c", "a\u{fffe}b\u{10ffff}c"),
    (10, "a\u{d}b", "ab"),
    (10, "aa\u{a0}bb\u{a0}cc", "aa\u{a0}bb\u{a0}cc"),
    (
        10,
        "\u{1f468}\u{200d}\u{1f469}x",
        "\u{1f468}\u{200d}\u{1f469}x",
    ),
    (5, "", ""),
    (5, "   ", "   "),
    (
        5,
        "one two three four five six seven eight nine ten",
        "one \ntwo \nthree\nfour \nfive \nsix \nseven\neight\nnine \nten",
    ),
    (5, "a\nb\n", "a\nb\n"),
    (5, "\n", "\n"),
    (5, "a\n\nb", "a\n\nb"),
    (5, "a\tb\tc", "a    \nb    \nc"),
    (5, "\tabc", "     \n   ab\nc"),
    (5, "\u{65e5}\u{672c}\tx", "\u{65e5}\u{672c} \nx"),
    (
        5,
        "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} ab",
        "\u{65e5}\u{672c}\n\u{8a9e}\u{30c6}\n\u{30ad}\u{30b9}\n\u{30c8} ab",
    ),
    (
        5,
        "e\u{301}e\u{301}e\u{301} x",
        "e\u{301}e\u{301}e\u{301} x",
    ),
    (
        5,
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} family",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\nfamil\ny",
    ),
    (5, "\u{2764}\u{fe0f} heart", "\u{2764}\u{fe0f} \nheart"),
    (
        5,
        "abcdefghijklmnopqrstuvwxyz",
        "abcde\nfghij\nklmno\npqrst\nuvwxy\nz",
    ),
    (
        5,
        "\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}",
        "\u{65e5}\u{672c}\n\u{8a9e}\u{65e5}\n\u{672c}\u{8a9e}\n\u{65e5}\u{672c}\n\u{8a9e}",
    ),
    (5, "alpha\u{a0}beta gamma", "alpha\nbeta \ngamma"),
    (
        5,
        "a\u{0}b\u{1}c\u{7f}d\u{85}e\u{9b}f",
        "a\u{0}b\u{1}c\u{7f}d\ne\u{9b}f",
    ),
    (5, "\u{e000}p\u{f8ff}", "\u{e000}p\u{f8ff}"),
    (5, "a\u{fffe}b\u{10ffff}c", "a\u{fffe}b\u{10ffff}c"),
    (5, "a\u{d}b", "ab"),
    (5, "aa\u{a0}bb\u{a0}cc", "aa\u{a0}bb\ncc"),
    (
        5,
        "\u{1f468}\u{200d}\u{1f469}x",
        "\u{1f468}\u{200d}\u{1f469}x",
    ),
    (1, "", ""),
    (1, "   ", " "),
    (
        1,
        "one two three four five six seven eight nine ten",
        "o\nn\ne\n \nt\nw\no\n \nt\nh\nr\ne\ne\n \nf\no\nu\nr\n \nf\ni\nv\ne\n \ns\ni\nx\n \ns\ne\nv\ne\nn\n \ne\ni\ng\nh\nt\n \nn\ni\nn\ne\n \nt\ne\nn",
    ),
    (1, "a\nb\n", "a\nb\n"),
    (1, "\n", "\n"),
    (1, "a\n\nb", "a\n\nb"),
    (1, "a\tb\tc", "a\nb\nc"),
    (1, "\tabc", " \n \n \n \n \n \n \n \na\nb\nc"),
    (1, "\u{65e5}\u{672c}\tx", " \n \n \n \n \n \nx"),
    (
        1,
        "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} ab",
        " \n \n \n \n \n \n \n \na\nb",
    ),
    (
        1,
        "e\u{301}e\u{301}e\u{301} x",
        "e\u{301}\ne\u{301}\ne\u{301}\n \nx",
    ),
    (
        1,
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} family",
        " \n \nf\na\nm\ni\nl\ny",
    ),
    (1, "\u{2764}\u{fe0f} heart", " \n \nh\ne\na\nr\nt"),
    (
        1,
        "abcdefghijklmnopqrstuvwxyz",
        "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np\nq\nr\ns\nt\nu\nv\nw\nx\ny\nz",
    ),
    (
        1,
        "\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}\u{65e5}\u{672c}\u{8a9e}",
        " \n \n \n \n \n \n \n \n ",
    ),
    (
        1,
        "alpha\u{a0}beta gamma",
        "a\nl\np\nh\na\n\u{a0}\nb\ne\nt\na\n \ng\na\nm\nm\na",
    ),
    (
        1,
        "a\u{0}b\u{1}c\u{7f}d\u{85}e\u{9b}f",
        "a\u{0}\nb\u{1}\nc\u{7f}\nd\ne\u{9b}\nf",
    ),
    (1, "\u{e000}p\u{f8ff}", "\u{e000}\np\n\u{f8ff}"),
    (1, "a\u{fffe}b\u{10ffff}c", "a\n\u{fffe}\nb\n\u{10ffff}\nc"),
    (1, "a\u{d}b", "a\nb"),
    (1, "aa\u{a0}bb\u{a0}cc", "a\na\n\u{a0}\nb\nb\n\u{a0}\nc\nc"),
    (1, "\u{1f468}\u{200d}\u{1f469}x", " \nx"),
];
