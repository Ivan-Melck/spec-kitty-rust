# Git operations continuation — implementation report

> **Scope.** This report covers `crates/sk-git/**` only, from the committed
> baseline `d64468a76`. It is implementation evidence, not an independent review
> and not a whole-project or CLI-parity claim. No workspace manifest, Kernel,
> Mission, CLI or fixture outside `crates/sk-git` was edited; nothing was
> committed, pushed, released, benchmarked or measured for speed. Verification is
> scoped (`cargo test/clippy/fmt -p sk-git`) with bounded jobs on the existing
> target directory.

Reference snapshot: upstream `spec-kitty/spec-kitty` at
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`, read through the lock-exact
`.venv-rust-oracle/bin/python` (`PYTHONPATH=src`,
`SPEC_KITTY_PACKS_ROOT=$PWD/packs`). Python is development tooling only; no
native path invokes it.

## 1. Delivered capabilities

These are real, executable behaviours with plan/apply or bounded-read
semantics — not projections of the Python API. Each row names the tests that
exercise it in a throw-away repository.

| Capability | Exact surface | Tests |
| --- | --- | --- |
| Captured operator index (`GIT_INDEX_FILE`) | `OperatorIndex`, `OperatorIndexSource`; `GitRepository::{operator_index, has_captured_index, operator_index_path, operator_index_lock_path}` | `tests/index_authority.rs` (6 + 1 helper), unit tests in `src/process.rs`, `src/safe_commit.rs` |
| Bounded typed tree/blob reads | `tree_entry_at`, `read_blob_at`, `read_blob_at_bounded`, `tree_paths_at`, `unmerged_paths`, `TreeEntry`/`TreeEntryKind`, `UnmergedPath`/`UnmergedStage` | `tests/objects.rs` (9) |
| Distinguished HEAD read (rollback seam) | `read_blob_at_head`, `BlobLookup::{Present, Absent, NoRevision}`, `GitError::UnreadableHead`, `GitError::BlobTooLarge` | `tests/objects.rs` |
| Real merge base | `merge_base(&Oid, &Oid) -> Option<Oid>` | `tests/objects.rs` |
| Pinned-head merge start | `plan_merge_start`, `apply_merge_start`, `merge_start`, `MergeStartPlan/Outcome/Status`, `merge_head()` | `tests/merge_start.rs` (6) |
| Safe merge conclusion (tip-pinned) | `MergeConclusionPlan::expected_operation_head`, re-read at apply | `tests/merge_start.rs` |
| Guarded merge abort | `plan_merge_abort`, `apply_merge_abort`, `merge_abort`, `MergeAbortPlan/Outcome`, `GitError::MergeAbortFailed`, `MergeStateChanged` | `tests/merge_start.rs` |
| Guarded prune of proven-stale registrations | `plan_worktree_prune`, `apply_worktree_prune`, `WorktreePruneEntry/Plan/Outcome`, `GitError::WorktreeNotStale` | `tests/worktree_prune.rs` (4) |
| Per-worktree sparse checkout | `sparse_checkout_state`, `plan_sparse_checkout`, `apply_sparse_checkout`, `set_sparse_checkout`, `disable_sparse_checkout`, `skip_worktree_paths`, `SparseCheckoutMode/Plan/Outcome/State` | `tests/sparse_checkout.rs` (6) |
| Bounded no-prompt remote HEAD | `resolve_remote_head`, `remote_default_branch`, `RemoteHead`, `RemoteHeadTarget` | `tests/remote_head.rs` (5) |

Supporting changes:

* `process.rs`: one new `GitProcess::new_with_index` binding, one captured
  `GIT_INDEX_FILE` re-applied per child. `GIT_INDEX_FILE` was removed from the
  repository-redirecting refusal list; every other selector remains refused.
* `repo_state.rs`: the held-index lock is resolved through the captured
  authority, not `<git-dir>/index.lock`.
* `safe_commit.rs` / `safe_commit_transaction.rs`: plans record the authority;
  apply refuses a different one; the preflight lock check uses the authority's
  `.lock`.
* `merge.rs`: the conclusion plan now pins the operation tip (`MERGE_HEAD`,
  `CHERRY_PICK_HEAD`, `REVERT_HEAD`) in addition to HEAD, and re-reads it.
* `error.rs`: `UnreadableHead`, `BlobTooLarge`, `WorktreeNotStale`,
  `MergeStateChanged`, `MergeAbortFailed`, `SparseCheckoutFailed`,
  `SparseCheckoutNotConfigured`, `SparsePatternFileTooLarge`. The state read uses
  `sparse-checkout list`; `sparse_pattern_file_text()` returns the raw
  user-authored file (comments included) bounded by
  `MAX_SPARSE_PATTERN_FILE_BYTES`.
* `lib.rs`/`README.md`: exports and accurate capability documentation.

## 2. Exact APIs for workspace integration

Signatures below are the public surface the coordinator can call today. All
errors are `sk_git::GitError` unless noted.

### 2.1 Restoring a coordination surface from a tip

```rust
pub fn tree_entry_at(&self, revision: &StartPoint, path: &GitPath)
    -> Result<Option<TreeEntry>, GitError>;
