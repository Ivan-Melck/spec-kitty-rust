---
type: reference
updated: 2026-11-03
audience: software-engineer
---
# Rust migration status

**Status: full-rewrite completion loop is active; not complete.** The previously published foundation passed 455 tests. Continue through [the completion gates](LOOP.md); neither that milestone nor a partial benchmark is a stopping condition. The active behavior baseline is upstream `9aed3a8012cb5eb22c0128b14d525039f95fea7a`. Fork `main` matches upstream `main` at `b91a226a15c0054f3bc0bf7e9c844df8a3689245` (fast-forwarded and remotely rechecked); the rewrite still needs the 643-commit semantic reconciliation. The latest two-commit tail after `7c50fd38b` changes neither production source nor dependency/pack pins. The first investigation covered 556 commits through `8ffeb0180`; the 85-commit tail also leaves dependency and pack pins unchanged, but adds review-budget/gate and WP-metadata changes. Reference source and fixtures must be rebound. Existing Python source, fixtures, and notices remain intact. Rust never invokes Python at runtime.

## Bounded contexts

| Crate | Implemented scope | Not yet implemented |
| --- | --- | --- |
| `sk-kernel` | Thread-bound process/family locks, native Git topology, bounded/no-follow I/O, durable mode-preserving replacement, generic journal append, clocks/digests, home/pack paths | Remaining kernel modules, all exotic topology/config cases, Windows equivalence and service handover |
| `sk-git` | Captured operator/private indexes, CAS refs, branch/worktree/merge/sparse/prune primitives; 31 exact corpus cases plus 1 disclosed correction; invocation-bound conclusion evidence; tracked-aware ignore probe (review pending) | G1–G4/B1/B2 independently accepted; capped-history S1 limitation remains; full CLI/protection/recovery composition and platforms |
| `sk-mission` | Identity/inventory, DAG/readiness, planning artifacts/templates/dependency patches; canonical Mission-write locks with renamed-legacy cross-language exclusion | Full create/coordination orchestration, requirement gates and lifecycle integration; rekey migration consumer |
| `sk-workspace` | Reviewed coordination bootstrap/rollback, canonical lanes locks, root-content preservation, locked recovery, journal/merge outcomes, planning/integration | CLI wiring unapproved; merge drivers, remote-default fallback, platform/concurrency breadth; prior Git attribution condition resolved within reviewed scope |
| `sk-charter` | Read-only vocabulary, charter/config/pack inspection and contained asset resolution | Full offering API and remaining governance integration |
| `sk-doctrine` | Activation transactions, all 26 profiles, nested context/floors/links; org-default and canonical validation corrections, 149 scoped tests | Scoped corrections independently accepted; nested model validation, scoped-language vocabulary, offering/compiler/interview and pack lifecycle remain |
| `sk-glossary` | Seed validation, typed terms/senses, deterministic ordering and lookups | Replay, extraction, middleware, activation and mutations |
| `sk-status` | Replay/audit/writers, real YAML mirror, review-cycle files, primary task commits/durability and verified rollback consumers; 64 MiB committed-log probe | Named F1/F2–F6 fixes verified; F1c ownership/concurrency independently approved, not whole-tranche parity; coordination, full pre-review/done/owned-checkout gates, projections and services |
| `sk-invocation` | Unwired Op records/writer/custody, Python-byte JSONL, shared Kernel reference path, 80 scoped tests | First review rejected stage 1; delivered large-record/mode/custody/resolver remediation awaits re-review; admission/catalog/executor/commit/lifecycle/CLI absent |
| `sk-runtime` | Pure planner, significance/RACI, answer authority, workflows and differential goldens | Complete governed composition, retrospective execution and remaining wire behavior |
| `sk-run-store` | Accepted durable store seam, canonical prompt content, previews, mapping/occurrence and board guards, CLI governance composition | Two prompt corrections independently accepted; StepContract absence can still falsely advance; decision-log warnings, remaining prompt fidelity and lifecycle/coordination remain |
| `sk-install` | Reviewed init concurrency/preservation, assets/skills/profiles/session, identity/ledger; 46 native migrations, normalized legacy settlement | 90 migrations, interactive/bundled/global installation, transports and remaining parity boundaries |
| `sk-cli` | Compatibility and inspection binaries; actual dispatch for status emit/lifecycle and task move/mark, plus partial lifecycle/runtime routes | Remaining required commands/options, coordination and commit/review composition, full-workflow equivalence |

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

Only the coordinator runs whole-workspace checks, with `CARGO_BUILD_JOBS=2` on this shared machine. Implementers run their crate tests. Independent reviewers examine fixed completed surfaces, and the coordinator reproduces and fixes findings. The latest stable 2026-11-03 Linux checkpoint passes **2,073 tests / 0 failures / 9 parent-exercised helpers**, 178 suites, strict all-target workspace Clippy, formatting and both release binaries. This includes 80 Op/199 Kernel/276 Git scoped cases; Op remediation and the ignore probe still need independent acceptance. A tool-budget interruption during release was inspected and the release-only continuation passed; both logs are retained. F1c and Git snapshot B1/B2 now have scoped independent approval; Op foundation is rejected pending its recorded remediation. The two next prompt corrections and earlier Git G1–G4 corrections also have scoped acceptance. The init oracle is required/lock-exact instead of silently skipped. Earlier counts, including 1,947 at `f42d5380b`, are historical. Tests use temporary HOME/XDG/TMP roots. Later source edits must repeat the affected gates; these checks do not close StepContract false advance, completion's 599 findings or speed acceptance. Individual subagent results are not a substitute for the aggregate run or independent review. See [verification evidence](VERIFICATION.md).

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
