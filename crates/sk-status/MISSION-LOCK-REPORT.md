# Mission-write lock protocol — implementation report

> Original implementation report; counts below are historical. Independent review
> and subsequent corrections are recorded in
> `docs/rust/reviews/mission-lock-disposition.md`.

Date: 2026-11-02  
Checkpoint: `ab3e0fcc8`  
Python behavior baseline: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`  
Scope: shared Mission-write lock protocol in `sk-status`, and authoring lock
integration in `sk-mission`. Other writers, the CLI, manifests and the lockfile
were not edited.

## 1. Baseline protocol this implements

The reference is not just the old lock filename. It is
`src/specify_cli/status/mission_write.py`, `status/locking.py`,
`workspace/root_resolver.py`, `missions/_read_path_resolver.py` and
`lanes/branch_naming.py`:

- **One key per Mission.** The key is the Mission directory name, or the
  coordination directory name `<slug>-<mid8>` when the canonical primary
  `meta.json` records `coordination_branch`. Resolution is the baseline cascade:
  `meta.mid8` → declared `mission_id[:8]` → the slug's eight-character Crockford
  tail. An exhausted legacy `NNN-` coordination identity uses the recorded
  `mission_slug`, falling back to the directory name only when absent; flattened
  or meta-less Missions use the directory name. A modern coordination Mission with an exhausted cascade is
  refused (`MissionLockKeyUnresolved`), never given an invented key.
- **Canonical primary metadata.** The key is read from the main checkout's
  `kitty-specs/<name>/meta.json`. A linked worktree's stale copy never decides,
  and a linked-worktree CWD still converges on the main checkout's Git common
  directory and lock file.
- **Common-dir lock file, holder sidecar.** The lock is
  `<git-common-dir>/spec-kitty-locks/<key>.status.lock`, with
  `<lock>.holder` written once per outermost acquire and cleared on the outermost
  release. `kernel/status.locking._git_common_dir`'s degrade chain is preserved:
  `.git` → `.kittify` → `.spec-kitty`, without minting a project marker.
- **Registered holds.** Per-thread `(real lock root, directory name) -> key`
  bookkeeping. A nested call for the same Mission returns the held key even if
  `meta.json` changed inside the hold. A coordination-named hold additionally
  registers the primary directory alias when the primary meta composes exactly
  that key, and a primary-named hold registers the coordination key slot.
- **Rekey extension.** `extend_mission_write_lock_to_moved_key` holds the key
  that `meta.json` records *now* while the enclosing hold is kept. It yields
  `false` when the key cannot be resolved or did not move, and `true` once the
  moved key's lock is additionally held.
- **Birth and migration knobs.** `birth=True` keys on the directory name because
  create has no `meta.json` yet. `fallback_to_dir_name=True` is the
  migration/repair carve-out; ordinary callers stay fail-closed.
- **Same-thread reentrancy.** One OS lock per resolved path; only the outermost
  release clears the holder sidecar; the OS guard is PID-fenced so a fork child
  inherits no authority.

The coordinator's earlier fixes are outside this diff and remain intact:
artifact/finalization rollback records intent before replacement, and
planning/topology repair refuses contradictory recorded topology. Lock
acquisition was replaced underneath those transactions; their write and rollback
code was not rewritten.

## 2. Native implementation

### `crates/sk-status/src/mutation_lock.rs`

- `mission_lock_key` / `mission_lock_key_with` implement the held-aware door and
  the `ignore_held` rekey read.
- `canonical_primary_meta`, `main_repo_root`, `is_worktree_gitdir`,
  `is_single_segment` implement the canonical-primary read-path leaf, including
  worktree-pointer following and the owned-linked-worktree fallback.
- `lock_name_from_meta`, `resolve_transaction_mid8`, `mid8_from_slug`,
  `is_legacy_numbered_slug`, `coordination_dir_name` implement the key cascade
  and its legacy/flattened carve-outs.
- `primary_alias` implements the coordination-name → primary-directory alias
  rule (`-<mid8>` suffix strip + canonical primary meta verification).
- `HoldRegistration` implements the baseline `registered_hold` `setdefault`
  save/restore semantics, including duplicate slots.
- `acquire_mission_lock` is now the full protocol door; it resolves the key,
  acquires the kernel `StatusLock`, and registers the primary name,
  coordination key slot and alias for the hold. `acquire_mission_lock_with`
  exposes `fallback_to_dir_name` and `birth`.
- `acquire_mission_birth_lock` is the create-only birth door.
- `extend_mission_lock_to_moved_key` returns a guard with `moved()` and
  `path()`, holding the moved key until drop, with held-key-first ordering.
- `MissionLockExtension` and `MissionLock` are `!Send`/`!Sync` through the
  kernel guard; registered holds are dropped before the OS lock.
- `mission_id[:8]` is taken by **characters** (`chars().take(8)`), never by an
  unchecked byte offset, so untrusted UTF-8 metadata cannot panic or corrupt
  the key.

The lock mechanics remain the existing kernel `StatusLock`: `.status.lock`
family path, `.holder` sidecar, PID-fenced truncation and same-thread
reentrancy. No second lock implementation was added.

### `crates/sk-status/src/mutation.rs`

Added the `ExtensionRequiresHold` refusal and re-exported the birth, options and
extension surfaces.

### `crates/sk-mission/src/write_lock.rs` (new)

Small authoring-policy adapter only: select the shared post-birth door or the
birth door with the baseline 10-second bound. It contains no lock mechanics.

### Authoring consumers

- `create.rs`: the retired
  `<root>/.kittify/spec-kitty-locks/mission-write.lock` is replaced by
  `acquire_mission_birth_lock(plan.spec_dir, plan.root)`. Validation, write
  transaction and rollback are unchanged.
- `artifacts.rs`: the same checkout-local lock is replaced by the keyed
  `lock_mission(self.mission_dir, root)`.
- `tasks.rs`: the same checkout-local lock is replaced by the keyed
  `lock_mission(tasks_dir.parent(), project_root)`. The existing compute/apply
  split and rollback are unchanged.

No other writer module or CLI file was edited.

## 3. Evidence

### New protocol tests — `crates/sk-status/tests/mission_write_lock.rs`

14 tests, all passing:

- primary and coordination directories take one lock file;
- linked-worktree CWD (real `git worktree`), main-checkout CWD and coordination
  directory converge on the same `<coord>.status.lock`, even with a stale lane
  `meta.json`;
- nested use through the coordination directory reuses the primary alias after
  the recorded identity is flattened inside the hold;
- nested use reuses the held key after `meta.json` changes, while
  `ignore_held` returns the on-disk key;
- a coordination-directory writer is excluded by a primary-directory hold
  (bounded timeout, `.holder` text names the PID);
- modern unresolved coordination key is refused; `fallback_to_dir_name`
  succeeds;
- birth lock keys on the directory name, not the post-birth coordination key,
  and the registered hold answers with the birth key inside the hold;
- legacy `042-legacy` uses the bare directory key;
- untrusted Unicode `mission_id` is sliced by characters (no byte-offset
  panic);
- rekey extension holds the moved key until drop and leaves the old key held;
- extension reports no move, requires an enclosing hold, and times out without
  releasing the enclosing key;
- **cross-language contention both ways** with
  `.venv-rust-oracle/bin/python`, `PYTHONPATH=src`,
  `SPEC_KITTY_PACKS_ROOT=$PWD/packs`:
  - native holder → Python `mission_write_lock(timeout=0.2)` raises
    `FeatureStatusLockTimeoutError`;
  - Python holder → native bounded acquire returns
    `KernelError::StatusLockTimeout` naming the Python holder, and succeeds
    after release.

The oracle process runs with `PYTHONDONTWRITEBYTECODE=1` and temporary
`HOME`/XDG roots under the fixture, so it cannot write bytecode or user state
into the checkout/home.

### Authoring integration tests — `crates/sk-mission/tests/write_lock.rs`

3 tests, all passing. Each holds the shared key on another thread (same-thread
takes are reentrant by contract) and proves create, artifact authoring and
dependency finalization refuse with their typed `Lock` error and write nothing.
Each also asserts the retired `.kittify/spec-kitty-locks/mission-write.lock`
does not exist.

### Commands run

```text
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p sk-status -p sk-mission
  -> 294 passed, 0 failed, 1 ignored (pre-existing subprocess child)

CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 \
  cargo clippy -p sk-status -p sk-mission --all-targets -- -D warnings
  -> passed

rustfmt --edition 2024 --check <changed files>
  -> passed

CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo check -p sk-cli
  -> passed (API compatibility with the frozen CLI crate)
```

A manual pinned-oracle key check reproduced the native results:

```text
060-test      -> 060-test-01COORD0
042-legacy    -> 042-legacy
unicode-demo  -> unicode-demo-éééééééé
modern        -> REFUSED MISSION_LOCK_KEY_UNRESOLVED
coord dir     -> 060-test-01COORD0
```

All fixtures are temporary directories. No commit, push, manifest edit, extra
Cargo target directory, aggregate workspace run or release build was performed.

## 4. Remaining gaps and limits

This is not a blanket full-parity claim.

- **Rollback is not reimplemented here.** The Rust status crate still has its
  own append/rollback surfaces; this task ported lock authority, not
  `capture_rollback_point` / `rollback_events_log` semantics. The coordinator's
  intent-before-write and routing-repair refusal code was left untouched and its
  existing tests still pass.
- **Bookkeeping transaction is not a native consumer yet.** The Python
  `BookkeepingTransaction` takes the same key through `transaction_lock_key`.
  No Rust equivalent was added or changed; only the shared door it must use is
  now available.
- **`mission_write_lock_dir` / broader alias helpers** from
  `_read_path_resolver` are not ported. The lock-relevant alias rule is
  implemented; other read-path aliases remain pending.
- **Non-ASCII numeric prefixes.** The baseline legacy carve-out matches Python
  regex `^\d{3}-`, whose `\d` is Unicode-aware. The native check is ASCII
  digits, which is conservative for real safe-segment slugs but can differ for
  crafted non-ASCII metadata. Modern coordination Missions still fail closed.
- **Windows/other platforms.** The code is portable, but only Linux was
  exercised. Fork/PID-fence and cross-process `.holder` behaviour on non-Unix
  platforms remain review obligations.
- **Independent review completed after this report.** The lock and authoring
  mechanics were accepted conditionally; the recorded-slug and oversized-meta
  findings now have coordinator fixes and additional permanent regressions.
  This does not certify verified log rollback, rekey consumers or platform parity.
- **Aggregate gates** (workspace tests, release build, benchmark, full CLI
  lifecycle) were intentionally not run under the shared-disk/job limits, so the
  completion-loop and performance gates remain open.