pub fn read_blob_at(&self, revision: &StartPoint, path: &GitPath)
    -> Result<Option<Vec<u8>>, GitError>;
pub fn read_blob_at_bounded(&self, revision: &StartPoint, path: &GitPath, limit: u64)
    -> Result<Option<Vec<u8>>, GitError>;          // limit <= MAX_BLOB_BYTES (16 MiB)
pub fn tree_paths_at(&self, revision: &StartPoint, prefix: Option<&GitPath>)
    -> Result<Vec<GitPath>, GitError>;
```

`StartPoint` is `Head`, a full `Oid`, or a validated `RefName`. Paths are
`GitPath` raw repository-relative bytes; the path is passed under
`--literal-pathspecs` and matched exactly against `-z` output, so `dir` never
answers for `dir2/…` and a literal `:(glob)*.txt` stays data. A missing path is
`Ok(None)`, not an error. A tree, submodule gitlink, or the root has no blob
body (`Ok(None)` / `TreeEntryKind::{Tree,Gitlink}`).

Use case: replace `COORD_SURFACE_RESTORE_REQUIRED`. Read the coordination tip's
`kitty-specs/<mission>/meta.json` with `read_blob_at(&StartPoint::Ref(tip_ref), …)`
and enumerate the mission directory with
`tree_paths_at(&StartPoint::Ref(tip_ref), Some(&mission_prefix))`, then restore
only those exact paths. The seed-trailer probe becomes
`read_blob_at(&tip, &GitPath::from_git_bytes(b"…/.seed").unwrap())`.

### 2.2 The `sk-status` rollback seam (requested by the status owner)

```rust
pub fn read_blob_at_head(&self, path: &GitPath) -> Result<BlobLookup, GitError>;
pub fn read_blob_at_head_bounded(&self, path: &GitPath, limit: u64)
    -> Result<BlobLookup, GitError>;

pub enum BlobLookup {
    Present(Vec<u8>),   // exact committed bytes
    Absent,             // HEAD exists; path is not a blob in it
    NoRevision,         // repository has no HEAD commit (unborn branch)
}
```

Intended `sk-status::mutation_rollback::capture_rollback_point` usage — replace
the injected `HeadCommittedProbe` shell workaround with:

```rust
let repository = sk_git::GitRepository::open(&root)?;
let log = sk_git::GitPath::from_git_bytes(b".kittify/events/status.events.jsonl")?;
match repository.read_blob_at_head(&log)? {
    sk_git::BlobLookup::Present(bytes) => HeadCommitted::Present(bytes),
    sk_git::BlobLookup::Absent => HeadCommitted::Absent,
    sk_git::BlobLookup::NoRevision => HeadCommitted::NoCommit,
}
```

Error semantics (the three absences stay distinct):

| Observation | Answer |
| --- | --- |
| `HEAD` resolves; path is a blob | `Present(exact bytes)` |
| `HEAD` resolves; path absent, deleted, a tree, or a gitlink | `Absent` |
| No `HEAD` commit (unborn branch) | `NoRevision` |
| `HEAD` malformed, names an invalid ref, or is detached at a missing/non-commit object | `Err(GitError::UnreadableHead { detail })` |
| Blob larger than the bound | `Err(GitError::BlobTooLarge { path, limit })` |
| git missing/failed/timed out | the matching typed error (`CommandFailed`, `GitUnavailable`, `Timeout`, …) |

`read_blob_at_head` reads only the commit tree: it never consults the index or
the work tree, so concurrent uncommitted operator edits cannot change the
rollback point, and the captured operator index does not affect it. There is no
shell fallback and no path quoting: the path is data. For a non-`HEAD` revision
(for example the recorded `HEAD` of a detached rollback point) use
`read_blob_at(&StartPoint::Oid(recorded), &path)`.

`rollback_events_log` can pair this with `plan_ref_transaction`/`apply_ref_transaction`
and `plan_safe_commit` for the write side; nothing in this crate writes a
rollback record itself.

### 2.3 Non-fast-forward dependency/planning merges

```rust
pub fn plan_merge_start(&self, theirs: &StartPoint) -> Result<MergeStartPlan, GitError>;
pub fn apply_merge_start(&self, plan: &MergeStartPlan) -> Result<MergeStartOutcome, GitError>;
pub fn merge_start(&self, theirs: &StartPoint) -> Result<MergeStartOutcome, GitError>;
pub fn plan_merge_conclusion(&self, expected_head: &Oid, message: Option<&str>)
    -> Result<MergeConclusionPlan, GitError>;
