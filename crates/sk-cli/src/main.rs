//! `spec-kitty-rust`: the native inspection binary.
//!
//! This shell is deliberately thin: it forwards to [`sk_cli::native_main`],
//! which owns argument parsing, dispatch, the versioned JSON envelope and the
//! exit-code contract. It never invokes Python.

use std::process::ExitCode;

fn main() -> ExitCode {
    sk_cli::native_main(std::env::args_os())
}
