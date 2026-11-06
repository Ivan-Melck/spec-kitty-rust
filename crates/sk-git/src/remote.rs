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
//! **Two freshness APIs, two contracts.**
//!
//! * [`GitRepository::check_branch_freshness`] is the READ-ONLY query: it never
//!   fetches, so a remote tip absent locally is
//!   [`FreshnessState::CompareFailed`] rather than a fabricated comparison.
//! * [`GitRepository::refresh_branches`] is the SOURCE-EXECUTION path: it is the
//!   Rust projection of `specify_cli/git/origin_freshness.py`'s own contact —
//!   one bounded `ls-remote` per remote, then ONE bounded `fetch` of the listed
//!   tips into `refs/remotes/<remote>/<branch>`, then the comparison against
//!   that tracking ref. Source's evidence gates use THIS one; the read-only
//!   query is not a waiver for it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::time::Duration;

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

/// What one remote's `HEAD` resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteHeadTarget {
    /// `HEAD` is the named branch (a remote default branch).
    Branch {
        /// The branch the remote advertised as `HEAD`.
        branch: BranchName,
        /// The tip it advertised for `HEAD`.
        oid: Oid,
    },
    /// `HEAD` points directly at a commit (a detached remote);
    Detached {
        /// The advertised tip.
        oid: Oid,
    },
    /// The remote answered, and its `HEAD` names no commit yet.
    Unborn,
}

impl RemoteHeadTarget {
    /// A stable machine-readable name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Branch { .. } => "branch",
            Self::Detached { .. } => "detached",
            Self::Unborn => "unborn",
        }
    }

    /// The default branch name, when `HEAD` is symbolic.
    pub fn branch(&self) -> Option<&BranchName> {
        match self {
            Self::Branch { branch, .. } => Some(branch),
            _ => None,
        }
    }

    /// The advertised tip, when there is one.
    pub fn oid(&self) -> Option<&Oid> {
        match self {
            Self::Branch { oid, .. } | Self::Detached { oid } => Some(oid),
            Self::Unborn => None,
        }
    }
}

/// The bounded, prompt-free answer to "what does this remote's `HEAD` name?".
#[derive(Debug, Clone)]
pub struct RemoteHead {
    remote: RemoteName,
    target: RemoteHeadTarget,
}

impl RemoteHead {
    /// The remote that answered.
    pub fn remote(&self) -> &RemoteName {
        &self.remote
    }

