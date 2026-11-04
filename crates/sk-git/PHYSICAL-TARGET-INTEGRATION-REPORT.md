# Canonical physical/unborn Git backend — coordinator integration

Status: **coordinator verified; fresh independent review pending**. Provisional
`/tmp/sk-physical-backend-integration.md` preceded inspection/fixes. Published base
`102f3b81b5f5a76505ef8b2a6f0f5a077d102819`; Source/oracle remains
`9aed3a8012cb5eb22c0128b14d525039f95fea7a` (CPython3.11.15 / 53 requirements).
Owner `PHYSICAL-TARGET-REPORT.md` is a historical pre-integration snapshot,
not current hashes/counts/isolation proof. No Op/CLI publication claim.

## Delivered authority

New exact-`RefName` request/target/applicable-byte-plan/outcome family observes
Born vs Unborn itself. It feeds the **same** planner and transaction as legacy
UTF8/byte requests. Private optional-real-parent state supports dynamic empty
index/tree + `commit-tree` without `-p` + existing `RefChange::create`/raw CAS.
Born routes pin actual physical ref/HEAD; no J/main seed, fake parent/branch/OID,
second hooks/index/object/ref writer or guessed rollback. Exact apply checks full
symbolic HEAD, ref state and captured index. Root uses Git's `(initial)` reflog
subject and preconversion bytes; Git writes HEAD's reflog itself.

Legacy borrowed getter types/data/order remain born-only. New exact request is
path-only: it does not expose pinned-parent/index-deletion/raw-byte-assertion
routes. Existing routes still share the core. `RefName` removes the invented
three-component minimum: actual Git accepts `refs/x`. This is not an assertion
that its UTF8 type represents every Git-legal byte name. `BranchName` unchanged.
Policy stays application-owned: exact names absent from the supplied
`ProtectedBranches` set are not magically protected; one canonical pure decision
core is reused, no new capability/authorization boolean. The Op must first
adjudicate Source J, independently of the publication identity P.

## Integration findings and controls

1. **Legacy public error regression, true 0/1 red.** Shared apply changed
   `PlanStale.what` from `refs/heads/work` to `work`. Permanent public legacy API
   test catches it. Restored full `destination_ref` for stale tip/HEAD errors.
   This is a native API compatibility red, not a fabricated upstream business
   counterexample. Other old error values/routes retain their prior authority.
2. **Actual native child CWD was wrong.** Owner used `env_clear`/isolated HOME
   but the native helper inherited the source crate CWD; verifier did too.
   Permanent actual-CWD/all-XDG/TMP assertions produced two genuine harness
   failures in the matrix/capability test. Explicit temporary CWD fixed them;
   Source driver also asserts actual roots. This is isolation repair, not a
   business red or proof that earlier owner pairs were fully isolated.
3. Corrupt-present ref probe **already passed**: planning errors rather than
   declaring root. Added as positive control, not claimed red/fix. No speculative
   ref-reader hardening/second resolver was added.
4. Added real unborn candidate/CAS-race evidence control: pre-commit publishes
   a real competing seed, native creates a real zero-parent candidate and returns
   `SafeCommitAdvanceUnconfirmed` with exact P/candidate/observed tip. Foreign
   tip, J shadow and authored bytes remain intact; no false success or rollback.
   This is a native evidence control, not Source failure-footprint parity.
5. Extended paired matrix from **8 to 10 actual Source/native rows**: unborn
   `refs/x` with `core.logAllRefUpdates=always`, and unborn NBSP with configured
   signing plus an unusable signer (ordinary Source/native force unsigned).
   All bound full objects/OIDs/parents/tree/refs/**raw reflog bytes**/worktree
   digests/index/status equal. Native destination matches Source HEAD; root has
   zero parent headers and J seed does not move. Legacy `HEAD` literal pair is
   Source helper's explicit target, not evidence that Op's ambiguous short J
   would pass its separate H==J check.
6. Captured alternate-index pair now also compares **raw default index bytes**,
   not only `ls-files` entries. Both sides leave the default bytes unchanged.

