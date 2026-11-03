# Native Op commit-or-skip probes — implementation record (FINAL)

Status: **historical owner delivery, before coordinator integration.** Current
bindings/gates are in `COMMIT-PROBES-INTEGRATION-REPORT.md`; the source hashes
below bind the original delivered snapshot, not later corrections. Owner's
policy-suite count is corrected to27 passing+1 helper from the retained log. This file was written provisionally
(before any code) and is now replaced in place with the delivered implementation,
red/green evidence, exact counts, hashes and remaining limits. It is not an
executor, admission, protection, commit or CLI claim.

## Delivered

Authorized scope only; no other source, manifest, lockfile, inventory,
capability or CLI file was touched. The two pre-existing dirty files
(`Cargo.lock`, `crates/sk-invocation/Cargo.toml`) are the coordinator's local
`sk-invocation -> sk-git` path edge and were not modified by this owner.

| File | State | sha256 |
|---|---|---|
| `crates/sk-git/src/symbolic.rs` | NEW | `1eeb6d0c795a03551f5e207d633ad5d14068e90a8ade279a0446c8cd4450f75a` |
| `crates/sk-git/tests/symbolic.rs` | NEW | `11c4f8c98bd77dd35641df8bb0941c30e2672ff885cee256c63d7b2a7d1cfb51` |
| `crates/sk-git/src/lib.rs` | +1 line (`mod symbolic;`) | `464a3abe6cc9016cef46e5dca1c792c586c92804c2af9788c33d099427766069` |
| `crates/sk-invocation/src/op_commit_policy.rs` | NEW | `5f22cd4a7020998a4a08feb5404abf42af36adaa63d0419f31184ed92f95d846` |
| `crates/sk-invocation/tests/op_commit_policy.rs` | NEW | `1b8573404142947e2f23c86feb75c2eea1018032dc3119fe8b8051da2e9acdb9` |
| `crates/sk-invocation/src/lib.rs` | module/export/doc lines | `e63244c57e2bb75565c1e4021619b58209535a42b5f11d3faf4a2cfdeca906c9` |

