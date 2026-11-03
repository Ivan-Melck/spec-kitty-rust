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
//! User YAML is **deserialized, never reserialized**: the document is parsed
//! once by the declared `serde_yaml_ng` reader into an in-memory value, which
//! rewrites nothing on disk. That reader is the same YAML engine shape the
//! pinned reference uses (YAML 1.2 core scalars), so it:
//!
//! * refuses a **multi-document** stream (the reference's ruamel `load` refuses
//!   one too), instead of merging the documents;
//! * refuses a **duplicate key** inside one mapping, instead of taking the first
//!   or the last one;
//! * **resolves anchors, aliases and `<<` merge keys** instead of silently
//!   taking the absent-key default (which used to un-protect an explicit list);
//! * keeps `yes` / `no` / `on` / `off` as strings, exactly like YAML 1.2.
//!
//! Items are then coerced with Python `str()` semantics, because the reference
//! does `frozenset(str(b) for b in raw_value)`. Coercing (rather than refusing)
//! keeps `- true` / `- 1` / `- null` behaving exactly as upstream instead of
//! turning a readable list into a new refusal.
//!
//! The one resolution input this module does not read yet is a remote default
//! branch when neither the loose `refs/remotes/origin/HEAD` symref nor the
//! bounded `ls-remote --symref` seam yields one — that gap protects *fewer*
//! branches than the reference in a repository whose remote is unreachable, and
//! is recorded in `REMEDIATION-REPORT.md` rather than waived.

use std::collections::BTreeSet;
use std::path::Path;

use sk_git::{GitRepository, ProtectedBranches, RemoteName};
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
    // First probe: the loose `refs/remotes/origin/HEAD` symref, byte-identical
    // to the reference's `git symbolic-ref --quiet --short
    // refs/remotes/origin/HEAD`. A symbolic ref cannot be packed, so reading the
    // file is the same read without a Git process.
    if let Some(branch) = symbolic_remote_head(&repository) {
        return Some(branch);
    }
    // Second probe: ask the remote itself through the bounded, prompt-free seam
    // (`git ls-remote --symref <remote> HEAD`), exactly where the reference
    // falls back to `describe_remote_head`. An unreachable remote, a timeout, or
    // a remote that advertises no branch yields no augmentation: the reference
    // answers `None` there too, and this call must never turn a protection read
    // into a network error.
    if !repository
        .configured_remotes()
        .ok()?
        .iter()
        .any(|remote| remote.as_str() == "origin")
    {
        return None;
    }
    let origin = RemoteName::parse("origin").ok()?;
    repository
        .remote_default_branch(&origin)
        .ok()
        .flatten()
        .map(|branch| branch.as_str().to_owned())
}

fn symbolic_remote_head(repository: &GitRepository) -> Option<String> {
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

/// Read `.kittify/config.yaml` and answer what it says about protection.
///
/// The document is parsed by `serde_yaml_ng` and never written back, so every
/// comment, quote style and block layout on disk survives untouched.
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
    parse_protection_config(text, path)
}

fn parse_protection_config(text: &str, path: &Path) -> Result<ProtectionScan, WorkspaceError> {
    let error = |detail: String| WorkspaceError::ProtectionConfig {
        path: path.to_path_buf(),
        detail,
    };
    // One document only. `from_str` refuses a second document (the reference's
    // ruamel `load` refuses one too) and refuses a duplicate key, rather than
    // letting either shape silently choose one of the competing values.
    let mut document: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)
        .map_err(|cause| error(format!("could not parse the YAML document: {cause}")))?;
    // Fold `<<` merge keys, so an anchored/merged mapping answers with its real
    // keys instead of falling through to the absent-key default. Anchors and
    // aliases are already resolved by the parser.
    document
        .apply_merge()
        .map_err(|cause| error(format!("could not resolve a YAML merge key: {cause}")))?;

    let mapping = match document {
        // An empty document (or `---` alone) parses as null, which the reference
        // turns into `{}` before lookup. It is the absent-key arm, not an error.
        serde_yaml_ng::Value::Null => return Ok(ProtectionScan::default()),
        serde_yaml_ng::Value::Mapping(mapping) => mapping,
        other => {
            return Err(error(format!(
                "the configuration must be a YAML mapping at the top level; found {} instead",
                python_type_name(&other)
            )));
        }
    };

    let Some(protection) = mapping.get("protection") else {
        return Ok(ProtectionScan::default());
    };
    // A non-mapping `protection` value (`protection: yes`, `protection: [a]`)
    // is the reference's `not isinstance(..., dict)` arm: the defaults apply.
    let Some(protection) = protection.as_mapping() else {
        return Ok(ProtectionScan::default());
    };
    match protection.get("protected_branches") {
        // Absent key, or an explicit null: the reference treats both as absent.
        None | Some(serde_yaml_ng::Value::Null) => Ok(ProtectionScan::default()),
        Some(serde_yaml_ng::Value::Sequence(items)) => Ok(ProtectionScan {
            // `frozenset(str(b) for b in raw_value)`: Python `str()` coercion,
            // so `- true` is the branch name "True" and `- 1` is "1".
            explicit: Some(items.iter().map(python_str).collect()),
        }),
        Some(other) => Err(error(format!(
            "protection.protected_branches in .kittify/config.yaml must be a list, got {}: {}",
            python_type_name(other),
            python_repr(other)
        ))),
    }
}

