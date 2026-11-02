//! Bounded, prompt-free remote **reads**.
//!
//! This module owns the only remote contact in the crate: `git ls-remote`.
//! There is no fetch, no clone, no push, no tag update and no remote
//! configuration write. A remote that cannot be asked is reported as
//! [`GitError::RemoteUnreachable`] (or, in a freshness verdict,
//! [`FreshnessState::Unreachable`]); it is never reported as "the branch is
//! absent". Only every remote answering cleanly with no match is a
//! [`RemoteBranchLookup::CleanMiss`].
//!
//! Children run with `GIT_TERMINAL_PROMPT=0`; when the effective transport is
//! plain ssh(1), `-o BatchMode=yes` is added. A user-pinned `GIT_SSH_COMMAND`
//! (or `core.sshCommand`) is preserved, never silently replaced. Each contact
//! is bounded by the [`GitOptions::timeout`](crate::GitOptions) of the handle.
//!
//! **Not implemented here (honest gap):** upstream fetches the listed tips into
//! `refs/remotes/<remote>/<branch>` before comparing. This adapter does not
//! fetch, so a freshness comparison requires the remote tip object to already
//! exist locally; otherwise the verdict is
//! [`FreshnessState::CompareFailed`] with that explanation.

use std::collections::BTreeMap;
use std::ffi::OsString;

use crate::error::GitError;
use crate::oid::Oid;
use crate::refname::{BranchName, RefName};
use crate::repository::GitRepository;

/// A configured remote name (never a URL or a transport command).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteName(String);

impl RemoteName {
    /// Validate a plain remote name.
    ///
    /// `origin`, `upstream-1`, `my.remote` are accepted. A URL, scp-like
    /// `host:path`, `ext::` helper, whitespace, control characters, `/`, `@`,
    /// `..`, and a leading `-` are refused.
    pub fn parse(value: &str) -> Result<Self, GitError> {
        let invalid = || GitError::InvalidRemoteName {
            name: value.to_string(),
        };
        if value.is_empty() || value.len() > 255 || value.starts_with('-') {
            return Err(invalid());
        }
        if value.contains("..") {
            return Err(invalid());
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(invalid());
        }
        if !value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(invalid());
        }
        Ok(Self(value.to_string()))
    }

    /// The remote name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RemoteName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::str::FromStr for RemoteName {
    type Err = GitError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// The branches one remote listed, with the tip it advertised for each.
#[derive(Debug, Clone)]
pub struct RemoteHeads {
    remote: RemoteName,
    heads: BTreeMap<BranchName, Oid>,
}

impl RemoteHeads {
    /// The remote that answered.
    pub fn remote(&self) -> &RemoteName {
        &self.remote
    }

    /// The listed tips.
    pub fn heads(&self) -> &BTreeMap<BranchName, Oid> {
        &self.heads
    }

    /// The tip of one branch, when the remote listed it.
    pub fn get(&self, branch: &BranchName) -> Option<&Oid> {
        self.heads.get(branch)
    }

    /// Whether the remote answered and did not list `branch`.
    ///
    /// This is a clean absence: the contact succeeded.
    pub fn is_absent(&self, branch: &BranchName) -> bool {
        !self.heads.contains_key(branch)
    }
}

/// The outcome of asking **every** configured remote whether one branch exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteBranchLookup {
    /// At least one remote listed the branch.
    Hit {
        /// The first remote (in `git remote` order) that listed it.
        remote: RemoteName,
        /// The advertised tip.
        oid: Oid,
    },
    /// Every configured remote answered, and none listed the branch.
    CleanMiss,
    /// At least one remote could not be asked cleanly; absence is not proven.
    Error {
        /// The failure detail.
        detail: String,
    },
    /// The repository has no configured remote.
    NoRemote,
}

/// How one local branch compares with its owning remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessState {
    /// Local and remote tips are equal.
    UpToDate,
    /// The remote has commits the local branch lacks.
    Behind,
    /// The local branch has commits the remote lacks.
    Ahead,
    /// Both sides have unique commits.
    Diverged,
    /// The remote lists the branch but there is no local branch.
    LocalMissing,
    /// The remote answered and does not list the branch.
    RemoteMissing,
    /// The remote could not be asked.
    Unreachable,
    /// No remote is configured.
    NoRemote,
    /// Several remotes are configured and none owns the branch.
    RemoteAmbiguous,
    /// The remote answered, but the two tips could not be compared locally.
    CompareFailed,
}

