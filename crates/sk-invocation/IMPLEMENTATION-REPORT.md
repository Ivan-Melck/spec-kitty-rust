# `sk-invocation` — Stage 1 implementation report (FINAL)

Status: **implementation complete for the stage-1 scope below; the F1–F7
review findings are remediated on top (see `REMEDIATION-REPORT.md`); not
approved.** The provisional version of this file was written before any crate
body existed (see "Provisional record, retained and corrected"). This version
records what was built, the evidence, the exact failures, and what stage 2
still needs.

No caller is wired. `compat/capabilities.rs`, the dispatcher, the manifests and
`Cargo.lock` were not touched by the implementation owner, and no invocation
capability is claimed. The coordinator owns the local-package manifest/lock
addition.

> Coordinator follow-up: three symlink unit tests in `src/resolve.rs` used
> unconditional `std::os::unix` imports. They are now `cfg(unix)`; Linux still
> exercised all 72 cases. No Windows execution/compile result is claimed.
>
> **Remediation follow-up (see `REMEDIATION-REPORT.md`):** the F1/F2 findings
> were fixed by the coordinator before this round (own-trail precondition
> ceiling, object-only mode rows), and this remediation round fixed F3–F7.
> The duplicate `src/resolve.rs` is **deleted**; path identity now uses the new
> kernel authority `sk_kernel::reference_path::resolve_reference_path` (the
> Python non-strict profile, not the containment resolver). The hash table in
> §3 is the stage-1 delivered snapshot; §3.1 records the post-remediation
> snapshot, including the deleted row.

## 1. Scope delivered

Stage 1 of `/tmp/sk-rust-native-invocation-composition-plan.md` §11 inside
`crates/sk-invocation/**` (never `Cargo.toml`):

- typed v2 `started`/`completed` Op events, pre-v2 (legacy) line detection, and
  not-a-lifecycle detection;
- the canonical invocation-id boundary (26-character upper-case Crockford,
  `fullmatch`) and genuine ULID minting as a separate operation;
- `ModeOfWork`, the `entry command → mode` table, and the pure record-read
  policy (`classify_mode_of_work`, `mode_permits_evidence`, `RecordedMode`);
- the Op lifecycle/close policy stage 1 owns: exclusive-create start, closure
  spine, already-closed, started-identity precondition, torn-tail refusal,
  index as a warning only, correlation links, `normalise_ref`;
- the locked `InvocationWriter` on the kernel event-log protocol;
- Op-record root resolution (`resolve_op_record_root`, `is_worktree_of`,
  `locate_op_record_root`) and the admission refusal value/codes/exact message
  vocabulary from `op_custody.py`;
- Python `json.dumps(model_dump(exclude_none=True))` byte shape, including
  `ensure_ascii=True` surrogate pairs and `", "`/`": "` separators.

Not delivered, and not simulated: the admission *decision* (`OpWriteRefused`
raising site), executor (`invoke`/`dry_run`/`complete_invocation`), profile
catalog/registry, router, governance context, glossary chokepoint/bundle and
observation line, model routing, executor auto-commit + `op_commit_policy`
skip/tracking probes, `CommitPort`, evidence promotion, StepContract, WP05
`kitty-ops/lifecycle/**` records, `MissionNextInvoked`, owned checkouts, CLI
wiring, capability-matrix edits, manifests/lockfile.

## 2. Typed API (public surface)

