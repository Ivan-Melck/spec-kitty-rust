# Safe-commit / path-scoped index transaction — implementation report

> Historical implementation report, predating independent review and coordinator
> fixes. The current disposition and aggregate evidence are in
> `docs/rust/reviews/correctness-continuation.md`; the counts and gaps below
> describe the original tranche, not approval of current sources.

**Status: implemented native substrate, verified on Linux; CLI integration remains
pending.** This report is deliberately explicit about what is real and what is
still missing, so a later CLI/integration tranche does not mistake the library
surface for a wired command.

## Provenance

| Item | Value |
|------|-------|
| Repository | `spec-kitty-rust` |
| Checkpoint | `ab3e0fcc8` |
| Oracle source baseline | `spec-kitty/spec-kitty` @ `9aed3a8012cb5eb22c0128b14d525039f95fea7a` |
| Oracle runtime | `.venv-rust-oracle/bin/python` (Python 3.11), `PYTHONPATH=src`, temporary `HOME`/`XDG_*` |
| Upstream modules read | `src/specify_cli/git/commit_helpers.py`, `src/specify_cli/core/commit_guard.py`, `src/specify_cli/git/ref_advance.py`, `src/kernel/git/listing.py`, `src/specify_cli/git/protection_policy.py`, plus the safe-commit test suites |
| Rust toolchain | `1.99.1` (workspace pin) |
| Build policy | `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, targeted `-p sk-git` runs only |

Files owned/changed by this tranche:

```
crates/sk-git/src/safe_commit.rs                       (new)
crates/sk-git/src/safe_commit_transaction.rs           (new)
crates/sk-git/src/process.rs                           (additive typed scoped-index seam)
crates/sk-git/src/error.rs                             (additive safe-commit errors)
crates/sk-git/src/lib.rs                               (new exports)
crates/sk-git/README.md                                (API/gap/test documentation)
crates/sk-git/SAFE-COMMIT-REPORT.md                    (this file)
crates/sk-git/tests/safe_commit.rs                     (new, 49 tests)
crates/sk-git/tests/safe_commit_concurrency.rs         (new, 2 tests)
crates/sk-git/tests/safe_commit_differential.rs        (new, 8 pinned oracle tests)
```

No existing lifecycle/remote code was rewritten. `process.rs` gained only a
typed, crate-private index-file capability; `error.rs` gained only new variants.

## What was built

A real, native path-scoped commit substrate — not a plan or a stub:

1. **Validation / planning** (`plan_safe_commit`) refuses, before any mutation:
   empty path sets and empty messages; index deletions that overlap requested
   paths; exact-bytes assertions without a pinned parent or for unrequested
   paths; absolute/`..` escapes and symlink loops; `.worktrees/` paths; directory
   arguments (while allowing symlinked directories, deleted paths, and submodule
   gitlinks); a HEAD that is not the declared branch; a missing destination
   branch; a pinned parent that is not the destination tip; an in-progress
   merge/rebase/cherry-pick/revert/sequencer or a held `index.lock`; and a
   protected destination without an asserted bookkeeping capability.
2. **Private-index transaction** (`apply_safe_commit`): seeds a private index
   from the pinned parent (`HEAD` by default), stages exactly the requested
   paths, optionally removes index-only deletions, rejects a no-op from the
   index, runs the repository's own commit hooks through `git hook run`, writes
   the tree, creates the commit with `git commit-tree -p <parent>`, and advances
   the branch with a ref compare-and-swap that requires that parent.
3. **Operator-state preservation**: unrelated staged (including partially
   staged), unstaged, untracked, and ignored content is never read, stashed,
   moved, or restored. The real index is written only *after* the commit exists,
   and only for the requested paths.
4. **Post-landing integrity**: requested paths are reconciled to the new HEAD; a
   residue is a typed error; the commit's own changed-path set is read back and
   any hook-swept extra paths are resynchronised and reported as
   `SafeCommitLanded { kind: UnrequestedPaths }`; the `post-commit` hook runs
   afterwards and its failure is an outcome warning.
5. **Pinned-parent mode** (`with_expected_parent`): additionally re-isolates the
   tree around the hooks (a hook can neither add paths nor change requested
   content) and verifies optional exact raw bytes with
   `git hash-object --stdin --no-filters`, before the CAS.

## Public API (exported from `sk_git`)

### Request / plan / outcome

| Item | Semantics |
|------|-----------|
| `SafeCommitRequest::new(BranchName, message, impl IntoIterator<Item = GitPath>)` | Exact worktree-path set on the declared short branch. |
| `.with_index_deletions(paths)` | Index-only removals (file stays on disk). Replaces any previous set. |
| `.with_expected_parent(Oid)` | Pin the parent; enables the isolated/CAS route. |
| `.with_expected_bytes(impl IntoIterator<Item = ExpectedBlob>)` | Assert staged blob bytes; requires a pinned parent and requested paths. |
| `.with_protection(ProtectedBranches, CommitGuardCapability)` | Supply the caller-resolved protected set and asserted capability. |
| Accessors | `destination`, `message`, `paths`, `index_deletions`, `expected_parent`, `expected_bytes`. |
| `GitRepository::plan_safe_commit(&request)` | Validation only; no mutation. Records HEAD, destination tip, route, normalised paths, guard verdict. |
| `GitRepository::apply_safe_commit(&plan)` | Re-reads HEAD/tip/operation markers, then executes. Refuses a plan prepared for another checkout. |
| `GitRepository::safe_commit(&request)` | Plan + apply. |
| `SafeCommitPlan` | `destination`, `message`, `worktree_root`, `head`, `destination_tip`, `route`, `paths`, `index_deletions`, `expected_parent`, `expected_bytes`, `guard_verdict`, `requested_paths`. |
| `SafeCommitOutcome` | `commit`, `destination`, `worktree_root`, `committed_paths` (commit's own changed set, renames split), `warnings`. |
| `SafeCommitRoute` | `Worktree`, `IndexDeletions`, `PinnedParent`. |

### Guard and path normalisation

| Item | Semantics |
|------|-----------|
| `CommitGuardCapability` | `Standard` (default), `ReleaseFlow`, `UpgradeBookkeeping`, `MergeBookkeeping`, `TestMode`, `PostConsolidationWrite`; mirrors upstream, no capability authorises a push. |
| `evaluate_commit_guard(&BranchName, &ProtectedBranches, capability)` | Pure decision; unprotected allowed, protected requires a non-standard capability. Echoes the destination. |
| `CommitGuardVerdict` | `allowed`, `destination`, `reason`. |
| `normalize_safe_commit_path(root, path)` | Resolves absolute/relative caller paths, follows symlinked parents, keeps the final component (a symlink is committed as a link), preserves Unix bytes, refuses `..`/outside/loop. |
| `ExpectedBlob::new(path, bytes)` | Exact staged-bytes assertion. |
| `SafeCommitRecovery` | `commit`, `affected_paths`, `repair_argv(program)` for a landed-but-unreconciled commit. |
| `WORKTREES_DIR`, `MAX_COMMIT_MESSAGE_LEN` | Path-policy constant, 64 KiB message bound. |

### Errors (additive, `GitError`)

Pre-landing refusals (nothing written): `SafeCommitEmptyChangeset`,
`SafeCommitHeadMismatch`, `SafeCommitDestinationMissing`,
`SafeCommitDirectoryArgument`, `SafeCommitPathPolicy`,
`SafeCommitPathOutsideWorktree`, `SafeCommitPathLoop`,
`SafeCommitProtectedBranch`, `SafeCommitIndexDeletionConflict`,
`SafeCommitInvalidRequest`, `SafeCommitNothingToCommit`,
`SafeCommitStageFailed`, `SafeCommitCommitFailed`, `SafeCommitHookRejected`,
`SafeCommitHookChangedPaths`, `SafeCommitExpectedBytesMismatch`,
`OperationInProgress`, `PlanStale`, `Unsupported`.

Post-landing (commit exists; nothing auto-reset): `SafeCommitLanded` with
`SafeCommitLandedKind::{IndexResyncFailed, IndexResidue, UnrequestedPaths,
CommitPathReadbackFailed}` and a boxed `SafeCommitRecovery`.

`SafeCommitNothingToCommit` carries the evidence that classified the no-op, so a
scripted caller can distinguish a benign no-op from a failure without parsing
message text.

### Subprocess seam addition

`process.rs` now has a crate-private `ScopedIndex`:

* the only constructor is `ScopedIndex::in_git_dir(absolute_git_dir)`;
* the name is generated there (`spec-kitty-safe-commit.<pid>.<nanos>.<serial>.tmp`),
  never a caller path;
* it is fed to git only through `EnvProfile::ScopedIndex`, which sets
  `GIT_INDEX_FILE` **after** every other repository-redirecting variable is
  removed;
* `Drop` removes the private index and its `.lock` sibling.

There is no second `Command` helper and no caller-supplied environment map.

## Invariants established by the tests

* Unrelated staged, partially staged, unstaged, untracked, and ignored content is
  byte-identical after a safe commit.
* A commit never carries a path outside the request (if a hook adds one, the
  outcome is reported as landed-with-extras and the extra index entries are
  resynchronised).
* A commit is built against the observed parent and CAS-advanced; a stale parent
  or concurrent branch move yields `PlanStale` with no ref moved. Concurrent
  writers cannot cause a commit built from an old tree to delete a newer commit's
  paths.
* Pathspecs are literal (`--literal-pathspecs` + `--`): names like `-x.txt`,
  `--output=…`, `*.txt`, `:(glob)*.txt`, `colon:name.txt`, and a newline in a
  name are data.
* Unix non-UTF-8 path bytes survive staging, commit, and readback.
* A rejected hook is not reclassified as an empty changeset; a hook printing
  "nothing to commit" while rejecting is still a hook rejection.
* A hook's warn-mode stderr on a successful commit becomes an outcome warning.
* A foreign `index.lock` is never removed; it refuses the commit before mutation.
  A lock that appears after the commit yields `SafeCommitLanded` with repair
  evidence.
* A landed commit whose index reconciliation or readback fails reports the
  commit id and paths, and never resets user state.

## Differential oracle evidence (pinned, no Python at test time)

`tests/safe_commit_differential.rs` embeds the structural outcomes produced by
the pinned Python oracle for eight scenarios:

| Scenario | Oracle outcome | Pinned assertions |
|----------|----------------|-------------------|
| unrelated staged + partial + untracked | landed | committed `{a.txt,new.txt}`; status `?? u.txt`, `M  other.txt`, `MM partial.txt`; `other.txt`/`partial.txt` unchanged in HEAD |
| rename pair (`git mv`) | landed | committed `{new.txt,old.txt}` (delete + add); clean status |
| identical content | `SAFE_COMMIT_STAGED_TREE_UNCHANGED` | `SafeCommitNothingToCommit`; HEAD unchanged; other staging preserved |
| directory argument | `SAFE_COMMIT_DIRECTORY_ARGUMENT` | typed refusal; HEAD/index unchanged |
| protected `main`, standard capability | `SAFE_COMMIT_PROTECTED_BRANCH` | typed refusal; HEAD/index unchanged |
| rejecting pre-commit hook | generic RuntimeError (not empty) | `SafeCommitHookRejected` carrying `pre-commit boom`; HEAD unchanged |
| index-only deletion | landed | committed `{c.md}`; status ` M d.md`, `?? c.md`, `M  other.txt`; file still on disk |
| hook `git add -u` sweep | `SAFE_COMMIT_UNREQUESTED_PATHS` | `SafeCommitLanded(UnrequestedPaths)` with `other.txt`; committed `{other.txt,spec.md}`; index resynced |

The oracle probe used a fresh temporary repository per scenario, branch
`kitty/mission-diff-01ABCDEF`, the pinned file contents in the test file, and a
temporary `HOME`/`XDG_*`. Object ids and timestamps are deliberately not pinned.

## Verification performed (Linux, this checkpoint)

```bash
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p sk-git --locked
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo clippy -p sk-git --all-targets --locked -- -D warnings
cargo fmt -p sk-git -- --check
```

Result: **179 passed, 0 failed** —

| Suite | Tests |
|-------|-------|
| `sk-git` unit (`--lib`) | 34 |
| `acceptance` | 27 |
| `bounded_process` | 7 |
| `lifecycle` | 34 |
| `remote_bounds` | 5 |
| `remote_lifecycle` | 10 |
| `safe_commit` | 49 |
| `safe_commit_concurrency` | 2 |
| `safe_commit_differential` | 8 |
| doctests | 3 |

The concurrency suite was additionally run 30 times back-to-back with no
failure. Strict Clippy and formatting pass. No workspace-wide, release, or
extra target-tree build was run for this tranche.

## Actual gaps for later CLI integration

These are real, deliberate gaps — not hidden by the passing library tests.

1. **No CLI wiring.** Nothing in `sk-cli` calls `safe_commit`. `spec-commit`,
   `agent` commits, upgrade autocommit, bookkeeping commits, mission creation
   commits, `next`, and the orchestrator API still need to call this substrate
   and map its typed errors/messages.
2. **Protection facts are caller-supplied.** `SafeCommitRequest` defaults to an
   **empty** protected set and `Standard`. Before any real use, the application
   must resolve `.kittify/config.yaml` (`protection.protected_branches`, default
   `{main, master}` ∪ remote default), the operator hatch
   `SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS`, and the mission-scoped
   `commit_to_target` fold, then call `with_protection`. The pure
   `evaluate_commit_guard` is ported; the resolver is not.
3. **Owned-checkout / decision-lock restriction is not enforced.** Upstream
   restricts `index_only_removals` to the caller-derived decision runtime lock
   and a validated `OwnedCheckout`. Here any requested path may be
   index-removed (still explicit), so integration must enforce ownership before
   using that mode.
4. **No commit router / report transaction / receipt qualification.** Upstream
   `coordination/commit_router.py` and `git/report_transaction.py` are not
   ported. Placement/`--to-branch` resolution into a destination branch is the
   caller's job; this API takes a validated `BranchName`.
5. **No `SAFE_COMMIT_*` string error codes.** The port uses typed variants.
   Consumers with a stable `error_code` contract (orchestrator API
   `allowed_error_codes`) need a mapping layer. `SafeCommitBackstopError` is
   intentionally not ported (upstream deleted its only callers).
6. **No directory expansion.** A CLI that accepts directories must expand them
   to files first (`git status --untracked-files=all`); the library refuses
   directories by design.
7. **`git hook run` requires Git >= 2.36.** Older Git is refused with
   `Unsupported` rather than skipping hooks. All commit hooks are run by git, not
   by shelling out to hook files.
8. **No-op hook divergence.** A no-op is detected from the private index before
   hooks run, so hooks do not run for it. Upstream's `git commit` runs hooks and
   then reports nothing to commit.
9. **Signing parity.** The default route commits unsigned exactly like upstream
   (`commit.gpgsign=false`); the pinned-parent route honors `commit.gpgsign`
   (`commit-tree -S`). Signature success/identity is not asserted.
10. **Expected-bytes budget.** Exact-byte assertions go through the seam's
    bounded stdin (4 MiB per blob); larger blobs refuse with `InputTooLarge`.
11. **Post-landing reconciliation bound.** Index reset retries while another
    writer holds the lock for at most ~120 ms; a longer lock yields
    `SafeCommitLanded(IndexResyncFailed)` with repair evidence. Callers must
    surface that as "landed, needs repair", never as "failed".
12. **Windows/non-UTF-8.** Unix non-UTF-8 paths are covered; on Windows the crate
    requires UTF-8 paths (crate-wide policy) and the symlink-loop errno mapping
    is platform-specific. No Windows/macOS run was performed.
13. **SHA-256 repositories** are accepted by `Oid` but not exercised by tests.
14. **Submodule gitlinks**: the directory-refusal exemption for a `160000` index
    entry is tested at plan time; staging a real submodule checkout is not.
15. **No performance claim.** This tranche is correctness-scoped; no benchmark
    separates a Python `safe_commit` run from the Rust substrate.
16. **Policy/limit divergence to document in the CLI.** The crate's default
    protected set is empty (fail-open by omission); the integration MUST attach
    the resolved set. Consider making protection an explicit constructor
    argument in a later API-breaking pass.
17. **A hook that itself runs `git commit`** inherits the private
    `GIT_INDEX_FILE` and could move HEAD. The outer compare-and-swap then refuses
    with `PlanStale` (no overwrite), but the nested commit may have landed. This
    pathological shape is not special-cased.
18. **No independent review or cross-platform CI** has certified this surface
    yet. Passing tests establish the covered contracts, not blanket upstream
    compatibility.

## Integration checklist

- [ ] Resolve protection facts (config + remote default + hatch + mission
      `commit_to_target`) and pass them with a capability.
- [ ] Map `SafeCommitLanded` to "committed, index repair needed" in every
      caller; never flatten it to "not committed".
- [ ] Surface `warnings` (hook warn-mode, post-commit hook failure) on the
      operator channel.
- [ ] Expand directory arguments before calling; never call with an unresolved
      `--to-branch` string.
- [ ] Enforce owned-checkout/decision-lock ownership before index-only removals.
- [ ] Add the `SAFE_COMMIT_*` error-code mapping if a stable code contract is
      required.
- [ ] Add CLI/orchestrator integration tests and end-to-end mission workflows.
- [ ] Run cross-platform CI and independent review before claiming parity.
