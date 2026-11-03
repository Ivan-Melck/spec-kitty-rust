//! Real, contained prompt artifacts for issued steps.
//!
//! Upstream writes prompts to a temporary namespace. The native store keeps
//! them inside the run directory so a prompt is inspectable, durable with the
//! run, and cannot escape it. These files are derived artifacts, not cursor
//! state: recovery never depends on them.

use std::path::{Path, PathBuf};

use sk_kernel::KernelError;

use crate::error::RunStoreError;

/// A prompt filename is a single, contained path component.
pub fn prompt_file_path(run_dir: &Path, step_id: &str) -> Result<PathBuf, RunStoreError> {
    let raw = run_dir.join(format!("{step_id}.prompt.md"));
    if step_id.is_empty() {
        return Err(RunStoreError::PromptPathEscape {
            step_id: step_id.to_owned(),
            path: raw,
        });
    }
    let safe = !step_id.is_empty()
        && step_id.len() <= 128
        && !step_id.starts_with('.')
        && !step_id.ends_with('.')
        && !step_id.contains("..")
        && step_id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        });
    let stem = step_id
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let windows_reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                number.len() == 1 && b"123456789".contains(&number.as_bytes()[0])
            });
    // '%' cannot appear in the legacy safe namespace, avoiding collisions
    // between an authored safe id and an encoded id. Preserve the real id in
    // the prompt content; filenames are derived artifacts, not step identity.
    let filename = if safe && !windows_reserved {
        format!("{step_id}.prompt.md")
    } else {
        format!(
            "%{}.prompt.md",
            sk_kernel::content_digest::sha256_hex(step_id.as_bytes())
        )
    };
    let relative = Path::new("prompts").join(filename);
    let resolved = sk_kernel::resolve_within(run_dir, &relative).map_err(|error| {
        RunStoreError::PromptPathEscape {
            step_id: step_id.to_string(),
            path: match error {
                KernelError::PathEscape { resolved, .. } => resolved,
                _ => raw,
            },
        }
    })?;
    if !resolved.starts_with(run_dir) {
        return Err(RunStoreError::PromptPathEscape {
            step_id: step_id.to_string(),
            path: resolved,
        });
    }
    Ok(resolved)
}

/// Write the real prompt artifact for an issued step and return its path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_prompt(
    run_dir: &Path,
    step_id: &str,
    step_title: &str,
    prompt: &str,
    run_id: &str,
    mission_key: &str,
    template_hash: &str,
) -> Result<PathBuf, RunStoreError> {
    let path = prompt_file_path(run_dir, step_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| {
            RunStoreError::Kernel(KernelError::Io {
                action: "create prompt directory",
                path: parent.to_path_buf(),
                source,
            })
        })?;
    }
    let content = format!(
        "# Mission step: {step_title}\n\nrun_id: {run_id}\nmission_key: {mission_key}\nstep_id: {step_id}\ntemplate_hash: {template_hash}\n\n---\n\n{prompt}\n"
    );
    sk_kernel::atomic_write(&path, content.as_bytes())?;
    Ok(path)
}
