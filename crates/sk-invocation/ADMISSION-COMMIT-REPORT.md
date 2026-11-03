# Native Op write admission and best-effort record/spine commit — FINAL

Status: **FINAL owner snapshot before coordinator corrections.** Later integration
changes, source bindings and gates are recorded in
`ADMISSION-COMMIT-INTEGRATION-REPORT.md`; the owner counts/hashes below are historical.
This is not an
independent review, aggregate gate, release gate, whole-command acceptance, or
performance claim.

Baseline: published `42994b2c226bfcfb87e3e682c608a2777179153a` (native Op
commit-skip probes, long-ref correction). Source reference
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`
(`specify_cli/invocation/executor.py:77-108,1027-1213`,
`op_commit_policy.py`, `op_custody.py`, `git/commit_helpers.py`
`preflight_commit`/`safe_commit`/`protection_policy.py`). No source, fixture,
oracle, pin, manifest, or lock change was made by this owner. The coordinator's
pre-existing local `sk-invocation -> sk-workspace` edge is the only dependency
input; this owner did not edit `Cargo.toml` or `Cargo.lock`.

## Implemented slice

Two new focused modules, re-exported from `lib.rs`:

* `crates/sk-invocation/src/op_admission.rs` —
  `OpCommitService::op_write_refusal` and
  `OpCommitService::require_op_write_admitted`.
* `crates/sk-invocation/src/op_commit.rs` — the repository-bound
  `OpCommitService`, the local first-object started-record reader, the shared safe-commit
  request builder, and `OpCommitService::commit_op_record`.

`OpCommitService` owns its `InvocationWriter`, its opened `GitRepository`
(boxed; captured operator-index authority, git program and 30-second timeout
stay sticky), and the source-lifetime primary-tracking cache. It declares no
unsafe code and no self-reference.

### Admission

The order is the source's:

1. No current branch (plain directory, bare repository, unreadable HEAD) admits;
   a repository probe that could not run is surfaced as
   `OpAdmissionError::RepositoryProbe`.
2. The accepted #6410 skip decision runs next (`probe_primary_tracking` then
   `op_commit_skip_reason`, with an explicit per-service cache). Ignored exact
   path, primary no-Ops, unresolved primary, or an undecidable probe admits.
3. Only the non-skipped path reaches protection. The project-root and
   record-checkout `ProtectionPolicy` values are resolved with the canonical
   hatch and unioned (`is_protected`). The union is projected onto the current
   destination branch for `SafeCommitRequest` with
   `CommitGuardCapability::Standard`; irrelevant protected-name syntax is never
   re-parsed into a new config refusal.
4. The same `build_safe_commit_request` builder used by the real commit feeds
   the ULID-shaped sentinel `kitty-ops/00000000000000000000000000.jsonl` into
   `GitRepository::plan_safe_commit`. The plan is read-only.
5. `SafeCommitProtectedBranch` becomes `OP_WRITE_PROTECTED_BRANCH` with the
   source's `protected branch <repr>` reason and no override advice. A malformed
   protection block becomes `OP_WRITE_PROTECTION_CONFIG_INVALID`, but only when
   the source would have reached protection (an empty-set read-only probe
   decides the earlier path/HEAD/destination failures; adapter-only
   `OperationInProgress`/`RepositoryEnvironmentRefused` are treated as reached
   protection). Every other plan failure admits and is observable after the
   close, matching the source's logged `SafeCommitError` branch.

Admission creates no `kitty-ops/` directory, index file, record, ref, or
worktree change. `require_op_write_admitted` reuses the existing
`OpWriteRefused`/`OpWriteAction` vocabulary and "Nothing was written" message.

### Actual commit

`commit_op_record` first reads the first JSON **object** under the writer's
record root exactly as `executor._read_started_event` does: Python
`str.splitlines()` boundaries, a raw/legacy object is accepted, and
missing/empty/non-object/bad-JSON rows are `Invalid invocation record: <id>`.
It deliberately does not use the stricter `InvocationWriter::read_started`
CLAIM/MODEL reader. `profile_id`/`action` use `str(value or "unknown")`
semantics for the tested scalar/default cases. Collections reuse the native
Python-repr formatter, but native map keys are sorted rather than insertion
ordered; this and nonfinite JSON acceptance remain legacy-input differences.

It then:

* commits **only** `kitty-ops/<ULID>.jsonl` for an agent close, or **only**
  `kitty-ops/op-closures.jsonl` for a doctor sweep, never a historical archive
  row;
* emits exactly `op(<profile>): <action> [first8]`, with ` (doctor sweep)`
  appended for the sweep;
* follows the source's detached/skip/no-worktree ordering;
* re-judges the branch and protection at the close (the primary-tracking cache
  is the only lifetime-cached answer);
* calls the real `GitRepository::safe_commit` with the captured operator index,
  real hooks, real index reconciliation, and real ref advance;
* returns `committed`/`refusal`/`skipped` plus typed `OpCommitEvidence`
  (`Committed` with OID/destination/changed paths/warnings, or
  `SafeCommitLanded` with the landed OID and affected paths). The last outcome
  is also retained on the service, matching `last_op_commit`.

No audit row is written, deleted, truncated, rolled back, or rewritten by this
module. A refusal after a landed-but-uncertain commit retains the row and the
typed landed OID. A genuine no-op (`SafeCommitNothingToCommit`) is a refusal,
never a fabricated success.

## Tests and first failures

New outside-in suite: `crates/sk-invocation/tests/op_admission_commit.rs`
(21 tests: 20 executed, 1 ignored child helper exercised by the parent). It
uses real temporary repositories and the live pinned source method surface
(`ProfileInvocationExecutor.op_write_refusal` / `_commit_op_record`, real
writer/Git/guard), not a fake executor or monkeypatched guard.

Covered:

* protected + positive primary tracking with a full no-write snapshot
  (HEAD/status/index/refs/record bytes unchanged) plus live-oracle refusal;
* ignored admission sentinel admits, while the real record id is still judged
  and refused; live-oracle close refusal;
* primary no-Ops/unresolved → admitted local write and skipped close;
* malformed protection with and without the earlier skip; malformed protection
  after an earlier source path failure; irrelevant protected-name syntax;
* project/record protection union, explicit-empty record override still losing
  to the project policy, reverse union, both-empty admission, and a real close
  in a linked worktree using that checkout's root/common dir while the project
  checkout stays put;
* operator hatch via explicit seam (`1`, `0`) with matching live-oracle
  admission; no global environment mutation;
* detached, plain non-Git, bare, and unborn symbolic roots with source
  outcomes;
* branch switch and configuration switch between admission and close;
* agent per-record commit, exact message, path scope, typed OID, retained
  record bytes, repeat-close no-op; doctor-sweep spine-only commit with archive
  bytes preserved;
* raw/legacy/default/malformed started rows (`unknown`, `True`, numeric text,
  invalid row bytes retained);
* failing pre-commit hook (row retained, no commit), landed hook extra with
  `SafeCommitLanded` evidence and retained commit/row, failing post-commit hook
  as a warning on a confirmed commit;
* dirty/partially staged unrelated content preserved; captured alternate
  `GIT_INDEX_FILE` child process reconciled only for the record, default index
  untouched;
* native ordinary-route unsigned commit under `commit.gpgsign=true`. The owner's
  original source-signing-disparity statement was incorrect: pinned
  `git/commit_helpers.py:1017` explicitly sets `commit.gpgsign=false` too.
  Coordinator adds a live source control in the later integration report;
* 30-second default Git options and the ULID-shaped admission sentinel.

Ordering note: the provisional report/plan was written before implementation;
the outside-in suite was then authored against the source contract. The owner
does not claim a per-behavior tests-first red run. The first executable reds are
preserved below.

Preserved first failures (not hidden):

1. First `cargo check`: duplicate inherent `committed`/`skipped` method names
   and a borrow of `self.primary_tracking` through `open_repository`; fixed by
   renaming constructors and borrowing `self.binding` directly.
2. First strict Clippy: `large_enum_variant` on `RepositoryBinding`; fixed by
   boxing the Git handle.
3. First test run: 16 passed, 2 failed. `oracle_method_surface_is_real...`
   raised `NameError: __file__` (test-harness mistake; replaced with the real
   `kernel.resolution.__file__` probe). The unborn case used a `main` fixture
   with an existing commit, so the source correctly answered "the primary
   branch (main) tracks no Op records"; the fixture was changed to `trunk` to
   produce the intended unresolved ref. Both were test faults, not production
   defects.

## Scoped gates

All Cargo invocations used `bash /tmp/sk-focused-cargo.sh` (whole-command
lease, `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, existing repo target,
isolated HOME/XDG/TMP):

