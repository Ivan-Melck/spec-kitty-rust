---
type: reference
updated: 2026-11-03
audience: software-engineer
---
# Unicode repr / Python-string integration

Status: historical0e3a51b9f coordinator snapshot. Independent review now accepts
this correction narrowly; later generator/count/scope corrections and current
bindings: `docs/rust/reviews/op-unicode-disposition.md`. Baseline published
4858240fb62632a7cce67f049f6932aeda6d0bdc; fixed Python source/oracle9aed3a8012cb5eb22c0128b14d525039f95fea7a unchanged. Provisional plan before edits:
`/tmp/sk-unicode-integration.md`.

## Real behavior

One canonical code-point repr primitive WITHIN sk-invocation with &str wrapper uses verified CPython
3.11.15/Unicode14.0.0 printability:701 ranges,969596 nonprintable points including
2048 surrogate units. Native PythonString preserves exact code points, paired/
nonpaired escapes, decoded key equality/duplicate order. Unused surrogate fields/
keys and nested representations now commit like source, instead of false invalid
records. Full1,112,064 Unicode-scalar repr comparison, quote/control/astral/nonprintable
and actual service/locked-source commit controls pass; bound author/committer/date
cases compare real OIDs/full objects, not subjects alone. Table data native-only,
no runtime Python/dependency/serde feature or writer/model/CLAIM change.

Owner unique business reds and historical bindings are retained in
`UNICODE-STRING-REMEDIATION-REPORT.md` and `/tmp/sk-unicode-probe.root/red-*.log`.
One later wrong-expectation control log was overwritten by owner: explicitly missing,
not retained or a business red. Owner177 non-doctest+1doc =178total passes (+17 from
161:10 integration+7unit), helpers2 additional, not177 total.

## Coordinator checks/corrections

- Generator originally verifies with cwd=source checkout. Actual real-verifier
  wrapper (no synthesized successful response) yields4positive/negative controls
  passing, isolation assertion failing: `/tmp/sk-unicode-coordinator.gtyxyG/generator-red.log`.
  Verifier now uses isolated temporary CWD; CPython implementation and Unicode14
  metadata explicitly pinned in addition to Python3.11.15/exact53 closure.
- Five generator unittest controls now pass:actual exact oracle/table, missing
  verifier/no output creation, REAL packaging lock drift before output, altered
  fixture table rejected without overwrite, actual verifier subprocess CWD not
  source checkout. Missing/drift/altered were passing controls, not fabricated
  product reds. `generator-green.log` and `generator-check.log` in same root.
- New generator and its test both pass targeted offline Ruff; full pre-existing
  developer-tool lint debt remains unwaived (`ruff.log`, same root).
- Hex consumption reuses the one existing peek decoder (no duplicated validation).
  Corrected comments that falsely implied source-identical first-surrogate or
  guard/encoding precedence. This is behavior-preserving source cleanup/docs;
  real-source ordering residual is explicit rather than asserted equal.

## Remaining actual capability and precedence gaps

A used top-level low surrogateDC80..DCFF can source-commit: POSIX surrogateescape
passes raw argv bytes; Git's default encoding handling emitsUTF8 and warning.
Native UTF8-only SafeCommitRequest still refuses truthfully. No ad-hoc Latin1/
replacement conversion is used: Git-config encodings/hooks/routes matter. Explicit
canonical byte-message/late-encoding preflight extension is requested, not silently
implemented. Source protection refusal precedes encoding; native encoding failure
precedes the canonical guard. Source can leave requested record staged on encoding
failure; native leaves index untouched. These decision/footprint/error gaps are not
approved by disclosure. Existing1000 operational budget/small recursion band and
runtime-setting limits, F4 judged-vs-physical/shadow false admission, F5 error text,
full executor/StepContract/CLI/workflow/platform/performance remain open.

## Historical0e3a51b9f stable verification / bindings

Owners idle; all entire Cargo via whole-command lease helper/2jobs/noincremental/
existing target/isolated HOME/XDG/TMP. **2228pass/0fail/11 helpers**,184 suites incl
doctests; includes Op178, not extra totals. Strict workspace all-target Clippy/full
fmt/both release CLIs and optimized Op library pass uninterrupted:
`/tmp/sk-native-unicode-stable-{aggregate,clippy,fmt,release}.log`.
Five Python controls/generatorcheck and targeted Ruff pass. Exact53 oracle and
completion599/PERF27/pending/valid accounting/0of16 retained in
`/tmp/sk-unicode-coordinator.gtyxyG/{oracle.log,completion.log,completion-report.json}`.

```
be5a5e3d917d748141f96a61a876dd9bc6f1946cfe95a4c00129834a22875797  crates/sk-invocation/src/python_unicode.rs
850a1eda431ca36afdd10596159b6e4037fcd5a19671530e66561515bcc55b0e  crates/sk-invocation/src/python_json.rs
898dc7ab62a58142d498fd9ca53f688f61f4d18c4c27fa18f6bf54f11a258765  crates/sk-invocation/src/legacy_json.rs
be36ba6729770bcf751fa8e9980802b3ff31542a7cc8363e6a747031d17d01ed  crates/sk-invocation/src/op_commit.rs
5a647a54a4f9acbb37a73875778bb851714e9bcf0ae6cdc55ec1bf2272228d32  crates/sk-invocation/src/lib.rs
292dd5fd2515e50b9ad71156ead103fab5a15ff7f0439c0b86d8d97497a68806  crates/sk-invocation/tests/op_admission_commit.rs
50e85657b97dbdd949eb223ec8b4a02f162c711004af1365826f5e2ad095b562  scripts/rust/generate_python_repr_unicode.py
240125f85646b5c5d9799b8b7954793090a61e33bfc52ec1116a4db17aa3dd17  scripts/rust/tests/test_generate_python_repr_unicode.py
```

Main independently synced/recheckedde5f4dc5f374170226a5331129c80927946e5d64,
705 semantic commits ahead. Latest10 changes owned tracer append/writer, alias
mission protection and tests/docs/Mission records (36files,+1898/-73), no pins.
Actual reference/oracle/source fixtures unadopted. Coordinator commits+PUSHES this
verified checkpoint; PR remains draft. New implementation/corrections not inherited
independent approval from earlier F1/F2; no whole parity, language casefold/word-category
P5, current measured campaign/build receipt or ≥3× promotion.
