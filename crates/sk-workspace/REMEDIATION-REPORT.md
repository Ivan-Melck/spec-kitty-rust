# `sk-workspace` remediation report

> **Historical for the findings below.** The independent review
> `/tmp/sk-rust-workspace-remediation-review.md` accepted most of this work and
> left residuals R1–R8. Their disposition, red/green evidence and the remaining
> gaps are in [`SECOND-REMEDIATION-REPORT.md`](SECOND-REMEDIATION-REPORT.md),
> which supersedes the "declared gaps" and the two claims corrected inline
> below (staged/dirty root-copy handling, and the `merge_base` primitive).

Status: **final for this recovery; self-attested, not an independent review.**
Scope: `crates/sk-workspace/**` only. No CLI, workspace manifest, or other crate
was edited. No commit, push, subagent, release/aggregate run, `cargo clean`, or
benchmark was performed.

Recovery context: the previous owner of workspace `c092b143` was cancelled
mid-tranche with no remediation report. The coordinator fixed exactly one
test-compile shadowing (`support::create_branch` inside
`undeclared_coord_mission`); it is preserved (`tests/coord_transaction.rs`).
The starting checkpoint log was `/tmp/sk-native-recovery-scoped.log`:
**152 passed / 4 failed**, `coord_transaction` 25 passed / 4 failed,
`planning_commit` 22 passed, doctests included.

Reference review: `/tmp/sk-rust-coordination-review.md` against `d64468a7`,
Python baseline `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.

## 1. Remaining failures: two defects, two stale expectations

Neither of the two test changes weakens a check: both now assert restoration
against the *seeded fixture bytes*, not against whatever the native code
returned.

| Failing test | Disposition | Why |
| --- | --- | --- |
| `bootstrap_rolls_back_a_branch_it_minted_when_the_surface_fails` | **source defect, fixed** | The failed `finish_bootstrap` left the newly materialized coordination worktree checked out on the minted branch. `rollback_mint` then asked the guarded deletion to delete a checked-out branch, which always refuses, so the mint survived a failed create. `rollback_mint` now re-takes the same claim + Mission locks, removes **only** a worktree that is registered on the minted tip and provably clean (`WorktreeDiscardPolicy::RefuseDirty`, expected head/branch pinned), then deletes the branch and preserves the tip under `refs/spec-kitty/coord-rollback/<slug>-<mid8>`. A worktree that cannot be proven removable leaves the branch in place and reports it. |
| `an_untracked_root_copy_is_removed_after_the_seed_commit` | **source defect, fixed** | `restore_root_copies` compared `git status` entries against the *composed* `kitty-specs/<slug>-<mid8>/…` path instead of the mission directory actually on disk (`kitty-specs/<slug>` for a pre-083 mission). Every root copy therefore classified as `clean` and the untracked copy was never removed, defeating the seed's single-home invariant. It now derives the repository-relative mission directory from `MissionWorkspace::mission_dir()` (with a lexical fallback) before matching status entries. Tracked-clean copies are left alone, **like** the reference; a staged-new or tracked-modified copy is preserved with a warning, which deliberately **diverges** from the reference (it runs `git restore --staged` / `git checkout --`, discarding the operator's uncommitted bytes). That divergence is intentional and is not claimed as parity. See `SECOND-REMEDIATION-REPORT.md` §R3 for the later correction of the same function: an untracked copy whose bytes differ from the committed coordination copy is now preserved too. |
| `an_aborted_transaction_restores_the_previous_bytes` | **stale expectation, corrected** | The fixture now seeds a *valid* JSONL row because the seed validates every row (`COORD_SEED_EVENT_LOG_MALFORMED` otherwise). The old assertion expected `# Decisions…`, bytes that can never legitimately be the pre-transaction content any more. The test now compares against the shared `DECISIONS_SEED_ROW` fixture constant — the actual previous bytes — so it still proves restoration. |
| `a_blocked_ref_advance_is_not_landed_and_the_write_is_restored` | **stale expectation, corrected** | Same fixture change; same correction. The test still proves the blocked advance is `NotLanded` and that the transaction's own write is restored. |

