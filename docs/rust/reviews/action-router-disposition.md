# Pure ActionRouter / canonical role defaults — coordinator integration

Follow-up to published `1514cf029eb9e8ffe0da8709b9ef381119b49cbd`.
**Coordinator-verified new library slice; independent review pending.** Not a
catalogue/executor/CLI/StepContract/workflow or performance approval.
Provisional `/tmp/sk-router-integration-plan.md` preceded integration probes.

## Actual implemented authority

`sk_invocation::ActionRouter` consumes a caller-supplied, registry-ordered typed
snapshot. It performs no IO, activation, model call, glossary observation or Op
write. It preserves empty-catalogue-before-hint, explicit empty hint, canonical
verb vs keyword tiers, stable priority selection, exact reason suffixes,
primary-role/custom-role semantics, first keyword and verb-role order, every
non-winning alternative and tied-top ambiguity. Router hint action is token-
derived; it is NOT the executor's role-default hint branch.

Exact Python code points are retained in profile/role/request/hint/keyword/
reason/error fields, including NUL and lone surrogates. The existing single
`PythonString` gained a minimal additive public seam; private units and checked
constructors remain. Existing legacy JSON parser, recursion/integer budgets,
Op writer/encoder/locale/protection/commit behavior did not change. Error
`message()` is authoritative Source text; Display is diagnostic, not wire.

The eight full role-capability rows now have one native home in sk-doctrine,
actually consumed by the router. Coor approved one LOCAL path edge
`sk-invocation -> sk-doctrine` and its Cargo.lock entry; no new external package,
version/pin, build script or source was adopted. Role capability first verbs
(implementer generate/reviewer audit) and59 router canonical verb mappings are
DIFFERENT Source authorities, not incorrectly merged tables.45 stop words exact.

Word membership/lowercase are CPython3.11.15/UCD14 facts, not Rust's newer
Unicode or casefold. Source generated facts cover all1,114,112 points including
surrogates:133548 word points/734 ranges,1432 lower singles/one expansion,
1112679 identities. Context classes are **decisive-Cased4253/149 ranges** and
**Case_Ignorable2602/427 ranges**; source_Cased actually has4453 points,200 also
ignorable. Both scans skip those200 (e.g.U+0345). The disjoint effective classes
implement Final_Sigma, not full/disjoint Unicode-property definitions.
Numeric data/digests are unchanged from delivery.

No Source/fixtures/packs/corpus/Git/Kernel/Workspace/CLI/pins/inventory change.
Generated facts are development-time only; product never calls Python/unsafe.

## Source coverage / chronology / small coordinator corrections

Owner report `crates/sk-invocation/ROUTER-REPORT.md` is historical-qualified;
original final bytes/mtime `/tmp/sk-router-integration.d6ZqNS/owner-delivery-report-original.md`.
Original provisional `/tmp/sk-router-1514/provisional/ROUTER-REPORT.PROVISIONAL.md`
sha256 `db20b3098282f131a4fa1bd33f2c9d5759f952cadb908cdd85f9875b202e5bef`.
Owner source/implementation logs `/tmp/sk-router-1514/` including final root
`final-20261104T141811Z`. Source tables/fixtures preceded Native comparison.

Real new-feature Sigma mismatches were found during implementation, not a
pre-existing1514 regression: the router was genuinely absent before this slice.
No deliberately wrong ASCII baseline manufactured a business red. Source-model
hypotheses, incorrect emoji/surrogate fixture expectations and compiler/style
failures remain distinguished from behavior failures. No baseline restoration
or passing control was relabelled a historical red.

54 live real ProfileRegistry cases compare ALL decision/error fields and order
via code points, including actual activation/language/legacy-source projections,
quote/control/surrogate hints and IDs. All1,114,112 points compare word/lower/
context data, plus2000+ composed deterministic sequences. Coor adds an
INDEPENDENT direct Source suffix observation `Alpha Sigma X Alpha` for EVERY
code point, not just the same two prefix probes used to generate context data.
`source-context-properties.dEk11k.log` independently confirms actual Cased/
Case_Ignorable overlap and corrects a generator prose error about leading acute.

Coor made `router_unicode` private and moved the five tests from integration
`tests/router_oracle.rs` to `src/router_oracle_tests.rs` with the SAME fail-closed
shared harness (test-only self-crate alias). This avoids shipping a test-only
public API; test count unchanged, no new test binary/suite. No copied production
logic or helper duplication. Removed unused profile-view Default (priority0
would be misleading; callers explicitly supply Source-model priority0..100,
default50). These are inspection-driven refinements, not new runtime reds.
One compile failure from reusing the harness inside the crate was corrected
with a test-only self alias; not a business red. No shared harness changed.

