# sk-status remediation report — recovered status owner `6cd7c445`

Status: **FINAL — six stale-expectation classes repaired, regression suite added, scoped verification green.**

- Repository: `spec-kitty-rust`, branch `rust/rewrite`, baseline commit
  `d64468a7693e4962712cbf3416d767440d22a649` plus the cancelled owner's
  preserved uncommitted edits (no `REMEDIATION-REPORT.md` existed).
- Reference: `spec-kitty/spec-kitty` `9aed3a8012cb5eb22c0128b14d525039f95fea7a`
  (pinned Python oracle `.venv-rust-oracle`, Python 3.11.15).
- Toolchain: rustc 1.99.1; every Cargo invocation through
  `bash /tmp/sk-focused-cargo.sh` (temporary HOME/XDG/TMP, retained build cache,
  `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, shared `flock`).
- No commit, push, manifest/lock change, release/aggregate build, timing run,
  subagent, whole-workspace check or Python production change was made.
- Scope: `crates/sk-status/**` (except the frozen Mission-lock mechanics),
  `crates/sk-cli/src/compat/{status,tasks}.rs`, status-specific additive
  `crates/sk-cli/src/compat/support.rs`, and
  `crates/sk-cli/tests/status_mutation.rs`. The already-added `sk-mission`
  read-only legacy metadata API was retained unchanged (no invented IDs).
  `compat/mod.rs`, `compat/capabilities.rs`, other tests, manifests and other
  crates were not edited.

---

## 0. Why this report exists

The status owner was cancelled with its edits preserved and no remediation
record. A fresh coordinator run recorded two scoped facts:

- `/tmp/sk-native-recovery-scoped.log`: `sk-status` **219 passed, 1 ignored**
  (subprocess helper), 20 suites — no failure in the native library.
- `/tmp/sk-cli-recovery-scoped.log`: `status_mutation` **27 passed, 6 FAILED**.

This report records the initial failure evidence, classifies every expectation
against the pinned Python product, and records the exact repairs and tests. The
final counts are updated in §6 before delivery.

---

## 1. Initial failing evidence (red)

Command (exact harness):

```sh
bash /tmp/sk-focused-cargo.sh test -p sk-cli --locked --test status_mutation
```

Result: **27 passed; 6 failed** (log `/tmp/sk-status-remediation-initial.log`).

| # | Test | Observed failure | Classification against pinned Python |
| - | ---- | ---------------- | ------------------------------------ |
| 1 | `status_emit_parses_inline_dependency_comments` | `seed WP01 -> done: Illegal transition: genesis -> done` | **Test-fixture defect.** Canonical `GenesisState.allowed_targets()` is `{planned, canceled}`, so an unforced `genesis -> done` is illegal in Python too (`validate_transition("genesis","done")` → `(False, 'Illegal transition: genesis -> done')`). The fixture bypassed the real lifecycle. |
| 2 | `move_task_refuses_a_rejection_without_a_rationale` | native returned `❌ Moving WP01 to 'planned' requires review feedback. … This requirement cannot be bypassed with --force.` | **Stale assertion.** The pinned `_guard_planned_rollback` requires a substantive `--review-feedback-file` on every review-rejection edge; the native message is byte-for-byte the canonical `_planned_rollback_message` Arm A. The old assertion expected the pre-remediation native wording (“requires a rationale”). |
| 3 | `move_task_refuses_a_review_ward_move_without_an_evidence_window` | `error_code` was `Null` instead of `EVIDENCE_WINDOW_UNCOMPUTABLE` | **Test-fixture defect masking the gate.** The hand-written `lanes.json` omitted `write_scope`, so the native lane reader refused the manifest as corrupt before the evidence gate ran. Python's `ExecutionLane.from_dict` defaults `write_scope`; a canonical writer always emits it. |
| 4 | `status_emit_enforces_the_for_review_commit_gate` | `corrupt lanes.json … missing field \`write_scope\`` | Same fixture defect as #3: the gate never saw a manifest. |
| 5 | `status_emit_accepts_a_planning_lane_with_a_qualifying_commit_since_claim` | same `missing field \`write_scope\`` | Same fixture defect as #3. |
| 6 | `move_task_records_the_reviewer_self_approval_lifecycle_event` | `rows.last()["to_lane"]` was `Null`, expected `"approved"` | **Stale ordering assertion.** The pinned executor writes the lane transition, then appends the `ReviewerSelfApproval` lifecycle fact (`tasks_move_task_executor._mt_execute`). The last row is the lifecycle event, which canonically has no lane field. |

No failure was caused by a missing native guard: N1–N12 were already
implemented in the preserved edits (see §3). The red run was six test/fixture
defects, not six product gaps.

---

## 2. Exact repairs

All paths are relative to the repository root.

### 2.1 Canonical lane fixture (`crates/sk-cli/tests/status_mutation.rs`)

- `Project` now carries its own `slug`; `seed()` uses it instead of the fixed
  `SLUG` constant, so a regression can use the numbered slug `057-lanehead`.
- `write_lanes()` delegates to a new `write_lanes_for(slug, lane_id, planning_commit)`
  that emits the **canonical** `to_dict` shape, including `write_scope` and
  `predicted_surfaces`. This unblocks the commit gate, the evidence window and
  the planning-lane case (#3–#5).
- `move_task_refuses_a_rejection_without_a_rationale` now asserts the canonical
  feedback requirement (message + “cannot be bypassed with --force”), the
  unchanged lane, and that the refusal writes no row (#2).
- `move_task_records_the_reviewer_self_approval_lifecycle_event` now finds the
  approved **transition** row (`from_lane: in_review`, `force: true`, operator
  note as `reason`), asserts the lifecycle row is appended last, and pins the
  exact seven-key canonical `ReviewerSelfApproval` payload (no lane field)
  (#6).
- `status_emit_parses_inline_dependency_comments` seeds `WP01` through the legal
  lifecycle `planned -> claimed -> in_progress -> approved -> done` (#1).
- `move_task_refuses_a_review_ward_move_without_an_evidence_window` now also
  requires the canonical code to lead the message, because the pinned non-owned
  `--json` envelope carries only the message and the code must stay readable
  there.
- `move_task_refuses_a_rejection_without_a_rationale` compares the row count
  before/after (the seeded `planned` row is legitimate; the refusal must add
  none), instead of asserting that no `planned` row exists at all.
- `status_emit_enforces_the_for_review_commit_gate` gives the forced bypass an
  explicit `--reason`: the pinned `check_force` requires an auditable
  actor + reason, so `--force` alone is refused by the writer after the gate is
  bypassed. This second fixture defect was masked by the corrupt-manifest
  failure and surfaced only once the gate saw a canonical manifest.

### 2.2 Status HEAD probe (`crates/sk-cli/src/compat/support.rs`)

`StatusHeadCommittedProbe::committed_event_ids` now calls
`read_blob_at_head_bounded(path, min(MAX_EVENT_LOG_BYTES, sk_git::MAX_BLOB_BYTES))`
instead of the 8 MiB `read_blob_at_head` default. Today that is the seam maximum
(16 MiB); if the Git owner raises the cap to the canonical 64 MiB, the same call
automatically requests the canonical bound. See §4 for the 64 MiB finding and
its residual.

### 2.3 Lint-only production corrections

Two preserved-edit lint defects were corrected without any behaviour change
(needed for the crate lints to stay green):

- `crates/sk-status/src/mutation_mirror.rs`: the `dependencies` match arm was
  written as a nested `if`; it is now a guarded arm (`collapsible_match`).
- `crates/sk-cli/src/compat/status.rs`: a needless `&base` borrow in the
  planning-lane claim-base check (`needless_borrow`).

`rustfmt` was applied to the owned `compat/{status,tasks}.rs`,
`compat/support.rs`, `tests/status_mutation.rs`,
`sk-status/src/mutation_mirror.rs` and
`sk-status/tests/mutation_rollback.rs`; the preserved edits had left the two
`compat` route files unformatted.

### 2.4 No production semantics were narrowed

No new refusal was added for valid upstream input. The only functional
production edit is the probe read bound (an increase). Every other change is
test/fixture text or an equivalent lint fix.

---

## 3. Review findings N1–N13 — disposition from the actual implementation

Legend: **Fixed** = implemented in the preserved edits and now covered by a
passing scoped test; **Accepted** = recorded divergence/INFO with no code change.

| Finding | Disposition | Where implemented / pinned |
| --- | --- | --- |
| **N1** reviewer independence (`REVIEW_NOT_INDEPENDENT` / `SINGLE_BRANCH_*`, placeholder `user`) | **Fixed** | `tasks::require_independent_approval` reads the implementer of record from the routed log, compares through `sk_status::actor_identities_independent`, and refuses a generic/placeholder actor. Tests: `move_task_refuses_an_approval_by_the_implementer_of_record`, `move_task_refuses_a_placeholder_approver`, `move_task_accepts_a_provably_independent_approver`. |
| **N2** `--self-review-fallback` option set + lifecycle fact | **Fixed** | Option-set validation before any write (`move_task_validates_the_self_review_fallback_option_set_before_writing`); a successful forced fallback appends the canonical `ReviewerSelfApproval` after the transition with `fallback_approved: true` and the seven-key payload (`move_task_records_the_reviewer_self_approval_lifecycle_event`). |
| **N3a** `agent status emit --to for_review` commit gate | **Fixed** | `status::enforce_for_review_commit_gate` (shared with `move-task`) refuses with `FOR_REVIEW_GATE` + `wp_id` + `lane_id`; `--force` bypasses. Test: `status_emit_enforces_the_for_review_commit_gate`; planning-lane positive: `status_emit_accepts_a_planning_lane_with_a_qualifying_commit_since_claim`. |
| **N3b** `move-task` evidence window | **Fixed** | `status::enforce_review_evidence_gate` runs before the commit gate and returns `EVIDENCE_WINDOW_UNCOMPUTABLE` / `IMPLEMENTATION_MISSING` / `IMPLEMENTATION_UNREADABLE` with the canonical recovery text. Test: `move_task_refuses_a_review_ward_move_without_an_evidence_window`. Positive-path coverage is shared with the planning-lane emit test (see residual R6). |
| **N4** `--to planned` requires review feedback (note never substitutes) | **Fixed** | `is_review_rejection_edge` + `planned_rollback_message` (canonical Arm A text, including “cannot be bypassed with --force”). Tests: `move_task_refuses_a_rejection_without_a_rationale`, `move_task_rejection_requires_a_substantive_feedback_file_not_a_note`. |
| **N5** feedback file only on edges that write a review cycle | **Fixed** | The feedback pointer is written to `review_ref` only on rejection/approved-reopen edges; a feedback file on e.g. `--to claimed` refuses before any write. Test: `move_task_refuses_a_feedback_file_on_an_edge_that_writes_no_review_cycle`. |
| **N6** lane-head stamp for numbered mission slugs | **Fixed** | `sk_workspace::naming::legacy_branch_body` strips `NNN-` only when the slug embeds a mid8; `code_lane_branch_name("057-lanehead", …)` → `kitty/mission-057-lanehead-lane-a`. New end-to-end regression: `status_emit_stamps_the_lane_head_for_a_numbered_mission_slug`. |
| **N7** canonical `reason` on every move-task row | **Fixed** | `tasks::move_task` writes the note, the done-override text, the forced default or `move-task: <old> -> <new>`; `#5446` keeps a bare forced review exit without a synthesized reason. Test: `move_task_writes_the_canonical_default_reason`. |
| **N8** approval/done `policy_metadata` sidecar | **Fixed** | `{tool, profile, model[, shell_pid]}` is written on every `approved`/`done` hop. Test: `move_task_records_approval_evidence_and_the_review_verdict` (asserts the exact sidecar). |
| **N9** `mark-status` case folding, `format`, `tasks.md` requirement, exit 1 when nothing resolves | **Fixed** | `tasks::mark_status` folds ids case-insensitively, reports `authored_roster`/`null`, refuses a missing `tasks.md`, and returns exit class 1 with the `{results, summary}` payload when nothing resolved. Tests: `mark_status_resolves_owners_case_insensitively_and_reports_the_format`, `mark_status_fails_with_exit_class_1_when_nothing_resolves`. |
| **N10** legacy mission with no `mission_id` | **Fixed** | The read-only legacy inventory/identity API keeps `mission_id: null` and omits the field from rows; every route accepts it. Tests: `legacy_missions_without_a_mission_id_keep_a_null_identity` and the new `legacy_missions_without_a_mission_id_move_tasks_without_inventing_one`. No ID is invented. |
| **N11** inline YAML comments in `dependencies` | **Fixed** | `mutation_readiness::frontmatter_dependencies` strips an inline `# …` outside quotes; the unit test pins `- WP01  # first` → `WP01`. CLI regression: `status_emit_parses_inline_dependency_comments`. |
| **N12** phase-1 mirror gating and filename case | **Fixed** | `mutation_mirror` requires a whole-model frontmatter read (`validate_wp_frontmatter`), matches `tasks/WPxx.md` case-sensitively, never creates `lane`, and preserves comments/quoting/CRLF. New regression: `status_emit_mirrors_a_whole_model_frontmatter_and_skips_an_off_model_one`. |
| **N13** `appended_event_ids` on a shrunken log (`None` vs `[]`) | **Accepted (INFO)** | The native sentinel stays `None`. `rollback_events_locked` refuses `LOG_SHRANK` before the ownership comparison, so no consumer can read it as “no appended rows”. New pin: `a_shrunken_log_wins_over_tail_ownership_and_appended_ids_is_unknown`. |

---

## 4. `HeadCommittedProbe` — consumer, bound and error evidence

- **Consumer.** The probe is real (`StatusHeadCommittedProbe`, reading the
  committed blob through `sk_git::GitRepository::read_blob_at_head*`, no shell,
  no path quoting) and its semantics match the baseline: an untracked path, an
  unborn `HEAD` and a non-repository answer empty; an unreadable `HEAD` or a
  malformed committed row is an error, which the rollback turns into
  `HEAD_UNREADABLE`. **It still has no production consumer** — only the two
  scoped rollback-probe tests (`the_status_rollback_head_probe_reads_the_committed_log`,
  `the_status_rollback_head_probe_retains_committed_parse_errors`) drive it. The
  verified rollback remains unwired (review D8), so every production caller
  would pass `head: None`. This is a residual, not a claimed fix.
- **Bound.** The pinned status-log bound is 64 MiB
  (`sk_status::mutation_log::MAX_EVENT_LOG_BYTES`), but the owned Git adapter
  caps one captured subprocess stream at 16 MiB
  (`sk_git::MAX_BLOB_BYTES`; `sk_git::process::MAX_OUTPUT_BYTES`). The probe
  previously used the 8 MiB default; it now requests
  `min(MAX_EVENT_LOG_BYTES, MAX_BLOB_BYTES)`, i.e. the seam maximum (16 MiB)
  today and the canonical bound automatically if the cap grows. **A committed
  log above the seam cap therefore fails closed as `HEAD_UNREADABLE` instead of
  being probed.** Reaching the canonical 64 MiB requires the incoming explicit
  `sk-git` read, which is outside this owner's boundary (`sk-git` is separately
  owned). Recorded as residual R2.
- **Committed error evidence.** A committed line that is not JSON returns
  `Err("the committed event log is not JSON: …")`; it is never dropped into an
  empty set. New regression:
  `the_status_rollback_head_probe_retains_committed_parse_errors`, which commits
  a malformed row and asserts the probe errors.
- **Corrupt Git is never absence.** The independent Git-operations review (F3)
  found that some corrupt `HEAD` shapes surface as `CommandFailed` /
  `NotARepository` rather than the documented `UnreadableHead`. The probe maps
  both to a hard error at open/read time; neither is `Absent` or `NoRevision`, so
  a rollback cannot read corruption as "no committed log". New regression:
  `the_status_rollback_head_probe_treats_corrupt_git_as_an_error_not_an_absence`
  (`ref: refs/heads/<empty>` → read error; empty `HEAD` → open error).
- **Incoming 64 MiB API.** The Git owner is adding an explicit supported 64 MiB
  committed-blob read while keeping the 8 MiB default. When that API is present,
  this adapter must request the canonical status bound
  (`MAX_EVENT_LOG_BYTES` / the new `MAX_STATUS_LOG_BYTES`) and a >16 MiB
  committed-log regression must be added. It is **not present at delivery**
  (`crates/sk-git/src/objects.rs` still exposes only
  `DEFAULT_MAX_BLOB_BYTES = 8 MiB` / `MAX_BLOB_BYTES = 16 MiB`), so the residual
  R2 stands. `sk-git` was not patched.

---

## 5. Environment incident during verification (resolved)

The first scoped `sk-cli` run after the edits failed to compile because
`crates/sk-install/src/{init,skills,error,session,template_tree}.rs` were being
edited concurrently by another owner (`E0609 no field warnings`, `E0599 missing
variant`, `E0308`, a mutable-borrow error). `sk-cli` depends on `sk-install`, so
the scoped CLI test could not build while that crate was mid-edit. I did **not**
touch, revert, stash or "fix" `sk-install`. A provisional test-binary run
against the last successful `sk-cli` rlib passed 38/38 while waiting (before the
corrupt-Git regression was added); once the other owner's build finished, the
official Cargo run below passed 39/39. No residual is carried from this
incident.

---

## 6. Verification (final counts)

All commands through `bash /tmp/sk-focused-cargo.sh`; no aggregate or release
build and no extra test target.

```sh
bash /tmp/sk-focused-cargo.sh test -p sk-status --locked
# 20 suites: 220 passed, 0 failed, 1 ignored (subprocess helper)
# log: /tmp/sk-status-remediation-final.log

bash /tmp/sk-focused-cargo.sh test -p sk-cli --locked --test status_mutation
# 39 passed, 0 failed, 0 ignored
# log: /tmp/sk-status-remediation-after-fixes.log

bash /tmp/sk-focused-cargo.sh clippy -p sk-status --all-targets --locked -- -D warnings
# clean

bash /tmp/sk-focused-cargo.sh clippy -p sk-cli --lib --locked
# 0 lints in compat/{status,tasks,support}.rs; remaining warnings are foreign:
# init.rs (1), runtime.rs (2) and the excluded dispatcher mod.rs (3)
```

- Initial red evidence: `27 passed, 6 failed`
  (`/tmp/sk-status-remediation-initial.log`).
- The `sk-status` count moved 219 → **220** with the new N13 pin; the single
  ignored case is the cross-process subprocess helper exercised by its parent.
- Doctests are part of `-p sk-status` (0 `sk_status` doctests, suite exits 0).
- `rustfmt --edition 2024 --check` is clean for
  `crates/sk-cli/src/compat/{status,tasks,support}.rs`,
  `crates/sk-cli/tests/status_mutation.rs`,
  `crates/sk-status/src/mutation_mirror.rs` and
  `crates/sk-status/tests/mutation_rollback.rs`.
- Oracle re-verified before the run:
  `{"ok": true, "python": "3.11.15", "installed_count": 53, "drift": [],
  "missing": [], "duplicate_versions": [], "unsatisfied": []}`.

---

## 7. Residual gaps (actual, not waived)

- **R1 Coordination routing.** `status emit`, `move-task`, `mark-status` and
  `materialize` still refuse a coordination-routed mission
  (`COORDINATION_SURFACE_UNAVAILABLE`); no coordination write surface.
- **R2 64 MiB committed-log probe.** The adapter can request at most 16 MiB
  (`sk-git` seam). A 16–64 MiB committed log fails closed as `HEAD_UNREADABLE`;
  closing it needs the incoming explicit 64 MiB `sk-git` read (see §4) and then
  a consumer/regression switch in this ownership. Not patched here.
- **R3 Verified rollback unwired.** The protocol, refusals and probe are
  implemented and tested, but no production route consumes them (D8).
- **R4 `lanes.json` reader strictness.** The native `sk_workspace::LanesManifest`
  reader requires `write_scope` (and `predicted_surfaces`), while Python's
  `ExecutionLane.from_dict` defaults them. A canonical writer always emits
  both, so the repaired tests use the canonical shape; a hand-written manifest
  missing them is still refused by the native reader. The strictness lives in
  `sk-workspace`, not in this owner's files.
- **R5 Additive error envelope.** Native `move-task` coded refusals carry
  `error_code` in JSON; the pinned non-owned envelope is
  `{"error": "<CODE>: …"}` only. The canonical code still leads the message, so
  a consumer keyed on the message remains compatible; the extra key is an
  additive native field, not a claimed byte-for-byte envelope.
- **R6 Positive `move-task` evidence-window path.** The negative gate is pinned;
  the positive path (owned change inside a real planning window) is covered by
  the shared planning-lane emit regression, not by a separate `move-task`
  fixture.
- **R7 Mirror validator strictness.** `validate_wp_frontmatter` treats a
  column-0 block list (`- T001` under `subtasks:`) as off-model and skips the
  mirror; Python's YAML read accepts it. It fails safe (no rewrite) and is
  pre-existing in the preserved edits, not introduced here.
- **R8 Declared omissions unchanged** from `CONTINUATION-REPORT.md` §4:
  auto-commit / protected-branch policy / review-cycle artifacts / verdict
  durability (D2), broader precondition gates (D3), `--to done` merge ancestry
  (D4), execution projection (D5), owned-checkout lifecycle (D6),
  `shell_pid_created_at` (D7), fan-out/transport, decision ledger,
  retrospective, `status migrate-envelope` CLI wiring, and coordination
  materialization.
- **R9 Capability matrix.** `compat/capabilities.rs` is excluded from this
  ownership; it still marks `agent status emit`/`move-task` `partial` without
  listing the now-implemented `for_review`/evidence gates. Updating the matrix
  remains a coordinator action.
- **R10 Aggregate gates.** No whole-workspace test/clippy/release/format or
  benchmark run was performed (explicitly out of scope). The scoped `sk-cli`
  lib lint still reports six foreign warnings (`compat/init.rs`,
  `compat/runtime.rs`, and the excluded `compat/mod.rs`), so a
  `-D warnings` lib gate for `sk-cli` cannot be green from this ownership; the
  status-owned files are clean.