```rust
// record.rs
pub struct InvocationId;                       // parse(&str) -> Result<Self, InvocationIdError>; mint()
pub enum ModeOfWork { Advisory, TaskExecution, MissionStep, Query }
pub enum OpOutcome { Done, Failed, Abandoned }
pub enum ClosedBy { Agent, DoctorSweep }
pub struct OpStartedFields { /* all 13 declared inputs, invocation_id typed */ }
pub struct OpStartedEvent;                     // new(fields) -> Result; from_object(&Map) -> Result
                                               // to_jsonl_line(); typed accessors; invocation_id()
pub struct OpCompletedFields { invocation_id, completed_at, outcome, closed_by, evidence_ref }
pub struct OpCompletedEvent;                   // new/from_object/to_jsonl_line/accessors
pub enum OpEvent { Started(OpStartedEvent), Completed(OpCompletedEvent) }
pub fn parse_op_event(&Value) -> Result<OpEvent, RecordParseError>

// modes.rs
pub const ENTRY_COMMAND_MODES: &[(&str, ModeOfWork)];
pub fn derive_mode(&str) -> Result<ModeOfWork, UnknownEntryCommand>;
pub enum RecordedMode { Absent, Known(ModeOfWork), Undetermined(Value) }
pub fn classify_mode_of_work(Option<&Value>) -> RecordedMode;
pub fn mode_permits_evidence(&RecordedMode) -> bool;

// error.rs
pub struct InvocationIdError;                  // "invocation_id must be a 26-char ULID, got <repr>"
pub struct RecordValidationError;              // "<field>: <constraint>"
pub struct LegacyRecordError;                  // exact outer template + upgrade guidance
pub enum RecordParseError { Legacy(..), NotLifecycle(String) }
pub enum InvocationError { InvalidInvocationId, PathRefused, Write, Record, AlreadyClosed,
                           InvalidCorrelationLink }

// writer.rs
pub const EVENTS_DIR: &str = "kitty-ops";      // + INDEX_PATH, OP_CLOSURES_FILENAME, RECORD_MODE
pub struct InvocationWriter;                   // new(root), with_lock_timeout(Duration)
pub struct StartedWrite { path, index_warning: Option<IndexWarning> }
pub struct IndexWarning { index_path, message }
pub struct CorrelationLink;                    // artifact(), artifact_kind(), commit(), at()
impl InvocationWriter {
  invocation_path(&InvocationId) -> Result<PathBuf, InvocationError>;
  write_started(&OpStartedEvent) -> Result<StartedWrite, InvocationError>;
  write_completed(&OpCompletedEvent) -> Result<PathBuf, InvocationError>;
  append_correlation_link(&InvocationId, CorrelationLink) -> Result<(), InvocationError>;
  read_started(&InvocationId) -> Result<OpStartedEvent, InvocationError>;
  read_started_mode(&InvocationId) -> Result<RecordedMode, InvocationError>;
  closed_ids() -> Result<BTreeSet<InvocationId>, InvocationError>;
  record_root(); events_dir();
}
pub fn op_closures_path(&Path) -> PathBuf;
pub fn append_op_closure(&Path, &OpCompletedEvent) -> Result<PathBuf, InvocationError>;
pub fn read_op_closures(&Path) -> Result<Vec<OpCompletedEvent>, InvocationError>;
pub fn closed_invocation_ids(&Path) -> Result<BTreeSet<InvocationId>, InvocationError>;
pub fn normalise_ref(&str, &Path) -> String;

// op_custody.rs
pub const OP_WRITE_PROTECTED_BRANCH, OP_WRITE_PROTECTION_CONFIG_INVALID;
pub enum OpWriteRefusalCode { ProtectedBranch, ProtectionConfigInvalid }
pub enum OpWriteAction { Open, Close }         // "open the Op" | "close the Op"
pub struct OpWriteRefusal { code, reason };    // protected_branch(&str), protection_config_invalid(..)
impl OpWriteRefusal { message(&Path, OpWriteAction) -> String }
pub struct OpWriteRefused { refusal, record_root, /* Display = message */ }
pub fn resolve_op_record_root(&Path /*project*/, &Path /*start*/) -> PathBuf;
pub fn is_worktree_of(&Path, &Path) -> bool;
pub fn locate_op_record_root(&Path, &str, Option<&Path>) -> Option<PathBuf>;
```

`python_json` is a private, domain-local line renderer (`PythonJsonObject`,
`python_repr`, `python_seconds`); nothing depends on `sk-status` and there is
no generic JSON facade. Reference-path identity is the kernel's
`reference_path` authority (see D11); this crate carries no resolver copy.

## 3. Source binding (sha256)

Pinned Python behaviour (`src/` baseline `9aed3a8012cb5eb22c0128b14d525039f95fea7a`):

```
978e890fa22ab0651af76badfa6d3610b9746bf78748784a413ea4f9961947e9  src/specify_cli/invocation/record.py
d9b7787f03ef7ba8a21c56e955892fff7f859b43498410241b3b3952bcc34eff  src/specify_cli/invocation/writer.py
45867b93587fee8232ce495bc30bc65abe9d6488b07c6ead0c22b5acbc22f94d  src/specify_cli/invocation/modes.py
997be074fda4a70d279a01ccb7c4579e752d4d0ebd4a739b45073a752fef8c75  src/specify_cli/invocation/errors.py
465abfd972c6c8352235b18e2c348c7d4e5a8860c8285ea50a4145b189b28f40  src/specify_cli/invocation/op_custody.py
c5b8092a29231bfbf60cb29859dc5cae0d2154a79c80b6d6030a0b83a55d14c1  src/specify_cli/invocation/lifecycle.py
23a0aab143de3b3972acf09a41b5a6a95ca2d5e216f0cf2062e37b0d2d9b0a4c  src/kernel/event_log_append.py
ce1cfcde1aa70959c8448dd64fc14541cfb71abee7f9224521440400fcda5c61  src/kernel/clock.py
15772f225425a8d2714d713007ed1fa10a36c74c86756a90acdd52ffd62131ae  src/kernel/git_topology.py
e50231414f0d513d88727832944e828329ba2c3ce88a9d024c79d1c34893cd4f  src/specify_cli/git/commit_helpers.py
fe6ed138e360ace576ba41b414e0325784f384aefeb6d7a3ae6fe573c397ea94  src/specify_cli/core/utils.py
8c31578b8c492d2da5045e0c7161f7ee30ae5f798a8eafd1e8116c32642f21e9  src/kernel/resolution.py
```

