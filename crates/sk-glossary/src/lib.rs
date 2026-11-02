//! Read-only glossary support for the Rust Spec Kitty implementation.
//!
//! This crate ports the observable seed and resolution behavior of upstream
//! `src/glossary/{models,scope,resolution,seed_schema,store}.py` without
//! event-ledger writes, Python runtime wrappers, LLM extraction, or seed
//! reserialization.
//!
//! The public flow is:
//!
//! ```no_run
//! use sk_glossary::{Glossary, LookupStatus};
//!
//! let glossary = Glossary::load("/path/to/repo")?;
//! let result = glossary.resolve("workspace")?;
//! if result.status == LookupStatus::Ambiguous {
//!     for candidate in &result.matches {
//!         println!("{}: {}", candidate.scope, candidate.definition);
//!     }
//! }
//! # Ok::<(), sk_glossary::GlossaryError>(())
//! ```
//!
//! Seeds are read from `.kittify/glossaries/<scope>.yaml`. Missing files are
//! empty scopes. Resolved seed paths must stay inside the repository root, so a
//! symlink escape is rejected. Unknown fields, malformed YAML, non-normalized
//! surfaces, and invalid values are rejected with structured validation errors.

#![forbid(unsafe_code)]

pub mod error;
pub mod models;
pub mod resolution;
pub mod scope;
pub mod seed;
pub mod store;

pub use error::{GlossaryError, SeedFileError, SeedValidationError};
pub use models::{Provenance, TermMetadata, TermSense, TermStatus, TermSurface};
pub use resolution::{
    ConflictType, LookupOptions, LookupResult, LookupStatus, SemanticConflict, SenseMatch,
    Severity, classify_conflict, resolve_term, score_severity,
};
pub use scope::{GlossaryScope, SCOPE_RESOLUTION_ORDER};
pub use seed::{MAX_SEED_BYTES, load_seed_file, parse_seed, seed_path};
pub use store::{Glossary, GlossaryStore};
