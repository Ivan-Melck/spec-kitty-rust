# sk-git

Bounded Git lifecycle adapter for the Spec Kitty Rust workspace.

`sk-git` answers questions about a checkout and performs a narrow, explicit set
of Git lifecycle mutations. It is not a general Git library: there is no
`push`, `fetch`, `clone`, tag update, remote configuration write, hook bypass,
or caller-supplied argv. Every invocation goes through one subprocess seam; every
mutation is prepared as a value that records its expected state and re-reads it
before writing.

## Design in one paragraph

Read-only probes answer questions (`status`, `worktrees`, ref tips, ancestry,
divergence, merge bases, bounded `ls-remote` and remote `HEAD`, and bounded typed
blob/tree reads). Mutations are split into **plan → apply**: `plan_*` validates
names, paths, protection facts, and the observed state; `apply_*` re-reads that
state, fails with `GitError::PlanStale` if anything moved, and then performs
exactly one bounded git operation (a compare-and-swap ref transaction, a worktree
add/remove, a guarded prune of proven-stale registrations, a pinned-head merge
start/abort, a merge conclusion commit, a per-worktree sparse-checkout change, or
a path-scoped safe commit whose tree was built in a private index against a
pinned parent). Nothing is retried and there is no unconditional fallback. A
refusal leaves the repository as found; a partial failure reports the observed
recovery facts. The one deliberate exception is the *post-landing* index reconciliation
of a safe commit, which retries a bounded number of times while another writer
holds git's index lock because the commit already exists and the write is
idempotent.

## Public API

### Discovery and read-only inspection

| Item | Purpose |
|------|---------|
| `GitRepository::open(path)` | Resolve a path (directory or file) to its work-tree root. |
| `GitRepository::open_with_options(path, &GitOptions)` | Same, with a pinned git program and timeout. |
| `GitRepository::root()` / `options()` | Canonicalised work-tree root / the bounding options. |
| `GitRepository::current_branch()` | Short branch name, or `None` on a detached HEAD. |
| `GitRepository::head_commit()` | HEAD commit id, or `None` in an unborn repository. |
| `GitRepository::is_dirty()` / `dirty_paths()` | Any staged, modified, or untracked path. |
| `GitRepository::status()` / `status_with(&StatusOptions)` | Typed porcelain-v1 entries. |
| `GitRepository::worktrees()` | `worktree list --porcelain` entries. |
| `GitRepository::git_dirs()` | Per-worktree git dir and shared common dir (`GitDirs`). |
| `GitRepository::in_progress_operations()` / `is_operation_in_progress()` | Rebase/merge/cherry-pick/revert/sequencer/index-lock markers. |
| `GitRepository::git_version()` / `supports_merge_tree_write_tree()` | Plumbing capability probe (git >= 2.38 for `merge-tree --write-tree`). |
| `GitRepository::read_ref(&RefName)` | Exact ref value, or `None` when absent. |
| `GitRepository::read_branch_tip(&BranchName)` | Local branch tip, or `None`. |
| `GitRepository::resolve_commit(&StartPoint)` | Resolve `HEAD`, a full `Oid`, or a `RefName` to a commit. |
| `GitRepository::commit_exists(&Oid)` | Whether a commit object is present. |
| `GitRepository::is_ancestor(&Oid, &Oid)` | Fast-forward ancestry test. |
| `GitRepository::divergence(&RefName, &RefName)` / `divergence_against_oid(...)` | Ahead/behind counts (`AheadBehind`). |
| `GitRepository::merge_base(&Oid, &Oid)` | Best common ancestor, or `None` for unrelated histories. |
| `GitRepository::tree_entry_at(&StartPoint, &GitPath)` | Exact tree entry (`TreeEntry`: kind, mode, id) at a validated revision. |
| `GitRepository::read_blob_at` / `read_blob_at_bounded` | Exact committed blob bytes, bounded before the body is captured: an 8 MiB default, and an explicit bound up to the 64 MiB canonical status-log bound. |
| `GitRepository::read_blob_at_head` | `BlobLookup::{Present, Absent, NoRevision}`: the rollback-point seam. |
| `GitRepository::tree_paths_at(&StartPoint, prefix)` | `ls-tree -r --name-only -z` paths, literal prefix scoped. |
| `GitRepository::unmerged_paths()` | Conflict evidence (`UnmergedPath`/`UnmergedStage`) from `ls-files -u -z`. |
| `GitRepository::operator_index()` / `operator_index_path()` / `operator_index_lock_path()` | The captured `GIT_INDEX_FILE` authority and its lock. |
| `GitDirs`, `Worktree`, `StatusEntry`, `GitPath`, `TreeEntry`, `BlobLookup` | Value types with raw Unix path bytes retained. |

