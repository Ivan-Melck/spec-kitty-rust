> **Historical.** The authoritative, current status of the writer surface —
> including typed lifecycle payloads, inner-state deltas, transition+claim
> annotations in one replacement, lane-head stamps, dependency readiness and
> the exact remaining gaps — is [`CONTINUATION-REPORT.md`](CONTINUATION-REPORT.md).
> Where this file disagrees with it (e.g. gap 5/7, M5/L11-era claims), the
> continuation report wins.

# sk-status writer implementation report

Status: **implemented and exercised** — writer substrate plus the real baseline
operations (transition emission, batch emission, annotations, lifecycle envelopes,
lifecycle-envelope migration). Not a claim of complete write parity: the exact
gaps are enumerated below.

- Repository: `/home/exedev/.t3/scratch/2026-11-02-pull-current-spec-kitty-from-442fd12a/spec-kitty-rust`
- Branch: `rust/rewrite` (no commits, no pushes made)
- Pinned behavior baseline: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`
- Oracle: `.venv-rust-oracle/bin/python` with `PYTHONPATH=src`,
  `spec-kitty-events==11.3.0` (pinned by `uv.lock`)
- Toolchain: `rustc 1.99.1`, `CARGO_BUILD_JOBS=2`

## Files delivered

New (all owned by this task):

- `crates/sk-status/src/mutation.rs` — error type + public facade.
- `crates/sk-status/src/mutation_lock.rs` — repo root, feature-dir
  canonicalisation, Mission/project lock keys and guards.
- `crates/sk-status/src/mutation_log.rs` — whole-file atomic batch replacement,
  readback, rollback, `status.json` materialization.
- `crates/sk-status/src/mutation_json.rs` — baseline PII sanitizer + Python-shaped
  JSON encoding.
- `crates/sk-status/src/mutation_emit.rs` — transition preparation/emission,
  batch, annotations, subtask gate.
- `crates/sk-status/src/mutation_lifecycle.rs` — lifecycle envelopes + dedupe.
- `crates/sk-status/src/mutation_migrate.rs` — strict-envelope migration.
- `crates/sk-status/src/mutation_ulid.rs` — canonical ULID minting.
- `crates/sk-status/tests/mutation_writer.rs`, `mutation_emit.rs`,
  `mutation_lifecycle.rs`, `mutation_migrate.rs`, `mutation_lock.rs`,
  `mutation_concurrency.rs`, `mutation_support/mod.rs`.
- `crates/sk-status/tests/fixtures/mutation/` — oracle-generated differential
  fixtures, their generator, and `provenance.json`.
- `crates/sk-status/WRITER-README.md` (this report's companion API reference),
  `crates/sk-status/WRITER-REPORT.md` (this file).

Edited (allowed surface only):

- `crates/sk-status/src/lib.rs` — module declarations, crate docs, and facade
  re-exports. No reader logic changed.

Deliberately **not** touched: `replay.rs`, `snapshot.rs`, `schema.rs`, `lane.rs`,
`transition.rs`, `guard.rs`, `event_stream.rs`, any of their tests, workspace
`Cargo.toml`, `Cargo.lock`, and `crates/sk-status/Cargo.toml`.

## Dependencies

**No new dependency was required and no manifest or lockfile was edited.** The
writers use only the crate's existing dependencies (`serde_json`, `thiserror`,
`sk-kernel`; `tempfile` as the existing dev-dependency). In particular:

- ULID minting is implemented locally (`mutation_ulid`) rather than adding the
  workspace `ulid` dependency.
- Python-compatible JSON encoding is implemented locally (`mutation_json`)
  rather than adding `serde` derive features.
- Time uses `sk_kernel::clock`; digests use `sk_kernel::content_digest`.

The coordinator therefore has nothing to integrate for dependencies. If the
coordinator later prefers the workspace `ulid`/`chrono` crates, `mutation_ulid`
is the only file to replace.

## Implemented baseline contracts

### 1. Lock authority (`mutation_lock`)

Source: `specify_cli/status/locking.py`, `specify_cli/status/mission_write.py`,
`specify_cli/workspace/root_resolver.py`, `kernel/lock_families` (native).

- `canonical_repo_root` → regular checkout returns itself; linked worktree returns
  the main checkout that owns the shared `.git`; separate-git-dir/submodule uses
  `core.worktree` or the `.kittify` marker; bare repo/outside Git is a refusal.
- `resolve_lock_root` mirrors `resolve_status_lock_root`: explicit root wins;
  non-`kitty-specs` directory anchors on itself; `kitty-specs/<slug>` resolves the
  canonical root, falling back to `parent.parent` outside Git.
- `canonicalize_feature_dir` mirrors `canonicalize_feature_dir`: a registered
  coordination worktree keeps its own directory; every other `kitty-specs/<slug>`
  path is rewritten to the main checkout's copy **when it exists**.
- `mission_lock_key` is the Mission directory name, or `<slug>-<mid8>` when the
  primary `meta.json` records `coordination_branch`. The mid8 cascade is
  `meta.mid8` → `mission_id[:8]` → the 8-char Crockford slug tail; a legacy
  `NNN-` slug falls back to the bare directory name; a modern coordination-routed
  Mission with an exhausted cascade is refused (`MissionLockKeyUnresolved`
  parity).
- Lock paths and guards come from `sk_kernel::lock_families`
  (`acquire_feature_status_lock`, `acquire_project_event_log_lock`,
  `status_lock_common_dir`), i.e. the kernel's one lock door, `.holder` sidecar,
  reentrancy and timeout mapping. Verified paths:
  `<common>/spec-kitty-locks/<key>.status.lock` and
  `<common>/spec-kitty-locks/__project__.status.lock`; non-Git degrade order
  `.git` → `.kittify` → `.spec-kitty`.

### 2. Durable batch replacement (`mutation_log` + `mutation_json`)

Sources: `status/store.py` (`append_raw_rows_atomic`,
`serialize_event_line`, `verify_event_readback`), `events/sanitizer.py`,
`kernel/atomic.py`.

- Reads the whole log without following a final symlink, bounded at 64 MiB.
- Refuses a non-blank line that is not a JSON object (`MalformedStream`, line
  number) **before** any replacement.
- Inserts a missing final newline without discarding existing bytes (baseline
  repair).
- Sanitizes each row (deep PII strip; top-level session timestamps →
  `session_duration_s`) and encodes it byte-identically to Python's
  `json.dumps(row, sort_keys=True)` (sorted keys, `", "`/`": "`, ASCII escaping,
  surrogate pairs).
- Replaces the file with `sk_kernel::atomic_write_with_options` — whole-file
  temp+fsync+rename+parent-sync. **No `O_APPEND` anywhere**, and the generic
  kernel event-log append helper is not used for either status log.
- Verifies by re-reading the file and comparing bytes; `verify_event_rows`
  additionally requires the expected event id and, when present,
  `mission_slug`/`wp_id`/`to_lane`/`mission_id` to match.
- `AtomicCommitIndeterminate` is recovered by readback: present-and-correct →
  `Durability::IndeterminateRecovered`; anything else →
  `MutationError::CommitIndeterminate` with the file preserved. Nothing is
  deleted.
- Explicit rollback: `capture_log_snapshot` / `restore_log_snapshot`; the restore
  refuses unless the log still holds exactly the bytes the caller wrote.
- `materialize_snapshot_file` uses the full consumer replay
  (`ReplayInput::read_dir` + `snapshot::materialize` + `render_snapshot_json`) and
  writes `status.json` atomically and no-op-stably.

### 3. Transition preparation and emission (`mutation_emit`)

Sources: `status/emit.py` (`emit_status_transition`,
`emit_status_transition_batch`, `emit_inner_state_changed`),
`status/transition_pipeline.py`.

Order preserved: canonicalise → lock → `mission_id` from `meta.json` → one
in-lock read of the log → derive `from_lane` (absent WP = `genesis`) → alias
resolution → review-gate inference → unnoted-forced-review-exit refusal → done
evidence build → `validate_transition` (the crate's single FSM authority) →
event build → append → verify readback → materialize.

- Wire row is `StatusEvent.to_dict()`: `reason_source`, `mission_id` and
  `review_result` are emitted only when populated, exactly as the baseline.
- Done evidence is rebuilt into the canonical `{review{reviewer,verdict,reference},
  repos[], verification[]}` shape; review results into
  `{reviewer,verdict,reference[,feedback_path]}`.
- `in_progress -> for_review` inference: implementation evidence = any prior
  transition for the WP; subtask completion = authored `subtasks:` roster from
  `tasks/WP*.md` frontmatter minus the reduced `subtasks` annotation slots
  (`done`/`skipped` only). Missing/ambiguous/unparseable roster fails closed with
  the baseline refusal text.
- A legacy alias that resolves to the current lane (`doing` while
  `in_progress`) is a **synthetic, unpersisted** event — no self-transition row.
- Batch: one lock, one log read, all members validated before any write, one
  verified replacement, one materialization.
- Annotation: one `InnerStateChanged` row (`kind: "annotation"`), no FSM edge, no
  `force_count`, verified readback, best-effort materialization.

### 4. Lifecycle envelopes (`mutation_lifecycle`)

Source: `status/lifecycle_events.py`.

- Nine-key envelope, `schema_version: "5.0.0"`, sorted keys / Python separators.
- `(event_type, dedup tuple)` dedupe evaluated under the lock; `artifact_path`
  compares file names; unknown event types are skipped without writing.
- Mission-scoped writes take the Mission lock; project-scoped writes take the
  `__project__` lock; an unresolvable repo root skips locking exactly like the
  baseline.
- `emit_project_initialized` builds the canonical `ProjectInitializedPayload`
  field set and dedupes on `project_uuid`.

### 5. Envelope migration (`mutation_migrate`)

Source: `status/migrate_lifecycle_envelope.py`.

- Only lifecycle rows are parsed as candidates; every other row (lane
  transitions, annotations, decision rows, retrospective rows) is copied
  byte-identically.
- Strict rewrite: drop `aggregate_type`, `schema_version: "3.0.0"`, synthesize
  `causation_id: null`, `correlation_id = event_id`, `data_tier: 0`, `node_id`,
  `lamport_clock: 0`, `build_id = uuid5(uuid5(NAMESPACE_URL,
  "spec-kitty:identity:build_id"), "<project_uuid>:<node_id>")`.
- `ReviewerSelfApproval` → `skipped_no_model`; null `project_uuid` →
  `skipped_no_project_uuid`; already-strict → `unchanged`.
- Snapshot to `<name>.pre-migration.bak` before the rewrite; an existing snapshot
  is a refusal; a re-run over a strictly migrated file is a clean no-op
  (`migrated_count == 0`, no snapshot, no refusal).
- Per-row SHA-256 of the canonical `(event_type, payload)` pair before/after.
- `node_id` = `sha256("<hostname>:<user>")[:12]` (injectable for tests);
  `build_id` reproduces the baseline UUIDv5 exactly.

### 6. ULIDs (`mutation_ulid`)

26-char Crockford base32, 48-bit millisecond timestamp + 80 bits of entropy,
monotonic within a millisecond; canonical uppercase shape enforced on injected
ids (the exact baseline `ULID_PATTERN`). The encoding is pinned against the ULID
specification's documented example.

## Verification evidence

All commands run in the repository above with `CARGO_BUILD_JOBS=2`.

```
cargo test -p sk-status
  lib                    14 passed
  event_stream           13 passed
  fsm_parity              4 passed
  fsm_semantics          16 passed
  mutation_concurrency    3 passed, 1 ignored (cross-process child body)
  mutation_emit           9 passed
  mutation_lifecycle      6 passed
  mutation_lock           9 passed
  mutation_migrate        9 passed
  mutation_writer        10 passed
  replay_errors           8 passed
  replay_io               4 passed
  replay_parity           6 passed
  replay_semantics       18 passed
  schema_audit            7 passed
  total                 136 passed, 0 failed, 1 ignored
