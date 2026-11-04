# Op physical/root publication — coordinator integration

Status: **coordinator verified; fresh independent review pending**. Provisional
`/tmp/sk-op-physical-integration.md` preceded integration. Published base
`3fabdf29f96d789658e5220246aa853e5f850e39`; Source/oracle
`9aed3a8012cb5eb22c0128b14d525039f95fea7a` unchanged. Owner
`OP-PHYSICAL-PUBLICATION-REPORT.md` is the historical pre-integration snapshot,
not current comment/test bindings. No CLI/full executor acceptance implied.

## Actual service composition

All Source-allowed calls in existing `OpCommitService` now use ONE physical
request/plan/byte-prepare/apply over the accepted Git mutation core. Source's
per-call J/H/P authority remains: shape → roots → H==J → refs/heads/J exists →
relative Source path proof → project OR record policy on RawJ + STANDARD guard,
then exact FULL P as RefName and real physical plan. The empty native protection
projection is the already-allowed J verdict, not a new privilege/P-policy lookup.
Native ProtectedBranches cannot express every J; it is never the Source guard.

Removed only artificial BranchName/non-heads/unborn blockers now implemented by
Git. Genuine native plan/ref representation failures stay explicit native no-plan,
not Source refusal/fake Ready. No J/main seed, guessed parent/OID, second writer or
new repository/index handle. Skip/primary cache/30s/captured index/rejudgment,
relative record/spine input, late codepoint→OS bytes, archives and last outcome
remain in the existing composition.

Evidence keeps `Committed { destination: BranchName, ... }` for representable
physical branches. Nonrepresentable exact refs use `CommittedPhysical` with typed
full P/actual OID/paths/warnings/observed root fact. `was_unborn()` returns None
for the legacy shape, even for its real root commit: that shape does not record
the fact and the accessor does not guess it. Full objects prove zero parents.
`AdvanceUnconfirmed.destination` now explicitly uses FULL P (`refs/...`), retaining
candidate/observed tip including None/detail. Landed repair evidence remains.
Success-only accessors returning None on repair/unconfirmed evidence are NOT
proof that nothing landed; their typed commit/candidate must be inspected.

## Source evidence and first failures

Owner retained five CURRENT-service native runtime refusal reds before wiring:
unborn NBSP, born/unborn refs/x, -name and refs/heads/refs/x. Important chronology:
those tests assert Native success BEFORE their paired Source commit call, so the
red attempts stop before that Source commit executes (Source admission can
already have run). Their later green attempts establish actual Source success
and full paired equality; these are NOT same-attempt Source-commit-first reds.
Earlier independent Source cases also informed expectations, not a substitute
for that chronology. See original logs under `/tmp/sk-op-physical.dgPlMU/`.
Compiler/harness/fixture/count/format failures are not business reds. First unborn
refs/x attempt was a fixture failure; corrected fixture then exposed the actual
native runtime refusal. Original attempts are retained, not reconstructed.

Source-real controls now cover those publications with bound full object/OID/
parents/P tip/J-main unchanged, raw reflog/record/path evidence; doctor root
spine-only/archive unchanged; raw80 root bytes/conversion; RawJ non-heads
protected/invalid born+unborn and project/record union; tag/HEAD ambiguity,
unshadowed unborn Step5 and prior D1/source-order controls. Root has **zero**
parent headers, not a shadow seed parent. Source helper's manual HEAD target is
not substituted for actual Op J/H mismatch.

Native-only controls include captured operator index/default raw bytes unchanged/
unrelated staged data, and real root CAS races (foreign tip and unreadable tip).
They retain actual zero-parent candidate/full P/observed Some or None/detail/
last outcome/authored bytes. These are not Source failure-footprint equality
claims. Read-only admission, old born/byte/locale/JSON/hook/doctor controls remain.

## Coordinator follow-up (no new business-red claim)

- Strict Op rustdoc initially failed in owner's retained `op-doc.nmIWIQ.log` on
  TWO pre-existing public→private links. Corrected to inline implementation names
  in `op_commit.rs` and **doc-only** `python_json.rs`; no parser/repr behavior
  change. Strict `RUSTDOCFLAGS=-D warnings` now passes, no warning suppression.
  This documentation/compiler failure is not a Source business red.