### Remote reads (no remote mutation)

| Item | Purpose |
|------|---------|
| `RemoteName::parse` | Plain configured-remote name only (no URL, scp path, `ext::`, or `-option`). |
| `GitRepository::configured_remotes()` | Configured names; a failure is an error, never an empty list. |
| `GitRepository::resolve_remote(&BranchName)` | `branch.<b>.remote`, then only remote, then `origin`. |
| `GitRepository::probe_remote_heads(&RemoteName, &[BranchName])` | One bounded `ls-remote --heads`; `RemoteHeads::is_absent` is a clean absence. |
| `GitRepository::remote_branch_lookup(&BranchName)` | Tri-state (+ no-remote) across every remote: `Hit`, `CleanMiss`, `Error`, `NoRemote`. |
| `GitRepository::check_branch_freshness(&BranchName)` | `FreshnessVerdict`: up-to-date/behind/ahead/diverged/local-missing/remote-missing/unreachable/no-remote/ambiguous/compare-failed. |
| `GitRepository::resolve_remote_head(&RemoteName)` | `ls-remote --symref <remote> HEAD` → `RemoteHeadTarget::{Branch, Detached, Unborn}`. |
| `GitRepository::remote_default_branch(&RemoteName)` | The advertised default branch name, when `HEAD` is symbolic. |
| `RemoteHeads`, `FreshnessVerdict`, `FreshnessState`, `RemoteBranchLookup`, `RemoteHead`, `RemoteHeadTarget` | Typed outcomes. |

`ls-remote` is the only contacting command. A failed or timed-out contact is
`GitError::RemoteUnreachable` (or a `FreshnessState::Unreachable` verdict); it is
never an absence. Upstream fetches tracking refs before comparing; this adapter
does not fetch, so a compare whose remote tip object is absent locally is
`FreshnessState::CompareFailed` with that explanation.

### Ref transactions (compare-and-swap)

| Item | Purpose |
|------|---------|
| `RefChange::set/create/delete` | One CAS operation with a required expected value (`None` = must be absent). |
| `GitRepository::plan_ref_transaction(message, changes)` | Validate refs, message, new commits, and every observed value. |
| `GitRepository::apply_ref_transaction(&plan)` | One atomic `update-ref --stdin` transaction; re-reads first. |
| `RefTransactionPlan` / `RefTransactionOutcome` / `RefState` | Planned operations and the previous/applied evidence. |
| `Oid` | Full 40- or 64-hex object id; no abbreviations or revspecs. |
| `BranchName`, `RefName`, `StartPoint` | Validated short/full refs and safe start points. |

Ref operands travel on `update-ref --stdin`, never on the option-parsed command
line. Every write carries the value the caller observed; there is no 2-argument
overwrite, retry, or fallback.

### Branches

| Item | Purpose |
|------|---------|
| `ProtectedBranches` | Caller-resolved protection facts; this crate never reads `.kittify`. |
| `GitRepository::plan_branch_creation` / `apply_branch_creation` | Create at a pinned start commit; refuses an existing branch. |
| `BranchAdvanceOptions` | `reset_clean_checkouts()`, `allow_non_fast_forward()`, `allow_protected_target(reason)`. |
| `GitRepository::plan_branch_advance` / `apply_branch_advance` | CAS advance; fast-forward checked; checkouts dirty-checked before the move and hard-reset only when explicitly allowed. |
| `GitRepository::preflight_branch_deletion` | `BranchDeletionPreflight`: tip, checkouts, unique commits, protection. |
| `BranchDeletionOptions` | `allow_unique_commits(reason)`, `allow_protected_target(reason)`, `preserve_tip_at(RefName)`. |
| `GitRepository::plan_branch_deletion` / `apply_branch_deletion` | CAS delete at the approved tip; checked-out branches always refused; unique-commit verdict re-derived at apply. |
| `GitRepository::unique_commits(branch, ignoring, extra_not)` | Commits reachable from the branch and no other local branch/remote/tag/base. |
| `GitRepository::checkouts_of(&BranchName)` | Registered worktrees that have the branch checked out. |
| `GitRepository::require_clean_checkout(path)` | Strict scan (untracked and ignored included). |