/// The Python type name `type(value).__name__` reports.
fn python_type_name(value: &serde_yaml_ng::Value) -> &'static str {
    match value {
        serde_yaml_ng::Value::Null => "NoneType",
        serde_yaml_ng::Value::Bool(_) => "bool",
        serde_yaml_ng::Value::Number(number) if number.is_f64() => "float",
        serde_yaml_ng::Value::Number(_) => "int",
        serde_yaml_ng::Value::String(_) => "str",
        serde_yaml_ng::Value::Sequence(_) => "list",
        serde_yaml_ng::Value::Mapping(_) => "dict",
        serde_yaml_ng::Value::Tagged(_) => "Tagged",
    }
}

/// Python `str(value)`, the coercion the reference applies to every list item.
fn python_str(value: &serde_yaml_ng::Value) -> String {
    match value {
        serde_yaml_ng::Value::Null => "None".to_owned(),
        serde_yaml_ng::Value::Bool(true) => "True".to_owned(),
        serde_yaml_ng::Value::Bool(false) => "False".to_owned(),
        serde_yaml_ng::Value::Number(number) => python_number(number),
        serde_yaml_ng::Value::String(text) => text.clone(),
        serde_yaml_ng::Value::Sequence(items) => {
            let rendered: Vec<String> = items.iter().map(python_repr).collect();
            format!("[{}]", rendered.join(", "))
        }
        serde_yaml_ng::Value::Mapping(mapping) => {
            let rendered: Vec<String> = mapping
                .iter()
                .map(|(key, item)| format!("{}: {}", python_repr(key), python_repr(item)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
        serde_yaml_ng::Value::Tagged(tagged) => python_str(&tagged.value),
    }
}

/// Python `repr(value)` for the containers `str()` renders with `repr()`.
fn python_repr(value: &serde_yaml_ng::Value) -> String {
    match value {
        serde_yaml_ng::Value::String(text) => python_string_repr(text),
        other => python_str(other),
    }
}

fn python_string_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character == quote => {
                out.push('\\');
                out.push(character);
            }
            other => out.push(other),
        }
    }
    out.push(quote);
    out
}