impl FreshnessState {
    /// A stable machine-readable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up_to_date",
            Self::Behind => "behind",
            Self::Ahead => "ahead",
            Self::Diverged => "diverged",
            Self::LocalMissing => "local_missing",
            Self::RemoteMissing => "remote_missing",
            Self::Unreachable => "unreachable",
            Self::NoRemote => "no_remote",
            Self::RemoteAmbiguous => "remote_ambiguous",
            Self::CompareFailed => "compare_failed",
        }
    }
}

/// One branch's read-only freshness evidence.
#[derive(Debug, Clone)]
pub struct FreshnessVerdict {
    branch: BranchName,
    remote: Option<RemoteName>,
    state: FreshnessState,
    behind: u64,
    ahead: u64,
    local_tip: Option<Oid>,
    remote_tip: Option<Oid>,
    detail: Option<String>,
}

impl FreshnessVerdict {
    /// The branch that was checked.
    pub fn branch(&self) -> &BranchName {
        &self.branch
    }

    /// The remote that owns the branch, when one was resolved.
    pub fn remote(&self) -> Option<&RemoteName> {
        self.remote.as_ref()
    }

    /// The verdict.
    pub fn state(&self) -> FreshnessState {
        self.state
    }

    /// Commits only the remote has (0 when not compared).
    pub fn behind(&self) -> u64 {
        self.behind
    }

    /// Commits only the local branch has (0 when not compared).
    pub fn ahead(&self) -> u64 {
        self.ahead
    }

    /// The local branch tip, when it exists.
    pub fn local_tip(&self) -> Option<&Oid> {
        self.local_tip.as_ref()
    }

    /// The tip the remote advertised, when it listed the branch.
    pub fn remote_tip(&self) -> Option<&Oid> {
        self.remote_tip.as_ref()
    }

    /// Extra context (a vanished branch, an unresolved compare, a transport
    /// error). Never parse this text; use [`FreshnessVerdict::state`].
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// Commits unique to each side of a local/remote comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    /// Commits on the local side only.
    pub ahead: u64,
    /// Commits on the other side only.
    pub behind: u64,
}

impl AheadBehind {
    /// Whether both sides carry unique commits.
    pub fn diverged(self) -> bool {
        self.ahead > 0 && self.behind > 0
    }
}

impl GitRepository {
    /// Names of the configured remotes, in `git remote` order.
    ///
    /// A failing `git remote` is an error, not "no remotes". A remote name this
    /// adapter refuses (for example a URL added as a name) is also an error,
    /// so a lookup can never silently omit an authority.
    pub fn configured_remotes(&self) -> Result<Vec<RemoteName>, GitError> {
        let output = self.process().checked(&["remote"])?;
        let text = crate::path::utf8_field(&output.stdout, "remote list")?;
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(RemoteName::parse)
            .collect()
    }

    /// Resolve the one remote that owns `branch`, using local configuration only.
    ///
    /// Order matches upstream: `branch.<branch>.remote` (`.` means "this
    /// repository" and is treated as unset), then the only configured remote,
    /// then `origin`. `None` means no remote applies; pair it with
    /// [`GitRepository::configured_remotes`] to distinguish "no remote" from
    /// "several, none owning".
    pub fn resolve_remote(&self, branch: &BranchName) -> Result<Option<RemoteName>, GitError> {
        let key = format!("branch.{}.remote", branch.as_str());
        let output = self.process().allowing(&["config", "--get", &key], &[1])?;
        if output.status.code() == Some(0) {
            let configured = crate::path::utf8_field(
                crate::path::strip_line_feed(&output.stdout),
                "branch remote",
            )?;
            if !configured.is_empty() && configured != "." {
                return Ok(Some(RemoteName::parse(&configured)?));
            }
        }

        let remotes = self.configured_remotes()?;
        if remotes.len() == 1 {
            return Ok(remotes.into_iter().next());
        }
        if let Some(origin) = remotes.into_iter().find(|name| name.as_str() == "origin") {
            return Ok(Some(origin));
        }
        Ok(None)
    }

