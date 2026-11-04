# Canonical operator-index lock endpoint — PROVISIONAL report (pre-probe, pre-code)

**Historical owner392-Git/3-binding snapshot.** Current393-Git/245-Op/2395-workspace
integration and4 bindings are in `INDEX-LOCK-ENDPOINT-INTEGRATION-REPORT.md`.
The absolute “before any probe” claim below is qualified: wrapped orientation00
started07:44:49, before the07:45:40 provisional. It preceded Source behavior
probes/production edits, not all orientation. Original text/copy retained.
Owner permanent cases are Native controls bound to earlier external Source
observations; coordinator adds a permanent live Source-first full-object pair
(relative+absolute link), not another pre-fix red. Coordinator also preserves
original Path::join capture (empty retains OS-string separator), adds byte/raw
symlink-target assertions and checked object reads; historical hashes below
are not current hashes. No stage bridge or broad parity is implied.

**STATUS: FINAL (sections 6–15 are the delivered, verified result).** The
provisional text below (sections 1–5) was written at 2026-11-04T07:45:40Z, before
any probe and before any production edit; its byte copy is preserved at
`/tmp/sk-index-lock-endpoint-report.provisional.md`
(sha256 `6035fa7316051c5ca0fa33116b75ff9d9725c2dff7f6282de89a77a9ef60084a`).

**Status: PROVISIONAL.** Written at 2026-11-04T07:45:40Z, **before** any probe of the
actual Git/CPython behaviour and **before** any production edit. A byte copy of this
provisional text is preserved at
`/tmp/sk-index-lock-endpoint-report.provisional.md`; this file will be extended with
FINAL actual hashes, counts, commands, first failures and limits, and only the FINAL
section marks the deliverable.

Base: `27e350dcbff372c89c2d15e666a6c6137a061db6` (clean, published, remote). Repo
`/home/exedev/.t3/scratch/2026-11-02-pull-current-spec-kitty-from-442fd12a/spec-kitty-rust`.
Role: implementation sub-agent under
`/tmp/sk-native-27e-index-lock-orders.md` and the coordinator assessment
`/tmp/sk-stage-bridge-coordinator-assessment.md`. Functional port; **not** a
security/cyber task. No commit/push/subagent; the coordinator owns publication.

## 1. What is already read (read facts, not proposals)

- `crates/sk-git/src/index_authority.rs` captures `GIT_INDEX_FILE` **once**, keeps
  `raw` verbatim (handed to every child git), keeps `absolute` (relative resolved
  against the discovered work-tree root) and a `lock` path computed as
  `<spelled absolute>.lock`.
- `crates/sk-git/src/repository.rs::operator_index_path()` returns the spelled
  absolute path for a captured selector and lazily `<git-dir>/index` otherwise.
  `operator_index_lock_path()` returns the captured `lock` field — i.e.
  `<spelled>.lock` — and a `+ ".lock"` suffix for the default index.
- `crates/sk-git/src/safe_commit_transaction.rs::OperatorIndexLock::acquire` is the
  only artifact lock: create-new, `same_file` identity, PID fence, body write,
  `ensure_owned` before/after hooks, `Drop` unlinks only the same identity. It is
  acquired from `repository.operator_index_lock_path()?` (line ~347).
- Read-only checks also call the same getter: `repo_state.rs:153` and
  `safe_commit.rs:1938` (`..._lock_path()?.exists()`).
