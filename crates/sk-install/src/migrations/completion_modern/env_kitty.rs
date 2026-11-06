//! Per-repo operator env-file provisioning.
//!
//! | id | effect |
//! | --- | --- |
//! | [`PROVISION_KITTY_ENV_ID`] | create `.kittify/.kitty.env`, its `config.yaml` pointer and its ignore coverage |
//!
//! The scaffold is a *template*, never a credential sink: only the governed
//! operator flags that are already set in the process environment are seeded by
//! value, the secret-shaped names are emitted as commented blanks, and
//! `SPEC_KITTY_PACKS_ROOT` is never read, written or named — an always-present
//! value there would flip the pack-root/template-root precedence on every later
//! invocation.
//!
//! The three side effects reuse the canonical writers: the pointer goes through
//! [`super::support::upsert_top_level_scalar`] (line edit with a semantic
//! proof, so authored comments survive), `.gitignore` through
//! [`crate::migrations::ignore::ensure_entries`], and `.claudeignore` through a
//! no-follow read plus an atomic replacement.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::upgrade::{MigrationOutcome, NativeMigration};

use super::support;

/// `3.2.8_provision_kitty_env`.
pub const PROVISION_KITTY_ENV_ID: &str = "3.2.8_provision_kitty_env";

/// The provisioned env file, project-relative.
pub const ENV_FILE_REL: &str = ".kittify/.kitty.env";
/// The ignore pattern that must be present in both ignore files.
pub const ENV_FILE_IGNORE_ENTRY: &str = ".kittify/.kitty.env";
/// The `config.yaml` pointer key.
pub const ENV_FILE_CONFIG_KEY: &str = "env_file";
/// The `config.yaml` pointer value (the loader's own default).
pub const ENV_FILE_CONFIG_VALUE: &str = "${SPEC_KITTY_HOME}/.kitty.env";
/// The `.claudeignore` path.
pub const CLAUDEIGNORE_REL: &str = ".claudeignore";
/// The `.gitignore` path.
pub const GITIGNORE_REL: &str = ".gitignore";

/// Never seeded, never written, never named (see the module docs).
pub const NEVER_SEED_VARS: [&str; 1] = ["SPEC_KITTY_PACKS_ROOT"];

/// Operator-set flag/pointer vars seeded with their current value when present.
pub const GOVERNED_OPERATOR_VARS: [&str; 7] = [
    "SPEC_KITTY_HOME",
    "SPEC_KITTY_NON_INTERACTIVE",
    "SPEC_KITTY_FORCE_INTERACTIVE",
    "SPEC_KITTY_SKIP_PRE_REVIEW_GATE",
    "SPEC_KITTY_NO_BANNER",
    "SPEC_KITTY_NO_NAG",
    "SPEC_KITTY_NO_UPGRADE_CHECK",
];

/// Secret-shaped vars: only ever emitted as a commented blank template.
pub const GOVERNED_SECRET_VARS: [&str; 2] = ["SPEC_KITTY_ORG_TOKEN", "SPEC_KITTY_ORG_AUTH_HEADER"];

fn env_file_path(project_root: &Path) -> PathBuf {
    project_root.join(ENV_FILE_REL)
}

fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(support::CONFIG_REL)
}

/// The scaffold text: seeded operator vars plus commented secret templates.
fn env_file_content() -> String {
    let mut lines = vec![
        "# Spec Kitty per-repo operator environment file.".to_string(),
        format!("# Provisioned by migration {PROVISION_KITTY_ENV_ID} -- see"),
        "# contracts/kitty-env-loader.md and contracts/provenance-and-channel.md.".to_string(),
        "#".to_string(),
        "# SPEC_KITTY_PACKS_ROOT is intentionally never written here: an".to_string(),
        "# always-set value flips the TEMPLATE_ROOT gate (kernel/paths.rs).".to_string(),
        String::new(),
    ];
    let mut seeded = 0usize;
    for var in GOVERNED_OPERATOR_VARS {
        if NEVER_SEED_VARS.contains(&var) {
            continue;
        }
        if let Ok(value) = std::env::var(var) {
            lines.push(format!("{var}={value}"));
            seeded += 1;
        }
    }
    if seeded > 0 {
        lines.push(String::new());
    }
    lines.push(
        "# Secret-shaped vars are never auto-seeded by value -- fill in by hand:".to_string(),
    );
    for var in GOVERNED_SECRET_VARS {
        lines.push(format!("# {var}="));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn env_file_missing(project_root: &Path) -> bool {
    !env_file_path(project_root).exists()
}

fn env_config_pointer_missing(project_root: &Path) -> Result<bool, InstallError> {
    let path = config_path(project_root);
    let Some(text) = support::read_optional_text(&path)? else {
        return Ok(true);
    };
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&text).map_err(|source| InstallError::ConfigInvalid {
            path: path.clone(),
            detail: source.to_string(),
        })?;
    if value.is_null() {
        return Ok(true);
    }
    let Some(mapping) = value.as_mapping() else {
        return Err(InstallError::ConfigInvalid {
            path,
            detail: "config root must be a mapping".to_string(),
        });
    };
    Ok(!mapping.get(ENV_FILE_CONFIG_KEY).is_some_and(python_truthy))
}

/// Python `bool()` on a YAML scalar (an empty value / `false` / `0` is falsy).
fn python_truthy(value: &serde_yaml_ng::Value) -> bool {
    match value {
        serde_yaml_ng::Value::Null => false,
        serde_yaml_ng::Value::Bool(flag) => *flag,
        serde_yaml_ng::Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        serde_yaml_ng::Value::String(text) => !text.is_empty(),
        serde_yaml_ng::Value::Sequence(items) => !items.is_empty(),
        serde_yaml_ng::Value::Mapping(mapping) => !mapping.is_empty(),
        serde_yaml_ng::Value::Tagged(tagged) => python_truthy(&tagged.value),
    }
}

fn ignore_entries(path: &Path) -> Result<std::collections::BTreeSet<String>, InstallError> {
    let Some(text) = support::read_optional_text_no_follow(path)? else {
        return Ok(std::collections::BTreeSet::new());
    };
    Ok(crate::migrations::ignore::splitlines(&text)
        .into_iter()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| line.trim().to_string())
        .collect())
}

