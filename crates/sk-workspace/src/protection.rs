//! Live protected-branch policy resolution.
//!
//! `sk-git` deliberately treats the protected set as an application fact and
//! defaults it to **empty**. That default is a library boundary, not a policy:
//! a caller that commits through the adapter without resolving this module
//! protects nothing. This module is the application owner of that fact.
//!
//! # Resolution table (mirrors `specify_cli/git/protection_policy.py`)
//!
//! | `.kittify/config.yaml` state                  | resolved set                    |
//! |----------------------------------------------|---------------------------------|
//! | no `protection:` block (or a non-mapping one) | `{main, master}` ∪ remote default |
//! | `protection.protected_branches: [a, b]`       | exactly `{a, b}`                |
//! | `protection.protected_branches: []`           | empty — nothing protected       |
//! | `protected_branches` is not a sequence        | typed config error (fail closed) |
//! | top level is not a mapping                    | typed config error (fail closed) |
//!
//! The remote default branch augments **only** the absent-key arm, exactly like
//! upstream: once an operator writes an explicit list, that list is the policy.
//!
//! # Mission-scoped hatch
//!
//! Two hatches un-protect a branch, and neither is ever inferred from message
//! text or file content:
//!
//! * the operator environment hatch
//!   `SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS` (`1` / `true` / `yes`);
//! * a `single_branch` mission's own `meta.json` `commit_to_target: true`,
//!   which un-protects **exactly that mission's `target_branch`**. A
//!   non-boolean value, an unstored/non-`single_branch` topology, or a missing
//!   target grants no bypass (fail closed).
//!
//! # Configuration reading
//!
//! `sk-workspace` does not depend on a YAML serializer, and user YAML must never
//! be re-serialized merely to inspect it. [`scan_protection_block`] is therefore
//! a narrow, read-only scanner for the `protection.protected_branches` mapping
//! and its block/flow sequence. Anything outside that shape (anchors, tags,
//! multi-document streams, or a `protected_branches` value that is a mapping)
//! is a typed refusal rather than a guessed answer. See
//! `crates/sk-workspace/COORDINATION-REPORT.md` for the proposed shared-reader
//! integration.

use std::collections::BTreeSet;
use std::path::Path;

use sk_git::{GitRepository, ProtectedBranches};
use sk_mission::{Mission, MissionTopology, list_missions, select_mission};

use crate::error::WorkspaceError;

/// Repository-relative path of the project configuration.
pub const CONFIG_REL_PATH: &str = ".kittify/config.yaml";

/// The one operator escape hatch.
pub const PROTECTION_HATCH_ENV: &str = "SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS";

/// The name defaults added when the operator configured no explicit list.
pub const DEFAULT_PROTECTED_BRANCHES: [&str; 2] = ["main", "master"];

/// Largest configuration document this module will read.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// A boundary-resolved protection policy.
///
/// Construction is the only place that touches the filesystem, Git, or the
/// environment; every decision afterwards is pure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectionPolicy {
    protected: BTreeSet<String>,
    operator_hatch_active: bool,
    mission_bypass_branch: Option<String>,
}

impl ProtectionPolicy {
    /// Resolve the policy for `repo_root` using the process environment hatch.
    pub fn resolve(repo_root: &Path) -> Result<Self, WorkspaceError> {
        let hatch = std::env::var(PROTECTION_HATCH_ENV).ok();
        Self::resolve_with_hatch(repo_root, hatch.as_deref())
    }

    /// Resolve the policy with an explicit hatch value.
    ///
    /// `hatch` is the raw environment value (`None` when unset). Only `1`,
    /// `true`, and `yes` are truthy, case-insensitively, matching upstream's
    /// `.lower() in (...)` check (no trimming).
    pub fn resolve_with_hatch(
        repo_root: &Path,
        hatch: Option<&str>,
    ) -> Result<Self, WorkspaceError> {
        let config_path = repo_root.join(CONFIG_REL_PATH);
        let scan = read_protection_config(&config_path)?;
        let protected = match scan.explicit {
            Some(branches) => branches.into_iter().collect(),
            None => default_branches_with_remote(repo_root),
        };
        Ok(Self {
            protected,
            operator_hatch_active: hatch.is_some_and(is_truthy_hatch),
            mission_bypass_branch: None,
        })
    }

    /// Resolve the policy and fold `mission_selector`'s `commit_to_target`.
    ///
    /// An unknown, ambiguous, or unreadable mission grants no bypass; the plain
    /// policy (fully protected) is returned.
    pub fn resolve_for_mission(
        repo_root: &Path,
        mission_selector: &str,
    ) -> Result<Self, WorkspaceError> {
        Self::resolve(repo_root)?.for_mission(repo_root, mission_selector)
    }

