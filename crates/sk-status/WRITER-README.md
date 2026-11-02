# sk-status writers

`sk_status::mutation` is the native **write** surface for the Spec Kitty status
domain. Everything else in this crate reads. The writer modules reproduce the
pinned Python baseline (`9aed3a8012cb5eb22c0128b14d525039f95fea7a`):

| Module | Baseline counterpart | Owns |
| --- | --- | --- |
| `mutation_lock` | `status/locking.py`, `workspace/root_resolver.py` | canonical repo root, feature-dir canonicalisation, Mission/project lock keys and guards |
| `mutation_log` | `status/store.py` (`append_raw_rows_atomic`, `verify_event_readback`), `status/reducer.py` (`materialize`) | whole-file atomic batch replacement, readback, rollback, `status.json` materialization |
| `mutation_json` | `status/store.py::serialize_event_line`, `events/sanitizer.py` | PII sanitizer + Python-shaped row encoding |
| `mutation_emit` | `status/emit.py`, `status/transition_pipeline.py` | `genesis→…→done` emission, batch emission, `InnerStateChanged` annotations |
| `mutation_lifecycle` | `status/lifecycle_events.py` | nine-key lifecycle envelopes in `canonical-events.jsonl` / `status.events.jsonl`, dedupe |
| `mutation_migrate` | `status/migrate_lifecycle_envelope.py` | one-shot strict-envelope migration with snapshot/refusal |
| `mutation_ulid` | `ulid` package usage | canonical 26-char Crockford ULIDs, monotonic per millisecond |

Read surfaces (`replay`, `snapshot`, `schema`, `event_stream`, `lane`,
`transition`, `guard`) are unchanged and never write.

## Lock authority

One `FileLock` per Mission, plus one for the project log. Both come from
`sk_kernel::lock_families`, so the filename, the folder, the `.holder` sidecar
and the wait policy are the kernel's, not a second implementation.

| Write | Lock file | Key |
| --- | --- | --- |
| `status.events.jsonl` (transitions, annotations, lifecycle rows) | `<git-common-dir>/spec-kitty-locks/<key>.status.lock` | Mission directory name, or `<slug>-<mid8>` when the primary `meta.json` declares `coordination_branch` |
| `.kittify/canonical-events.jsonl` | `<git-common-dir>/spec-kitty-locks/__project__.status.lock` | fixed `__project__` sentinel |

Non-Git trees degrade exactly as the baseline does: `<root>/.git` when it is a
real directory, else `<root>/.kittify`, else `<root>/.spec-kitty` — never a
directory that would turn the tree into a fake project/repository root. A linked
worktree resolves the **main** checkout's common dir, so every worktree of one
repository contends on one lock. A registered coordination worktree keeps its
own `kitty-specs/<slug>` surface; no other worktree copy is written.

Locks are reentrant per thread, bounded by default (10 s for ordinary writes) or
unbounded when a caller asks for it, and hold a readable `.holder` sidecar while
the outermost guard lives.

## Write protocol

`append_rows_atomic(path, rows, options)`:

1. refuse a final-component symlink at `path`;
2. read the existing log (bound: 64 MiB) and refuse a non-blank line that is not
   a JSON object, with its 1-based line number, **before** touching the file;
3. insert a missing trailing newline (repair, never truncate);
4. sanitize each new row (strip `machine_name`, `hostname`, `workspace_path`,
   `developer_name`, `developer_email` at every depth; fold
   `session_started_at`/`session_ended_at` into `session_duration_s`) and encode it
   exactly as Python's `json.dumps(row, sort_keys=True)` — sorted keys,
   `", "`/`": "`, ASCII-escaped strings;
5. replace the whole file with `sk_kernel::atomic_write` (temp file in the same
   directory, fsync, rename, parent fsync) — **never** `O_APPEND`;
