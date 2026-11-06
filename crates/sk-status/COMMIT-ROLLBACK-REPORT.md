# sk-status commit/rollback tranche — implementation report (FINAL for this tranche)

Status: **implemented and exercised in the owned scope; NOT an independent
approval and NOT a completion claim.** Coordination materialisation and the
owned-checkout lifecycle remain refused; nothing here claims those workflows.

- Repository: `spec-kitty-rust`, branch `rust/rewrite`, baseline HEAD
  `d64468a7693e4962712cbf3416d767440d22a649` plus every owner's preserved
  uncommitted edits (status, CLI, Git, init, next, workspace, install).
- Reference: `spec-kitty/spec-kitty` `9aed3a8012cb5eb22c0128b14d525039f95fea7a`,
  pinned oracle `.venv-rust-oracle` (Python 3.11.15). Python is development
  tooling only; no native route invokes it.
- Toolchain: rustc 1.99.1; every Cargo invocation through
  `bash /tmp/sk-focused-cargo.sh` (temporary HOME/XDG/TMP, retained build cache,
  `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, shared `flock`, `--locked`).
- No commit, push, manifest/lock edit, aggregate/release build, timing run,
  subagent or Python production change was made by this tranche.
- Git owner `ac2df51d` is complete: `sk_git::MAX_BLOB_BYTES` is the canonical
  64 MiB explicit bound with the capture limit plumbed through the subprocess
  seam; the default stays 8 MiB. `GitError::MergeConclusionUnconfirmed` is boxed
  and means *possibly landed → retain*; safe-commit landed variants are unchanged.

## 1. Owned files

| Path | Change |
| --- | --- |
| `crates/sk-status/src/mutation_commit.rs` | **new** config resolution, canonical review-cycle artifact model/writer, durability value, canonical commit messages |
| `crates/sk-status/src/mutation_mirror.rs` | R7: real `serde_yaml_ng` parse + `WPMetadata` rules; quoted-key/anchor/CRLF-safe byte-local lane edit; oracle-valid block-list/alias tests |
| `crates/sk-status/src/lib.rs`, `src/mutation.rs` | module registration/re-exports only |
| `crates/sk-cli/src/compat/tasks/commit.rs` | **new** child module: auto-commit authority, protection preflight, safe-commit attempt and landed/indeterminate/not-landed classification |
| `crates/sk-cli/src/compat/tasks.rs` | `mod commit;`; tri-state `--auto-commit/--no-auto-commit`; real primary/non-owned commit + review-cycle + verified-rollback lifecycle for `move-task` and `mark-status`; canonical pointer refs; flag-parser tests |
| `crates/sk-cli/src/compat/support.rs` | R2: obsolete 16 MiB probe comment replaced with the landed 64 MiB contract (behaviour already requested the canonical bound) |
| `crates/sk-cli/src/compat/status.rs` | no semantic change; rustfmt only |
| `crates/sk-cli/tests/status_mutation.rs` | 9 new acceptance tests (48 total) and three stale CLI expectations updated to the canonical pointer semantics (plus the mirror `doing` alias expectation in `sk-status`) |
| `crates/sk-status/COMMIT-ROLLBACK-REPORT.md` | this report |

Not edited: `mutation_lock.rs` / frozen Mission-lock tests, `compat/mod.rs`,
`compat/capabilities.rs`, `compat.rs`, manifests/lockfile, dispatcher,
installer, runtime, `sk-git`, `sk-workspace`, `sk-mission`, every other existing
test, Python sources.

Registration needs: **none** — `crates/sk-cli/src/compat/tasks/commit.rs` is a
private child of the owned `tasks.rs` (`mod commit;`), and
`mutation_commit` is a normal `sk-status` module. No dispatcher/capability
manifest change is required.

## 2. Canonical sources traced

- `specify_cli/core/agent_config.py::load_agent_config` /
  `get_auto_commit_default` — `agents` then `tools`, then the top level,
  booleans only, missing file → `True`.
- `specify_cli/cli/commands/agent/tasks_move_task.py`
  (`_mt_resolve_targets`, `_mt_resolve_feedback`, `_mt_gather_review_facts`,
  `_mt_finalize_plan`, `_mt_execute`, `_mt_output`,
  `_mt_commit_lane_deliverables`) and
  `tasks_move_task_executor.py` — auto-commit resolution, protected-branch
  refusal before the event-log read, review evidence → emit → release order.
- `tasks_verdict_persistence.py` (`VerdictDurabilitySignal`,
  `_persist_approved_review_cycle`, `persist_rejected_review_cycle_for_rollback`,
  `_persist_review_cycle_with_queue`, `revert_committed_verdict_write`) —
  `local_only`/`durable`/`persistence_failed`, `no_auto_commit`, the approval
  no-op probe and the exact JSON keys.
- `review/cycle.py` (`create_rejected_review_cycle`,
  `_review_cycle_write_dir`, `_commit_review_cycle_artifact`,
  `VerdictPersistenceOutcome`, pointer validation) and
  `review/artifacts.py` (`ReviewCycleArtifact.to_dict/write`) — canonical
  `kitty-specs/<slug>/tasks/<wp_slug>/review-cycle-N.md`, exclusive create,
  `review-cycle://` pointer, exact committed-byte readback.
- `cli/commands/agent/tasks_mark_status.py` — event-only subtask writes and the
  shared transactional commit leg.
- `tasks_shared.py::_protected_branch_status_commit_error` /
  `_skip_target_branch_commit` and `coordination/commit_router.py::commit_for_mission`
  — the protected-primary refusal wording/remedy and the safe-commit
  placement/policy facts.
- `status/mission_write.py` verified rollback (`capture_rollback_point`,
  `appended_event_ids`, `rollback_events_log`, `rollback_status_artifacts`) —
  already implemented natively in `mutation_rollback.rs`; now wired.

## 3. What the native lifecycle now does

### 3.1 Auto-commit decision (`mutation_commit::read_auto_commit_config`)

`bool` flag resolution is **tri-state** exactly like upstream's `bool | None`:

| Input | Decision |
| --- | --- |
| `--auto-commit` (last wins) | `true` |
| `--no-auto-commit` (last wins) | `false` |
| neither | project config `agents.auto_commit` / `tools.auto_commit` / top-level `auto_commit`; missing config → `true` |

The YAML file is parsed with `serde_yaml_ng` (never reserialised). A non-mapping
document/section or invalid YAML is a typed refusal; a non-boolean value falls
back to `true` like the reference.

### 3.2 Protected destination, before any mutation

`commit::prepare` resolves `sk_workspace::ProtectionPolicy` for the mission
(config list, remote default, operator hatch, `commit_to_target`) and refuses:

```
Refusing to run `spec-kitty agent tasks move-task` with auto-commit on protected
branch '<target>' before mutating status files. Run status commit operations
from an allowed coordination/lane branch, or unblock with:
--start-branch <topic-branch> or SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS=1.
```

It then requires a readable checkout on the mission target branch
(`AUTO_COMMIT_REQUIRES_GIT` / `AUTO_COMMIT_TARGET_CHECKOUT_REQUIRED`), so no
status row is written when a commit could not be represented. `mark-status`
uses the same preflight.

### 3.3 Review-cycle evidence (D2)

- **Rejection family** (`--to planned`, `in_review -> in_progress`): the
  reviewer's feedback file is written verbatim as the artifact body; the
  cycle number is `max(parsed)+1` over the consulted `tasks/<wp_slug>` dirs; the
  on-wire `review_ref`, `review_result.reference` and `review_feedback` all use
  the canonical `review-cycle://<slug>/<wp>/review-cycle-N.md` pointer. The
  raw draft path is no longer on-wire review authority, and the user's draft is
  **never deleted** (pre-existing file preservation).
- **Approval/done**: a canonical approval artifact is written (`Approved by
  <reviewer>: <reference>`, reproduction command carrying the reviewer) unless
  the current event-sourced verdict is already `approved` (the pinned no-op
  probe). The approval evidence/review reference becomes the pointer.
- Artifacts are exclusive-create (`ReviewCycleError::Existing` on collision),
  validated against the pinned frontmatter shape, and byte-rendered with the
  oracle-recorded layout (`mutation_commit` module tests pin the exact bytes for
  the rejection and approval shapes; the R7 test fixture was re-validated
  through the pinned `WPMetadata` model).

### 3.4 Commit + durability (D2/R3)

When auto-commit is on:

1. acquire the canonical `MissionLock` (`LockWait::Unbounded`) and capture a
   `RollbackPoint` for `status.events.jsonl` + `status.json`;
2. write the review-cycle artifact (if any) under that hold;
3. emit the transition (writer re-enters the same per-thread hold);
4. issue **one** path-scoped `safe_commit` (existing `sk_git` seam,
   `CommitGuardCapability::Standard`, caller-resolved protected set) containing
   the status artifacts for a stored topology (`lanes`/`single_branch`) and the
   review-cycle artifact;
5. durable `verdict_durably_persisted` is `true` **only** when the commit landed
   and `git show <target>:<evidence_ref>` equals the local artifact bytes
   (`destination_readback_mismatch` otherwise, files retained).

`--no-auto-commit` writes the same local bytes, commits nothing, and reports the
pinned `local_only` / `no_auto_commit` outcome. No commit is fabricated and no
verdict is reported durable from a local write.

Canonical commit messages: `chore: Record review-cycle-N (<verdict>) for WP on
<slug>` when evidence is included, else `chore(spec-kitty): status transition
<slug>`.

### 3.5 Verified rollback and landed/indeterminate rule

`commit::classify_apply_error`:

| Safe-commit outcome | Class | Behaviour |
| --- | --- | --- |
| `SafeCommitLanded` (post-landing index repair failed) | `Landed` | keep; durability decided by readback |
| `SafeCommitAdvanceUnconfirmed`, timeout/spawn/I/O/command/HEAD failures, `MergeConclusionUnconfirmed`, unknown variants | `Indeterminate` | **never** truncate/rollback/delete; command reports `COMMIT_INDETERMINATE` and leaves files for inspection |
| empty changeset, HEAD mismatch, path policy, protected branch, stage/commit/hook rejection, plan stale, in-progress operation, … | `NotLanded` | verified rollback allowed |

On `NotLanded`, the route calls
`sk_status::mutation::rollback_status_artifacts(&lock, &point, Some(&event_ids),
Some(&StatusHeadCommittedProbe))` where `event_ids` is exactly the transition
plus its in-replacement annotation (or every `mark-status` annotation). The
protocol re-checks log existence/shrink, whole-row tail, multiset ownership and
committed-at-`HEAD` before cutting only this operation's tail; `status.json` is
restored only after the log half succeeds. The evidence artifact this call wrote
is removed only after a proven not-landed outcome. User feedback files and
pre-existing artifacts are untouched.

### 3.6 `mark-status`

- auto-commit resolves from config/flags with the same protected/HEAD preflight;
- on a stored topology the annotation rows + snapshot/a snapshot commit under
  the same lock/rollback point, with the same not-landed rollback and
  indeterminate retention; flat/legacy missions keep the reference's
  primary-uncommitted annotation write;
- no commit is attempted when nothing changed: an all-`already_satisfied` mark
  (and an alias-collapse/self-transition no-op in `move-task`) creates no commit
  and reports success, instead of turning an unchanged tree into a commit
  failure.

## 4. R7 — mirror YAML fidelity (closed for the tested contract)

`mutation_mirror::validate_wp_frontmatter` now:

- extracts the frontmatter block and parses it with **`serde_yaml_ng`**, so
  block sequences, aliases/anchors, flow collections and quoted keys are read
  as the reference YAML reader reads them;
- validates the source `WPMetadata` rules: `extra="forbid"` top-level keys,
  `work_package_id` `^WP\d{2,}$`, non-blank `title`, `doing`→`in_progress` lane
  alias with non-display sentinels refused, `shell_pid` int coercion,
  `base_commit` 7–40 hex (or `unknown`), legacy scalar/list coercions for
  `dependencies`/`requirement_refs`/`tracker_refs`/`owned_files`/`tags`/
  `create_intent`, `subtasks`/`history` lists and string-only `phases`;
- still performs a **byte-local edit only**: comments, quotes, CRLF, anchors,
  a quoted `lane` key and the body are preserved; no user YAML is reserialised;
- accepts the legacy compact `subtasks:[]` authoring shape by normalising only
  the *parse* input (documented widening; the pinned reader itself rejects a
  missing space, but the pre-existing status fixture corpus authors it);
- keeps the fail-safe skip for a genuinely off-model document (unknown key,
  bad identity, invalid lane).

Tests: `block_sequences_aliases_and_quoted_keys_are_valid_model_input` (the
exact fixture was validated through the pinned `WPMetadata`:
`WP01 Demo ['T001','T002'] ['WP00'] planned`),
`a_quoted_lane_key_an_anchor_and_crlf_are_preserved`, the existing off-model
skip test (its stale `lane: doing` case now correctly mirrors because `doing` is
a valid alias), and the existing preservation/CRLF/case tests.

## 5. R2 / R6

- **R2**: the probe requests
  `min(MAX_EVENT_LOG_BYTES, MAX_BLOB_BYTES)` = the canonical 64 MiB now that the
  Git owner landed the explicit bound; the obsolete 16 MiB comment is replaced.
  New regression `the_status_rollback_head_probe_reads_a_committed_log_beyond_the_old_16_mib_cap`
  commits a **>16 MiB** JSONL log and reads every row id. The existing corrupt-Git
  regressions remain: garbage/empty `HEAD` and an empty symbolic target are hard
  errors (never `Absent`/empty set).
- **R6**: new `move_task_accepts_a_real_evidence_window_for_owned_files` builds a
  real single-branch Git fixture (planning commit, committed `src/` change,
  canonical `lanes.json`) and exercises the positive `move-task --to for_review`
  evidence-window path, not only the shared emit regression.

## 6. Evidence

### 6.1 Oracle

```
PYTHONPATH=$PWD/src SPEC_KITTY_PACKS_ROOT=$PWD/packs \
  .venv-rust-oracle/bin/python scripts/rust/verify_oracle.py --root .
{"checked_runtime_requirements": 53, "drift": [], "duplicate_versions": [],
 "installed_count": 53, "missing": [], "ok": true, "python": "3.11.15",
 "unsatisfied": []}
```

### 6.2 Tests / lints (all through the focused helper)

| Command | Result | Log |
| --- | --- | --- |
| `test -p sk-status --locked` | **231 passed, 0 failed, 1 ignored** (20 suites; the ignored case is the parent-exercised subprocess helper) | `/tmp/sk-status-commit-rollback.log` |
| `test -p sk-cli --locked --test status_mutation` | **48 passed, 0 failed** | `/tmp/sk-status-commit-cli.log` |
| `test -p sk-cli --locked --lib` | **7 passed, 0 failed** (flag tri-state, commit classifier, support units) | `/tmp/sk-status-commit-lib.log` |
| `test -p sk-cli --locked --test compat` | **26 passed, 0 failed** (actual binary dispatch incl. `move-task`/`mark-status`) | `/tmp/sk-status-commit-compat.log` |
| `clippy -p sk-status --all-targets --locked -- -D warnings` | clean | scoped run |
| `clippy -p sk-cli --lib/--test status_mutation --no-deps -- -D warnings` | clean | scoped run |
| `rustfmt --edition 2024 --check <owned files>` | clean | scoped run |

`sk-cli --all-targets` Clippy still stops on a **foreign** lint in
`crates/sk-cli/tests/init_oracle_diff.rs` (`needless_borrows_for_generic_args`);
the file is outside this ownership and untouched.

### 6.3 Red / green observed during this tranche

- The pre-change route never wrote review-cycle evidence, never committed, and
  always reported `verdict_durably_persisted:false`. Adopting canonical pointers
  made the three stale expectations fail exactly on the intended semantic change
  (raw feedback path / `PR#42` vs `review-cycle://…`); they were updated to the
  canonical pointer + artifact assertions.
- The R7 move to a real YAML parse turned the existing
  `the_phase1_lane_mirror_updates_the_authored_field_only` fixture (an off-model
  `subtasks:[]` line) from accepted to skipped; the documented read-only
  normalisation restored the intended mirror while keeping genuine off-model
  documents skipped, and the previously stale `lane: doing` expectation now
  mirrors (it is a valid alias).
- New acceptance tests were added with their implementation; the exact
  red/green logs above are the final green evidence, not a claim of a
  pre-recorded red log for every new path.

### 6.4 Hashes of the owned sources

```
5a60921610b94bdcf57964df01fb73f2a82a465b631358f26c21a5f6646ee39e  crates/sk-status/src/mutation_commit.rs
fbc377feada4e3caf1af3937a676cead9f19a4b49b82755920d41a65f19649fc  crates/sk-status/src/mutation_mirror.rs
fb1303276c69ad53d1453a38e99fbc712118bc6dd90126cfb1c0d60f9a5d6513  crates/sk-status/src/lib.rs
59130f0d2b8ae587ce0b84d9729c1688d49a86adeeeef47bd5695c27107cbe80  crates/sk-status/src/mutation.rs
da187a275472c0b1940f3aea6408f1c1a4a52c7568dc5f1856077e6d3b51ac50  crates/sk-cli/src/compat/tasks.rs
71942731c5c2f5e90e95d42da91c02acc9ec086ef33d80f0d9e2b3d41cbaf36b  crates/sk-cli/src/compat/tasks/commit.rs
b662a75543bba85a8dac9753d9fa7b0afdf6496b1f2808f497c0de4f61a87c7e  crates/sk-cli/src/compat/support.rs
fbc7bb113d7956c0a07a56583895e0f4dc6905f49c56c1fd7655d868c4cc07c2  crates/sk-cli/src/compat/status.rs
12995352ea8446957dc3d7aae016540c808998b6dcd39b635caf4d5c300214b8  crates/sk-cli/tests/status_mutation.rs
```

(Hashes are of the working-tree sources after the final rustfmt/clippy round.)

## 7. Residuals (actual, not waived)

1. **Coordination routing** remains refused (`COORDINATION_SURFACE_UNAVAILABLE`);
   no coordination write surface is implemented or claimed.
2. **Owned checkout** (`--owned-checkout`) remains refused; the owned
   single-branch lifecycle is not implemented.
3. **`--to done` merge ancestry** still requires `--done-override-reason`; the
   Git merge-ancestry gate is not ported.
4. **Auto-commit needs a readable target checkout.** Even for a flat topology
   (where the status artifacts stay uncommitted) the preflight requires the
   target branch checked out, because the evidence commit still targets it; the
   reference's target-branch resolution also requires Git. `--no-auto-commit`
   keeps the local-only path.
5. **Flat/legacy status commit parity.** The reference leaves a flat mission's
   status artifacts uncommitted; this native path also leaves them uncommitted
   (only evidence commits), while stored `lanes`/`single_branch` topologies
   commit status + evidence together. The single-commit composition (status +
   artifact) is a native lifecycle choice, recorded here rather than claimed as
   byte-for-byte upstream commit sequencing.
6. **Review-cycle byte formatting.** The artifact is valid for the pinned reader
   and pinned by oracle-recorded bytes for the rejection/approval shapes; an
   exotic frontmatter scalar may render with different (valid) quoting than
   ruamel. The artifact body is the caller's bytes.
7. **Legacy compact-flow parse widening.** `subtasks:[]` (no space) is accepted
   only for the *read*; the pinned reader rejects that spelling. The authored
   bytes are never rewritten.
8. **R5 proposal (not applied — `compat/mod.rs` is excluded).** Native coded
   refusals carry an additive JSON `error_code` key; the pinned non-owned
   envelope is `{"error": "<CODE>: …"}` only. Precise change for the dispatcher
   owner: for non-owned `move-task`/`mark-status` envelopes, either (a) omit the
   `error_code` key while keeping the code as the message prefix, or (b) declare
   the additive key in the capability contract. The code already leads the
   message, so no message change is needed.
9. **Capability matrix** (`compat/capabilities.rs`) is outside this ownership;
   the four route entries still need the now-real commit/durability gaps
   collapsed and the remaining gaps (coordination, owned checkout, done
   ancestry, projection, fan-out) listed.
10. **No aggregate/release/format-at-workspace/performance run.** `sk-cli`
   `--all-targets` Clippy still trips the foreign `init_oracle_diff.rs` lint;
   the status-owned lib/test targets are clean. Independent review is still
   required before any promotion.

## 8. Non-negotiables observed

No guessed or fake commit; durability `true` only for a landed commit with exact
byte readback; no fabricated verdict durability; no bypass of the protection
policy; no Python production path; no edits to the frozen Mission-lock
mechanics, dispatcher, capabilities, manifests, or other owners' crates/tests;
the canonical Mission lock and `StatusHeadCommittedProbe` are the real
authorities used across capture/write/commit/rollback.
