---
type: reference
updated: 2026-11-04
audience: software-engineer
---
# Shared PythonString / repr foundation — after a1802cda6

**Coordinator-verified H1a relocation; independent review pending.** Not a profile
reader, catalogue, identity-graph, executor, CLI or performance implementation.
The [transport assessment](profile-transport-design-assessment.md) remains binding:
its colliding surrogate-to-NUL POC is not approved and Source27/Native26 valid
profile loss is still unfixed.

## One authority, existing API retained

`sk-kernel` now defines the single PythonString/InvalidCodepoint type, inherent
repr method, canonical formatter and CPython3.11.15/UCD14 printability table.
Invocation re-exports the same nominal types and callable repr; TypeId agrees
between the two aliases within a build (not a promise of stable TypeId/type_name
across builds). Existing public constructors, getters, error text, Debug/Display,
traits and inherent repr remain. New owned/append seams validate code-point range
or append already-valid data; no public unchecked constructor/private-field hack.

The primitive never pairs surrogates. The existing JSON reader still pairs
adjacent high/low escapes under its own grammar; Source YAML need not pair them.
Checked parser appends preserve its former accepted range and left-to-right
pairing. Router changes only accommodate the checked owned constructor.
No graph refusal, fake scalar ID, second string/formatter, global serde change,
new dependency, manifest/lock, Source, pack/pin, fixture or CLI change.

The 789-line/19566-byte table moves byte-identically, SHA256
`be5a5e3d917d748141f96a61a876dd9bc6f1946cfe95a4c00129834a22875797`.
701 numeric ranges/969596 points and the private router Unicode facts are
unchanged. Coordinator comparison proves repr algorithms identical apart from
namespace/visibility/docs. Generator AST is identical apart from output/doc path;
exact interpreter/UCD/verifier/write/check behavior is unchanged. Its existing
non-check mode writes the table; this is not a newly invented no-overwrite guard.
Existing exhaustive public repr/router oracle files and generator tests are
unchanged. New tests cover alias assignment/TypeId, API/errors, NUL, astral,
all2048 surrogates, unpaired primitive data, ordering, Debug/Display and checked
append failure without mutation.

## Review of delivery and small coordinator corrections

Owner report `crates/sk-kernel/SHARED-PYTHON-STRING-REPORT.md` retains its original
PROVISIONAL prefix (`cc451bdad974588e410315f620499e201423550a5406b54b62a8fd169847f608`).
Original full delivery copied byte-for-byte to
`/tmp/sk-h1a-integration.ISflPp/owner-report-original.md`, SHA256
`b4dc7c0eb4a07fc09c3fb81f0dc38101de074c73870ebd9d3bae3ff46ea4a320`.
Owner evidence `/tmp/sk-h1a-kernel-pythonstring.naJDur1G/command-log-index.tsv`.
Task activity additionally reveals TWO unwrapped orientation commands omitted
from that index: raw `ls/git rev-parse/wc` at15:39:01 and report-directory/absence
`ls` at15:39:04 (exit2). They precede the15:39:08 provisional. Thus its original
“before any probe” / “every attempt indexed” claims are false;15:39:29 was only
the first WRAPPED probe. This is an instruction/evidence chronology breach, not
a business red. Original prefix, timestamps and activity are retained, not
backdated or replaced by later positives. Coordinator's own wrapped receipts
independently establish the actual frozen subject and gates below.
Its combined totals are correct; its per-crate splits are not:

| Receipt | Kernel | Invocation | Doctrine | Total |
| --- | ---: | ---: | ---: | ---: |
| Owner baseline | 199 | 268 | not run | 467 |
| Owner after / coordinator repeat | 208 | 272 | 178 | 658 |

Seven new Kernel plus six new alias tests add13; two existing table unit tests
move Invocation→Kernel. Kernel grows9 and Invocation4; no capability is promoted
by that accounting. The owner's baseline standalone53 receipt follows its tests;
existing test oracle helpers verify the locked interpreter themselves before
returning it. Coordinator standalone53 precedes its own test runs.

Coordinator fixes the disclosed pre-existing Kernel private intra-doc link
with one comment-only edit; strict docs now pass, not an accepted failing gate.
Clarifies primitive-vs-JSON pairing and within-build alias identity in comments.
Targeted Python formatting initially fails on three pre-existing formatting
regions/four hunks, reproduced from HEAD via stdin. Coordinator folds those
format-only changes and proves generator AST/output/table unchanged. Neither
compiler/docs/format failures nor pure-move baseline checks are business reds.
Owner E0432 (accidental repr-json removal, restored), fmt-check, Clippy and
rustdoc first failures remain in its logs; no deliberately wrong baseline was
used. Coordinator first aggregate hits the outer300s timeout without a receipt;
one inspection finds no surviving Cargo/test process. Fresh complete run passes;
partial output is not counted as a gate.

All eight occurrence categories and the exact move were classified before
implementation; [map](../occurrence-maps/python-string-kernel.yaml) retains
immutable historical paths/public callers and records coordinator doc/style
exceptions. No active host Mission metadata or runtime Bulk Edit Gate pass is
fabricated. This is not a terminology/API rename.

## Coordinator gates and evidence

