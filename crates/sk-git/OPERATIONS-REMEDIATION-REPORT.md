# Git operations remediation — implementation report

> **Scope.** `crates/sk-git/**` only, from committed baseline `d64468a76`. This
> report addresses the blocking findings of the completed independent review
> `/tmp/sk-rust-git-operations-review.md` (verdict **NOT APPROVED**). It is
> implementation evidence, not independent approval, and not a workspace, CLI,
> parity, performance or release claim. No file outside `crates/sk-git` was
> edited; nothing was committed, pushed, or released.

| Item | Value |
| --- | --- |
| Baseline (and still current) HEAD | `d64468a7693e4962712cbf3416d767440d22a649` |
| Branch | `rust/rewrite` (not pushed) |
| Review artifact | `/tmp/sk-rust-git-operations-review.md` (FINAL, NOT APPROVED) |
| Real-Git F2 repro | `/tmp/sk-mergeprobe.S0sTxW` (preserved, unmodified) |
| F1 fix (preserved) | `/tmp/sk-merge-index-binding-{red,green,scoped}.log`, unchanged |
| Toolchain | rustc 1.99.1, git 2.43.0, Linux, SHA-1 repositories |
| Cargo sandbox | `bash /tmp/sk-focused-cargo.sh` (temp HOME/XDG/TMP, `--locked`, `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, shared lease, existing `target/`) |
| Disk | 83 GiB free at completion (88 GiB at start); no ENOSPC encountered |

`/tmp/sk-git-review2` (the older probe logs the review already found missing)
does not exist and is not relied on. F2 is re-derived natively from the same
Git-level shape as the preserved repro, plus a real-repository red run.

## 1. Finding disposition

| Finding | Status | What changed |
| --- | --- | --- |
| **F1** (BLOCKING, fixed by coordinator) | **Preserved** | Verification that a plan prepared on one captured-index handle refuses to apply on another, leaving `HEAD`, `MERGE_HEAD` and both indexes byte-identical. The unit test is unchanged and green; the authority comparison now runs before every other apply precondition. |
| **F2** (BLOCKING) | **Fixed** | `apply_merge_conclusion` now reports the commit *this call created*, proven from its real parent set, and returns typed `GitError::MergeConclusionUnconfirmed` (created commit + observed `HEAD` + marker state + diagnostics) whenever the post-hook state is not the pinned one. |
| **F3** | **Fixed** | Corrupt `HEAD` shapes are `UnreadableHead`, not `NotARepository` (garbage/empty/invalid-ref at `open`) and not `CommandFailed` (empty symbolic target at read). |
| **F4** | **Fixed** | Explicit bounded blob reads accept 64 MiB with the capture limit plumbed through the subprocess seam (still enforced, never truncated); the default stays 8 MiB. |
| **F6** | **Fixed (doc)** | Sparse `mode()` documentation now states what the implementation does (absent `core.sparseCheckoutCone` reads as non-cone; unconfigured is `NoCone` + `configured() == false`). |
| **F7** | **Proved** | Native abort-with-untracked test: an untracked file added after planning is `PlanStale`, and pre-existing untracked files survive a real `git merge --abort`. |
| **F8** | **Fixed** | The corpus test now pins the single documented divergence by name (`pathspec_magic_literal_name`), not only by count. |

## 2. F2 — merge conclusion commit identity (main work)

### 2.1 Defect reproduced red-first

`/tmp/sk-merge-conclusion-identity-red.log` (13:26:28Z, before the fix):

```
test a_hook_moved_head_is_never_reported_as_the_created_conclusion_commit ... FAILED
  left:  "d7ef76ed7f3c071a6da0ec1dd51b4df2cd8f47c0"   <- reported new_head
  right: "d7ef76ed7f3c071a6da0ec1dd51b4df2cd8f47c0"   <- the pre-existing decoy
test a_conclusion_that_leaves_the_operation_marker_is_not_reported_as_success ... FAILED
  a conclusion that leaves MERGE_HEAD behind is unconfirmed, got Ok("16b96b5b…")
```

Both are real git repositories with a real `post-commit` hook. The first hook runs
`git update-ref refs/heads/main <decoy>` — exactly the preserved
`/tmp/sk-mergeprobe.S0sTxW` shape — and the old code reported the decoy as
`new_head`. The second hook re-creates `MERGE_HEAD` after the commit; the old code
returned `Ok` and never noticed.

### 2.2 Behaviour now

`MergeConclusionPlan` pins: captured index authority, worktree root, checked-out
branch, operation, `HEAD`, operation tip, message.

`apply_merge_conclusion`:

1. compares **operator index first**, then worktree root, then the concludeable
   operation, `HEAD`, branch and operation tip — nothing is written on any
   mismatch (`PlanStale`);
2. runs exactly the same one argv as before (`git -c commit.gpgsign=false commit
   [--no-edit | -m msg]`); hooks, author/committer identity and the repository's
   message handling are untouched, `--no-verify` is never passed;
3. reads the post-state once (`HEAD`, operation marker) and **identifies the
   commit this call created** only from evidence bound to the invocation:
   * git's own commit summary line (`[main <abbrev>] …`) printed by the child,
   * a reflog entry appended to the **pinned** ref/branch after a publication
     watermark captured immediately before the command ran.
   A candidate must also carry the real parent set exactly `{pinned HEAD,
   operation tip}` for a merge and `{pinned HEAD}` for a cherry-pick/revert
   (`CHERRY_PICK_HEAD`/`REVERT_HEAD` record the source commit in the message,
   not as a parent). The parent set is a necessary check, **not creation
   proof**, and the observed `HEAD` is no longer accepted as a candidate merely
   because its parents match (a hook can point the branch at a pre-existing
   identical merge). A reflog rewritten after the watermark (for example
   expired by a hook) cannot be bound to the call and yields no candidate;
4. returns `Ok(MergeConclusionOutcome)` only when the command completed
   successfully, the proven created commit is the current `HEAD`, and the
   operation marker is consumed. `new_head()` is that proven commit — never a
   post-hook `HEAD` on a guess;
5. returns `GitError::CommandFailed` only when the command completed with a
   failure status and the state proves nothing landed: `HEAD` is still the
   pinned commit, the marker still records the pinned tip, and no commit this
   call created is identifiable. This keeps the rejecting-`pre-commit` contract;
6. a command that **started and then failed without a status** (timeout,
   captured-output bound, readback I/O while a `post-commit` hook still ran) may
   already have landed: it is reconciled into
   `GitError::MergeConclusionUnconfirmed` with the candidate/observed/marker
   evidence, never returned as an ordinary retryable failure. Only a proven
   pre-execution failure (temporary-file setup or spawn) stays an ordinary
   error;
7. otherwise returns `GitError::MergeConclusionUnconfirmed(Box<…>)` carrying
   `candidate` (a proven created commit, or `None` when unknown — which does not
   mean "not landed"), `previous_head`, `observed_head`, `operation_consumed`,
   the `marker` state (`Consumed`/`Unchanged`/`Changed`/`Unreadable`) and the
   accumulated diagnostic notes. Nothing is reset, deleted, reverted or cleaned
   up: a possibly landed commit is left in place.

API compatibility: the `plan`/`apply` signatures, `MergeConclusionOutcome` and
its `operation()/previous_head()/new_head()/branch()` accessors are unchanged.
New surface: `MergeConclusionPlan::{worktree_root(), branch()}` and the
`MergeConclusionUnconfirmed` payload (re-exported), whose later correction adds
the public `marker: OperationMarkerState` field (`operation_consumed` is kept for
readers and is `true` exactly when `marker == Consumed`). The payload is boxed so
the error type stays small (scoped Clippy `result_large_err` would otherwise fire
across the crate).

### 2.3 Contracts proved natively

`crates/sk-git/tests/merge_start.rs` (13 passed;
`/tmp/sk-merge-conclusion-identity-green.log`):

| Contract | Test |
| --- | --- |
| Post-commit hook moves `HEAD` → unconfirmed; candidate is the **real** merge commit (two pinned parents, in the reflog), observed `HEAD` is the decoy, `MERGE_HEAD` consumed, work-tree content preserved, nothing rolled back | `a_hook_moved_head_is_never_reported_as_the_created_conclusion_commit` |
| Post-commit hook re-creates `MERGE_HEAD` → unconfirmed; `HEAD` *is* the created commit and the marker is left in place (late failure, no cleanup) | `a_conclusion_that_leaves_the_operation_marker_is_not_reported_as_success` |
| Rejecting `pre-commit` hook → `CommandFailed`, `HEAD` unchanged, `MERGE_HEAD` still present, no conclusion in the reflog | `a_rejecting_pre_commit_hook_is_a_refusal_that_leaves_the_operation_in_progress` |
| Failing `post-commit` hook (exit 1) cannot change a confirmed landing; the hook really ran | `a_failing_post_commit_hook_cannot_change_a_confirmed_conclusion` |
| Valid conflicted cherry-pick concludes (`--no-edit`), marker consumed, single parent, git's prepared message kept | `a_conflicted_cherry_pick_concludes_with_the_picked_commit_as_second_parent` |
| Valid conflicted revert concludes with an explicit message, marker consumed, single parent | `a_conflicted_revert_concludes_with_the_reverted_commit_as_second_parent` |
| Clean `--no-ff` conclusion still has two parents | `a_clean_merge_start_stops_at_the_pinned_head_and_concludes_with_two_parents` |

Unit tests in `src/merge.rs`: F1 authority refusal (unchanged), summary-line
parsing, and the reflog fallback naming only a commit with the pinned parent set.

## 3. F3 — corrupt-HEAD taxonomy

git 2.43.0 refuses to recognise a repository whose `HEAD` is garbage, empty, or
`ref: <invalid>` ("fatal: not a git repository"), so the adapter classified those
as `NotARepository`. `GitRepository::open` now classifies them locally, before
returning that answer: it walks to the enclosing git directory (`.git` dir,
`gitdir:` indirection, or a bare/git directory), reads `HEAD` without following a
final symlink, and reports `GitError::UnreadableHead` when the file is empty, not
UTF-8, neither `ref: <qualified ref>` nor a full object id, or names an invalid
ref. A readable `HEAD` leaves `NotARepository` untouched — no non-repository is
re-classified.

`git symbolic-ref --quiet HEAD` exits 0 (target), 1 (detached) or 128 (`fatal: No
such ref: HEAD` for `ref: refs/heads/`). Only 0/1 are answers; every other status
is now `UnreadableHead` (with git's diagnostic), not `CommandFailed`.

Fail-closed consumption is unchanged: `read_blob_at_head*` still maps only the
confirmed unborn branch to `NoRevision`, and the status consumer
(`sk_cli::compat::support::StatusHeadCommittedProbe`) still turns any open/read
error into a hard error — `open` is now merely more precise about *which* typed
error a corrupt `HEAD` is. No status/CLI source was edited.

`tests/objects.rs`: `a_corrupt_head_is_unreadable_not_a_missing_repository`
(garbage, empty, `ref: garbage`) and
`an_empty_symbolic_head_target_is_unreadable_not_a_command_failure`; the existing
non-UTF-8-target test still passes.

## 4. F4 — explicit 64 MiB bounded blob read

* `DEFAULT_MAX_BLOB_BYTES` stays **8 MiB**; `MAX_BLOB_BYTES` is now the canonical
  status-log bound **64 MiB**.
* The limit is no longer just a constant: `GitProcess` gained
  `run_with_output_limit` / `checked_with_output_limit`, which validate the bound
  against a 64 MiB capture ceiling and pass it into `run_profiled`. The child's
  streams are still polled and killed above the limit, `read_output` still takes
  `limit + 1` and refuses above the limit, and the error reports the actual limit.
  Ordinary commands keep the 16 MiB per-stream cap.
* `read_entry_blob` runs `git cat-file blob` under the caller's bound, so an
  accepted blob is returned whole — there is no second, smaller truncation point.

`tests/objects.rs`:

| Test | Proves |
| --- | --- |
| `an_explicit_bound_beyond_the_old_seam_cap_reads_a_large_status_log` | a 17 MiB committed log (above the old 16 MiB capture cap) is returned byte-exact with the explicit bound; the 8 MiB default still refuses it |
| `a_full_sixty_four_mib_blob_reads_exactly_and_one_byte_over_refuses` | a 64 MiB committed log is returned whole at `MAX_BLOB_BYTES`; `MAX_BLOB_BYTES - 1` refuses with `BlobTooLarge` |
| `process::tests::captured_output_is_bounded_even_after_a_fast_child_exits` | the explicit 64 MiB capture is enforced (a 64 MiB + 1 file refuses) and a bound above the ceiling is `InvalidPlan` before running |

**Integration note for the status owner (2606a83a):** the existing
`StatusHeadCommittedProbe` call site already passes `sk_git::MAX_BLOB_BYTES`, so
it now requests and receives 64 MiB automatically; only its comment (which says
the seam caps at 16 MiB) is stale. That file is owned elsewhere and was not
edited.

## 5. F6 / F7 / F8

* **F6** — `SparseCheckoutState::mode()` documentation corrected (verified
  against git 2.43: `git sparse-checkout init` writes
  `core.sparseCheckoutCone=true`; an absent key is non-cone, and an unconfigured
  checkout reports `NoCone` with `configured() == false`). Behaviour unchanged;
  the 6 sparse tests still pass.
* **F7** — `abort_preserves_untracked_files_and_refuses_an_untracked_change_after_planning`
  (green, `/tmp/sk-abort-untracked-proof.log` was the pre-change proof run; it is
  also in the green suite).
* **F8** — `safe_commit_conformance` now accumulates the divergence names and
  asserts they equal exactly `["pathspec_magic_literal_name"]`. Corpus and
  manifest are **unchanged** (`manifest.json` sha256
  `b42363011a1b9590…`, 32 cases = 31 `match` + 1 `documented_divergence` + 0
  `open_mismatch`); the conformance suite passes (`2 passed, 1 helper ignored`).

## 6. Verification performed

All Cargo work through `bash /tmp/sk-focused-cargo.sh`; temp HOME/XDG/TMP; no
workspace aggregate, release build, benchmark, or timing run; no Python oracle
regeneration.

```sh
# red, before the fix (real git repos + real hooks)
cargo test -p sk-git --locked --test merge_start   # 7 passed; 2 failed  13:26:28Z
cargo test -p sk-git --locked --test objects       # 9 passed; 4 failed  13:26:48Z
# green, after the fix
cargo test -p sk-git --locked                       # 246 passed; 0 failed; 3 ignored  13:41:33–13:43:12Z
cargo test -p sk-git --locked --test merge_start     # 13 passed          /tmp/sk-merge-conclusion-identity-green.log
cargo test -p sk-git --locked --test objects         # 13 passed          /tmp/sk-corrupt-head-and-blob-bound-green.log
cargo test -p sk-git --locked --lib merge::tests     # 3 passed (F1 preserved)  /tmp/sk-f1-preserved-green.log
cargo clippy -p sk-git --all-targets --locked -- -D warnings   # clean    /tmp/sk-git-remediation-clippy.log
cargo fmt    -p sk-git -- --check                    # clean
```

Final scoped run (`/tmp/sk-git-remediation-scoped.log`), 18 suite results:

| Suite | Passed | Ignored |
| --- | ---: | ---: |
| lib (`sk_git`) | 40 | 0 |
| acceptance | 27 | 0 |
| bounded_process | 7 | 0 |
| index_authority | 6 | 1 (helper) |
| lifecycle | 36 | 0 |
| merge_start | 13 | 0 |
| objects | 13 | 0 |
| remote_bounds | 5 | 0 |
| remote_head | 5 | 0 |
| remote_lifecycle | 10 | 0 |
| safe_commit | 57 | 0 |
| safe_commit_authority | 1 | 1 (helper) |
| safe_commit_concurrency | 2 | 0 |
| safe_commit_conformance | 2 | 1 (helper) |
| safe_commit_differential | 8 | 0 |
| sparse_checkout | 6 | 0 |
| worktree_prune | 4 | 0 |
| doctests | 4 | 0 |
| **Total** | **246** | **3** |

That is **+13** over the coordinator's F1 checkpoint (233 / 0 / 3): 7 merge
contract tests, 4 object/HEAD/capture tests, 2 conclusion-evidence unit tests.
The three ignored tests remain parent-exercised subprocess helpers. No test was
deleted, weakened, or newly ignored.

## 7. Source identity (current bytes)

| File | SHA-256 |
| --- | --- |
| `src/error.rs` | `ec6bca1bfd1077b4c7df39a9e4fe8340d14c04e94867782e714678b7b537b400` |
| `src/merge.rs` | `d8cfb383e9eab7b260245b52b00b6540c20753b1339fe7ce8d8d7a256979691f` |
| `src/objects.rs` | `6c9995c1b05974f0208e886dfa67d91eec6799e3b5b13f0ec39c4c6f79ee2115` |
| `src/process.rs` | `bf8f87df140d20d74819ded97e99c8ab04eeecb0ce278897db28ee35c680e0a6` |
| `src/repository.rs` | `ded86c1fa8fc6de4a9df7eb5abe43855bf2ad84dc21774ca26a72fccc95a728b` |
| `src/sparse.rs` | `b41280f794db27160f8c2e0c97d976f7fc4b66af552e786c0b78f5d2a34c1b30` |
| `src/lib.rs` | `f65e1481dbb51601c50cfeba6b5fbe7a3c70a7507d3529ca38c1ae0a1633c9e9` |
| `README.md` | `42c335e4697d37f41d4c1deb51febe21a79d7fe5686d050dac7fa4acbd1923b1` |
| `tests/merge_start.rs` | `cbdbe569416008277e2779b0514764cdea9998f13175ad57067b1ed7851a89e6` |
| `tests/objects.rs` | `29c5cdd8e252ce8f5599f72a875fce6cc3b42187b5121788ae1e9dcf53ec7e0a` |
| `tests/safe_commit_conformance.rs` | `8edceaa77c816a9545b68dfe5d754ac9fdb8f256d1808713946031f2293f053e` |
| `tests/fixtures/safe_commit/manifest.json` (unchanged) | `b42363011a1b95900a1d13a47cf91132b48dbd539c09b2027ef07b0163bcb90a` |

## 8. Residuals and integration changes required

1. **Caller handling of `MergeConclusionUnconfirmed` (not patched, outside
   ownership).** `crates/sk-workspace/src/planning.rs` currently runs
   `let _ = repository.merge_abort();` on *any* conclusion error. After a clean
   landing that path is harmless (`MERGE_HEAD` is gone → `NoOperationInProgress`),
   but if a hook re-created the marker, an abort there could discard a landed
   commit. The coordinator should special-case `MergeConclusionUnconfirmed`:
   surface it as landed-with-evidence (its `candidate`, `observed_head`), inspect,
   and never blind-abort. Within `sk-git`, nothing is rolled back.
2. **Status consumer comment.** `crates/sk-cli/src/compat/support.rs` still
   describes the seam as capping at 16 MiB; behaviour is now correct (it passes
   `sk_git::MAX_BLOB_BYTES` = 64 MiB). Comment-only, other owner's file.
3. **Not claimed.** No improvement to octopus merges (still refused), squash
   conclusions, rebase conclusions, `--no-verify`/signing policy (unchanged:
   `commit.gpgsign=false`, hooks never skipped), or Windows/macOS/SHA-256.
4. **Evidence sources.** If a repository has reflogs disabled *and* git's summary
   line is unavailable, a landed conclusion is detected as unconfirmed with
   `candidate: None` (fail-closed, evidence retained) rather than mis-reported;
   the observed `HEAD` is no longer used as a candidate fallback. This is
   deliberate: a parent-set match is not creation proof.
5. **Corpus qualification unchanged:** the conformance test replays committed
   oracle recordings; no Python was run in this continuation.
6. **Correction (later pass, 2026-11-02).** The original F2 wording above (and
   the earlier `HEAD` readback fallback) overstated the guarantee: a
   parent-set match is not evidence that *this* call created the commit. Both
   were corrected in `src/merge.rs`, `src/error.rs`, `src/lib.rs` and
   `README.md`: reflog evidence is now bounded to entries appended after a
   pre-invocation watermark on the pinned ref, the `HEAD` fallback is removed,
   started-process failures are reconciled as typed unconfirmed, and the marker
   is classified as consumed/unchanged/changed/unreadable. The source hashes in
   §7 are the earlier checkpoint's and are not the current bytes; current
   evidence is in `crates/sk-git/CONCLUSION-EVIDENCE-REPORT.md`.

## 9. Boundaries observed

* Edited only `crates/sk-git/**`; `cargo fmt`/`clippy`/`test` scoped with
  `-p sk-git`; no workspace manifest, lock, target-dir, release, aggregate or
  benchmark work; no `nohup`/poll loops; no subagents.
* F1 fix preserved byte-for-byte in intent and assertions; its red/green logs
  were not touched.
* No deletions of prior evidence; new logs listed above.
* No commit, push, worktree, or cache cleanup. `git rev-parse HEAD` is still
  `d64468a7693e4962712cbf3416d767440d22a649`.
* No production Python path, no new refusal of a previously valid input beyond
  the corrupt-`HEAD` taxonomy correction and the plan-time branch/root pins that
  mirror the existing merge-start plan.