fn gitignore_missing_entry(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!ignore_entries(&project_root.join(GITIGNORE_REL))?.contains(ENV_FILE_IGNORE_ENTRY))
}

fn claudeignore_missing_entry(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!ignore_entries(&project_root.join(CLAUDEIGNORE_REL))?.contains(ENV_FILE_IGNORE_ENTRY))
}

fn append_claudeignore_entry(project_root: &Path) -> Result<(), InstallError> {
    let path = project_root.join(CLAUDEIGNORE_REL);
    // A symlinked (or otherwise unreadable) `.claudeignore` is refused: it
    // would let the append clobber whatever the link points at.
    let existing = support::read_optional_text_no_follow(&path)?.unwrap_or_default();
    // Pinned shape: `splitlines()`, a separating blank line only when the last
    // authored line is non-empty, the entry, then a terminating blank line —
    // joined with `\n` exactly (the join already ends the file with one LF).
    let mut lines: Vec<String> = crate::migrations::ignore::splitlines(&existing)
        .into_iter()
        .map(str::to_string)
        .collect();
    if lines.last().is_some_and(|line| !line.trim().is_empty()) {
        lines.push(String::new());
    }
    lines.push(ENV_FILE_IGNORE_ENTRY.to_string());
    lines.push(String::new());
    let text = lines.join("\n");
    sk_kernel::atomic_write(&path, text.as_bytes())
        .map(|_| ())
        .map_err(|error| InstallError::io("write", &path, std::io::Error::other(error.to_string())))
}

/// `detect`: the file, its pointer, or either ignore entry is absent.
pub fn detect_provision_kitty_env(project_root: &Path) -> Result<bool, InstallError> {
    if env_file_missing(project_root)
        || env_config_pointer_missing(project_root)?
        || gitignore_missing_entry(project_root)?
        || claudeignore_missing_entry(project_root)?
    {
        return Ok(true);
    }
    Ok(false)
}

/// `can_apply`: only when something is still to provision.
pub fn can_apply_provision_kitty_env(project_root: &Path) -> Result<Option<String>, InstallError> {
    if detect_provision_kitty_env(project_root)? {
        return Ok(None);
    }
    Ok(Some(
        ".kitty.env already provisioned (file, config pointer, ignore coverage all present)"
            .to_string(),
    ))
}

/// `apply`: provision each missing piece, reporting one change line per piece.
pub fn apply_provision_kitty_env(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();

    if env_file_missing(project_root) {
        let path = env_file_path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| InstallError::io("create directory", parent, source))?;
        }
        sk_kernel::no_follow::write_text_no_follow(&path, &env_file_content(), 0o644).map_err(
            |error| InstallError::io("write", &path, std::io::Error::other(error.to_string())),
        )?;
        changes.push(format!("Created {ENV_FILE_REL}"));
    }

    if env_config_pointer_missing(project_root)? {
        let path = config_path(project_root);
        let text = support::read_optional_text(&path)?.unwrap_or_default();
        let (edited, changed) = support::upsert_top_level_scalar(
            &path,
            &text,
            ENV_FILE_CONFIG_KEY,
            ENV_FILE_CONFIG_VALUE,
        )?;
        if changed {
            sk_kernel::no_follow::write_text_no_follow(&path, &edited, 0o644).map_err(|error| {
                InstallError::io("write", &path, std::io::Error::other(error.to_string()))
            })?;
        }
        changes.push(format!(
            "Registered {ENV_FILE_CONFIG_KEY}: {ENV_FILE_CONFIG_VALUE} in config.yaml"
        ));
    }

    if gitignore_missing_entry(project_root)? {
        crate::migrations::ignore::ensure_entries(project_root, &[ENV_FILE_IGNORE_ENTRY], false)?;
        changes.push(format!("Added {ENV_FILE_IGNORE_ENTRY} to .gitignore"));
    }

    if claudeignore_missing_entry(project_root)? {
        append_claudeignore_entry(project_root)?;
        changes.push(format!("Added {ENV_FILE_IGNORE_ENTRY} to .claudeignore"));
    }

    if changes.is_empty() {
        return Ok(MigrationOutcome::unchanged(vec![
            "already provisioned".to_string(),
        ]));
    }
    Ok(MigrationOutcome::changed(changes))
}

/// The provisioning implementation.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[NativeMigration {
    id: PROVISION_KITTY_ENV_ID,
    detect: detect_provision_kitty_env,
    can_apply: can_apply_provision_kitty_env,
    apply: apply_provision_kitty_env,
}];
