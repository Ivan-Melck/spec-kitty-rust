# SOURCE-STAGING-REMEDIATION-REPORT — FINAL

**Historical10:09 remediation snapshot (668 scoped,24 stage tests).** Current
coordinator gates/bindings/remaining limits are in
`SOURCE-STAGING-INTEGRATION-REPORT.md` (672 scoped,2429 workspace). Coordinator
found additional bare-config, hook-lock-body and hook-lock-mode Source-first
runtime reds and fixed them; descriptor permissions/PID checks and actual NUL
consumer assertions strengthened. Hashes/counts below are historical, not
current. Only Linux was built/executed; “non-Unix compiles” below is not a
cross-target receipt and must not be read as certification.

- FINAL written: 2026-11-04T10:09Z (UTC).
- PROVISIONAL (written 2026-11-04T09:50:20+00:00, before any probe or code
  analysis) preserved verbatim at
  `/tmp/sk-fb19-remediation-report.PROVISIONAL.md`, sha256
  `57200e7b036f18e36fea92e5da0687df722e059f0dcdc941001353e1e3d9ff32`.
- Repo: `/home/exedev/.t3/scratch/2026-11-02-pull-current-spec-kitty-from-442fd12a/spec-kitty-rust`
- Published HEAD **unchanged**: `fb19c5c52d92c21d71e55610cc3448b5edc1b544`
  (tree `01925e8a85632515f8ce2b944646b4b80e6cd904`, branch `rust/rewrite`,
  upstream `origin/rust/rewrite`, `+0 -0`). Nothing committed or pushed.
- Status: **implemented, executed and verified in the shared dirty tree; frozen
  for parent integration.** No aggregate / workspace Clippy / workspace docs /
  release / commit / push was run.