A branch checked out in any worktree is never deleted or advanced without an
explicit resync policy. Preservation refs must live under
`refs/spec-kitty/` (`BOOKKEEPING_REF_PREFIX`) and are created create-only, so an
earlier record is never overwritten.

### Worktrees

| Item | Purpose |
|------|---------|
| `WorktreeStart::NewBranch(StartPoint)` / `ExistingBranch` | Create a branch at a pinned commit, or attach an existing unoccupied branch. |
| `GitRepository::plan_worktree_add` / `apply_worktree_add` | Absolute, normalised, outside git dirs, absent/empty, unregistered; parent created by apply; failure reports whether a branch/registration was left behind. |
| `WorktreeDiscardPolicy` | `RefuseDirty` (default), `AllowUntracked`, `AllowAll`. |
| `GitRepository::plan_worktree_removal` / `apply_worktree_removal` | Exact HEAD/branch/status fingerprint; locked, primary, operation-in-progress, and tracked changes refused; fingerprint rechecked at apply. |
| `GitRepository::plan_worktree_prune` / `apply_worktree_prune` | Prune exactly the registrations git proves stale (missing directory, `prunable`, known admin id); live checkouts and the primary are never removed. |
| `WorktreeAddPlan/Outcome`, `WorktreeRemovalPlan/Outcome`, `WorktreePrunePlan/Outcome` | Explicit plans and recovery evidence. |

Removal is destructive, so tracked changes always refuse; untracked files need
`AllowUntracked`; ignored files additionally need `AllowAll`. Placement policy
(where lane worktrees live and which branch a lane receives) remains an
application concern.

`worktree prune` is repository-wide, so the prune plan enumerates every
candidate, refuses one whose directory still exists, and requires git's own
`--dry-run` to name exactly the planned administrative ids before the real
prune. A registration that appeared, recovered, or vanished between plan and
apply is `PlanStale`.

### Sparse checkout (per worktree)

| Item | Purpose |
|------|---------|
| `GitRepository::sparse_checkout_state()` | Configured mode and logical patterns, with the per-worktree `info/sparse-checkout` path. |
| `GitRepository::plan_sparse_checkout` / `apply_sparse_checkout` | Validate patterns, pin `HEAD`, re-read the previous state, then `git sparse-checkout set [--cone\|--no-cone] --stdin`. |
| `GitRepository::set_sparse_checkout(mode, patterns)` | Plan and apply in one call. |
| `GitRepository::disable_sparse_checkout()` | Restore the full work tree; a non-sparse checkout is `SparseCheckoutNotConfigured`. |
| `GitRepository::skip_worktree_paths()` | Every index entry currently marked skip-worktree. |
| `GitRepository::sparse_pattern_file_text()` | The raw per-worktree pattern file (read-only, comments preserved, bounded by `MAX_SPARSE_PATTERN_FILE_BYTES`). |
| `SparseCheckoutMode`, `SparseCheckoutPlan/Outcome/State` | Typed mode, plan and read-back state. |

The pattern file and mode are resolved through the target checkout's own git
directory, so changing a lane never rewrites the primary checkout's (or a
sibling lane's) sparse state. No `--force` is ever passed: git's own refusal to
remove locally modified paths, and its preservation of untracked/ignored data,
stay in force. A merge, rebase, cherry-pick, revert, sequencer run, held index
lock, or unmerged index refuses a sparse change.

### Merge start, conclusion, abort

