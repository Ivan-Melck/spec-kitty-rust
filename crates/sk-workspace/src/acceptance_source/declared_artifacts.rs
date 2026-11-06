//! The declared artifact set Source's acceptance path reads.
//!
//! Source `_missing_artifacts(planning_read_dir, mission)` builds two lists:
//!
//! * **required** — the configured `input.spec.main` / `output.plan.main` /
//!   `output.tasks.list` filenames, which the pinned software-dev
//!   `expected-artifacts.yaml` resolves to `spec.md` / `plan.md` / `tasks.md`.
//!   They are deliberately *not* `mission.artifacts.required` (a research
//!   Mission's extra `findings.md` is enforced by its path conventions, not by
//!   this list), so this module keeps them as the pinned trio while remaining
//!   derived from that manifest's values.
//! * **optional** — `_optional_artifact_tokens(mission)`: the resolved
//!   Mission's own `artifacts.optional` in authored order, and only
//!   [`FALLBACK_OPTIONAL_ARTIFACTS`] when no Mission resolved at all. An
//!   `artifacts` block that declares nothing yields an EMPTY optional set — it
//!   is never quietly replaced by the fallback list.
//!
//! Missing entries are rendered exactly as Source renders them:
//! `str((planning_read_dir / token).relative_to(planning_read_dir))`, i.e. the
//! token's `pathlib` text with `contracts/` becoming `contracts`. Existence has
//! `os.path.exists` semantics.

use std::path::Path;

use sk_kernel::KernelError;
use sk_kernel::PythonString;
use sk_mission::MissionDefinition;
use sk_mission::path_conventions::tokens::relative_posix_text;

use crate::error::WorkspaceError;

/// Source `_FALLBACK_OPTIONAL_ARTIFACTS`: the optional tokens Source uses for
/// "no Mission config available", in Source's order.
pub const FALLBACK_OPTIONAL_ARTIFACTS: [&str; 4] =
    ["quickstart.md", "data-model.md", "research.md", "contracts"];

/// Source `_optional_artifact_tokens`.
///
/// A resolved Mission supplies its declared `artifacts.optional` verbatim
/// (possibly empty). Only an unresolved Mission falls back — Source guards on
/// `getattr(mission.config, "artifacts", None) is not None`, and a validated
/// `MissionConfig` always carries the block.
pub fn optional_artifact_tokens(definition: Option<&MissionDefinition>) -> Vec<PythonString> {
    match definition {
        Some(definition) => definition.optional_artifacts().to_vec(),
        None => FALLBACK_OPTIONAL_ARTIFACTS
            .iter()
            .map(|token| PythonString::from_scalars(token))
            .collect(),
    }
}

/// The subset of `tokens` that is absent under `planning_read_dir`, rendered as
/// Source renders it.
///
/// # Errors
///
/// * [`WorkspaceError::InvalidArtifact`] for a token Source cannot probe: an
///   absolute token makes `Path.relative_to()` raise `ValueError`, and an
///   unpaired surrogate makes `os.stat` raise `UnicodeEncodeError`. Both are
///   Python-side crashes on this path, so refusing with a typed error preserves
///   the outcome instead of inventing a benign "present" or "missing" verdict.
/// * [`WorkspaceError::Kernel`] for a path probe that fails for a reason other
///   than absence.
pub fn missing_artifact_texts(
    planning_read_dir: &Path,
    tokens: &[PythonString],
) -> Result<Vec<String>, WorkspaceError> {
    let mut missing = Vec::new();
    for token in tokens {
        let text = match token.to_scalar_string() {
            Ok(text) => text,
            Err(_) => {
                return Err(WorkspaceError::InvalidArtifact {
                    artifact: "mission artifact token",
                    detail: format!(
                        "artifact token {} has no filesystem form here (unpaired surrogate); \
                         Source's stat probe raises UnicodeEncodeError for it",
                        token.repr()
                    ),
                });
            }
        };
        if text.starts_with('/') {
            return Err(WorkspaceError::InvalidArtifact {
                artifact: "mission artifact token",
                detail: format!(
                    "artifact token {} is absolute; Source's Path.relative_to() raises \
                     ValueError for it",
                    token.repr()
                ),
            });
        }
        let rendered = relative_posix_text(&text);
        if rendered.is_empty() {
            // Source: `Path("")` is `.`, and the Mission directory exists.
            continue;
        }
        let candidate = planning_read_dir.join(&rendered);
        let exists = sk_kernel::paths::source_path_exists(&candidate).map_err(|source| {
            WorkspaceError::Kernel(KernelError::Io {
                action: "probe a declared artifact",
                path: candidate.clone(),
                source,
            })
        })?;
        if exists {
            continue;
        }
        missing.push(rendered);
    }
    Ok(missing)
}