pub fn apply_merge_conclusion(&self, plan: &MergeConclusionPlan)
    -> Result<MergeConclusionOutcome, GitError>;
pub fn plan_merge_abort(&self) -> Result<MergeAbortPlan, GitError>;
pub fn apply_merge_abort(&self, plan: &MergeAbortPlan) -> Result<MergeAbortOutcome, GitError>;
pub fn merge_abort(&self) -> Result<MergeAbortOutcome, GitError>;
pub fn merge_head(&self) -> Result<Option<Oid>, GitError>;
```

`MergeStartOutcome::status()` is `AlreadyUpToDate | Stopped | Conflicted`;
`Conflicted` carries `conflicts(): &[UnmergedPath]` with stage-1/2/3 mode+oid
evidence. The merge is `git -c merge.autoStash=false merge --no-ff --no-commit
<theirs>`; `HEAD` cannot move by construction. A start refuses any in-progress
operation, an unborn HEAD, and a dirty **tracked** checkout (`DirtyWorktree`);
untracked/ignored user files do not block it and are preserved. The abort plan
pins `HEAD`, `MERGE_HEAD`, conflicts and the whole status fingerprint, so an
edited or resolved merge is `PlanStale`, never silently discarded.

Workspace mapping: replace `MergeRequired` from `integrate_tip` /
`integrate_dependency_lanes` with `merge_start`, then
`plan_merge_conclusion(plan.head(), Some(…))` when clean (or `merge_abort` on a
conflict the caller will not resolve). The conflict path list is real evidence
for the caller's refusal, not a parsed message.

### 2.4 Lane ancestry without the `is_ancestor` approximation

```rust
pub fn merge_base(&self, left: &Oid, right: &Oid) -> Result<Option<Oid>, GitError>;
```

`None` means "unrelated histories", which two `is_ancestor` calls cannot
represent. Use it in `LaneAllocator::guard_planning_ancestry` in place of the
two-direction approximation.

### 2.5 Interrupted materialization repair

```rust
pub fn plan_worktree_prune(&self) -> Result<WorktreePrunePlan, GitError>;
pub fn apply_worktree_prune(&self, plan: &WorktreePrunePlan)
    -> Result<WorktreePruneOutcome, GitError>;
```

A candidate is pruned only when git reports it `prunable`, its checkout
directory is absent, its administrative directory is known, and it is not the
primary checkout. Apply re-derives the set, requires git's `--dry-run` to name
exactly the planned administrative ids, runs `worktree prune --expire now`, then
verifies every planned registration is gone and every live registration is
unchanged. A recovered candidate or a newly stale one is `PlanStale`. Requires
git ≥ 2.31 (`--expire`), otherwise `Unsupported`.

Workspace mapping: an interrupted `COORD_SURFACE_RESTORE_REQUIRED`/conflict can
plan a prune of only the dead registration, then re-run materialization. It does
not remove live directories, does not delete a checkout with a directory, and
does not touch the primary.

### 2.6 Per-worktree lane sparse checkouts

```rust
pub fn sparse_checkout_state(&self) -> Result<SparseCheckoutState, GitError>;
pub fn plan_sparse_checkout(&self, mode: SparseCheckoutMode, patterns: &[String])
    -> Result<SparseCheckoutPlan, GitError>;