    /// Fold an already-loaded mission's `commit_to_target` into this policy.
    ///
    /// This is the pure fold; call [`ProtectionPolicy::for_mission`] when only a
    /// selector is available.
    pub fn scoped_to_mission(&self, mission: &Mission) -> Self {
        let Some(bypass) = mission_bypass_branch(mission) else {
            return self.clone();
        };
        let mut scoped = self.clone();
        scoped.mission_bypass_branch = Some(bypass);
        scoped
    }

    /// Fold the persisted `commit_to_target` of `mission_selector`.
    pub fn for_mission(
        &self,
        repo_root: &Path,
        mission_selector: &str,
    ) -> Result<Self, WorkspaceError> {
        match load_mission(repo_root, mission_selector) {
            Some(mission) => Ok(self.scoped_to_mission(&mission)),
            None => Ok(self.clone()),
        }
    }

    /// Is `branch` protected (a short branch name, not `refs/heads/...`)?
    ///
    /// The mission hatch applies before the operator hatch so a mission's own
    /// opt-out is observable in the answer even while the operator hatch is on.
    pub fn is_protected(&self, branch: &str) -> bool {
        if self.mission_bypass_branch.as_deref() == Some(branch) {
            return false;
        }
        self.protected.contains(branch) && !self.operator_hatch_active
    }

    /// Is `branch` the repository's primary branch, or configured protected?
    ///
    /// `primary_branch` is an explicit keyword argument upstream precisely so a
    /// caller cannot silently omit it: a `single_branch` mission's target is
    /// protected when it *is* the primary branch even if the configured list
    /// replaced the name defaults.
    pub fn is_protected_target(&self, branch: &str, primary_branch: &str) -> bool {
        if self.operator_hatch_active || self.mission_bypass_branch.as_deref() == Some(branch) {
            return false;
        }
        branch == primary_branch || self.is_protected(branch)
    }

    /// The resolved protected names, for diagnostics.
    pub fn protected_branches(&self) -> &BTreeSet<String> {
        &self.protected
    }

    /// Is the operator environment hatch in effect?
    pub fn operator_hatch_active(&self) -> bool {
        self.operator_hatch_active
    }

    /// The mission-scoped bypass branch, when one is folded in.
    pub fn mission_bypass_branch(&self) -> Option<&str> {
        self.mission_bypass_branch.as_deref()
    }

    /// The protected set as the Git adapter's fact type.
    ///
    /// The hatch and the mission bypass are applied here too: they are policy,
    /// and the adapter must not re-derive them. An active hatch yields an empty
    /// set, and the mission's own bypass branch is removed from the set.
    pub fn branch_set(&self) -> Result<ProtectedBranches, WorkspaceError> {
        if self.operator_hatch_active {
            return Ok(ProtectedBranches::default());
        }
        let names: Vec<&str> = self
            .protected
            .iter()
            .map(String::as_str)
            .filter(|name| Some(*name) != self.mission_bypass_branch.as_deref())
            .collect();
        Ok(ProtectedBranches::parse(names)?)
    }
}

fn is_truthy_hatch(value: &str) -> bool {
    matches!(value.to_lowercase().as_str(), "1" | "true" | "yes")
}

/// The mission's `commit_to_target` bypass branch, if it grants one.
///
/// `commit_to_target` is a `single_branch`-only opt-out: a hand-edited `meta`
/// on any other, or unstored, topology never un-protects the target.
fn mission_bypass_branch(mission: &Mission) -> Option<String> {
    if mission.topology() != Some(MissionTopology::SingleBranch) {
        return None;
    }
    match mission.meta_field("commit_to_target") {
        Some(serde_json::Value::Bool(true)) => {}
        _ => return None,
    }
    match mission.meta_field("target_branch") {
        Some(serde_json::Value::String(target)) if !target.trim().is_empty() => {
            Some(target.trim().to_owned())
        }
        _ => None,
    }
}

/// Load `mission_selector` from the primary checkout, or `None` when the
/// selector cannot be resolved. Selection failure is not an error: it simply
/// grants no bypass.
fn load_mission(repo_root: &Path, mission_selector: &str) -> Option<Mission> {
    let missions = list_missions(repo_root).ok()?;
    select_mission(&missions, mission_selector).ok().cloned()
}