- The previous owner's historical report `crates/sk-git/SOURCE-STAGING-REPORT.md`
  is untouched (sha256 `2c329b7522c5750e6168d2c09168f14274a8a063fd32633d4321a03680a00d55`,
  including the coordinator's UNACCEPTED header); the original byte copy and
  sha256 `de7233d2…` remain at
  `/tmp/sk-source-stage-integration.CSHjFu/owner-delivery-report-original.md`.

Evidence root for every command below:
`/tmp/sk-evidence-fb19-remediation-0952/` (38 entries, one fresh `mktemp` log
per attempt, `command-log-index.tsv`; no log or fixture was deleted).

## 1. Source authority read

Actual Python reference: `src/specify_cli/git/commit_helpers.py`
(`safe_commit`, `_stage_requested_files`, `_unstage_requested_files`,
`_restore_staged_patch`, `_run_commit_capture_sha`,
`_assert_committed_paths_requested`). Actual matched Git C: git **v2.43.0**
`builtin/commit.c` (`prepare_index` partial commit: real-index
`add_remove_files`, `refresh_index`, `write_locked_index`; false index from
`HEAD`, `add_remove_files`, refresh, write), `read-cache.c`
(`add_to_index`/`ie_match_stat`/`refresh_cache_ent`,
`fill_stat_cache_info` → `CE_UPTODATE`), `path.c` (`calc_shared_perm`,
`adjust_shared_perm`), `setup.c` (`git_config_perm`), `tempfile.c`/`lockfile.c`.
Fetched files are under `/tmp/git-2.43-src/` (e.g. `commit.c`
`1ec6b9187102f247814fff1f08f36fec5f35d2a2ca4bd6594d9e7177f2fc4148`,
`read-cache.c` `84887aa943a4417dac965a789658f86bb0b097c6a170792aad6468aa1f2fc4d6`,
`path.c` `d10626db89278caaa48084090e5dbceccfaecf4a2e1460b61a88f2d104ee6c51`).

Measured probe (actual Source + identical fixture; `/tmp/probe-stateful.sh`,
logs `probe-stateful-{old,future,now}.*`):

| fixture | Source filter invocations | real index blob | committed blob |
|---|---|---|---|
| worktree mtime 2001 (strictly older than index, non-racy) | 2 | pass 1 | pass 2 |
| worktree mtime 2035 (strictly newer, racy) | 3 | pass 2 | pass 3 |

`git commit --only` re-adds the paths to the **real** index first. A stat-clean
entry is skipped; a racily-clean one is re-hashed (`CE_MATCH_RACY_IS_DIRTY`),
updating the real index. The false index then re-adds once more and the commit
uses that pass. The refresh between them is a no-op for those entries because
`fill_stat_cache_info` marked them `CE_UPTODATE`. The real index therefore
keeps an earlier filter pass than the landed object — an observable `M`
residue, not an invariant to normalize. A replay of the same C steps was also
measured (`probe-prepare-{old,future}.*`) to confirm the branch points.

## 2. What was implemented

### A. Actual `core.sharedRepository` index permissions
`install_index_body` now stats the created sibling (mode `0666 & ~umask`),
applies git's own `calc_shared_perm` adjustment for the effective
`core.sharedRepository` value read through `git config` (same parsing as
`git_config_perm`), and only then renames. `umask`/unset, `group`, `all`,
`0660`, `0666`, compatibility `1`/`2` and explicit negative modes are handled;
directories' execute-bit rule is irrelevant for the index file. No authored
`0644`, no old-index mode copy, no `NamedTempFile` `0600`, no plain
`0666 & umask` claim.

### B. Stateful clean-filter / Source post-commit index
The ordinary route now performs the reference's partial-commit prepare re-add
on the **real** index after the argv boundary (`prepare_operator_index_after_stage`),
with worktree existence checking like `lstat` in `add_remove_files`. A missing
file is skipped (the ordered stage already removed it). This reproduces the
non-racy/racy branch selection by git itself.

The first candidate `write-tree` in a separate process would otherwise treat a
future-dated entry as racily clean and add a fourth filter pass the reference
never runs. `retire_candidate_index_raciness` moves only the **private**
candidate index file's timestamp past the newest staged worktree file, which is
the separate-process equivalent of Source's in-core `CE_UPTODATE`; entries and
body are untouched and the operator index is never touched.

The ordinary route no longer resets requested paths to the landed tree after
success. It reads the commit's changed paths, reports hook-swept extra paths as
`UnrequestedPaths` and reconciles only those extras to the landed sha. The
pinned-parent and index-deletion routes keep their existing requested-path
reset/residue contract. This removes the contradiction that forced Native clean
while Source legitimately kept `M`.

The native-only reset-interposition test
(`reconciliation_uses_the_landed_commit_even_when_head_advances_again`) was
reworked as ordered, Source-grounded coverage: an ordinary pre-commit hook
sweeps an extra tracked path into the commit, and the interposed **extra-path**
reset advances HEAD again after landing; the extra index entry must still be
reconciled to the landed sha (asserted against `HEAD` content). The pinned route
keeps its requested-path interposition. No ordinary reset was forced.

### C. Sibling-index ownership
`install_index_body` now captures and holds the created file's `same_file`
identity for its whole life, resolves the mode through it, verifies the path
still names that identity immediately before the atomic same-directory rename,
and removes a temporary after write/permission/rename failure **only** when the
path still names this attempt's file. A symlink, replaced inode or vanished path
is preserved and the error names the possible leftover. The shared
`path_is_identity` guard is also used by the operator lock's ownership check.
No size/mtime/name-only ownership, no blind unlink, no new dependency.

### D. Process / catch / recovery
- `has_head_commit` now propagates spawn/timeout/capture errors instead of
  `.unwrap_or(false)` claiming an unborn HEAD; a completed non-zero exit is
  still `false`, matching `_run_git_text`.
- `unstage_requested` ignores only a **completed** non-zero unstage (`restore
  --staged` / `rm --cached`); spawn/capture errors propagate, matching the
  reference's direct `subprocess.run` scope (its only swallowed class is a
  `GitCommandError` from the staged-path probe).
- `restore_requested_staging` wraps every internal failure (private-index
  creation, copy, unstage, publish) in `SafeCommitRecoveryFailed` with the
  original failure text, so the original cause is no longer replaced by a bare
  `?` error.
- Before replaying the captured patch, recovery compares the current operator
  index with the exact body this transaction last published. If an
  out-of-band/foreign writer changed it, the foreign/current data is preserved,
  the old patch is not replayed, and the error carries the original failure and
  observed endpoint (the held `.lock` is explicitly not treated as excluding
  out-of-band writers).
- Possibly-landed/unconfirmed CAS, landed repair, lost lock and unconfirmed
  install still never restore.
- The 30 s native bound, one-outer-process vs many-command emulation, and Python
  patch-text `errors="replace"` + universal-newline replay remain documented
  differences, not parity claims.

### E. Truthful docs / tests
`TransactionFailure::MessageRejected` no longer claims the 64 KiB native bound is
a reference over-long-message `E2BIG` class; it is the crate's own payload
refusal and leaves the stage like the reference's post-staging argv boundary.
Module/route docs now describe the real prepare re-add, the real-index residue
and the extras-only post-landing reconciliation; the stale “real index is not
written until the commit exists” and ordinary-route `IndexResidue` claims were
removed. The Source-first shared-mode test now covers
`umask`/`group`/`all`/`0660`/`0666`; helper tests pin `calc_shared_perm` and
`git_config_perm` parsing for umask variants (including `0600`/`0640` base
modes), install replacement/no-sibling/permission behavior and the
identity-replacement fence.

