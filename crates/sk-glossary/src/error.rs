//! Typed failures for glossary seed loading, validation, and resolution.

use std::fmt;
use std::io;
use std::path::PathBuf;

use serde::Serialize;
use thiserror::Error;

/// Every fallible public glossary operation returns this error type.
#[derive(Debug, Error)]
pub enum GlossaryError {
    /// A term surface was empty.
    #[error("surface must not be empty")]
    EmptySurface,

    /// A term surface was not in the normalized form required by upstream.
    #[error(
        "surface must be normalized (lowercase, trimmed): got {value:?}, expected {expected:?}"
    )]
    NonNormalizedSurface { value: String, expected: String },

    /// A definition was empty or contained only whitespace.
    #[error("definition must not be empty")]
    EmptyDefinition,

    /// Confidence fell outside the upstream `0.0..=1.0` range.
    #[error("confidence must be 0.0..1.0, got {value}")]
    InvalidConfidence { value: f64 },

    /// A scope string was not one of the four canonical scopes.
    #[error("unknown glossary scope {value:?}")]
    InvalidScope { value: String },

    /// A status string was not one of the three canonical statuses.
    #[error("unknown glossary term status {value:?}")]
    InvalidStatus { value: String },

    /// A seed file failed parsing or schema validation.
    #[error(transparent)]
    SeedFile(#[from] SeedFileError),

    /// A seed path resolved outside the repository root.
    #[error("glossary seed path {path} resolves outside repository root {root}")]
    PathEscape { root: PathBuf, path: PathBuf },

    /// The shared bounded regular-file reader refused the seed.
    #[error("failed to read glossary seed {path}: {source}")]
    BoundedRead {
        path: PathBuf,
        #[source]
        source: sk_kernel::KernelError,
    },

    /// The seed file could not be read.
    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// A single seed validation failure with its location inside the file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SeedValidationError {
    /// Seed file that failed validation.
    pub file_path: PathBuf,
    /// Zero-based index into the root `terms` list, when the error is term-scoped.
    pub term_index: Option<usize>,
    /// Raw surface text from the offending term, when one was present.
    pub term_surface: Option<String>,
    /// Offending field name, when the error is field-scoped.
    pub field: Option<String>,
    /// Human-readable explanation.
    pub message: String,
}

impl SeedValidationError {
    /// Build a validation error record.
    pub fn new(
        file_path: impl Into<PathBuf>,
        term_index: Option<usize>,
        term_surface: Option<String>,
        field: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file_path: file_path.into(),
            term_index,
            term_surface,
            field,
            message: message.into(),
        }
    }
}

/// Aggregated validation failure for one glossary seed file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedFileError {
    file_path: PathBuf,
    errors: Vec<SeedValidationError>,
}

impl SeedFileError {
    /// Build an aggregated seed error from one or more validation records.
    pub fn new(file_path: impl Into<PathBuf>, errors: Vec<SeedValidationError>) -> Self {
        Self {
            file_path: file_path.into(),
            errors,
        }
    }

    /// The seed file that failed validation.
    pub fn file_path(&self) -> &std::path::Path {
        &self.file_path
    }

    /// All collected validation records.
    pub fn errors(&self) -> &[SeedValidationError] {
        &self.errors
    }
}

impl fmt::Display for SeedFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} validation error(s) in {}:",
            self.errors.len(),
            self.file_path.display()
        )?;

        for error in &self.errors {
            let mut location = Vec::new();
            if let Some(index) = error.term_index {
                match &error.term_surface {
                    Some(surface) => location.push(format!("term[{index}] '{surface}'")),
                    None => location.push(format!("term[{index}]")),
                }
            }
            if let Some(field) = &error.field {
                location.push(field.clone());
            }
            let location = if location.is_empty() {
                "file".to_owned()
            } else {
                location.join(" → ")
            };
            write!(formatter, "\n  - {location}: {}", error.message)?;
        }

        Ok(())
    }
}

impl std::error::Error for SeedFileError {}