All first coordinator failures retained:
`/tmp/sk-physical-integration.86WO9L/coordinator-reds.zjtMu5.log` =
9 passing / 3 failing / 1 helper: one legacy-value red + two harness failures.
The corrupt-ref positive control was already green in that run. After repairs,
`physical-controls-green.Yoxb8M.log`: 13 runnable tests pass + one helper; ten
matrix rows are subcases, not ten additional test-count entries.

## Stable verification

Unique evidence index: `/tmp/sk-physical-integration.86WO9L/command-log-index.tsv`.
Every attempt uses `/tmp/sk-evidence-run.sh`; every whole Cargo command additionally
uses `/tmp/sk-focused-cargo.sh` (whole-command lease, 2 jobs/no incremental,
existing target/isolated roots). No producer active during aggregates.

| Check | Actual result/log |
| --- | --- |
| Git including doctests | **357 / 0 / 8 helpers**, 28 suites; `scoped-git-green.3ryce2.log` |
| Workspace including doctests | **2,347 / 0 / 15 helpers**, **192 suites**; `stable-aggregate.Yy7tYS.log` |
| strict workspace all-target Clippy | green `stable-clippy.6h6mx1.log` |
| full fmt check | green `stable-fmt.zK1pFD.log` |
| both release CLIs + optimized Op library | green `stable-release.lXMZjH.log` |
| scoped locked/offline check | green `offline-focused-check.xt4dBN.log` |
| exact53 oracle | green `oracle-final.nTdOHn.log` |
| completion | expected exit1 `completion-final.xyFsc6.log`: **599 / PERF27 / pending / accounting valid / 0-of-16** |

Arithmetic: base2334 + owner10 + coordinator3 =2347. Git344+10+3=357;
Op233/Kernel199/doctrine175/charter80 unchanged and INCLUDED, not extra totals.
New helper/suite increases totals14→15 /191→192. Corpus32cases/manifest unchanged.
`final-bindings-and-freeze.reJ9EQ.log` verifies unchanged mutation consumers,
Kernel/Workspace/CLI/Source/parsers/OSencoders/writer/deps/lock/fixtures/pins;
both main remotes remain `de5f4dc5f374170226a5331129c80927946e5d64`, local main
stale/unchanged and 705-commit semantic adoption still required.

Current SHA-256 bindings:

```text
a0afe48a298dd000710928becd303249f3c99153fde20ddf0e650e5df87c3970  crates/sk-git/src/refname.rs
6b15a44eb02da149a96623c566d70e451e0fe253701828fb969f0b8a82b89405  crates/sk-git/src/safe_commit.rs
c60c4122261f49817a34fc62acc767c36f0c9418f13cd14d82454ed8c2ee349f  crates/sk-git/src/safe_commit_transaction.rs
2976bf487385f71b3cb98de84608136ad9617638d08ceb4835c606c67a9d2022  crates/sk-git/src/safe_commit_physical.rs
0148fa7768cf955efbad0cd2705cdc71da6fec7cd5682ab386a98c4b4934517e  crates/sk-git/src/lib.rs
83b9c524e525091516b8fa52896f02cd9b0bafaf4a573ca6fa61cbc9bc8796dc  crates/sk-git/tests/safe_commit_physical.rs
bb6f6428cc341f103351abd23651f059ca68fc1063682e2e3aaa863b9d441ff1  crates/sk-git/tests/symbolic_head.rs
```

## Required next

Fresh independent review of this Git slice; then real Op consumer wiring,
without changing Source J/H/P/order/policy/tag/P-only quirks or inventing a
BranchName for non-heads evidence. Current Op still truthfully reports
`SourceAllowedButNativeCannotPublish` for unborn/exact/non-heads shapes. Native
capability is **not** that wiring or Source-valid refusal parity.

Exact index-deletion/pinned/raw assertion APIs, broader Source ref/path/IO/probe/
error/abbrev/nonUTF8/platform behavior; Source stage-before-encoding/index-on-error/
missing-spine/NUL footprints; locale archives/other codecs/nontext settings/wide
argv; full invocation/StepContract/protected-main advancement/CLI/migrations/
workflows/upstream adoption and actual ≥3× performance campaign remain required.
No workflow/platform/performance certification follows from these library gates.
