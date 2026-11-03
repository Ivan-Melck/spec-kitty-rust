//! `4.0.0rc7_track_charter_pack_yaml`: make `pack.yaml` trackable again.
//!
//! The charter-pack cutover rewrites a project's blanket
//! `.kittify/charter-packs/**` ignore rule but does not add an allow-rule for
//! the `pack.yaml` the sibling `pack_level` migration writes. This migration
//! adds `!.kittify/charter-packs/pack.yaml` only where the blanket rule is
//! present and the allow-rule is absent, grouping it with the sibling
//! charter-pack allow-rules. Both plain and root-anchored spellings are
//! recognised, and the inserted rule matches the blanket rule's spelling.
//!
//! An unsafe/unreadable `.gitignore` is a non-fatal warning (the operator can
//! add one line by hand); a file that is known to need the rule but cannot be
//! written is a failure. No directory or file is created.

use std::path::Path;

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;

use super::git;

const GITIGNORE: &str = ".gitignore";
const PACK_YAML: &str = ".kittify/charter-packs/pack.yaml";

/// The blanket ignore rule the cutover emits.
pub const CHARTER_PACK_BLANKET_RULE: &str = ".kittify/charter-packs/**";
/// The root-anchored spelling of the blanket rule.
const ANCHORED_BLANKET_RULE: &str = "/.kittify/charter-packs/**";
/// The allow-rule that keeps the authored descriptor trackable.
pub const CHARTER_PACK_YAML_ALLOW_RULE: &str = "!.kittify/charter-packs/pack.yaml";
const ANCHORED_PACK_YAML_ALLOW_RULE: &str = "!/.kittify/charter-packs/pack.yaml";
const BLANKET_RULES: [&str; 2] = [CHARTER_PACK_BLANKET_RULE, ANCHORED_BLANKET_RULE];
const ALLOW_RULES: [&str; 2] = [CHARTER_PACK_YAML_ALLOW_RULE, ANCHORED_PACK_YAML_ALLOW_RULE];
const ALLOW_RULE_PREFIXES: [&str; 2] = ["!.kittify/charter-packs/", "!/.kittify/charter-packs/"];

/// Whether the text carries the blanket rule but not the `pack.yaml` allow-rule.
pub fn needs_pack_yaml_allow_rule(text: &str) -> bool {
    let rules: Vec<&str> = text.lines().map(str::trim).collect();
    rules.iter().any(|rule| BLANKET_RULES.contains(rule))
        && !rules.iter().any(|rule| ALLOW_RULES.contains(rule))
}

/// Insert the allow-rule after the last blanket rule and its sibling allows.
pub fn ensure_pack_yaml_allow_rule(text: &str) -> (String, bool) {
    let mut lines: Vec<String> = if text.is_empty() {
        Vec::new()
    } else {
        text.split_inclusive('\n').map(str::to_string).collect()
    };
    let stripped: Vec<String> = lines.iter().map(|line| line.trim().to_string()).collect();
    let blanket_indexes: Vec<usize> = stripped
        .iter()
        .enumerate()
        .filter(|(_, rule)| BLANKET_RULES.contains(&rule.as_str()))
        .map(|(index, _)| index)
        .collect();
    if blanket_indexes.is_empty()
        || stripped
            .iter()
            .any(|rule| ALLOW_RULES.contains(&rule.as_str()))
    {
        return (text.to_string(), false);
    }
    let blanket_index = *blanket_indexes.last().expect("non-empty");
    let allow_rule = if stripped[blanket_index] == ANCHORED_BLANKET_RULE {
        ANCHORED_PACK_YAML_ALLOW_RULE
    } else {
        CHARTER_PACK_YAML_ALLOW_RULE
    };
    let mut insert_at = blanket_index + 1;
    while insert_at < stripped.len()
        && ALLOW_RULE_PREFIXES
            .iter()
            .any(|prefix| stripped[insert_at].starts_with(prefix))
    {
        insert_at += 1;
    }
    let newline = if lines.iter().any(|line| line.ends_with("\r\n")) {
        "\r\n"
    } else {
        "\n"
    };
    let anchor = insert_at.saturating_sub(1);
    if anchor < lines.len()
        && !lines[anchor].is_empty()
        && !lines[anchor].ends_with('\n')
        && !lines[anchor].ends_with('\r')
    {
        lines[anchor].push_str(newline);
    }
    lines.insert(insert_at, format!("{allow_rule}{newline}"));
    (lines.concat(), true)
}