/// `{main, master}` augmented with the remote default branch.
///
/// The remote default is read from the local `refs/remotes/origin/HEAD` symbolic
/// ref, which is the same first probe upstream makes. A missing or unreadable
/// symbolic ref yields no augmentation (upstream then asks the remote; this
/// crate has no `ls-remote --symref` read yet — see the report).
fn default_branches_with_remote(repo_root: &Path) -> BTreeSet<String> {
    let mut branches: BTreeSet<String> = DEFAULT_PROTECTED_BRANCHES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    if let Some(remote_default) = remote_default_branch(repo_root) {
        branches.insert(remote_default);
    }
    branches
}

fn remote_default_branch(repo_root: &Path) -> Option<String> {
    let repository = GitRepository::open(repo_root).ok()?;
    let common_dir = repository.git_dirs().ok()?.common_dir().to_path_buf();
    let path = common_dir.join("refs/remotes/origin/HEAD");
    let bytes = sk_kernel::read_bounded(&path, 4096).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?.trim();
    let target = text.strip_prefix("ref:")?.trim();
    // `refs/remotes/<remote>/<branch>`; a branch name may itself contain `/`,
    // so only the remote segment is stripped.
    let rest = target.strip_prefix("refs/remotes/")?;
    let (_remote, branch) = rest.split_once('/')?;
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_owned())
    }
}

/// What the configuration says about `protection.protected_branches`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ProtectionScan {
    /// `None` means "no explicit list": the absent-key default arm applies.
    explicit: Option<Vec<String>>,
}

fn read_protection_config(path: &Path) -> Result<ProtectionScan, WorkspaceError> {
    let bytes = match sk_kernel::read_bounded(path, MAX_CONFIG_BYTES) {
        Ok(bytes) => bytes,
        Err(sk_kernel::KernelError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(ProtectionScan::default());
        }
        Err(error) => return Err(WorkspaceError::Kernel(error)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| WorkspaceError::ProtectionConfig {
        path: path.to_path_buf(),
        detail: "configuration is not valid UTF-8".to_owned(),
    })?;
    scan_protection_block(text, path)
}

/// Scan the `protection` mapping for an explicit `protected_branches` list.
///
/// Read-only and non-reserializing: the document is inspected line by line and
/// never written back.
fn scan_protection_block(text: &str, path: &Path) -> Result<ProtectionScan, WorkspaceError> {
    let error = |detail: String| WorkspaceError::ProtectionConfig {
        path: path.to_path_buf(),
        detail,
    };

    // (indent, content-without-indent) for every meaningful line.
    let mut lines: Vec<(usize, &str)> = Vec::new();
    for raw in text.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let content = line.trim_start();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let marker = content.trim_end();
        if marker == "---" || marker == "..." {
            continue;
        }
        let indent = line.len() - content.len();
        lines.push((indent, content));
    }
    // A document that *starts* with a block-sequence item is a sequence, not a
    // mapping. A `- item` line at indentation 0 later in the document is the
    // value of the preceding key (YAML allows a block sequence at its parent
    // key's indentation), so it is not a second top-level key.
    if let Some((_, first)) = lines.first()
        && first.starts_with('-')
    {
        return Err(error(
            "the top level is a YAML sequence, not a mapping".to_owned(),
        ));
    }
    for (indent, content) in &lines {
        if *indent == 0 && !content.starts_with('-') && split_key(content).is_none() {
            return Err(error(format!(
                "the top level is not a YAML mapping (line {content:?})"
            )));
        }
    }

    let Some(index) = lines.iter().position(|(indent, content)| {
        *indent == 0 && split_key(content).is_some_and(|(key, _)| key == "protection")
    }) else {
        return Ok(ProtectionScan::default());
    };
    let inline = split_key(lines[index].1)
        .map(|(_, value)| value)
        .unwrap_or("");

    // Collect the `protection:` block: following meaningful lines indented
    // deeper than the key.
    let mut block: Vec<(usize, &str)> = Vec::new();
    for (indent, content) in lines.iter().skip(index + 1) {
        let is_item = content.starts_with('-');
        if *indent == 0 && !is_item {
            break;
        }
        block.push((*indent, content));
    }

    let inline = strip_comment(inline).trim();
    if !inline.is_empty() {
        if inline == "{}" {
            return Ok(ProtectionScan::default());
        }
        let Some(fields) = parse_flow_mapping(inline) else {
            // A non-mapping value (`protection: yes`, `protection: [a]`) is the
            // absent-key arm upstream, not an error.
            return Ok(ProtectionScan::default());
        };
        let Some((_, value)) = fields
            .into_iter()
            .find(|(key, _)| key == "protected_branches")
        else {
            return Ok(ProtectionScan::default());
        };
        return explicit_from_inline(&value, path);
    }

    // Block mapping. Sequence-only blocks (`protection:` then `- a`) are not a
    // mapping, matching upstream's `not isinstance(..., dict)` arm.
    let child_indent = block
        .iter()
        .filter(|(_, content)| !content.starts_with('-'))
        .map(|(indent, _)| *indent)
        .min();
    let Some(child_indent) = child_indent else {
        return Ok(ProtectionScan::default());
    };
    let Some(position) = block.iter().position(|(indent, content)| {
        *indent == child_indent
            && split_key(content).is_some_and(|(key, _)| key == "protected_branches")
    }) else {
        return Ok(ProtectionScan::default());
    };
    let (_, content) = block[position];
    let value = strip_comment(split_key(content).map(|(_, value)| value).unwrap_or("")).trim();
    if !value.is_empty() {
        return explicit_from_inline(value, path);
    }

    // Block sequence: deeper lines that begin with `-`.
    let mut items = Vec::new();
    for (indent, content) in block.iter().skip(position + 1) {
        // Sequence items may sit at the key's own indentation (`key:` then
        // `- item`); a non-item line at that indentation starts the next key.
        if *indent < child_indent || (*indent == child_indent && !content.starts_with('-')) {
            break;
        }
        let Some(item) = content.strip_prefix('-') else {
            return Err(error(
                "protection.protected_branches is a mapping, not a sequence".to_owned(),
            ));
        };
        let item = item.trim();
        if item.is_empty() {
            return Err(error(
                "protection.protected_branches contains an empty sequence entry".to_owned(),
            ));
        }
        if split_key(item).is_some() {
            return Err(error(
                "protection.protected_branches is a mapping, not a sequence".to_owned(),
            ));
        }
        items.push(parse_scalar(item, path)?);
    }
    Ok(ProtectionScan {
        explicit: Some(items),
    })
}

