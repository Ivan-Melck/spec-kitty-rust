---
type: reference
updated: 2026-11-02
audience: software-engineer
---
# Rust migration status

**Status: full-rewrite completion loop is active; not complete.** The previously published foundation passed 455 tests. Continue through [the completion gates](LOOP.md); neither that milestone nor a partial benchmark is a stopping condition. The source baseline is upstream `9aed3a8012cb5eb22c0128b14d525039f95fea7a`. Existing Python source, fixtures, and notices remain intact. Rust never invokes Python at runtime.

## Bounded contexts

| Crate | Implemented scope | Not yet implemented |
| --- | --- | --- |
| `sk-kernel` | Thread-bound process/family locks, native Git topology, bounded/no-follow I/O, durable mode-preserving replacement, generic journal append, clocks/digests, home/pack paths | Remaining kernel modules, all exotic topology/config cases, Windows equivalence and service handover |
| `sk-git` | Bounded reads, CAS refs, branch/worktree lifecycle, remote probes, merge conclusion and private-index safe-commit substrate | Safe-commit CLI wiring/conformance evidence, remote fetch/clone, full protection resolution and recovery |
| `sk-mission` | Identity/inventory, DAG/readiness, planning artifacts/templates/dependency patches; new canonical Mission-write lock integration | Independent lock review, full create/coordination orchestration, requirement gates and lifecycle integration |
| `sk-workspace` | Artifact taxonomy/placement, ownership, lane planning/allocation, journaling and recovery | Coordination seeding/materialization, occupancy, dependency/planning merges and remaining reviewed guards |
| `sk-charter` | Read-only vocabulary, charter/config/pack inspection and contained asset resolution | Full offering API and remaining governance integration |
| `sk-doctrine` | DRG primitives, activation planning/ledger, profiles and action-context substrates; independent review completed | Review findings, transactional shared ledger, org-required floors, full nested context/profile fidelity and synthesis |
| `sk-glossary` | Seed validation, typed terms/senses, deterministic ordering and lookups | Replay, extraction, middleware, activation and mutations |
| `sk-status` | Consumer replay/audit, canonical whole-file writer, lifecycle/envelope migration; new Mission alias/rekey protocol | Remaining writer-review findings, independent lock review, mirrors/projections, full payload models and service delivery |
| `sk-runtime` | Pure planner, significance/RACI, answer authority, workflows and differential goldens | Complete governed composition, retrospective execution and remaining wire behavior |
| `sk-run-store` | Durable run storage, prompts; reviewed nullable identity, repeated answers and indeterminate-index recovery now have regressions | Complete baseline differential corpus, remaining review findings and full next orchestration |
| `sk-install` | Init/assets/skills substrate, identity/ledger, upgrade guards and 16 native migrations; shared ledger-lock contention verified | Init recovery/full CLI delivery, remaining review findings, 120 migrations, profiles/global provisioning and transports |
| `sk-cli` | Native `spec-kitty` compatibility binary plus inspection binary; real partial lifecycle/runtime routes and explicit refusals | Canonical mutation integration, all remaining commands/options and full-workflow equivalence |

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

Only the coordinator runs whole-workspace checks, with `CARGO_BUILD_JOBS=2` on this shared machine. Implementers run their crate tests. Independent reviewers examine fixed completed surfaces, and the coordinator reproduces and fixes findings. The latest integrated Linux checkpoint passed **1,449 tests** (five subprocess helpers ignored in isolation and exercised by parents), strict workspace Clippy, formatting and a fresh release build. Tests ran with temporary HOME/XDG/TMP roots. The 455- and 1,276-test results are historical. Subsequent edits must repeat these gates before publication. Individual subagent results are not a substitute for the aggregate run or independent review. See [verification evidence](VERIFICATION.md).

The FSM tests consume the checked-in upstream conformance corpus; the runtime planner tests reproduce two upstream next-decision fixtures. Neither is blanket upstream compatibility. Existing Python suites have not been claimed green by virtue of Rust checks.

## Remaining delivery sequence

1. Run the inventory-driven completion loop in `LOOP.md`; integrate, test, review, benchmark and continue while any required behavior or speed gate remains pending.
2. Refresh existing draft PR #1 with verified tranches and explicit remaining scope; do not merge automatically.
3. Implement the external events/read/write-service client against pinned contracts and fixture tests; establish canonical reduction and archive conformance before claiming status parity.
4. Add persistent runtime storage with shared process locks, atomic/fsync semantics, stale-plan CAS, authorized human decisions, audit policy and real prompt-file generation.
5. Implement Mission creation, lane/worktree/coordination allocation and ownership/placement using validated transactional plans, with recovery tests.
6. Port full charter execution and verified pack acquisition; preserve user-authored commands, skills and YAML comments.
7. Port acceptance, consolidation, merge/recovery and migration protocols with failure injection and cross-platform tests.
8. Port the remaining CLI/integration surface, install/release packages, benchmark realistic projects, and complete a command-by-command compatibility inventory before retiring Python.

The later items are substantial work, not invisible compatibility shims. The initial draft must remain clearly marked incomplete until they are verified.