fn read_ignore(path: &Path) -> Result<Option<String>, InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(InstallError::IgnoreFileUnsafe {
            path: path.to_path_buf(),
            detail: "Refusing to read symlinked .gitignore".to_string(),
        }),
        Ok(metadata) if !metadata.file_type().is_file() => Err(InstallError::IgnoreFileUnsafe {
            path: path.to_path_buf(),
            detail: "Could not safely open .gitignore: not a regular file".to_string(),
        }),
        Ok(_) => {
            let raw = std::fs::read(path)
                .map_err(|source| InstallError::io("read .gitignore", path, source))?;
            let text = String::from_utf8(raw).map_err(|error| InstallError::IgnoreFileUnsafe {
                path: path.to_path_buf(),
                detail: format!(".gitignore is not valid UTF-8: {error}"),
            })?;
            Ok(Some(
                text.strip_prefix('\u{feff}').unwrap_or(&text).to_string(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::io("inspect .gitignore", path, source)),
    }
}

/// Detection: a project whose blanket rule still hides `pack.yaml`.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    let path = project_root.join(GITIGNORE);
    match read_ignore(&path) {
        Ok(Some(text)) => Ok(needs_pack_yaml_allow_rule(&text)),
        Ok(None) => Ok(false),
        Err(InstallError::IgnoreFileUnsafe { detail, .. }) => {
            // The source lets the symlink case fail closed; a directory/other
            // unreadable shape selects the migration so apply() can warn.
            if detail.contains("symlink") {
                Err(InstallError::IgnoreFileUnsafe { path, detail })
            } else {
                Ok(true)
            }
        }
        Err(error) => Err(error),
    }
}

/// `can_apply`: only an existing project.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if !project_root.exists() {
        return Ok(Some(format!(
            "Project path does not exist: {}",
            project_root.display()
        )));
    }
    Ok(None)
}

/// Apply the allow-rule backfill.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let path = project_root.join(GITIGNORE);
    let text = match read_ignore(&path) {
        Ok(text) => text,
        Err(error) => {
            // Non-fatal: leave the unsafe .gitignore untouched and tell the
            // operator, rather than failing the whole upgrade over a cosmetic
            // allow-rule they can add by hand.
            return Ok(MigrationOutcome {
                changed: false,
                changes: Vec::new(),
                warnings: vec![format!(
                    "{GITIGNORE} could not be read safely ({error}); add {CHARTER_PACK_YAML_ALLOW_RULE} by hand if needed"
                )],
            });
        }
    };
    let Some(text) = text else {
        return Ok(MigrationOutcome::skipped());
    };
    if !needs_pack_yaml_allow_rule(&text) {
        return Ok(MigrationOutcome::skipped());
    }
    let (updated, changed) = ensure_pack_yaml_allow_rule(&text);
    if !changed {
        return Ok(MigrationOutcome::skipped());
    }
    sk_kernel::atomic_write(&path, updated.as_bytes()).map_err(|error| {
        InstallError::io("write .gitignore", &path, std::io::Error::other(error))
    })?;
    let mut warnings = Vec::new();
    if git::check_ignore(project_root, PACK_YAML)? == Some(true) {
        warnings.push(format!(
            "{PACK_YAML} is still ignored by another {GITIGNORE} rule; check your {GITIGNORE} so it can be committed"
        ));
    }
    Ok(MigrationOutcome {
        changed: true,
        changes: vec![format!(
            "Added {CHARTER_PACK_YAML_ALLOW_RULE} to {GITIGNORE}"
        )],
        warnings,
    })
}
