//! The stable JSON envelope and process exit-code contract.
//!
//! One command produces exactly one JSON document on stdout. Diagnostics go to
//! stderr. The envelope is versioned so a script can branch on `ok`, `command`,
//! and the exit-code class without parsing prose.

use std::io::{self, Write};

use serde_json::{Value, json};

/// Version of the machine-readable envelope.
pub const SCHEMA_VERSION: u32 = 1;

/// A command completed and produced its result.
pub const EXIT_SUCCESS: u8 = 0;
/// The command ran, but the requested data or domain answer is a refusal.
pub const EXIT_DOMAIN: u8 = 1;
/// The invocation itself was malformed (unknown command, bad option value).
pub const EXIT_USAGE: u8 = 2;
/// The runtime planner returned a blocked decision. This is still a valid
/// query result (one JSON document, `ok: true`), but a supervising script must
/// not treat the run as advancing.
pub const EXIT_BLOCKED: u8 = 3;

/// A typed command failure.
#[derive(Debug)]
pub struct CliError {
    kind: &'static str,
    message: String,
    details: Option<Value>,
    exit_code: u8,
}

impl CliError {
    /// A malformed invocation or option value (exit 2).
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: "usage",
            message: message.into(),
            details: None,
            exit_code: EXIT_USAGE,
        }
    }

    /// A data or domain refusal (exit 1).
    pub fn domain(message: impl Into<String>) -> Self {
        Self {
            kind: "domain",
            message: message.into(),
            details: None,
            exit_code: EXIT_DOMAIN,
        }
    }

    /// A filesystem or other I/O failure (exit 1).
    pub fn io(message: impl Into<String>) -> Self {
        Self {
            kind: "io",
            message: message.into(),
            details: None,
            exit_code: EXIT_DOMAIN,
        }
    }

    /// An internal adapter failure (exit 1). Never used for user input.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: "internal",
            message: message.into(),
            details: None,
            exit_code: EXIT_DOMAIN,
        }
    }

    /// Attach a machine-readable detail object.
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// The stable error-kind string.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The process exit code for this failure.
    pub fn exit_code(&self) -> u8 {
        self.exit_code
    }
}

/// The data a successful command returns, plus the process exit code.
#[derive(Debug)]
pub struct CommandData {
    /// The command's `data` payload.
    pub data: Value,
    /// Normally zero; a blocked runtime query uses [`EXIT_BLOCKED`].
    pub exit_code: u8,
}

impl CommandData {
    /// A normal success (exit 0).
    pub fn success(data: Value) -> Self {
        Self {
            data,
            exit_code: EXIT_SUCCESS,
        }
    }

    /// A successful query with a non-zero advisory/blocking exit code.
    pub fn with_exit(data: Value, exit_code: u8) -> Self {
        Self { data, exit_code }
    }
}

/// One completed command.
#[derive(Debug)]
pub struct Success {
    /// Stable dotted command name.
    pub command: &'static str,
    /// The command result.
    pub data: Value,
    /// Process exit code.
    pub exit_code: u8,
}

/// Emit the success document and return its exit code.
pub fn emit_success(success: &Success) -> u8 {
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "command": success.command,
        "exit_code": success.exit_code,
        "data": success.data,
    });
    if let Err(error) = write_document(&document) {
        eprintln!("error: could not write the JSON result to stdout: {error}");
        return EXIT_DOMAIN;
    }
    success.exit_code
}

/// Emit the failure document (and a stderr diagnostic), returning its exit code.
pub fn emit_failure(command: &str, error: &CliError) -> u8 {
    eprintln!("error: {}", error.message());
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "ok": false,
        "command": command,
        "exit_code": error.exit_code(),
        "error": {
            "kind": error.kind(),
            "message": error.message(),
            "details": &error.details,
        },
    });
    if let Err(write_error) = write_document(&document) {
        eprintln!("error: could not write the JSON failure to stdout: {write_error}");
    }
    error.exit_code()
}

fn write_document(document: &Value) -> io::Result<()> {
    let text = serde_json::to_string(document).map_err(io::Error::other)?;
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    lock.write_all(text.as_bytes())?;
    lock.write_all(b"\n")?;
    lock.flush()
}
