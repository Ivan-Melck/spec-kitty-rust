//! Validated branch and ref names, and safe start points.
//!
//! Every name this crate passes to git has already been checked against the
//! structural rules of `git check-ref-format`, plus one extra rule: a leading
//! `-` is always refused. Combined with argv-only invocation, that closes ref
//! *option* injection (`git branch --end-of-options` is still used where git
//! supports it) as well as revspec injection (`@{`, `..`, `^`, `:`, `~`).
//! Syntax validation invents no total-length cap: Git and the host filesystem
//! own operational ref limits, which are reported by the existing process seam.

use std::fmt;

use crate::error::GitError;
use crate::oid::Oid;

/// A short local branch name (`main`, `feature/topic`).
///
/// This is **not** a fully-qualified ref. Use [`BranchName::as_ref_name`] to get
/// `refs/heads/<name>`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BranchName(String);

impl BranchName {
    /// Validate a short branch name.
    pub fn parse(value: &str) -> Result<Self, GitError> {
        validate(value, "branch name")?;
        if value.starts_with("refs/") {
            return Err(invalid("branch name", value, "must not start with `refs/`"));
        }
        if value == "HEAD" {
            return Err(invalid("branch name", value, "`HEAD` is not a branch name"));
        }
        Ok(Self(value.to_string()))
    }

    /// The name as git spells it short.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The fully-qualified local branch ref.
    pub fn as_ref_name(&self) -> RefName {
        // A validated short branch name can never make an invalid
        // `refs/heads/` ref: the one forbidden prefix is already refused.
        RefName(format!("refs/heads/{}", self.0))
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for BranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "BranchName({:?})", self.0)
    }
}

impl std::str::FromStr for BranchName {
    type Err = GitError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// A fully-qualified ref name (`refs/heads/main`, `refs/x`,
/// `refs/spec-kitty/lane-tip/x`).
///
/// "Fully qualified" means exactly what git means by it: the name starts with
/// `refs/`. Git also permits a one-component namespace below `refs/`
/// (`refs/x`), which `git check-ref-format refs/x` accepts. An earlier version
/// of this adapter invented a `refs/<namespace>/<name>` minimum, which rejected
/// that Git-legal shape; the invented rule was removed against an actual
/// `git check-ref-format` control. Every structural, NUL, control-character,
/// option and revspec rule below is unchanged, and there is still no invented
/// total-length cap.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefName(String);

impl RefName {
    /// Validate a fully-qualified ref name.
    ///
    /// The answer is git's own structural rule set plus argv-safety: a `refs/`
    /// prefix, no `..`, no `@{`, no empty path component, no component starting
    /// with `.`, no `.lock` suffix, no space or `~^:?*[\\`, no control byte, and
    /// no leading `-`. A one-level namespace (`refs/x`) is legal.
    pub fn parse(value: &str) -> Result<Self, GitError> {
        validate(value, "ref name")?;
        if !value.starts_with("refs/") {
            return Err(invalid("ref name", value, "must start with `refs/`"));
        }
        Ok(Self(value.to_string()))
    }

    /// The fully-qualified ref for a local branch.
    pub fn branch(branch: &BranchName) -> Self {
        branch.as_ref_name()
    }

    /// The remote-tracking ref `refs/remotes/<remote>/<branch>`.
    pub fn remote_tracking(remote: &crate::remote::RemoteName, branch: &BranchName) -> Self {
        // A validated remote name cannot introduce an invalid ref component;
        // remote names exclude `/`, `:`, `@`, whitespace and leading `-`.
        Self(format!(
            "refs/remotes/{}/{}",
            remote.as_str(),
            branch.as_str()
        ))
    }

    /// The full name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this ref lives under `refs/heads/`.
    pub fn is_branch(&self) -> bool {
        self.0.starts_with("refs/heads/")
    }

    /// The short branch name when this is a `refs/heads/` ref.
    pub fn short_branch(&self) -> Option<BranchName> {
        let short = self.0.strip_prefix("refs/heads/")?;
        BranchName::parse(short).ok()
    }

    /// Whether this ref is inside the given namespace prefix (`refs/spec-kitty/`).
    pub fn is_under(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix)
    }
}

impl fmt::Display for RefName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for RefName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RefName({:?})", self.0)
    }
}

impl std::str::FromStr for RefName {
    type Err = GitError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// A commit a new branch or worktree may start from.
///
/// There is no arbitrary-revision variant: a caller names a validated ref, a
/// full object id, or `HEAD`. `HEAD` is resolved to an object id while the plan
/// is prepared, so a later checkout cannot silently change what the plan means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartPoint {
    /// The current `HEAD` commit.
    Head,
    /// Exactly this object id.
    Oid(Oid),
    /// A fully-qualified ref, peeled to a commit.
    Ref(RefName),
}

impl StartPoint {
    /// The operand passed to git.
    pub(crate) fn operand(&self) -> &str {
        match self {
            Self::Head => "HEAD",
            Self::Oid(oid) => oid.as_str(),
            Self::Ref(reference) => reference.as_str(),
        }
    }
}

