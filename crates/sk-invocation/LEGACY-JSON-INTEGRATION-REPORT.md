---
type: reference
updated: 2026-11-03
audience: software-engineer
---
# Op legacy JSON correction: coordinator integration

Status: historical758a99d41 coordinator snapshot. Independent review now confirms
reported F1/F2 narrowly; later findings/runtime corrections and current bindings:
`LEGACY-RUNTIME-LIMITS-REPORT.md`. Baseline published `2fc7d5a5053da5e9eb47eb4dfbf4c74c5e88e5a9`.
Source/oracle `9aed3a8012cb5eb22c0128b14d525039f95fea7a` unchanged.

## Delivered behavior and canonical boundaries

Private `legacy_json.rs` models only the executor's loose first-object read:
ordered dicts/last duplicate value in first position, exact integer tokens,
float-token conversion with overflow/nonfinite/underflow, JSON strings and
whole-value validation. `started_field` uses its Python truthiness/str. Existing
Python string/float formatters are reused; no runtime Python, dependency, global
serde preserve_order, strict writer/model/CLAIM or first-line splitter change.
All higher admission/policy/branch/cache/SafeCommit/evidence logic is unchanged,
including F3's canonical protection-detail correction.

Reported F1 source-commit/native-refusal for bare NaN/Infinity and F2 nested-map
order/overflow subjects are corrected for the exercised rows. This is not
arbitrary raw JSON equivalence. Source lone-surrogate values/keys, integer-digit
and recursion ceilings and full Unicode printability remain real differences;
unused lone-surrogate fields can still cause native refusal where source commits,
and over-ceiling integers may commit natively where source refuses. F4 NBSP shadow
branch and F5 exception repr/error classification remain open. Safe direction or
disclosure does not close valid-input gaps.

## Red/green and evidence corrections

Provisional coordinator plan `/tmp/sk-legacy-json-integration.md` predates edits.
Owner true red `/tmp/sk-legacy-json-red.log`:3 pass/4 fail through existing
`commit_op_record`, for bare NaN, mixed nonfinite collections, authored map order
and1e400 subject. Other constants/values subsequently have live-oracle green
controls; no invented per-vector first failure. Owner compile E0599 was disclosed
but its log was overwritten by the rerun; original compile evidence is unavailable,
not represented as retained or a behavioral red. Actual behavioral red remains.

Corrected owner arithmetic: **154 =138 baseline +10 unit [9 parser +1 float]
+6 NEW integration**. Filtered7-pass run includes one old control, not seven new
integration tests. Owner source bindings/counts remain historical snapshots in
`LEGACY-JSON-REMEDIATION-REPORT.md`.

Corrected overclaim: same tree/parents/message does NOT alone prove identical
commit objects; all author/committer metadata/timestamps/other headers matter.
Owner successful-case helpers establish native OID==native HEAD and source
subject/decision/path/byte preservation, not blanket cross-side OID identity.
Also corrected source read-error classification: UnicodeDecodeError is outside
_read_started_event's IndexError/OSError/JSONDecodeError catch and reaches the
close's generic exception handler.

## Coordinator additional controls

Both added tests pass immediately; these are positive validation, NOT product
red/fix claims:

- Decoded duplicate profile keys, nested escaped duplicate keys, Unicode scalar
  chunks/surrogate pair/control escapes and an unused NaN field produce the exact
  live-source subject. A malformed unused field refuses without HEAD/byte mutation
  on both sides, proving whole-row validation rather than profile-only extraction.
  `/tmp/sk-legacy-json-coordinator-whole-row.log`.
- Subprocess first establishes source acceptance of800 nested arrays, then
  exercises real native commit without losing the process. Scoped child-only
  author/committer names/emails/dates bind complete objects. Four actual paired
  commits (deep value, NaN, ordered maps, overflowing floats) have equal source/
  native OIDs; last three also compare full cat-file commit bytes. No OID is
  fabricated, no global environment mutated, no ceiling parity inferred.
  Initial depth-only control `/tmp/sk-legacy-json-coordinator-nesting.log`;
  expanded fixed-metadata control `/tmp/sk-legacy-json-object-control.log`.

## Stable gates and completion

Owners idle; every entire Cargo invocation via `/tmp/sk-focused-cargo.sh`, whole
command lease/2jobs/no incremental/existing target/isolated HOME/XDG/TMP.

- **2,206 passed /0 failed /11 parent-exercised helpers**,184 result suites
  including doctests. Includes Op156 (44 unit,4 locks,30 admission/commit integration,
  30 policy,15 custody,4 oracle,2 paths,26 writer,1 doctest), not added again.
- Strict workspace all-target Clippy, full fmt check, both release CLI binaries
  AND optimized sk-invocation library pass, no interruption.
  `/tmp/sk-native-legacy-json-final-{aggregate,clippy,fmt,release}.log`.
  Earlier stable chain retained separately, before expanded object controls.
- Exact oracle Python3.11.15/53 requirements, zero drift/missing/duplicates/
  unsatisfied: `/tmp/sk-legacy-json-stable-oracle.log`.
- Completion **exit1,599 findings/PERF27/pending**, accounting valid,0/16 workflows.
  `/tmp/sk-legacy-json-stable-completion{.log,-report.json}`;
  isolated root `/tmp/sk-legacy-json-completion.DOPcih` retained.
- Fork/upstream main rechecked, both1bff48901c62ac971ba11e286c94a4de214f0752;
  still681-commit semantic gap. No source/pin/manifest/lock/inventory promotion.

## Historical758a99d41 source bindings

```
b4a1dbebf8dae7cdd19121b576e811daf9d992be405c36aa8be5eaae08d8b245  crates/sk-invocation/src/legacy_json.rs
a28c68e3ce2eb34ded2a821d34905343c271063a1d5a6eef440abbd73e7fa085  crates/sk-invocation/src/op_commit.rs
ccd29eb9a51902c875434e4bf4959b6c375f7619b3851a439ef5a19235e11c9a  crates/sk-invocation/src/python_json.rs
c8df30ad8b1854d4f83029c2a3467a43f1175866e9fa7426d71d592176cca4f9  crates/sk-invocation/src/lib.rs
444dc56a14ab3d5da7f79448734947b63849e1383e4e29a0a1a578a8bc471dcc  crates/sk-invocation/tests/op_admission_commit.rs
```

The earlier b80 review accepts only its exercised admission/commit slice; it is
not independent approval of this new parser/correction. Coordinator commits and
PUSHES verified checkpoint, keeps PR#1 draft, then requests fresh correction
review. Full profile executor/catalogue/router/claim/CLI/StepContract, protected-main
false-advance repair, workflows/platforms/services/migrations and current equivalent-
work performance campaign/≥3× acceptance remain open. Test volume/release builds
are not full rewrite or performance acceptance.
