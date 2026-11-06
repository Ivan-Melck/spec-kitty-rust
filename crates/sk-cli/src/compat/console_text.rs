//! Source CliConsole plain-text policy, separate from physical stdout codecs.
//!
//! One sanitizer/width/layout composition; machine JSON deliberately bypasses it.
//! Emoji-code expansion, styles/TTY sizing and global startup timing are still
//! separate unimplemented capabilities, not permission to change Source prose.
use sk_kernel::{PythonString, python_console::layout_console_text, python_text::is_digit};
use std::ffi::OsString;

/// Resolve Source Rich's non-interactive console width.
pub fn width_from_environment() -> Result<usize, String> {
    width_with(|name| std::env::var_os(name))
}

fn width_with(lookup: impl Fn(&str) -> Option<OsString>) -> Result<usize, String> {
    let Some(columns) = lookup("COLUMNS") else {
        return Ok(80);
    };
    let Some(columns) = columns.to_str() else {
        return Ok(80);
    };
    if columns.is_empty() || !columns.chars().all(|ch| is_digit(ch as u32)) {
        return Ok(80);
    }
    // The isdigit guard is Rich policy; actual int() and its startup ceiling
    // are the existing shared Kernel authorities, not another decimal parser.
    let raw_setting = lookup("PYTHONINTMAXSTRDIGITS");
    let setting = raw_setting
        .as_ref()
        .map(|value| {
            value.to_str().ok_or_else(|| {
                sk_kernel::python_json::PythonJsonErrorKind::IntegerDigitLimitSetting
                    .message()
                    .to_owned()
            })
        })
        .transpose()?;
    let limit = sk_kernel::python_json::source_integer_digit_limit(setting)
        .map_err(|error| error.to_string())?;
    let number = sk_kernel::python_int::parse_text(&PythonString::from_scalars(columns), limit)
        .map_err(|error| format!("ValueError: {error}"))?;
    Ok(number.parse::<usize>().unwrap_or(usize::MAX))
}

/// The Source CliConsole string policy, then the existing Rich layout owner.
/// `None` means zero width emits no segment (not even an empty line).
pub fn prepare(value: &PythonString, width: usize) -> Option<PythonString> {
    // CliConsole.render_str sanitizes before layout; render sanitizes emitted
    // segments again. Filtering C0 can expose a formerly incomplete CSI, so the
    // second pass is observable and must not be assumed idempotent.
    (width != 0).then(|| sanitize(&layout_console_text(&sanitize(value), width)))
}

/// Source sanitize_terminal_text's two complete sequence substitutions, then
/// C0 filtering. This is display policy, never JSON/data serialization.
fn sanitize(value: &PythonString) -> PythonString {
    let csi = without_sequences(value.codepoints(), false);
    let osc = without_sequences(&csi, true);
    PythonString::from_units(
        osc.into_iter()
            .filter(|code| matches!(*code, 9 | 10) || *code >= 32)
            .collect(),
    )
    .expect("subsequences retain valid Python code points")
}

fn without_sequences(text: &[u32], osc: bool) -> Vec<u32> {
    let mut result = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if let Some(end) = sequence_end(text, at, osc) {
            at = end;
        } else {
            result.push(text[at]);
            at += 1;
        }
    }
    result
}

fn sequence_end(text: &[u32], at: usize, osc: bool) -> Option<usize> {
    let (single, suffix) = if osc { (0x9d, 0x5d) } else { (0x9b, 0x5b) };
    let mut cursor = if text[at] == single {
        at + 1
    } else if text[at] == 0x1b && text.get(at + 1) == Some(&suffix) {
        at + 2
    } else {
        return None;
    };
    if osc {
        while cursor < text.len() && !matches!(text[cursor], 7 | 0x1b | 0x9c) {
            cursor += 1;
        }
        if matches!(text.get(cursor), Some(7 | 0x9c)) {
            cursor += 1;
        } else if text.get(cursor) == Some(&0x1b) && text.get(cursor + 1) == Some(&0x5c) {
            cursor += 2;
        }
        Some(cursor)
    } else {
        while cursor < text.len() && (0x30..=0x3f).contains(&text[cursor]) {
            cursor += 1;
        }
        while cursor < text.len() && (0x20..=0x2f).contains(&text[cursor]) {
            cursor += 1;
        }
        (cursor < text.len() && (0x40..=0x7e).contains(&text[cursor])).then_some(cursor + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn width_preserves_python_digit_and_integer_failure_semantics() {
        let width =
            |text: &str| width_with(|name| (name == "COLUMNS").then(|| OsString::from(text)));
        assert_eq!(width("10"), Ok(10));
        assert_eq!(width("١٠"), Ok(10));
        assert_eq!(width("0"), Ok(0));
        assert_eq!(width(" 10 "), Ok(80));
        assert_eq!(
            width("²"),
            Err("ValueError: invalid literal for int() with base 10: '²'".to_owned())
        );
        assert!(
            width(&"1".repeat(4301))
                .unwrap_err()
                .contains("4301 digits")
        );
        assert_eq!(width("9999999999999999999999999"), Ok(usize::MAX));
    }
}