fn invalid(kind: &'static str, name: &str, detail: &'static str) -> GitError {
    GitError::InvalidRefName {
        kind,
        name: name.to_string(),
        detail,
    }
}

fn validate(value: &str, kind: &'static str) -> Result<(), GitError> {
    if value.is_empty() {
        return Err(invalid(kind, value, "is empty"));
    }
    if value.starts_with('-') {
        return Err(invalid(
            kind,
            value,
            "starts with `-` and could be read as an option",
        ));
    }
    if value.starts_with('/') || value.ends_with('/') {
        return Err(invalid(kind, value, "starts or ends with `/`"));
    }
    if value.ends_with('.') {
        return Err(invalid(kind, value, "ends with `.`"));
    }
    if value.contains("..") {
        return Err(invalid(kind, value, "contains `..`"));
    }
    if value.contains("@{") {
        return Err(invalid(kind, value, "contains `@{`"));
    }
    if value.contains("//") {
        return Err(invalid(kind, value, "contains an empty path component"));
    }
    if value == "@" {
        return Err(invalid(kind, value, "`@` is a synonym for `HEAD`"));
    }
    for byte in value.bytes() {
        if byte < 0x20 || byte == 0x7f {
            return Err(invalid(kind, value, "contains a control character"));
        }
        if matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\') {
            return Err(invalid(
                kind,
                value,
                "contains a character git forbids in ref names",
            ));
        }
    }
    for component in value.split('/') {
        if component.is_empty() {
            return Err(invalid(kind, value, "contains an empty path component"));
        }
        if component == "." || component == ".." || component.starts_with('.') {
            return Err(invalid(
                kind,
                value,
                "contains a component beginning with `.`",
            ));
        }
        if component.ends_with(".lock") {
            return Err(invalid(kind, value, "a component ends with `.lock`"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_real_branch_names() {
        for name in ["main", "feature/topic", "release/1.2", "user.name/part-1"] {
            assert!(BranchName::parse(name).is_ok(), "{name}");
        }
        assert_eq!(
            BranchName::parse("feature/topic")
                .unwrap()
                .as_ref_name()
                .as_str(),
            "refs/heads/feature/topic"
        );
    }

    #[test]
    fn nested_ref_syntax_has_no_invented_total_length_limit() {
        let branch = vec!["x".repeat(219); 5].join("/");
        assert_eq!(branch.len(), 1099);
        let parsed = BranchName::parse(&branch).expect("Git syntax permits long nested names");
        let full = format!("refs/heads/{branch}");
        assert_eq!(parsed.as_ref_name().as_str(), full);
        assert_eq!(
            RefName::parse(&full).unwrap().short_branch().unwrap(),
            parsed
        );
    }

    #[test]
    fn refuses_option_and_revspec_shaped_branch_names() {
        for name in [
            "-f",
            "--force",
            "HEAD",
            "refs/heads/main",
            "a..b",
            "a@{1}",
            "a b",
            "a~1",
            "a^",
            "a:b",
            "a?",
            "a*",
            "a[",
            "a\\b",
            "",
            "/main",
            "main/",
            "main/.hidden",
            "main.lock",
            "a//b",
            "@",
            "main\n",
        ] {
            assert!(
                matches!(
                    BranchName::parse(name),
                    Err(GitError::InvalidRefName { .. })
                ),
                "{name:?} should be refused"
            );
        }
    }

    #[test]
    fn fully_qualified_refs_need_the_refs_prefix_only() {
        assert!(RefName::parse("refs/heads/main").unwrap().is_branch());
        assert_eq!(
            RefName::parse("refs/heads/feature/x")
                .unwrap()
                .short_branch()
                .unwrap()
                .as_str(),
            "feature/x"
        );
        for name in ["refs/", "heads/main", "main", "HEAD", "-"] {
            assert!(RefName::parse(name).is_err(), "{name:?}");
        }
        // Git's own rule is "one or more `/` unless `--allow-onelevel`"; a
        // one-component namespace below `refs/` is legal (`check-ref-format
        // refs/x` succeeds), so the adapter must not invent a deeper minimum.
        for name in ["refs/main", "refs/x", "refs/notheads/lane"] {
            assert_eq!(RefName::parse(name).unwrap().as_str(), name);
        }
        // A `refs/heads/` ref whose suffix is not a representable short branch
        // name is still a real, exact ref.
        for name in ["refs/heads/HEAD", "refs/heads/-name", "refs/heads/-x"] {
            let parsed = RefName::parse(name).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(parsed.as_str(), name);
            assert!(parsed.is_branch());
        }
        assert_eq!(
            RefName::parse("refs/heads/HEAD").unwrap().short_branch(),
            None,
            "`HEAD` is not a `BranchName`; the exact ref still exists"
        );
    }

    #[test]
    fn a_branch_ref_can_never_be_built_outside_refs_heads() {
        let branch = BranchName::parse("topic/x").unwrap();
        assert_eq!(RefName::branch(&branch).as_str(), "refs/heads/topic/x");
    }
}
