# Native ignore probe — implementation record

**Status: FINAL for the scoped delivery.** This file was written provisionally
(before any code) and is now replaced in place with implementation, red/green
evidence, exact counts and final hashes.

> Coordinator reproduction: 276/0/4 helpers and strict scoped Clippy pass.
> The initial whole-crate fmt check found two assertion-wrapping differences
> (`/tmp/sk-ignore-probe-coordinator-fmt.log`); Git-only formatting corrects
> them and the next check passes (`…-fmt-green.log`). The current test hash is
> `b8c17296813d99f26f2be51dfd5f6460a95111f19fb1f7a9f20e94b863eef540`;
> the owner's table below is the pre-format delivery snapshot. No behavior
> changed. Kernel is a dependency and its separately owned additive resolver
> work is still active; a stable dependency rerun and independent review are
> pending at that point. Both owners subsequently finished; the coordinator
> stable aggregate reproduces 2,073/0/9 helpers, including 276 Git tests, with
> strict workspace Clippy/fmt and a completed release-only continuation after
> an inspected tool timeout. Independent ignore-probe approval remains pending.

## Independent review and coordinator follow-up

`/tmp/sk-rust-ignore-probe-review.md` independently gives **scoped PASS** at
`38834239d` (source `e74626dc…`, formatted tests `b8c17296…`). It reproduces
276/0/4 helpers, verifies 46 native-vs-Git byte cases and 42/44 UTF-8 Python
comparisons (two deliberate literal-operand normalization differences), typed
signal/capture errors and unchanged dirty/index state. No CLI/Op policy or
performance approval.

Coordinator closes the small review follow-ups without changing the method:
adds allowed-exit-1 capture-overflow and signalled-child tests; documents the
literal `""`/trailing-slash difference from Python `PurePath`; updates current
hash rows below. Both new cases pass immediately (coverage additions, not
invented red-first product fixes). Scoped **278/0/4 helpers**, 20 suites,
strict Clippy/fmt pass in `/tmp/sk-ignore-review-followup-*.log`. Ignore suite
is now 15 passing cases plus its parent-exercised helper. The accepted method
body is unchanged; the new source hash includes only the doc clarification.

## Delivered

One generic, read-only Git probe for future Op admission, with no Op policy or
CLI wiring:

- **NEW `crates/sk-git/src/ignore.rs`**: inherent
  `GitRepository::is_path_ignored(&GitPath) -> Result<bool, GitError>`;
- **NEW `crates/sk-git/tests/ignore.rs`**: real-repository, seam-level,
  captured-operator-index and live Python-oracle contracts;
- **`crates/sk-git/src/lib.rs`**: one added line, `mod ignore;`. No re-export
  was needed because the method is inherent and `GitPath` is already public.
  No crate-level documentation text was changed.

No other source, manifest, lockfile, capability, CLI, README or Op file was
touched. The only modified tracked file owned by this delivery is
`crates/sk-git/src/lib.rs`; the two implementation/test files and this report
are new.

## Semantics

The accepted implementation is exactly:

```text
git check-ignore -q --no-index -- <GitPath as OsStr>
```

- exit 0 -> `Ok(true)`;
- exit 1 -> `Ok(false)`;
- any other exit status -> `GitError::CommandFailed` with git's status and
  stderr;
- unavailable/spawn failure -> `GitError::GitUnavailable` / `GitError::Spawn`;
- timeout -> `GitError::Timeout`;
- capture/read I/O failure -> the corresponding `GitError`.

A failed probe is never `Ok(false)`. `--no-index` is mandatory and has no
ordinary-index default, so a tracked-but-ignored path still answers true. The
probe goes through the existing `GitProcess` seam on the `GitRepository` handle,
so it uses that handle's canonical root, pinned git program, timeout and
captured operator-index binding. It adds no shell, wrapper, process-wide
environment change, status/index/worktree write or path restriction.

`GitPath::as_os_str()` is passed after `--` as raw argv data, preserving a
leading `-`, spaces, colons, UTF-8 names and (on Unix) non-UTF-8 bytes.

## Source authority

- `src/specify_cli/invocation/op_commit_policy.py::is_path_ignored`
- SHA-256:
  `09ac1c3967dad3a7d768d1979b8454ddd74987e2413861814a2e9b0fcc1dfff2`
