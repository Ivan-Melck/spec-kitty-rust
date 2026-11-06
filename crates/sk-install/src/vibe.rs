//! Mistral Vibe's project skill-path pointer (`.vibe/config.toml`).
//!
//! Vibe resolves the shared Agent Skills through a gitignored pointer:
//!
//! ```toml
//! skill_paths = [".agents/skills"]
//! ```
//!
//! Losing that pointer breaks Vibe's command surface even though the skills are
//! installed, so `init` establishes it and a re-run restores it surgically
//! without re-writing unrelated TOML. The edit is textual and verified: the
//! parsed `skill_paths` value must equal the expected list and every other
//! top-level key must be unchanged. A `skill_paths` that is neither a string nor
//! a list of strings is a typed refusal (never a best-effort rewrite), and a
//! symlinked config is never followed.
//!
//! Only the top-level key is parsed, matching upstream's scope: this module is
//! not a TOML round-trip library and does not reformat the document.

use std::path::{Path, PathBuf};

use crate::error::InstallError;

/// Project-relative Vibe config path.
pub const VIBE_CONFIG_REL: &str = ".vibe/config.toml";

/// The shared skill root Vibe must be pointed at.
pub const VIBE_SKILL_PATH: &str = ".agents/skills";

/// Ensure `.vibe/config.toml` lists the shared skill root; returns `true` when written.
pub fn ensure_project_skill_path(project_root: &Path) -> Result<bool, InstallError> {
    let path = project_root.join(VIBE_CONFIG_REL);
    let existing = read_config(&path)?;
    let updated = match &existing {
        Some(text) => add_skill_path(text).map_err(|detail| InstallError::YamlEdit {
            path: path.clone(),
            detail,
        })?,
        None => Some(format!("skill_paths = [\"{VIBE_SKILL_PATH}\"]\n")),
    };
    let Some(updated) = updated else {
        return Ok(false);
    };
    verify(&path, existing.as_deref().unwrap_or(""), &updated)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| InstallError::io("create directory", parent, source))?;
    }
    sk_kernel::atomic_write(&path, updated.as_bytes()).map_err(|error| {
        InstallError::io("write vibe config", &path, std::io::Error::other(error))
    })?;
    Ok(true)
}

/// Whether `.vibe/config.toml` already points at the shared skill root.
pub fn skill_path_configured(project_root: &Path) -> bool {
    let path = project_root.join(VIBE_CONFIG_REL);
    read_config(&path)
        .ok()
        .flatten()
        .and_then(|text| parse_skill_paths(&text).ok().flatten())
        .is_some_and(|paths| match paths {
            SkillPaths::One(value) => value == VIBE_SKILL_PATH,
            SkillPaths::Many(values) => values.iter().any(|value| value == VIBE_SKILL_PATH),
        })
}

fn read_config(path: &Path) -> Result<Option<String>, InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(InstallError::IgnoreFileUnsafe {
            path: path.to_path_buf(),
            detail: "vibe config is a symbolic link; refusing to follow it".to_string(),
        }),
        Ok(metadata) if !metadata.file_type().is_file() => Err(InstallError::IgnoreFileUnsafe {
            path: path.to_path_buf(),
            detail: "vibe config is not a regular file".to_string(),
        }),
        Ok(_) => {
            let raw = std::fs::read(path).map_err(|error| InstallError::io("read", path, error))?;
            String::from_utf8(raw)
                .map(Some)
                .map_err(|error| InstallError::IgnoreFileUnsafe {
                    path: path.to_path_buf(),
                    detail: error.to_string(),
                })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(InstallError::io("inspect", path, source)),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SkillPaths {
    One(String),
    Many(Vec<String>),
}

/// Return the updated text, or `None` when the pointer is already present.
fn add_skill_path(text: &str) -> Result<Option<String>, String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    match find_skill_paths(text)? {
        None => Ok(Some(format!(
            "skill_paths = [\"{VIBE_SKILL_PATH}\"]{newline}{text}"
        ))),
        Some(found) => {
            let current = parse_value(&found.value)?;
            match current {
                SkillPaths::Many(values) => {
                    if values.iter().any(|value| value == VIBE_SKILL_PATH) {
                        return Ok(None);
                    }
                    // Insert before the closing bracket, keeping every comment.
                    let close = found
                        .value
                        .rfind(']')
                        .ok_or("skill_paths has no closing bracket")?;
                    let prefix = &found.value[..close];
                    let trimmed = prefix.trim_end();
                    let sep = if trimmed.trim_end_matches([' ', '\t']).ends_with('[') {
                        ""
                    } else {
                        ", "
                    };
                    let insert = format!("{sep}\"{VIBE_SKILL_PATH}\"");
                    let value = format!("{}{insert}{}", prefix, &found.value[close..]);
                    Ok(Some(splice(text, &found, &value)))
                }
                SkillPaths::One(value) => {
                    if value == VIBE_SKILL_PATH {
                        return Ok(None);
                    }
                    let value = format!("[\"{}\", \"{VIBE_SKILL_PATH}\"]", escape(&value));
                    Ok(Some(splice(text, &found, &value)))
                }
            }
        }
    }
}

