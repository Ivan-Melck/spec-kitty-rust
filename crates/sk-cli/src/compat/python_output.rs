//! The CLI's **physical stdout** seam, over the one shared stream composition.
//!
//! The codec, the startup policy and the error/exit classes are owned by
//! [`sk_kernel::python_stream`] (promoted there in CF79 so Doctrine's activation
//! warnings and the CLI's stdout share ONE composition of the reference's
//! `TextIOWrapper` behaviour). This module keeps the CLI's public names as
//! same-item re-exports — no wrapper type, no second encoder — plus the two
//! things that are genuinely stdout-only:
//!
//! * the Rich-console text preparation for the human arm (width from the
//!   environment, then the shared stdout policy), and
//! * the reference's own `PYTHONIOENCODING=utf-8` hint Rich appends to a stdout
//!   encoding failure.
//!
//! Doctrine must not depend on this crate, so the shared composition lives in
//! Kernel; the measured stderr facts (always `backslashreplace`, no BOM) and
//! the stderr writer live there too.

use std::io::{self, Write};

pub use sk_kernel::python_stream::{
    StreamCodec as StdoutCodec, StreamError as StdoutError, StreamPolicy as StdoutPolicy,
    WriteClass as SourceStdoutClass,
};

use sk_kernel::PythonString;

/// The reference's own Rich-supplied hint, appended to every `UnicodeEncodeError`
/// raised while writing stdout (Rich 15.0.0 `Console._write_buffer`).
pub const PYTHONIOENCODING_HINT: &str =
    "*** You may need to add PYTHONIOENCODING=utf-8 to your environment ***";

/// Write one rendered document plus the reference's trailing newline.
///
/// The reference's last step is a single `sys.stdout.write(text)` (Rich writes
/// the whole buffered text once; builtin `print` writes the document and the
/// newline). Encoding therefore happens before any byte is written: an
/// unencodable unit leaves stdout untouched, exactly like the reference.
pub fn write_stdout_line(
    value: &PythonString,
    class: SourceStdoutClass,
) -> Result<(), StdoutError> {
    let policy = StdoutPolicy::from_environment()?;
    let mut line = value.clone();
    line.push_scalars("\n");
    let bytes = policy.encode(&line)?;
    let mut stdout = io::stdout().lock();
    match stdout.write_all(&bytes) {
        Ok(()) => match stdout.flush() {
            Ok(()) => Ok(()),
            Err(error) if is_broken_pipe(&error) => Err(StdoutError::BrokenPipe { class }),
            Err(error) if is_unavailable(&error) => Err(StdoutError::Unavailable {
                detail: os_error_text(&error),
            }),
            Err(error) => Err(StdoutError::Io {
                class,
                detail: os_error_text(&error),
            }),
        },
        Err(error) if is_broken_pipe(&error) => Err(StdoutError::BrokenPipe { class }),
        Err(error) if is_unavailable(&error) => Err(StdoutError::Unavailable {
            detail: os_error_text(&error),
        }),
        Err(error) => Err(StdoutError::Io {
            class,
            detail: os_error_text(&error),
        }),
    }
}

/// Write one human-console text document (Rich `console.print(text)` class).
///
/// This is the public seam the non-include context emitters must use, so the
/// whole CLI shares one stdout policy instead of a per-command encoder.
pub fn write_source_text(value: &PythonString) -> Result<(), StdoutError> {
    // Startup stdio policy precedes console rendering even at zero width.
    StdoutPolicy::from_environment()?;
    let width = super::console_text::width_from_environment()
        .map_err(|detail| StdoutError::InvalidStartup { detail })?;
    match super::console_text::prepare(value, width) {
        Some(text) => write_stdout_line(&text, SourceStdoutClass::RichConsolePrint),
        None => Ok(()),
    }
}

/// Write one JSON document through builtin `print()` class semantics.
pub fn write_source_json(value: &PythonString) -> Result<(), StdoutError> {
    write_stdout_line(value, SourceStdoutClass::BuiltinPrint)
}

/// A closed pipe, including the `EPIPE` shape the reference reports.
fn is_broken_pipe(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::BrokenPipe || error.raw_os_error() == Some(32)
}

/// Stdout is not an open descriptor (`EBADF`): the reference has no
/// `sys.stdout` object at all.
fn is_unavailable(error: &io::Error) -> bool {
    error.raw_os_error() == Some(9)
}

/// The reference `OSError` text for one failed write.
fn os_error_text(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("OSError: [Errno {code}] {error}"),
        None => format!("OSError: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI's public names still resolve to the shared Kernel declarations and
    /// keep the stdout write classes' exit codes.
    #[test]
    fn the_stdout_seam_keeps_the_shared_names_and_write_classes() {
        assert_eq!(
            StdoutCodec::Utf8.name(),
            sk_kernel::python_stream::StreamCodec::Utf8.name()
        );
        assert_eq!(
            StdoutError::BrokenPipe {
                class: SourceStdoutClass::RichConsolePrint
            }
            .exit_code(),
            1
        );
        assert_eq!(
            StdoutError::BrokenPipe {
                class: SourceStdoutClass::BuiltinPrint
            }
            .exit_code(),
            120
        );
        assert_eq!(
            StdoutError::Io {
                class: SourceStdoutClass::BuiltinPrint,
                detail: "OSError: [Errno 28] No space left on device".to_owned()
            }
            .exit_code(),
            120
        );
        assert!(
            StdoutError::UnicodeEncode {
                message: "x".to_owned()
            }
            .is_encode_failure()
        );
    }
}
