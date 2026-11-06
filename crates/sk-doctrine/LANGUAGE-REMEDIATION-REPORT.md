# Language input fidelity — coordinator remediation checkpoint

Prepared after `38834239dcccf6171f7f56c0d62036117d11cb4c`.
Provisional plan: `/tmp/sk-language-review-remediation.md` (written before edits).
Input: `/tmp/sk-rust-language-scope-review.md`, scoped PASS with findings for
170-test additive provider, not N5/CLI approval. The review's 286/265/23 totals
are internally inconsistent; no numerical conformance certification is inferred.
Concrete fixtures/findings and reproduced Cargo counts are the useful evidence.

## Corrections made directly, not delegated

- P2: one read-only parser strips a leading UTF-8 BOM before serde_yaml_ng.
  Artifact, configured, compiled and transcript readers retain valid multi-key
  documents. Source bytes are never rewritten. Config/compiled/interview positive
  vectors actually yield python/zig/unknown respectively in the pinned oracle.
- P3: safe-loader language reads skip the entire document containing an unknown
  retained YAML tag, including otherwise plain items. Explicit standard `!!str`
  positive still contributes. Compiled charter reads keep their separate loader
  policy, rather than extending safe-loader refusals to the round-trip accessor.
- P4: additive `sk_charter::load_org_pack_registry` uses the same canonical
  org-block/pack parser as whole-project validation. Shared bounded config I/O
  preserves path/read authority and strips UTF-8 BOM. Registry-only reads ignore
  unrelated charter-pointer/project-namespace errors; `load_project_config`
  remains strict. New direct API tests prove this distinction and identical
  malformed-pack errors, absence/presence semantics and BOM handling. No second
  registry validator or shadow config authority was introduced.
- P8: historical report correctly identifies its pre-image/readme hashes,
  170-test scope and the CLI's full existing catalog/tier/interview copy. It no
  longer calls that copy just a ten-token detector or claims transcript encoding
  matches strict-UTF-8 artifact handling. Historical source hashes are labelled
  as the reviewed inputs; current checkpoint bindings are below.

## Actual failures and verification

`/tmp/sk-language-tolerance-red.log`: **9 pass / 4 assertion failures** through
existing public service entrypoints/live required oracle. The BOM tier test
collects all three differences before asserting; no compile/setup red is passed
off as a native defect. Unknown-tag removal/standard-string positive and org
config cases are discriminating. Scoped green initially **251/0** before three
additional canonical registry API tests. Logs retained under
`/tmp/sk-language-tolerance-{green,clippy,fmt,fmt-check}.log`.

After all implementation owners stopped, every entire Cargo command ran through
`bash /tmp/sk-focused-cargo.sh` (existing target, shared whole-command lease,
2 jobs, no incremental, isolated HOME/XDG/TMP):

- Workspace tests **2,105 pass / 0 fail / 9 parent-exercised helpers**, 180 suites
  including doctests. Includes doctrine174, charter80, Op82, Kernel199, Git278;
  those scoped counts are NOT added to the total again.
- Strict workspace all-target Clippy, whole-workspace fmt check, both release
  binaries: pass, no tool interruption on this chain.
- `/tmp/sk-native-language-stable-{aggregate,clippy,fmt,release}.log` retained.
- Completion still exits1: **599 findings / PERF27 / pending**, accounting valid,
  **0/16 workflows**, two implemented entries/488 required. Native correctness
  gates do not promote compatibility inventory or performance acceptance.
  `/tmp/sk-language-stable-completion{.log,-report.json}` and isolated sandbox
  `/tmp/sk-language-completion.9QLJr1` retained.

No production Python, source/oracle/pin rebinding, dependency/lock change, CLI
binding, whole-stage approval or measured campaign is claimed.

## Source/test bindings for this checkpoint

```text
70ceaf687f3be01a20ad29c1033f8e6af5a3d1b02566f18b1578c1db5cb5dbb3  crates/sk-doctrine/src/language_scope.rs
af2b62962b8956e0c2ef5738b9959e628de0a0667758fffb4b7c2dd9d4ebdf83  crates/sk-doctrine/src/language_vocabulary.rs
7e23e6764c634a761df0109368aeafb1e7c6adcb201b51059d6de19d9803953c  crates/sk-doctrine/src/language_yaml.rs
3b0bb8f96fe2d8fbab19c238765c17228f7edf5653931d29fbc8c726024ca7d0  crates/sk-doctrine/src/lib.rs
a5706ab2bd958480be169728d1df142e727b0382ed1a7348fe0c79c320b09cfa  crates/sk-doctrine/tests/language_scope_oracle.rs
2ee143f8b4fd19dcc6914d052997e8b04eb813655d2ca5fad9473714c1dd0ace  crates/sk-charter/src/config.rs
ecefd8c804fa56386b0ca48a4977ae3fcd8e76e30624830770f85b9ebb7b7a0a  crates/sk-charter/src/lib.rs
64df92a85065a6dc8cc1b0914258f8fb07e052697a5fcf6c1f81e40ca2b6c899  crates/sk-charter/tests/org_registry.rs
```

## Remaining work

P1 blocks binding: delete the CLI's separate catalog/tier/interview implementation,
route its live profile gate through the doctrine service, and consolidate profile
family/sentinel rules. This is a real behavior change, not an import-only cleanup;
it needs CLI differential/acceptance and independent integration review.
P5 Unicode casefolding/word categories, P6 numeric exponent repr, P7 full charter
encoding ingestion/provenance/error propagation remain genuine functional gaps.
The three encoding policies must not be conflated. Platform breadth, logging,
context/profile nested validation, full invocation/StepContract composition,
90 missing migrations,16 absent performance workloads and all workflow/performance
gates remain open. The coordinator's P2–P4 corrections are tested, not freshly
independently approved. No whole language subsystem/CLI parity is asserted.