Baseline commit: `12b48e0c5f4dc8596559d5732cf2eac1c0426fce`. Source reference:
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`.

Delivered API:

- `GitRepository::symbolic_ref_short_target(&RefName) -> Result<Option<String>, GitError>`
  — generic read-only `git symbolic-ref --quiet --short <validated RefName>`
  through the existing captured `GitProcess` seam (handle root, pinned program,
  timeout, captured operator index, removed repository selectors). Exit 0 is
  the target, exit 1 is `None`, any other completed status is
  `GitError::CommandFailed`, and spawn/timeout/capture failures keep their
  typed errors.
- `COMMON_PRIMARY_BRANCHES`, `PrimaryTracking`, `OpCommitProbeError`,
  `is_op_record_path`, `resolve_primary_refs`, `primary_tracks_op_records`,
  `probe_primary_tracking`, `op_commit_skip_reason`, `OpCommitSkipProbe`.

No production stub was added. No executor, admission decision, protection
union, auto-commit, lifecycle/StepContract execution, dispatcher, capability,
manifest, lockfile or Python runtime call was added.

## Semantics (source-grounded)

- **Primary resolution** (`resolve_primary_refs`): the
  `refs/remotes/origin/HEAD` short target is read first; with `origin/` stripped
  its local then `origin` ref are probed, then the `main`/`master`/`develop`
  ladder runs, skipping the already-probed name. Nothing resolved is an empty
  vector, never the checked-out branch. At most **nine resolution Git calls**
  (1 symbolic-ref + 2 dangling-name + 6 ladder) before any tree read.
- **Tracking** (`primary_tracks_op_records`): `ls-tree --full-tree -r
  --name-only -z` at each ref with the literal `kitty-ops/` pathspec; the first
  path matching `(?:^|/)kitty-ops/[0-9A-HJKMNP-TV-Z]{26}\.jsonl$` wins. The id
  is matched as 26 upper-case Crockford characters, never decoded as a ULID, so
  `Z`×26 and `0`×26 count. `ops-index.jsonl`, `op-closures.jsonl` and arbitrary
  JSON do not. Either the local or the `origin` tree can satisfy. The
  reference's Python `$`-anchor quirk (one trailing newline still matches) is
  preserved deliberately and tested.
- **Skip decision** (`op_commit_skip_reason`): reuse the accepted
  `GitRepository::is_path_ignored` (`check-ignore -q --no-index`); the ignore
  reason comes first, then the unresolved-primary or named-primary-with-no-Ops
  reason, else `None`. Exact strings are asserted natively and compared with the
  live oracle.
- **Best effort** (`OpCommitSkipProbe`): mirrors `executor._op_commit_skip_reason`
  — primary tracking is probed and cached **first**, then the helper is asked
  (ignore before tracking). A probe failure becomes
  `git probe failed while deciding whether to commit the Op record: …`; it is
  not a success, commit or refusal. A successful tracking cache survives a later
  ignore failure; a failed tracking probe is not cached and retries. The cache
  is per value, not global, and is never invalidated (matching the reference's
  executor field lifetime).
- **Strictness split:** the generic symbolic-ref read is strict about every
  non-1 status. The Op policy deliberately tolerates a *completed* non-zero
  symbolic-ref or ref-existence answer as "absent", because the reference's
  `_ref_exists` compares only the exit code. Never-started, timeout and
  capture/read failures raise `OpCommitProbeError`, carrying the failing phase
  (symbolic-ref, ref existence, tree listing, ignore probe). A signalled
  `check-ignore` is a probe error; a signalled `rev-parse` is "absent".

## Evidence

### Initial red (real, labelled)

1. **Missing-symbol compiler red** for the new generic API, after the tests were
   written first:
   `/tmp/sk-symbolic-red.log` — 16 × `E0599 no method named
   symbolic_ref_short_target`. This is a missing-symbol compile failure, not a
   native assertion failure. After implementing `crates/sk-git/src/symbolic.rs`
   and its one `mod` line: **13/13 pass** (`/tmp/sk-symbolic-green.log`).
2. **Missing-symbol compiler red** for the Op policy module:
   `/tmp/sk-op-probes-red.log` — 1 × `E0432 unresolved imports …` for every new
   symbol. Also a missing-symbol compile failure, not a behavioral red.
3. First runnable Op-policy run (`/tmp/sk-op-probes-green2.log`) was a **real
   test-harness red: 22 passed / 4 failed**, all in the new tests, none a
   product defect hidden by a stub:
   - the fake `git` used for the timeout case answered exit 0 for the
     unrecognized `check-ignore`, which the policy correctly read as "ignored";
     the fixture now answers exit 1 explicitly;
   - fake tree listings were matched on `$1`, but `tree_paths_at` correctly
     prefixes argv with `--literal-pathspecs`, so the tree controls were not
     being exercised; the scripts now key on `$2 = ls-tree` and assert the exact
     prefixed argv (this also strengthened the "no tree during resolution"
     control);
   - one fake `rev-parse` pattern did not accept the `origin` ref, so only the
     local tree was read; corrected to make both refs resolve;
   - the oracle script read `sys.argv[1]`, but the shared `run_python` helper
     passes the script only; the root now travels on stdin.
   - one helper function was shadowed by a local binding; renamed.
   No production code was changed to make these pass, and no assertion was
   weakened.
4. One scoped Clippy red (`/tmp/sk-op-git-clippy.log`): an unused fixture
   constant; removed. One scoped fmt red was corrected by running scoped
   `cargo fmt -p sk-git -p sk-invocation` and re-running all gates.

### Green

All Cargo invocations used `bash /tmp/sk-focused-cargo.sh` (shared
whole-command lease `/tmp/spec-kitty-rust-cargo.lock`, 2 jobs, no incremental,
existing `target/`, isolated HOME/XDG/TMP):

```
bash /tmp/sk-focused-cargo.sh test -p sk-git -p sk-invocation --locked
bash /tmp/sk-focused-cargo.sh clippy -p sk-git -p sk-invocation --all-targets --locked -- -D warnings
bash /tmp/sk-focused-cargo.sh fmt -p sk-git -p sk-invocation -- --check
```

- `/tmp/sk-op-final-tests.log`: **405 passed / 0 failed / 5 ignored**, across
  **29 suite results including doctests**. The 4 ignored `sk-git` cases are the
  pre-existing parent-exercised subprocess helpers; the 1 ignored
  `sk-invocation` case is the new captured-index child, exercised and asserted
  by its passing parent test (`a_captured_alternate_index_stays_bound_and_the_
  probes_write_nothing`). This is a scoped two-package run, not a workspace
  aggregate.
- `/tmp/sk-op-final-clippy.log`: strict scoped all-target Clippy, pass.
- `/tmp/sk-op-final-fmt.log`: scoped format check, pass.
- New suites: `crates/sk-git/tests/symbolic.rs` **13/13**;
  `crates/sk-invocation/tests/op_commit_policy.rs` **28 tests (27 pass + 1
  parent-exercised ignored helper)**. Three new module unit tests live in the
  `sk-invocation` lib target; one new `no_run` doctest was added.
- Required oracle: `.venv-rust-oracle/bin/python` is **Python 3.11.15**;
  `scripts/rust/verify_oracle.py --root .` reports `ok: true`,
  `checked_runtime_requirements: 53`, `missing: []`, `drift: []`,
  `unsatisfied: []`. The differential test `full_values_match_the_pinned_python_source`
  compares full values (refs list, tracked flag, primary name, and every exact
  skip string) for seven real fixtures: local-main record, origin-only record,
  dangling `origin/HEAD`, tracked-but-ignored, unresolved, named-primary-no-Ops,
  and exact-path sentinel ignore. It fails closed if the oracle is missing or
  drifted.

### Real-repository matrix covered

`origin/HEAD` normal and origin-only targets; dangling `origin/HEAD` falling
through the ladder; both refs of a name existing (ambiguous local+origin) with
different trees in either direction; `main`/`master`/`develop` order and a
develop-only ladder hit; a topic-only record that must not satisfy; 26-character
Crockford `Z`/`0` ids accepted and lowercase/invalid/spine/index/arbitrary JSON
excluded; tracked-but-ignored path; sentinel probe path versus a real record
under an exact-path ignore rule; gridded probe failures (non-1 exit, signal,
missing executable, timeout, capture overflow, tree-listing failure) and the
best-effort reason; successful-cache retention and failed-tracking retry;
error-phase variants; unchanged `HEAD`, index, staging and worktree, including a
captured alternate `GIT_INDEX_FILE` in a subprocess; a fake-git worst case of
exactly nine resolution calls in source order with no tree read, the
already-probed name skipped, and tree reads strictly after resolution.

## Limits and declared divergences

- The reference also accepts a bare filesystem root; the native API requires an
  already-opened `GitRepository`, so a non-repository root is refused at open
  (`GitError::NotARepository`). This is not full command parity.
- Probe timeouts use the repository handle's `GitOptions` (default 10 s), not a
  hard-coded 30 s. Callers needing the reference's budget pass
  `GitOptions { timeout: Duration::from_secs(30), .. }`. No second timeout
  authority was introduced.
- Reason strings and the `check-ignore` operand use the literal `GitPath`
  spelling. As in the accepted ignore-probe review, Python's `PurePath`
  normalization of `""` to `.` and of trailing separators is not reproduced;
  Op-record paths use neither form. A non-UTF-8 path is rendered lossily in the
  reason text only.
- A successful-but-unusable generic answer (empty symbolic target, non-UTF-8 ref
  name, non-UTF-8/non-commit object id) is a typed probe error rather than the
  reference's truthiness answer. Real Git cannot produce those shapes for these
  refs; the generic read is deliberately strict and the policy maps only the
  completed non-zero answer to "absent".
- A non-UTF-8 stderr from a *failed* Git command is rendered lossily inside
  `GitError` instead of raising a decode `ValueError` as Python does. Mapping
  remains phase-dependent: completed symbolic/ref failures can become absence,
  while ignore/tree failures remain probe errors. This is a genuine boundary,
  not a claim that every undecodable failure is equivalent.
- The original live-oracle differential ran from the repository root despite
  the temporary-CWD requirement. Read-only intent did not waive that rule.
  Coordinator fixes the shared helper/verifier and adds a real CWD/source-import
  assertion; all tests rerun, as recorded in the integration report.
- Fake-git seam cases are Unix-only (`/bin/sh`), like the existing ignore-probe
  suite. No Windows or macOS execution is claimed, and no workspace aggregate,
  release build, performance, platform, CLI or capability result is claimed.
- The `OpCommitSkipProbe` cache is not invalidated on a branch switch, matching
  the reference executor's per-instance field; only a failed probe retries.
- Protected-main false advance, admission, protection union and the actual
  best-effort auto-commit remain open later slices. No commit or push was made
  by this owner.
