# Source staging/recovery: independent review and follow-up

> Historical `1a7a62f45` timestamp/coverage follow-up below. The later existing
> one-shot execution-facade correction (F1), current gates and six bindings are
> in [one-shot-source-disposition.md](one-shot-source-disposition.md). Explicit
> read-only construction contracts remain unchanged; this original review is
> not a new verdict on either follow-up.

## Independent review at `f4d662f7c18f018c28f7830a56ad94ba2f09b51d`

**NARROW PASS WITH FINDINGS**, exercised ordinary Op staging/late-message/
requested-patch recovery only. Final `/tmp/sk-rust-source-staging-review.md`,
provisional `/tmp/sk-rust-source-staging-review.provisional.20261104T110157.md`,
evidence `/tmp/sk-source-stage-f4d-review.1519359/command-log-index.tsv`.
Reviewer reproduced Git426/Op246 =672/0/13 helpers/40 suites including7 doctests,
strict scoped Clippy/fmt/docs/offline/exact53 and fail-closed missing root,
all11 bindings/exact17-file scope. Own config/Source foreign-index probes also
pass. Workspace2429/release/completion are coordinator receipts, not reviewer
approval. No whole CLI/platform/process/performance acceptance follows.

## Findings / coordinator actions

- **F1 prepared APIs:** still validate payload before apply. Read-only plan/
  prepare is a Native construction contract, not proof of Source execution
  footprint. Source-compatible one-shot facade behavior needs a separate
  correction; this follow-up does not claim that gap closed or mutate planning.
- **F2 index equality:** proven semantic entries/blob/mode/stage, changed-path
  set, refs, worktree and full commit objects where paired. NOT raw index/stat/
  TREE/cache-tree/refresh parity. Retain this required residual explicitly.
- **F3 equal timestamp boundary:** coordinator turned the potential gap into a
  genuine Source-FIRST runtime red (`equal-timestamp-source-first-red.B0PkCd.log`):
  Source runs3 stateful filter passes, Native2, producing DIFFERENT commit trees.
  The public Source helper accepts an os.PathLike message; its `__fspath__` and
  the equivalent Native late supplier set the actual index mtime equal to the
  requested entry mtime at the argv boundary. This is a helper/callback fixture,
  **not** a claim that ordinary Op string encoding invokes that callback.
  Same-second/equal-nanosecond boundary is deterministic, with no sleeps/races.
  Fix: read the source index through one descriptor and retain its modified
  timestamp on the private copy before Git reads it. Replacing it with copy time
  changed Git's racy-clean branch. Green9mluZf confirms full Source object/state
  equality. No new user-index timestamp mutation by the fix; only the private
  copy receives the observed timestamp. Broader timestamp/stat equivalence is
  still not certified.
- **F4 foreign-current-index recovery:** permanent live Source-FIRST regression
  now captures a pre-existing requested patch, then a rejecting hook writes
  `FOREIGN INDEX BYTES` into the actual index. Both return RecoveryFailed,
  preserve those bytes and worktree data, leave HEAD unchanged/no lock. Native
  also retains original-failure evidence. A positive adoption of reviewer proof,
  not an invented business red or permission to overwrite foreign staging.
- **F5 hygiene:** four DBG print blocks and the MEASURED progress print removed;
  assertion diagnostics and historical measurement logs retained. Structured
  child-result `debug=` fields are evidence, not discarded debug residue.
- **F6 process breach:** first owner's unauthorized aggregate/release runs stay
  disclosed/historical; never relabelled as coordinator receipts.
- **F7 lock modes:** permanent Source-FIRST Linux hook matrix for umask/group/
  all/0660/0666/bare, full commit equality, also run under CHILD umask0077.
  Other raw/signed config real-index coverage remains in the earlier suite.
  No VM-wide umask/environment change; no Windows/macOS certification.

Small findings fixed by coordinator without a new delegate/re-check agent.
The review certifies its fixedf4d subject only, not a new independent verdict
on this runtime timestamp correction.

## Stable follow-up gates / bindings

Evidence `/tmp/sk-source-stage-followup.DBNJMk/command-log-index.tsv`.
Provisional `/tmp/sk-source-stage-review-followup.md` preceded probes/code.

| Gate | Actual |
| --- | --- |
| workspace/doctests | **2432/0/18 helpers/194 suites**, `stable-aggregate.3Oy8iG.log` |
| Git+Op scoped | **675/0/13/40**, Git429+Op246, `scoped-git-op.aOmDbW.log` |
| strict workspace all-target Clippy | `stable-clippy.WfsNQ1.log`, pass |
| full fmt | `stable-fmt.kIdymI.log`, pass |
| strict Git+Op docs | `strict-docs.6LQ8US.log`, pass |
| all-target locked/offline | `locked-offline.JwCmOI.log`, pass |
| both release CLIs + optimized Op | `stable-release.lFZwzu.log`, pass |
| exact CPython3.11.15/53 | `oracle-final.DT2q0T.log`, pass |
| completion | expected exit1 `completion-final.RRc2zJ.log`: **599/PERF27/pending/accounting valid/0-of-16** |

29 stage tests +1 ignored parent-exercised helper; three tests added, helper/
suite totals unchanged. Permanent controls `permanent-review-regressions.SSwvn7.log`,
child0077 `shared-lock-child-0077.94NIlv.log` green. All commands uniquely wrapped,
whole Cargo leased/focused2 jobs/no incremental/existing target/isolated roots.
No manifest/dependency/Source/pin/corpus/inventory mutation; all nonscope files
checked againstf4d. Other9 integration bindings stay unchanged; its11 hashes
are historicalf4d values, not silently current.

```text
59e076eefcf471d61012073e6ea91b8eb04fd6bc570cdcd1d89d8e75d2c10359  crates/sk-git/src/safe_commit_transaction.rs
4a5ff0e99e50bf4c2ba01e0ec7762c1cea3b32a1a193b1c3a69f8efad3936e9a  crates/sk-git/tests/safe_commit_source_stage.rs
```

## Required, not waived

Prepared-message/one-shot footprint (F1), raw stat/TREE/refresh (F2), broader
racy/concurrent timing, process outer-one/native-many spawn/capture/timeout/
restore-causality, foreign-data/missing-index cases, rare partial fstat,
retarget/check-rename/unlink/post-CAS/six-link windows, dynamic config/ACL/gid/
hook/filter/nonUTF8/platform/version/bounds; full executor/CLI/StepContract/
protected-main false advance/workflows/upstream718 adoption and measured≥3×.
Remote mains15ab39216 still match; localmain/baseline9aed unchanged.
