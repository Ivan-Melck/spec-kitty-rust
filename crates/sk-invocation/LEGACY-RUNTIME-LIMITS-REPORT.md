---
type: reference
updated: 2026-11-03
audience: software-engineer
---
# Legacy JSON review and runtime refusal correction

Baseline published758a99d418818e3f73286c4e0ac4feae4a165dfc. Fixed behavior source
9aed3a8012cb5eb22c0128b14d525039f95fea7a unchanged. Provisional plan before edits:
`/tmp/sk-op-legacy-runtime-remediation.md`. No dependency/manifest/pin/source-Python
change, runtime Python, additional writer or higher admission/commit/policy change.

## Independent review and limits

`/tmp/sk-rust-op-legacy-json-review.md` gives narrow PASS WITH FINDINGS:44 actual
service/source rows,24 equal subject/real OID/complete commit-object pairs,12 both
refused,3 repr differences,5 decision differences. Reported F1/F2 fixes accepted,
not arbitrary-JSON or executor/CLI parity. Artifacts `/tmp/sk-review758-run.olnupp`,
`/tmp/sk-review758-deep.9UcQ4J`; original artifacts/reports retained unchanged.

Review package arithmetic contains a typo: listed suites sum156 PASSES plus2
parent-exercised ignored helpers, not154pass+2ignored. Coordinator earlier logs
establish156/0/2; this does not change the independent44-row observations.

Residuals: nested Unicode printability, lone surrogates, default integer ceiling,
caller-dependent recursion and error repr. New unbounded reader also introduced
process stack overflow on source-refused deep input; it simultaneously corrected
pre758's false refusal of source-valid128..~992 nesting. No source-valid depth
crashed in the independent samples. These distinctions are preserved, not waived.

## True reds through existing service

- `/tmp/sk-legacy-runtime-depth-red.log`:0pass/1fail. Source first refuses10000
  nested arrays; native child aborts SIGABRT6/stack overflow instead of returning
  an outcome. Parent survives and retains native fixture
  `/tmp/sk-focused-cargo.KkMuIo/tmp/.tmpIZ2Ua3`, source `.tmpppkXAG` (same parent).
- `/tmp/sk-legacy-runtime-int-red.log`:0/1. Source refuses4301-digit integer;
  native actually commits (real OID in log), violating the default source ceiling.
- Intermediate default-only fix exposed a legitimate startup-setting gap:
  `/tmp/sk-legacy-runtime-env-red.log`:0/1. With existing PYTHONINTMAXSTRDIGITS=0,
  source commits4301 digits while default-only native refuses. Corrected before
  publication, not concealed as a supported-input restriction.

## Correction and positive controls

Exact53/CPython3.11.15 facts in `/tmp/sk-legacy-runtime.cRzmnU/{oracle.log,defaults.json,
env-facts.jsonl}`: default recursion budget1000, integer digits4300. Source files
contain no setrecursionlimit/set_int_max_str_digits calls. A native operational
1000-value-frame guard now prevents process loss and returns refusal. It is NOT a
fixed JSON-format contract or exact emulation of the source's ~992–994 variable
call-stack ceiling. The smaller remaining native-accept/source-refuse band and
custom Python recursion/runtime limit mutation settings remain genuine gaps.

Integer-only digit checking excludes the minus sign and respects existing valid
PYTHONINTMAXSTRDIGITS startup settings, including0(unlimited) and640. Empty means
default; source signed-int parsing/leading ASCII whitespace/minimum640/overflow
rules have measured facts and pure tests. Invalid settings cause this reader to
refuse, not a claim of whole-program CPython startup-error equivalence. No global
environment mutation or new Spec Kitty setting. Float mantissas/exponents are not
subject to the integer ceiling; long float overflow still matches source inf.

Public-service controls cover4301 used/unused refusal with HEAD/bytes retained,
4300 and negative4300 acceptance,4301-digit float acceptance, configured640 boundary
and0 unlimited in scoped child processes, clean10000-depth refusal with full native
HEAD/refs/status/index/record snapshot unchanged. Existing source-valid800 nesting
and four bound-metadata equal-object cases still pass. Reading/guarding never
rewrites durable records or clamps authored numeric values.

## Gates and current bindings

Owner idle, all Cargo via whole-command lease helper/2jobs/no incremental/existing
target/isolated HOME/XDG/TMP. Scoped Op **161pass/0fail/2 helpers**; workspace
**2,211pass/0fail/11 helpers**,184 suites including doctests. Strict workspace
all-target Clippy/full fmt/both release CLI binaries and optimized Op library pass
without interruption: `/tmp/sk-native-legacy-runtime-final-{aggregate,clippy,fmt,release}.log`.
Earlier2209/default-only logs remain separate.
Completion still599/PERF27/pending, accounting valid,0of16 workflows;
`/tmp/sk-legacy-runtime.cRzmnU/{completion.log,completion-report.json}`.

```
e1a5c84e602601162c57bd69381beb5fd7d0ac6f84984ae7b8772ed548d467c2  crates/sk-invocation/src/legacy_json.rs
40b63a8ac99e94927a54d0cd15e8faefa71e6d02995b17b1c721b5125cd39af4  crates/sk-invocation/src/op_commit.rs
5b977f6509aaff50c325862bb8b1f9cbbf4329041f1644e496252b04c7fcef8f  crates/sk-invocation/tests/op_admission_commit.rs
```

Small coordinator corrections are tested, not a new independent verdict. Unicode/
surrogate/error and remaining recursion-band gaps, shadow-branch false admission/
publication, full executor/StepContract/protected-main/CLI/workflows/platforms and
performance acceptance remain open. Main separately synchronized to
ba53b7aa6af82d01e56fdb5e3c9de23194816b76;695-commit semantic gap, latest14 includes
owned-evidence/accept/move-task and orchestrator1.23 changes, unadopted; no pins change.
Coordinator commits+PUSHES verified checkpoint, PR stays draft, no ≥3× claim.
