# Language composition remediation — coordinator checkpoint

Input: `/tmp/sk-rust-language-binding-review.md`, reviewing published
`1eb5c99efe947319d720d5e1b3765701488eaab4`. Provisional plan written before
edits: `/tmp/sk-language-composition-remediation.md`. The independent review
accepts signal binding/P1 consolidation narrowly; F1–F4 are real remaining
composition/evidence defects. This correction is implementation evidence, not
fresh independent N5/whole-language/next approval.

## Changes and corrected attributions

- **F1:** Runtime now reads root-level org charter graphs and canonical org
  `drg/fragment.yaml` as distinct inputs. Root-graph-only, fragment-only and
  both-shapes packs work; project still reads its canonical root `graph.yaml`
  or sharded graphs. Configured pack names/declaration order are retained from
  typed `OrgPackRoot`, not reconstructed from directory basenames. Source:
  `src/charter/activation/_drg_helpers.py:149-204,253-305` and
  `drg_activation.py:161-172`. The review's blanket project-fragment claim was
  too broad: Python's project input is a root-level DRG graph, not an org-shaped
  project fragment. Live reference controls cover that distinction.
- **F2:** Render actual `context.profile_links` and actual context diagnostics.
  Positive admission requires BOTH the selected profile's unique channel link
  AND its delivered tactic ID, and rejects governance-unavailable output. A
  missing selected profile removes that link and exposes its unresolved
  diagnostic; a governance failure cannot pass the no-signal admission check.
  Changing Zig to Rust also selects the Rust profile pin so the positive channel
  is actually checked, not merely the absence of an exclusion. Both-shapes
  control asserts one link, not duplicate deliveries. These are real model
  outputs, never a success marker fabricated by the CLI.
- **F3:** Test fixture uses canonical plural `agent_profiles`/`tactics`, declares
  profile graph nodes, writes actual tactic artifacts and the compiled mission
  context needed by the reference. Required live `build_charter_context` reads
  that same fixture and confirms the selected profile's citation; missing-profile
  reference control confirms no citation. The withdrawn earlier assertion was
  removed for the WRONG reason: skipped graph composition, not renderer breadth.
- **F4:** Shared lossless `YamlDocument` semantic/syntax parsing removes the
  leading BOM in memory and restores it on render. Original full bytes remain
  the observation/precondition. Read-only composition and actual list mutation
  preserve the BOM, comments, quoting, other values and byte-exact unchanged
  round-trip. `sk-charter::load_project_config` ALREADY supported BOM; the CLI
  control proves it succeeds before exercising the later activation/YAML reader.
  The review's attribution to that strict config reader was wrong; its observed
  governance failure was real. No ad-hoc CLI strip or source-file rewrite.

Native compact text now exposes `Profile Links` and previously hidden context
resolution diagnostics. It is not byte-identical to Python's full text renderer;
no full prompt/section-format parity is claimed. The new reference comparison
checks actual profile-cited delivery, not full `next` composition/Op execution.

## Actual red/green evidence

- `/tmp/sk-language-composition-red.log`: **28 pass / 7 assertion failures**
  through actual `next` after correcting the org fixture, before production
  fixes. Failures include positive cited delivery, missing-profile diagnostic,
  root-graph-only composition and BOM governance.
- Initial BOM round-trip probes with a leading comment/inline key passed
  **1/0**, even though the shell expected a red. Those were non-discriminating
  shapes, NOT defect reproductions; filenames containing `red` do not change
  their outcome. Logs `/tmp/sk-language-bom-{roundtrip,nested}-red.log` retained.
- Real leading-map/no-leading-comment BOM reproducer:
  `/tmp/sk-language-bom-leading-map-red.log`, **0 pass / 1 assertion failure**.
- Scoped fixed CLI **73/0** (35 runtime_next,9 lib,26 compat,3 library binding),
  doctrine **175/0** including doctests. Logs
  `/tmp/sk-language-composition-{cli,doctrine}-green.log`.
- Final both-shapes/canonical-name changes are covered again by the coordinator
  aggregate below. No compiler failure or missing oracle is called native red.

Every entire Cargo invocation uses `bash /tmp/sk-focused-cargo.sh`: existing
target, shared whole-command lease,2 jobs,no incremental,temporary HOME/XDG/TMP.
All implementation owners idle. Stable **2,117 passed / 0 failed / 9
parent-exercised helpers**,181 suites incl doctests; strict workspace all-target
Clippy/full fmt and both release binaries pass without interruption.
Logs `/tmp/sk-native-composition-stable-{aggregate,clippy,fmt,release}.log`.
Includes doctrine175, not added again to the workspace total.

Completion remains **599 findings / PERF27 / pending**, accounting valid,
**0/16 workflows**. `/tmp/sk-composition-stable-completion{.log,-report.json}`,
sandbox `/tmp/sk-composition-completion.vTpcLQ` retained. Oracle/source baseline
9aed3a80 and dependency/pack pins unchanged. No compatibility inventory promotion,
performance campaign/build receipt/platform/full workflow/3x acceptance.

## Fixed source/test bindings

```text
3ad08d5a416861f3ac4f8c17eefdc51357a40d010f6b8cb3801d609d899b22ab  crates/sk-cli/src/compat/runtime.rs
d6a4eaacd702c453ef6403c03c3ffa95aea9b4b89d3a2ee5aa2fee239869a5a6  crates/sk-cli/tests/runtime_next.rs
1540bcf30eb613a2778af589b643bde33d7aaf8105df0181baf36c1288f13092  crates/sk-doctrine/src/yaml.rs
```

P5 Unicode casefold/word categories, P6 float repr and P7 full charter encoding
recovery/provenance/error policy still change live profile admission. Remaining
renderer breadth, nested governance validation, StepContract/protected-main false
advance, unwired Op executor/admission/commit/lifecycle, missing commands/services/
90 migrations/16 workloads and all full completion/performance gates remain open.
Coordinator commits and PUSHES verified checkpoint; no merge or release.