pub fn apply_sparse_checkout(&self, plan: &SparseCheckoutPlan)
    -> Result<SparseCheckoutOutcome, GitError>;
pub fn set_sparse_checkout(&self, mode: SparseCheckoutMode, patterns: &[String])
    -> Result<SparseCheckoutOutcome, GitError>;
pub fn disable_sparse_checkout(&self) -> Result<SparseCheckoutOutcome, GitError>;
pub fn skip_worktree_paths(&self) -> Result<Vec<GitPath>, GitError>;
```

Call these on a `GitRepository` opened at the **lane worktree** path. The
pattern file and `core.sparseCheckout{,Cone}` are per worktree (resolved through
`git rev-parse --git-path info/sparse-checkout` in that checkout), so a lane
never rewrites the primary or a sibling lane. Patterns are validated (one line,
no NUL/CR/LF, non-empty, no leading `-`) and sent on stdin to
`git sparse-checkout set [--cone|--no-cone] --stdin`. No `--force` is passed:
git's own refusal to remove locally modified paths and its preservation of
untracked/ignored data stay in force. A merge/rebase/cherry-pick/revert/
sequencer/lock/unmerged index refuses; requires git ≥ 2.26.

### 2.7 Remote default-branch augmentation

```rust
pub fn resolve_remote_head(&self, remote: &RemoteName) -> Result<RemoteHead, GitError>;
pub fn remote_default_branch(&self, remote: &RemoteName) -> Result<Option<BranchName>, GitError>;
```

`ls-remote --symref <remote> HEAD` through the existing remote seam: terminal
prompts disabled, ssh `BatchMode=yes` when the effective transport is plain ssh,
the handle's timeout, bounded capture. `RemoteHeadTarget::{Branch,Detached,Unborn}`
distinguishes a symbolic default branch, a detached remote, and an empty remote.
A failed/timed-out contact is `RemoteUnreachable`, never a fabricated absence.

Workspace mapping: `ProtectionPolicy` can fall back to this probe when the loose
`refs/remotes/<remote>/HEAD` file is absent, instead of returning no
augmentation when the remote exists.

## 3. Captured operator-index authority (`GIT_INDEX_FILE`)

### 3.1 Capture and routing

* Captured **once** in `GitRepository::open_with_options`, after discovery and
  **before** root canonicalisation.
* A relative value is resolved against the discovered work-tree root — the
  directory child git runs in, and therefore git's own semantics (git moves to
  the top of the work tree during setup). The test opens the repository from
  `sub/deep` and proves the index still lands at `<root>/custom-index`.
* The captured spelling is re-applied **verbatim** to every child. A symlinked
  index path keeps its spelling; the adapter never resolves the final component.
  git's own follow-through behaviour (write through the link) is preserved and
  the symlink itself survives (tested).
* Every operator-index path is routed through the authority:
  preflight `ls-files --stage` (path shape), residue `diff --cached`, resync
  `reset --quiet`, post-commit hooks, `status`, merge start/conclusion/abort,
  sparse operations, and the held-index `.lock` check. Private
  `ScopedIndex`/`CommitHook` profiles still override the captured value **for
  that child only**.
* The process environment is never mutated (`std::env::set_var` is not used);
  the child still sees the caller's value after a commit (tested).
* `<git-dir>/index` is never substituted for a captured index. The default
  index remains implicit (no invented `GIT_INDEX_FILE`) when nothing was set.
* Plans bind the authority: `SafeCommitPlan::index_authority()` is compared at
  apply (`PlanStale { what: "operator index" }`); merge and sparse plans compare
  it too. A unit test plans on one handle, swaps the authority on a clone, and
  proves nothing is written.
* Every other repository-redirecting variable (`GIT_DIR`, `GIT_WORK_TREE`,
  `GIT_COMMON_DIR`, object/alternate/namespace/shallow/graft/replace/prefix/
  ceiling selectors) stays `RepositoryEnvironmentRefused`. The fence was not
  broadly removed: this is one captured *index*, not a shared repository
  authority.

### 3.2 Corpus closure

The 32-case scoped-safe-commit corpus was regenerated with the verified oracle
(`verify_oracle.ok = true`, no drift/missing). The former
`git_index_file_selector_refused` case is now
`git_index_file_selector_honoured` with `expectation = "match"`:
**31 exact matches, 1 documented literal-path divergence
(`pathspec_magic_literal_name`), 0 open mismatches.** Both sides are compared
field-by-field, including `.git/index`, the custom index entries and flags,
status, refs, reflog, commit/tree/parents, and changed paths.

Drift failures are retained: a missing/extra case, a per-case SHA-256 mismatch,
a changed generator without regeneration, a failed recorded oracle verification,
or a changed expectation set fails `corpus_manifest_is_complete_and_undrifted`.
The corpus size (32) and the required case names are pinned.

The parent's recent corrections are preserved and re-proved by the regenerated
corpus: ordinary commits force unsigned signing, ordinary hooks see
`GIT_EDITOR=:`, ordinary reflog takes the first message line, pinned reflog uses
the requested message, and an initially empty candidate still runs `pre-commit`
without letting its reformat rescue the no-op. Literal path selection remains
exact (`--literal-pathspecs`; unrequested paths are never staged).

## 4. Preservation, concurrency and recovery evidence

| Property | Evidence |
| --- | --- |
| Unrelated operator index content survives | `tests/safe_commit.rs` (57) incl. staged/partial/untracked/ITA/unmerged/deletion/sparse cases; regenerated corpus |
| Captured custom index: `.git/index` untouched, custom reconciled | `tests/index_authority.rs::an_absolute_custom_index_…`, corpus case |
| Relative and symlinked index spellings | `tests/index_authority.rs::a_relative_selector_…`, `…symlinked_index_…` |
| Index lock on the captured authority (never on `.git/index.lock`) | unit `a_captured_index_lock_is_checked_on_the_captured_authority` |
| Plan/apply authority binding | unit `applying_a_plan_under_a_different_captured_index_is_refused` |
| Concurrent writer after CAS is reconciled to the landed commit | corpus `post_cas_competitor_reconciled_to_landed_commit`; `tests/safe_commit_concurrency.rs` |
| Nested hook commit is unconfirmed, candidate unreachable | corpus `nested_hook_advance_conflict` |
| Merge start never moves HEAD; conflict stages are real | `tests/merge_start.rs::a_conflicted_start_…` |
| Merge abort restores HEAD/work tree and refuses after an edit | `tests/merge_start.rs::abort_refuses_…`, `…abort_restores_…` |
| Untracked user files survive merge start and sparse set | `tests/merge_start.rs::a_dirty_checkout_…`, `tests/sparse_checkout.rs::a_cone_set_…` |
| Only proven-stale registrations are pruned; live checkout + files survive | `tests/worktree_prune.rs::only_a_proven_stale_…`, `…live_checkout_is_never_…` |
| A recovered registration refuses instead of being pruned | `tests/worktree_prune.rs::a_registration_that_recovered_…` |
| Per-worktree sparse state does not leak to primary/sibling | `tests/sparse_checkout.rs::a_cone_set_…`, `…no_cone_…` |
| Disable restores the full work tree | `tests/sparse_checkout.rs::disabling_restores_…` |
| Remote contact failures are typed, not absence | `tests/remote_head.rs::an_unreachable_remote_…` |
| Unreadable HEAD is not absence; unborn is not an error | `tests/objects.rs::head_lookup_distinguishes_…`, `…unreadable_head_…` |
| Literal prefix/path matching and blob bound | `tests/objects.rs` (`prefix_sharing`, `deleted_path`, `blob_over_the_bound`) |

All tests use temporary repositories, `HOME`, `XDG_*` and `TMPDIR` roots. No test
reads or writes the workspace checkout, a real user repository, or a network
remote; the remote tests use local bare repositories. `GIT_INDEX_FILE` is
exercised through child processes so the parent's environment is never mutated.

## 5. Verification performed

Toolchain: Rust 1.99.1, git 2.43.0, Linux; `CARGO_BUILD_JOBS=2
CARGO_INCREMENTAL=0`, existing `target/`, `--locked`. No aggregate workspace
build, no release build, no timing run.

```
cargo test  -p sk-git --locked                                  → 232 passed, 0 failed, 3 ignored
cargo test  -p sk-git --locked --test safe_commit_conformance    → 2 passed, 1 ignored (helper), 5.6 s
cargo clippy -p sk-git --all-targets --locked -- -D warnings     → clean
cargo fmt    -p sk-git -- --check                                → clean
```

One earlier full run hit the known shared-`target/` artifact churn: a concurrent
cargo process replaced `libtempfile-*.rlib` mid-invocation and the doctest
compilation failed with `can't find crate for 'tempfile'`. A scoped rebuild and
the doctest rerun passed (4/4), and the clean full rerun above passed end to
end. That failure was build-artifact churn, not a source or test failure.