    /// Ask one remote for the tips of `branches` with one bounded `ls-remote`.
    ///
    /// An empty `branches` list returns an empty answer without contacting the
    /// remote. A failed or timed-out contact is
    /// [`GitError::RemoteUnreachable`]. A successful contact that omits a
    /// requested branch is a clean absence, readable through
    /// [`RemoteHeads::is_absent`].
    pub fn probe_remote_heads(
        &self,
        remote: &RemoteName,
        branches: &[BranchName],
    ) -> Result<RemoteHeads, GitError> {
        if branches.is_empty() {
            return Ok(RemoteHeads {
                remote: remote.clone(),
                heads: BTreeMap::new(),
            });
        }

        let mut wanted = BTreeMap::new();
        let mut args: Vec<String> = vec![
            "ls-remote".to_string(),
            "--heads".to_string(),
            "--".to_string(),
            remote.as_str().to_string(),
        ];
        for branch in branches {
            let reference = branch.as_ref_name();
            wanted.insert(reference.as_str().to_string(), branch.clone());
            args.push(reference.as_str().to_string());
        }

        let ssh_command = self.non_interactive_ssh_command()?;
        let output = match self.process().run_remote(&args, ssh_command) {
            Ok(output) => output,
            Err(GitError::Timeout {
                timeout, ref args, ..
            }) => {
                return Err(GitError::RemoteUnreachable {
                    remote: remote.as_str().to_string(),
                    detail: format!("timed out after {timeout:?}: git {}", args.join(" ")),
                    timed_out: true,
                    timeout: Some(timeout),
                });
            }
            Err(error) => return Err(error),
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(GitError::RemoteUnreachable {
                remote: remote.as_str().to_string(),
                detail: if stderr.is_empty() {
                    "git ls-remote failed without a diagnostic".to_string()
                } else {
                    stderr
                },
                timed_out: false,
                timeout: None,
            });
        }

        let text = crate::path::utf8_field(&output.stdout, "remote heads")?;
        let mut heads = BTreeMap::new();
        for line in text.lines() {
            let Some((oid, reference)) = line.split_once('\t') else {
                return Err(GitError::InvalidOutput {
                    detail: format!("malformed ls-remote line: {line:?}"),
                });
            };
            let Some(branch) = wanted.get(reference) else {
                // Patterns are exact; an unrelated line means the remote
                // answered something this probe did not ask for. Ignoring it
                // is safe because the requested set is matched by full ref.
                continue;
            };
            heads.insert(branch.clone(), Oid::parse(oid)?);
        }
        Ok(RemoteHeads {
            remote: remote.clone(),
            heads,
        })
    }

    /// The tri-state (plus no-remote) presence of `branch` across every remote.
    ///
    /// A hit on any remote wins; otherwise any failed contact makes the answer
    /// [`RemoteBranchLookup::Error`], never a fabricated absence. The probe is
    /// not memoized: callers that would repeat it in one operation should hold
    /// the result themselves.
    pub fn remote_branch_lookup(
        &self,
        branch: &BranchName,
    ) -> Result<RemoteBranchLookup, GitError> {
        let remotes = self.configured_remotes()?;
        if remotes.is_empty() {
            return Ok(RemoteBranchLookup::NoRemote);
        }
        let mut saw_error = false;
        let mut last_error = String::new();
        for remote in remotes {
            match self.probe_remote_heads(&remote, std::slice::from_ref(branch)) {
                Ok(heads) => {
                    if let Some(oid) = heads.get(branch) {
                        return Ok(RemoteBranchLookup::Hit {
                            remote,
                            oid: oid.clone(),
                        });
                    }
                }
                Err(GitError::RemoteUnreachable { detail, .. }) => {
                    saw_error = true;
                    last_error = detail;
                }
                Err(error) => return Err(error),
            }
        }
        if saw_error {
            return Ok(RemoteBranchLookup::Error { detail: last_error });
        }
        Ok(RemoteBranchLookup::CleanMiss)
    }

