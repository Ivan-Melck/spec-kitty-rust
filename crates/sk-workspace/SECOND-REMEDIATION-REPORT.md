# `sk-workspace` second remediation — residuals R1–R8

Status: **DELIVERED — implementation evidence, not an independent review.**

Coordinator addendum: after the owner's 182-pass checkpoint, a real-oracle
regression reproduced the declared Unix undecodable-path lock mismatch. The
fallback digest now preserves filesystem bytes through Python-compatible
`surrogateescape` JSON escapes rather than U+FFFD. Valid UTF-8 still uses the
same ASCII JSON encoding. Coordinator verification: **183 passed, 0 failed**
(13 suites including doctests), scoped Clippy clean; logs
`/tmp/sk-lanes-surrogate-path-red.log` and
`/tmp/sk-ws-second-remediation-coordinator{,-clippy}.log`. The owner checkpoint
counts/hashes below precede this localized correction; independent re-review
is still required.
Scope: `crates/sk-workspace/**` only. No CLI, workspace manifest, another crate,
or kernel file was edited; no commit, push, subagent, release/aggregate run,
`cargo clean`, or benchmark. All Cargo ran through
`bash /tmp/sk-focused-cargo.sh` (temporary `HOME`/`XDG_*`/`TMPDIR`, shared
`CARGO_HOME`/`RUSTUP_HOME`, existing `target/`, `CARGO_BUILD_JOBS=2
CARGO_INCREMENTAL=0`, `--locked`, one Cargo lease).

Input review: `/tmp/sk-rust-workspace-remediation-review.md` (FINAL). Baseline
HEAD: `d64468a7693e4962712cbf3416d767440d22a649` (unchanged; this tranche is
uncommitted source). Prior reports kept: `REMEDIATION-REPORT.md` (its two false
claims corrected inline, see §7) and `COORDINATION-REPORT.md`.

**CLI wiring remains unwired and unapproved.** R7 (merge-driver parity) is
declared, not waived; the remote-default fallback gap stays open.

---

## 1. What was requested and what changed

| # | Residual (review) | Status | Where the fix lives |
| --- | --- | --- | --- |
| R1 | Coordination Mission directory composed with the NNN-**stripping** composer | **Fixed** | `workspace.rs::coord_mission_dir_name` (new), `probe_coordination`, `coord.rs::establish`/`seed_coordination_surface`, `topology.rs::coordination_mission_dir` |
| R2 | A dependency on the **planning lane** resolved with `code_lane_branch_name` (which refuses it) | **Fixed** | `planning.rs::integrate_dependency_lanes` now resolves through `naming::lane_branch_name(.., target_branch)` |
| R3 | An **untracked, differing** root copy was unlinked and reported as preserved | **Fixed** | `coord.rs::restore_root_copies` + `root_copy_is_reproduced`; receipt `preserved_root` is now structural |
| R4 | `lanes.json` lock in-tree (`<mission_dir>/.lanes.json.lock`) instead of the reference's common-dir path | **Fixed** | `lanes.rs::lanes_json_lock_path`, `LanesStore` (canonical lock by default, unbounded/reentrant like the reference) |
| R5 | `recover_interrupted` proved/resolved **without** the claim + Mission locks | **Fixed** | `coord.rs::recover_interrupted` takes `CoordLocks` first, then reads every fact under the hold |
| R6 | Integration journal faults discarded (`let _ = journal.resolve(..)`) | **Fixed** | `planning.rs`: one `resolve_intent` helper, `IntegrationReport::{journal_path, journal_fault}`, faults in the failure text and code |
| R7 | No ephemeral merge-driver activation; unrelated histories did not fail the all-or-nothing loop | **Unrelated histories fixed; driver parity declared** | `planning.rs`: unrelated histories are a typed failure that rewinds; the driver gap is documented with the precise API it needs (§5) |
| R8 | Event logs decoded lossily (U+FFFD rewrite) | **Fixed** | `coord.rs::decode_seed_text` — strict UTF-8, pathful refusal, no rewrite |