fn splice(text: &str, found: &Found, replacement: &str) -> String {
    format!(
        "{}{}{}",
        &text[..found.value_start],
        replacement,
        &text[found.value_end..]
    )
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Clone, Debug)]
struct Found {
    value: String,
    value_start: usize,
    value_end: usize,
}

fn find_skill_paths(text: &str) -> Result<Option<Found>, String> {
    let mut offset = 0_usize;
    let mut in_table = false;
    while offset < text.len() {
        let line_end = text[offset..]
            .find('\n')
            .map(|index| offset + index + 1)
            .unwrap_or(text.len());
        let line = &text[offset..line_end];
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_table = true;
        }
        if !in_table
            && !trimmed.starts_with('#')
            && !trimmed.is_empty()
            && let Some(rest) = trimmed.strip_prefix("skill_paths")
            && let Some(after) = rest.trim_start().strip_prefix('=')
        {
            let value_lead = after.len() - after.trim_start().len();
            let value_start = offset
                + (line.len() - trimmed.len())
                + (trimmed.len() - rest.len())
                + (rest.trim_start().len() - after.len())
                + 1
                + value_lead;
            // Consume multi-line arrays until brackets balance.
            let mut cursor = value_start;
            let mut depth = 0_i32;
            let mut quote: Option<char> = None;
            let mut comment = false;
            while cursor < text.len() {
                let character = text[cursor..].chars().next().unwrap();
                if comment {
                    if character == '\n' {
                        comment = false;
                    }
                } else if let Some(active) = quote {
                    if active == '"' && character == '\\' {
                        cursor += character.len_utf8();
                        if cursor < text.len() {
                            cursor += text[cursor..].chars().next().unwrap().len_utf8();
                        }
                        continue;
                    }
                    if character == active {
                        quote = None;
                    }
                } else {
                    match character {
                        '"' | '\'' => quote = Some(character),
                        '#' => comment = true,
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 {
                                cursor += 1;
                                break;
                            }
                        }
                        '\n' if depth == 0 => break,
                        _ => {}
                    }
                }
                cursor += character.len_utf8();
            }
            let raw = text[value_start..cursor].trim().to_string();
            let end = value_start + text[value_start..cursor].len();
            return Ok(Some(Found {
                value: raw,
                value_start,
                value_end: end,
            }));
        }
        offset = line_end;
    }
    Ok(None)
}

fn parse_skill_paths(text: &str) -> Result<Option<SkillPaths>, String> {
    match find_skill_paths(text)? {
        Some(found) => parse_value(&found.value).map(Some),
        None => Ok(None),
    }
}

fn parse_value(raw: &str) -> Result<SkillPaths, String> {
    let raw = strip_comment(raw).trim();
    if raw.starts_with('[') {
        let inner = raw
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .ok_or("skill_paths array is not closed")?;
        let mut values = Vec::new();
        for part in split_top_level(inner) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            values.push(parse_string(part)?);
        }
        Ok(SkillPaths::Many(values))
    } else {
        Ok(SkillPaths::One(parse_string(raw)?))
    }
}

fn strip_comment(raw: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut previous = '\0';
    for (index, character) in raw.char_indices() {
        match quote {
            Some(active) if character == active && previous != '\\' => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character == '#' => return &raw[..index],
            None => {}
        }
        previous = character;
    }
    raw
}

fn split_top_level(raw: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    let mut previous = '\0';
    for (index, character) in raw.char_indices() {
        match quote {
            Some(active) if character == active && previous != '\\' => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character == ',' => {
                parts.push(&raw[start..index]);
                start = index + 1;
            }
            None => {}
        }
        previous = character;
    }
    parts.push(&raw[start..]);
    parts
}

fn parse_string(raw: &str) -> Result<String, String> {
    if let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        let mut value = String::new();
        let mut chars = inner.chars();
        while let Some(character) = chars.next() {
            if character == '\\' {
                match chars.next() {
                    Some('"') => value.push('"'),
                    Some('\\') => value.push('\\'),
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some(other) => return Err(format!("unsupported escape \\{other}")),
                    None => return Err("dangling escape".to_string()),
                }
            } else {
                value.push(character);
            }
        }
        return Ok(value);
    }
    if let Some(inner) = raw
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Ok(inner.to_string());
    }
    Err(format!("skill_paths entry {raw:?} is not a TOML string"))
}