    /// Read-only freshness for one branch against the remote that owns it.
    ///
    /// The remote is contacted once. Nothing is fetched, so a remote tip absent
    /// from the local object database yields
    /// [`FreshnessState::CompareFailed`] with an explicit detail instead of a
    /// fabricated comparison. An unreachable remote is a verdict, not an
    /// error; a broken `git remote`/config read still is an error.
    pub fn check_branch_freshness(
        &self,
        branch: &BranchName,
    ) -> Result<FreshnessVerdict, GitError> {
        let local_tip = self.read_branch_tip(branch)?;
        let remotes = self.configured_remotes()?;
        let Some(remote) = self.resolve_remote(branch)? else {
            let state = if remotes.is_empty() {
                FreshnessState::NoRemote
            } else {
                FreshnessState::RemoteAmbiguous
            };
            return Ok(FreshnessVerdict {
                branch: branch.clone(),
                remote: None,
                state,
                behind: 0,
                ahead: 0,
                local_tip,
                remote_tip: None,
                detail: if remotes.is_empty() {
                    None
                } else {
                    Some(
                        remotes
                            .iter()
                            .map(RemoteName::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                },
            });
        };

        let heads = match self.probe_remote_heads(&remote, std::slice::from_ref(branch)) {
            Ok(heads) => heads,
            Err(GitError::RemoteUnreachable { detail, .. }) => {
                return Ok(FreshnessVerdict {
                    branch: branch.clone(),
                    remote: Some(remote),
                    state: FreshnessState::Unreachable,
                    behind: 0,
                    ahead: 0,
                    local_tip,
                    remote_tip: None,
                    detail: Some(detail),
                });
            }
            Err(error) => return Err(error),
        };

        let Some(remote_tip) = heads.get(branch).cloned() else {
            let vanished = if self
                .read_ref(&RefName::remote_tracking(&remote, branch))?
                .is_some()
            {
                Some(format!(
                    "{remote}/{branch} was seen here before, but {remote} no longer lists {branch}"
                ))
            } else {
                None
            };
            return Ok(FreshnessVerdict {
                branch: branch.clone(),
                remote: Some(remote),
                state: FreshnessState::RemoteMissing,
                behind: 0,
                ahead: 0,
                local_tip,
                remote_tip: None,
                detail: vanished,
            });
        };

        let Some(local_tip) = local_tip else {
            return Ok(FreshnessVerdict {
                branch: branch.clone(),
                remote: Some(remote),
                state: FreshnessState::LocalMissing,
                behind: 0,
                ahead: 0,
                local_tip: None,
                remote_tip: Some(remote_tip),
                detail: None,
            });
        };

        if !self.commit_exists(&remote_tip)? {
            return Ok(FreshnessVerdict {
                branch: branch.clone(),
                remote: Some(remote),
                state: FreshnessState::CompareFailed,
                behind: 0,
                ahead: 0,
                local_tip: Some(local_tip),
                remote_tip: Some(remote_tip),
                detail: Some(
                    "the remote tip is not present locally; sk-git performs no fetch, so the two tips cannot be compared"
                        .to_string(),
                ),
            });
        }

        let comparison = self.divergence_against_oid(&RefName::branch(branch), &remote_tip)?;
        let state = if comparison.behind > 0 {
            if comparison.ahead > 0 {
                FreshnessState::Diverged
            } else {
                FreshnessState::Behind
            }
        } else if comparison.ahead > 0 {
            FreshnessState::Ahead
        } else {
            FreshnessState::UpToDate
        };
        Ok(FreshnessVerdict {
            branch: branch.clone(),
            remote: Some(remote),
            state,
            behind: comparison.behind,
            ahead: comparison.ahead,
            local_tip: Some(local_tip),
            remote_tip: Some(remote_tip),
            detail: None,
        })
    }

    /// Count commits unique to each side of `local...other`.
    pub fn divergence(&self, local: &RefName, other: &RefName) -> Result<AheadBehind, GitError> {
        self.divergence_operands(local.as_str(), other.as_str())
    }

    /// Count commits unique to each side of a local ref and an object id.
    pub fn divergence_against_oid(
        &self,
        local: &RefName,
        other: &Oid,
    ) -> Result<AheadBehind, GitError> {
        self.divergence_operands(local.as_str(), other.as_str())
    }

    fn divergence_operands(&self, left: &str, right: &str) -> Result<AheadBehind, GitError> {
        let range = format!("{left}...{right}");
        let output = self
            .process()
            .checked(&["rev-list", "--left-right", "--count", &range])?;
        let text = crate::path::utf8_field(&output.stdout, "divergence counts")?;
        let mut fields = text.split_whitespace();
        let ahead = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| GitError::InvalidOutput {
                detail: format!("git rev-list --count printed no ahead count: {text:?}"),
            })?;
        let behind = fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| GitError::InvalidOutput {
                detail: format!("git rev-list --count printed no behind count: {text:?}"),
            })?;
        Ok(AheadBehind { ahead, behind })
    }