```

New tests: 46 integration tests (+1 ignored cross-process child body) and 11
in-module unit tests (`mutation_json` 6, `mutation_ulid` 5). The pre-existing
reader tests are unchanged and still green.

```
cargo fmt -p sk-status -- --check      # clean
cargo clippy -p sk-status --all-targets # no warnings from writer code
```

The single remaining Clippy warning is pre-existing in
`crates/sk-status/tests/schema_audit.rs:148` (`cloned_ref_to_slice_refs`), a file
owned by the coordinator/previous tranche and outside this task's write scope.

### Differential oracle fixtures

Generated once by the pinned Python implementation and committed with
`provenance.json` (baseline commit, interpreter, `spec-kitty-events==11.3.0`,
generation command). Regeneration is deterministic: running
`generate_fixtures.py` twice produced identical SHA-256 sums for every fixture
file.

| Fixture | What the oracle produced | Native assertion |
| --- | --- | --- |
| `status_append` | `store.append_raw_rows_atomic` + `reducer.materialize` over a log with a transition, an annotation, a lifecycle row, a structured actor, PII fields, session timestamps, a non-ASCII reason, and a torn (newline-less) tail | appended log **and** `status.json` byte-identical; second materialization reports no change |
| `emit_sequence` | four `emit.emit_status_transition` calls (`genesis→planned→claimed→in_progress→for_review`) with injected ULIDs/timestamps | log and `status.json` byte-identical |
| `lifecycle_project` | `emit_project_initialized` with injected id/time + a second dedupe call | log byte-identical; second call `AlreadyPresent`; built-in builder reproduces the same line |
| `lifecycle_mission` | `persist_lifecycle_event_local` (`WPCreated`, canonical payload, mission lock) | log byte-identical |
| `migration` | `migrate_lifecycle_envelope` with injected `node_id`, mixed log (legacy, strict, skipped, non-lifecycle, annotation, decision, retrospective) | migrated log, `.pre-migration.bak`, and the per-row manifest (actions + content hashes) byte/value-identical, including `build_id` |

### Concurrency

- 4 threads claim 4 different WPs simultaneously: all 8 rows land (no lost
  whole-file replacement) and every WP ends `claimed`.
- 2 threads claim the **same** WP: exactly one succeeds, the loser gets
  `IllegalEdge(claimed → claimed)`; exactly one claim row exists.
- 2 **separate OS processes** claim different WPs of one Mission concurrently:
  both succeed and both rows are in the log. The child body is an `#[ignore]`d
  test invoked by the parent through `--ignored --exact`, so it is a real second
  process, not a thread.

