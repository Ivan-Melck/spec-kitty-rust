# sk-kernel

Foundation primitives for the Rust Spec Kitty implementation. This crate is
the kernel floor of the workspace: it does not depend on any other Spec Kitty
crate, and it forbids unsafe Rust through the workspace lint.

The reference implementation during migration is upstream
`spec-kitty/spec-kitty` commit `9aed3a8012cb5eb22c0128b14d525039f95fea7a`
(2026-11-02). These are bounded ports of the named upstream behaviors, not a
claim of full Python-kernel parity.

## Public contract

Exported at the crate root:

```rust
MissionSlug, WpId
discover_project_root(start: &Path) -> Result<PathBuf, KernelError>
resolve_within(root: &Path, relative: &Path) -> Result<PathBuf, KernelError>
read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>, KernelError>
atomic_write(path: &Path, bytes: &[u8]) -> Result<bool, KernelError>
```

All failures are variants of `KernelError`. Production code contains no
`unsafe`, `unwrap`, `expect`, `panic!`, `todo!`, or `unimplemented!`.

## Identifier semantics

- `MissionSlug` ports `kernel`'s canonical safe-segment grammar,
  `^[A-Za-z0-9][A-Za-z0-9._-]*$`. It rejects empty and non-ASCII input,
  leading/trailing whitespace, a leading `.`, any `..` substring, path
  separators, NUL, and all other punctuation. Interior dots are allowed, as
  upstream's reconciled grammar allows.
- `WpId` ports the status authority's `normalize_wp_id`
  (`specify_cli/status/models.py`): trim ASCII whitespace, upper-case ASCII,
  then require `WP` plus one or more ASCII digits. `wp01` and ` WP01 ` parse to
  canonical `WP01`. Non-ASCII whitespace and non-ASCII digits are rejected.
- Both implement `Clone`, `Debug`, `Display`, `Eq`, `Hash`, `Ord`,
  `PartialEq`, `PartialOrd`, `FromStr`, `Serialize`, and `Deserialize`.
  Deserialization revalidates through `FromStr`; derived deserialization is
  deliberately not used. `Display` and serialization return the stored
  canonical form.

## Path and I/O semantics

### `discover_project_root`

- `SPECIFY_REPO_ROOT` (Tier 1) wins when it names an existing directory,
  even without `.kittify`, matching upstream #1965. An empty, missing, or
  file-valued variable falls through. A worktree-named override resolves to
  its primary checkout.
- Otherwise walks `start` and every ancestor. A linked-worktree `.git` file
  with the `<main>/.git/worktrees/<name>` topology re-anchors at the primary
  checkout when the primary has `.kittify`.
- A `.git` directory is a project boundary when it contains `.kittify` or a
  `HEAD` file. An empty `.git` directory stops the walk rather than climbing
  into an enclosing checkout (upstream #2610 semantics).
- A real `.kittify` directory is the non-Git fallback marker. A broken
  `.kittify` symlink is not a marker.
- Absolute `GIT_CEILING_DIRECTORIES` entries bound the walk; the listed
  directory itself is still examined. Relative entries are ignored, as Git
  ignores them.

### `resolve_within`

- Rejects absolute paths, Windows drive-absolute and drive-relative spellings,
  UNC/rooted backslash spellings, any `..` component (POSIX or Windows
  separator), and path components that reintroduce an absolute root.
- Resolves the root and the candidate with symlinks expanded. Symlink targets
  are expanded even when the target does not exist yet, so a dangling link to
  an outside directory fails closed.
- Accepts a missing tail lexically under the resolved root. Rejects a symlink
  loop as `KernelError::SymlinkLoop`.
- Returns the resolved candidate, not the caller's spelling. Empty `relative`
  resolves to the resolved root, matching upstream's `PurePosixPath("")`
  behavior.

### `read_bounded`

- Requires a regular file. Directories, FIFOs, and device nodes are typed I/O
  errors. Unix opens use `O_NONBLOCK | O_NOCTTY` before checking descriptor
  metadata so opening a FIFO without a writer cannot hang the read.
- Checks metadata length first, then reads at most `max_bytes + 1`. A file
  that grows during the read cannot exceed the bound undetected.
- `max_bytes == 0` accepts only empty files.

### `atomic_write`

- Reads the existing bytes and returns `Ok(false)` without touching the file
  when they are identical: no temporary file, no inode replacement, no mtime
  change.
- Changed content is written to a temporary file in the destination
  directory, flushed, `sync_all`ed, and renamed over the destination. Readers
  therefore see complete old or complete new content.
- The parent directory must already exist, matching upstream
  `atomic_write(..., mkdir=False)`.
- Concurrent writers are last-writer-wins and atomic; there is no
  cross-process lock.

## Deliberate gaps

This tranche implements only the public contract above. It intentionally does
not yet provide the rest of upstream `src/kernel/`:

- `get_kittify_home`, `SPEC_KITTY_HOME`, pack-root/template-root resolution,
  and asset-root discovery.
- Git adapters (`run_git`, listing, blame, remote freshness).
- Guarded reads, meta decoding, YAML I/O, content digests, locks, clocks, and
  event-ledger append (upstream event writes are service-owned; this crate
  does not write event ledgers).
- A `stop` parameter or test-only walk ceiling for project discovery; only
  `GIT_CEILING_DIRECTORIES` bounds the walk.
- Windows case-insensitive containment. Containment compares resolved
  components case-sensitively, so a differently-cased but contained Windows
  path may be refused rather than accepted.
- `read_bounded` follows symlinks. Callers that need a final-component
  no-follow policy must apply it at their owning boundary.
- `atomic_write` does not preserve an existing file's mode, owner, ACLs,
  extended attributes, or hard links, and does not fsync the parent directory
  after rename. New files use the temporary-file default mode (`0600` on
  Unix). A destination symlink is followed for the equality read and replaced
  by the rename.
- Upstream failure ordering and diagnostic codes are not reproduced
  byte-for-byte. Typed variants and messages are Rust-native.

## Verification

From the workspace root, with bounded jobs on a shared VM:

```bash
CARGO_BUILD_JOBS=2 cargo test -p sk-kernel
CARGO_BUILD_JOBS=2 cargo clippy -p sk-kernel --all-targets -- -D warnings
cargo fmt -p sk-kernel -- --check
```

The suite covers discovery (including environment overrides in a child
process), traversal and symlink escape (including dangling links and loops),
bounded reads, no-op-stable atomic writes, adversarial identifier input, and
proptest properties that every parsed identifier remains one safe ASCII path
component.