Native authorities reused (unmodified):

```
7c344858cdf62589d54f3aed8d8d33048cf47f21be5ca52c6829940b5a6b3281  crates/sk-kernel/src/event_log_append.rs
b1a9eeca7d38a1d718f0750a2b6dc9635f865ff346b298de1ad4dcbcb5bf3678  crates/sk-kernel/src/clock.rs
efe61ee9349c354b1d430321c46ff922ea2990ad0e03c9d583aa7a25d3ce044d  crates/sk-kernel/src/git_topology.rs
3dc1c42cfca9e60e79ae6bf18b75d4035a67a18f28bb143f4ce1b0efa05e0f1c  crates/sk-kernel/src/containment.rs
```

Written by this task (`Cargo.toml` untouched: `d65a4607…21ca63`):

```
843f1c3766d57516a54098542b92a4bf08621b27b89b70a3c756dc440a3e819e  src/error.rs            (134 lines)
d7862b66805ed940eeb0a39f7beeeeffbb0587d7c54cfd7c0340792abf15521a  src/lib.rs              ( 53)
cc2b8be5b60c95965d788db398a2ca423707d0e5d005c5cf261754e68ff862cb  src/modes.rs            (229)
2d7a33ff372e0f47faf166ed3e27ad3d37e279976b31adfd4acd9a88aaed5da9  src/op_custody.rs       (376)
f2d853f62d05aa36774ea746d7a5e1117aa6c49390f5e2dd9140f48f38bf1d5c  src/python_json.rs      (231)
0170e3a2152edad0dcfa702d4a3a291965c001f004c76ae3ce7487a84e335671  src/record.rs           (959)
# src/resolve.rs (171 lines, hash a7250dd3…) was deleted in the F5
# remediation: superseded by the kernel reference_path authority; see §3.1.
0a703b07f58476b03a6588a093124dc1a3eb4b69974c8d5301057032fab3cde5  src/writer.rs           (865)
c24a6fd64dfc79347a314e83cf9486f42fec8c4d9e1ccaedd65f92f840c634c5  tests/lock_contention.rs    (231)
f9e2f070716f84bbb9ce6a6503f6b0395df7ff49e7205feed0ecfbdf0fefb9ad  tests/op_custody_roots.rs   (227)
9ab35bf7b62f960369e7d916f8f870c0517ecba025f7c72a0eff9da0bdf7a9c3  tests/oracle_vectors.rs     (537)
16f6f6d74d1c577d4ed418efe2c46b04a4097aeaa267b24345b2a366d515749b  tests/writer_filesystem.rs  (672)
e6cca12a437bf7a47ad567de52c283e2d202932292fcc697ff6fdd2f0d539d65  tests/support/mod.rs        (198)
```

### 3.1 Post-remediation snapshot (this round)

Current files: the coordinator's F1/F2 edits are in `writer.rs`, `Cargo.toml`
and `tests/writer_filesystem.rs`; the rest is this remediation round.
`src/resolve.rs` no longer exists.

```
843f1c3766d57516a54098542b92a4bf08621b27b89b70a3c756dc440a3e819e  src/error.rs
833a971b33a0340b2c35a120398efadfb16a8ff8daf3cb9278c62ce14f165bc5  src/lib.rs
cc2b8be5b60c95965d788db398a2ca423707d0e5d005c5cf261754e68ff862cb  src/modes.rs
7694f7bff9f5c8258e020cc2ce7a4059126d12a318dad2bae8ef00b46a161da4  src/op_custody.rs
53cad7da714c82bc956e9fe166e8a22b608badf333c61fe1a97316a72960e4b9  src/python_json.rs
0170e3a2152edad0dcfa702d4a3a291965c001f004c76ae3ce7487a84e335671  src/record.rs
7d5cd86a6895b5b98bc51ca93d165fe043a712d354916e79b8ded759bdd2fef5  src/writer.rs
c24a6fd64dfc79347a314e83cf9486f42fec8c4d9e1ccaedd65f92f840c634c5  tests/lock_contention.rs
8ff3ef84d1966362624645fdbefd3f28b426ccd65fd900e214fe780b0f474107  tests/op_custody_roots.rs
17513e96a55c53bb0a45a8d42217e4c1930ca99f26452cd5a41be232439f7e64  tests/oracle_vectors.rs
e5cf9cf85683d1562761674e276c11bfbe0d10412437fc5c49923d95529b3201  tests/reference_path_oracle.rs  (new)
e6cca12a437bf7a47ad567de52c283e2d202932292fcc697ff6fdd2f0d539d65  tests/support/mod.rs
bd7488fea14b397ae79276fb02afa3486a014c2c890fa093159d455c7f8d137a  ../sk-kernel/src/reference_path.rs (new)
ed1bec27c6e2e60988f53975b79db97b38e485f30b264e14656b8c03a8221c7d  ../sk-kernel/src/lib.rs  (module + re-export lines only)
```