fn verify(path: &Path, before: &str, after: &str) -> Result<(), InstallError> {
    let expected = match parse_skill_paths(after) {
        Ok(Some(SkillPaths::Many(values))) => values,
        _ => {
            return Err(InstallError::YamlEdit {
                path: path.to_path_buf(),
                detail: "edited vibe config did not produce a skill_paths array".to_string(),
            });
        }
    };
    if !expected.iter().any(|value| value == VIBE_SKILL_PATH) {
        return Err(InstallError::YamlEdit {
            path: path.to_path_buf(),
            detail: "edited vibe config does not list .agents/skills".to_string(),
        });
    }
    if !before.is_empty() {
        // Every other top-level key must be unchanged: compare the documents
        // with the skill_paths assignment (and its continuation lines) removed.
        result_preserved(path, unowned_lines(before), unowned_lines(after))?;
    }
    Ok(())
}

fn unowned_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut skipping = false;
    let mut depth = 0_i32;
    for line in text.lines() {
        let trimmed = line.trim();
        if skipping {
            depth += trimmed.matches('[').count() as i32;
            depth -= trimmed.matches(']').count() as i32;
            if depth <= 0 {
                skipping = false;
                depth = 0;
            }
            continue;
        }
        if trimmed.starts_with("skill_paths") && trimmed.contains('=') {
            depth = trimmed.matches('[').count() as i32 - trimmed.matches(']').count() as i32;
            skipping = depth > 0;
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        lines.push(trimmed.to_string());
    }
    lines
}

fn result_preserved(
    path: &Path,
    before: Vec<String>,
    after: Vec<String>,
) -> Result<(), InstallError> {
    if before != after {
        return Err(InstallError::YamlEdit {
            path: path.to_path_buf(),
            detail: "vibe config edit changed an unowned TOML key".to_string(),
        });
    }
    Ok(())
}

/// The config path helper.
pub fn vibe_config_path(project_root: &Path) -> PathBuf {
    project_root.join(VIBE_CONFIG_REL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn creates_the_pointer_when_the_file_is_absent() {
        let directory = tempfile::tempdir().expect("temp");
        assert!(ensure_project_skill_path(directory.path()).expect("ensure"));
        assert_eq!(
            std::fs::read_to_string(directory.path().join(VIBE_CONFIG_REL)).unwrap(),
            "skill_paths = [\".agents/skills\"]\n"
        );
        assert!(skill_path_configured(directory.path()));
        assert!(!ensure_project_skill_path(directory.path()).expect("no-op"));
    }

    #[test]
    fn extends_an_existing_array_and_preserves_comments_and_other_keys() {
        let directory = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(VIBE_CONFIG_REL),
            "# vibe settings\ntheme = \"dark\"  # mine\nskill_paths = [\"local/skills\"]  # keep\n[other]\nkey = \"value\"\n",
        );
        assert!(ensure_project_skill_path(directory.path()).expect("ensure"));
        let text = std::fs::read_to_string(directory.path().join(VIBE_CONFIG_REL)).unwrap();
        assert!(text.contains("# vibe settings"));
        assert!(text.contains("theme = \"dark\"  # mine"));
        assert!(text.contains("[other]\nkey = \"value\""));
        assert!(
            text.contains("skill_paths = [\"local/skills\", \".agents/skills\"]  # keep")
                || text.contains("skill_paths = [\"local/skills\" , \".agents/skills\"]  # keep"),
            "{text}"
        );
        assert!(skill_path_configured(directory.path()));
    }

    #[test]
    fn converts_a_single_string_pointer() {
        let directory = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(VIBE_CONFIG_REL),
            "skill_paths = \"local/skills\"\nmodel = \"x\"\n",
        );
        assert!(ensure_project_skill_path(directory.path()).expect("ensure"));
        let text = std::fs::read_to_string(directory.path().join(VIBE_CONFIG_REL)).unwrap();
        assert!(text.contains("skill_paths = [\"local/skills\", \".agents/skills\"]"));
        assert!(text.contains("model = \"x\""));
    }

    #[test]
    fn refuses_a_non_string_pointer() {
        let directory = tempfile::tempdir().expect("temp");
        write(
            &directory.path().join(VIBE_CONFIG_REL),
            "skill_paths = 42\n",
        );
        let error = ensure_project_skill_path(directory.path()).expect_err("refused");
        assert_eq!(error.code(), "yaml_not_editable");
    }

    #[test]
    fn refuses_a_symlinked_config() {
        let directory = tempfile::tempdir().expect("temp");
        let target = directory.path().join("elsewhere.toml");
        std::fs::write(&target, "skill_paths = []\n").unwrap();
        std::fs::create_dir_all(directory.path().join(".vibe")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, directory.path().join(VIBE_CONFIG_REL)).unwrap();
        let error = ensure_project_skill_path(directory.path()).expect_err("refused");
        assert_eq!(error.code(), "ignore_file_unsafe");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "skill_paths = []\n"
        );
    }
}
