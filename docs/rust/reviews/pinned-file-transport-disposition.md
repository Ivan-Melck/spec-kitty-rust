# Pinned post-hook message files: direct Source transport

Follow-up to `ee60eb5f3eb614e2294f33ef01d7782703b5b7dc`.
**Coordinator-verified bounded fix, not a new independent review.** The prior
one-shot review remains bound to21254 and the earlier small corrections remain
in `one-shot-payload-disposition.md`. Provisional implementation plan
`/tmp/sk-pinned-file-transport-plan.md` preceded probes/code.

## Actual Source and correction

`_safe_commit_with_expected_parent` writes UTF8 COMMIT_EDITMSG before building
its private index; `_build_expected_parent_tree` runs authored hooks and checks
requested tree/bytes; `_create_expected_parent_commit` passes the actual file
to Git's `commit-tree -F`. Source does NOT pre-read/cap/clean that post-hook file
or apply ordinary `git commit` empty-message rejection. Git can read a hook-
replaced symlink. This is file transport, not an ordinary `-m` argv boundary.

Native now passes the pinned file directly to the SAME existing commit-tree
writer, without its redundant cleanup reads/64KiB body cap/ordinary empty check.
An Option distinguishes the un-read pinned file from a materialized ordinary
body; no invented empty body substitutes authored bytes. Full signed/pinned
object creation, expected parent/bytes, re-staged path invariant, real candidate,
CAS/ref/reflog/observed-tip/readback and index repair remain with existing cores.
Pinned reflogs still use the ORIGINAL message, not the hook-written subject.

Ordinary/index-deletion cleanup and empty checks stay unchanged. Public
read-only prepared validation, initial64KiB/empty/NUL rules and one-shot pinned
prevalidation remain unchanged. Consequently initialSource-valid65537 and some
initial empty/NUL pinned cases are STILL real gaps; this is NOT pinned/N3 closure
or approval of Native construction as Source execution.

No synthetic argv probe, candidate precompute publisher, hooksPath replacement,
second writer, dependency/manifest/Source/corpus/pin/inventory/Op-runtime/
Kernel/Workspace/CLI change. The larger A3′ proposal is not approved; separate
`source-argv-design-assessment.md` records why.

## Source-first reds and permanent controls

Evidence `/tmp/sk-source-argv-assessment.2FDb0X/command-log-index.tsv`.
All three controls start with valid initial `subject` and a real expected
parent, pre-staged requested v2/worktreev3 plus unrelated/partial state.

- Hook writes160KiB file, WITHOUT final LF: Source commits EXACT bytes via-F.
- Hook empties the file: Source commits an empty message; Native formerly
  invented an ordinary empty-message failure.
- Hook replaces file with a symlink to an authored worktree message: Source
  commits it and preserves target bytes; Native formerly refused no-follow.

All Source success/class/body/full artifact/reflog premises precede Native.
Pairs bind FULL commit objects, raw HEAD/destination reflogs (original subject),
refs/index/worktree and unchanged real COMMIT_EDITMSG. No raw-stat equality claim.

First run `pinned-file-source-first-red.3j6Cod.log` at13:24:36: empty/symlink
are actual Native business reds; large case instead failed a coordinator Source
expectation that-F would append LF. Corrected to actual Source no-LF behavior,
not normalized bytes. **13:25:16** fresh
`pinned-all-source-first-business-red.4tpmEl.log`: all3 Source premises pass,
then Native fails body cap / empty abort / no-follow respectively, BEFORE any
production edit. Green `pinned-direct-file-green.xKjojM.log`. No later baseline
reproduction relabelled original red, no old log/object/data deleted.

## Stable coordinator gates / bindings

Every probe uniquely wrapped; entire Cargo uses focused lease/2jobs/no
incremental/existing repository target/isolated roots. Actual Source/native
children assert temporary CWD/HOME/allXDG/TMP and exact CPython3.11.15/53.
No other producer edited during aggregate/release gates. Scoped formatter only
changed the two intended files; no foreign formatting or suppressed warnings.

| Gate | Actual receipt |
| --- | --- |
| Workspace incl. doctests | **2447/0/19 parent-exercised helpers/194 suites**, `stable-aggregate.7XWIgW.log` |
| Git+Op scoped | **690/0/14/40**, Git444 + Op246, `scoped-git-op.sUwBpa.log` |
| Strict workspace all-target Clippy | `workspace-strict.QMtzRh.log`, pass |
| Full fmt | `fmt-final.r2CyaN.log`, pass |
| Strict Git+Op docs | `strict-docs.GhSvP0.log`, pass |
| Workspace all-target locked/offline | `offline-all-targets.eqseJW.log`, pass |
| Both release CLIs + optimized Op | `release-final.rNQ0vO.log`, pass |
| Exact oracle | `oracle-final.UqosUS.log`, pass53 |
| Completion | expected exit1 `completion-final.iAgDX4.log`, **599/PERF27/pending/accounting valid/0-of-16** |

Only two source/test overrides; every other ee60 source/test byte unchanged:

```text
1f9324f69b40d4402e3652bee349d65ac639ef412da168495479ecd8cff0d94c  crates/sk-git/src/safe_commit_transaction.rs
cac7314de2d9666c2a1dd1ecc11ecbdb32c86993aaf0d62880b19c7be1ab69cc  crates/sk-git/tests/safe_commit_source_stage.rs
```

## Required, not waived

Initial payload/real argv/E2BIG/total-env fidelity, ordinary/index-deletion
post-hook bodies and reflogs, pinned private filename/hook argv/source-phase
breadth, public construction-vs-execution boundaries, process timeout/capture/
recovery causality, raw stat/TREE/refresh, foreign index/partial-fstat/retarget/
postCAS/unlink windows, codec/OS/Git/FS platforms; full executor/catalogue/router/
StepContract/CLI/protected-main false advance,90 migrations/16 workloads/
0-of-16 workflows, upstream718 semantic adoption and measured>=3x.
No full parity, release, security or performance claim. PR stays draft.