## 2. Review findings — disposition

All rows were re-read against the current source, not inferred from a green run.

| Finding | Disposition and location | Regression evidence |
| --- | --- | --- |
| **P1-1** mint before locks/protection | Fixed. `bootstrap_inner` resolves the destination, applies `ensure_unprotected`, takes `CoordLocks` (claim → canonical Mission lock), journals a `Mint` intent, then mints/validates; materialization/seed runs through the same `establish`. A refused protected create leaves no branch. | `bootstrap_refuses_a_protected_destination_before_minting_it`, `bootstrap_refuses_a_diverged_coordination_branch`, `bootstrap_rolls_back_a_branch_it_minted_when_the_surface_fails` |
| **P1-2** unlocked planning/integration | Fixed. `PlanningCommitPlan::apply` and `run_integrations` take `CoordLocks` and journal a `Commit`/`Integration` intent before mutating; the destination tip is re-read under the lock. | `a_planning_commit_lands_on_the_target_with_pinned_bytes`, `the_journal_is_mission_scoped_and_durable`, `a_later_conflict_rewinds_every_earlier_dependency_advance` |
| **P1-3** `single_branch` planning destination | Fixed. `Workspace::write_branch` is the one rule used by `PlanningCommitPlan::prepare` and by the planning-lane `integration_checkout`. | `a_single_branch_mission_commits_its_recorded_mission_branch`, `a_protected_target_is_refused` |
| **P2-4** pin lost-update / clobber | Fixed. `LanesStore::update` is one locked read-modify-write; `record_planning_commit_with` preserves an existing pin by default and only re-points on `PlanningPinMode::Replace`. | `a_pin_write_reads_the_current_manifest_under_the_lock`, `an_existing_pin_is_preserved_unless_explicitly_repointed`, `an_orphaned_or_foreign_recorded_pin_is_refused` |
| **P2-5** demotion guard fail-open | Fixed. `check_meta_demotion` reads the baseline from `HEAD` (`read_blob_at_head`), uses Python truthiness so `""`/`null` count as dropped, refuses an unparseable HEAD baseline, and has no caller-supplied-baseline bypass. | `an_empty_or_null_working_coordination_branch_is_a_demotion`, `a_preserved_coordination_branch_passes_the_demotion_guard`, `an_unborn_or_absent_head_baseline_is_a_first_commit`, `an_unparseable_head_baseline_is_refused` |
| **P2-6** seed byte-copy / single home / husk | Fixed. The seed restores COORD kinds from the branch tip, validates and prefix-merges both event logs (malformed/duplicate/fork refused with typed codes), carries root-only files, keeps the upstream seed trailer, removes untracked root copies (defect above), and removes a Mission directory it created on any failure. | `a_malformed_event_log_is_refused_rather_than_committed`, `a_forked_event_log_is_refused_rather_than_merged`, `an_untracked_root_copy_is_removed_after_the_seed_commit`, `a_failed_seed_commit_leaves_the_surface_empty_not_a_materialized_husk`, `a_lost_coordination_directory_is_restored_from_the_branch_tip` |
| **P2-7** recovery equality / `Abandoned` relabel / landed-then-`Err` | Fixed. `recover_interrupted` accepts a candidate reachable from the tip for every kind (not only `Integration`); `CoordJournal::begin` leaves an older intent pending instead of claiming `Abandoned`; a landed commit with a failed journal write is returned as landed with `journal_fault` attached. | `recovery_accepts_a_landed_candidate_that_was_built_on`, `recovery_completes_a_landed_commit_and_clears_an_unlanded_one`, `a_landed_commit_with_a_journal_fault_is_still_reported_as_landed`, `recovery_reports_an_indeterminate_tip_instead_of_guessing` |
| **P2-8** read authority / stray dir / undeclared branch | Fixed. `coord_read_surface` is the single policy used by both `read_coordination_surface` and `resolve_artifact_surface`: `Unmaterialized`/`Deleted` refuse for COORD kinds, only `None`/`Empty` degrade to primary. A stray unregistered directory is `COORDINATION_WORKTREE_STRAY_DIRECTORY` and is never adopted or deleted. A lost `coordination_branch` record derives the canonical name; a derived-and-absent branch is `COORD_BRANCH_UNDECLARED_AND_ABSENT`, not data loss. | `resolved_surface_matrix_is_exhaustive_over_kind_topology_and_state`, `a_stray_directory_at_the_coordination_path_is_typed_not_adopted`, `an_undeclared_coordination_branch_is_derived_when_it_exists`, `an_undeclared_and_absent_coordination_branch_is_not_reported_as_data_loss`, `a_deleted_coordination_branch_refuses` |
| **P2-9** dependency integration invariants | Fixed beyond the review's minimum: `(parallel_group, lane_id)` order for both lanes (planning included), missing branch skipped with a `SkippedDependency` receipt, already-ancestor skipped, divergent history gets a **real merge commit** (unrelated histories still `MergeRequired`), and the loop is all-or-nothing: a later failure rewinds to the exact pre-loop tip and the error carries the receipts and the rollback. | `dependencies_run_in_parallel_group_then_lane_order`, `a_missing_dependency_branch_is_skipped_with_a_receipt`, `a_later_conflict_rewinds_every_earlier_dependency_advance`, `a_divergent_lane_gets_a_real_merge_commit`, `unrelated_histories_report_merge_required_without_mutating`, `integration_resolves_the_lane_branch_through_the_one_naming_grammar` |
| **P2-10** YAML anchors/aliases/duplicate/multi-document | Fixed. The hand-written block scanner is gone; `serde_yaml_ng` parses one document, then `apply_merge()` folds `<<`; a non-mapping `protection` value is the absent-key arm; sequence items use Python `str()` coercion; duplicate keys and multi-document streams are typed `ProtectionConfig` refusals. | `an_anchored_or_aliased_list_is_read_not_defaulted`, `a_multi_document_stream_is_refused_like_the_reference`, `duplicate_protection_keys_are_refused_like_the_reference`, `scalar_items_keep_python_str_coercion`, `indentless_sequences_and_flow_comments_are_read`, `a_non_mapping_or_nested_protection_block_is_the_absent_key_arm` |
| **N6** numeric-slug lane naming | Fixed. `naming::legacy_branch_body` is the one helper (`_idempotent_legacy_body`): a stale `NNN-` is stripped only when the slug embeds its own mid8; a pure legacy `057-slug` keeps its prefix on `mission_branch_name` and `code_lane_branch_name`. | lib test `code_lane_body_matches_the_branch_grammar`; `integration_resolves_the_lane_branch_through_the_one_naming_grammar` |
| **P3-11** small items | `MissionHold::derived_path` is now `path()`; `CoordLocks::acquire` no longer takes an unused mission key; `acquire` re-probes under the locks; `stage_path` carries its unobserved-bytes contract; `Indeterminate` records stay terminal (`pending()` filters `Intent`). | `mission_hold_receipt_reports_the_recorded_legacy_key_and_actual_path`, `a_landed_commit_with_a_journal_fault_is_still_reported_as_landed` |

