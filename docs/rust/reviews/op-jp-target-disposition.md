# Op J/H/P independent review and path follow-up

## Verdict and limits

Independent `/tmp/sk-rust-op-jp-target-review.md` at published `fc61898c9`:
**PASS for the exercised Op-reachable J/H/P/source-path slice**. All 13 bindings,
Git343/Op233 (576 combined, 9 parent-exercised helpers), strict scoped Clippy/fmt
and exact53 oracle reproduced. Six shadow rows, D1, tag mismatch, actual-P-only
protection, non-heads limitations and isolated born full-object comparison were
checked against live pinned Source. No new Op-reachable false admit/commit/refusal,
data, phase or authority gap found. This is not full Op/platform/CLI/workflow or
performance acceptance. Small follow-up below is coordinator-tested, not a fresh
independent verdict. Historical reports/reviewer evidence remain unchanged.

## F1: propagating directory-probe error, not safe refusal

Source `preflight_commit(Path("sub/../dir"))`, when `dir` really is a directory,
raises **ValueError** from Source `kernel.git.paths.GitPath.parse` before the
`GitCommandError` catch. Native previously returned the safe directory refusal;
an Op-style caller would admit that rather than propagate. Actual Op record/spine
paths cannot contain `..`, but the public shared projection still required repair.

True live-Source/public-native red **0/1**:
`/tmp/sk-jp-review-followup.OQxeQO/relative-probe-class-red.oCx7Y5.log`.
The existing shared Source projection now constructs its validated index-probe
path before the command catch and returns `SourcePathRefusal::Unreadable` on this
non-SafeCommit failure. No change to native planner path rules/raw `GitPath`.
The Source dialect is private to the shared path authority, not copied into Op.

One permanent paired test covers relative `sub/../dir`, missing `sub/../missing`
(still allowed; parser is not run for a non-directory), absolute directory
normalization, `./dir`/redundant separators, root `.`/empty input and an absolute
outside directory. Python Path constructor lexical normalization is distinct from
resolving relative parents/symlinks. Absolute inputs still reuse canonical Kernel
resolution; no speculative containment/protection refusal was added. All paired
assertions use the exact oracle in actual temporary CWD/HOME/all XDG/TMP roots.

## F2: correct the coordinator's source reading

Actual `_commit_op_record` passes `op_path.relative_to(record_root)`; doctor and
admission also pass **relative** paths through `_op_commit_kwargs`. Earlier
coordinator wording claiming actual writer/usually absolute input was wrong.
Review found equivalence for reachable shapes, not evidence that Source uses the
absolute shape. Native now passes the exact relative input instead of keeping an
unnecessary absolute projection. Existing six shadow/D1/tag/P-only/commit controls
remain green. The three absolute library controls remain valid public-API tests,
not executor-shape parity claims. Historical integration header and VERIFICATION
explicitly correct the prior statement; no historical logs were rewritten.

## F3 and evidence hygiene

Commented the load-bearing `.git/x` real-OID disambiguator: without it short HEAD
is `x`, with it `refs/x`. This is a fixture explanation, not a new product red.
Review disclosed two unwrapped early exploratory commands (uncited); the owner
previously disclosed two as well. Coordinator's initial follow-up orientation
`git status` was also outside the wrapper, is not retained/cited evidence and
violated the wrapper rule. All follow-up evidence-bearing probes/gates below use
fresh logs, including repeated labels and failures. No overwrite/cleanup.

## Verification

Evidence root: `/tmp/sk-jp-review-followup.OQxeQO/command-log-index.tsv`.
Every Cargo command uses `/tmp/sk-focused-cargo.sh`: whole-command lease, 2 jobs,
no incremental, existing target and isolated roots. Every recorded probe/gate
uses `/tmp/sk-evidence-run.sh` and a new log.

- Scoped Git+Op **577 passed / 0 failed / 9 helpers**; Git344, Op233.
  `scoped-green.YZ5dhg.log`; scoped strict Clippy `clippy-scoped.xIlwNC.log`.
- Workspace **2,334 / 0 / 14 helpers**, **191 suites including doctests**:
  `stable-aggregate.ByVrib.log`. One new test over parent2333, not nine tests for
  its paired subcases. Kernel199/doctrine175/charter80 unchanged, included totals.
- Strict workspace all-target Clippy `stable-clippy.CCGO05.log`; full fmt check
  `stable-fmt.nNZS3q.log`; both release CLIs and optimized Op library
  `stable-release.EuMTkT.log`: green.
- Exact53 oracle `oracle-final.DZM9Tj.log`: green, CPython3.11.15.
- Parent chained command timed out during completion **after** tests/strict/fmt/
  release/oracle had finished successfully. Partial completion log
  `completion-final.vpjCnM.log` has no terminal receipt; process inspection
  `timeout-process-inspection.WqNqIn.log` showed no live Cargo/rustc/completion.
  Fresh standalone `completion-final.7AETZt.log` then completed exit1 as expected:
  **599 findings / PERF27 / pending / accounting valid / 0-of-16 workflows**.
  Timeout is not completion success and neither log was discarded.
- `upstream-remote-recheck.TflOJi.log`: both main remotes remain
  `de5f4dc5f374170226a5331129c80927946e5d64`. Local main unchanged/stale; Source/
  oracle remains `9aed3a8012cb5eb22c0128b14d525039f95fea7a`, 705 semantic gap.

Current changed code/test SHA-256 bindings (`binding-and-gate-analysis.pjy2FK.log`):

```text
a6f4f6362daceaa283cf9a6010e7d82bd99ad16b87abb6d52ff1e0d9ae1326d3  crates/sk-git/src/safe_commit_paths.rs
1bf35a052ffd6f7281e345386e0b750834d37272b2d31abb3e3d51fe4da20c30  crates/sk-git/tests/source_path_preflight.rs
8fb0e884eb88187261037935931bc6c0f7985b99d5191437ef92a7a7e92e134d  crates/sk-git/tests/source_path_oracle.rs
0475d1e310dec26be4ff034db31240f4ec71405748490cc8c4e7972d5e5d26c3  crates/sk-invocation/src/op_target.rs
c9e6ff53c1a0cfd2ea0f595e380cf2be365ecbf4c7270642fd4faeb8191e7eb8  crates/sk-invocation/tests/op_admission_commit.rs
```

Other eight fc618 integration bindings unchanged. No mutation-backend/Kernel/
Workspace/CLI/parser/OS-encoder/writer/manifest/lock/Source/fixture/pin changes.

## Still required

Unborn root publication/absent-ref CAS; exact-existing-ref and non-heads publication
(including Source-valid `refs/x`, `HEAD`, `-name` shapes); broader Source path/IO/
probe/wording/F5/abbreviation/nonUTF8/platform semantics; Source staging-before-
encoding/index-on-error/missing-spine/NUL footprints; native message/reflog/process
bounds; locale archives/other codecs/nontext environments/wide argv; full invocation/
StepContract/protected-main advancement/CLI/migrations/workflows/upstream semantic
adoption and actual ≥3× performance campaign. Truthful native no-plan/disclosure
is **not** Source-valid publication parity. Next increment must implement the
canonical backend, not add another validator/writer or fake parent/evidence.