### Failure and recovery

- Malformed existing stream → typed refusal with line number, file unchanged, no
  temporaries left.
- Symlinked log path → refusal, symlink target untouched.
- Oversized log bound → refusal, nothing created.
- Injected `AtomicCommitIndeterminate` after a correct rename → recovered by
  readback (`IndeterminateRecovered`).
- Injected `AtomicCommitIndeterminate` after a foreign write → `CommitIndeterminate`,
  `may_have_mutated() == true`, file preserved.
- Injected readback mismatch → `ReadbackMismatch`, file preserved.
- Rollback restores the captured bytes only while the log still holds exactly
  what this caller wrote; a foreign tail produces `RollbackRefused` and is kept.
- Lock contention with a bounded wait → kernel `StatusLockTimeout` naming the
  holder; same-thread reentrancy keeps one holder record and clears it only on
  the outermost drop.
- Guard refusals (illegal edge, unnoted forced review exit, empty annotation
  delta, non-ULID injected id, missing subtask roster) persist nothing.

## Preserved semantics (verified, not assumed)

- Mission lock key/name/folder, reentrancy, `.holder` sidecar lifecycle, bounded
  timeout text, non-Git degrade order, project sentinel.
- Linked-worktree convergence on the main checkout's lock directory, and
  coordination-worktree preservation of its own `kitty-specs` surface.