6. read the file back and require byte equality with the intended content;
7. an `AtomicCommitIndeterminate` outcome is *recovered by readback*: present and
   correct is success (`Durability::IndeterminateRecovered`); anything else is
   `MutationError::CommitIndeterminate` and the file is left for recovery — never
   deleted.

Rollback is explicit: `capture_log_snapshot` before the write and
`restore_log_snapshot(&snapshot, &bytes_we_wrote)` after a failed follow-up step.
The restore refuses when the log no longer holds exactly the bytes this caller
wrote, so another writer's rows and a committed log are never clobbered.

`materialize_snapshot_file(feature_dir)` runs the full consumer replay
(`ReplayInput::read_dir` + `snapshot::materialize`), renders `status.json` with
the pinned renderer, and writes it atomically. The kernel write is no-op stable,
so an unchanged snapshot never touches the file.

## Emitting a transition

```rust
use sk_status::mutation::{ActorValue, TransitionRequest, emit_status_transition};

let mut request = TransitionRequest::new(
    feature_dir,          // kitty-specs/<slug>
    "034-my-mission",
    "WP01",
    "claimed",            // canonical lane or input alias (`doing`)
    ActorValue::identity("claude"),
);
request.repo_root = Some(repo_root);   // optional explicit lock root
request.force = false;
request.reason = None;
request.review_ref = None;
request.subtasks_complete = None;      // inferred for in_progress -> for_review
request.at = None;                     // default: now
request.event_id = None;               // default: fresh ULID

let outcome = emit_status_transition(&request)?;
```

What the shell does, in order: canonicalise the feature directory → take the
Mission lock → read `mission_id` from `meta.json` → read/reduce the log once →
resolve the alias → infer the review gates → refuse an unnoted forced
`in_review`/`approved` exit → build done evidence → validate through the crate's
single `transition::validate_transition` → append the event → verify its
readback → materialize `status.json`. A refusal happens before any write.

`emit_status_transition_batch` validates a same-Mission/WP sequence under one
lock and lands every row in **one** verified replacement (all-or-nothing).
`emit_inner_state_changed` persists an `InnerStateChanged` annotation (no FSM
edge, no `force_count`) and verifies its readback.

## Lifecycle envelopes

```rust
let mut request = LifecycleRequest::new(
    mission_event_log_path(&feature_dir),
    "WPCreated",
    payload,                 // caller-supplied payload object
    "WP01",
    "WorkPackage",
);
request.dedupe_keys = BTreeMap::from([
    ("mission_slug".to_owned(), json!("034-my-mission")),
    ("wp_id".to_owned(), json!("WP01")),
]);
request.mission_slug = Some("034-my-mission".to_owned()); // routes the Mission lock
let outcome = persist_lifecycle_event_local(&request)?;
```

The envelope is the baseline nine-key shape with `schema_version: "5.0.0"`.
`(event_type, dedup tuple)` idempotence is evaluated under the lock, so two
producers cannot both append the same fact. An unknown event type is skipped
(never written). `artifact_path` dedupe compares file names.

## Envelope migration

```rust
let manifest = migrate_lifecycle_envelope(&log_path, /*dry_run=*/false, None, None)?;
```

Rewrites only lifecycle rows: drop `aggregate_type`, set `schema_version:
"3.0.0"`, synthesize `causation_id`, `correlation_id`, `data_tier`, `node_id`,
`lamport_clock`, `build_id`. Non-lifecycle rows are copied byte-identically. The
pre-migration text goes to `<name>.pre-migration.bak`; an existing snapshot is a
refusal; a re-run over an already-strict file is a clean no-op.

## Non-goals / pending

The writer is intentionally not a full port of every Python side effect. The
authoritative list with reasons lives in `WRITER-REPORT.md`; the main ones are
Git-derived lane-head stamping, the phase-1 frontmatter `lane` mirror, the
ledger-gated `execution_projection` refresh, transport fan-out, and the full
`spec_kitty_events.project_lifecycle` payload models.