The three ignored tests are subprocess helpers exercised by their parents
(`captured_index_child`, `redirected_safe_commit_child`, `native_case_helper`).
Suite totals: lib 37, acceptance 27, bounded_process 7, index_authority 6 (+1),
lifecycle 36, merge_start 6, objects 9, remote_bounds 5, remote_head 5,
remote_lifecycle 10, safe_commit 57, safe_commit_authority 1 (+1),
safe_commit_concurrency 2, safe_commit_conformance 2 (+1),
safe_commit_differential 8, sparse_checkout 6, worktree_prune 4, doctests 4.

Oracle regeneration command (development only, run from the repository root):

```sh
PYTHONPATH="$PWD/src" SPEC_KITTY_PACKS_ROOT="$PWD/packs" \
  .venv-rust-oracle/bin/python \
  crates/sk-git/tests/fixtures/safe_commit/generate_oracle.py
```

## 6. Source identity

Bound to the current source bytes (the shared checkout is edited concurrently).
SHA-256 of the main new/changed files:

| File | SHA-256 |
| --- | --- |
| `src/index_authority.rs` | `57609c7252ac1071c24895cf60cf3ca1a0b4610acf43188f614f621003880df0` |
| `src/objects.rs` | `c5896946ebe7d2b451fea51f3e69fa026f30161d774fdf8a9bee29b8499cf299` |
| `src/merge_start.rs` | `3af5ca4d728775d26ec63ae9731e9574593aa2bfa385aedc0b016154fb2e11c3` |
| `src/worktree_prune.rs` | `b1892b44eefc4defa552a444a4001b737ff928e5ae8951e0372f8bdcf67f94e0` |
| `src/sparse.rs` | `abfe02ab2dd639c3230e242cca079b56202eaa7686c38fe8c1e8b46c405180de` |
| `src/process.rs` | `be11aa3ed3845f13917dcbed678ae21d55e00772c66c279405f796d440f74e58` |
| `src/repository.rs` | `6a722037c6b05c922339249a46661a088ffc8162adeb97ceb644911cf629cd41` |
| `src/safe_commit.rs` | `1b858f72a408456abe69c13395daba0d5c66d1f5d599e8360f0b2ccb7aafc27a` |
| `src/safe_commit_transaction.rs` | `cd0dec51b99ed6d544a8e80a10b8ed4b4d8f2cb6638638f9a5360c0a5a181d61` |
| `src/repo_state.rs` | `60c49a0af2ca1da660cffbf40ab45c2ec3169f07ae5b344d51db06c2ba370bc6` |
| `src/merge.rs` | `47f3ff8e9d324567fba4bdc654bf75920780324f1f3528677d6f4fb9ead6df00` |
| `src/remote.rs` | `7a671c58ad7f90f22da178eb810a14c1a8ee8073c9929a51aa24288815b1cae8` |
| `src/error.rs` | `66bffe5f7344a754be2566225a1ea6c3927df0e3a076a5d93eb5f1dda3290766` |
| `src/lib.rs` | `4c86c765cc04988b4ef6bde8270393aa4c7b9d5ed1d2be47e8017f494619aeec` |
| `tests/fixtures/safe_commit/generate_oracle.py` | `56cddf8bbf0158797da075008c56764a8dc6cfb7e8e00602dda64ec237588cf4` |
| `tests/fixtures/safe_commit/manifest.json` | `b42363011a1b95900a1d13a47cf91132b48dbd539c09b2027ef07b0163bcb90a` |
| `tests/index_authority.rs` | `c3357a256b875619d536c767946bd310de3e0a430ddbb0caa73b0909c0a83567` |
| `tests/merge_start.rs` | `0aaf09486471f4a5f458dca608cbee17560373996b8660bf767600c5fa5a2c06` |
| `tests/objects.rs` | `681a29cb96eb49e3630b6555236a57dab7c8060e055f8e45cf74d8f80f114002` |
| `tests/remote_head.rs` | `459d6183c4a9ad2548a03de31a1ca2a59b607997d9dfbf3b77c89e2d4bcd8558` |
| `tests/sparse_checkout.rs` | `b96b4410f2c2a25b9511369ddb41d0aaf257fea6b3585b1d1da7e81a0b49f635` |
| `tests/worktree_prune.rs` | `324fc6e3aedfb0c47e8193d9e9b05849db42303b360e7e928fada694c79fc064` |