fn explicit_from_inline(value: &str, path: &Path) -> Result<ProtectionScan, WorkspaceError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(ProtectionScan::default());
    }
    let Some(list) = parse_flow_sequence(value) else {
        return Err(WorkspaceError::ProtectionConfig {
            path: path.to_path_buf(),
            detail: format!("protection.protected_branches must be a sequence, observed {value:?}"),
        });
    };
    let mut items = Vec::with_capacity(list.len());
    for entry in list {
        items.push(parse_scalar(&entry, path)?);
    }
    Ok(ProtectionScan {
        explicit: Some(items),
    })
}

fn parse_scalar(raw: &str, path: &Path) -> Result<String, WorkspaceError> {
    let raw = raw.trim();
    let bad = || WorkspaceError::ProtectionConfig {
        path: path.to_path_buf(),
        detail: format!("unsupported scalar in protection.protected_branches: {raw:?}"),
    };
    if let Some(inner) = raw
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Ok(inner.replace("''", "'"));
    }
    if let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(character) = chars.next() {
            if character != '\\' {
                out.push(character);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                _ => return Err(bad()),
            }
        }
        return Ok(out);
    }
    if raw.starts_with('[') || raw.starts_with('{') || raw.contains(':') {
        return Err(bad());
    }
    Ok(strip_comment(raw).trim().to_owned())
}

/// Split `key: value` outside quotes; returns the unquoted key and raw value.
fn split_key(content: &str) -> Option<(&str, &str)> {
    let bytes = content.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(mark) if byte == mark => quote = None,
            Some(_) => {}
            None if byte == b'\'' || byte == b'"' => quote = Some(byte),
            None if byte == b'#' && (index == 0 || bytes[index - 1].is_ascii_whitespace()) => {
                return None;
            }
            // A mapping colon is followed by whitespace or end of line.
            None if byte == b':'
                && (index + 1 == bytes.len() || bytes[index + 1].is_ascii_whitespace()) =>
            {
                let key = content[..index].trim();
                let key = unquote_key(key)?;
                return Some((key, content[index + 1..].trim()));
            }
            None => {}
        }
        index += 1;
    }
    None
}

fn unquote_key(key: &str) -> Option<&str> {
    if let Some(inner) = key
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Some(inner);
    }
    let inner = key
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(key);
    if inner.is_empty() { None } else { Some(inner) }
}

/// Remove an unquoted trailing comment.
fn strip_comment(content: &str) -> &str {
    let bytes = content.as_bytes();
    let mut quote: Option<u8> = None;
    for (index, byte) in bytes.iter().enumerate() {
        match quote {
            Some(mark) if *byte == mark => quote = None,
            Some(_) => {}
            None if *byte == b'\'' || *byte == b'"' => quote = Some(*byte),
            None if *byte == b'#' && (index == 0 || bytes[index - 1].is_ascii_whitespace()) => {
                return &content[..index];
            }
            None => {}
        }
    }
    content
}