- Whole-file replacement, newline repair, PII sanitization, Python row bytes,
  sorted keys, ASCII escaping, `reason_source`/`mission_id`/`review_result`
  conditional emission.
- Readback evidence and event identity matching.
- `status.json` bytes and no-op stability.
- Lifecycle envelope bytes, dedupe semantics (including `artifact_path`
  basename), unknown-type skip, project/mission lock routing.
- Migration bytes, UUIDv5 `build_id`, snapshot refusal, idempotence, skip
  actions, content hashes.
- FSM validation through the crate's single `validate_transition`, alias
  collapse, force+note rule, done-evidence requirements.

## Deliberate divergences (and why)

1. **Malformed existing stream is refused before the append.** The baseline
   appended first and then failed `verify_event_readback`, leaving the new row on
   a log no reader can parse. Refusing first fails closed and keeps the file
   byte-identical. (Also documented in `mutation.rs`.)
2. **Bounded log size (64 MiB).** The baseline reads unbounded; a corrupted or
   hostile file should not exhaust memory before the refusal.
3. **Batch ordering when `at` is omitted.** The baseline chains the producer
   timestamp by +1 µs per batch member. Native requests carry an explicit `at`
   (fixtures pin distinct values, so bytes match); when a caller omits `at`,
   each member gets the current timestamp and the monotonic ULID keeps the
   diary's `(at, event_id)` order. Order-equivalent, not byte-equivalent, in that
   omitted-`at` case.
