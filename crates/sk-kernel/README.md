# sk-kernel

Foundation primitives for the Rust Spec Kitty implementation. This crate is
the kernel floor of the workspace: it does not depend on any other Spec Kitty
crate, it forbids unsafe Rust through the workspace lint, and it adds no
third-party dependency beyond the shared workspace set.

The reference implementation during migration is upstream
`spec-kitty/spec-kitty` commit `9aed3a8012cb5eb22c0128b14d525039f95fea7a`
(2026-11-02). Existing Python sources remain the development oracle; the Rust
kernel never invokes Python at runtime.

## Public contract

Exported at the crate root:

```rust
MissionSlug, WpId
discover_project_root(start: &Path) -> Result<PathBuf, KernelError>
resolve_within(root: &Path, relative: &Path) -> Result<PathBuf, KernelError>
read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, KernelError>
atomic_write(path: &Path, bytes: &[u8]) -> Result<bool, KernelError>
atomic_write_with_options(path, bytes, AtomicWriteOptions) -> Result<bool, KernelError>
substantively_equal(existing, candidate) -> bool
substantively_equal_projected(existing, candidate, strip) -> Result<bool, KernelError>

// Frozen consumer contract:
FileLock
impl FileLock {
    pub fn acquire(path: &Path, timeout: Duration) -> Result<Self, KernelError>;
    pub fn path(&self) -> &Path;
    pub fn holder(&self) -> &LockHolder;
}
LockHolder::read(path) -> Result<Option<Self>, KernelError>
LockHolder::read_diagnostics(path) -> Result<Option<Self>, KernelError>
anchored_lock_path(anchor, common_dir, subdirectory) -> Result<PathBuf, KernelError>
anchored_lock_path_with(anchor, common_dir, subdirectory, &AnchoredLockName) -> Result<PathBuf, KernelError>

// Per-family policies and names:
LockWait::{NonBlocking, Bounded(Duration), Unbounded}
LockOptions { wait, reentrant }
impl FileLock {
    pub fn acquire_with(path: &Path, options: LockOptions) -> Result<Self, KernelError>;
    pub fn try_acquire(path: &Path) -> Result<Option<Self>, KernelError>;
    pub fn acquire_unbounded(path: &Path) -> Result<Self, KernelError>;
}

StatusLock, StatusLockHolder
acquire_feature_status_lock, acquire_project_event_log_lock, acquire_checkout_claim_lock
feature_status_lock_path, project_event_log_lock_path, checkout_claim_lock_path

git_common_dir, git_toplevel, git_configured_worktree, discover_git_layout

get_kittify_home, get_runtime_state_root, ensure_runtime_state_root
user_cache_dir, user_state_dir
get_built_in_pack_root, resolve_built_in_pack_root(_from), get_packs_root_default
get_package_asset_root, resolve_installed_sibling
home_directory, render_runtime_path, to_posix, posix_tree_path, repo_tree_path
BUILT_IN, ORG, PROJECT, PACK_TIERS, pin_filename
```

Public modules:

```rust
sk_kernel::clock::{Clock, SystemClock, FrozenClock, now_*, parse_*, format_stamp, from_epoch}
sk_kernel::content_digest::{sha256_digest, sha256_hex, sha1_hex}
sk_kernel::guarded_read::{read_guarded, read_guarded_text, collapse_guarded,
                          read_bounded_no_follow}
sk_kernel::no_follow::{open_no_follow, read_no_follow, read_text_no_follow,
                       write_no_follow, write_text_no_follow, chmod_fd,
                       chmod_no_follow, utime_no_follow, file_mode,
                       fd_relative_dir_ops_supported}
sk_kernel::event_log_append::{
    EventLogAppendOptions, TornTailPolicy,
    append_event_log_lines, append_event_log_lines_guarded,
    event_log_lock_path, EVENT_LOG_LOCK_TIMEOUT,
}
sk_kernel::git_topology::{GitLayout, ...}
sk_kernel::lock_families::{StatusLock, StatusLockHolder, ...}
sk_kernel::locks::{LockWait, LockOptions, AnchoredLockName, AnchorDigestAlgorithm, ...}
sk_kernel::paths::..., sk_kernel::user_dirs::..., sk_kernel::pack_tiers::...
```