Native checkpoint the work was applied on: `bfc79fdfd413e69059b6a1475d31e70317f8b6b8`
(the coordinator's `M Cargo.lock` and the two `docs/rust/*` edits were already
present and are not mine).

## 4. Fidelity corrections against the plan (implemented as the source is)

1. **Id boundary is the regex, not a ULID decoder.** Implementation:
   `chars.count() == 26 && all(is_crockford)` on
   `[0-9A-HJKMNP-TV-Z]`. `"Z"*26` and `"0"*26` are valid ids; lowercase,
   `I/L/O/U`, 25/27 characters, a trailing newline and a trailing space are
   refused. `ulid` 1.2.1's `from_string` is *not* a substitute: it accepts
   26 `Z`s and silently drops the two overflow bits (`Z…Z` renders back as
   `7…Z`); that is asserted in `record::tests::id_boundary_is_the_regex_not_ulids_decoder`.
   The earlier `as_ulid()` helper was **removed** as a lossy façade.
2. **No invented validators.** `governance_context_hash` is any string (`""`
   ok, `"abcdef0123456789"` is just a value); `started_at`/`completed_at` are
   any non-empty string (`"not-a-time"` ok); `router_confidence`,
   `mission_id`, `wp_id`, `evidence_ref` are plain optional strings (`""` ok);
   `model_id` is the one optional field with a 1-character floor;
   `profile_id`/`action`/`actor`/`started_at`/`completed_at` are the non-empty
   required strings.
3. **Unknown keys ignored, never denied**, and dropped on re-serialisation.
4. **No `schema_version`** field exists in the v2 wire; the line is exactly
   the declared fields.
5. **No compact/UTF-8 wire.** Op lines use field order, `", "`, `": "`,
   `ensure_ascii=True`; supplementary planes become lower-case UTF-16
   surrogate pairs (`\ud834\udd1e`), `U+0001` → `\u0001`, `U+007F` → `\u007f`.
6. **No admission decision.** `permission`/skip probes are absent, so
   `OpWriteRefusal` is only a value + message; nothing decides it.
7. Pydantic `frozen=True` is a Python model property with no wire/reader
   consequence; Rust values are immutable by construction (`new` is the only
   validating constructor) but fields are not `const`-frozen.

## 5. Byte and validation evidence (live oracle)

`tests/oracle_vectors.rs` runs the pinned oracle
(`SK_INVOCATION_PYTHON`/`SK_KERNEL_PYTHON` → `.venv-rust-oracle/bin/python`,
verified by `scripts/rust/verify_oracle.py`, never skipped) and compares:

