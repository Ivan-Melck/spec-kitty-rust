---
type: reference
updated: 2026-11-02
audience: software-engineer
---
# Rust migration status

**Status: native foundation implemented, independently reviewed, and locally verified. Not a complete rewrite.** The source baseline is upstream `9aed3a8012cb5eb22c0128b14d525039f95fea7a`. Existing Python source, fixtures, and notices remain intact. Rust never invokes Python at runtime.

## Bounded contexts

| Crate | Implemented scope | Not yet implemented |
| --- | --- | --- |
| `sk-kernel` | Validated ASCII identifiers, project discovery, contained path resolution, bounded reads, no-op-stable atomic replacement | Complete kernel parity, shared locks, Windows identity equivalence, metadata-preserving/directory-durable writes |
| `sk-git` | Read-only repository, branch, commit, status and worktree inspection; bounded subprocesses and lossless Unix path storage (CLI display strings have raw-byte hex companions) | Remote operations, destructive Git policies, branch/worktree allocation |
| `sk-mission` | Canonical ULID identity, strict selectors, metadata inventory, Markdown frontmatter reads, dependency DAG and explicit-state readiness | Creation, authoritative status reconstruction, owned checkouts, artifact placement, Mission acceptance |
| `sk-charter` | Kind vocabulary, config and charter bundle inspection, pack roots/descriptors, asset catalogs and contained resolution | Activation, synthesis, full frozen offering API, DRG evaluation, downloading and pin verification, core-version compatibility evaluation |
| `sk-glossary` | Seed validation, typed terms/senses, deterministic scope ordering, lookup/conflict output | Event replay, extraction pipeline, middleware, activation, seed mutations |
| `sk-status` | Lane FSM conformance and bounded read-only event inspection | Canonical shared reducer, snapshot/archive byte parity, event writes and transactional coordination |
| `sk-runtime` | Pure DAG planner, validated templates/workflows, exhaustive decision payloads, pure result application and stale-plan checks | Persistent runs/cursor locking, full audit significance policy, authorized answers, prompt files, full governed `next` |
| `sk-cli` | Native read-only command adapter and binary acceptance tests | Complete upstream command tree; unsupported commands must fail, not report success |

The available crate READMEs and public API documentation describe individual compatibility differences. A serializable inspection result is not evidence that upstream mutation protocols have been ported. In particular, event inspection does **not** claim to compute authoritative current lanes.

## How Rust improves the design

- Cargo crate dependencies enforce domain boundaries instead of relying exclusively on import-analysis tests.
- Private validated identifier newtypes prevent accidental interchange of raw selectors, path components, and identities; serde revalidates the boundary.
- Exhaustive decision payloads prevent a blocked result from carrying a runnable step or a terminal result from carrying a decision request.
- Decision planning and result application are pure; filesystem, CLI, and Git side effects live in adapters.
- Typed errors distinguish absent data, corrupt data, ambiguity, failed processes, and unsupported behavior.
- RAII owns temporary files and child processes; no unsafe code is allowed in workspace crates.
- Deterministic maps/orderings and compatibility fixtures make output checkable, not merely plausible.

These are design improvements, not measured performance claims. Cross-platform CI is configured; local Linux tests do not establish macOS/Windows success.

## Verification policy

Only the coordinator runs whole-workspace checks, with `CARGO_BUILD_JOBS=2` on this shared machine. Implementers run their crate tests. Independent reviewers examine fixed completed surfaces, and the coordinator reproduces and fixes findings. The aggregate Linux run passes **455 tests**, including 3 compile-only doctests; strict workspace Clippy and formatting checks pass, and the release binary builds. Individual subagent results are not a substitute for that aggregate run. See [verification evidence](VERIFICATION.md).

The FSM tests consume the checked-in upstream conformance corpus; the runtime planner tests reproduce two upstream next-decision fixtures. Neither is blanket upstream compatibility. Existing Python suites have not been claimed green by virtue of Rust checks.

## Remaining delivery sequence

1. Publish the reviewed foundation as an incomplete draft, retaining its limitations and remaining work.
2. Publish a draft PR for this foundation; do not merge automatically.
3. Implement the external events/read/write-service client against pinned contracts and fixture tests; establish canonical reduction and archive conformance before claiming status parity.
4. Add persistent runtime storage with shared process locks, atomic/fsync semantics, stale-plan CAS, authorized human decisions, audit policy and real prompt-file generation.
5. Implement Mission creation, lane/worktree/coordination allocation and ownership/placement using validated transactional plans, with recovery tests.
6. Port full charter execution and verified pack acquisition; preserve user-authored commands, skills and YAML comments.
7. Port acceptance, consolidation, merge/recovery and migration protocols with failure injection and cross-platform tests.
8. Port the remaining CLI/integration surface, install/release packages, benchmark realistic projects, and complete a command-by-command compatibility inventory before retiring Python.

The later items are substantial work, not invisible compatibility shims. The initial draft must remain clearly marked incomplete until they are verified.