All failures are variants of `KernelError`. Production code contains no
`unsafe`, `unwrap`, `expect`, `panic!`, `todo!`, or `unimplemented!`.

## Identifier semantics

- `MissionSlug` ports `kernel`'s canonical safe-segment grammar,
  `^[A-Za-z0-9][A-Za-z0-9._-]*$`. It rejects empty and non-ASCII input,
  leading/trailing whitespace, a leading `.`, any `..` substring, path
  separators, NUL, and all other punctuation. Interior dots are allowed.
- `WpId` ports the status authority's `normalize_wp_id`
  (`specify_cli/status/models.py`): trim ASCII whitespace, upper-case ASCII,
  then require `WP` plus one or more ASCII digits.
- Both implement `Clone`, `Debug`, `Display`, `Eq`, `Hash`, `Ord`,
  `PartialEq`, `PartialOrd`, `FromStr`, `Serialize`, and `Deserialize`.
  Deserialization revalidates through `FromStr`.

## Path and I/O semantics

### `discover_project_root`

Project-root discovery remains the upstream `locate_project_root` contract,
not Git's `/`-discovery contract. `SPECIFY_REPO_ROOT` wins when it names an
existing directory. A linked-worktree `.git` file re-anchors at the primary
checkout when it has `.kittify`. A `.git` directory with `.kittify` or `HEAD`
is a boundary; an empty `.git` directory stops the walk. `.kittify` is the
non-Git fallback. Absolute `GIT_CEILING_DIRECTORIES` entries bound the walk;
the listed directory itself is examined (upstream's documented semantics,
which differ from Git's; see the Git-topology section for the Git contract).

### `resolve_within`

Rejects absolute paths, Windows drive spellings, UNC/rooted spellings, `..`,
and components that reintroduce an absolute root. Expands symlinks even for a
missing tail so a dangling link outside the root fails closed. A symlink loop
is `SymlinkLoop`. Returns the resolved candidate, not the caller's spelling.

### `read_bounded`

Requires a regular file. Unix opens use `O_NONBLOCK | O_NOCTTY` before
metadata inspection, so a FIFO cannot hang the read. Checks metadata length,
then reads at most `max_bytes + 1`. `max_bytes == 0` accepts only empty files.
Follows a final symlink; use `no_follow::read_no_follow` where the owning
boundary needs no-follow.

### `no_follow`

`open_no_follow` adds `O_NOFOLLOW` on Unix and owns the flag combination so a
caller cannot drop it. Non-Unix platforms do a best-effort pre-open check; the
residual race is explicit. `read_no_follow` and `read_text_no_follow` apply the
bounded, regular-file, nonblocking-on-Unix policy. `write_no_follow` /
`write_text_no_follow` write through the same no-follow door. `chmod_fd`,
`chmod_no_follow`, `utime_no_follow` and `file_mode` port the host-safe
symlink-aware helpers; Linux has no `lchmod`, so `chmod_no_follow` falls back
to the default-follow form exactly as upstream does. `utime_no_follow` asks
for `AT_SYMLINK_NOFOLLOW` and falls back only where the host rejects the flag.
`fd_relative_dir_ops_supported` is the capability gate for fd-relative
directory work (Unix only).

### `atomic_write`

- Reads the existing bytes and returns `Ok(false)` without touching the file
  when they are identical: no temporary, no inode replacement, no mtime change.
- Changed content goes to a temporary in the destination directory, is
  flushed and `sync_all`ed, renamed, and (Unix) followed by a parent-directory
  `sync_all`. A parent-sync failure returns `AtomicCommitIndeterminate`, never
  success. `KernelError::mutation_may_have_committed()` is true for it.
- `AtomicWriteOptions { create_parents }` is the additive `mkdir=True` form;
  `atomic_write` keeps the default “parent must exist” contract.
- `substantively_equal` is the pure byte comparison; the projected form lets a
  caller ignore volatile fields (the caller captures its keys in the closure)
  without the kernel assuming a serialization format.
- Existing regular-file permission bits are preserved before the rename. New
  files use the temporary default (`0600` on Unix). A destination symlink is
  followed for the equality read but the rename replaces the link node.
- FIFOs, devices, directories and symlinks to non-regular files are refused
  before a temporary is created. Concurrent writers are atomic
  last-writer-wins; serialised read-modify-write takes a `FileLock` first.

## Locking

### `FileLock` (frozen contract)

`FileLock::acquire(path, timeout)` remains an exclusive, cross-process,
dedicated-path lock backed by safe `fs2` (`flock(2)` / `LockFileEx`). It is
bounded by `timeout` (`Duration::ZERO` is one attempt, then `LockTimeout`) and
same-thread reentrant. Release truncates the holder record and keeps the inode.
A planted symlink, FIFO, device or directory is refused. New lock parents are
`0700` on Unix; an existing directory is never chmodded. Guards are statically
neither `Send` nor `Sync` and `#[must_use]`; an inherited fork guard cannot
reenter or truncate its parent's record.

### Explicit per-family policies

`LockOptions` names the two upstream knobs separately:

- `LockWait::NonBlocking` — one attempt; contention is `LockNotAcquired`.
  `FileLock::try_acquire` maps that to `Ok(None)`.
- `LockWait::Bounded(d)` — retry to the deadline; `Duration::ZERO` is one
  attempt and reports `LockTimeout`.
- `LockWait::Unbounded` — retry until acquired (`acquire_unbounded`).
- `LockOptions::reentrant()` / `non_reentrant()` — whether a same-thread
  acquire reuses the existing holding. `LockOptions::default()` is the
  upstream sync default (`NonBlocking`, non-reentrant); the frozen
  `FileLock::acquire` explicitly opts into bounded + reentrant.

### Anchored lock names

`anchored_lock_path` is unchanged: resolve the anchor once, digest its
resolved, case-normalised identity, place
`<common>/spec-kitty-locks[/<subdirectory>]/<digest[0..32]>.lock`. The
additive `anchored_lock_path_with` supports prefix, suffix, SHA-256 or SHA-1,
and digest length 1..=64. SHA-1 exists only for the Mission checkout-claim
filename discriminator; it is not an integrity digest. Prefix/suffix must be
plain filename fragments (separators are refused). Subdirectories must be
relative and traversal-free.

### Status, project-log and checkout-claim families

Upstream keeps these rules in `specify_cli.status.locking`; the kernel owns the
parts every writer must agree on:

- `feature_status_lock_path(common, "<mission-dir-name>")` →
  `<common>/spec-kitty-locks/<mission-dir-name>.status.lock`. Keyed by the
  mission **directory name**, never a bare slug. Empty, traversal-bearing and
  separator-bearing keys are refused.
- `project_event_log_lock_path(common)` → `.../__project__.status.lock`.
- `checkout_claim_lock_path(checkout, common)` →
  `.../__checkout-<sha1[0..16]>__.status.lock`, keyed on the resolved,
  case-normalised checkout.
- `StatusLock` acquires with the caller's `LockWait`, is reentrant per thread,
  writes the `<lock>.holder` sidecar only on the outermost acquire, clears it
  only on the outermost release, and maps a bounded timeout to
  `KernelError::StatusLockTimeout` with the rendered holder.
- `StatusLockHolder` round-trips the sidecar contract byte-for-byte:
  sorted-key JSON with `pid`, `thread`, `acquired_at`.
- `acquire_checkout_claim_lock` enforces the fixed order: **checkout claim
  first, then any Mission lock**. A foreign status lock already held by the
  thread is `LockOrderViolation` with the held keys, never a deadlock.
- `StatusLockHolder` is the timeout-diagnostic authority. `LockHolder` (the
  lock file's own record) is a separate diagnostic; on Windows a mandatory
  lock may make that range unreadable.

**The status owner still owns the whole-file status write.** The generic
event-log append helper is for interim append-only run/decision/retrospective
families only. `status.events.jsonl` and `canonical-events.jsonl` must not be
appended through `append_event_log_lines`; they need the status lock
authority, atomic whole-file batch replacement, verification and recovery
protocol (or the pinned external writer service).

### Event-log batch append

`event_log_append` is the interim locked writer for append-only JSONL logs,
ported from `kernel.event_log_append`:

1. The anchor is the log (or a one-file-per-record family directory). The lock
   is `<git-common-dir>/spec-kitty-locks/event-logs/<sha256-prefix>.lock`,
   discovered from the filesystem, **without running Git**. Outside a Git
   repository the lock is `<anchor>.event-log.lock`. A marker that claims HEAD
   but is not a complete repository refuses rather than moving the lock to a
   different authority.
2. One `FileLock` with a bounded timeout; same-thread reentrancy is allowed.
3. Torn tail policy is explicit per log: `Refuse` (default, writes nothing) or
   `TruncateToLastNewline` (cut, fsync, then append). Repair itself is not
   rolled back.
4. An optional precondition sees the post-repair bytes under the lock.
5. The batch is validated (non-empty, no embedded `\n`/`\r`) and written as
   one logical unit on an `O_APPEND` descriptor.
6. The unit is `sync_data`ed, read back from the pre-append offset, and
   compared byte-for-byte.
7. A short write, write/sync/readback error or mismatch truncates to the
   pre-append size and fsyncs. A failed rollback reports
   `AppendRollbackFailed` and `mutation_may_have_committed()` is true; the
   caller must apply its torn-tail/recovery policy.
8. A newly created log's parent directory is fsynced on Unix when
   `sync_new_log_directory` is true (default), so the empty file's directory
   entry is durable before the append is acknowledged. Windows cannot express
   a directory fsync through safe `std::fs`; that limit is explicit.

The module validates framing only, never row schema.

## Git topology discovery (no subprocess)

`git_topology` reproduces upstream's three probes without running Git:
`git_common_dir`, `git_toplevel`, and `git_configured_worktree`. It honours:

- `.git` directories and `.git` pointer files (`gitdir:`), including
  linked-worktree `commondir` and absorbed submodule git dirs;
- `GIT_DIR` (absolute, relative, or a gitfile), `GIT_COMMON_DIR`,
  `GIT_OBJECT_DIRECTORY`, `GIT_WORK_TREE`, `GIT_CEILING_DIRECTORIES`, and
  `GIT_DISCOVERY_ACROSS_FILESYSTEM`;
- bare repositories and paths inside a Git directory;
- an incomplete `.git` directory exactly as Git does: the parity probes ignore
  it and keep walking;
- `core.worktree` and `core.bare` from the local config (a bounded, no-subprocess
  parser for the keys this crate needs).

`git_common_dir` applies upstream's interior check: a path that is, or is
inside, the common dir is `NotAGitRepository`. `git_toplevel` reports
`GitTopologyUnavailable` for a bare/directly-entered git dir.

**Deliberate stricter lock behaviour.** `event_log_lock_path` uses
`strict_git_common_dir`, which refuses a marker that claims HEAD but is not a
complete repository (`LockUnavailable`) instead of ignoring it as Git would.
That keeps a durable writer from silently moving to a different lock
authority. The parity probes remain Git-equal; this one difference is tested
and documented rather than hidden.

## Concurrency and Windows diagnostics

- `FileLock` is advisory local-filesystem mutual exclusion, not a distributed
  lock.
- `LockHolder::read` is strict: on Windows a mandatory-locked range that
  another process cannot read is `LockHolderUnreadable`, never a false “no
  holder”. Operator diagnostics use `LockHolder::read_diagnostics`, which
  degrades to `None` for that platform limit. The status family's `.holder`
  sidecar remains the source of truth for status-lock diagnostics.
- `LockHolder::age_s` / `is_stuck` reproduce upstream's advisory helpers; they
  never break a stale lock.

## Home, user-dir and pack paths

- `get_kittify_home`: `SPEC_KITTY_HOME` (non-empty) else POSIX
  `~/.kittify` / Windows `%LOCALAPPDATA%\spec-kitty` (fallback
  `~/.spec-kitty`).
- `get_runtime_state_root`: same overrides, POSIX `~/.spec-kitty`;
  `ensure_runtime_state_root` is the only creating/re-hardening door.
- `user_cache_dir(windows_local_appdata)`: Windows `%LOCALAPPDATA%` else
  `~/AppData/Local`; otherwise `$XDG_CACHE_HOME` else `~/.cache`.
  `user_state_dir`: `$XDG_STATE_HOME` else literal `~/.cache`, on every
  platform.
- `pack_tiers` owns the single `built-in` spelling, rank-ordered tuple, and
  `<pack>.source.yaml` pin name.
- `resolve_installed_sibling` is the bounded ancestor walk with a recognised
  `src`/`site-packages`/`dist-packages` boundary (one hop past, or four
  ancestors without one), plus `*` segment matching.
- `get_built_in_pack_root` takes `SPEC_KITTY_PACKS_ROOT` when its `built-in`
  child exists, otherwise walks to the installed/checkout sibling and reports
  the ignored override through `BuiltInPackRoot::ignored_override` (no silent
  fallback).
- `get_package_asset_root` gives `SPEC_KITTY_PACKS_ROOT` precedence, then the
  accepted `SPEC_KITTY_TEMPLATE_ROOT` legacy shapes, then the installed
  `packs/built-in/missions` tree. It fails closed; there is no legacy
  `specify_cli/missions` fall-through.

## Clock and digests

- `clock` is the canonical wall-clock door: `SystemClock` is the only real
  boundary, `FrozenClock` is the test double, and `now_utc_iso`,
  `now_utc_seconds`, `now_utc_stamp`, `now_utc_compact_stamp`, `now_utc`,
  `now_epoch`, `parse_iso`, `parse_stamp`, `format_stamp`, `from_epoch` are
  distinct serialization contracts. `now_utc_iso` matches Python
  `datetime.isoformat()`; `from_epoch` rounds to Python's microsecond
  resolution so values round-trip.
- `content_digest::sha256_digest` returns `sha256:<hex>` over exact bytes;
  `sha256_hex`/`sha1_hex` return bare lower-case hex. SHA-1 is a lock-name
  discriminator only. No line-ending, whitespace or path normalisation is
  applied.

## Deliberate divergences from the Python baseline

These are safer or platform-forced differences, not accidental gaps:

1. **Strict lock topology.** A HEAD-bearing but incomplete `.git` marker
   refuses the event-log lock instead of moving it (Git/Python ignore the
   marker and may use a parent repository).
2. **New-log directory fsync.** Unix fsyncs the parent after creating a log;
   Python does not.
3. **Windows holder diagnostics.** The lock-file holder record is explicitly
   unreadable under a mandatory lock; the separate `.holder` sidecar is used
   for status diagnostics.
4. **SHA-1 implementation.** A small in-crate, safe SHA-1 reproduces
   `hashlib.sha1(..., usedforsecurity=False)` for the one stable lock filename;
   no new dependency is added.
5. **Lock-name validation.** Prefix/suffix and named status keys are rejected
   if they contain separators/traversal, where Python interpolates them
   directly. Valid names are byte-identical.
6. **`LockOptions::default()`** is non-blocking/non-reentrant (upstream sync
   default); `FileLock::acquire` remains bounded/reentrant by explicit choice.
7. **Home fallback.** If `HOME`/`USERPROFILE` is absent, Rust reports
   `HomeDirectoryUnavailable`; Python's `Path.home()` may consult the password
   database.
8. **Pack-root anchor.** The default walk anchors at the workspace manifest or
   running executable because a Rust binary has no importable module file; the
   resolved checkout tree is the same as Python's.
9. **Date-only `parse_iso`.** Rust normalises `YYYY-MM-DD` to UTC midnight;
   Python returns a naive datetime. Every aware ISO form (including `Z`,
   numeric offsets and space separators) matches Python's instant.
10. **Sidecar clear ordering.** The status guard clears its `.holder` sidecar
    before releasing the OS lock. Upstream releases first and then clears,
    which lets a newly-acquired holder's sidecar be erased by the old holder
    in the gap. Clearing first removes that race: the absent-sidecar window
    is before the unlock (no contender can hold the lock yet), not after it.

## Remaining required APIs and limits

This crate is not the complete kernel/write service. Known remaining work:

- **Status/canonical write protocol.** Whole-file batch replacement,
  verification, recovery, schema validation, ledger reduction and the external
  writer-service handover are owned by the status/events owners. This crate
  supplies their lock paths, waits, sidecar and holder contracts only.
- **Event-log schema.** The append primitive validates framing, not rows; the
  write service remains the validation authority.
- **Git config coverage.** `git_configured_worktree` parses `core.worktree`
  and `core.bare` from the local config only. `include.path`/`includeIf`,
  `extensions.worktreeConfig` (`config.worktree`), alternates, and exotic
  object stores are not resolved natively.
- **Windows.** Case-insensitive containment, non-Unicode lock-anchor identity,
  owner/ACL/xattr/hard-link/sparse preservation, directory fsync, and the
  mandatory-lock diagnostics path need execution on Windows CI; local Linux
  tests do not establish them.
- **Packaging layouts.** Installed wheel/binary pack-root placement and the
  macOS/Windows `platformdirs` branches need a packaged-install test.
- **Project discovery.** There is still no `stop` parameter/test-only walk
  ceiling beyond `GIT_CEILING_DIRECTORIES`; `discover_project_root` keeps
  upstream's project-root semantics, not Git's.
- **Write observer.** `substantively_equal` (including the caller-supplied
  projection) is ported, but the `observe_writes`/`notify_written` write
  observer used by the finalize write ledger is not.
- **Async lock facade.** Only the synchronous guard exists; upstream's async
  `MachineFileLock` facade has no Rust consumer yet.
- **Distributed/network-filesystem locks.** `flock` semantics on network
  filesystems are outside the guarantee. Holder metadata is diagnostic, never
  a stale-lock authority.
- **Event-log durability.** Disk-failure behaviour beyond `fsync` is not
  claimed; a torn-tail repair is intentionally not rolled back.
- **Parser error source.** Guarded parser errors are collapsed to `Display`
  text; the original typed error is not preserved as a `source`.

### Upstream `src/kernel` modules still outside this crate

These remain required for complete kernel-floor parity and are **not**
implemented here (some are owned by a higher crate; none is silently stubbed):

| Upstream module | Status / owner |
| --- | --- |
| `atomic.py` (write observers) | Core write and `substantively_equal` projection ported; `observe_writes`/`notify_written` pending |
| `_safe_re.py` (general safe-segment regex) | Identifier grammar ported only |
| `env_expand.py` | Pending (environment/token expansion) |
| `yaml_io.py`, `meta_decode.py`, `text_decode.py`, `schema_utils.py` | Pending (YAML/meta/text/schema boundaries) |
| `charter_pack_paths.py`, `resolution.py` | Pending; pack-root primitives landed, charter resolvers live above kernel |
| `service_pin.py`, `service_artifacts.py`, `service_hosts.py`, `release_assets.py`, `site_urls.py` | Pending (external service pin/asset/host contracts) |
| `activation_ledger_store.py` | Pending (ledger store) |
| `vcs_lock.py` | Pending (VCS-held lock abstraction) |
| `tree_removal.py` | Pending (safe tree removal) |
| `glossary_runner.py`, `glossary_types.py` | Owned by `sk-glossary`; not kernel |
| `mission_runtime_shape.py` | Pending runtime-shape validation |
| `git/` subpackage (blame, listing, paths, remote, ref advance, …) | Owned by `sk-git`; not kernel |
| `errors.py` | Represented by Rust `KernelError`; upstream codes/messages are not byte-identical |

The status/canonical writer protocol is deliberately **not** in this crate's
event-log helper. `append_event_log_lines` is only for the baseline's interim
run/decision/retrospective log families; `status.events.jsonl` and
`canonical-events.jsonl` require the status lock, whole-file atomic batch
replacement, verification and recovery (or the pinned external writer
service).

## Verification

From the workspace root, with bounded jobs on a shared VM:

```bash
CARGO_BUILD_JOBS=2 cargo test -p sk-kernel
CARGO_BUILD_JOBS=2 cargo clippy -p sk-kernel --all-targets -- -D warnings
cargo fmt -p sk-kernel -- --check
```

The suite contains **186 tests** (unit + integration + doctests). Differential
tests use the pinned interpreter
`/home/exedev/.local/share/uv/tools/spec-kitty-cli/bin/python` with
`PYTHONPATH=<repo>/src` (override with `SK_KERNEL_PYTHON`). They skip with a
printed note if the oracle is absent; Git-oracle legs still run.

Coverage highlights:

- `git_topology_parity`: 27 real layouts × `git rev-parse` × the pinned Python
  probe — plain, subdirectory, linked worktree, separate git dir, bare root and
  subdir, absorbed submodule, incomplete/malformed/broken markers, Git
  interiors, file input, `core.worktree`, and every supported Git environment
  override/ceiling combination.
- `lock_path_parity`: anchored SHA-256/SHA-1 names, prefix/suffix, 1/16/32/40/64
  digest lengths, event-log subdirectory, status/project/checkout names and
  the historical checkout SHA-1 formula, against Python.
- `cross_language`: Python holds → Rust contends and acquires after exit; Rust
  holds → Python contends; 50 mixed Python/Rust event-log rows stay whole and
  ordered; a killed Python holder releases the OS lock; the status sidecar
  round-trips in both directions.
- `home_paths_parity`: home, runtime state, cache/state XDG branches,
  `SPEC_KITTY_HOME`, `SPEC_KITTY_PACKS_ROOT` (valid, invalid and fallback),
  `SPEC_KITTY_TEMPLATE_ROOT`, pack tiers and assets against Python.
- `clock_parity`: every producer and parse/from-epoch contract under one frozen
  instant, byte-for-byte against Python.
- `lock_families`: non-blocking/bounded/unbounded waits, non-reentrant
  contention, sidecar once-per-outermost semantics, holder-naming timeout,
  checkout-before-Mission ordering, checkout reentrancy.
- `path_contracts`: no-follow text read/write and symlink refusal, chmod/utime
  helpers, atomic `create_parents` and no-op stability.
- `event_log`: torn-tail refuse/repair, precondition after repair, JSONL
  framing, short-write and readback rollback, failed-rollback
  `AppendRollbackFailed`/`mutation_may_have_committed`, worktree `commondir`
  placement, multi-process appends, lock timeout.
- `locks`: multi-process timeout/serialization, killed holder, fork/inherited
  guard refusal, non-LIFO reentrancy, planted symlink/FIFO refusal.
- `durability`/`io`/`special_files`/`containment`/`discovery_env`: atomic
  commit indeterminacy, bounded/no-follow reads, containment escape/loops,
  environment-sensitive project discovery.

Development probes under `/tmp/kprobe` (not committed) reproduced the prior
fork/topology hazards and were used while building the parity fixtures.