    /// Its `HEAD` target.
    pub fn target(&self) -> &RemoteHeadTarget {
        &self.target
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
    /// Seconds the contact was given when it timed out (`unreachable` only).
    timeout_seconds: Option<f64>,
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

    /// The bound the contact was given when it timed out (`unreachable` only).
    pub fn timeout_seconds(&self) -> Option<f64> {
        self.timeout_seconds
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

/// The bound Source gives one `ls-remote` (`kernel.git.remote.LS_REMOTE_TIMEOUT`).
pub const SOURCE_LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(5);
/// The bound Source gives one branch `fetch` (`kernel.git.remote.FETCH_TIMEOUT`).
pub const SOURCE_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// One branch's freshness after an actual Source-style refresh.
///
/// The verdict is the Source-shaped fact; `tracking_tip` and `fetched` are the
/// OBSERVATIONS the refresh made to reach it, so a caller (or a test) can bind
/// the tracking ref and whether this call transferred the branch instead of
/// trusting a verdict alone.
#[derive(Debug, Clone)]
pub struct RefreshedFreshness {
    verdict: FreshnessVerdict,
    tracking_tip: Option<Oid>,
    fetched: bool,
}

impl RefreshedFreshness {
    /// The freshness verdict.
    pub fn verdict(&self) -> &FreshnessVerdict {
        &self.verdict
    }

    /// `refs/remotes/<remote>/<branch>` as it stands AFTER the refresh.
    pub fn tracking_tip(&self) -> Option<&Oid> {
        self.tracking_tip.as_ref()
    }

    /// Whether this call transferred this branch (its tracking ref was not
    /// already at the tip the remote listed).
    pub fn fetched(&self) -> bool {
        self.fetched
    }
}

impl GitRepository {
    /// The URL git reports for one configured remote (`git remote get-url <name>`).
    ///
    /// This is the canonical LOCAL remote-URL read, and it is deliberately
    /// `remote get-url`, not `config --get remote.<name>.url`:
    ///
    /// * `get-url` applies the repository's `url.<base>.insteadOf` rewrites, so the
    ///   caller sees the URL git would actually use (a config read would not);
    /// * `get-url` returns the FIRST configured URL when the remote carries
    ///   several, which is the single value a `.strip()`-and-compare caller sees;
    /// * nothing here contacts the network — no `ls-remote`, no fetch.
    ///
    /// It reproduces the reference's former-checkout proof read
    /// (`subprocess.run(["git", "remote", "get-url", "origin"], timeout=2,
    /// check=True, capture_output=True, text=True).stdout.strip()`). The bound is
    /// the repository's [`GitOptions::timeout`], so a caller that needs the
    /// reference's 2 s contact opens the repository with that timeout.
    ///
    /// `Ok(None)` means git answered "No such remote" — a typed absence, since
    /// the reference's `check=True` turns exactly that case into its caught
    /// exception. Every other failure is an error, so a broken repository or a
    /// failed invocation is never read as a missing remote.
    pub fn remote_url(&self, remote: &RemoteName) -> Result<Option<String>, GitError> {
        let args = ["remote", "get-url", remote.as_str()];
        let output = self.process().run(&args)?;
        if output.status.success() {
            let text = crate::path::utf8_field(&output.stdout, "remote url")?;
            // The reference `.strip()`s the captured stdout.
            return Ok(Some(text.trim().to_owned()));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.code() == Some(2) && stderr.contains("No such remote") {
            return Ok(None);
        }
        Err(self.process().failed(&args, &output))
    }

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
    ///
    /// This is the GRAMMAR; the bound is the handle's own. The Source execution
    /// path calls the bounded form with Source's own 5 s `ls-remote` bound.
    pub fn probe_remote_heads(
        &self,
        remote: &RemoteName,
        branches: &[BranchName],
    ) -> Result<RemoteHeads, GitError> {
        self.probe_remote_heads_bounded(remote, branches, self.options().timeout)
    }

    fn probe_remote_heads_bounded(
        &self,
        remote: &RemoteName,
        branches: &[BranchName],
        timeout: Duration,
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
        let output = match self
            .process()
            .run_remote_bounded(&args, ssh_command, timeout)
        {
            Ok(output) => output,
            Err(GitError::Timeout { timeout, .. }) => {
                return Err(GitError::RemoteUnreachable {
                    remote: remote.as_str().to_string(),
                    detail: unreachable_detail(remote, b"", true),
                    timed_out: true,
                    timeout: Some(timeout),
                });
            }
            Err(error) => return Err(error),
        };
        if !output.status.success() {
            return Err(GitError::RemoteUnreachable {
                remote: remote.as_str().to_string(),
                detail: unreachable_detail(remote, &output.stderr, false),
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

    /// Resolve one remote's `HEAD` with a bounded, prompt-free read.
    ///
    /// This is the probe a caller uses when `refs/remotes/<remote>/HEAD` is
    /// absent: it asks the remote itself. It contacts the remote **once** with
    /// `git ls-remote --symref <remote> HEAD`, under the same no-prompt policy
    /// as every other remote read (terminal prompts disabled, ssh `BatchMode`
    /// applied when the effective transport is plain ssh, the handle's timeout).
    ///
    /// A failed or timed-out contact is [`GitError::RemoteUnreachable`], never a
    /// fabricated "no default branch". A successful contact that advertises an
    /// unborn `HEAD` is [`RemoteHeadTarget::Unborn`].
    pub fn resolve_remote_head(&self, remote: &RemoteName) -> Result<RemoteHead, GitError> {
        let ssh_command = self.non_interactive_ssh_command()?;
        let args = [
            "ls-remote".to_string(),
            "--symref".to_string(),
            "--".to_string(),
            remote.as_str().to_string(),
            "HEAD".to_string(),
        ];
        let output = match self.process().run_remote(&args, ssh_command) {
            Ok(output) => output,
            Err(GitError::Timeout { timeout, .. }) => {
                return Err(GitError::RemoteUnreachable {
                    remote: remote.as_str().to_string(),
                    detail: unreachable_detail(remote, b"", true),
                    timed_out: true,
                    timeout: Some(timeout),
                });
            }
            Err(error) => return Err(error),
        };
        if !output.status.success() {
            return Err(GitError::RemoteUnreachable {
                remote: remote.as_str().to_string(),
                detail: unreachable_detail(remote, &output.stderr, false),
                timed_out: false,
                timeout: None,
            });
        }

        let text = crate::path::utf8_field(&output.stdout, "remote head")?;
        let mut symbolic: Option<String> = None;
        let mut head: Option<Oid> = None;
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((left, reference)) = line.split_once('\t') else {
                return Err(GitError::InvalidOutput {
                    detail: format!("malformed ls-remote line: {line:?}"),
                });
            };
            if reference != "HEAD" {
                continue;
            }
            if let Some(target) = left.strip_prefix("ref: ") {
                symbolic = Some(target.trim().to_string());
            } else {
                head = Some(Oid::parse(left.trim())?);
            }
        }
        let target = match (symbolic, head) {
            (Some(reference), oid) => {
                let short = reference.strip_prefix("refs/heads/").ok_or_else(|| {
                    GitError::InvalidOutput {
                        detail: format!(
                            "remote HEAD advertises a symbolic ref outside refs/heads: {reference:?}"
                        ),
                    }
                })?;
                let branch = BranchName::parse(short)?;
                match oid {
                    Some(oid) => RemoteHeadTarget::Branch { branch, oid },
                    None => RemoteHeadTarget::Unborn,
                }
            }
            (None, Some(oid)) => RemoteHeadTarget::Detached { oid },
            (None, None) => RemoteHeadTarget::Unborn,
        };
        Ok(RemoteHead {
            remote: remote.clone(),
            target,
        })
    }

    /// The remote's default branch name, when it advertises one.
    ///
    /// A convenience over [`GitRepository::resolve_remote_head`] for callers
    /// that only need the protection-augmentation fact.
    pub fn remote_default_branch(
        &self,
        remote: &RemoteName,
    ) -> Result<Option<BranchName>, GitError> {
        Ok(self.resolve_remote_head(remote)?.target().branch().cloned())
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
                timeout_seconds: None,
            });
        };

        let heads = match self.probe_remote_heads(&remote, std::slice::from_ref(branch)) {
            Ok(heads) => heads,
            Err(GitError::RemoteUnreachable {
                detail,
                timed_out,
                timeout,
                ..
            }) => {
                return Ok(FreshnessVerdict {
                    branch: branch.clone(),
                    remote: Some(remote),
                    state: FreshnessState::Unreachable,
                    behind: 0,
                    ahead: 0,
                    local_tip,
                    remote_tip: None,
                    detail: Some(detail),
                    timeout_seconds: timed_out
                        .then(|| timeout.map_or(0.0, |bound| bound.as_secs_f64())),
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
                timeout_seconds: None,
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
                timeout_seconds: None,
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
                timeout_seconds: None,
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
            timeout_seconds: None,
        })
    }

    /// Count commits unique to each side of `local...other`.
    pub fn divergence(&self, local: &RefName, other: &RefName) -> Result<AheadBehind, GitError> {
        self.divergence_operands(local.as_str(), other.as_str())
    }

    /// Source-execution freshness: contact, FETCH, then compare — the API every
    /// Source evidence gate actually runs.
    ///
    /// This is `specify_cli/git/origin_freshness.py`'s `check_branches` /
    /// `_check_probes` / `_check_remote` on top of the one remote/ref authority:
    ///
    /// 1. resolve the remote each branch's configuration names (local reads);
    /// 2. ONE bounded `ls-remote --heads <remote> <refs...>` per remote
    ///    ([`SOURCE_LS_REMOTE_TIMEOUT`]);
    /// 3. ONE bounded `fetch --no-tags --no-recurse-submodules --no-auto-gc
    ///    --refmap= <remote> +refs/heads/<b>:refs/remotes/<remote>/<b> ...` for
    ///    the listed branches whose tracking ref is not already at that tip
    ///    ([`SOURCE_FETCH_TIMEOUT`]) — it never writes `refs/heads`;
    /// 4. compare each local branch with its tracking ref (`rev-list
    ///    --left-right --count`), so a local object that is genuinely absent is
    ///    never read as up to date.
    ///
    /// A failed contact is [`FreshnessState::Unreachable`] with the bound that
    /// expired, never "the branch is absent". `refs/heads` is untouched; only
    /// `refs/remotes/<remote>/*` and git's own `FETCH_HEAD` move — this is what
    /// makes a Source-valid comparison possible, and it is why this is a
    /// separate API from the read-only query.
    pub fn refresh_branches(
        &self,
        branches: &[BranchName],
    ) -> Result<Vec<RefreshedFreshness>, GitError> {
        let remotes = self.configured_remotes()?;
        let mut slots: Vec<Option<RefreshedFreshness>> =
            (0..branches.len()).map(|_| None).collect();
        let mut by_remote: Vec<(RemoteName, Vec<usize>)> = Vec::new();
        for (index, branch) in branches.iter().enumerate() {
            match self.resolve_remote(branch)? {
                Some(remote) => match by_remote.iter_mut().find(|(name, _)| name == &remote) {
                    Some((_, indexes)) => indexes.push(index),
                    None => by_remote.push((remote, vec![index])),
                },
                None => {
                    let state = if remotes.is_empty() {
                        FreshnessState::NoRemote
                    } else {
                        FreshnessState::RemoteAmbiguous
                    };
                    let detail = if remotes.is_empty() {
                        self.removed_remote_detail()?
                    } else {
                        Some(
                            remotes
                                .iter()
                                .map(RemoteName::as_str)
                                .collect::<Vec<_>>()
                                .join(", "),
                        )
                    };
                    slots[index] = Some(RefreshedFreshness {
                        verdict: FreshnessVerdict {
                            branch: branch.clone(),
                            remote: None,
                            state,
                            behind: 0,
                            ahead: 0,
                            local_tip: None,
                            remote_tip: None,
                            detail,
                            timeout_seconds: None,
                        },
                        tracking_tip: None,
                        fetched: false,
                    });
                }
            }
        }
        for (remote, indexes) in by_remote {
            let wanted: Vec<BranchName> = indexes
                .iter()
                .map(|&index| branches[index].clone())
                .collect();
            let refreshed = self.refresh_remote(&remote, &wanted)?;
            for (slot, value) in indexes.into_iter().zip(refreshed) {
                slots[slot] = Some(value);
            }
        }
        Ok(slots
            .into_iter()
            .map(|slot| slot.expect("every requested branch has one verdict"))
            .collect())
    }

    /// The Source removed-remote note: this clone still holds remote-tracking
    /// refs although no remote is configured.
    fn removed_remote_detail(&self) -> Result<Option<String>, GitError> {
        let listed = self.process().run(&[
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            "refs/remotes/",
        ])?;
        if !listed.status.success() || listed.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        Ok(Some(
            "This clone has remote-tracking refs but no remote is configured (the remote was \
             removed?), so nothing was checked against origin: a teammate's newer status \
             evidence or lane work would not be seen"
                .to_string(),
        ))
    }

    /// Whether `refs/remotes/<remote>/<branch>` already holds exactly `tip`.
    ///
    /// Source's `_tracking_ref_is_at`: a local read; any doubt is `false`, so the
    /// branch is fetched (never skipped on a guess).
    fn tracking_ref_is_at(
        &self,
        remote: &RemoteName,
        branch: &BranchName,
        tip: &Oid,
    ) -> Result<bool, GitError> {
        let reference = RefName::remote_tracking(remote, branch);
        let peel = format!("{}^{{commit}}", reference.as_str());
        let output = self
            .process()
            .allowing(&["rev-parse", "--verify", "--quiet", peel.as_str()], &[1])?;
        Ok(output.status.success()
            && crate::path::strip_line_feed(&output.stdout) == tip.as_str().as_bytes())
    }

    /// One remote's contact and comparison, for the Source execution path.
    fn refresh_remote(
        &self,
        remote: &RemoteName,
        branches: &[BranchName],
    ) -> Result<Vec<RefreshedFreshness>, GitError> {
        let heads =
            match self.probe_remote_heads_bounded(remote, branches, SOURCE_LS_REMOTE_TIMEOUT) {
                Ok(heads) => heads,
                Err(GitError::RemoteUnreachable {
                    detail,
                    timed_out,
                    timeout,
                    ..
                }) => {
                    let seconds =
                        timed_out.then(|| timeout.map_or(0.0, |bound| bound.as_secs_f64()));
                    return Ok(branches
                        .iter()
                        .map(|branch| RefreshedFreshness {
                            verdict: FreshnessVerdict {
                                branch: branch.clone(),
                                remote: Some(remote.clone()),
                                state: FreshnessState::Unreachable,
                                behind: 0,
                                ahead: 0,
                                local_tip: None,
                                remote_tip: None,
                                detail: Some(detail.clone()),
                                timeout_seconds: seconds,
                            },
                            tracking_tip: None,
                            fetched: false,
                        })
                        .collect());
                }
                Err(error) => return Err(error),
            };

        let mut transfer: Vec<BranchName> = Vec::new();
        for branch in branches {
            let Some(tip) = heads.get(branch) else {
                continue;
            };
            if !self.tracking_ref_is_at(remote, branch, tip)? {
                transfer.push(branch.clone());
            }
        }
        if !transfer.is_empty() {
            let mut args: Vec<String> = vec![
                "fetch".to_string(),
                "--no-tags".to_string(),
                "--no-recurse-submodules".to_string(),
                "--no-auto-gc".to_string(),
                "--refmap=".to_string(),
                remote.as_str().to_string(),
            ];
            for branch in &transfer {
                args.push(format!(
                    "+refs/heads/{0}:{1}",
                    branch.as_str(),
                    RefName::remote_tracking(remote, branch).as_str()
                ));
            }
            let ssh_command = self.non_interactive_ssh_command()?;
            match self
                .process()
                .run_remote_bounded(&args, ssh_command, SOURCE_FETCH_TIMEOUT)
            {
                Ok(output) if output.status.success() => {}
                Ok(output) => {
                    let detail = unreachable_detail(remote, &output.stderr, false);
                    return Ok(unreachable_all(remote, branches, detail, None));
                }
                Err(GitError::Timeout { timeout, .. }) => {
                    return Ok(unreachable_all(
                        remote,
                        branches,
                        unreachable_detail(remote, b"", true),
                        Some(timeout.as_secs_f64()),
                    ));
                }
                Err(error) => return Err(error),
            }
        }

        let mut results = Vec::with_capacity(branches.len());
        for branch in branches {
            let tracking = RefName::remote_tracking(remote, branch);
            let tracking_tip = self.read_ref(&tracking)?;
            let fetched = transfer.iter().any(|candidate| candidate == branch);
            let Some(remote_tip) = heads.get(branch).cloned() else {
                let detail = if tracking_tip.is_some() {
                    Some(format!(
                        "{remote}/{branch} was seen here before, but {remote} no longer lists {branch}: it may have been deleted on {remote}"
                    ))
                } else {
                    None
                };
                results.push(RefreshedFreshness {
                    verdict: FreshnessVerdict {
                        branch: branch.clone(),
                        remote: Some(remote.clone()),
                        state: FreshnessState::RemoteMissing,
                        behind: 0,
                        ahead: 0,
                        local_tip: None,
                        remote_tip: None,
                        detail,
                        timeout_seconds: None,
                    },
                    tracking_tip,
                    fetched,
                });
                continue;
            };
            if self.read_branch_tip(branch)?.is_none() {
                results.push(RefreshedFreshness {
                    verdict: FreshnessVerdict {
                        branch: branch.clone(),
                        remote: Some(remote.clone()),
                        state: FreshnessState::LocalMissing,
                        behind: 0,
                        ahead: 0,
                        local_tip: None,
                        remote_tip: Some(remote_tip),
                        detail: None,
                        timeout_seconds: None,
                    },
                    tracking_tip,
                    fetched,
                });
                continue;
            }
            let comparison = self.divergence(&branch.as_ref_name(), &tracking);
            let (state, behind, ahead, detail) = match comparison {
                Ok(counts) => {
                    let state = if counts.behind > 0 {
                        if counts.ahead > 0 {
                            FreshnessState::Diverged
                        } else {
                            FreshnessState::Behind
                        }
                    } else if counts.ahead > 0 {
                        FreshnessState::Ahead
                    } else {
                        FreshnessState::UpToDate
                    };
                    (state, counts.behind, counts.ahead, None)
                }
                // Fail closed, exactly as Source: a comparison git cannot make is
                // never read as up to date.
                Err(error) => (FreshnessState::CompareFailed, 0, 0, Some(error.to_string())),
            };
            results.push(RefreshedFreshness {
                verdict: FreshnessVerdict {
                    branch: branch.clone(),
                    remote: Some(remote.clone()),
                    state,
                    behind,
                    ahead,
                    local_tip: None,
                    remote_tip: Some(remote_tip),
                    detail,
                    timeout_seconds: None,
                },
                tracking_tip,
                fetched,
            });
        }
        Ok(results)
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

/// Every branch of one remote marked unreachable by the SAME failed contact.
fn unreachable_all(
    remote: &RemoteName,
    branches: &[BranchName],
    detail: String,
    timeout_seconds: Option<f64>,
) -> Vec<RefreshedFreshness> {
    branches
        .iter()
        .map(|branch| RefreshedFreshness {
            verdict: FreshnessVerdict {
                branch: branch.clone(),
                remote: Some(remote.clone()),
                state: FreshnessState::Unreachable,
                behind: 0,
                ahead: 0,
                local_tip: None,
                remote_tip: None,
                detail: Some(detail.clone()),
                timeout_seconds,
            },
            tracking_tip: None,
            fetched: false,
        })
        .collect()
}

/// Source `RemoteUnreachable.__str__`'s detail: `remote <name> unreachable: <first stderr line>`.
///
/// `kernel.git.remote.RemoteUnreachable` takes the FIRST stderr line (or
/// `timed out` when the bound expired, or `no error output` when git was
/// silent) — never the whole multi-line diagnostic. The operator text and the
/// freshness `detail` both carry exactly that string, so the same rule applies
/// to every remote contact here.
fn unreachable_detail(remote: &RemoteName, stderr: &[u8], timed_out: bool) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    let detail = if !stderr.is_empty() {
        stderr.lines().next().unwrap_or_default().trim().to_owned()
    } else if timed_out {
        "timed out".to_owned()
    } else {
        "no error output".to_owned()
    };
    format!("remote {remote} unreachable: {detail}")
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