* `test -p sk-invocation` → **136 passed / 0 failed / 2 ignored**
  (34 lib + 4 lock, 20+1 ignored new integration, 30+1 ignored pre-existing
  helper, 15 custody, 4 oracle vectors, 2 reference-path, 26 writer, 1 doctest).
  Log: `/tmp/sk-op-admission-commit-tests.log`. The ignored child helper is
  executed by its parent test; the other ignored test is the pre-existing
  `op_commit_policy` subprocess helper.
* `clippy -p sk-invocation --all-targets -- -D warnings` → pass.
  Log: `/tmp/sk-op-admission-commit-clippy.log`.
* `fmt -p sk-invocation -- --check` → pass.
  Log: `/tmp/sk-op-admission-commit-fmt.log`.
* `check -p sk-invocation --all-targets --locked --offline` → pass, confirming
  the coordinator-prepared dependency edge has no lock drift.
  Log: `/tmp/sk-op-admission-commit-locked-check.log`.
* Oracle: exact `.venv-rust-oracle/bin/python3.11`, Python 3.11.15, verified
  lock-exact **53/53** checked runtime requirements, zero drift/missing/
  duplicates/unsatisfied. Isolated probe root
  `/tmp/sk-op-admission-oracle.ZxIZT0`; log
  `/tmp/sk-op-admission-oracle.log`.