- **14 line vectors** (`record_bytes_match_the_lock_exact_oracle`): contract
  started/completed, all-optionals, no-optionals, empty request in query mode,
  non-ASCII (`héllo ☃ 𝄞` plus `\u0001`/`\u007f`/quote/backslash/tab/newline,
  non-ASCII profile/mission/wp/model ids), an ASCII printable mix
  (`/~!@#$%^&*()_+={}[]|;:,.<>?`'\"\\`), ASCII controls
  (`\u0000\u0008\u000b\u000c\r\u001f`), 26 `Z`s, 26 zeros, doctor-sweep
  abandoned, empty evidence, unknown-key tolerance. **Byte equality** with
  `json.dumps`, plus a hard-coded pin of the oracle's contract line and of the
  surrogate-pair escaping, so oracle drift cannot hide.
- **30 validation vectors**
  (`record_validation_outcomes_match_the_lock_exact_oracle`): accepted shapes,
  free-text acceptance, every required/literal failure, legacy completed
  (missing `closed_by`), legacy started (missing v2 fields), legacy
  literal/type failures, not-lifecycle events, and the id edges above. The
  native classification (`ok:OpStartedEvent` / `ok:OpCompletedEvent` /
  `legacy` / `value_error`) must equal the oracle's in every case; the test
  also asserts ≥5 accepted, ≥10 legacy, ≥3 not-lifecycle (non-vacuity).
- **Archive bytes** (`writer_archive_bytes_match_the_lock_exact_oracle`): the
  index line, `artifact_link` and `commit_link` written to disk by the real
  writer are compared to the oracle's `json.dumps` of the same reference dicts
  (insertion order preserved by constructing the dicts in the Python child, not
  by round-tripping a serde map), and the trail/spine line order is asserted.

Probe roots used to establish these shapes and preserved:
`/tmp/sk-inv-probe/{probe.py,probe.out,probe2.py,probe2.out}` (pydantic lax
coercions, free-text fields, id edges, `repr`, `json.dumps` escaping).

## 6. Writer protocol and exact failure text

Implemented with the kernel authority (no second lock, no second append
protocol):

- Start: `event_log_lock_path(kitty-ops)` + `FileLock::acquire` (family lock),
  then `OpenOptions::create_new` with mode `0o600`, one `write_all` of
  `line + "\n"`, `sync_all`, then drop the lock and append the index (own
  anchor, `no_follow`, mode `0o600`).
- Appends (`completed`, `artifact_link`, `commit_link`): kernel
  `append_event_log_lines_guarded` with `create=false`, `no_follow=true`,
  `TornTailPolicy::Refuse`, one unit, fsync + readback, `lock_anchor =
  kitty-ops`. Under that lock the precondition reads the existing rows and
  checks, in the reference's order: first row is `event=started`; its embedded
  `invocation_id` equals the requested id; and (only for `write_completed`) no
  `completed` row exists.
- Spine: `append_op_closure` uses the path-anchored append (`no_follow`, mode
  `0o600`, create-parents by the caller's `mkdir`), never the family anchor;
  `read_op_closures` skips blank/malformed/started lines.

Message vocabulary (every string is the reference's). The trigger column is
where this port asserts it, so a rendered-but-unasserted message is visible:

| Message | Asserted by |
|---|---|
| `Invocation record not found: <id>` | filesystem integration |
| `Invocation record must not be a symbolic link: <path>` | filesystem integration |
| `Refusing to access path outside trusted roots: <resolved>` | filesystem integration |
| `ULID collision on <path> — retry with a new invocation_id` | filesystem integration |
| `Failed to write invocation record: <error>` | filesystem + lock integration |
| `Failed to open invocation record safely: <protocol text>` | refusal-seam unit test (`writer::tests`); the kernel's own no-follow error is asserted in `sk-kernel` |
| `Invocation record is unreadable: <id>` | filesystem integration |
| `Invocation record is unreadable: <id> (<protocol text with Error code: EVENT_LOG_TORN_TAIL.>)` | filesystem integration |
| `Invocation record has no started event: <id>` | filesystem integration |
| `Invocation record identity mismatch: requested='<id>', embedded='<id>'` | filesystem integration |
| `Invocation <id> is already closed.` | filesystem integration |
| `Failed to append completed event: <error>` | lock-contention integration (real timeout) |
| `Failed to append correlation event: <error>` | refusal-seam unit test (`writer::tests`) |
| `Exactly one of ref or sha must be provided` | filesystem integration |
| `Refusing to open the Op: …` / `Refusing to close the Op: …` (protected branch, invalid protection config) | unit tests (`op_custody::tests`) |

Protocol refusal text rendered from the kernel's typed error (the pinned
interim protocol's own wording): torn tail, final-component symlink
(`Error code: EVENT_LOG_SYMLINK.`, seam-asserted), lock
timeout (`<anchor>: event-log lock <path> was not acquired within <n>s … Error
code: EVENT_LOG_LOCK_TIMEOUT.`), and lock unavailable
(`Error code: EVENT_LOG_LOCK_UNAVAILABLE.`, exercised end-to-end by an
unresolvable `.git` marker), from
`kernel.event_log_append`. `python_seconds` renders the timeout the way the
reference's f-string does (`10.0`, `0.2`).

## 7. Evidence: commands, counts, logs

All Cargo ran through `bash /tmp/sk-focused-cargo.sh` (existing target dir,
whole-command lease, `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, temporary
HOME/XDG/TMP, retained CARGO/RUSTUP homes). Scoped formatting only this crate;
no aggregate, no release, no clean.

| Gate | Result | Log |
|---|---|---|
| `cargo check -p sk-invocation --locked --offline` | pass | `/tmp/sk-invocation-stage1-check.log` |
| `cargo test -p sk-invocation --locked --offline` (stage-1 snapshot) | **72 passed, 0 failed, 0 ignored** (30 unit; 4 lock contention; 12 custody/roots; 3 oracle; 23 filesystem; 0 doctests) | `/tmp/sk-invocation-stage1-test.log` |
| `cargo test -p sk-invocation -p sk-kernel --locked --offline` (post-remediation) | `sk-invocation` **80 passed, 0 failed, 0 ignored**; `sk-kernel` 199 passed/1 ignored helper. Full breakdown in `REMEDIATION-REPORT.md` §4 | `/tmp/sk-op-remediation-*.log` |
| `cargo clippy -p sk-invocation -p sk-kernel --all-targets --locked --offline -- -D warnings` | pass, no warnings | `/tmp/sk-op-remediation-clippy.log` |
| `rustfmt --edition 2024 --check` on every file this work touched | pass | `/tmp/sk-op-remediation-fmt.log` |
| `git diff --name-only` | only `crates/sk-invocation/**` (minus `Cargo.toml`), the new kernel `reference_path` module + its two `lib.rs` lines, and the pre-existing other-owner edits; no manifest, CLI, capability or production-Python change | — |

Cross-process lock contention is real, both directions
(`tests/lock_contention.rs`):

1. Native holds the `kitty-ops` family lock → a real Python child calling
   `kernel.event_log_append.append_event_log_lines(..., timeout_s=0.3)` reports
   `EventLogLockTimeoutError` and the trail is byte-identical; after the native
   release the same child appends successfully, proving it is the same lock.
2. A real Python child holds `kernel.event_log_append.event_log_lock` → native
   `write_completed` **and** `write_started` fail with
   `Failed to …: … Error code: EVENT_LOG_LOCK_TIMEOUT.`, the trail is
   byte-identical, no new trail is created, and after the holder exits the
   native append succeeds.
3. A sibling native thread holding the family lock blocks another native
   writer; releasing it restores the append.
4. Holding the family lock does **not** block a spine append (the spine is
   path-anchored), pinning the reference's two anchoring choices.

Git tests run real `git` in isolated temporary repositories (scratch HOME,
`GIT_CONFIG_NOSYSTEM=1`, no global config): linked worktree ownership, root
checkout ownership, another repository, outside any checkout, a relative
`gitdir:` pointer, locator precedence and cross-checkout lookup, invalid ids,
and the no-Git fallback.

## 8. Exact failures observed (and what they were)

Initial state, before any code:

- `cargo check -p sk-invocation --offline` → pass (skeleton).
- `cargo test -p sk-invocation --offline` → pass, **0 tests** (no behaviour).

During implementation (all real, none waived):

1. Compile error `E0603` — `RecordParseError` imported through the private
   `record.rs` re-import instead of `error.rs`. Module wiring only; fixed.
2. Dead-code warning for an unused `started_fields` helper; the helper and its
   `#[allow(clippy::too_many_arguments)]` were removed rather than carried.
3. `record::tests::id_boundary_is_the_regex_not_ulids_decoder` **failed**: I had
   claimed `as_ulid()` would be `None` for 26 `Z`s, but `ulid` 1.2.1 accepts the
   spelling and truncates the overflow bits. The **API was the defect** (a
   lossy conversion with no consumer); `as_ulid` was deleted and the test now
   pins the decoder's leniency as the reason it cannot be the boundary.
4. `record::tests::lax_boolean_coercion_matches_pydantic` failed twice: my
   expectation table wrongly listed `0.5`/`2` as accepted and then wrongly
   listed the padded string `" true "` as accepted. The oracle probe
   (`/tmp/sk-inv-probe/probe2.out`) is the authority; the table was corrected
   (padded text, non-0/1 numbers, `null` and containers are refused).
5. `writer_filesystem::an_embedded_started_id_mismatch_is_refused` failed on
   my expectation, not the code: the reference uses `{…!r}`, so the message
   quotes both ids (`requested='…', embedded='…'`). Expectation corrected.
6. `op_custody_roots::locate_finds_a_record_held_by_another_checkout` failed in
   its own cleanup fixture (`remove_file` of a file the branch never created).
   Split into two independent fixtures.

Not observed: any oracle byte/validation mismatch (the serializer matched the
oracle on its first execution), any cross-process lock mismatch, any flaky
lock test.

## 9. Declared divergences (with reason; each has a test or a note)

- **D1** `write_started` returns `StartedWrite { path, index_warning }` rather
  than a bare `PathBuf` (plan §3.1). The crate has no logging dependency and
  the reference logs a WARNING on a refused index; the warning must be
  observable instead of printed. Covered by
  `a_refused_index_append_is_a_warning_only`.
- **D2** The created trail is `fsync`ed before the start is acknowledged
  (Python closes a buffered handle). Deliberate durability, matching plan §6.1
  ("exclusive-create started (durable before returning)"); bytes are
  unaffected.
- **D3** Validation-failure *detail* inside `LegacyRecordError` is native
  (`"<field>: <constraint>"`) instead of Pydantic's multi-line
  `ValidationError` dump. The error type, the legacy trigger conditions, and
  the outer template (`Legacy Op record[ <id>]: <reason>. Run 'spec-kitty
  upgrade' …`) are preserved. Exact residual: the detail text (Pydantic's
  per-error lines, its final "or", and its value/type hints) is not
  reproduced; it is operator detail inside a typed error, not wire text.
- **D4** Kernel-owned refusal classes are rendered from the kernel's typed
  error into the pinned interim-protocol wording (torn tail, symlink, lock
  timeout, lock unavailable). Other kernel failures (`TooLarge`, `ShortWrite`,
  `ReadbackMismatch`, `AppendRollbackFailed`, generic I/O) use the kernel's own
  display text, so their detail text differs from Python's exception text; the
  writer's own wrapper (`Failed to append completed event: …`) is preserved.
- **D5** **Resolved in remediation (F1).** The Op append precondition reads
  the whole owned trail (`max_precondition_bytes = u64::MAX`) because the
  reference reads the whole record and this writer created it; the generic
  kernel default (8 MiB) is unchanged for every other consumer. Covered by the
  coordinator's 9 MiB close/read regression and by
  `an_over_eight_mebibyte_record_closes_through_both_writers` (native start →
  reference close, reference start → native close, byte-equal trails).
- **D6** `invocation_path` on a missing `kitty-ops` directory validates the id
  and returns the lexical path (Python resolves a nonexistent root and accepts
  it too). If `kitty-ops` exists but is not a directory, this port refuses with
  the `outside trusted roots` wording where Python's containment check passes
  and the later create fails; creation failure itself is
  `InvocationError::Write` where Python lets `_ensure_dir`'s raw `OSError`
  escape.
- **D7** Non-object JSON rows: `check_existing` treats them as unreadable and
  `parse_op_event` returns `NotLifecycle`, where Python raises `AttributeError`
  (`.get` on a non-dict) for both. Not exercised by the oracle matrix.
- **D8** `read_op_closures` returns a typed error when the spine itself is
  unreadable (Python raises `OSError`); per-line tolerance (malformed, blank,
  started lines skipped) matches.
- **D9** **Corrected in remediation (F4).** `read_started` and
  `read_started_mode` read **at this writer's record root only**; they are not
  the composed claim reader. Their message text comes from the reference writer
  (`Invocation record not found: <id>`) and the CLI claim reader
  (`Op record not found for invocation_id='<id>'`, `First Op record is not a
  started event …`, identity mismatch with `repr`). The reference claim reader
  first composes the root (`locate_op_record_root(repo_root, id, preferred =
  resolve_op_record_root(repo_root)) or repo_root`) and then reads; a record
  opened in a linked worktree is therefore *missing* from the wrong root here.
  The stage-2 claim reader must compose the locator explicitly; the method doc
  comments say so and no implicit redirect is invented. `read_started_mode`
  requires an object first row (F2), so malformation is never absence; an
  unparseable first line maps to `Invalid invocation record: <id>` where Python
  would surface `JSONDecodeError` (detail-text residual).
- **D10** JSON text with a lone surrogate escape cannot be read: `serde_json`
  rejects it while Python's `json.loads` can hold a lone surrogate in a `str`.
  Rust `String` cannot represent one; this is a reader limitation, not a
  writer divergence.
- **D11** **Resolved in remediation (F5).** The duplicate `src/resolve.rs` is
  deleted. Path identity (`normalise_ref`, custody `resolve_or_keep`) uses the
  new shared kernel authority `sk_kernel::reference_path::
  resolve_reference_path`, whose documented profile is Python's **non-strict**
  `resolve_rejecting_loops`: missing/dangling/unreadable (`EACCES`) components
  carried lexically, `..` applied after expansion, real cycle detection only,
  **no total-link cap** (41/42/200-link acyclic chains resolve), interior NUL
  refused, finite repeated links memoised. That profile is deliberately
  **distinct** from the containment/lock policy (`resolve_within` /
  `fs_resolve::resolve_non_strict`: 40-link budget, non-`NotFound` errors
  propagate), whose behaviour is unchanged. The old copy was *not* an
  equivalent of the strict kernel resolver (link budget, error tolerance), so
  exporting that resolver unchanged was never the fix. Evidence:
  `tests/reference_path_oracle.rs` compares both `normalise_ref` and the raw
  kernel API against the pinned Python module for long chains, loops, EACCES,
  dangling/missing, `..`, repeated reuse, absolute/relative/URL and NUL.
- **D12** `python_repr` (used only in operator messages) keeps printable
  non-ASCII verbatim and escapes controls. Exact residual: Python's
  `str.isprintable()` is Unicode-category driven, so NBSP-class and other
  format/space code points are escaped there and kept here; ids, branches and
  paths in practice are unaffected. F7 also fixed the source-backed float
  details: timeout seconds and JSON-number reprs now use CPython's float repr
  (`1e-07`, `1e+20`).
- **D13** WP05 lifecycle records (`kitty-ops/lifecycle/<ulid>.json`, the legacy
  `lifecycle.jsonl` reader, pairing/orphan helpers) are **not** ported; they
  are plan §10 P12 / §11 stage 5, not stage 1. No claim.
- **D14** `OpWriteRefused` is a value you can construct; nothing in this crate
  raises it. Admission ordering (probe → mint → started) remains stage 2.
- **D15** Custody's `git worktree list` probe captures Git's stdout in a
  temporary regular file and polls `try_wait` against the same 30 s bound as
  the reference (F3). That is an implementation choice, not a divergence: it
  drains every listing size, keeps the reference's `None` degradation for
  spawn/non-zero/timeout failure, and reaps the direct child on every path.
  Exact residual: the timeout still abandons a listing that takes longer than
  30 s (as the reference's `subprocess` timeout does); no output cap is added.
  Related detail residuals, all refusal-only: `append_op_closure` when
  `kitty-ops` is a regular file wraps the `mkdir` failure as
  `Failed to write Op closure: …` where Python raises a raw
  `FileExistsError`; `read_op_closures` on an unreadable spine is typed where
  Python raises `OSError` (D8).

## 10. Remaining stage-2 prerequisites (explicit)

1. **Admission decision**: `AdmissionFacts`/`op_write_refusal` needs
   `sk-workspace::protection::ProtectionPolicy` (protected-branch set,
   `scoped_to_mission`, fail-closed resolve), the union of project-root and
   record-root policies, a bounded `current branch` read, and the
   `sk-git` skip probes that do not exist yet — `git check-ignore --no-index`
   and "does the primary branch track `kitty-ops/<ULID>.jsonl`" (plan P8).
   `OpWriteRefused` raising sites then go around `invoke`/`close`.
2. **Executor**: `_resolve_routing_and_context` (explicit hint → catalog
   resolve; empty-charter generic fallback → router), governance context
   text/hash (P3; until then the hash is native-text and must be declared),
   glossary chokepoint bundle + `glossary_checked` line (P4) — the bundle type
   is stage 2, so `write_glossary_observation` is not implemented here —
   model routing (P2, advisory), `invoke`/`dry_run` effect order, and
   `complete_invocation`'s evidence-mode gate, `AlreadyClosed` via
   `closed_ids`, evidence promotion, artifact/commit links and the propagator
   (P6 omitted).
3. **Catalog**: activation-gated merged profile catalog, legacy flat
   `.kittify/profiles/*.agent.yaml` overlay (P5), language scope (N5), profile
   resolution/role fallback.
4. **Executor auto-commit**: `OpCommitOutcome`, `CommitPort` over
   `sk-git::{SafeCommitRequest, evaluate_commit_guard}` with capability
   `STANDARD`, exact message `op(<profile_id>): <action> [<first 8>]`, skip
   rules first, union protection preflight, and `last_op_commit`; failures are
   non-fatal for the invocation.
5. **Root placement wiring**: call `resolve_op_record_root` /
   `locate_op_record_root` from the executor **and explicitly from the claim
   reader** (compose `preferred = resolve_op_record_root(repo_root)` then
   `locate_op_record_root(...) or repo_root`, then read at that root — D9; the
   current `read_started*` methods only read this writer's root); add the
   owned-checkout / owned record root option (P7) rather than faking `owned`.
6. **WP05 lifecycle + `MissionNextInvoked`** (P12/A15) and the `next`
   composition seam (§8): not started here.
7. **Optional tidy**: move the protocol-message rendering into a shared
   authority if a second writer ever needs it (today it is the only one). The
   kernel resolver export requested by the old D11 is done
   (`sk_kernel::reference_path`, D11).

## 11. Provisional record, retained and corrected

The provisional report (written before coding) listed the same scope, source
hashes and plan corrections; three claims changed as the code met the source:

- `InvocationId::as_ulid()` was planned as a documented "None outside the
  128-bit range" accessor. It does not exist: `ulid` 1.2.1 is lenient and
  lossy, so the accessor was removed rather than left as a misleading façade.
- The provisional divergence list said "index warning carried by the return
  value" as a candidate; it is now the implemented `StartedWrite`.
- The provisional text said the failure-detail divergence was "candidate";
  D3/D4/D5 are now the confirmed, tested list.

No capability, dispatcher, manifest, lockfile, aggregate or release gate was
touched, and this report is not an approval of the stage-1 code or of the
invocation feature.

### 11.1 Remediation round (F1–F7)

The independent review's F1–F5 defects and F6/F7 report/detail findings are
addressed; `REMEDIATION-REPORT.md` holds the red-first failures, the exact
commands, the final hashes and the residual scope. This section keeps the
stage-1 narrative; it is not a re-review and does not promote the crate.
