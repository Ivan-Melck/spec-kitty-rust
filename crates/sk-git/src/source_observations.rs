//! Source-compatible read-only observations composed from the canonical owners.
//!
//! Read-only doctor consumers need a few observations that the reference takes
//! straight from Git text or from a full ref listing. Each one here composes an
//! existing owner instead of adding a second authority:
//!
//! * [`GitRepository::status_source_observed`] is the reference's
//!   `git status --short` shape — the untracked mode comes from the
//!   repository's own `status.showUntrackedFiles`, not from a Native override —
//!   and it runs through the ONE status owner and parser
//!   ([`crate::status::parse_status_z`]).
//! * [`GitRepository::remote_tracking_roster`] reproduces the reference's
//!   `refs/remotes/` scan: one [`GitRepository::refs_under`] read (the
//!   `for-each-ref` + [`crate::Oid`] + [`crate::RefName`] authority) split
//!   `<remote>/<branch...>` exactly as `partition("/")` does. The typed
//!   [`crate::RemoteHeads`] contact API and the local `RefState` reads are
//!   untouched: this is a *name* roster, not a second ref model.
//!
//! Deliberately NOT here: a new parsed-status model, a second worktree parser,
//! a second sparse path resolver, or any aggregate health verdict. Source
//! searches the resolved lane path as a plain substring anywhere in the raw
//! worktree text, without requiring a `worktree ` prefix. That raw text lives
//! with its observation owner
//! ([`crate::WorktreePorcelainObservation`]); the Source sparse read lives with
//! the sparse owner; the Source revision seam lives with the object owner.

use std::collections::BTreeSet;

use crate::error::GitError;
use crate::oid::Oid;
use crate::refname::RefName;
use crate::repository::GitRepository;
use crate::status::{StatusEntry, StatusOptions};

/// The prefix the reference scans for remote-tracking refs.
const REMOTES_PREFIX: &str = "refs/remotes/";

/// One remote-tracking ref as the reference's `refs/remotes/` scan sees it.
///
/// *reference* is the exact fully-qualified name (`refs/remotes/origin/main`).
/// *remote* and *branch* are the reference's split of everything after the
/// `refs/remotes/` prefix at the FIRST `/`: `origin/HEAD` → (`origin`, `HEAD`)
/// and `origin/kitty/mission-x` → (`origin`, `kitty/mission-x`). A ref spelled
/// `refs/remotes/name` with no `/` yields remote `name` and an empty branch,
/// exactly like `str.partition("/")`.
///
/// A symbolic remote `HEAD` is an ordinary entry here (it is what Git lists);
/// a symbolic ref whose target is missing is not listed by Git at all, which is
/// also what the reference's own `for-each-ref` sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTrackingRef {
    reference: RefName,
    remote: String,
    branch: String,
    oid: Option<Oid>,
    symbolic_target: Option<RefName>,
}

impl RemoteTrackingRef {
    /// The exact fully-qualified ref name.
    pub fn reference(&self) -> &RefName {
        &self.reference
    }

    /// The remote component (the first path component after `refs/remotes/`).
    pub fn remote(&self) -> &str {
        &self.remote
    }

    /// The branch component (everything after the first `/`), which the
    /// reference compares with a coordination branch name.
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// The object id Git reported for this ref, when it printed one.
    pub fn oid(&self) -> Option<&Oid> {
        self.oid.as_ref()
    }

    /// The symbolic target for a symbolic ref (for example a remote `HEAD`).
    pub fn symbolic_target(&self) -> Option<&RefName> {
        self.symbolic_target.as_ref()
    }

    /// Whether this ref is symbolic.
    pub fn is_symbolic(&self) -> bool {
        self.symbolic_target.is_some()
    }
}

/// The local remote-tracking refs, in Git's `for-each-ref` order.
///
/// The roster is the reference's cheap local answer before any remote contact:
/// [`RemoteTrackingRoster::contains_branch`] is "does any configured remote's
/// tracking ref name this branch?" without touching the network.
#[derive(Debug, Clone, Default)]
pub struct RemoteTrackingRoster {
    entries: Vec<RemoteTrackingRef>,
    branch_names: BTreeSet<String>,
}

impl RemoteTrackingRoster {
    /// Every listed remote-tracking ref, in Git's order.
    pub fn entries(&self) -> &[RemoteTrackingRef] {
        &self.entries
    }

    /// The set of branch components (everything after `<remote>/`), which is
    /// what a health check needs when it asks "is this branch known locally
    /// under any remote?".
    ///
    /// Built from every listed entry, so a symbolic remote `HEAD` contributes
    /// `HEAD` and a `refs/remotes/<name>` spelling contributes an empty
    /// component — exactly the components the reference's split produces.
    pub fn branch_names(&self) -> &BTreeSet<String> {
        &self.branch_names
    }

    /// Whether any listed remote-tracking ref's branch component is exactly
    /// *branch*.
    pub fn contains_branch(&self, branch: &str) -> bool {
        self.branch_names.contains(branch)
    }

    /// Whether Git listed no remote-tracking refs.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The number of listed remote-tracking refs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl GitRepository {
    /// The reference-shaped status observation for this checkout.
    ///
    /// Same owner, parser and error contract as
    /// [`GitRepository::status_with`], with
    /// [`StatusOptions::source_defaults`]: no `--untracked-files` flag, so the
    /// repository's own `status.showUntrackedFiles` decides (Source's
    /// `git status --short`), and ignored entries omitted.
    ///
    /// Native keeps its read-only status hardening (`--no-optional-locks`,
    /// `GIT_OPTIONAL_LOCKS=0`, `core.fsmonitor=false`), so this observation
    /// never refreshes or writes the index. A failed status (including an
    /// invalid `status.showUntrackedFiles` value) is a typed error, never an
    /// empty listing.
    pub fn status_source_observed(&self) -> Result<Vec<StatusEntry>, GitError> {
        self.status_with(&StatusOptions::source_defaults())
    }

    /// Every local remote-tracking ref, in one `for-each-ref` read.
    ///
    /// Reproduces the reference's `refs/remotes/` scan: the same prefix, the
    /// same `<remote>/<branch...>` split at the first `/`, and the same
    /// inclusion of a symbolic remote `HEAD`. A failed read is a typed error;
    /// an empty roster means Git succeeded and there are no remote-tracking
    /// refs (no configured remote has ever been fetched).
    pub fn remote_tracking_roster(&self) -> Result<RemoteTrackingRoster, GitError> {
        let mut entries = Vec::new();
        let mut branch_names = BTreeSet::new();
        for listing in self.refs_under(REMOTES_PREFIX)? {
            let (remote, branch) = {
                let reference = listing.reference();
                let Some(rest) = reference.as_str().strip_prefix(REMOTES_PREFIX) else {
                    return Err(GitError::InvalidOutput {
                        detail: format!(
                            "git listed {reference} outside the requested {REMOTES_PREFIX} prefix"
                        ),
                    });
                };
                match rest.split_once('/') {
                    Some((remote, branch)) => (remote.to_string(), branch.to_string()),
                    None => (rest.to_string(), String::new()),
                }
            };
            branch_names.insert(branch.clone());
            entries.push(RemoteTrackingRef {
                reference: listing.reference().clone(),
                remote,
                branch,
                oid: listing.oid().cloned(),
                symbolic_target: listing.symbolic_target().cloned(),
            });
        }
        Ok(RemoteTrackingRoster {
            entries,
            branch_names,
        })
    }
}