No aggregate, release, full-workspace, foreign-crate fmt, cargo clean, cache/
evidence deletion, global env/config mutation, process-group signal, polling,
or manual source-repo checkout was used.

## Historical owner-snapshot source hashes

```
7da03b5e91b6630775255ad459770961c64b64d452956a3126eb24c7d484f6b7  crates/sk-invocation/src/op_admission.rs
281d14934b477a8ca70bc12ff0752081c3ff2a61875372a19c92dd80d33e70d3  crates/sk-invocation/src/op_commit.rs
c00bb8a9b0637d3c953e4e7f17fc79107c2632ed008b1f118efb40ebde9d9309  crates/sk-invocation/tests/op_admission_commit.rs
fcf04e83cf2f1a2d368b7bea212edbb8fe2c75f9201807b0ffe36c79261f46ec  crates/sk-invocation/src/lib.rs
```

The report's own hash is intentionally not self-referential.

## Remaining gaps / claims not made

* `_current_branch`'s `git rev-parse --abbrev-ref HEAD` fallback is not
  reproduced; the native symbolic answer is used. Detached HEAD agrees.
* Generic Git failures use native typed/Display text, never a fabricated Python
  `repr(Exception)`. A non-UTF-8 record is a generic read failure (source's
  `UnicodeDecodeError` escapes its caught set). `serde_json` also rejects bare
  `NaN`/`Infinity` that Python's `json.loads` accepts. Native JSON map ordering
  also differs from insertion-ordered Python repr for nested object-valued raw
  `profile_id`/`action`. Disclosure does not make these valid-source inputs
  compatible.
* The `sk-workspace` protection reader's own remote-default probe keeps that
  crate's accepted options; the explicit 30-second budget applies to the
  `OpCommitService` handle.
* The repository is bound at service construction. A record root that only
  becomes a repository later is not re-probed (source probes per call); normal
  composition constructs the service for one command.
* Native `plan_safe_commit` has adapter post-guard checks
  (`OperationInProgress`, `RepositoryEnvironmentRefused`) that the source
  preflight lacks. Admission still admits on those in the normal case (the
  close reports them), and the malformed-config ordering probe treats them as
  reached protection.
* Correction: ordinary commits are intentionally unsigned on **both** source
  and native routes. This is not a signing-parity gap. The later coordinator
  control uses `commit.gpgsign=true` plus an unusable signer on real repos for
  both implementations.
* Cross-checkout CLAIM composition is **not** implemented; the started-record
  read remains local to the writer's record root, like the source helper.
* No full `ProfileInvocationExecutor`, dispatcher, StepContract, `next`/CLI
  wiring, protected-main false-advance repair, whole-command parity, inventory
  promotion, platform acceptance, or performance claim is made.
