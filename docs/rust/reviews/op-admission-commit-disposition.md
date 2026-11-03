# Op admission / actual commit: scoped review and open differences

Audience: software-engineer. Updated: 2026-11-03.

Independent review of published `b80e5ab57284769df9b096f32c112c0124408db7`:
`/tmp/sk-rust-op-admission-commit-review.md`. Reviewer loaded Renata and linked
doctrine; production unchanged and clean. **SCOPED PASS WITH FINDINGS for the
exercised seam, explicitly not valid-input parity certification.**

## Established evidence

- Independent real native/Python matrix: admission22/22 decisions and22/22
  read-only (HEAD/refs/status/index/record snapshots unchanged); commit30/33
  decisions. All33 preserve audit bytes, no fabricated OIDs/rewinds or native
  commits where source refuses. Per-record vs doctor-spine path scope, dirty
  archive, alternate/unrelated staging, hooks, truthful landed/unconfirmed
  evidence, branch/config rejudge, cache and unsigned controls exercised.
- Op138 pass/0fail/2 parent-exercised helpers, scoped strict Clippy/fmt and
  Python3.11.15 exact53 oracle reproduced. Seven b80 source hashes match.
- Artifacts `/tmp/sk-rust-op-admission-review-{probes.jsonl,summary.txt,tests.log,oracle.log}`,
  `/tmp/sk-op-probe/run-matrix.sh`. These are independent samples, not full
  commands/platforms/workflows or performance acceptance.

## Findings and disposition

| Finding | Actual source difference | Disposition |
| --- | --- | --- |
| F1 | Bare NaN/Infinity legacy rows: source commits; native refuses invalid record | OPEN commit-decision divergence, not just repr. Implement native loose reader acceptance, not a stricter claim/model parser |
| F2 | Nested map insertion order and overflow1e400 change subjects/commit objects (source inf vs native1e+400) | OPEN functional output/OID differences; keep standard writers separate; no global serde map-order switch |
| F3 | Invalid protection reason adds WorkspaceError path/type prefix | Corrected below using existing canonical policy detail; actual equality test replaces false 'verbatim' assertion |
| F4 | Checked-out topic<NBSP> with local topic shadow: source commits on NBSP branch; native HEAD mismatch refuses | OPEN valid-input commit-decision divergence. Original coordinator NBSP no-shadow control did not cover this. Safe direction is not parity |
| F5 | Source exception repr/type vs native error diagnostics, including invalid/nonUTF8/hook/no-op/landed/nested | OPEN wire/message boundary; preserve native truthful typed landed/uncertain evidence, never fabricate Python execution or erase durable data |

Review objections O1/O3: public module docs now explicitly classify F1/F4 as
source-commit/native-refusal gaps and F2 as commit-object differences, including
1e400. O2: false test wording removed and exact source reason assertion added.
O4: no executor/catalogue/router/glossary/claim/CLI/StepContract/protected-main
false-advance/workflow/platform/≥3× or681-commit reconciliation approval follows.

## Coordinator F3 correction

Provisional remediation `/tmp/sk-op-admission-review-remediation.md` predates edits.
Existing-service exact required-oracle reason assertion fails0/1 at b80:
`/tmp/sk-op-policy-detail-red.log`. No compiler/harness failure substituted.
`WorkspaceError::ProtectionConfig { detail, .. }` already owns the source-facing
reason; Op now carries that detail instead of formatting the wrapper. Canonical
workspace errors unchanged for other callers; no copied config parser or global
policy change. Skip/path reachability/decision semantics remain unchanged.

Scoped green: Op138/0/2 helpers, strict all-target Clippy, scoped fmt;
`/tmp/sk-op-policy-detail-{green,clippy,fmt}.log`. Actual owner-idle aggregate
repeats2188/0/11 helpers,184 suites including doctests; strict workspace all-target
Clippy/full fmt/both release binaries pass with no interruption. Logs
`/tmp/sk-native-op-policy-detail-{aggregate,clippy,fmt,release}.log`.
Current changed bindings:

```
3e19a8b7c79f2dbafd5d814d6d522614db93550e9f1d2dfefd1999fed8b0c646  crates/sk-invocation/src/op_commit.rs
77509270c63651ebbf7f1bca968ed82a5e288c17e6a58495dcd506bb8870e493  crates/sk-invocation/tests/op_admission_commit.rs
```

This small correction is coordinator-tested, not a fresh independent verdict.
Historical b80 integration/owner bindings remain snapshots, not current-source
promises. Coordinator commits and PUSHES verified work; draft PR remains draft.
Next: genuine F1/F2 legacy JSON fixes, source-grounded F4 canonical composition,
then fresh independent review; disclosure alone does not close those findings.