## 3. Chronology (actual first failures retained, not backdated)

| UTC | event | log |
|---|---|---|
| 09:50 | PROVISIONAL report written (hash above) | — |
| 09:51 | git v2.43 C fetched for provenance | `/tmp/git-2.43-src/` |
| 09:52 | Source probe: now/old/future counts and blobs; one wrapper invocation failed (`exit 127`) and is retained as a harness failure | `probe-stateful-now.zz3TFU.log` (127), `probe-stateful-*.BVJFUv/PT6Rvs/ozk8Gs.log` |
| 09:54 | C prepare-step replay probe | `probe-prepare-old.lp0PQU.log`, `probe-prepare-future.XVsJ14.log` |
| 09:56 | first compile after A/B/C: borrow error E0596 (`lock` behind `&`), fixed by not writing the lock body again | `build-lib-after-A-C-B.k12zEK.log` |
| 09:56 | stage suite: 22 passed / 2 failed; permission red now green; stateful failed only on the old contradictory “Native clean” tail; idempotent failed on the old 2-add bound | `stage-suite-after-A-C-B.KuAEXU.log` |
| 09:57 | deterministic old/future stateful rewrite; first run: Native 4 vs Source 3 (separate-process `write-tree` re-hash) | `filter-tests-2.I47FRm.log` |
| 09:57 | private candidate raciness retirement added; old 2/2/2, future 3/3/3, full commit/index equality | `filter-tests-3.ticFUL.log` |
| 09:58 | full 24-test stage suite green | `stage-suite-after-B.LS08Ar.log` |
| 09:58–09:59 | sk-git suite: prepare re-add broke rename/deletion (`pathspec did not match`) and ordinary reconciliation; 2 corpus + 1 differential case also failed; fixed by `lstat`-shaped skip and the Source-grounded reconciliation rework | `sk-git-suite.BmwlZt.log`, then `safe-commit-tests-2.igeT4c.log` |
| 10:00–10:02 | D fixes, docs, unit tests added; full sk-git 422/0; Op 246/0 | `sk-git-suite-4.ldpuoJ.log`, `sk-invocation-suite-2.MfDDWN.log` |
| 10:03 | scoped Clippy: two collapsible-if errors, fixed | `clippy-scoped.Yaidoe.log` |
| 10:03 | scoped Clippy green | `clippy-scoped-2.gzbN8O.log` |
| 10:04 | scoped fmt check green; strict docs (`-D warnings`) green | `fmt-check-2.NQcUca.log`, `docs-strict.IYFvIK.log` |
| 10:05 | pinned oracle exact53: `ok:true`, CPython 3.11.15, no drift/missing | `oracle53.vJw5Ap.log` |
| 10:06–10:08 | final `-p sk-git -p sk-invocation`: **668 passed / 0 failed / 13 ignored** (40 result lines incl. doctests); 32-case corpus green; stage suite repeated green; after the truthfulness rename of the ordinary “resync” test, the same 668/0/13 was re-run and fmt check repeated | `git-op-final.2hnWf8.log`, `final-git-op-after-rename.9aplXn.log`, `corpus32-final.y5YrAc.log`, `stage-repeat-final.ctbVjC.log`, `fmt-check-3.WSkjhZ.log` |
| 10:07–10:08 | freeze status/hashes/processes | `freeze-final.qLyqJ2.log`, `freeze-final-2.Sb39wL.log` |

The 09:42 coordinator business reds were reproduced by this agent before any
fix in `stage-suite-current.qLs80b.log` (22/2, same two failures:
`shared_permissions` left 436/right 432; stateful `cached` left empty/right
`M\tkitty-ops/rec.jsonl`). The later repeated measurement is not presented as
the original pre-fix chronology.

## 4. Acceptance evidence (actual)

- Shared permissions, Source first, then Native: `umask`, `group`, `all`,
  `0660`, `0666` all equal (`shared_repository_index_permissions_match_after_encoding_failure`).
- Stateful clean filter, Source first, 3 repeats per deterministic mtime:
  non-racy `source=2 native=2`, racy `source=3 native=3`; **full commit object
  equal**, full `State` equal, Source and Native `cached` both exactly
  `M\tkitty-ops/rec.jsonl\n`, index blob ≠ committed blob (pass data preserved,
  not stripped).
