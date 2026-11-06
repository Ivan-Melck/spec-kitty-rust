# CPython 3.11.15 `pathlib` reference for the Source rglob adaptation

`crates/sk-kernel/src/source_glob.rs` adapts CPython 3.11.15 `pathlib`'s
`Path.rglob("*.yaml")` selector chain for the built-in offering-source scan that
feeds the reference-pointer index.

## Pinned source facts

| Fact | Value |
| --- | --- |
| Interpreter | `Python 3.11.15 (main, Aug 7 2026) [Clang 22.1.3]` (the repository's lock-exact `scripts/rust/verify_oracle.py` oracle) |
| Source file | `/home/exedev/.local/share/uv/python/cpython-3.11.15-linux-x86_64-gnu/lib/python3.11/pathlib.py` |
| SHA-256 | `3b21ac188fd9ac20bacc8ccd976fabd5d8ccf5000f8e157d3dfff005b228fb3d` |
| Adapted members | `_make_selector` (fixed `**` + `*.yaml`), `_Selector.select_from`, `_WildcardSelector._select_from`, `_RecursiveWildcardSelector._iterate_directories`/`_select_from`, `_ignore_error`, `PurePath._cparts`/`__lt__`, `PurePath.name`, `PurePath.stem` |
| Licence custody | [PSF licence](cpython-3.11.15-reference-LICENSE.txt) in this directory (retained; no new dependency, no new legal approval) |

## What the Rust adaptation preserves

* `select_from` first requires the starting path to be a directory (`is_dir`
  absorbing only `_ignore_error` OSErrors and ValueError, not EACCES); an absent
  or non-directory root yields no candidates, other stat failures escape.
* The leaf `*.yaml` match is `fnmatch`'s translation, applied to the directory
  entry name — never `suffix == ".yaml"`, never an existence/regular-file test.
  A directory, a dangling symlink or any other entry whose name ends in `.yaml`
  is a candidate.
* Recursion uses `entry.is_dir(follow_symlinks=False)`, so child directory
  symlinks are not descended while an explicitly supplied symlinked root is
  scanned. `_ignore_error`'s `ENOENT`/`ENOTDIR`/`EBADF`/`ELOOP` mean "not a
  directory"; a `scandir` `PermissionError` stops that subtree; every other
  scan error propagates (no healthy-empty substitution).
* Ordering is the caller's `sorted(Path)`: `PurePath.__lt__` compares
  `_cparts`, the tuple of (case-normalised) decoded components — not the
  slash-joined string. `PurePath.name`/`stem` are reproduced as pure Python-text
  helpers in `sk_kernel::paths` (construction drops empty/`.` components, keeps
  `..`, and `stem` removes only the last suffix).
* No artificial depth, candidate-count or recursion cap is introduced.

## Deliberate differences (explicit, not approximations presented as parity)

* POSIX names are matched on native bytes before the ONE captured filesystem
  codec decodes them for sorting/index keys. The Windows ASCII suffix check now
  uses native wide units, without replacement; no Windows run certifies it.
* Python's selector makes separate wildcard/recursive scans, materialises each
  scandir iterator, and de-duplicates with a yielded set. The current Native
  single-pass walk matches the static-tree cases checked here; it does not yet
  establish syscall/race/error-first/deduplication parity under concurrent directory
  mutation. Source caller-dependent recursion limits and Native recursive-stack
  behavior remain unapproved, not a newly authorized cap or acceptance policy.
* `Path.rglob`'s audit event (`sys.audit`) has no Rust analogue.

Nothing here changes Source, packs, pins, goldens, manifests or `Cargo.lock`.