- Source behavior reproduced: exit 0 true, exit 1 false, other return code
  raises `OpCommitProbeError`; source uses `--no-index` for the same reason.

An automated live differential runs the exact Python function on the same
UTF-8 fixture and compares 16 paths (ignored, negated, nested, missing, tracked
and awkward spellings). The oracle is required, never skipped: the test selects
`SK_KERNEL_PYTHON` or `.venv-rust-oracle/bin/python` and first runs
`scripts/rust/verify_oracle.py`. Result: Python 3.11.15, 53 locked runtime
requirements checked, no drift/missing/unsatisfied/duplicate versions, and all
16 answers matched the native probe.

## Tests and controls

The original delivery had 14 tests: 13 passing and 1 parent-exercised ignored
subprocess helper (`captured_index_probe_child`). The post-review coverage adds
two more passing cases as recorded above.

Real repositories:

1. ignored vs unignored paths;
2. tracked-but-ignored with direct `git ls-files` proof, a plain
   `git check-ignore` exit-1 control, and a `--no-index` exit-0 control;
3. missing-but-ignored paths (without creating them);
4. nested `.gitignore`, directory rules and negations;
5. leading `-`, colon, `:literal`, spaces, UTF-8, `#`, quote;
6. Unix non-UTF-8 basename bytes as true and false controls;
7. HEAD, `symbolic-ref`, default-index bytes, `ls-files -s` staging bytes and
   every worktree file byte unchanged across probes.

Seam-level:

8. exact recorded argv is `check-ignore -q --no-index -- -leading.log`, 0 is
   true and 1 is false;
9. exit 3 is typed `CommandFailed { status: Some(3) }`, not false;
10. a removed git executable is typed `GitUnavailable`, not false;
11. a hung probe is typed `Timeout`, not false.

Captured operator index, through a child process (no parent environment
mutation): a custom index is `HEAD` plus a force-added `extra.log` that the
default index does not track. The child reports
`authority=<custom>`, `captured=true`, `source=environment` and unchanged
`GIT_INDEX_FILE`. The probe is true for `tracked.log`, `extra.log` and
`keep.log`, and false for `important.log` and `a.txt`. Discriminating controls
show plain `check-ignore` with the custom selector answers exit 1 for
`extra.log` (tracked in that index) while the default index answers exit 0, yet
`--no-index` answers exit 0 under both. The parent then proves HEAD, default
index bytes, custom index bytes, both staging lists and every worktree byte are
unchanged.

## Red/green evidence

- Initial scoped run after implementation was already green:
  `/tmp/sk-git-ignore-probe-initial.log` — 13 passed / 0 failed / 1 ignored.
  There is therefore no implementation-first failure log; negative controls
  were run after green to prove the tests are discriminating.
- Negative control A removed `--no-index` from the argv:
  `/tmp/sk-git-ignore-probe-red-no-index.log` — **8 passed / 5 failed / 1
  ignored**, exit 101. First failure:
  `non_one_exit_status_is_a_typed_failure_not_false` expected
  `["check-ignore","-q","--no-index","--","broken.log"]` and observed
  `["check-ignore","-q","--","broken.log"]`. The other failures were exact
  argv, tracked-but-ignored, captured-index and live-oracle (tracked.log
  answered false where the source authority answers true).
- Negative control B used raw `run` instead of `allowing(&[1])`, so exit 3
  became `Ok(false)`: `/tmp/sk-git-ignore-probe-red-nonone.log` — **12 passed /
  1 failed / 1 ignored**, exit 101. Failure:
  `non_one_exit_status_is_a_typed_failure_not_false`, line 471,
  `exit 3 must not become Ok(false)`.
- The green implementation was restored byte-identically after each control
  (`/tmp/sk-git-ignore-rs.green.bak`, SHA-256
  `773f87e46189af9a2361cf9f7d222442aa440e6011b390d5c979ecaec6560c49`), then
  the final run was repeated. A later non-behavioral doctest-example wording
  change produced the final implementation hash below; the tests and strict
  Clippy were rerun after it.

Original delivery's final scoped run, through `/tmp/sk-focused-cargo.sh` with the existing target,
shared whole-command lease, two jobs, no incremental and temporary
HOME/XDG/TMP:

