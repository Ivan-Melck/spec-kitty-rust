# sk-run-store

Persistent governed mission-run storage for the Rust Spec Kitty rewrite.

This crate is the durable substrate beneath `spec-kitty next`. It owns the
baseline run layout, identity index, event journal, run-cursor lock, durable
`start` / `query` / `advance` / `answer` transactions, governed answer
authorization, and deterministic recovery. Planning and answer policy stay in
`sk-runtime`; this crate only decides what to persist and when.

**Status: substrate implemented and tested; full `next` compatibility is not
claimed.** See [Remaining gaps](#remaining-full-next-gaps).

## Persisted layout (baseline, no native-only format)

```text
.kittify/runtime/feature-runs.json
.kittify/runtime/feature-runs.json.lock                 # index-only sidecar
.kittify/runtime/runs/<32-hex-run-id>/state.json
.kittify/runtime/runs/<32-hex-run-id>/mission_template_frozen.yaml
.kittify/runtime/runs/<32-hex-run-id>/run.events.jsonl
.kittify/runtime/runs/<32-hex-run-id>/state.json.lock   # cursor-only sidecar
.kittify/runtime/runs/<32-hex-run-id>/prompts/<step>.prompt.md
```

- `state.json` is the upstream `MissionRunSnapshot` JSON shape. No Rust-only
  cursor field (for example `actor_context` or `schema_version`) is written.
- `mission_template_frozen.yaml` is the verbatim source bytes; `template_hash`
  is their SHA-256. Live-template drift is reported, never silently re-frozen.
- `feature-runs.json` stores a repo-relative `run_dir` token keyed by
  `mission_id` (or `legacy-<slug>`), with the upstream entry fields.
- Prompt files are real UTF-8 files contained under the run directory (upstream
  writes them to a temporary namespace). They are derived artifacts, not cursor
  state, and recovery never depends on them.

## Public library API

```rust
RunStore::open(root) -> Result<RunStore, RunStoreError>
RunStore::with_lock_timeout(Duration) -> RunStore

RunStore::start(&StartRequest) -> Result<StartOutcome, RunStoreError>
RunStore::run_for_mission(slug, mission_id) -> Result<Option<RunRef>, RunStoreError>
RunStore::query(&RunRef) -> Result<QueryResult, RunStoreError>
RunStore::query_mission(slug, mission_id) -> Result<QueryResult, RunStoreError>
RunStore::read_state(&RunRef) -> Result<RunState, RunStoreError>

RunStore::advance(&RunRef, &AdvanceRequest) -> Result<AdvanceOutcome, RunStoreError>
RunStore::plan_advance(&RunRef, &AdvanceRequest) -> Result<PlannedAdvance, RunStoreError>
RunStore::commit_advance(&PlannedAdvance) -> Result<AdvanceOutcome, RunStoreError>
RunStore::answer(&RunRef, &AnswerRequest) -> Result<AnswerOutcome, RunStoreError>
RunStore::recover(&RunRef) -> Result<RecoveryReport, RunStoreError>
```

`RunRef`, `RunState`, `StartRequest`, `StartOutcome`, `QueryResult`,
`IssuedStepView`, `DecisionView`, `AdvanceRequest`, `PlannedAdvance`,
`AdvanceOutcome`, `AnswerRequest`, `AnswerOutcome`, `RecoveryReport` and
`ErrorInfo` are serde types, so a coordinator CLI can pass them between
processes. `PlannedAdvance` is a caller-owned value, not a persisted format.

## Lock and transaction authority

- The per-run cursor lock is the upstream `state.json.lock` sidecar, acquired
  through `sk_kernel::FileLock` (cross-process `flock`/`LockFileEx`, bounded
  wait, holder record, release truncates without unlinking).
- `advance` plans and commits under **one** cursor hold, from the same frozen
  template and live hash.
- `plan_advance` is non-mutating. `commit_advance` re-acquires the lock,
  re-reads, reconciles, and refuses with `STALE_ADVANCE_PLAN` when the live
  cursor is not the plan's source or the caller-evaluated issued step changed.
  The cursor's answered/completed values are unchanged on a refusal; the
  reconciliation that runs first may still have appended a derivable event or
  rolled the cursor forward from the journal, which is a repair, not a
  successful transaction.
- `answer` performs read → authorize → mutate → persist → append as one cursor
  hold. Audit owner authorization and LLM delegation come from
  `sk_runtime::apply_decision_answer`; the store cannot skip them.
- An audit denial appends the upstream `DecisionAuthorityDenied` payload and
  leaves the cursor unchanged.
- Cursor datetimes are written in the upstream Pydantic `state.json` shape
  (`2026-01-01T00:00:00Z`, six fractional digits when non-zero); journal lines
  keep the upstream `datetime.isoformat()` shape (`+00:00`). One answer
  transaction writes both formats, exactly as the reference does.
- The index has its own `feature-runs.json.lock`. Start takes index → releases →
  creates the run → takes index again. Recovery index repair takes cursor →
  index. No path takes index → cursor, so there is no cycle.
- Journal appends use the kernel's single locked event-log primitive (its own
  event-log lock, torn-tail refusal, one-unit write, fsync, readback,
  rollback). No second event writer or Python fallback exists.

