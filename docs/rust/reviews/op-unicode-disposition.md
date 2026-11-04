---
type: reference
updated: 2026-11-04
audience: software-engineer
---
# Unicode Op review: narrow acceptance and residual contracts

Independent review `/tmp/sk-rust-op-unicode-review.md`, fixed published
0e3a51b9f0edf11c2ed73cb8e47bdd03e9ec22e6: **NARROW PASS WITH FINDINGS**.
No production changes by reviewer. Profile/doctrine loaded, exact53 oracle,
eight source hashes reproduced; scoped Op178pass/0fail/2additional helpers,
strict scoped Clippy/fmt. Not whole-Op/executor/CLI/platform/performance approval.

## Established independently

- Unicode14 table701 maximal ranges/969596 nonprintable points (2048surrogates),
  membership equal across the full code-point space; scalar repr1,112,064 cases
  and20008 composed strings byte-identical to CPython3.11.15.
- Real bound-metadata service/source29cases:17 both commit with equal actual
  OIDs/full objects/subjects/paths,10 both refuse,2 source-commit/native-refuse
  used-low-surrogate cases. Unused/nested/nonpaired/decoded-key semantics fixed.
- Generator exact/missing/real-lock-drift/altered/CWD controls pass. Earlier
  missing overwritten control log remains missing, not reconstructed as evidence.

Precision correction: review prose says1,112,064 table code points, but retained
`probes/table_check.py` loops `range(0x110000)` = **1,114,112 code points**, including
2048surrogates. Scalar repr really is1,112,064. Coordinator reruns the actual table
checker from isolated CWD/HOME/XDG/TMP with zero mismatches:
`/tmp/sk-unicode-disposition.eFbMFP/table-actual-cwd.P4nHWP.log`. Original checker log
records source-checkout CWD; do not claim every review command had isolated CWD.
This harness-policy qualification does not change observed data; original logs
remain immutable in `/tmp/sk-unicode-review-20261103T234042-2548171/`.

## Findings, not waived

| Item | Disposition |
| --- | --- |
| A | One canonical repr is **within sk-invocation**, not repository-wide. Workspace/Doctrine helpers remain; non-ASCII protection detail and YAML quoted-type naming can differ |
| B/C | Source-used DC80..DCFF commits via OS bytes; nativeUTF8 seam refuses. Protection/path/stage/encoding precedence and source staged-on-error footprint remain real |
| D | Used NUL is valid JSON; source ValueError after staging vs native NUL preflight/no staging. Added decision/durable-data control, NOT footprint parity |
| E | Source repr(Exception) vs native bare/typed diagnostics remains F5; malformed controls pin decision, not exact error text |
| F | Shadow born/unborn physical ref publication and four protected/invalid admission false-admits remain open |
| G | Remote origin/main and upstream/main match; local main stale/unchanged. Docs now say remote explicitly, no foreign worktree branch mutation |
| H | Generated tests hardcoded old range totals. Fixed below; no Unicode pin/table data changes |
| I | Missing control-red content remains an evidence-hygiene finding, never a verified product red |
| J | Used NUL/non-ASCII policy-detail controls now added; they do not bless known mismatches |

## Coordinator H fix and controls

Provisional `/tmp/sk-unicode-review-disposition-plan.md` before edits. Pure generator
render with two ranges/four points emitted test asserts701/969596: true0/1 red
`/tmp/sk-unicode-disposition.eFbMFP/generator-red.TjmUmx.log`. Template now derives
len/total. Real committed Unicode table regenerates BYTE-IDENTICALLY, no pin change.
Six generator controls pass (including new tiny-render consistency); targeted
Python Ruff passes. New public-service used-NUL and non-ASCII invalid-config tests
pass immediately: positive decision/data/read-only validation, not fabricated reds
or closure of precedence/footprint/text gaps.

Stable owner-idle **2230pass/0fail/11helpers**,184 suites/doctests; includes Op180.
Strict workspace all-target Clippy/full fmt/both release CLIs+optimized Op library
pass. Unique log paths in `/tmp/sk-unicode-disposition.eFbMFP/gate-log-index.txt`;
scoped/new controls in same retained root. Exact53 oracle and completion599/PERF27/
pending/valid accounting/0of16 (`completion-report.json`). Main remotes rechecked
de5f4dc5f374170226a5331129c80927946e5d64,705 source-semantic gap. No native/source
inventory or measured-speed promotion.

Current changed bindings (other0e3 bindings unchanged):

```
268ceb6332a0510dbd6cf324ded7f5fd684daa5ab24f6527af960926529de85d  scripts/rust/generate_python_repr_unicode.py
4aab66726e5eea14f95f4e840d4986e2ce0c43965b4b44a5775d8047bf00c0fa  scripts/rust/tests/test_generate_python_repr_unicode.py
9f63b4c86d9b3cb4f7a600536e9106fa4b1d84cbe0a7ebd9354a75bb5c1224f6  crates/sk-invocation/tests/op_admission_commit.rs
```

Small generator correction coordinator-tested, not fresh independent verdict.
Next is actual bounded canonical Git byte-message/target-progress capability,
then Op composition/failure-footprint/unborn/exact-ref work with fresh review.
`git-byte-message-plan.md` records the amended plan, not implementation. Coordinator
commits+PUSHES verified checkpoint; PR stays draft, full completion remains open.
