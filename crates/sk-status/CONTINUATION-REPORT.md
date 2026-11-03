# sk-status continuation report — canonical mutation semantics + compatibility facade

Status: **implemented and exercised; not a full-parity verdict.**

- Baseline: `spec-kitty/spec-kitty` `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
- Oracle: `.venv-rust-oracle/bin/python` (3.11, `spec-kitty-events==11.3.0` from
  `uv.lock`), verified with `scripts/rust/verify_oracle.py --root .`
  (`{"ok": true, "installed_count": 53, "drift": [], "missing": []}`).
- Toolchain: rustc 1.99.1; `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`; existing
  `target/`; no commit, push, manifest, dependency, release/aggregate build or
  timing work was performed.
- The coordinator's Mission-lock disposition (`docs/rust/reviews/mission-lock-disposition.md`,
  review `/tmp/sk-rust-mission-lock-review.md`) is integrated here: F5 is fixed
  and the "not waived" verified-rollback obligation is implemented in
  `mutation_rollback.rs`. Lock files (`mutation_lock.rs`,
  `tests/mission_write_lock.rs`, `tests/mutation_lock.rs`) were **not edited**.
- This report **supersedes** the gap lists in `WRITER-README.md` /
  `WRITER-REPORT.md` where they disagree (both now carry a pointer).

Owned surfaces touched:

| Path | Change |
| --- | --- |
| `crates/sk-status/src/mutation_delta.rs` | **new** typed `WPInnerStateDelta` + Python coercions |
| `crates/sk-status/src/mutation_payloads.rs` | **new** pinned lifecycle payload models/validation |
| `crates/sk-status/src/mutation_readiness.rs` | **new** dependency-readiness verdict + declared-dependency reader |
| `crates/sk-status/src/mutation_mirror.rs` | **new** phase-1 frontmatter `lane` mirror (byte-local edit) |
| `crates/sk-status/src/mutation_rollback.rs` | **new** verified Mission-write rollback protocol (`capture_rollback_point` / `rollback_events_log` / `rollback_status_artifacts`, injected committed-at-`HEAD` probe) |
| `crates/sk-status/src/mutation_emit.rs` | typed annotation, transition+annotation in one replacement, lane-head stamp, in-lock readiness, mirror hook, canonical WP-id rule, **unbounded flat wait policy** |
| `crates/sk-status/src/mutation_lifecycle.rs` | payload validation/canonicalisation at the persistence seam, **unbounded wait policy** |
| `crates/sk-status/src/mutation_migrate.rs` | size check before snapshot (L3), zeroed refusal manifest (L7/baseline MIG4), **unbounded wait policy** |
| `crates/sk-status/src/snapshot.rs` | `project_cancellation_provenance` widened to `pub(crate)` for the readiness verdict (no reader behaviour change) |
| `crates/sk-status/src/mutation_json.rs` | `python_number` visibility only (no encoding change) |
| `crates/sk-status/src/mutation_log.rs` | doc clarification: `restore_log_snapshot` is the byte-snapshot primitive, **not** the verified rollback protocol |
| `crates/sk-status/tests/mutation_rollback.rs` | **new** 10 verified-rollback tests |
| `crates/sk-status/src/lib.rs`, `mutation.rs` | module declarations/re-exports; no reader logic change |
| `crates/sk-status/tests/mutation_contracts.rs` | **new** 9 writer-contract tests |
| `crates/sk-status/tests/mutation_oracle_contracts.rs` | **new** 5 oracle-vector tests |
| `crates/sk-status/tests/fixtures/mutation/contract_vectors.json` | **new** oracle-generated contract vectors |
| `crates/sk-status/tests/fixtures/mutation/generate_contract_vectors.py` | **new** generator (pinned oracle, dev-only) |
| `crates/sk-status/tests/mutation_migrate.rs` | one assertion updated to the baseline MIG4 zeroed manifest |
| `crates/sk-cli/src/compat/status.rs` | `EmitArgs` + `emit()` + `lifecycle()`/`lifecycle_at()` + shared actor/WP-id/probe helpers |
| `crates/sk-cli/src/compat/tasks.rs` | `MoveTaskArgs` + `move_task()`; `MarkStatusArgs` + `mark_status()` |
| `crates/sk-cli/tests/status_mutation.rs` | **new** 16 end-to-end mutation-route tests incl. a full oracle derivation comparison |

Deliberately **not** touched: `mutation_lock.rs`, `tests/mission_write_lock.rs`,
`tests/mutation_lock.rs`, all `sk-mission` sources, `compat/mod.rs`, every other
existing `compat/*.rs`, workspace manifests/lockfile, Python sources.

---

## 1. What is now real (semantics covered)

### 1.1 Typed inner-state deltas (`mutation_delta.rs`)

`InnerStateDelta` is the native `models.WPInnerStateDelta`, validated and
canonicalised at the write boundary:

- every documented slot: `shell_pid`, `shell_pid_created_at`, `subtasks`,
  `note`, `tracker_refs`, `tracker_refs_replace`, `agent`, `assignee`,
  `review` (the four-key `ReviewOverride`), the resolved-binding group
  (`role`, `agent_profile`, `agent_profile_version`, `model`, `provider`) and
  `release_runtime_claim`;
- `""` → absent for the eight `_SCALAR_FIELDS` only (so a blank note or
  `assignee: "  "` still persists, exactly as the baseline);
- Python `int()`/`bool()`/`str()` coercions (`shell_pid: "4242"`, `4242.9`,
  `true`; `release_runtime_claim: 0` false, `"false"` true; review fields
  `str`-coerced);
- subtask values: a canonical lane, or `skipped`; the input alias
  `not_applicable` folds to `skipped` and is never persisted;
- `is_empty()` = all slots absent and `release_runtime_claim == false`;
- presence-only `to_json_value()` (absent leaves the reducer slot untouched);
- `ReviewOverride::complete` and the narrow all-empty `is_release_sentinel`.

**Fail-closed strengthening (recorded, not baseline):** an unknown delta key or
a non-string value in a `str | None` slot is refused. The baseline frozen
dataclass performs no runtime type check, so `{"agent": 5}` could be written
verbatim; the native writer refuses it instead of persisting an off-contract
row.

### 1.2 Lifecycle payload models (`mutation_payloads.rs`)

`canonicalize_lifecycle_payload(event_type, payload)` validates against the
pinned 11.3.0 models and re-encodes the model's canonical wire shape:

- `ProjectInitialized`, `Specify/Plan/Tasks Started|Completed`, `WPCreated`,
  `ReviewerSelfApproval` — `spec_kitty_events.project_lifecycle` (required
  fields, `min_length=1`, `ge` bounds, `extra="forbid"`);
- `MissionCreated` — `spec_kitty_events.lifecycle.MissionCreatedPayload` via the
  core builder's shape, including `apply_keep_none_fields` (`mission_number`
  survives as explicit `null`; other absent optionals are dropped);
- `MissionReopened` / `FollowUpRecorded` — the core-local shapes, including the
  `commit`/`pr` cross-field requirement;
- datetime normalisation for pydantic `datetime` slots (`at`,
  `initialized_at`, `created_at` on `WPCreated`) to the `model_dump(mode="json")`
  form (`...Z`, six microsecond digits when non-zero); pydantic's numeric-epoch
  extension is accepted; `MissionCreated.created_at` and
  `ReviewerSelfApproval.timestamp` are model **strings** and stay verbatim;
- the one documented local-log extra: `artifact_path` on *Started* events
  (`emit_artifact_phase_local` appends it after the canonical model validated).

`persist_lifecycle_event_local` now validates **every** known event type before
building the envelope. An unknown type is still the baseline's no-op skip. The
caller-facing API (`LifecycleRequest`, `persist_lifecycle_event_local`,
`append_lifecycle_event`, `emit_project_initialized`, `ProjectInitializedRequest`)
is unchanged, which is what `sk-install::lifecycle` consumes.

### 1.3 Canonical emit behaviour (`mutation_emit.rs`)

- **Transition + claim annotation are one durability unit.** `TransitionRequest`
  gained `annotation_delta: Option<InnerStateDelta>`; `prepare_transition` mints
  the typed `AnnotationDraft` and the shell appends transition + annotation rows
  in the **same** verified whole-file replacement (baseline FR-004). Batch
  members do the same.
- **Lane-head stamp.** `TransitionRequest.lane_head` merges into
  `policy_metadata["lane_head"]` (`LANE_HEAD_KEY`) without touching any other
  key, and only when a stamp exists (baseline probe semantics: absent = no
  stamp). The Git probe is supplied by the CLI adapter (sk-status never runs
  Git).
- **Dependency readiness is resolved in-lock, never failed open.**
  `dependency_ready` (explicit override) > `dependency_verdict` > a verdict
  computed from the reduced write surface and the WP's authored
  `dependencies`. Acceptable endings are `approved`, `done`, and `canceled`
  **with operator provenance** (`reason_source == "operator"`); ambiguous or
  unreadable declarations produce an unsatisfied verdict (so only the guarded
  entry edges refuse and `--force` still wins).
- **Phase-1 lane mirror.** `mirror_phase1_frontmatter_lane` runs when
  `meta.json.status_phase >= 1` (numeric only; `1.0`/`"1.0"` are off, matching
  `int(str(value))`), updates only an already-present top-level `lane:`, and is
  a byte-local edit — comments, ordering, quoting and the rest of the file are
  preserved (no YAML reserialisation).
- **Canonical WP-id rule.** `emit_inner_state_changed` matches `^WP\d+$` against
  the caller's string and no longer upper-cases (`wp01` is refused, baseline
  `annotate()` parity).
- **Typed post-commit failures.** `MutationError::EventMissing` /
  `PostCommitVerification` (from the parent fix) are honoured by both shells via
  `after_commit`; the new rows go through the same readback path.

### 1.4 Migration

Only envelope bookkeeping is rewritten (unchanged from the parent work): payload
objects and the original `project_uuid` field strings are preserved byte-for-byte
(only the `build_id` uuid5 name uses the canonical UUID spelling). Two residuals
from the independent review were closed:

- the result-size bound is checked **before** the `.pre-migration.bak` snapshot,
  so a refused oversized migration no longer leaves a one-shot trap;
- the existing-snapshot refusal returns the baseline's **zeroed** manifest
  (`total_rows=0`, zero counts, empty rows) instead of the computed one.

### 1.4a Wait policy (mission-lock review F5)

The baseline's **flat** status doors take `feature_status_lock` /
`project_event_log_lock` with their `UNBOUNDED_LOCK_WAIT` default; only the
coordination transaction passes the 10 s bound. Native defaults now match:

| Door | Baseline default | Native default |
| --- | --- | --- |
| `emit_status_transition`, batch, `emit_inner_state_changed` | unbounded | `LockWait::Unbounded` |
| lifecycle appenders (mission + project logs) | unbounded | `LockWait::Unbounded` |
| envelope migration | unbounded | `LockWait::Unbounded` |
| `agent status materialize` | unbounded | `LockWait::Unbounded` |
| coordination transaction | 10 s | caller supplies `MISSION_WRITE_LOCK_TIMEOUT` explicitly |

Every door still honours an explicit `lock_wait`, so the coordination owner can
pass the bounded 10 s policy without a second code path. Regression tests pin
the defaults in `mutation_emit`, `mutation_lifecycle` and `mutation_migrate`.

### 1.4b Verified rollback protocol (`mutation_rollback.rs`)

The mission-lock review's required non-lock half is implemented. It is **not**
the byte-snapshot primitive (`mutation_log::restore_log_snapshot`, now
documented as such): a rollback is only allowed to cut rows that are provably
this operation's own and uncommitted.

- `capture_rollback_point(&MissionLock, feature_dir)` requires the registered
  hold: the `&MissionLock` is a type-level precondition, and the directory must
  be covered by that lock's root and resolve to its key (a same-named Mission in
  another checkout is refused). It captures the log size/existence and
  `status.json` bytes.
- `appended_event_ids(&point)` reads the tail and returns the ids of whole rows,
  or `None` for a torn/unparseable tail.
- `rollback_events_log(&lock, &point, expected_event_ids, head_probe)` refuses,
  leaving every byte identical, when: the log vanished after existing at
  capture (`LogVanished`), shrank (`LogShrank`), the tail is not whole JSON event
  rows (`TailUnparseable` — the indeterminate/torn-write case), the tail ids are
  not the caller's multiset (`TailNotOwned` — a foreign row), a tail id is
  committed at `HEAD` (`TailAlreadyCommitted`), or the committed log could not
  be read/proven (`HeadUnreadable`). It never extends the file, fsyncs a cut,
  and re-checks the resulting size. An I/O failure is the only
  "may be partly rewritten" outcome (`IoError`).
- `rollback_status_artifacts` restores the captured `status.json` bytes (or
  removes a file that did not exist) only when the log half succeeded.

The committed-at-`HEAD` probe is **injected** (`HeadCommittedProbe`): this crate
never runs Git. Passing `None` refuses a non-empty tail with `HeadUnreadable`
(the baseline's fail-closed behaviour), never a silent cut. A Git-adapter owner
supplies the probe; the exact missing adapter operation is a public
"read blob at `HEAD` for a repo-relative path" (see §4).

### 1.5 CLI mutation facade

`crates/sk-cli/src/compat/status.rs`

- `EmitArgs` mirrors the upstream `agent status emit` option surface
  (positional/`--wp`, `--to`, `--actor`, `--mission`, `--force`, `--reason`,
  `--evidence-json`, `--review-ref`, `--review-result-json`,
  `--workspace-context`, `--subtasks-complete`,
  `--implementation-evidence-present`, `--execution-mode`, `--json`).
- `emit()` reproduces the baseline order and refusals: seed-shaped `--actor`
  refused before I/O; selector agreement (`WPxx` normalisation); mission
  selection; **coordination-routed mission refused with
  `COORDINATION_SURFACE_UNAVAILABLE`** (never a silent primary write); evidence
  JSON error text; `--review-result-json` shape/verdict checks; structured
  4-key actor (compact `tool:model:profile`, role from the hop); lane-head probe
  from `lanes.json` + `sk_git::GitRepository::read_branch_tip`; upstream success
  payload `{event_id, wp_id, work_package_id, from_lane, to_lane,
  status_events_path, actor}` and the human line.
- `lifecycle()` / `lifecycle_at()` implement `derive_mission_lifecycle`:
  mission identity, active/review/blocked/terminal counts, weighted completion,
  last transition/activity, age, stale/abandoned/archived classification,
  reopened authority from lifecycle events vs `merged_at`, and the
  `post_mission_events` list; JSON + human renderings.

`crates/sk-cli/src/compat/tasks.rs`

- `MoveTaskArgs` mirrors the upstream `move-task` parser (including the
  `--agent`/`--actor` and `--note`/`--reason` last-wins aliases via clap alias
  sets, `--auto-commit/--no-auto-commit`, tracker refs, reviewer/approval
  options, owned checkout).
- `move_task()` performs the canonical status mutation: alias-resolved target,
  review-rejection rationale requirement (`--review-feedback-file` or `--note`),
  in-review verdict construction (`approved` / `changes_requested` with the
  feedback pointer or `review:<WP>`), approval `DoneEvidence`, cancellation
  `reason_source` (`operator` for a non-blank note, else `synthetic`), the
  `planned → claimed` claim sidecar (`{shell_pid, shell_pid_created_at: "",
  agent}` exactly like `_mt_hop_policy_metadata` when no OS baseline is
  captured), off-axis `assignee`/`note`/`tracker_refs` annotation, lane-head
  stamp, and the upstream success envelope.
- `mark_status()` validates `done|pending|skipped|not_applicable`, resolves the
  authored `subtasks:` roster ownership (one delta **per WP**, `--wp` to
  disambiguate, unattributable ids are a hard refusal), records the alias as
  `skipped`, detects already-satisfied slots from the reduced snapshot, and emits
  the upstream `{results, summary}` payload.

---

## 2. Oracle evidence

`tests/fixtures/mutation/contract_vectors.json` is generated by
`generate_contract_vectors.py` against the pinned oracle. It records the oracle
identity (`baseline`, Python 3.11, `spec-kitty-events==11.3.0`, command) and:

- **all 12 canonical lifecycle payloads**, each produced by the pinned
  producer's own emitter (`emit_project_initialized`,
  `emit_mission_created_local`, `emit_artifact_phase_local`,
  `emit_wp_created_local`, `emit_reviewer_self_approval`,
  `emit_mission_reopened`, `emit_follow_up_recorded`) and read back from the
  persisted log. `mutation_oracle_contracts.rs` asserts native canonicalisation
  equals the oracle payload **and** is idempotent.
- **5 payloads the pinned pydantic models refuse** (extra key, empty required
  string, wrong list element type, negative `wp_count`, extra
  `MissionCreated` field). The native seam must refuse all of them.
- **9 `WPInnerStateDelta` vectors** (`from_dict → to_dict`, `is_empty`) covering
  PID string/float/bool coercion, blank scalars, `"  "` preservation, the
  `not_applicable` alias, empty review sentinel, and `release_runtime_claim`
  truthiness.
- the pinned `build_claim_policy_metadata` shape.
- a full **lifecycle derivation** plus the exact status rows and post-mission
  envelopes it was derived from; `status_mutation.rs` replays those inputs
  natively and compares the entire derived payload field for field.

Local workflow post-state is exercised end to end by
`crates/sk-cli/tests/status_mutation.rs` (16 tests): emit payload/actor/rows,
seed refusal, coordination refusal, selector/verdict validation, rejection with
feedback, rejection without rationale, approval evidence, done override,
claim sidecar + off-axis deltas, owned-checkout refusal, mark-status grouping /
idempotence / alias / ambiguity, lifecycle state transitions, and the oracle
derivation comparison.

---

## 3. Exact parser/dispatch integration proposal (coordinator applies)

### 3.1 `crates/sk-cli/src/compat/mod.rs`

Replace the two `UnsupportedArgs` payloads and the four `unsupported(...)` arms.

Parser (in `AgentStatusCommand` / `AgentTasksCommand`):

```rust
pub enum AgentStatusCommand {
    Validate(MissionJsonArgs),
    Materialize(MissionJsonArgs),
    /// Emit a canonical status transition.
    Emit(status::EmitArgs),
    /// Show the canonical mission lifecycle state.
    Lifecycle(MissionJsonArgs),
}

pub enum AgentTasksCommand {
    // … existing variants unchanged …
    /// Move a work package between lanes.
    #[command(name = "move-task")]
    MoveTask(tasks::MoveTaskArgs),
    /// Record subtask status (event-sourced).
    #[command(name = "mark-status")]
    MarkStatus(tasks::MarkStatusArgs),
}
```

Dispatch (inside `dispatch`'s `Command::Agent { command }` match):

```rust
AgentStatusCommand::Emit(args) => {
    let root = support::project_root()?;
    status::emit(&root, &args)?;
    Ok(0)
}
AgentStatusCommand::Lifecycle(args) => {
    let root = support::project_root()?;
    status::lifecycle(&root, args.mission.as_deref(), args.json)?;
    Ok(0)
}
```

```rust
AgentTasksCommand::MoveTask(args) => {
    let root = support::project_root()?;
    tasks::move_task(&root, &args)?;
    Ok(0)
}
AgentTasksCommand::MarkStatus(args) => {
    let root = support::project_root()?;
    tasks::mark_status(&root, &args)?;
    Ok(0)
}
```

No other `mod.rs` change is required: the route bodies own all output (JSON on
stdout, human lines) and all refusals are `CompatError`s the existing error
renderer already turns into `{"error": …}` / exit 1/2.

### 3.2 `crates/sk-cli/src/compat/capabilities.rs`

Replace the four `"unsupported"` entries (`agent status emit`,
`agent status lifecycle`, `agent tasks move-task`, `agent tasks mark-status`)
with `"state": "partial"` entries in `"commands"`, for example:

```rust
{
    "name": "agent status emit",
    "state": "partial",
    "mutates": true,
    "surface": "canonical status.events.jsonl writer under the Mission lock",
    "gaps": [
        "coordination-routed missions refuse (no coordination write surface)",
        "execution projection refresh (.kittify/derived) not implemented",
        "status/lifecycle fan-out transport not attempted"
    ]
},
{
    "name": "agent status lifecycle",
    "state": "implemented",
    "mutates": false,
    "surface": "derive_mission_lifecycle over status.events.jsonl + canonical-events.jsonl"
},
{
    "name": "agent tasks move-task",
    "state": "partial",
    "mutates": true,
    "gaps": [
        "auto-commit / commit orchestration not performed",
        "pre-review regression gate, review-cycle artifacts and verdict durability not ported",
        "--to done refuses without --done-override-reason (merge-ancestry Git gate)",
        "owned-checkout lifecycle refused"
    ]
},
{
    "name": "agent tasks mark-status",
    "state": "partial",
    "mutates": true,
    "gaps": ["history/dossier side writes and commit orchestration not performed"]
}
```

(The exact JSON key set is whatever `capabilities.rs` already uses; only the
state/reason text changes.)

### 3.3 Nothing else consumes the new API

`sk-install::lifecycle` keeps calling
`persist_lifecycle_event_local(&LifecycleRequest)`; the signature and
`LifecycleAppend`/`LifecycleSkip` shapes are unchanged, so no installer edit is
needed (its 135 lib tests, including the lifecycle suite, pass against the
validating writer).

---

## 4. Remaining external contracts / known divergences (not completion)

1. **Coordination surface.** `agent status emit`, `move-task`, `mark-status` and
   `status materialize` refuse a mission with a recorded
   `coordination_branch`. The coordination writer/transaction owner must supply
   a genuinely routed target; until then nothing writes primary as a fallback,
   and coordination parity is **not** claimed.
2. **Execution-state projection.** `refresh_execution_projection`
   (`.kittify/derived/<slug>/`: status/board/progress/lifecycle JSON, gated by
   `ledger.projection`, skipped during in-progress Git operations) is not
   implemented. The lane mirror and `status.json` materialisation are.
3. **Fan-out / transport.** Local persistence is complete; `status/adapters`,
   `runtime.next.event_bridge` delivery and resolved-binding fan-out are not
   attempted. No success is claimed for delivery.
4. **Git-derived lifecycle gates.** Pre-review regression gate, review-cycle
   artifacts/verdict durability, auto-commit and path-scoped commits, protected
   branch commit policy, done merge-ancestry (hence the explicit
   `--done-override-reason`), reviewer auto-detection from `git config
   user.name` (native falls back to the acting identity), and owned-checkout
   transitions are external contracts of other owners. The CLI accepts the
   compatibility flags but does not fake their effects.
5. **`shell_pid_created_at` baseline.** The OS process-creation probe is not
   implemented; the claim sidecar records the pinned empty-string slot that
   `_mt_hop_policy_metadata` writes when capture yields no baseline.
6. **Verified-rollback Git probe.** The protocol is implemented and tested with
   injected probes, but the workspace's Git adapter exposes no public
   "blob at `HEAD` for this repo-relative path" read, so native callers pass
   `None` and a non-empty tail refuses `HEAD_UNREADABLE` (safe, but not the
   baseline's successful cut of provably-uncommitted rows). Wiring needs that
   one `sk-git` read (or an equivalent injected probe from the owner).
7. **Datetime offsets.** pydantic preserves a non-UTC offset on dump; the
   kernel parser normalises to UTC, so an offset input renders as its UTC
   instant. The producer always supplies aware-UTC values in practice.
8. **Strict deltas.** Unknown delta keys / non-string scalars are refused
   (deliberate fail-closed divergence from the unvalidated baseline dataclass).
9. **Still absent in `sk-status`:** `status migrate-envelope` CLI wiring,
   `status doctor`, aggregate/progress/lifecycle derived views, decision-ledger
   rows, and the retrospective writers. Migration payload/envelope semantics are
   covered; the CLI route is not wired.
10. **No performance claim.** No timing work was run in this tranche.
11. **Verification scope.** Only `sk-status` and `sk-cli` were exercised. Two
    `sk-cli` `compat.rs` failures and two `sk-install`
    `full_init_baseline.rs` failures are present in the shared tree from
    concurrently modified `runtime.rs`/`init.rs`/`sk-install` surfaces
    (`next` exit class; generated agent/skill/command prompt surfaces). They do
    not touch the files or routes of this tranche and were left to their owners.

---

## 5. Verification evidence

All under temporary `HOME`/`XDG`/`TMP`; every command/probe used a temporary
project root, never this checkout's runtime state.

```sh
PYTHONPATH=src SPEC_KITTY_PACKS_ROOT=$PWD/packs \
  .venv-rust-oracle/bin/python scripts/rust/verify_oracle.py --root .
# {"ok": true, "installed_count": 53, "drift": [], "missing": [], "unsatisfied": []}

CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p sk-status --locked
# 211 passed, 0 failed, 1 ignored (cross-process child body); 20 suites
#   lib 44, event_stream 13, fsm_parity 4, fsm_semantics 16,
#   mission_write_lock 17, mutation_concurrency 3 (+1 ignored),
#   mutation_contracts 9, mutation_emit 10, mutation_lifecycle 6,
#   mutation_lock 9, mutation_migrate 10, mutation_oracle_contracts 5,
#   mutation_rollback 10, mutation_writer 11, replay_errors 8, replay_io 5,
#   replay_parity 6, replay_semantics 18, schema_audit 7
# Log: /tmp/sk-status-tests2.log

CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p sk-cli --locked --test status_mutation
# 16 passed, 0 failed  (/tmp/sk-final-cli2.log)
cargo test -p sk-cli --locked --test review_regressions      # 6 passed
cargo test -p sk-cli --locked --test repository_authority    # 1 passed, 1 ignored
cargo test -p sk-cli --locked --test cli                     # 23 passed
# Full -p sk-cli run: compat.rs 22 passed / 2 failed — both foreign (see §4.10)

CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p sk-install --locked
# 135 lib passed (lifecycle suite included) + 7 + 4/6; the 2 failures are the
# foreign full_init_baseline agent/skill surface tests (see §4.10)

CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo clippy -p sk-status --all-targets --locked -- -D warnings
# clean (exit 0, /tmp/sk-status-clippy3.log)
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo clippy -p sk-cli --all-targets --locked --no-deps -- -D warnings
# only crates/sk-cli/src/compat/runtime.rs:786 remains (foreign in-flight file;
# all status/tasks code is clean) — /tmp/sk-cli-clippy4.log
rustfmt --edition 2024 --check <every file in §"Owned surfaces touched"
  excluding new-file list above>   # clean
```

`cargo fmt -p sk-status -- --check` at workspace scope is still **not** run
here: `mutation_lock.rs` and `tests/mission_write_lock.rs` belong to the frozen
Mission-lock review and the coordinator owns workspace formatting. The files in
this tranche are rustfmt-clean individually.

---

## 6. Handoff notes

- Apply §3.1 and §3.2 verbatim; no dependency, manifest or lockfile change is
  required (the new modules use existing `serde_json`, `thiserror`, `sk-kernel`;
  the CLI probe uses existing `sk-git`/`sk-workspace`/`chrono`).
- Regenerate the contract vectors with the documented command if the baseline
  or `spec-kitty-events` pin moves; the test asserts the recorded oracle
  identity, so drift fails loudly instead of passing vacuously.
- The next status tranche should take the coordination writer (item 1) or the
  derived-projection refresh (item 2); both need their owning external
  contracts, not another local re-implementation.