## Recovery and non-mutating query

Upstream commits an advance as *events → `state.json`* and an answer as
*`state.json` → answered event*. Recovery handles both crash windows without a
new persisted format:

- **Explicit** `recover` (and mutating transactions, which reconcile
  automatically) will:
  - truncate a torn event tail **only** in `recover`; a mutating transaction
    refuses it with `TORN_EVENT_TAIL`;
  - append a derivable missing `MissionRunStarted` or
    `DecisionInputAnswered` event;
  - roll the cursor forward from `NextStepAutoCompleted`, `NextStepIssued`,
    `DecisionInputRequested`, `DecisionInputAnswered`,
    `SignificanceEvaluated` (including a LOW gate's implicit completion) and
    `MissionRunCompleted`;
  - repair a missing index entry.
- Reconciliation never guesses: a cursor completion with no journal proof, a
  conflicting answer, an input disagreement, an unknown event type or an
  unreplayable event is `INCONSISTENT_RUN_JOURNAL`.
- `query` performs the same analysis read-only and returns
  `RECOVERY_REQUIRED` with the differences instead of presenting a misleading
  state. It creates no lock, no index entry and no prompt.
- `recover` is idempotent; a second call reports `is_noop()`.

**Compatibility note:** the implicit reconciliation of derivable facts and the
explicit torn-tail repair are deliberate improvements over the pinned Python
runtime, which would leave the run stuck and require operator tooling. They
never invent a new event, format, or writer; every repaired fact comes from the
baseline journal or cursor. Torn-tail handling remains refusal-first.

## Parity evidence (Rust test ↔ upstream source)

| Rust test | Baseline authority |
| --- | --- |
| `start_creates_the_baseline_run_layout_with_frozen_raw_template_sha` | `engine.start_mission_run`, `_freeze_template`, `_write_snapshot` |
| `index_records_repo_relative_source_index_identity`, `second_start_resumes…` | `runtime_bridge_io.get_or_start_run`, `run_index` |
| `query_is_non_mutating…`, `query_surfaces_the_issued_step…` | `runtime_bridge_query._query_read_runtime_plan` (read-only path) |
| `advance_issues_a_step_and_writes_events_before_the_cursor` | `engine._commit_advance` |
| `advance_success_completes…`, `failed_results_record…` | `engine.apply_result` / `plan_advance` / `_commit_advance` |
| `a_stale_expected_issued_step…`, `committing_the_same_plan_twice…` | `engine._refuse_stale_plan`, `commit_advance` (#5682/#5854) |
| `input_answer_records…`, `llm_answers_require…` | `engine._apply_decision_answer`, `sk_runtime::apply_decision_answer` |
| `audit_answers_require_the_mission_owner…`, `audit_rejection…` | `engine._authorize_audit_answer`, audit approve/reject |
| `low/medium/high significance…` | `engine._evaluate_audit_significance`, `plan_advance` significance/RACI folding |
| `a_held_cursor_lock_times_out_a_real_second_process` | `runtime.next.run_lock.run_cursor_lock`, `kernel.locks` |
| `competing_processes_produce_exactly_one_advance` | `next_step` plan+commit under one lock (#5854) |
| `a_torn_event_tail…`, `events_ahead…`, `a_state_ahead…` | `kernel.event_log_append`, engine commit order |
| `frozen_template_hash_drift…`, live drift query | `engine._load_frozen_template`, `planner` drift rule |
| `an_index_token_outside_the_project…` | `run_index.resolve_run_dir` containment (#5390) |
| `a_legacy_run_without_identity…` | `runtime_bridge_io._entry_for_mission` |
| `no_stdout` guard / `ErrorInfo` | stdout-independent typed errors for CLI integration |
| `the_store_replays_the_oracle_advance_and_human_answer_byte_for_byte` (`tests/oracle_differential.rs`) | full cursor + journal bytes of a real oracle `requires_inputs` transcript (`tests/fixtures/next_scenario.json`, generated by `tests/oracle/generate_next_fixtures.py`) |
| `indeterminate_index_commit_preserves_the_run_the_visible_index_names` | `kernel.atomic` indeterminate-commit contract |

## Remaining full-`next` gaps

The command surface lives in `sk-cli/src/compat/runtime.rs` (query/advance/
answer composition, board override, guards, prompt authoring). This crate is the
storage substrate. It does **not** implement:

1. Mission resolution, mission-board progress, finalized/merged overrides,
   analysis-currency checks, or the `Decision`/`DecisionEnvelope` CLI JSON — all
   of which are now composed by the CLI adapter, not here.
2. WP iteration inside `implement`/`review`, workspace resolution/claims,
   owned-checkout authority, coordination topology or Git branch work.
3. Guard evaluation (artifact presence, acceptance facts, WP status) and the
   composition dispatch around an advance.
4. Decision-log Git commits, decision-timeout handling, retrospection gates,
   retrospective records, or the memory/learning hook.
5. The Mission status canonical reducer, `status.events.jsonl`, lane allocation,
   acceptance, consolidation, merge or recovery doctor.
6. The event write service / EventBridge client. The kernel's interim locked
   append is used; open-core binds no transport.
7. Mission `meta.json`/ultimatum identity discovery, pack/template discovery,
   workflow composition from `meta.json`, or governance/charter context.
8. A CLI, JSON exit-code mapping, machine output, or a stable `next --query`
   envelope.
9. Cross-platform CI evidence (tests currently run on Linux only), a
   Python-vs-Rust byte-level differential corpus for these files, and the full
   rewrite performance benchmark.
10. Byte-for-byte JSON parity for non-ASCII text: `serde_json` emits UTF-8,
    while Python's default `json.dump` escapes non-ASCII (`ensure_ascii=True`).
    Parsed values and key ordering match; raw bytes can differ for non-ASCII
    mission names/paths.
11. Full-fidelity replay of optional metadata that the baseline journal cannot
    carry (for example a RACI or soft-gate record when the corresponding event
    is absent). Cursor-critical facts are replayed; metadata is preserved from
    the durable cursor when present.
12. A hostile-filesystem capability sandbox. Paths are containment-checked with
    the kernel resolver and run ids are validated, but concurrent symlink swaps
    are outside the guarantee.
13. A benchmark. Every mutating transaction replays the journal to prove
    consistency, so the cost is O(events) per mutation; that has not been
    optimized or measured against the Python reference.
14. ~~Byte-identical event values for `mission_id`/`mission_slug`.~~ Fixed: the
    cursor and the local journal now leave both null (pinned-engine shape); the
    identity index and `inputs.mission_slug` carry identity, and the tolerant
    reader resolves it. `the_store_replays_the_oracle_advance_and_human_answer_byte_for_byte`
    pins this.
15. Prompt files are contained under `<run_dir>/prompts/` with a hashed name for
    ids outside the safe grammar, while the reference writes a shared
    `~/.spec-kitty/spec-kitty-prompts/<hash>/` namespace. Content and identity
    are preserved; the path differs, so a byte comparison of `prompt_file`
    across implementations is not a parity claim.
16. A bounded-read oversize refusal is now the typed `TOO_LARGE` code (the
    kernel's `TooLarge` is translated), and torn-tail repair runs under the
    event-log lock. Neither is a new persisted format.

## Verification

```sh
CARGO_BUILD_JOBS=2 cargo test -p sk-run-store
CARGO_BUILD_JOBS=2 cargo clippy -p sk-run-store --all-targets -- -D warnings
cargo fmt -p sk-run-store -- --check
```

Tests use `tempfile` fixtures only. They never touch this repository's
`.kittify/runtime` or the user's home directory.