## Stable coordinator receipts

Evidence `/tmp/sk-router-integration.d6ZqNS/command-log-index.tsv`.
No owner edited during aggregate/release gates. Every probe uniquely wrapped;
whole Cargo focused lease/2jobs/no incremental/existing target/isolated roots.
Live Source children assert actual temporary CWD/HOME/allXDG/TMP and exact53.

| Gate | Actual receipt |
| --- | --- |
| Workspace/doctests | **2472/0/19 parent-exercised helpers/194 suites**, `stable-aggregate.Pj5QQ5.log` |
| Git+invocation+doctrine scoped | **890/0/14/45**, Git444 + invocation268 + doctrine178, `scoped-final.fAqoVw.log` |
| Strict workspace all-target Clippy | `strict-workspace.5sIDxG.log`, pass |
| Full fmt | `fmt-final.UeHNI3.log`, pass |
| Strict invocation+doctrine docs | `strict-docs.v1Z3dV.log`, pass |
| Workspace all-target locked/offline | `offline-final.fPOEW8.log`, pass |
| Both release CLIs + optimized invocation | `release-final.wMdCwx.log`, pass |
| Exact CPython3.11.15/53 | `oracle-publish.WNShSl.log`, pass |
| Generator --check | `generator-final-check.QQY1pv.log`, pass |
|11 generator Python controls | `generator-tests-final.HHjpyv.log`, pass |
| Targeted two-file Ruff | `target-python-ruff-final.S7ishS.log`, pass |
| Completion | expected exit1 `completion-final.qTGapR.log`, **599/PERF27/pending/accounting valid/0-of-16** |

Ruff isn't installed in the exact runtime-only53 environment; the first lint
attempt failed setup, not behavior. Used existing `/tmp/pytest-venv/bin/ruff`,
no environment/dependency installation or mutation. Full pre-existing Python
lint debt is not waived. Unit-oracle move explains unchanged194-suite total;
owner's original integration-file path/counts are historical, not silently rebound.

## Current twelve source/test/manifest SHA256 bindings

```text
bec4e709a4516da5d0d35751f7f99fe5bcfaef8dff674c0c520e69ee791c1215  Cargo.lock
c398166f19f7b632f2373b36367899183c12448dbd1b6f2c9aebaa00e43fed72  crates/sk-invocation/Cargo.toml
20705bc9c7dbc3225b19aba4b457e0ec6690f1853132e31703d6d5f276abf160  crates/sk-invocation/src/router.rs
5cdb2209e24efd189a73f3dfd0c79320c298c8eabc525c9b4a241e974a755816  crates/sk-invocation/src/router_unicode.rs
8d2f6e9c25c5f7efcdaa0d2e9dab145c80f00fdd0c0656bca5056ada73041df1  crates/sk-invocation/src/router_unicode_data.rs
a1574f7a2bab482313486d7c2e7d9de98057bd0f84acae046d0c916cf4dd0b14  crates/sk-invocation/src/router_oracle_tests.rs
0f63e4ded7f66965f34a5beef790f42bda673dc452eb5178867798c64688c3e4  crates/sk-invocation/src/legacy_json.rs
1bdc4e463b67678e081cde51ee5e89a270608762894d831b9c542ab3023db7ec  crates/sk-invocation/src/lib.rs
01069f82ab90f766ae399d1547cdc5d530757f167fd7c3839609b98230327cd3  crates/sk-doctrine/src/role_capabilities.rs
3730a9b8159e8e71146c52ee3cb77001d97c7d75ac9999a243930d36e6aef1f5  crates/sk-doctrine/src/lib.rs
9bf1cd46f1715750ab2244ba5041224d88bb54819a081d27ba389cbe18227991  scripts/rust/generate_router_unicode.py
bad80573b3739a315ed63a44d2bb763f3a70fdb21899b85e0e34cb300ef4886d  scripts/rust/tests/test_generate_router_unicode.py
```

## Required, not waived

Catalogue (lenient org-root reuse + ungated legacy overlay), executor/payload/
governance text/hash/model advisory/glossary chokepoint/StepContract and single
plan_advance->commit_advance, owned/lifecycle/CLI/protected-main false advance;
Doctrine P5 casefold/P6 numbers/P7 encoding/nested governance; Source initial
65537/ordinary-delete argv/post-hook publication and unapprovedA3′, other Git/
process/index/codec/platform windows,90 migrations/16 workloads/upstream718
semantic adoption/0-of-16 workflows/genuine measured>=3x. No inventory promotion
or timing-based benchmark claim. PR stays draft, no merge/release.
