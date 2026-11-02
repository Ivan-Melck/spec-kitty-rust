//! `spec-kitty`: the upstream-shaped native CLI.
//!
//! This shell forwards to [`sk_cli::compat::main`]; all routing, payloads and
//! refusal policy live in the library. It never invokes Python.

use std::process::ExitCode;

fn main() -> ExitCode {
    sk_cli::compat::main(std::env::args_os())
}