Frozen-owner Linux run: **2485 passed /0 failed /19 parent-exercised helpers /
196 suites including doctests**. Kernel208/Invocation272/Doctrine178 scoped
**658/0/4/39**, original Source oracles included. Git444 is unchanged; Git+
Invocation+Doctrine are894/0/14/46 within the aggregate, not extra passes.
Strict workspace all-target Clippy, full fmt, Kernel+Invocation+Doctrine strict
rustdoc, workspace all-target locked/offline check, both release CLIs and
optimized Invocation library pass. Exact CPython3.11.15/53, repr generator
--check/6 controls, router generator--check/11 controls, targeted Ruff and Python
format pass. Every Cargo command uses the whole-command lease/2jobs/no incremental,
existing repository target and isolated HOME/XDG/TMP wrapper.

Receipts `/tmp/sk-h1a-integration.ISflPp/command-log-index.tsv`:
- `aggregate-complete.UKTiuD.log`; `scoped-final.Yln2Ru.log`
- `strict-final.HeV91S.log`; `fmt-final.U312I7.log`
- `strict-docs-final.HLI0n5.log`; `offline-final.7PZpkJ.log`
- `release-final.JGcpb7.log`; `exact-oracle-final.6mye8K.log`
- `repr-generator-check-final.3bhvLM.log`; `repr-generator-tests-final.Ol2ZcI.log`
- `router-generator-check.gZuF0i.log`; `router-generator-tests.KphD1x.log`
- `targeted-python-ruff-final.G30tqY.log`; `targeted-python-format-final.51OlNI.log`
- `counts-and-relocation-bindings.rmmDVm.log`; final bindings below
- `completion.mfOtp2.log` / `completion-report.json`: expected exit1,
  **599 findings/PERF27/pending/accounting valid/0-of-16 workflows**, unchanged.

No independent approval is rebound from old Unicode/router review subjects to
this relocation. Remote mains remain15ab39216, local main61882b8, Source9aed;
718 upstream commits are unadopted. Profile transport/full catalogue/graph,
executor/governance/glossary/model/StepContract/protected-main advance, remaining
workflows/migrations/services/CLI, Source argv and platform/real-fork gaps and
actual equivalent-work ≥3× campaign remain required. Draft PR; no merge/release.

## Current runtime/tool bindings (SHA256)

Four manifests/lock entries are unchanged; other hashes supersede only their
corresponding older bindings. Earlier review subjects remain historical.

```text
ec422682aa56a9ec12215610fcdd48ca4a8c1673d6ebd53dc890d28ae5ed32c7  Cargo.toml
bec4e709a4516da5d0d35751f7f99fe5bcfaef8dff674c0c520e69ee791c1215  Cargo.lock
dc940a43cbedb7e940101d4771e52857f714900a89955f82c5c2549910c44856  crates/sk-kernel/Cargo.toml
c398166f19f7b632f2373b36367899183c12448dbd1b6f2c9aebaa00e43fed72  crates/sk-invocation/Cargo.toml
70cb20eb96d0601f1c0b8ce08c03b659e2a7a5945dac1a6eb5c8387f3b7436f6  crates/sk-kernel/src/codepoint.rs
dc622bff70d98a3fef16d13e44e75724f5ae3a9ea7ec4feb4e821faa5d3d0faa  crates/sk-kernel/src/python_repr.rs
be5a5e3d917d748141f96a61a876dd9bc6f1946cfe95a4c00129834a22875797  crates/sk-kernel/src/python_unicode.rs
413c8a629b742eb0f5c1eb9e3d5b5436875a5492c22aaf54bed94d4021c6fa00  crates/sk-kernel/src/lib.rs
434534a9cf83509b8d397223da498a0783d520b56c20655e1a1d7e6f5f4e6e41  crates/sk-kernel/src/git_topology.rs
b555f9c08142733e318e5b05accc06c1aa7200072b965c76f5e70815dcc59cc8  crates/sk-kernel/tests/python_string.rs
60a2017b5e726819b7c9a741161d84ef93d0c95b9e565e23d2330a6eef496a4b  crates/sk-invocation/src/legacy_json.rs
c974117463dfcf729c86809bdc80fb943a5a6975e421c8358b9cb7fac9b6d130  crates/sk-invocation/src/python_json.rs
b3733bfcd28047a190d2ed2616ef7826569912bd98d26b426a9147bea80db6bb  crates/sk-invocation/src/lib.rs
23a75a112b83ec7ac5ceaf371f9394e3f55c3fb41e58563bf9188227b73cfd62  crates/sk-invocation/src/router.rs
f4032efee28c66bbe96b181f7b3a980b2c2dce35089aea4a8a33db4ba0106bd2  crates/sk-invocation/tests/python_string_alias.rs
68d6b2d6458ab658ec485b37049d958eb016f1d2834e170b0d4e5ad0e260e703  scripts/rust/generate_python_repr_unicode.py
```

`final-binding-and-provisional-check.5gh0Wy.log` verifies the entire original
owner delivery remains a prefix and its original4226-byte provisional hash
is unchanged. `occurrence-map-final-validation.G9e4mY.log` validates the updated
classification schema in asserted isolated CWD/HOME/allXDG/TMP. This is schema
validation, not a Mission runtime gate or Source behavior parity claim.