- Idempotent filter: full post-commit state equal.
- All 24 Source-first stage tests green (argv/Unicode/NUL stage retention;
  linked worktree; symlinked index; corrupt-index capture gate; requested-only
  restore; binary/rename/deletion/mode; HEAD moved by hook; recovery rejection
  with original failure; foreign lock; retarget during staging; unborn control).
- Existing Git controls: 422 passed (baseline 393 + 24 new stage + 5 new
  helper/unit tests). Existing Op controls: 246 passed. Corpus 32 unchanged.
- Static/scoped gates: fmt check, strict scoped docs, scoped
  `clippy -p sk-git -p sk-invocation --all-targets -- -D warnings` all green;
  oracle exact53 green.

## 5. Frozen bindings

Working-tree sha256 (also in `freeze-final-2.Sb39wL.log`; the earlier
`freeze-final.qLyqJ2.log` predates only the test rename, whose new hash is
below):

| file | sha256 |
|---|---|
| `crates/sk-git/src/error.rs` | `bfb3d7332022eb5ec0a46408474deced22236de9fa879a12a5a9cdd4edcc2021` |
| `crates/sk-git/src/lib.rs` | `7fc49222da29fa6cd22d7da0f44e37f0663a05e8072b03dea355062927e77998` |
| `crates/sk-git/src/process.rs` | `00b6e4e8130558aa2063411f6f17b088dcdc2d216a5aecbb6dc49a3a5c82af57` |
| `crates/sk-git/src/safe_commit.rs` | `2526daccccdb364ccb4fa0c8c61fff439e6a0bb5d6cd66cd9558dd791919cfac` |
| `crates/sk-git/src/safe_commit_physical.rs` | `4344c69b7f0c5a623d1d655b017f425c54107997429700f5a15fbebfc88852a1` |
| `crates/sk-git/src/safe_commit_transaction.rs` | `d044eab6feddc5997a87d997f560519e9955c4d58b0f4ac2d5bc2ac6fd5c6068` |
| `crates/sk-invocation/src/op_commit.rs` | `e5dab3ed33c944c589a4cc9d3de864d5b51ddb29cd17b8b3d7ad4f1f45e96884` |
| `crates/sk-git/tests/safe_commit_source_stage.rs` | `588557b1fac6033239de52f7f7f6476b3066516aebd69dd14ed6c8eda694b987` |
| `crates/sk-git/tests/safe_commit.rs` | `963cb22224574b0f9e18f48affa166491283abc6ea891c6c5f752ca437b0173a` |
| `crates/sk-invocation/tests/op_admission_commit.rs` | `ba7328d20844f7e3267d9a7fe1c1e1ef9c6ad63ee85bc3445a1e6e3e83dda2d5` |
| `crates/sk-invocation/tests/op_byte_message.rs` | `7d809378d3eb2c3eebfd1ef11e577db3d80fc39356c15531f71df3e18630fcc9` |
| `crates/sk-git/SOURCE-STAGING-REPORT.md` (previous owner, untouched) | `2c329b7522c5750e6168d2c09168f14274a8a063fd32633d4321a03680a00d55` |

Diff: 10 tracked files changed (`+1661/−425`) plus the two new stage files
(`tests/safe_commit_source_stage.rs`, this report). No manifest, lockfile,
dependency, Source, pin, corpus, golden, Kernel, Workspace or CLI byte changed.
No build/test process remained at freeze (`freeze-final` prints `none`; all
commands were foreground, bounded, no retries by polling).

## 6. Limits / not claimed

- Non-default process umask is exercised through the `calc_shared_perm` helper
  cases and the end-to-end `umask`/`group`/`all`/`0660`/`0666` configs; a
  dedicated child-process non-default-umask end-to-end run was not added (no
  process-global umask mutation and no new dependency were allowed).
- The candidate raciness retirement is a separate-git-process emulation of
  Source's in-core `CE_UPTODATE`; it changes only the private index file's
  timestamp. `refresh_index` of unrelated, non-requested entries is not
  separately emulated (the reference's at-commit refresh is a no-op for the
  just-added requested entries).
- 30 s native bound vs no reference timeout; Python patch text
  `errors="replace"` + universal newlines; one outer `git commit` vs Native's
  several processes; hook spawn failure is not automatically “outer commit
  never started”. These remain documented differences.
- Only Linux was executed; non-Unix code paths compile but were not run.
- No aggregate/workspace Clippy/docs/release, no commit/push, no performance,
  full-command, workflow, StepContract, upstream-718 or accounting claim.

## 7. Parent owns

Fresh independent review, aggregate/release gates, commit and push, and any
broader scope. The working tree is frozen as listed above; HEAD was not moved.