Also folded in, from the Git owner's completion message (`ac2df51d`): the
integration protocol now handles `GitError::MergeConclusionUnconfirmed` as
**possibly landed** — it is never aborted, never rewound, and its candidate
evidence is retained (§4).

## 2. Per-residual detail

### R1 — the coordination home keeps a legacy `NNN-` prefix

`MissionWorkspace::coord_mission_dir_name()` is the VERBATIM composer
(`naming::coord_mission_dir_name`), and it is now the **only** composer used by
the coordination probe, the read surface's `coordination_mission_dir`, the
transaction's `feature_dir`, and the seed. `canonical_mission_dir_name()` (the
NNN-stripping one) stays for primary-side names (mission branch body, the
primary mission directory name), with a doc comment stating which is which.
Branch names were already verbatim and are unchanged.

Regression: `a_legacy_numeric_slug_keeps_its_prefix_in_the_coordination_home`
— a `057-lanehead` mission with a reference-created coordination worktree
carrying `kitty-specs/057-lanehead-<mid8>/status.events.jsonl` probes
`Materialized`, the read surface answers that directory, the transaction reuses
it (`seed().is_none()`), a write lands there, and the stripped
`lanehead-<mid8>` directory never appears in any checkout. The test also runs the
reference's own `coord_mission_dir_name` and `mission_dir_name` for that slug
under the pinned oracle and asserts native matches the first and differs from the
second, so "verbatim, not stripped" is proven against the reference, not
asserted by prose.

### R2 — a planning-lane dependency resolves to `target_branch`

The loop resolves every dependency through
`naming::lane_branch_name(&manifest.mission_slug, &lane_id,
&manifest.target_branch)`, exactly like `worktree_allocator._merge_dependency_lane_tips`
→ `lane_branch_name(..., target_branch=…)`. `code_lane_branch_name` (which
refuses the planning lane by design) is no longer used on this path; the
code-lane branch name it produces is unchanged for every other lane.

Red → green: flipping the call back to `code_lane_branch_name` reproduces the
old refusal (`InvalidArtifact { artifact: "lane id" }`,
`/tmp/sk-ws-r2-planning-lane-red.log`). Green:
`a_dependency_on_the_planning_lane_resolves_to_the_target_branch` proves the
receipt names `work` (the target branch) and the lane fast-forwards to it.

### R3 — an uncommitted, differing root copy is never deleted

The old step deleted *every* untracked candidate path, including ones the
branch tip carries with **different bytes** that were committed nowhere, and the
receipt derived `preserved_root` by parsing warning text — so it could report a
deleted file as preserved.

The new rule is per path, source-backed, and structural:

* an untracked root copy is removed only when the durable coordination content
  **reproduces** its bytes — byte equality for a non-log file, row-prefix
  containment for an event log (the seed carried the tail, or the root stream is
  an already-committed prefix);
* an untracked copy that differs is **authored divergence**: it is kept, named in
  `preserved_root`, and a warning says so. Nothing silently claims otherwise;
* staged-new / tracked-modified copies stay preserved (deliberate divergence from
  the reference, which discards them) and tracked-clean copies are left alone
  (same as the reference);
* the restore runs on **every** successful seed outcome, including the
  `SafeCommitNothingToCommit` "already durable at the tip" arm, which is the
  exact shape the review described.

Red (both shapes, `/tmp/sk-ws-r3-rootcopy-red.log`, produced by flipping the
duplicate test to the old "delete any untracked copy" decision):
`a_differing_untracked_root_copy_is_preserved_not_deleted` and
`a_differing_root_copy_survives_an_unchanged_seed` both fail with
`the operator's root copy survives: NotFound`.

Green (exact bytes are asserted, not prose):

* the operator's bytes are
  `{"authored_by": "operator", "wps": {"WP01": "in_progress"}, "version": 7}\n`
  while the coordination tip holds `{"authored_by": "reference-seed", "wps": {}}\n`;
  after the seed the root file is byte-identical, `preserved_root ==
  ["status.json"]`, `restored_root` does not name it, the committed coordination
  copy is unchanged, and the divergence is warned;