No temporary `Oid` placeholder remains: a source grep finds no empty/zero OID,
`todo!`, `unimplemented!`, or placeholder in `crates/sk-workspace`.

### Changes forced by the lint gate

`cargo clippy -D warnings` (the crate's checkpoint gate) refused the new
`IntegrationError` and `CoordBootstrapFailure` for `clippy::result_large_err`.
Both now box their large payloads (`IntegrationError::{error, report}`,
`CoordBootstrapFailure::error`). Only `sk-workspace` tests consume these types;
no other crate references them. `tests/planning_commit.rs` matches through
`failure.error.as_ref()`.

## 3. Verification evidence

All Cargo ran through `/tmp/sk-focused-cargo.sh` (temporary HOME/XDG/TMP,
`CARGO_HOME`/`RUSTUP_HOME` shared, `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`,
`--locked`, shared Cargo lease). Logs: `/tmp/sk-workspace-recovery-final.log`,
`/tmp/sk-workspace-recovery-clippy.log`,
`/tmp/sk-workspace-recovery-clippy-fulldeps.log`.

```
cargo test -p sk-workspace --locked
  → 162 passed, 0 failed, 0 ignored, 0 doctests
    51 lib (+ naming/parser unit tests)
    10 allocation
     7 artifact_home
     1 coord_lock_receipt
    29 coord_transaction
     4 lanes_store
     7 lifecycle_workspace
     4 owned
    22 planning_commit
    21 protection_policy
     6 topology_occupancy
     0 doc-tests

cargo clippy -p sk-workspace --all-targets --locked -- -D warnings
  → clean (also clean with --no-deps)
cargo fmt -p sk-workspace -- --check
  → clean
```

Recovery delta: the failed checkpoint's 4 failures are recovered; 6 protection
regressions were added; 152 → 162 tests.

### Oracle probe (isolated)

`scripts/rust/verify_oracle.py --root .` on the pinned
`.venv-rust-oracle/bin/python` (3.11.15, 53 requirements, no drift):
`{"ok": true, "drift": [], "missing": [], "unsatisfied": []}`.

The YAML expectations were re-derived, not copied from the review, by running
`ProtectionPolicy.resolve(root)` under that interpreter in a throwaway
`HOME`/`XDG`/`TMP` tree (real `git init` repositories). Observed:

```
anchored-block              OK    protected=['release']
alias-to-mapping            OK    protected=['release']
merge-key                   OK    protected=['release']
multi-document              ERROR ProtectionConfigError: expected a single document in the stream
duplicate-protection-keys   ERROR ProtectionConfigError: … found duplicate key "protection" …
duplicate-key-in-block      ERROR ProtectionConfigError: … found duplicate key "protected_branches" …
boolish-scalar              OK    protected=['master', 'yes']
indentless-sequence         OK    protected=['release']
flow-with-comment           OK    protected=['release']
protection-non-mapping      OK    protected=['main', 'master']
nested-not-top              OK    protected=['main', 'master']
```

The native tests assert exactly those answers (sequences as a set, error class
for the two refusals).

## 4. Declared gaps (not waived)

- **No independent review of this remediation** and no aggregate workspace
  test/clippy/release/benchmark; those remain coordinator-owned. This report is
  an owner's evidence, not an approval of `sk-git`'s safe-commit substrate.
- **Remote default branch.** Resolution reads the loose
  `refs/remotes/origin/HEAD` symref and the bounded `ls-remote --symref` seam;
  no `git remote show origin` / `describe_remote_head` fallback. That can
  protect *fewer* branches than the reference when the remote is unreachable.
- **History shapes.** Divergent histories now get a real merge.
  **Correction (review R7 / residual):** the earlier text here said "there is no
  `merge_base` primitive". That was false: `sk-git` exposes
  `Repository::merge_base` (`crates/sk-git/src/objects.rs`) and this crate calls
  it (`planning.rs`, the unrelated-history arm). What remained open was planning
  *ancestry* (two `is_ancestor` calls against the target), not a missing
  primitive — and unrelated histories are now a typed failure that rewinds the
  loop (see `SECOND-REMEDIATION-REPORT.md` §R7), not a mutation-free
  `MergeRequired` receipt.
- **`stage_path`** still stages caller-written bytes the transaction never
  observed or snapshotted (documented, upstream-equivalent). It is not an owned
  write and gets no byte rollback.
- **Concurrency.** Claim/Mission ordering and the journal are exercised
  functionally, but there is no stress/fault-injection harness for a racing
  writer, and the seed/commit behaviours inherit whatever the safe-commit
  substrate does.
- **Platforms.** Only Linux/Git was run; Windows/macOS path, hook and atomic
  replacement behaviour is not claimed.
- **CLI wiring** is out of scope and remains absent; this crate is not
  production-complete.
- **`COORDINATION-REPORT.md`** remains the historical pre-review report; its top
  note and the planning/integration sections now point at this file and are
  corrected where they contradicted the remediated source.