    /// Resolve the effective non-interactive ssh command for a remote read.
    ///
    /// `GIT_SSH_COMMAND` wins; then `core.sshCommand`; then plain `ssh`. A
    /// command that is not ssh-like is preserved unchanged. `None` means the
    /// user pinned `GIT_SSH` and no command is injected.
    fn non_interactive_ssh_command(&self) -> Result<Option<OsString>, GitError> {
        if let Some(command) = std::env::var_os("GIT_SSH_COMMAND")
            && !command.is_empty()
        {
            // Preserve native path bytes while applying the same no-prompt
            // rule used for core.sshCommand. An explicit ssh pin is not an
            // exemption from BatchMode.
            let rendered = command.to_string_lossy();
            if is_ssh_like(&rendered) && !rendered.contains("BatchMode") {
                let mut command = command;
                command.push(" -o BatchMode=yes");
                return Ok(Some(command));
            }
            return Ok(Some(command));
        }
        let configured = self
            .process()
            .allowing(&["config", "--get", "core.sshCommand"], &[1])?;
        let mut command = if configured.status.code() == Some(0) {
            crate::path::utf8_field(
                crate::path::strip_line_feed(&configured.stdout),
                "core.sshCommand",
            )?
        } else if std::env::var_os("GIT_SSH").is_some() {
            return Ok(None);
        } else {
            "ssh".to_string()
        };
        if command.trim().is_empty() {
            command = "ssh".to_string();
        }
        if is_ssh_like(&command) && !command.contains("BatchMode") {
            command.push_str(" -o BatchMode=yes");
        }
        Ok(Some(OsString::from(command)))
    }
}

fn is_ssh_like(command: &str) -> bool {
    let Some(program) = command.split_whitespace().next() else {
        return false;
    };
    let program = program.trim_matches(['"', '\'']);
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    name == "ssh" || name == "ssh.exe"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_remote_names_and_refuses_transports() {
        for name in ["origin", "upstream", "my.remote", "fork_1", "a-b"] {
            assert!(RemoteName::parse(name).is_ok(), "{name}");
        }
        for name in [
            "",
            "-origin",
            "https://example.com/repo.git",
            "ssh://host/repo",
            "host:path",
            "ext::sh -c id",
            "origin/team",
            "a..b",
            "a b",
            "origin\n",
            "foo@bar",
        ] {
            assert!(
                matches!(
                    RemoteName::parse(name),
                    Err(GitError::InvalidRemoteName { .. })
                ),
                "{name:?}"
            );
        }
    }

    #[test]
    fn ssh_detection_uses_the_program_basename() {
        assert!(is_ssh_like("ssh"));
        assert!(is_ssh_like("/usr/bin/ssh -i key"));
        assert!(is_ssh_like("\"C:\\OpenSSH\\ssh.exe\" -i key"));
        assert!(!is_ssh_like("plink -batch"));
        assert!(!is_ssh_like(""));
    }

    #[test]
    fn divergence_reports_both_directions() {
        let both = AheadBehind {
            ahead: 2,
            behind: 1,
        };
        assert!(both.diverged());
        assert!(
            !AheadBehind {
                ahead: 0,
                behind: 3
            }
            .diverged()
        );
    }
}