/// Python `str()` of a YAML scalar number.
fn python_number(number: &serde_yaml_ng::Number) -> String {
    if let Some(int) = number.as_i64() {
        return int.to_string();
    }
    if let Some(int) = number.as_u64() {
        return int.to_string();
    }
    let value = number.as_f64().unwrap_or(f64::NAN);
    // Python renders an integral float with a trailing `.0` and writes the
    // exponent sign; keep both shapes so `- 1.0` is the branch name "1.0".
    let rendered = value.to_string();
    if !rendered.contains(['e', 'E']) && !rendered.contains('.') {
        return format!("{rendered}.0");
    }
    match rendered.split_once(['e', 'E']) {
        Some((mantissa, exponent)) if !exponent.starts_with(['+', '-']) => {
            format!("{mantissa}e+{exponent}")
        }
        _ => rendered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str) -> ProtectionScan {
        parse_protection_config(text, Path::new(CONFIG_REL_PATH)).expect("parse")
    }

    fn refusal(text: &str) -> WorkspaceError {
        parse_protection_config(text, Path::new(CONFIG_REL_PATH)).expect_err("refusal")
    }

    fn explicit(text: &str) -> Vec<String> {
        scan(text).explicit.expect("explicit list")
    }

    #[test]
    fn explicit_block_sequence_replaces_the_defaults() {
        assert_eq!(
            explicit("protection:\n  protected_branches:\n    - release\n    - trunk\n"),
            vec!["release".to_owned(), "trunk".to_owned()]
        );
    }

    #[test]
    fn empty_flow_sequence_unprotects_everything() {
        assert_eq!(
            explicit("protection:\n  protected_branches: []\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn flow_sequence_and_quotes_are_read() {
        assert_eq!(
            explicit("protection:\n  protected_branches: ['release', \"trunk/x\", main]  # note\n"),
            vec![
                "release".to_owned(),
                "trunk/x".to_owned(),
                "main".to_owned()
            ]
        );
    }

    #[test]
    fn flow_mapping_is_read() {
        assert_eq!(
            explicit("protection: {protected_branches: [release]}\n"),
            vec!["release".to_owned()]
        );
    }

    #[test]
    fn absent_block_or_key_uses_the_default_arm() {
        assert_eq!(scan("merge:\n  strategy: squash\n").explicit, None);
        assert_eq!(scan("protection:\n  other: 1\n").explicit, None);
        assert_eq!(scan("protection:\n").explicit, None);
        assert_eq!(scan("").explicit, None);
        assert_eq!(scan("# only a comment\n---\n").explicit, None);
        assert_eq!(
            scan("protection:\n  protected_branches: null\n").explicit,
            None
        );
    }

    #[test]
    fn a_non_mapping_protection_block_uses_the_default_arm() {
        // YAML 1.2 keeps `yes` a string, exactly like the reference's ruamel
        // loader; a non-mapping `protection` value is the absent-key arm.
        assert_eq!(scan("protection: yes\n").explicit, None);
        assert_eq!(scan("protection:\n- main\n").explicit, None);
    }

    #[test]
    fn an_anchored_mapping_keeps_its_explicit_list() {
        // The narrow scanner used to treat the anchor as "not a mapping" and
        // silently protect only {main, master}: a fail-open on the one thing
        // this module computes.
        assert_eq!(
            explicit("protection: &p\n  protected_branches: [release]\n"),
            vec!["release".to_owned()]
        );
    }

    #[test]
    fn an_alias_to_a_mapping_keeps_its_explicit_list() {
        assert_eq!(
            explicit("base: &b\n  protected_branches: [release]\nprotection: *b\n"),
            vec!["release".to_owned()]
        );
    }

    #[test]
    fn a_merge_key_keeps_its_explicit_list() {
        assert_eq!(
            explicit("shared: &s\n  protected_branches: [release]\nprotection:\n  <<: *s\n"),
            vec!["release".to_owned()]
        );
    }

    #[test]
    fn python_str_coercion_is_applied_to_each_item() {
        assert_eq!(
            explicit("protection:\n  protected_branches: [master, yes, 1, null, 1.0]\n"),
            vec![
                "master".to_owned(),
                "yes".to_owned(),
                "1".to_owned(),
                "None".to_owned(),
                "1.0".to_owned()
            ]
        );
    }

    #[test]
    fn a_multi_document_stream_is_refused() {
        // The reference's ruamel `load` refuses a multi-document stream; the
        // narrow scanner merged the documents instead.
        let error = refusal("protection:\n  protected_branches: [release]\n---\nother: 1\n");
        assert!(
            matches!(error, WorkspaceError::ProtectionConfig { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_duplicate_key_is_refused() {
        let error = refusal(
            "protection:\n  protected_branches: [release]\n  protected_branches: [trunk]\n",
        );
        assert!(
            matches!(error, WorkspaceError::ProtectionConfig { .. }),
            "{error}"
        );
        let error =
            refusal("protection:\n  protected_branches: [release]\nprotection:\n  other: 1\n");
        assert!(
            matches!(error, WorkspaceError::ProtectionConfig { .. }),
            "{error}"
        );
    }

    #[test]
    fn a_non_mapping_top_level_and_a_non_list_value_are_typed_refusals() {
        for text in [
            "just-a-scalar\n",
            "- release\n",
            "protection:\n  protected_branches: release\n",
            "protection:\n  protected_branches:\n    release: yes\n",
        ] {
            let error = refusal(text);
            assert!(
                matches!(error, WorkspaceError::ProtectionConfig { .. }),
                "{text:?}: {error}"
            );
        }
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