## 7. Unfinished obligations and proposed needs

Not implemented or not claimed (kept explicit rather than papered over):

1. **Merge backend breadth.** No octopus merge (`MERGE_HEAD` with several tips
   is `OperationInProgress`), no squash merge, no `merge-tree --write-tree`
   simulation, no fresh-linked-worktree proof, no rebase/cherry-pick start. A
   merge start requires a clean **tracked** checkout; git's own refusal still
   protects untracked overrides, but this adapter will not attempt a merge over
   a dirty work tree.
2. **Sparse checkout breadth.** No `--force`, no per-pattern "add/remove"
   mutation separate from `set`, no non-UTF-8 pattern support (patterns are
   `String`, matching git's one-line file format), no `sparse-checkout reapply`
   wrapper. `core.sparseCheckout` in the *shared* config is not rewritten; only
   the per-worktree mode is set by git's own command.
3. **Prune breadth.** `git worktree prune --expire now` only (git ≥ 2.31); the
   plan refuses a candidate whose directory exists, so it cannot remove a broken
   admin entry for a present directory. It never deletes a checkout tree.
4. **Object reads.** No tree-diff/log/blame/numstat listings, no streaming for
   blobs above the 16 MiB seam cap, no submodule object traversal (a gitlink is
   `Absent`, never read as a commit), no SHA-256 or Windows evidence.
5. **Remote.** No fetch/clone/push/tag/remote-configuration write, no fetch
   before freshness compare, no caching of the remote HEAD probe. `ls-remote` is
   the only contact.
6. **Repository selectors.** Only `GIT_INDEX_FILE` is captured. `GIT_DIR`,
   `GIT_WORK_TREE`, `GIT_COMMON_DIR`, object/namespace selectors and the rest
   remain explicit refusals until one shared repository authority owns lock
   discovery and the opened adapter. Capture is per handle, not process-wide.
7. **CLI and composition.** No CLI wiring, no consolidation/acceptance
   composition, no performance claim. The workspace's `MergeRequired`,
   `COORD_SURFACE_RESTORE_REQUIRED`, lane sparse policy and remote-default
   augmentation are ready to consume the APIs above but were not edited here.
8. **Cross-platform / hash.** Linux/SHA-1 only. Symlink behaviour is tested on
   Unix; Windows/macOS equivalence is unverified.

Dependency needs proposed to the parent: **none.** Every capability above uses
the existing `sk-kernel` path, `tempfile`, `thiserror` and the standard library.
No manifest change was made or is required for this tranche.

Recommended integration order for the coordinator (no edits made here):
`read_blob_at_head`/`read_blob_at` for the status rollback seam and coordination
restore → `merge_base` in lane ancestry → `merge_start`/`merge_abort` for
dependency integration → `plan_worktree_prune` for interrupted materialization →
`set_sparse_checkout` for lane isolation → `resolve_remote_head` in protection
policy → CLI wiring last.

## 8. Boundaries observed

* Owned files only: `crates/sk-git/**`. No workspace manifest, `Cargo.toml`,
  Kernel, Mission, CLI or status source was edited.
* No commits, pushes, workspace/build manifests (`Cargo.toml`/`Cargo.lock`),
  extra targets, aggregate or release builds, timings, subagents, or production
  Python wrappers. The corpus's own fixture `manifest.json`/`PROVENANCE.json`
  were regenerated with the checker-required digests.
* No shell-made worktrees and no marker-file substitutes for real operations;
  every mutation is a real git command in a temporary repository.
* Probe/runtime state stayed under temporary `HOME`/`XDG`/`TMP` and temporary
  project roots. No real repository was mutated by a test.