/// Parse a single-line flow mapping `{a: b, c: [d, e]}` into raw entries.
fn parse_flow_mapping(value: &str) -> Option<Vec<(String, String)>> {
    let inner = value.strip_prefix('{')?.strip_suffix('}')?;
    let mut entries = Vec::new();
    for entry in split_top_level(inner, ',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key, item) = split_key(entry)?;
        entries.push((key.to_owned(), item.trim().to_owned()));
    }
    Some(entries)
}

/// Parse a single-line flow sequence `[a, b]` into raw entries.
fn parse_flow_sequence(value: &str) -> Option<Vec<String>> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?;
    let mut items: Vec<String> = Vec::new();
    for entry in split_top_level(inner, ',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        items.push(entry.to_owned());
    }
    Some(items)
}

/// Split on `separator` at nesting depth zero, outside quotes.
fn split_top_level(content: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for character in content.chars() {
        match quote {
            Some(mark) => {
                current.push(character);
                if character == mark {
                    quote = None;
                }
            }
            None => match character {
                '\'' | '"' => {
                    quote = Some(character);
                    current.push(character);
                }
                '[' | '{' => {
                    depth += 1;
                    current.push(character);
                }
                ']' | '}' => {
                    depth = depth.saturating_sub(1);
                    current.push(character);
                }
                character if character == separator && depth == 0 => {
                    parts.push(std::mem::take(&mut current));
                }
                other => current.push(other),
            },
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str) -> ProtectionScan {
        scan_protection_block(text, Path::new(CONFIG_REL_PATH)).expect("parse")
    }

    #[test]
    fn explicit_block_sequence_replaces_the_defaults() {
        let scan = scan("protection:\n  protected_branches:\n    - release\n    - trunk\n");
        assert_eq!(
            scan.explicit,
            Some(vec!["release".to_owned(), "trunk".to_owned()])
        );
    }

    #[test]
    fn empty_flow_sequence_unprotects_everything() {
        let scan = scan("protection:\n  protected_branches: []\n");
        assert_eq!(scan.explicit, Some(Vec::new()));
    }

    #[test]
    fn flow_sequence_and_quotes_are_read() {
        let scan =
            scan("protection:\n  protected_branches: ['release', \"trunk/x\", main]  # note\n");
        assert_eq!(
            scan.explicit,
            Some(vec![
                "release".to_owned(),
                "trunk/x".to_owned(),
                "main".to_owned()
            ])
        );
    }

    #[test]
    fn flow_mapping_is_read() {
        let scan = scan("protection: {protected_branches: [release]}\n");
        assert_eq!(scan.explicit, Some(vec!["release".to_owned()]));
    }

    #[test]
    fn absent_block_or_key_uses_the_default_arm() {
        assert_eq!(scan("merge:\n  strategy: squash\n").explicit, None);
        assert_eq!(scan("protection:\n  other: 1\n").explicit, None);
        assert_eq!(scan("protection:\n").explicit, None);
    }

    #[test]
    fn a_non_mapping_protection_block_uses_the_default_arm() {
        assert_eq!(scan("protection: yes\n").explicit, None);
        assert_eq!(scan("protection:\n- main\n").explicit, None);
    }

    #[test]
    fn malformed_shapes_fail_closed() {
        assert!(scan_protection_block("-\n", Path::new(CONFIG_REL_PATH)).is_err());
        assert!(scan_protection_block("just-a-scalar\n", Path::new(CONFIG_REL_PATH)).is_err());
        assert!(
            scan_protection_block(
                "protection:\n  protected_branches: release\n",
                Path::new(CONFIG_REL_PATH)
            )
            .is_err()
        );
        assert!(
            scan_protection_block(
                "protection:\n  protected_branches:\n    release: yes\n",
                Path::new(CONFIG_REL_PATH)
            )
            .is_err()
        );
    }

    #[test]
    fn comments_and_document_markers_are_ignored() {
        let scan = scan(
            "# config\n---\nprotection:\n  # keep this comment\n  protected_branches:\n    - release\n",
        );
        assert_eq!(scan.explicit, Some(vec!["release".to_owned()]));
    }

    #[test]
    fn only_truthy_hatch_values_activate() {
        for value in ["1", "true", "TRUE", "yes", "Yes"] {
            assert!(is_truthy_hatch(value), "{value}");
        }
        for value in ["", "0", "no", "on", " true "] {
            assert!(!is_truthy_hatch(value), "{value:?}");
        }
    }
}