* the unchanged-seed variant proves `landed.is_none()`, the same preservation,
  and an exhaustive receipt;
* the equal-bytes variant still removes the duplicate (`restored_root` names it,
  `preserved_root` is empty), so single-home restoration is not weakened.

### R4 — the `lanes.json` lock is the reference's lock

`lanes_json_lock_path(feature_dir)` mirrors
`specify_cli.lanes.persistence.lanes_json_lock` step for step:

1. `resolve_lanes_dir(feature_dir).resolve()` — the resolved manifest path;
2. `git_common_dir`, then `git_toplevel`;
3. `<common>/spec-kitty-lanes-locks/<lanes.json relative to the worktree>.lock`;
4. `NotAGitRepository` → `<tempdir>/spec-kitty-lanes-locks/<sha256>.lock` over
   `json.dumps(as_posix(), ensure_ascii=True, separators=(",", ":"))`
   (`_non_git_lanes_lock_path`, reproduced byte-exactly including non-ASCII
   escaping);
5. any other topology failure — and a manifest outside its own worktree — is a
   typed refusal (`LANES_LOCK_TOPOLOGY_UNRESOLVED` / `LANES_LOCK_OUTSIDE_WORKTREE`).
   **No** improvised fallback to an in-tree lock.

`LanesStore::new` now resolves that path at lock time and waits unbounded and
reentrant, matching `machine_file_lock(blocking=True, timeout_s=None,
reentrant=True)`. `with_lock_path` remains an explicit override for
tests/embedded callers and is documented as bypassing the authority.

Evidence (`tests/lanes_lock_authority.rs`, 7 tests, real oracle):

* native path == the reference's own `lanes_json_lock(dir).lock_path` for a real
  repository (`the_lanes_lock_is_the_reference_common_dir_path`);
* the primary checkout and a lane worktree name **one** lock
  (`a_lane_worktree_locks_the_same_manifest_the_primary_checkout_does`);
* a non-Git project's digest path == the reference's
  `_non_git_lanes_lock_path` for a non-ASCII (`café`) path;
* a manifest that resolves outside its worktree is refused;
* **mixed contention, both directions**, over the same lock file:
  a Python `lanes_json_lock` holder makes the native `update` wait (and the
  native call is observed *before* waiting on the child, so a non-contending
  writer fails), and a native `FileLock` holder makes the Python writer report
  `contended` (then `acquired` after release);
* a store write leaves `git status --porcelain` **empty** and no
  `<mission_dir>/.lanes.json.lock`, so a later `fast_forward`/rollback is no
  longer refused as `INTEGRATION_CHECKOUT_DIRTY`.

The serialized read-modify-write (`update`), the pin CAS behaviour and
`record_planning_commit`'s `PreserveExisting` semantics are unchanged.

### R5 — recovery runs under the claim + Mission locks

`recover_interrupted` takes `CoordLocks::acquire` **before** it reads anything,
on the same checkout a transaction for that mission takes (the coordination
worktree when the topology routes through it, otherwise the primary root), then
reads the pending set, the branch tips and the worktree registry under the hold.
The doc comment no longer implies unserialized behaviour.

