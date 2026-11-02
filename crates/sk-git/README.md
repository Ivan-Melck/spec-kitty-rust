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
divergence, bounded `ls-remote`). Mutations are split into **plan → apply**:
`plan_*` validates names, paths, protection facts, and the observed state;
`apply_*` re-reads that state, fails with `GitError::PlanStale` if anything
moved, and then performs exactly one bounded git operation (a compare-and-swap
ref transaction, a worktree add/remove, or a merge conclusion commit). Nothing
is retried and there is no unconditional fallback. A refusal leaves the
repository as found; a partial failure reports the observed recovery facts.

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
| `GitDirs`, `Worktree`, `StatusEntry`, `GitPath` | Value types with raw Unix path bytes retained. |

### Remote reads (no remote mutation)

| Item | Purpose |
|------|---------|
| `RemoteName::parse` | Plain configured-remote name only (no URL, scp path, `ext::`, or `-option`). |
| `GitRepository::configured_remotes()` | Configured names; a failure is an error, never an empty list. |
| `GitRepository::resolve_remote(&BranchName)` | `branch.<b>.remote`, then only remote, then `origin`. |
| `GitRepository::probe_remote_heads(&RemoteName, &[BranchName])` | One bounded `ls-remote --heads`; `RemoteHeads::is_absent` is a clean absence. |
| `GitRepository::remote_branch_lookup(&BranchName)` | Tri-state (+ no-remote) across every remote: `Hit`, `CleanMiss`, `Error`, `NoRemote`. |
| `GitRepository::check_branch_freshness(&BranchName)` | `FreshnessVerdict`: up-to-date/behind/ahead/diverged/local-missing/remote-missing/unreachable/no-remote/ambiguous/compare-failed. |
| `RemoteHeads`, `FreshnessVerdict`, `FreshnessState`, `RemoteBranchLookup` | Typed outcomes. |

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
| `WorktreeAddPlan/Outcome`, `WorktreeRemovalPlan/Outcome` | Explicit plans and recovery evidence. |

Removal is destructive, so tracked changes always refuse; untracked files need
`AllowUntracked`; ignored files additionally need `AllowAll`. Placement policy
(where lane worktrees live and which branch a lane receives) remains an
application concern.

### Merge conclusion

| Item | Purpose |
|------|---------|
| `GitRepository::plan_merge_conclusion(expected_head, message)` | Detect exactly one concludeable operation and pin HEAD/message. |
| `GitRepository::apply_merge_conclusion(&plan)` | `git -c commit.gpgsign=false commit [--no-edit \| -m msg]`; no `-a`, pathspec, `--no-verify`, `--allow-empty`, or `--amend`. |
| `ConcludedOperation`, `MergeConclusionPlan/Outcome` | Operation and before/after HEAD evidence. |

A rejected hook leaves the operation in progress; this crate does not abort it.

## Guarantees

- **One bounded subprocess seam.** Arguments are passed as argv entries, never
  through a shell. Ref/OID operands are validated types; ref writes use
  `update-ref --stdin`.
- **Timeout with kill + reap.** A finite bound applies to every invocation. On
  expiry the direct git child is killed and reaped before `GitError::Timeout` is
  returned. Stdout/stderr are bounded per stream (16 MiB) and stdin is bounded.
- **No interactive prompts.** Children get `GIT_TERMINAL_PROMPT=0`, null stdin
  (or a bounded payload), and no pager. A remote read adds ssh
  `-o BatchMode=yes` when the effective transport is plain ssh; a user-pinned
  `GIT_SSH_COMMAND`/`core.sshCommand` is preserved.
- **Read-only probes.** Status uses `--no-optional-locks`, `GIT_OPTIONAL_LOCKS=0`,
  and `-c core.fsmonitor=false`, so it does not refresh/write the index or run a
  repository-configured fsmonitor hook.
- **Explicit repository identity.** Inherited `GIT_DIR`, `GIT_WORK_TREE`,
  `GIT_COMMON_DIR`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, and related
  redirecting variables are removed from every child.
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
  Mission stays in the application layer.
- **No automatic push, fetch, clone, or remote mutation.** Remote contact is a
  bounded `ls-remote` read only.

## Upstream parity and gaps

Reference: upstream `spec-kitty/spec-kitty` at
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`, Python `src/kernel/git` and
`src/specify_cli/git`.

| Upstream | Here | Gap |
|----------|------|-----|
| `kernel/git/runner.py` | `process.rs` seam | Narrower: closed environment profiles, no caller env map. Bounded outputs/stdin added. |
| `kernel/git/listing.py`, `paths.py` | status, worktree, ref reads, raw `GitPath` | No tree/index/numstat/blame/log/diff path listings; `GitPath` component relations not ported. |
| `kernel/git/remote.py` | `ls-remote` probe, divergence, remote resolution | No `fetch_branches`, `fetch_tags`, `clone_repository`, subtree fetch, or tracking-ref update. Freshness compare therefore cannot materialize a missing remote tip. |
| `specify_cli/git/origin_gate.py`, `origin_freshness.py` | `FreshnessVerdict`, `remote_branch_lookup` primitives | No mission/status-log scoping, no fetch-before-compare, no merge-gate policy or refusal messages. Evidence-gate policy stays app-layer. |
| `specify_cli/git/ref_advance.py` | CAS ref transactions, branch advance/resync, unique commits, bookkeeping ref refs | No `advance_branch_ref_for_commit` path-scoped seam, no dedicated `restore_branch_ref`/`resync_checkouts_to_tip`, no `meta.json` VCS-lock-only exemption, no injected residue classifier. |
| `specify_cli/git/destructive_guard.py` | worktree remove, branch delete, strict clean scan | No guarded `reset --hard`, `merge --abort` (including edited-conflict detection), `worktree prune`, or tree deletion. Residue classification is not injected. |
| `specify_cli/git/protection_policy.py` | `ProtectedBranches` fact carrier | Config/YAML/hatch/remote-default resolution deliberately not implemented here. |
| `specify_cli/git/merge_conclusion.py` | merge/revert/cherry-pick conclusion commit | No fresh-linked-worktree proof, squash conclusion, or arbitrary committing-op passthrough. |
| `specify_cli/git/merge_tree_probe.py` | `supports_merge_tree_write_tree` | No merge-tree simulation. |
| `specify_cli/git/report_transaction.py` | ref/commit primitives only | The report transaction, receipt qualification, and commit router are not implemented. |
| `specify_cli/lanes/worktree_allocator.py` | worktree add/attach/remove plans | No sparse-checkout policy, lane layout, branch naming, dependency merges, or ownership/placement rules. |
| `specify_cli/git/commit_helpers.py` | merge conclusion only | No `safe_commit`, path-scoped index transaction, or commit guard. |

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
- User/system Git configuration still affects behaviour (attributes,
  `core.autocrlf`, `status.showUntrackedFiles`, `safe.directory`, hooks invoked
  by a real commit). The adapter does not rewrite configuration.
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

Current count: **111 tests** — 28 unit, 27 existing acceptance, 6 bounded
subprocess, 33 lifecycle, 10 remote lifecycle, 4 remote bounded-process, and 3
compile-only doctests.

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
- merge conflict resolution conclusion, moved HEAD/operation refusal;
- git-dir/common-dir distinction, in-progress markers, version probe.