| Item | Purpose |
|------|---------|
| `GitRepository::plan_merge_start(&StartPoint)` | Pin HEAD, branch, the other commit, and the merge base; refuse an in-progress operation, an unborn HEAD, or a dirty tracked checkout. |
| `GitRepository::apply_merge_start(&plan)` | `git -c merge.autoStash=false merge --no-ff --no-commit <theirs>`; HEAD cannot move, conflicts come from real index stages. |
| `GitRepository::merge_start(&StartPoint)` | Plan and apply in one call. |
| `GitRepository::plan_merge_conclusion(expected_head, message)` | Detect exactly one concludeable operation and pin the checkout, its branch, HEAD, the operation tip, and the message. |
| `GitRepository::apply_merge_conclusion(&plan)` | `git -c commit.gpgsign=false commit [--no-edit \| -m msg]`; no `-a`, pathspec, `--no-verify`, `--allow-empty`, or `--amend`. Returns the commit **this call created**, proven from invocation-bound evidence (git's own summary, or a reflog entry appended after the pre-invocation publication watermark on the pinned ref), never a post-hook `HEAD` read and never a parent-set match alone. |
| `GitError::MergeConclusionUnconfirmed` | The commit may have landed but the post-state is not the pinned one (a hook moved `HEAD`, rewrote or re-created the operation marker, or a readback failed). Carries the proven created commit when there is one (`None` means unknown, not "not landed"), the observed `HEAD`, the marker state (`Consumed`/`Unchanged`/`Changed`/`Unreadable`), and git's diagnostic; nothing is rolled back. |
| `GitRepository::plan_merge_abort()` / `apply_merge_abort(&plan)` | Pin `HEAD`, `MERGE_HEAD`, conflicts, and the full status fingerprint; refuse an edited/resolved merge, then `git merge --abort`. |
| `ConcludedOperation`, `MergeStartPlan/Outcome`, `MergeAbortPlan/Outcome`, `MergeConclusionPlan/Outcome` | Operation, conflict and before/after HEAD evidence. |

`MergeStartOutcome::status()` is `AlreadyUpToDate`, `Stopped`, or `Conflicted`;
a conflicted start carries the stage-1/2/3 entries read from the index, not text
scraped from git's stderr. A rejected hook leaves the operation in progress; use
the guarded abort (or conclude after resolving) deliberately.

Conclusion identity is evidence-based and invocation-bound. Hooks still run
(nothing is disabled and `--no-verify` is never passed), so a `post-commit` hook
can move the branch after git wrote the commit, even onto a pre-existing commit
with the same parents. The created commit is therefore identified only from
git's own commit summary or from a reflog entry appended to the **pinned** ref
after a publication snapshot captured before the command ran. The real parent
set (HEAD plus `MERGE_HEAD` for a merge; HEAD alone for a cherry-pick or revert)
is a necessary check, but a parent match alone is never creation proof and the
observed `HEAD` is never accepted as a candidate. Fallback requires complete
pre/post histories below the 256-entry bound, with the prior snapshot preserved
as the exact suffix; only the appended count is eligible. Duplicate record bytes
cannot move this boundary. Rewritten, truncated or saturated history is unbound;
Git's own summary remains usable on ordinary long-history repositories. Hook `commit (amend)` publications are not
the original conclusion and are excluded from fallback creation evidence.
`Ok` is returned only when that proven commit is also the current `HEAD` and the
operation marker is consumed; anything else is `MergeConclusionUnconfirmed`.
A completed `git commit` that fails with `HEAD` still pinned, the marker still
recording the pinned tip, and no identified created commit is a plain
`CommandFailed` (the ordinary rejecting-`pre-commit`/`commit-msg` shape). When
history cannot be bound, this is not proof that no commit object ever existed. A command that started and then
failed without a status (timeout, capture bound, readback I/O) may already have
landed, so it is reported as unconfirmed evidence instead of a retryable error.

### Captured operator index

| Item | Purpose |
|------|---------|
| `GitRepository::operator_index()` | `OperatorIndex`: source (`Default`/`Environment`), the captured spelling, the absolute path, and the lock path. |
| `GitRepository::has_captured_index()` | Whether `GIT_INDEX_FILE` was inherited at open. |
| `GitRepository::operator_index_path()` / `operator_index_lock_path()` | The index and `.lock` every operator-index operation uses. |

`GIT_INDEX_FILE` is read **once**, when the repository is opened. A relative
value is resolved against the discovered work-tree root (git's own semantics);
the spelling is then handed to every child verbatim, so a symlinked index path
keeps its spelling and git's own follow-through behaviour. Nothing is written to
the process environment, `<git-dir>/index` is never silently substituted, and
every operator-index read, write, lock check and post-commit reconciliation uses
the captured path. A plan records its authority and refuses to apply under a
different one. Private scoped indexes override the captured value only for the
candidate-tree commands. Every *other* repository-redirecting variable
(`GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, object/namespace selectors, …)
remains an explicit `RepositoryEnvironmentRefused` until one shared repository
authority owns both lock discovery and the opened adapter.

### Safe commit (path-scoped index transaction)

| Item | Purpose |
|------|---------|
| `SafeCommitRequest::new(destination, message, paths)` | Commit exactly `paths` on the declared branch. |
| `.with_index_deletions(paths)` | Commit index-only removals (`git rm --cached` semantics); the files stay on disk. |
| `.with_expected_parent(oid)` | Pin the parent; the tree is built against it and the ref advances only by compare-and-swap. |
| `.with_expected_bytes([ExpectedBlob])` | Assert exact staged blob bytes (requires a pinned parent). |
| `.with_protection(&ProtectedBranches, CommitGuardCapability)` | Supply the caller-resolved protected set and the asserted capability. |
| `GitRepository::plan_safe_commit(&request)` | Validate destination/HEAD/parent, message, paths, protection, and in-progress state; no mutation. |
| `GitRepository::apply_safe_commit(&plan)` | Re-check the observed state, then run the private-index transaction. |
| `GitRepository::safe_commit(&request)` | Plan and apply in one call. |
| `SafeCommitPlan` / `SafeCommitOutcome` | Observed HEAD/tip, route, normalised paths, guard verdict; landed commit, committed path set, warnings. |
| `SafeCommitRecovery::repair_argv` | Exact argv (no shell quoting) to repair the index after a landed-but-unreconciled commit. |
| `evaluate_commit_guard(&BranchName, &ProtectedBranches, CommitGuardCapability)` | The pure protection decision (mirrors upstream `commit_guard.evaluate`). |
| `normalize_safe_commit_path(root, path)` | Resolve absolute/relative caller paths, keep raw bytes, refuse escape and symlink loops. |

`SafeCommitRoute::Worktree` (the default) seeds a private index from `HEAD`,
stages only the requested paths, runs the repository's commit hooks with
`git hook run` against that index, writes the tree, creates the commit with
`git commit-tree -p <observed HEAD>`, and advances the branch with a
compare-and-swap. The real index is not written until the commit exists; it is
then resynchronised for exactly the requested paths. Unrelated staged, partially
staged, unstaged, and untracked content is never read, moved, stashed, or
restored. `SafeCommitRoute::IndexDeletions` is the same transaction plus
`git rm --cached` removals. `SafeCommitRoute::PinnedParent` additionally
re-isolates the tree around the hooks, so a hook cannot add paths or change
requested content, and verifies optional exact raw bytes.

A commit that lands but whose index reconciliation or readback fails is
`GitError::SafeCommitLanded`, carrying the commit id and the paths to repair.
A hook that sweeps tracked files into the commit is reported the same way
(`SafeCommitLandedKind::UnrequestedPaths`) after the extra index entries are
resynchronised. A losing compare-and-swap is `GitError::PlanStale`: no ref
moved and no user state changed. A rejected or failing hook is
`GitError::SafeCommitHookRejected` with its stderr.

## Guarantees

- **One bounded subprocess seam.** Arguments are passed as argv entries, never
  through a shell. Ref/OID operands are validated types; ref writes use
  `update-ref --stdin`.
- **Timeout with kill + reap.** A finite bound applies to every invocation. On
  expiry the direct git child is killed and reaped before `GitError::Timeout` is
  returned. Stdout/stderr are bounded per stream (16 MiB; a bounded blob read may
  request its own limit up to 64 MiB, which is still enforced, never truncated)
  and stdin is bounded.
- **No interactive prompts.** Children get `GIT_TERMINAL_PROMPT=0`, null stdin
  (or a bounded payload), and no pager. A remote read adds ssh
  `-o BatchMode=yes` when the effective transport is plain ssh; a user-pinned
  `GIT_SSH_COMMAND`/`core.sshCommand` is preserved.
- **Read-only probes.** Status uses `--no-optional-locks`, `GIT_OPTIONAL_LOCKS=0`,
  and `-c core.fsmonitor=false`, so it does not refresh/write the index or run a
  repository-configured fsmonitor hook.
- **Explicit repository identity.** Inherited `GIT_DIR`, `GIT_WORK_TREE`,
  `GIT_COMMON_DIR`, `GIT_OBJECT_DIRECTORY`, and related redirecting variables
  are removed from every child. `GIT_INDEX_FILE` is the single captured
  exception: it is bound to the handle at open and re-applied verbatim (see
  [Captured operator index](#captured-operator-index)).
- **Raw paths.** Porcelain status and worktree listings are parsed from `-z`
  records. On Unix paths are exact `OsString` bytes; `GitPath` display is lossy
  by name and never parsed back.
- **Failure is never absence.** Only git's confirmed detached/unborn answer is
  `None`; everything else is a typed error. Remote failure is never a clean
  miss.
- **Every mutation is planned.** Expected ref values, HEAD, worktree
  fingerprints, protected facts, and unique-commit verdicts are recorded and
  rechecked. `PlanStale` carries what was expected and what was observed.
- **No protected/default-branch shortcut.** Advancing or deleting a branch in
  the caller-supplied protected set requires an explicit, non-empty override
  reason. Resolving that set from configuration, the remote default branch, or a
  Mission stays in the application layer. A safe commit is refused on a
  protected destination unless the call site asserts a bookkeeping
  `CommitGuardCapability`; the decision itself is the pure
  `evaluate_commit_guard`.
- **Path-scoped commits.** A safe commit builds its tree in a private index
  seeded from the pinned parent, so unrelated index content can never enter the
  commit. Pathspecs are literal (`--literal-pathspecs` plus `--`), raw path
  bytes are preserved, and the real index is only reconciled for the requested
  paths after the commit exists. A commit built against a stale parent is
  refused by the ref compare-and-swap; it is never landed on a moved branch.
- **Bounded committed reads.** Blob bodies are size-checked with
  `git cat-file -s` before capture; a blob over the caller's bound is
  `GitError::BlobTooLarge`. The default bound is 8 MiB; an explicit bound may go
  up to 64 MiB (the canonical status-log bound), and that bound is plumbed
  through the subprocess seam as the child's capture limit, so an accepted log
  is returned whole. Paths and revisions are validated types, tree paths are
  literal and matched exactly, and an unreadable `HEAD` is
  `GitError::UnreadableHead`, never "absent". A `HEAD` that is garbage, empty, or
  names an invalid ref is unreadable at `open` (git itself reports such a
  repository as "not a git repository"), and an empty symbolic target is
  unreadable at read time rather than a generic command failure.
- **No automatic push, fetch, clone, or remote mutation.** Remote contact is a
  bounded `ls-remote` read only (including `--symref ... HEAD`).

## Upstream parity and gaps

Reference: upstream `spec-kitty/spec-kitty` at
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`, Python `src/kernel/git` and
`src/specify_cli/git`.

| Upstream | Here | Gap |
|----------|------|-----|
| `kernel/git/runner.py` | `process.rs` seam | Narrower: closed environment profiles, no caller env map. Bounded outputs/stdin added. |
| `kernel/git/listing.py`, `paths.py` | status, worktree, ref reads, raw `GitPath`, typed tree/blob reads, merge base | No index/numstat/blame/log/diff listings; `GitPath` component relations not ported. |
| `kernel/git/remote.py` | `ls-remote` probe, divergence, remote resolution | No `fetch_branches`, `fetch_tags`, `clone_repository`, subtree fetch, or tracking-ref update. Freshness compare therefore cannot materialize a missing remote tip. |
| `specify_cli/git/origin_gate.py`, `origin_freshness.py` | `FreshnessVerdict`, `remote_branch_lookup` primitives | No mission/status-log scoping, no fetch-before-compare, no merge-gate policy or refusal messages. Evidence-gate policy stays app-layer. |
| `specify_cli/git/ref_advance.py` | CAS ref transactions, branch advance/resync, unique commits, bookkeeping ref refs | No `advance_branch_ref_for_commit` path-scoped seam, no dedicated `restore_branch_ref`/`resync_checkouts_to_tip`, no `meta.json` VCS-lock-only exemption, no injected residue classifier. |
| `specify_cli/git/destructive_guard.py` | worktree remove, guarded worktree prune, branch delete, strict clean scan, pinned merge start/abort with edited-conflict detection | No guarded `reset --hard` or tree deletion. Residue classification is not injected. |
| `specify_cli/git/protection_policy.py` | `ProtectedBranches` fact carrier | Config/YAML/hatch/remote-default resolution deliberately not implemented here. |
| `specify_cli/git/merge_conclusion.py` | merge start (`--no-ff --no-commit`), merge/revert/cherry-pick conclusion commit, guarded abort | No fresh-linked-worktree proof, squash conclusion, or arbitrary committing-op passthrough. |
| `specify_cli/git/merge_tree_probe.py` | `supports_merge_tree_write_tree` | No merge-tree simulation. |
| `specify_cli/git/report_transaction.py` | ref/commit primitives only | The report transaction, receipt qualification, and commit router are not implemented. |
| `specify_cli/lanes/worktree_allocator.py` | worktree add/attach/remove plans, per-worktree sparse-checkout operations, pinned-head dependency merge start | No lane layout, branch naming, dependency-merge composition, or ownership/placement rules. |
| `specify_cli/git/commit_helpers.py` | `safe_commit`, private-index path-scoped transaction, expected parent, index deletions, expected raw bytes, protected-branch guard, landed-but-unreconciled recovery, captured `GIT_INDEX_FILE` operator-index authority | No `SafeCommitBackstopError` whole-index staging probe (deleted upstream), no owned-checkout/decision-lock restriction on index-only removals, no mission-scoped `commit_to_target` fold or config/hatch resolution (application layer), no report transaction / receipt qualification / commit router, no CLI wiring. |

Passing tests establish the covered contracts, not blanket Python
compatibility.

## Limits

- Requires a work tree. Bare repositories are rejected with
  `GitError::BareRepository`.
- Worktree listing prefers `git worktree list --porcelain -z` (git 2.36+) and
  falls back to newline-delimited porcelain output; a path containing a newline
  cannot be represented by the older format.
- Repository-root discovery uses `git rev-parse --show-toplevel`, which cannot
  represent a root ending in a newline. Ref names are UTF-8 `String`s; paths are
  not subject to that restriction on Unix.
- The timeout kills the direct git child only. Remote helpers and other
  descendants are outside the bound.
- Guarded worktree prune needs Git >= 2.31 (`worktree prune --expire`);
  sparse-checkout operations need Git >= 2.26. Older Git is refused with
  `GitError::Unsupported`, never silently skipped.
- User/system Git configuration still affects behaviour (attributes,
  `core.autocrlf`, `status.showUntrackedFiles`, `safe.directory`, commit hooks).
  The adapter does not rewrite configuration. A safe commit runs hooks with
  `git hook run`, which needs Git >= 2.36; older Git is refused with
  `GitError::Unsupported` rather than skipping hooks. Its exact-bytes assertion
  is bounded by the seam's 4 MiB stdin budget.
- A `SafeCommitRequest` starts with an **empty** protected set and
  `CommitGuardCapability::Standard`. The application layer must resolve and
  attach the real facts with `with_protection`; the crate never reads
  `.kittify/config.yaml`, the remote default branch, or the operator hatch.
- `apply_worktree_add` creates missing parent directories before git runs; a
  failed `worktree add` can leave an empty parent directory.
- A failed branch delete after a preservation ref was created leaves that
  `refs/spec-kitty/` ref in place as recovery evidence.
- `apply_branch_advance` moves the ref first and then resets checkouts. If a
  reset fails, `GitError::ResyncFailed` reports the checkout that is behind its
  own HEAD; there is no automatic rollback.
- Plan validation reduces but does not eliminate filesystem races. Correctness
  for refs rests on the CAS; worktree removal rechecks a fingerprint but is not
  an OS-level capability sandbox.

## Tests

All tests use real temporary Git repositories (or a fake `git` executable for
bounded-process assertions). No test touches the workspace checkout or a user
remote. They need Git >= 2.28 and, for the `/proc` reap assertion, Linux; the
fake-executable tests are Unix-only.

```bash
CARGO_BUILD_JOBS=2 cargo test -p sk-git
CARGO_BUILD_JOBS=2 cargo clippy -p sk-git --all-targets -- -D warnings
cargo fmt -p sk-git -- --check
```

Current count: **232 tests** (3 ignored subprocess helpers exercised by their
parents) — 37 unit, 27 acceptance, 7 bounded subprocess, 6 captured-index
authority (+1 helper), 36 lifecycle, 6 merge start/conclusion/abort, 9
tree/blob/merge-base reads, 10 remote lifecycle, 5 remote bounded-process, 5
remote `HEAD`, 57 safe-commit acceptance, 1 retained-selector authority (+1
helper), 2 safe-commit contention, 2 scoped-safe-commit conformance (+1 helper,
one of which runs the committed 32-case oracle corpus), 8 pinned safe-commit
differential, 6 sparse checkout, 4 worktree prune, and 4 doctests.

Coverage highlights:

- ref option/revspec injection refusal; full-OID and ref validation; tag-object
  ref writes refused;
- CAS create/update/delete, atomic multi-ref transactions, stale expected values;
- refs/worktrees moving between plan and apply, with `PlanStale` evidence;
- fast-forward and explicit non-fast-forward advance, protected-target override,
  clean-checkout resync, dirty checkout refusal, unborn zero-old CAS;
- guarded deletion: unique commits, checked-out branches, protected targets,
  tip preservation in `refs/spec-kitty/`, unique verdict changing after planning;
- worktree add/attach, unsafe and occupied path refusal, clean/untracked/ignored/
  tracked removal policies, locked and detached worktrees, non-UTF-8 paths;
- remote hit vs clean miss vs unreachable vs timeout vs output cap vs no remote;
  freshness up-to-date/ahead/behind/diverged/local-missing/remote-missing/
  compare-failed/ambiguous;
- merge conflict resolution conclusion, moved HEAD/operation refusal, a
  `post-commit` hook that moves `HEAD` (reported as unconfirmed with the proven
  created commit), a hook that re-creates the operation marker (unconfirmed), a
  rejecting `pre-commit` hook (refusal, operation still in progress), a prior
  identical merge in the reflog that a rejected conclusion must not claim, a
  hook that repoints `HEAD` at a foreign same-parent commit after expiring
  reflogs and pruning the created object (unconfirmed, no candidate), a noisy
  hook that cannot inject a fake summary, an amending `post-commit` hook, a
  post-commit hook that overruns the capture bound (typed unconfirmed, not a
  retryable failure), a failing `post-commit` hook that cannot change a
  confirmed landing, and valid merge, cherry-pick and revert conclusions;
- pinned-head merge start (`--no-ff --no-commit`) with real stage-1/2/3 conflict
  evidence, guarded abort that refuses an edited merge, abort-with-untracked
  preservation, and a tip-pinned conclusion;
- bounded typed tree/blob reads (literal and prefix-scoped paths, symlinks,
  size bounds including the full 64 MiB bound and a 17 MiB log above the old
  16 MiB capture cap), merge bases, and the distinguished
  `BlobLookup::{Present,Absent,NoRevision}` HEAD read with `UnreadableHead` for a
  corrupt HEAD (non-UTF-8 target, garbage, empty, empty symbolic target, invalid
  ref);
- captured `GIT_INDEX_FILE`: absolute, root-relative and symlinked spellings,
  untouched `<git-dir>/index`, authority-bound plans, captured-index lock
  checks, and unchanged process environment;
- guarded worktree prune (proven-stale only, recovered registration refusal,
  live checkout preservation) and per-worktree sparse checkouts (cone/no-cone,
  untracked preservation, primary/sibling isolation, disable restore);
- remote `HEAD` resolution: symbolic default branch, detached remote, unborn
  remote, and typed unreachable failure;
- safe commit: exact-path staging, unrelated staged/partial/unstaged/untracked
  preservation, path bytes (including non-UTF-8), literal-pathspec injection,
  rename pairs, ignored files, symlinked files/directories, gitlinks, index-only
  deletions, expected parent + raw bytes, protected-branch capability refusal,
  hook rejection vs empty changeset, hook-swept extras, landed-but-unresynced
  recovery, held-lock refusal, concurrent contention, and pinned differential
  outcomes from the upstream Python oracle;
- git-dir/common-dir distinction, in-progress markers, version probe.