`test -p sk-git --locked --no-fail-fast` ->
**276 passed / 0 failed / 4 ignored**, 20 suite results including doctests.
The ignore suite is **13 passed / 0 failed / 1 parent-exercised helper**; the
five sk-git doctests pass. Log:
`/tmp/sk-git-ignore-probe-final.log`.

Scoped strict Clippy pass:
`clippy -p sk-git --all-targets --locked -- -D warnings`
(`/tmp/sk-git-ignore-probe-final-clippy.log`). Scoped formatting check pass on
the two new owned Rust files; `lib.rs` needed no reformat beyond the one-line
module declaration.

## Final hashes

- `crates/sk-git/src/ignore.rs`:
  `28981ece767ea1403ef776d7132fd35d81eabd15aabc83f311a6c94a65ce61fe` (post-review doc clarification)
- `crates/sk-git/tests/ignore.rs`:
  `2a69280757d36f845baea24aa7736ba7edc02e52998a93b84fd3e6a1252e2d56` (post-review capture/signal coverage)
- `crates/sk-git/src/lib.rs` after registration:
  `ca6742e37a1c8cce9088c10593aa8d892a138e7c9e00c81e469f5a4770bae270`
  (before: `030db72da01882233eae0b8f2f2eba995ffeea53f80a65aedc207acd3f6ec71e`)
- Python source authority:
  `09ac1c3967dad3a7d768d1979b8454ddd74987e2413861814a2e9b0fcc1dfff2`
- `crates/sk-git/Cargo.toml`:
  `0c6da84319ef19c682eb784f2d6f1443dd64b52f99651e33639e5bef2219ba3a`
  (unchanged)
- `Cargo.lock`:
  `bf38fec864055d116efac533ff7b10d8539aaedc69702a094b38cbea636dbd3d`
  (unchanged)
- Published repository HEAD remained
  `1ad605bb7744b8cae1d1a95b6bf449b7d1b92c9a`. Toolchain: rustc 1.99.1,
  git 2.43.0, Python oracle 3.11.15.

The workspace was not clean while this ran. Concurrent owners had modified
`crates/sk-invocation/{Cargo.toml,src/writer.rs,tests/writer_filesystem.rs}`,
`crates/sk-status/ARTIFACT-OWNERSHIP-REPORT.md`, `docs/rust/*`, and
`crates/sk-git/{src/merge.rs,DUPLICATE-BOUNDARY-REPORT.md}`. At final write,
`crates/sk-git/src/merge.rs` was
`e10b2e88fc4af06d3388bfc5fbac4903a09d9d72f3da8df580fb7ca6e6607fe8`; those
paths belong to other owners and were not edited here.

## Limits and non-claims

- This is an unwired library probe. No CLI, dispatcher, Op admission/skip/
  commit decision, close ordering, governor or whole-Op workflow consumes it.
  No CLI or whole-Op capability is promoted.
- No aggregate/release build, workspace-wide gate, independent review,
  performance measurement or cross-platform run is part of this delivery.
  Linux scoped results only; the non-UTF-8 test is Unix-only.
- `--` stops option parsing but does not disable git pathspec magic
  (`:(...)`). The probe passes the operand exactly as the Python authority
  does; it neither sanitises nor reinterprets pathspec magic. Literal colon
  names are covered and pass. Absolute and `..` operands are likewise not
  revalidated, matching the source and avoiding an invented path restriction.
- Literal operands are passed unchanged; Python `PurePath(...).as_posix()`
  additionally maps `""` to `"."` and strips trailing separators (`nested/`).
  Those spellings can have different answers. They are not used for Op-record
  paths; no full Python-adapter operand-parity claim is made.
- Ignore answers still depend on repository ignore files, `.git/info/exclude`,
  `core.excludesFile` and user/system Git configuration, as they do for the
  source oracle. The adapter does not rewrite configuration.
- The live oracle differential covers UTF-8 spellings only, because the
  reference API takes a POSIX string; the non-UTF-8 case is covered natively
  against real Git, not against Python.
- The timeout kill applies to the direct git child only (the existing seam
  limitation); a descendant is outside the bound.
- The README and crate-level operation list were deliberately not edited under
  the explicit registration exception. The public method carries full API
  documentation and a compiled doctest.
