# Merge conclusion creation-evidence remediation — FINAL

> **STATUS: FINAL implementation evidence.** This pass closed the independent
> review's **F2-R (MEDIUM)** finding and the coordinator's static finding about
> `apply_merge_conclusion` propagating a bounded-process failure that can occur
> after the commit landed. It is implementation evidence, not independent
> approval, and not a workspace, CLI, parity, performance or release claim.
> Scope: `crates/sk-git/**` only. No source or manifest outside `sk-git` was
> edited; nothing was committed, pushed, or released.

| Item | Value |
| --- | --- |
| Baseline (and still current) HEAD | `d64468a7693e4962712cbf3416d767440d22a649` (`rust/rewrite`, frozen) |
| Input review | `/tmp/sk-rust-git-remediation-review.md` (FINAL; F2-R MEDIUM) |
| Prior report | `crates/sk-git/OPERATIONS-REMEDIATION-REPORT.md` (corrected in place) |
| Toolchain | rustc 1.99.1, git 2.43.0, Linux, SHA-1 repositories |
| Cargo seam | `bash /tmp/sk-focused-cargo.sh` (temp HOME/XDG/TMP, `--locked`, `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, existing `target/`, shared `flock` lease), `-p sk-git` only |
| Evidence root | `/tmp/sk-rust-git-remediation-review-logs/` — all pre-existing files preserved; new logs listed in §6 |
| Final scoped result | **256 passed, 0 failed, 3 parent-exercised helpers**, 18 suite results incl. 4 doctests |
| Manifest / corpus | `b42363011a1b9590…` unchanged: **32 cases = 31 `match` + 1 `documented_divergence` + 0 `open_mismatch`** |
| Kernel | **not changed and not re-run here**; the accepted 190-pass release correction remains independent |

## 1. What was wrong

The previous fix accepted a candidate commit when its real parent set was exactly
the pinned one, trying git's summary, then a reflog search over the last 50
`HEAD` entries, then the observed `HEAD`. The parent set is **necessary but not
creation proof**. The independent review reproduced two native cases, and the
coordinator noticed a third defect:

| # | Shape | Old behaviour |
| --- | --- | --- |
| **R1** | A prior identical `--no-ff` merge is still in `logs/HEAD`; the branch is reset to the pinned head, the same merge is re-started, and a `pre-commit` hook rejects the conclusion. | Reflog search returned the **pre-existing** merge (same parents, subject `commit (merge): …`), so the call was `MergeConclusionUnconfirmed` with a foreign `candidate` instead of a plain `CommandFailed`. |
| **R2** | A `post-commit` hook repoints the branch at a **pre-existing same-parent** commit, expires reflogs, and prunes the object this call created. | The summary id no longer resolved, the reflog had no entry, so the **`HEAD` readback** was accepted and the call returned a **false confirmed `Ok`** naming a commit this call never created. |
| **LATE** | The child `git commit` is killed by the bounded-process timeout/capture bound **after** the commit object and ref update, while a `post-commit` hook is still running. | `self.process().run(&args)?` propagated `Timeout`/`OutputTooLarge` as an ordinary retryable error; a caller could `merge_abort`/rewind a landed conclusion. |
| **MARKER** | A hook rewrites `MERGE_HEAD` to another tip, or makes it unreadable. | Any present marker was treated as untouched, so a completed failing command could still be classified as "nothing landed". |

## 2. Behaviour now

`MergeConclusionPlan` is unchanged (captured index, root, operation, `HEAD`,
branch, operation tip, message; F1 authority comparison still first).

`apply_merge_conclusion`:

1. re-proves every pinned precondition (unchanged F1/F2 plan-state pins);
2. captures a **publication mark** for the pinned reference
   (`refs/heads/<branch>`, or `HEAD` when detached) *immediately before* running
   the command: the newest raw `%H%x00%gs` reflog record, `Empty` when the ref had
   no entries, or `Unreadable` when it could not be read;
3. runs exactly the same argv as before
   (`git -c commit.gpgsign=false commit [--no-edit | -m msg]`) through a
   phase-aware seam. Hooks, identity and signing policy are untouched;
   `--no-verify` is never passed and no valid input is newly refused;
4. reconciles the post-state once (`HEAD`, marker tip) and identifies the commit
   this call created **only** from invocation-bound evidence:
   * git's own summary line printed by this child (git routes hook stdout to
     stderr; the first header whose commit carries the pinned parents wins), or
   * a reflog entry appended to the **pinned** reference **after** the
     publication mark. Entries at/below the mark are historical; a reflog that no
     longer contains the mark was rewritten and yields no candidate.
   The real parent set is still required, but **a parent match alone is never
   creation proof**, and the observed `HEAD` is never accepted as a candidate;
5. returns `Ok(MergeConclusionOutcome)` only when the command completed
   successfully, the proven created commit is the current `HEAD`, and the marker
   is `Consumed`;
6. returns `CommandFailed` only when the command **completed** with a failure
   status *and* the state proves nothing landed: `HEAD` is exactly the pinned
   commit, the marker is exactly `Unchanged`, and no created commit is
   identifiable (the rejecting-`pre-commit` contract, now independent of any
   historical reflog candidate);
7. turns a **started** process failure with no status (`Timeout`,
   `OutputTooLarge`, wait/readback `Io`) into
   `MergeConclusionUnconfirmed` with the candidate/observed/marker evidence and
   the process diagnostic. A proven pre-execution failure (temporary-file setup
   or spawn) remains an ordinary error, because nothing can have changed;
8. otherwise returns `MergeConclusionUnconfirmed` carrying `candidate` (`None`
   means **unknown**, never "not landed"), `previous_head`, `observed_head`, the
   retained `operation_consumed` bool, the new public `marker:
   OperationMarkerState` (`Consumed`/`Unchanged`/`Changed{recorded}`/`Unreadable`)
   and the probe notes. Nothing is reset, deleted, reverted or cleaned up.

`candidate` is therefore either a commit this call provably created (when the
published commit still resolves/is in the invocation-bound reflog) or `None`.
The docs that said "always the created commit" or listed the `HEAD` readback as a
last resort were corrected in `src/merge.rs`, `src/error.rs`, `src/lib.rs`,
`README.md` and the prior operations report.

### API compatibility

`plan`/`apply`, `MergeConclusionOutcome` and its accessors are unchanged. The
payload gains one public field (`marker`) and re-exports `OperationMarkerState`;
`operation_consumed` is retained and equals `marker == Consumed`. The payload is
still boxed. No external code constructs the payload (only `sk-git` does), so
downstream readers compile unchanged.

## 3. Red-first evidence (before the fix)

`/tmp/sk-rust-git-remediation-review-logs/conclusion-evidence-red.log`
(`test -p sk-git --locked --test merge_start`, 14:19Z; 14 passed / 4 failed):

| Case | Red observation |
| --- | --- |
| `a_prior_identical_merge_in_the_reflog_is_not_evidence_this_call_created_one` | `MergeConclusionUnconfirmed { candidate: Some(b6766f9c…), observed: 3330ee41…, consumed: false }` — the foreign historical merge, instead of `CommandFailed`. |
| `a_hook_that_repoints_head_to_a_foreign_same_parent_commit_is_not_a_confirmed_landing` | `Ok(MergeConclusionOutcome { new_head: 5d0df9ca… })` — the pre-existing foreign commit, a false confirmed landing. |
| `a_post_commit_hook_that_overruns_the_capture_bound_is_unconfirmed_not_retryable` | `OutputTooLarge { stream: "stderr", limit: 16777216 }` — an ordinary retryable error after the commit landed. |
| `a_post_commit_hook_that_amends_keeps_the_created_commit_as_the_candidate` | red for the test's first shape (`Timeout` from a self-recursive amend hook). That was a test-hook defect, not a product defect; the hook now guards recursion with `SK_AMENDING`, and the case is green. |

## 4. Green evidence and per-case matrix

All commands in `/tmp/sk-focused-cargo.sh`; final bytes bound in §5.

`/tmp/sk-rust-git-remediation-review-logs/conclusion-evidence-green-verified.log`:
**256 passed / 0 failed / 3 ignored**, 18 suite results including doctests
(`lib 42, acceptance 27, bounded_process 7, index_authority 6+1 helper,
lifecycle 36, merge_start 21, objects 13, remote_bounds 5, remote_head 5,
remote_lifecycle 10, safe_commit 57, safe_commit_authority 1+1 helper,
safe_commit_concurrency 2, safe_commit_conformance 2+1 helper,
safe_commit_differential 8, sparse_checkout 6, worktree_prune 4, doctests 4`).

That is **+10** over the reviewed 246: 8 new `merge_start` integration cases and
2 new unit cases (reflog boundary; process phase seam). No test was deleted,
weakened, or newly ignored; the same three helpers remain parent-exercised.

### 4.1 Required identity matrix (all native, real git)

| Contract | Test | Result |
| --- | --- | --- |
| Post-commit hook moves `HEAD` → unconfirmed, proven created merge commit, decoy not undone, work tree kept | `a_hook_moved_head_is_never_reported_as_the_created_conclusion_commit` | pass |
| Post-commit hook re-creates `MERGE_HEAD` → unconfirmed, `HEAD` = created, marker left in place | `a_conclusion_that_leaves_the_operation_marker_is_not_reported_as_success` | pass |
| Rejecting `pre-commit` on a clean history → `CommandFailed`, HEAD unchanged, marker intact | `a_rejecting_pre_commit_hook_is_a_refusal_that_leaves_the_operation_in_progress` | pass |
| **R1**: prior identical merge in the reflog + reset + rejecting hook → plain `CommandFailed`, no changes | `a_prior_identical_merge_in_the_reflog_is_not_evidence_this_call_created_one` | pass (was red) |
| **R2**: hook repoints at a pre-existing same-parent commit, expires reflogs and prunes the created object → typed unconfirmed, `candidate: None`, observed HEAD = foreign, no rollback | `a_hook_that_repoints_head_to_a_foreign_same_parent_commit_is_not_a_confirmed_landing` | pass (was red) |
| Noisy post-commit fake `[main 1111111]` summary → real created commit confirmed | `a_noisy_post_commit_hook_cannot_inject_a_fake_summary_candidate` | pass |
| Post-commit **amend** (same parent set, new object) → unconfirmed; candidate is the commit this call created, observed is the amended commit; both carry the pinned parents | `a_post_commit_hook_that_amends_keeps_the_created_commit_as_the_candidate` | pass |
| **LATE, landed**: post-commit hook overruns the 16 MiB capture bound → typed unconfirmed, created commit retained, never `OutputTooLarge` | `a_post_commit_hook_that_overruns_the_capture_bound_is_unconfirmed_not_retryable` | pass (was red) |
| **LATE, not landed**: pre-commit hook overruns the bound (and exits 1) → typed unconfirmed with `candidate: None`, marker `Unchanged`, no retryable NotLanded claim | `a_started_process_failure_before_landing_stays_typed_unconfirmed` | pass |
| **MARKER changed**: rejecting command rewrites `MERGE_HEAD` to another tip → unconfirmed, `marker: Changed{recorded}`, not `CommandFailed` | `a_failing_conclusion_that_rewrites_the_marker_is_not_a_plain_refusal` | pass |
| **MARKER unreadable**: rejecting command writes garbage to `MERGE_HEAD` → unconfirmed, `marker: Unreadable` | `a_failing_conclusion_that_makes_the_marker_unreadable_is_unconfirmed` | pass |
| Failing post-commit (exit 1) cannot change a confirmed landing; hook really ran | `a_failing_post_commit_hook_cannot_change_a_confirmed_conclusion` | pass |
| Clean `--no-ff` merge = 2 parents; conflicted cherry-pick/revert = 1 parent, markers consumed | existing conclusion/start tests | pass |
| F1 captured-index authority refuses before mutating | `merge::tests::conclusion_refuses_a_different_operator_index_before_mutating` | pass |
| Reflog evidence only above the publication watermark; historical entry rejected; rewritten reflog unbound | `merge::tests::{the_reflog_fallback_names_only_a_commit_with_the_pinned_parent_set, reflog_entries_older_than_the_publication_boundary_are_not_creation_evidence}` | pass |
| Pre-exec failure `NotStarted`, running timeout `StartedFailure` | `process::tests::a_spawn_failure_is_not_started_and_a_running_timeout_is_a_started_failure` | pass |

### 4.2 Preserved scoped contracts

* **F1** captured operator index: the authority comparison still runs first; its
  test and `a_merge_start_and_conclusion_write_only_the_captured_index` are green.
* **F3** corrupt-HEAD taxonomy: `objects` 13/13, including
  `a_corrupt_head_is_unreadable_not_a_missing_repository` and
  `an_empty_symbolic_head_target_is_unreadable_not_a_command_failure`.
* **F4** blob bounds: default **8 MiB**, explicit **64 MiB**; the exact-64-MiB and
  one-byte-over tests are green; capture limits are unchanged
  (16 MiB ordinary, 64 MiB explicit ceiling).
* **F6** sparse `mode()` documentation/behaviour: 6/6 sparse tests green.
* **F7** abort-with-untracked: the guarded-abort test is green.
* **F8** corpus: the conformance suite asserts the divergence names and
  `cases.len() == 32`; manifest SHA-256 is unchanged
  (`b42363011a1b9590…`), **31 exact + 1 named literal
  (`pathspec_magic_literal_name`) + 0 open**.

## 5. Final source identity and state

HEAD is still `d64468a7693e4962712cbf3416d767440d22a649`. Files changed by this
pass (SHA-256):

| File | SHA-256 |
| --- | --- |
| `crates/sk-git/src/merge.rs` | `8e276b70ba76c37af69ec2103653a3d884eda57fd37903812c756071854643ec` |
| `crates/sk-git/src/process.rs` | `f6a226a58bd6c850c82cb0349b4e23d8c2ce9a81237fabf2e1fdf8ac6036e6df` |
| `crates/sk-git/src/error.rs` | `7eb6697cfeed481bfc0c56fada97922fba0158c690464a7b0d077c0bbfb1325a` |
| `crates/sk-git/src/lib.rs` | `030db72da01882233eae0b8f2f2eba995ffeea53f80a65aedc207acd3f6ec71e` |
| `crates/sk-git/README.md` | `f63b7822f522805d1f4af252346ac6607cd6e18ff02f8c8e00ef407a65dcf498` |
| `crates/sk-git/tests/merge_start.rs` | `6da4ad7bea92ec9134d704d47d5ddc085f25027b46239a270939a537306c738e` |
| `crates/sk-git/OPERATIONS-REMEDIATION-REPORT.md` | `0da68bb4d1987727dd5086e70830c0416f2598a51b65f46aefb165276a8280d4` |
| `crates/sk-git/tests/fixtures/safe_commit/manifest.json` (unchanged) | `b42363011a1b95900a1d13a47cf91132b48dbd539c09b2027ef07b0163bcb90a` |

Only the seven `crates/sk-git/**` files above plus this report were written by
this pass. `crates/sk-kernel` was **not** edited (its pre-existing
`src/locks.rs` modification is the already-accepted release correction and was
not re-run here; `sk-kernel` was not a dependency of the scoped command).

## 6. Evidence inventory (all under `/tmp/sk-rust-git-remediation-review-logs/`)

New (nothing above it was modified):

| Path | Content |
| --- | --- |
| `conclusion-evidence-red.log` | 14:19Z red run: 14 passed / 4 failed, with the R1 foreign candidate, R2 false `Ok`, and late-failure `OutputTooLarge` |
| `conclusion-evidence-green-merge.log`, `…-green-merge-final.log` | intermediate green `merge_start` runs |
| `conclusion-evidence-green-merge-unit.log` | merge unit tests |
| `conclusion-evidence-green-sk-git.log`, `…-green-sk-git-final.log` | intermediate full scoped runs |
| `conclusion-evidence-green-verified.log` | **final 256 / 0 / 3**, 18 suite results |
| `conclusion-evidence-clippy.log`, `…-clippy-final.log`, `…-clippy-verified.log` | `clippy -p sk-git --all-targets --locked -- -D warnings`, exit 0 |
| `conclusion-evidence-fmt.log`, `…-fmt-apply.log`, `…-fmt-final.log`, `…-fmt-verified.log` | `fmt -p sk-git -- --check`, exit 0 (one apply of rustfmt to `sk-git` only) |

Pre-existing evidence in that directory (review probes, `sk-git-tests.log`,
`sk-kernel-tests.log`, `final-hashes.txt`, …) was read-only and remains intact.

## 7. Needed consumer changes (reported, not made)

`crates/sk-workspace/src/planning.rs` and `crates/sk-cli/src/compat/tasks/commit.rs`
are owned elsewhere and were not edited.

* `planning.rs` already handles `MergeConclusionUnconfirmed` without abort/rewind.
  Its `landed` test (`tip == candidate || is_ancestor(candidate, tip)`) remains
  safe with the corrected `candidate`: `None` yields `landed == false` and a typed
  `UnconfirmedMerge` (never `merge_abort`); a proven created commit is still the
  right witness. If the workspace wants the finer marker distinction, it must
  adopt the new `payload.marker` field; nothing requires it to compile.
* `sk-cli` maps `MergeConclusionUnconfirmed` to `CommitAttempt::Indeterminate`,
  which is the correct treatment for both started-process late failures and
  hook-moved conclusions.
* No external struct literal constructs `MergeConclusionUnconfirmed`, so the
  added field is source-compatible. The workspace's frozen 183-test snapshot and
  status' safe-commit consumer need no code change; their cached reports name the
  older field list and should be refreshed on their own schedule.

## 8. Residuals and limits

1. **Evidence availability.** With no resolvable summary and a disabled/rewritten
   reflog, a landed conclusion is reported unconfirmed with `candidate: None`.
   That is deliberate: unknown is not "not landed", and a parent-set match is not
   creation proof.
2. **Started failures are conservative.** A timeout/capture failure after spawn is
   always unconfirmed, even when the immediate state looks unchanged; without a
   completion status the process cannot be proven harmless. Only pre-execution
   failures are ordinary retryable errors.
3. **Amend/nested hooks.** A `post-commit` amend yields unconfirmed with the
   pre-amend commit as `candidate` and the amended commit as `observed_head`; the
   hook-created commit is not claimed as this call's creation. Hooks still run and
   are never skipped (`--no-verify` remains unused, no new refusal).
4. **Not claimed.** No change to octopus merges, squash/rebase conclusions,
   signing policy, reflog configuration, workspace/CLI/status behaviour, kernel
   release work, Windows/macOS/SHA-256, or any security threat model. R2's
   destructive hook is a correctness probe, not a hardened threat-model claim.
5. **Not run here.** Workspace aggregate, release build, packaging, benchmarks,
   timing, Python oracle regeneration, kernel suites. `sk-kernel` was untouched.
   F1/F3/F4/F6/F7/F8 were preserved by their existing scoped tests, not
   re-certified beyond the sk-git suite.