- Clarified success getter docs: None does not deny a known landed repair or
  unconfirmed candidate. No runtime accessor/enum semantics changed.
- Captured-index child now clears ambient environment (retaining explicit PATH/
  isolated roots/index) and requires its parent root instead of silently passing
  when invoked without one. Parent still proves the actual commit. Harness
  strengthening was positive, not a fabricated runtime red/fix.

## Stable verification and bindings

Unique commands: `/tmp/sk-op-physical-integration.UedJO9/command-log-index.tsv`.
Every attempt uses evidence wrapper; every WHOLE Cargo command uses focused
whole-command lease/2 jobs/no incremental/existing target/isolated roots. No
producer active during coordinator aggregates.

| Gate | Actual result/log |
| --- | --- |
| Op tests/doctests | **244/0/3 helpers**, 10 suites; `scoped-op-green.ZJzEqf.log` (before final harness-only strengthening), same 244 in final aggregate |
| Workspace/doctests | **2,358/0/16 helpers**, **192 suites**; `stable-aggregate.z91z1c.log` |
| strict workspace all-target Clippy | green `stable-clippy.bgJAzF.log` |
| full fmt | green `stable-fmt.FoUJIn.log` |
| both release CLIs + optimized Op library | green `stable-release.J2lxsp.log` |
| locked/offline all-target check | green `offline-focused-check.frk9nU.log` |
| strict Op docs, no-deps/offline | green `strict-op-doc-final.j4FA5J.log` |
| exact53 CPython3.11.15 oracle | green `oracle-final.HBRBVH.log` |
| completion | expected exit1 `completion-final.VcXvQY.log`: **599/PERF27/pending/accounting valid/0-of-16** |

Arithmetic: base2347+11=2358; Op233+11=244, new child15→16 helpers, suites unchanged.
Git357/Kernel199/doctrine175/charter80 INCLUDED, unchanged. Final binding/freeze
check `final-bindings-freeze-and-counts.gr7daY.log`: all seven accepted Git bindings
unchanged; Kernel/Workspace/CLI/Source/OSencoders/writer/legacy parser/deps/locks/
fixtures/pins unchanged. python_json differs ONLY by doc-link text (code tokens
checked). Both main remotes still de5f4dc5f374170226a5331129c80927946e5d64;
local main stale/unchanged, 705 semantic gap remains.

```text
78a5d7ead31d3b3c15f39c106385fe5763e476ba68cfed8568f3f75162a3dccf  crates/sk-invocation/src/op_target.rs
cd443e0887f9329880811bc3d07d302b29e3132f7f12f8e7a44153b8aa2baec1  crates/sk-invocation/src/op_commit.rs
a933502c4f56a7cab07090dbbbd7bccdc685d00893049ee327931e4252114e72  crates/sk-invocation/src/op_admission.rs
bbe67fba7cb55a3bafe0f274cc7d0f350c55a60903da960d3702fe61f25c1be9  crates/sk-invocation/src/python_json.rs
aa8f8dafe2816fd99af35ca3ef34d31f7f9d3d360e0527ba0e35d1250da3136b  crates/sk-invocation/tests/op_admission_commit.rs
da04777c23f9b1b1830f03f44333462ead2fe9692fcf11dc8160fc4e3777cf93  crates/sk-invocation/tests/support/mod.rs
```

## Still REQUIRED

Fresh independent Op publication review; COMMIT_EDITMSG/hook argument artifacts,
noncommit failure phase/identity, Source stage7-before8/index-on-encoding-error/
missing-spine/NUL, broader ref/path/IO/probe/F5/abbrev/nonUTF8/platform and message/
reflog/process bounds; locale/archive/other-codec/nontext/wide-argv/construction
context; full catalogue/router/governance/glossary/ProfileInvocation/StepContract/
protected-main advancement/CLI/lifecycle/migrations/workflows, upstream semantic
adoption and real ≥3× equivalent-work performance acceptance. Supported service
publication is not full Op/executor/workflow parity. Native no-plan/disclosure
never waives valid Source inputs; durable/possibly-landed data is never rewound.