- `prospective_operator_index` reads `repository.operator_index_path()?` (the
  spelled path — `fs::read` follows a final symlink, so the body itself is the
  target's bytes; that is not the mismatch).
- Existing test `crates/sk-git/tests/index_authority.rs::a_symlinked_index_keeps_its_spelling_and_gits_own_follow_through`
  proves only that the commit follows the link and succeeds; it does **not** prove
  lock exclusion. It must keep passing unchanged (spelled public getter truth).
- Coordinator assessment (mandatory) records as measured before this task:
  git opens `<resolved target>.lock` and renames onto the resolved target
  (`symlink-detail`, `symlink-lockname` logs), native locks `<spelled>.lock`.
  Native `Fixture::new` in the throw-away probe crate deleted reused PID roots;
  this task must not repeat that (fresh unique temp dir per case, no
  `remove_dir_all` of reused fixtures, no evidence deletion).
- Design doc `/tmp/sk-source-staging-bridge-design.md` is **not** implementation
  authority. Its D1 (whole-preimage restore), D2 (blanket Unix-only `Unsupported`)
  and D6 (symlink `Unsupported`) are **rejected**. Only its measured NEW fact —
  Git's lock endpoint follows the final symlink — is carried forward.

## 2. Problem statement (to be confirmed by Source-first probes)

`operator_index_lock_path()` is the single canonical lock authority used by the
artifact lock andby every existence check. For a captured index whose final path
component is a symlink, Git's own writer locks and renames the **resolved target**,
so our `<spelled>.lock` does not exclude Git: a concurrent/hook `git add` against
the captured index would succeed and mutate the operator's staging under our
transaction. Source (`safe_commit` via real `git commit --only`) holds the resolved
lock, so the same attempt fails.

Everything else must be preserved: raw env capture once and verbatim, the public
spelled getters, plan/apply authority, PID/`same_file` identity, DIRC body,
foreign-lock refusal, CAS/readback/fences, no new unsupported refusals, no new
lock namespace, no second resolver.

## 3. Planned bounded fix (subject to measured Git semantics)

ONE canonical derivation, reused by the artifact lock and every check:

- `index_authority.rs` gains the only implementation of Git's final-component
  symlink resolution (hop/chase/relative/absolute/dot/parent/dangling/loop/
  readlink-error/non-UTF8 semantics **as measured on the host Git**, cap only as
  measured, no invented 5/40), returning the physical index endpoint.
- `repository.rs::operator_index_lock_path()` derives `<physical endpoint>.lock`
  **at call time** (not frozen at open) from the existing authority; the spelled
  `explicit_path()`/`explicit_lock_path()` candidate getters stay, documented as
  the spelled candidate vs the actual resolved endpoint.
- No call-site refactor beyond the existing single acquire/check sites, unless a
  minimally-scoped wiring change proves necessary (would be reported).

Explicit non-goals (per orders): no real operator stage/bridge, no recovery/restore
publisher, no Op runtime/CLI/Source/Kernel/manifest/deps/pin/inventory change, no
message-phase/closure API, no Windows certification claim, no mutation-ownership
claim beyond the measured endpoint (retarget-during-hook and post-CAS resync
windows are reported, not magically closed).

## 4. Probe plan (each attempt its own fresh log via `/tmp/sk-evidence-run.sh`)

1. Host Git 2.43.0 lock-endpoint matrix under isolated temp roots, with `strace`
   to read the actually opened `<...>.lock` / rename target: plain, relative final
   symlink, absolute final symlink, chain (several lengths → measured cap),
   parent-component symlink, dangling, loop, `./`/`..` spelling, non-UTF8 bytes,
   empty selector, default `<git-dir>/index` that is itself a symlink, linked
   worktree default index.
2. Source-first runtime RED at the real production entry point
   (`src/specify_cli/git/commit_helpers.py::safe_commit` under the exact
   CPython 3.11.15 oracle): captured index is a symlink (relative and absolute
   target), pre-commit hook attempts `git add` against the **explicit original
   `GIT_INDEX_FILE`**, captures its actual exit without refusing the outer commit.
   Source holds the resolved lock ⇒ 128 and foreign index unchanged. Then the
   native public API on the same fixture shape ⇒ expected 0 and a mutated operator
   stage. Source first, never behind a native assert.
3. After the fix: the same fixture must show the native attempt refused (128) and
   the operator index/foreign lock preserved; plus the healthy-path suite
   (relative/absolute/chain/dangling/default/linked/raw getter/default-index local
   bytes/read-only plans).

## 5. Delivered (this section superseded by sections 6–15)

The provisional plan held; every claim below is measured, not designed.

---

# FINAL

## 6. Measured Git lock-endpoint semantics (host Git 2.43.0)

Every command ran through `/tmp/sk-evidence-run.sh` into a per-attempt log. Git
behaviour probes: `/tmp/sk-evidence-27e-indexlock-probe/`
(`probe2-*`, `probe3-*`); each case gets a fresh `mktemp -d` root, isolated
`HOME`/`XDG_CONFIG_HOME`/`TMPDIR`, and `strace` of
`openat/rename/readlink/newfstatat`. Source was read first (`git version 2.43.0`,
`argv` recorded in every log).

| case | measured lock file git creates | endpoint after the chase |
|---|---|---|
| plain relative `idx` | `repo/idx.lock` (readlink `idx` → `EINVAL`) | `repo/idx` |
| final link → `real-index` | `repo/real-index.lock`, rename onto `repo/real-index`; the link survives | `repo/real-index` |
| final link → absolute target | `repo/real-index.lock` | the absolute target |
| chain 2/3/4/5 links | `repo/real-index.lock` (5 readlink calls reach the file) | `repo/real-index` |
| chain 6 links | `repo/f.idx.lock`, rename **onto `repo/f.idx`** — the still-symlinked 5th path is replaced by a regular index | `repo/f.idx` (chase stopped) |
| chain 7 links | `repo/f.idx.lock`, same stop after 5 dereferences | `repo/f.idx` |
| two-link loop `a↔b` | `repo/b.idx.lock`, rename onto `repo/b.idx` (regular file) | `b` after 5 dereferences |
| parent-directory link `dir/idx` | `repo/dir/idx.lock`, rename through the parent link | final component only |
| dangling link → `missing.idx` | `repo/missing.idx.lock`, target created | `repo/missing.idx` |
| `./sub/../idx` selector | `repo/./sub/../idx.lock` (no lexical normalisation) | same spelling |
| link → `./sub/../real.idx` | `repo/./sub/../real.idx.lock` | joined, unnormalised |
| link in `sub/` → `../real.idx` | `repo/sub/../real.idx.lock` | joined, unnormalised |
| non-UTF-8 name `bad\xffidx` | `repo/bad\377idx.lock` (bytes preserved) | same bytes |
| empty `GIT_INDEX_FILE` | `repo/.lock`, rename onto `repo/` → `ENOTDIR`, `git` exits 128 | `repo/` |
| default `.git/index` that is a link → `../alt/real.idx` | `repo/.git/../alt/real-index.lock` | `.git/../alt/real.idx` |
| linked worktree default index | `<git-dir>/worktrees/linked/index.lock` | unchanged |
| parent is a file (`seed.txt/idx`) | `repo/seed.txt/idx.lock`, open `ENOTDIR` → git 128 | unchanged (`readlink` `ENOTDIR`) |
| parent not searchable (`locked/idx`) | `repo/locked/idx.lock`, open `EACCES` → git 128 | unchanged (`readlink` `EACCES`) |
| target is a directory | `repo/real.idx.lock` then git 128 | the directory |

**Conclusion measured, not assumed:** git resolves **only the final component**,
joins a relative target to that path's parent **without normalising**, uses **at
most five `readlink` calls** (a still-symlinked path after five *is* the endpoint),
and treats **every** `readlink` error as "stop here" (`ENOENT`, `EINVAL`,
`ENOTDIR`, `EACCES`). There is no 5/40 invention: five is what this host Git does,
including the surprising chain-6/loop case where git replaces the fifth link with
a regular index.

## 7. Source-first runtime RED (real `safe_commit`, exact CPython 3.11.15)

Log `sourcefirst-index-lock3.iYJb2g.log` (probe `/tmp/sk-index-lock-source-probe.py`).
Process isolation asserted inside the probe: `cwd=/tmp/sk-oracle-idxlock-root/cwd`,
`HOME`, `TMPDIR`, `XDG_{CACHE,CONFIG,DATA,STATE}_HOME` all under that root;
`python 3.11.15`; `PYTHONPATH=<repo>/src`, `SPEC_KITTY_PACKS_ROOT=<repo>/packs`.
Entrant point: `src/specify_cli/git/commit_helpers.py::safe_commit` (not a
hand-rolled git sequence).

Fixture per case: `real-index` (HEAD + unrelated staged `u.cfg`), `link-index` a
symlink to it (relative in S1, absolute in S2), requested `requested.txt`
untracked, pre-commit hook that writes `hooked.txt` and runs
`GIT_INDEX_FILE=<original captured value> git add -- hooked.txt`, recording its
own exit and the target index hash, then `exit 0` (never refusing the commit).

| case | source result | captured index | hook attempt | ref / default index |
|---|---|---|---|---|
| S1 relative link | commit `ae6245d4…` landed | `97946b3c…` 262 B → `da638463…` 361 B, `requested.txt` added, `u.cfg` preserved | `add_exit=128`, `Unable to create '.../real-index.lock': File exists`, before/after sha `5ba18892…` identical | `HEAD 767d2363…` → `ae6245d4…`; `.git/index` sha unchanged |
| S2 absolute link | commit `ae6245d4…` landed | same shape | `add_exit=128`, before/after sha `dfb2bb8f…` identical | unchanged default index |
| S3 foreign `real-index.lock` ("foreign lock body") | `RuntimeError: failed to stage requested files … 'requested.txt': fatal: Unable to create '.../real-index.lock': File exists` | `1c9f6274…` unchanged, entries unchanged | n/a (no hook) | `HEAD` unchanged `767d2363…`; foreign file preserved verbatim |

Also measured: the hook's own `GIT_INDEX_FILE` is git's per-commit spelling
(`.git/next-index-<pid>.lock`), so the attempt has to name the original captured
value explicitly — which is what the fixture does. The alias survives as a
symlink in every case.

## 8. Native RED on the same business shape (retained, pre-fix)

Tests were added **before** the production edit and run against the unmodified
crate: `red02-native-business-mismatch.jY6RZr.log` (exit 101). Not a compiler or
harness failure — the real public API ran and its footprint differed:

1. **Hook write (business mismatch)** — same relative-link fixture; the native
   child opened the repo, ran `GitRepository::safe_commit`, and the hook's
   explicit `git add` **succeeded and mutated the operator index**:
   `spelled_lock=yes`, `target_lock=no`, `add_exit=0`,
   `before_sha=41ee9791…` → `after_sha=b85bf10f…`. Source (S1) refused the same
   write with 128 and identical bytes.
2. **Foreign lock at the resolved endpoint** — native held `<spelled>.lock`,
   ignored the foreign `<target>.lock`, **landed commit `6b0b20b6…`** and then
   failed its reconciliation with
   `fatal: Unable to create '.../link-index.lock': File exists`. Source (S3)
   refused before any mutation.
3. **Retarget after plan** — the same wrong-name lock meant the hook's write was
   accepted (`add_exit=0`, index sha changed) instead of being excluded.

## 9. The fix (one canonical endpoint, bounded)

| file | change |
|---|---|
| `crates/sk-git/src/index_authority.rs` | the ONE final-component resolver `resolve_final_symlink` (measured semantics above, limit `FINAL_COMPONENT_READLINK_LIMIT = 5`, any `readlink` error stops, OS bytes and spelling preserved, never fails) and `lock_path_for` (byte-append `.lock`). `capture` now builds a relative spelling as `<root>/<spelling>` by concatenation (git's own absolute-path prefix, so an empty selector is `<root>/` and its lock is `<root>/.lock`, exactly git's measured `repo/.lock`); the empty special case is gone because the model is now git's. `explicit_path`/`explicit_lock_path` documents say *spelled candidate* vs *actual resolved endpoint*; both public getters and their values are retained (the `explicit_lock_path` value is byte-identical to before). |
| `crates/sk-git/src/repository.rs` | `operator_index_lock_path()` = `lock_path_for(resolve_final_symlink(operator_index_path()))`, derived **at call time**. `operator_index_path()` still returns the captured spelling. No new public API, no second resolver, no copied validator, no new git invocation (it was already lazy for the default index), no new error variant, no new `Unsupported`. |
| `crates/sk-git/tests/index_authority.rs` | permanent consumer regressions and healthy-path controls (section 10); the one child harness grew `lock=`, `plan-foreign`, `retarget`, `SK_BRANCH` and bounded object metadata, plus isolated-root assertions. |

Everything else is untouched: raw `GIT_INDEX_FILE` capture and verbatim
re-execution, plan/apply authority equality, `OperatorIndexLock` create-new +
`same_file` identity + PID fence + `Drop` (never unlinks a foreign name), DIRC
lock body, hooks/CAS/readback/fences, unrelated-staged preservation,
`JHPRawJ`, pinned route, index-deletion route, 32-case corpus, read-only plans.

Because the derivation is at observation/apply time (not frozen at open), a
symlink retargeted after the plan is followed to its new target and the plan is
**not** treated as stale — the test in section 10 proves the lock moves and the
old target is never written.

## 10. Permanent regressions and controls added

Integration (`crates/sk-git/tests/index_authority.rs`, 7 new tests):

- `a_hook_writing_the_captured_index_is_refused_at_gits_lock_endpoint` — Source-first
  consumer regression: the hook's explicit write must exit 128, captured index
  bytes unchanged, `hooked.txt` not staged, `unrelated.txt` preserved, requested
  path reconciled, symlink intact, `object_type=commit` / `object_size>0` /
  `object_parents=1`, both locks released.
- `a_foreign_lock_at_the_resolved_endpoint_blocks_apply_and_is_preserved` — foreign
  `<target>.lock` present **after** the plan refuses the apply with
  `OperationInProgress:index-lock`, and present **before** the plan refuses the
  read-only plan; in both cases the foreign bytes, captured index bytes, default
  index and `HEAD` are unchanged and the spelled candidate is never locked.
- `a_link_retargeted_after_the_plan_moves_the_lock_to_the_new_target` — plan under
  the old target, retarget, apply: accepted (not stale), hook sees the new
  target's lock, the old target is byte-identical, the new target is reconciled,
  the retargeted spelling survives.
- `a_link_chain_and_an_absolute_link_resolve_before_locking` — two-link chain and
  an absolute target both resolve to the real index; links survive.
- `a_dangling_final_link_is_locked_where_git_creates_the_index` — one hop, git
  creates the target, the lock is `<target>.lock`, default index untouched.
- `a_symlinked_default_index_is_locked_at_its_target` — `.git/index` as a symlink:
  lock at `.git/../alt/real-index.lock` (unnormalised, exactly git's spelling),
  target reconciled, unrelated staging preserved, `.git/index.lock` absent.
- `a_linked_worktree_default_index_lock_stays_in_its_own_git_dir` — linked
  checkout locks `<git-dir>/worktrees/linked/index.lock`; the primary index is
  untouched.

Unit (`crates/sk-git/src/index_authority.rs`, 9 new tests): not-a-symlink;
missing file and empty selector; relative target joined without normalisation
(incl. `./sub/../`); absolute target; five-link chain resolves and six-link chain
stops at the fifth path; loop stops; parent-directory link never chased; dangling
target plus non-UTF-8 bytes; `.lock` appended, never substituted
(`index.tar` → `index.tar.lock`).

New helpers: 6 (test module `ChildRun::field`, `install_hook`, `lock_of`,
`assert_isolated_child`, `describe_git_error`, `describe_object`) + 1 unit
`temp_root` = **7**. The child process asserts the actual fixture `CWD`,
`HOME`, `XDG_{CONFIG,DATA,STATE,CACHE}_HOME` and `TMPDIR` roots before acting
(`env_clear` alone is not treated as isolation), and the Source probe asserts the
same on its side.

## 11. Gates actually run (each its own fresh log)

Evidence root `/tmp/sk-evidence-27e-indexlock-074449` (`command-log-index.tsv`).
Whole-Cargo commands all went through `/tmp/sk-focused-cargo.sh` (whole lease
`flock`, `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, existing repo `target/`,
isolated sandbox `HOME`/`XDG_*`/`TMPDIR`, repo `CWD`).

| gate | log | result |
|---|---|---|
| first business red (native, pre-fix) | `red02-native-business-mismatch.jY6RZr.log` | exit 101, 3 failed — business footprint only |
| resolver unit tests | `green01-index-authority-unit.KHAh8L.log` | 9 passed |
| index-authority integration | `green08-index-authority-foreign-plan.xLf7T5.log` | 13 passed / 1 ignored helper |
| full `sk-git` (final tree) | `final-git-tests.8PcQWk.log` | **29 binaries, 392 passed / 0 failed / 9 ignored** (was 376/0/9 → **+16**) |
| full `sk-invocation` (Op) | `final-op-tests.yMfT99.log` | **245 passed / 0 failed / 3 ignored** (unchanged) |
| Git+Op doctests | `final-doctests.HKEWyK.log` | 6 + 1 passed |
| scoped strict Clippy (all targets, `-D warnings`) | `final-clippy.rPOub7.log` | pass |
| `cargo fmt --all -- --check` | `final-fmt.ATPgVo.log` | pass |
| strict rustdoc (`RUSTDOCFLAGS=-D warnings`, no deps) | `final-strict-docs.NAbGIL.log` | pass |
| locked offline | every Cargo command above used `--locked --offline` | pass |
| exact-53 oracle (CPython 3.11.15) | `final-oracle-exact53.BtjeK3.log` | `{"checked_runtime_requirements": 53, "drift": [], "missing": [], "ok": true, "python": "3.11.15"}` |
| `git diff --check` | `final-diff-check.1c5iK7.log` | pass |

No aggregate, release build, commit, push, subagent, background poll, watcher or
process-group signal was used.

## 12. Frozen state (actual)

- `HEAD = 27e350dcbff372c89c2d15e666a6c6137a061db6` (clean published base;
  changes are working-tree only).
- `git status --porcelain` (`freeze-status.UKZed0.log`):
  `M crates/sk-git/src/index_authority.rs`, `M crates/sk-git/src/repository.rs`,
  `M crates/sk-git/tests/index_authority.rs`,
  `?? crates/sk-git/INDEX-LOCK-ENDPOINT-REPORT.md` — nothing else in the tree
  changed, so every non-scope file (Op, Kernel, Workspace, CLI, Source, manifests,
  `Cargo.lock`, pins, inventory, corpus, fixtures) is frozen.
- `git diff --stat` (`freeze-diffstat.TK2GpX.log`): 3 files, 1003 insertions,
  37 deletions.
- sha256 (`freeze-hashes.j9rKET.log`):
  - `bba9d7c71514241f42b8c05919b0e4ca7a0e1ce83c9a5f5ab17b24a1cec8eb41  crates/sk-git/src/index_authority.rs`
  - `b38cfe7f3ece1e3bdc6be2295f39fc20dcc1c467d61ae394672d6998fa879121  crates/sk-git/src/repository.rs`
  - `bfa0e1bf299ca341d011c40b4b1e7809617e54cc893f8826f761b7723dbae58e  crates/sk-git/tests/index_authority.rs`
  - this report is excluded from its own hash list (writing it changes its bytes);
    the staged (HEAD) `crates/sk-git` listing hash — i.e. the frozen base listing,
    unchanged because nothing was staged — was
    `3946741e8c59239f30efe1e5b57bb0e15a19ad8631d8ff99ede2ee927f74f69a`
    (`freeze-tree`).
- No background or long-running process was started; the only process the freeze
  probe observed was the one-shot `ps` in its own command (pid 248559,
  `freeze-pids.TertDq.log`), and no poller, watcher or server was used.

## 13. Remaining limits — required, not waived

- **Retarget inside the hook / post-CAS window is not closed.** The endpoint is
  correct at each observation/apply point, but a symlink retargeted *while* the
  hook runs, or between the CAS and the post-landing reconciliation, can still
  move the target under the same transaction. A single getter cannot own that
  window; the stage-bridge work must test it against Source explicitly.
- **F1 (`fstat`-created-lock warning) stays open.** Not addressed here; the
  artifact lock still relies on `same_file` identity, and no new partial-fstat
  warning was invented or removed.
- **Native hook-time `GIT_INDEX_FILE` is the scoped index**
  (`<git-dir>/spec-kitty-safe-commit.<pid>…tmp`) while Source shows
  `.git/next-index-<pid>.lock` — measured again here, pre-existing, unchanged.
- **No Windows certification.** The resolver uses portable `std::fs::read_link`
  and pure path joining (no `Unsupported` refusal, no Unix runtime control), but
  only Linux was exercised.
- **No mutation-ownership claim** beyond the measured endpoint: this change makes
  our lock the file git locks; it does not claim the whole publication/restore
  window is closed.
- Git's five-dereference depth is measured for **Git 2.43.0** only; a different
  host git version must re-measure (the constant is documented as measured, not
  as a spec).
- Empty-selector failure classes stay divergent in the general case (git exits
  128 on the rename onto the work-tree directory; native's own `fs::read` of the
  directory becomes `Io`); unchanged by this fix and not "fixed" by guessing.

## 14. First failures retained (all logs kept)

- `trace-debug-source-fixture.rDHWEM.log` (exit 128) and
  `sourcefirst-index-lock{,2}.*` — my own probe fixture bugs (a self-referential
  symlink, then a hook env not exported); product untouched, logs preserved.
- `red01-native-gits-lock-endpoint.suiQJI.log` — the first RED attempt, where the
  getter assertion fired before the business assertions; retained, then the tests
  were reordered so the *business* footprint is the first failure (`red02`).
- `green02`/`green03`/`green07` (exit 101) — harness bugs in my new tests
  (isolation parent for `sub/deep`, expected unnormalised spelling, and a
  not-yet-patched child branch). Fixed in the tests, logs kept.
- `gate-strict-docs.2NpmmA.log` — real `rustdoc` failure:
  `public documentation for explicit_lock_path links to private item
  resolve_final_symlink`. Fixed by wording (no `#[allow]`, no suppression);
  `gate-strict-docs2` green.

## 15. Explicitly not done (per the orders)

No commit, push, merge, tag, release build, aggregate workspace run, dependency or
`Cargo.lock` change, manifest/pin/inventory change, Op runtime or consumer change,
Source change, parser/encoder change, new `Unsupported`/hardening/refusal, no
stage bridge, no real operator stage, no patch-restore publisher, no message
closure/manifest work, no evidence deletion, no global environment mutation, no
poll/watcher/process-group signal. The coordinator owns integration, publication
and any follow-up review; the next step (the actual stage bridge) must consume
this endpoint authority and stay Source-faithful.