4. **Worktree registry read without Git.** `canonicalize_feature_dir` reads
   `<git-common-dir>/worktrees/*/gitdir` instead of `git worktree list
   --porcelain`. For ordinary checkouts the registered set is the same; a
   prunable/locked-entry edge may differ, and the baseline shape fallback is used
   when the registry is unreadable. This keeps the writer subprocess-free (the
   kernel's own rule) so a lock can never be held across a child process.
5. **`from_lane` uses the diary fold only.** `derive_from_lane` calls
   `replay::reduce_diary` (the pinned `diary.reduce_parsed` equivalent), exactly
   the baseline `reducer.reduce` semantics for lane derivation. It is *not*
   `snapshot::materialize`, and the writer never uses `reduce_rows` to produce
   `status.json`.
6. **`python_compact_json` float formatting.** Integers and all observed row
   shapes match Python byte-for-byte; floats use serde's shortest round-trip
   representation, which agrees with Python `repr` for the values these logs
   carry today. No float is written by the writer itself.

## Gaps — not implemented (exact)

Writer-adjacent behaviour that the baseline performs and the native writer does
not, each with the reason:

1. **`policy_metadata.lane_head` stamp.** Baseline `status/lane_head.py` probes a
   Git branch. This crate never runs Git, so no stamp is written. Downstream
   approval-bound/attribution consumers that require the stamp need the Git
   adapter (or an injected probe) before this can be closed.
2. **Phase-1 frontmatter `lane` mirror.** Baseline rewrites a user-authored WP
   Markdown frontmatter when `meta.json.status_phase >= 1`. Not ported: it would
   reserialize user Markdown (a charter-level preservation hazard) and belongs
   with a frontmatter-preserving writer.
3. **Execution-state projection refresh.** Baseline `refresh_execution_projection`
   is gated on `ledger_posture` and writes `.kittify/derived/`. Not ported.
4. **Fan-out / transport.** `status/adapters.py` and
   `runtime.next.event_bridge` are transport seams; open-core registers no
   handler. Local persistence is complete, delivery is not attempted, and no
   success is claimed for it.
5. **Typed annotation delta.** `WPInnerStateDelta` is not re-implemented; callers
   pass a delta object with only populated slots. Empty deltas are refused and
   readback is verified, but per-field type validation/`None` omission is the
   caller's responsibility.
6. **Lifecycle payload models.** The canonical
   `spec_kitty_events.project_lifecycle` models are an external contract; this
   crate does not re-implement pydantic validation/serialization. Only
   `ProjectInitialized` has a builder, and that builder passes
   `initialized_at`/timestamps through verbatim (pydantic normalizes datetimes to
   `...Z`). All other event types take a caller-supplied payload object.
7. **Dependency readiness from frontmatter.** Baseline `_declared_dependencies`
   (plus `readiness_from_snapshot`) resolves a WP's declared `dependencies`.
   Native `prepare_transition` accepts the verdict as `dependency_ready`; an
   omitted verdict fails open exactly like the guard's `None`.
8. **Subtask YAML coverage.** The roster reader supports the canonical authored
   forms (`subtasks:` block list, inline list, `[]`); comments inside the block
   are skipped and quoted values unquoted. Other YAML shapes (anchors, nested
   structures, non-string scalars) are refusals, never a silently empty roster.
   No YAML dependency was added.
9. **Owned-checkout / moved-key lock nuances.** `registered_hold` alias
   registration, `extend_mission_write_lock_to_moved_key`, and the owned-checkout
   primary-meta branch (plan A4) are not ported. A writer that mutates
   `coordination_branch`/`mid8` mid-hold is out of scope for this substrate.
10. **Checkout-claim lock shell.** The kernel guard exists
    (`acquire_checkout_claim_lock`), but no transition shell takes it; the
    single-branch claim window is not yet implemented.
11. **`status materialize` recovery command and write observers.** A
    `materialize_snapshot_file` failure is reported in
    `EmitOutcome::materialize_error` (baseline logs a warning and keeps the event
    authoritative), but there is no recovery command or
    `kernel.atomic.notify_written` observer integration.
12. **Environment-override Git topologies.** `canonical_repo_root` handles
    regular repositories, linked worktrees and separate-git-dir/submodule
    checkouts; `GIT_DIR`/bare edge parity with `resolve_canonical_root` is not
    exhaustively verified by tests.
13. **Performance.** No benchmark, no performance claim. The writer is
    correctness-first; the only measured property is that reads are bounded and
    the 136-test crate suite completes in well under a second of test time.

## Notes for the coordinator

- `crates/sk-status/src/lib.rs` is the only tracked file changed; it adds module
  declarations, the facade re-exports and updated crate docs. It does not change
  any reader behaviour.
- No workspace manifest/lockfile edit is required: no dependency was added.
- `tests/fixtures/mutation/` includes `generate_fixtures.py`. It is a
  fixture-generation tool for the pinned oracle, never invoked by Rust, and is
  excluded from the test runtime.
- `crates/sk-status/tests/schema_audit.rs:148` has a pre-existing Clippy warning;
  it was left untouched.
- The task's requested targeted tests were run for the new files; whole-workspace
  checks and aggregate Clippy/format remain coordinator-owned per `LOOP.md`.