Red → green: removing the acquire reproduces the unlocked pass
(`/tmp/sk-ws-r5-unlocked-recovery-red.log`, "recovery only made progress after
the claim was released"), because a Python holder owns the canonical
checkout-claim lock and the test observes the native return **before** waiting on
the child. Green:
`recovery_holds_the_claim_and_mission_locks_while_proving_its_verdicts` also
asserts the verdict was resolved (nothing pending) under the hold.

### R6 — integration journal faults are reported, never dropped

One `resolve_intent` helper (used by the planning commit, the seed and the
integration loop) returns the fault instead of discarding it.
`IntegrationReport` gained `journal_path` and `journal_fault`; a landed loop
returns `Ok` with the fault attached (a landed mutation is never turned into a
failure), and a failed loop reports the fault in the message and switches the
code to `INTEGRATION_FAILED_WITH_JOURNAL_FAULT`.

Evidence: `a_landed_integration_journals_its_applied_outcome` (no fault, journal
record `Applied`), `an_unreadable_journal_stops_the_loop_before_any_mutation`
(no durable intent ⇒ no mutation), and the unit test
`planning::tests::a_journal_fault_is_returned_never_discarded` (the intent is
durable, then the document breaks — the helper returns the fault).

The failure path also preserves the step's own typed code
(`INTEGRATION_MERGE_CONFLICT`, `INTEGRATION_UNRELATED_HISTORIES`, …) instead of
flattening everything into `INTEGRATION_FAILED`; the all-or-nothing rewind and
its receipt are unchanged.

### R7 — driver parity declared; unrelated histories now fail the loop

Two separate things were hidden under R7.

**(a) Unrelated histories — fixed.** `git merge` refuses an unrelated-history
merge, and the reference then aborts and resets to its pre-loop ref (#1915). A
mutation-free `MergeRequired` receipt instead let an **earlier** dependency's
advance survive, the opposite of all-or-nothing. `MergeRequired` is gone from
`IntegrationOutcome`; the step returns a typed
`INTEGRATION_UNRELATED_HISTORIES` "not landed" failure, so the loop rewinds and
keeps its receipts. Tests:
`unrelated_histories_fail_the_loop_instead_of_leaving_it_unintegrated` and
`a_later_unrelated_dependency_fails_and_rewinds_earlier_advances` (earlier
fast-forward applied, later unrelated step fails, tip back at the pre-loop base,
`applied()` still names the earlier lane).

**(b) Ephemeral merge-driver activation — declared, not implemented.** Upstream
runs both merges inside `_ephemeral_merge_driver_activation`, which appends the
registry's attribute lines to `$GIT_COMMON_DIR/info/attributes` for one merge and
removes exactly those lines afterwards. Implementing it faithfully needs three
seams that do not exist, and faking any of them would be worse than the gap:

1. the driver **commands** (`spec-kitty merge-driver-event-log`, `-meta`,
   `-traces`, `-acceptance-matrix`, `-issue-matrix`, `-review-cycle`,
   `-decision-index`) — no crate implements them; `sk-install::attributes`
   already says its plan configures drivers "but not functional yet";
2. a **git-config write seam** — `sk-git` has none (that is why `sk-install`
   emits a plan instead of applying it);
3. an **info/attributes seam** with the exact append/remember/remove-exactly
   semantics.

Precise API proposal (owning crates; no kernel change):
`sk_git::GitRepository::{set_local_config, remove_local_config}`,
`sk_git::GitRepository::{read_info_attributes, write_info_attributes}`
(common-dir `info/attributes`, atomic), and
`sk_workspace::planning::ephemeral_merge_driver_activation(repository) ->
MergeDriverActivation` returning the appended lines so `Drop` removes exactly
them. Until then a `kitty-specs/**` conflict aborts and rewinds (fail-closed, no
data loss). The same text is in the `planning.rs` module docs so it cannot drift
away from the code.

### R8 — event logs are decoded strictly

Both logs are decoded with `String::from_utf8` and a pathful refusal
(`COORD_SEED_EVENT_LOG_NOT_UTF8`, byte offset, the reference's
`encoding="utf-8"` behaviour) instead of `from_utf8_lossy`. Nothing is written
when the decode fails, so the authored bytes are preserved and no U+FFFD
rewrite can be committed.

Red → green: restoring the lossy decode
(`/tmp/sk-ws-r8-utf8-red.log`) makes
`a_non_utf8_event_log_is_refused_with_its_path_and_never_rewritten` fail with a
successful `CoordTransaction` whose seed **committed** the log (a JSON row is
valid after substitution). Green: the typed refusal names
`status.events.jsonl`, the on-disk bytes are unchanged, and no husk is left.

## 3. Git dependency (owner `ac2df51d`) — hashes and API bindings

`sk-workspace` compiles only against the Git owner's current tree. Reviewed
snapshot (sha256, first 16 hex), and what this crate binds to:

| File | sha256 (16) | Surface used here |
| --- | --- | --- |
| `crates/sk-git/src/error.rs` | `ec6bca1bfd1077b4` | boxed `MergeConclusionUnconfirmed { operation, worktree_root, candidate, previous_head, observed_head, operation_consumed, detail }` |
| `crates/sk-git/src/merge.rs` | `d8cfb383e9eab7b2` | `plan_merge_conclusion`, `apply_merge_conclusion`, `merge_abort` |
| `crates/sk-git/src/merge_start.rs` | `3af5ca4d728775d2` | `plan_merge_start`, `apply_merge_start`, `MergeStartStatus` |
| `crates/sk-git/src/objects.rs` | `6c9995c1b05974f0` | `read_blob_at_bounded` (now 64 MiB explicit), `read_blob_at_head`, `tree_paths_at`, `merge_base` |
| `crates/sk-git/src/repository.rs` | `ded86c1fa8fc6de4` | `open`, `read_branch_tip`, `is_ancestor`, `current_branch`, `worktrees`, `status_with`, `remote_branch_lookup` |
| `crates/sk-git/src/safe_commit.rs` | `1b858f72a408456a` | `safe_commit` + landed/unconfirmed outcomes |
| `crates/sk-git/src/worktree_ops.rs` | `3dc48af4bbb545d8` | `plan/apply_worktree_removal` (mint rollback), `plan_worktree_add` |
| `crates/sk-git/src/branch.rs` | `603fc98f0759485b` | branch advance/deletion CAS, `require_clean_checkout` |
| `crates/sk-git/src/process.rs` | `bf8f87df140d20d7` | bounded capture |
| `crates/sk-git/src/lib.rs` | `f65e1481dbb51601` | re-exports (`MAX_BLOB_BYTES`, `MergeConclusionUnconfirmed`) |

Two consequences of that owner's completion, both consumed here:

* `MAX_BLOB_BYTES` is now the canonical **64 MiB** status-log bound with the
  default still 8 MiB. `MAX_PLANNING_ARTIFACT_BYTES` and
  `MAX_SEED_FILE_BYTES` both use it, so the seed no longer refuses a legitimate
  event log above the old 16 MiB local cap.
* `apply_merge_conclusion` reports a possibly landed conclusion as
  `MergeConclusionUnconfirmed` and never resets anything, so the caller must not
  abort. See §4.

## 4. `MergeConclusionUnconfirmed` protocol (Git owner's integration request)

`planning::integrate_into_checkout` matches that error variant **before** any
other conclusion error and:

1. **never** calls `merge_abort` on that arm (a hook may have re-created
   `MERGE_HEAD` *after* the commit landed);
2. re-reads the destination tip and only treats the merge as landed when the
   candidate (which `sk-git` only names with the pinned parent set) is the tip or
   reachable from it; an unreadable ancestry answer is **not** evidence of no
   landing;
3. on `landed && operation_consumed` returns a real `Merged` receipt with the
   substrate's diagnostic in `LandedMerge::unconfirmed_detail` and continues;
4. otherwise fails the loop as `INTEGRATION_MERGE_UNCONFIRMED` with an
   `UnconfirmedMerge` evidence record (`operation`, `candidate` — `None` means
   **unknown, never "not landed"** —, `previous_head`, `observed_head`,
   `operation_consumed`, `destination_tip`, `landed`, `detail`), journals
   `Indeterminate`, and performs **no rollback**;
5. `From<IntegrationError> for WorkspaceError` returns that typed error rather
   than the rolled-back/refused shape, so a landed merge is never reported as an
   ordinary retryable failure.

Only the remaining conclusion errors (e.g. a rejecting `pre-commit` hook, which
leaves the pinned `HEAD`, the marker and the index untouched) abort the
half-merge and join the normal all-or-nothing rewind.

Test (real repository, real hook):
`a_hook_restored_merge_marker_is_never_aborted_or_rewound` installs a
`post-commit` hook that rewrites `MERGE_HEAD` with the source tip. It asserts the
`INTEGRATION_MERGE_UNCONFIRMED` code, `landed == true`, the destination tip **is**
the created commit (not the pre-merge one), the marker is still present (no
abort), no rollback, no "rolled" text, and a `Merged` receipt in the report.

## 5. Verification (this tranche, scoped)

All Cargo through `/tmp/sk-focused-cargo.sh`; `--locked`; no aggregate, release,
benchmark or oracle regeneration.

```sh
bash /tmp/sk-focused-cargo.sh test -p sk-workspace --locked
  → 182 passed, 0 failed, 0 ignored, 0 doctests   # /tmp/sk-ws-second-remediation-scoped.log
bash /tmp/sk-focused-cargo.sh clippy -p sk-workspace --all-targets --locked -- -D warnings
  → clean                                          # /tmp/sk-ws-second-remediation-clippy.log
bash /tmp/sk-focused-cargo.sh fmt -p sk-workspace -- --check
  → clean
```

| Suite | Passed | Delta vs the review's 163 |
| --- | ---: | ---: |
| lib (`sk_workspace`) | 52 | +1 (`a_journal_fault_is_returned_never_discarded`) |
| allocation | 10 | — |
| artifact_home | 7 | — |
| coord_lock_receipt | 1 | — |
| coord_transaction | 35 | +6 (R1, R3 ×3, R8, R5) |
| lanes_lock_authority | 7 | +7 (new file, R4) |
| lanes_store | 5 | +1 (lock-path expectations split out) |
| lifecycle_workspace | 7 | — |
| owned | 4 | — |
| planning_commit | 27 | +5 (R2, R6 ×2, R7 ×2; the old `MergeRequired` test replaced) |
| protection_policy | 21 | — |
| topology_occupancy | 6 | — |
| doctests | 0 | — |
| **Total** | **182** | **+19** |

Red logs (each produced by a one-line behaviour flip, then reverted; the
fixed source is restored and re-verified green):

* `/tmp/sk-ws-r3-rootcopy-red.log` — `2 failed` (both R3 shapes, file deleted).
* `/tmp/sk-ws-r8-utf8-red.log` — `1 failed` (non-UTF-8 log **committed**).
* `/tmp/sk-ws-r2-planning-lane-red.log` — `1 failed` (`InvalidArtifact` on the
  planning lane).
* `/tmp/sk-ws-r5-unlocked-recovery-red.log` — `1 failed` (recovery returned
  mid-hold).

Preserved/untouched evidence: `/tmp/sk-lane-reader-defaults-red.log`, the
review's 163-test logs, `/tmp/sk-merge-index-binding-scoped.log`, and the Git
owner's reports/logs.

The pre-existing 4-field lane-reader default correction
(`omitted_optional_lane_fields_use_the_reference_defaults`) is preserved and
still green.

## 6. Source identity (sha256, owner checkpoint before coordinator addendum)

```
src/coord.rs        7d3533125b25c64bbea8fb3380f2703d0c744aa4a450e0d01dc98a653ea0de04
src/planning.rs     a64376589aa7137a78645502fe2cb6a498969daf47957c5d99ddbef7bd17872f
src/lanes.rs        e12254b6ed7cb57ec24b6dd84be891ae6cea82bf5be0f8b2507d061f1392216a
src/workspace.rs    14460224d4faf646d1eb5f48e68120afab90e8f94981268a7d3fc70a9bcc7393
src/topology.rs     9962ae519011aed74d0dce49a7ac17024cca5c7914ba00061e2ebf0d4eb84edc
src/lib.rs          652b925fc871cce06c5dffc44ad2a034a8cc2e7770c3f0d007d3600612d07860
tests/coord_transaction.rs   c34e6d0bc8110bc8efd1910abb0076a5753507e6b887b3cc6a6082892cacb4ab
tests/planning_commit.rs     36ef70c1b235af9876323e50452317c6b1a11bf2baebfbd7a88bf70362ca2158
tests/lanes_store.rs         182dc88f818119e7a788532789e0201db7f9866d353c6d3c5507a4e984d1ce6e
tests/lanes_lock_authority.rs 5a6933d076704213b7e7d0d91dcc4dc1bd33ae6afef37e9a7cdbd98c5c10829e
tests/support/oracle.rs      164f80033806f822e7923e75c5c58bbd015b284c56de697286a98527bf41bc46
```

Unchanged from the reviewed tranche: `src/naming.rs e389b82b…`,
`src/artifact.rs f8c5ac8a…`, `src/protection.rs ac5cfa0b…`, `src/surface.rs
61cd60b3…`, `src/error.rs 0a50fa0f…`.

## 7. Corrections to `REMEDIATION-REPORT.md`

* "staged/dirty root copies are still preserved with a warning, **matching the
  reference**" — false. Native preserves them deliberately; the reference
  unstages+deletes / `git checkout --`s them. The text now says so and does not
  claim parity.
* "**there is no `merge_base` primitive**" — false. `sk-git` exposes
  `Repository::merge_base` (`objects.rs`) and `planning.rs` calls it. The open
  item was planning *ancestry* (two `is_ancestor` calls), and unrelated histories
  are now a failure with a rewind rather than a mutation-free receipt.

## 8. Residuals (not waived)

1. **Merge-driver activation (R7b)** — declared above with the precise API. A
   `kitty-specs/**` bookkeeping conflict aborts and rewinds where upstream
   union/field-merges. Fail-closed, no data loss, real behaviour gap.
2. **Remote default branch** — still `origin/HEAD` plus a bounded
   `ls-remote --symref`; no `git remote show origin` fallback. Can protect fewer
   branches when the remote is unreachable. Direction unchanged, still declared.
3. **No platform/stress evidence** — Linux/Git only. The claim/Mission ordering,
   the seed/commit and the new recovery locking are exercised functionally and by
   real cross-language contention tests, but there is no fault-injection harness
   for a racing writer, and no Windows/macOS run.
4. **Unix non-UTF-8 fallback digest corrected after delivery** — see the
   coordinator addendum. The new real-oracle regression mixes valid Unicode,
   an invalid byte and a UTF-8 surrogate encoding and now names the same lock as
   Python. This does not establish Windows path/casefold parity.
5. **Windows casefold** — `_non_git_lanes_lock_path` casefolds a
   `PureWindowsPath`; not implemented (no Windows run).
6. **`LanesStore::write`** remains an unconditional clobbering API (one test
   caller, no production caller); `update` is the CAS.
7. **`stage_path`** still stages caller-written bytes the transaction never
   observed (documented, upstream-equivalent).
8. **Other best-effort journal writes** — the `Drop`/abandon path and the
   bootstrap rollback still discard their journal fault (there is no receipt to
   attach it to). Only the commit, seed, planning-commit and integration paths
   report it.
9. **No CLI wiring** (unwired, unapproved). `sk-cli` still only reads
   `LanesStore`; nothing in this tranche implies CLI approval.
10. **Not claimed**: aggregate workspace tests, release build, benchmarks,
    performance, upstream Python-suite parity, or any independent approval.

## 9. Boundaries observed

* Edited only `crates/sk-workspace/**`. `crates/sk-git/**` (owner `ac2df51d`),
  `sk-cli`, the workspace manifests/`Cargo.lock`, `docs/rust/*` (coordinator-
  owned `DECISIONS`/`LOOP`/`VERIFICATION`), the inventory and the benchmark
  harness were **not** touched.
* No commit, push, worktree, staging, subagent, `cargo clean`, release build,
  aggregate workspace run, benchmark or oracle regeneration.
* Every Cargo invocation used `/tmp/sk-focused-cargo.sh` (one shared lease,
  existing `target/`, temporary `HOME`/`XDG_*`/`TMPDIR`); no `nohup`, `sleep` or
  poll loop was used to drive anything.
* The two "red" runs were one-line behaviour flips of the fixed source, each
  reverted immediately and re-verified green; the fixed `coord.rs`/`planning.rs`
  hashes in §6 are the bytes that produced the 182-pass run.
* Git HEAD is still `d64468a7693e4962712cbf3416d767440d22a649`; this tranche is
  uncommitted source, and the immutable-commit evidence quoted above belongs to
  the Git owner's report, not to a commit created here.
