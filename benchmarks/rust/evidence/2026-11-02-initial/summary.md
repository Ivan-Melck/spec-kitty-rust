# Spec Kitty Rust vs Python benchmark

**Gate verdict: `BLOCKED`**

- blocked: required workload pending: mission.inventory.full — No pinned upstream command emits the Rust inventory document. specify_cli.context.mission_resolver.list_missions_for_selection exposes only slug/mid8/friendly_name; comparing the full document would require reimplementing the Rust projection in the oracle. Tracked as mission.list.projection (diagnostic).
- blocked: required workload pending: mission.show — No pinned upstream command emits the same per-mission WP summary. agent tasks list-tasks/status are lane-and-state views; using them would compare different work. The dependency edge subset is covered diagnostically by mission.graph.projection.
- blocked: required workload pending: project.root — Upstream locates the project root internally (locate_project_root) but exposes no CLI contract for it. A comparison would be startup-class discovery, not a representative workload; it must not be used as gate evidence.
- blocked: required workload pending: git.inspect — Upstream has Git helpers but no read-only inspection command with an equivalent document. ops log / safe-commit are different contracts. Needs a shared inspection contract before timing is meaningful.
- blocked: required workload pending: mission.dependency_planning.full — Upstream planning surfaces (orchestrator-api mission-state, agent tasks status) reconstruct lanes/readiness; Rust mission graph is explicitly read-only and does not evaluate state. Outputs are not equivalent. Edge-only projection is diagnostic.
- blocked: required workload pending: status.replay — Rust sk-status has no canonical reducer. events inspect states it is not a reducer. Upstream specify_cli.status.reducer.reduce / agent status materialize are the oracle, but no native counterpart exists yet.
- blocked: required workload pending: runtime.next.governed — Rust runtime query is a pure planner over an explicit snapshot. Upstream next reads and writes a run store, resolves owned workspaces, emits prompt files, and applies gates; it is a different contract and must be implemented before it can be paired.
- blocked: required workload pending: events.inspect — Rust events inspect is a Rust-specific typed projection (its category partition and torn-tail semantics have no upstream typed inspector). Python events tail is a line stream, not an equivalent contract. Must not be benchmarked as if equivalent.
- blocked: required workload pending: governance.charter — No pinned upstream CLI emits the Rust charter.inspect document. Upstream charter status/context/validate are different contracts; activation/pack acquisition are mutating and unimplemented in Rust.
- blocked: required workload pending: glossary.activation — Rust sk-glossary implements seed validation + lookup only. Upstream glossary event replay/extraction/middleware/activation mutations have no native counterpart.
- blocked: required workload pending: lifecycle.mutations — Rust CLI is read-only: no project init, mission creation, lane/worktree ownership, event writes, acceptance, consolidation, merge, or recovery. These are the workloads that matter most for the final gate and must be native before they are benchmarked.
- warning: high variance (rust cv=0.177) for runtime.query.step
- warning: high variance (rust cv=0.240) for runtime.query.blocked
- warning: high variance (rust cv=0.295) for runtime.query.terminal
- warning: high measurement noise classification for runtime.query.terminal
- warning: high variance (rust cv=0.279) for runtime.query.pending-decision
- warning: high measurement noise classification for runtime.query.pending-decision
- warning: high variance (rust cv=0.343) for runtime.query.missing-input
- warning: high measurement noise classification for runtime.query.missing-input
- warning: high variance (rust cv=0.152) for runtime.query.audit-blocking
- warning: high variance (rust cv=0.238) for runtime.query.audit-advisory
- warning: high variance (rust cv=0.176) for runtime.query.drift
- warning: high variance (rust cv=0.166) for glossary.lookup.resolved
- warning: high variance (rust cv=0.176) for glossary.lookup.unknown
- warning: high variance (rust cv=0.191) for glossary.lookup.critical-unknown
- warning: high variance (rust cv=0.609) for glossary.lookup.scoped-team
- warning: high measurement noise classification for glossary.lookup.scoped-team
- warning: wide ratio CI for glossary.lookup.scoped-team (53.71–194.47x, factor 3.62)
- warning: startup workloads are reported separately and are not gate evidence: startup.version, startup.help
- warning: diagnostic workloads excluded from the gate: mission.list.projection, mission.graph.projection, transition.check.planned-claimed, transition.check.claimed-in_progress, transition.check.for_review-in_review, transition.check.approved-done, transition.check.planned-done-refused

## Measurement context

| Fact | Value |
|---|---|
| Host | llama-ramen |
| CPU | AMD EPYC 9554P 64-Core Processor × 40 |
| Memory | 62.8 GiB |
| OS | Linux 6.12.113 |
| Load at end | {"1m": 7.7490234375, "5m": 9.083984375, "15m": 9.43798828125} |
| Rust toolchain | rustc 1.99.1 (58cf8f9c3 2026-10-13) |
| Oracle Python | Python 3.11.15 (main, Aug  7 2026, 02:25:55) [Clang 22.1.3 ] |
| Repo HEAD | 6cb5cc241 |
| Rust binary sha256 | 8c08853d9bbfcf68512ab3ffa488defd |
| uv.lock sha256 | 15f03342f4caac91c67210cb0bb7c3d4 |
| Dependency identity | exact |
| Packs root | /home/exedev/.local/share/uv/tools/spec-kitty-cli/lib/python3.11/site-packages/packs |

Seed `20261102`, 5 repetitions per side, 1 warmup(s), budget 900.0s, elapsed 539.5s, processed 220/220 timed tasks, truncated=False.

## Gate-eligible strict workloads

| Workload | Class | Equivalence | Ratio | 95% CI | Rust median | Python median | n R/P | Noise | Status |
|---|---|---|---|---|---|---|---|---|---|
| `runtime.query.step` | work | strict | 111.84× | 97.36–161.99 | 30.89 ms | 3.454 s | 5/5 | moderate | ok |
| `runtime.query.blocked` | work | strict | 126.20× | 88.28–152.55 | 27.39 ms | 3.457 s | 5/5 | moderate | ok |
| `runtime.query.terminal` | work | strict | 109.01× | 75.06–169.21 | 32.56 ms | 3.549 s | 5/5 | high | ok |
| `runtime.query.pending-decision` | work | strict | 124.20× | 98.48–197.42 | 27.70 ms | 3.441 s | 5/5 | high | ok |
| `runtime.query.missing-input` | work | strict | 120.72× | 68.80–149.35 | 29.62 ms | 3.576 s | 5/5 | high | ok |
| `runtime.query.audit-blocking` | work | strict | 133.39× | 108.71–163.90 | 26.49 ms | 3.533 s | 5/5 | moderate | ok |
| `runtime.query.audit-advisory` | work | strict | 137.85× | 92.01–160.70 | 25.30 ms | 3.487 s | 5/5 | moderate | ok |
| `runtime.query.unschedulable` | work | strict | 134.94× | 125.93–170.61 | 24.36 ms | 3.287 s | 5/5 | moderate | ok |
| `runtime.query.drift` | work | strict | 140.07× | 105.09–168.43 | 24.27 ms | 3.400 s | 5/5 | moderate | ok |
| `glossary.lookup.resolved` | work | strict | 89.19× | 72.22–110.87 | 52.72 ms | 4.702 s | 5/5 | moderate | ok |
| `glossary.lookup.ambiguous` | work | strict | 85.08× | 72.22–100.61 | 56.12 ms | 4.774 s | 5/5 | moderate | ok |
| `glossary.lookup.unknown` | work | strict | 99.73× | 73.66–109.44 | 46.83 ms | 4.670 s | 5/5 | moderate | ok |
| `glossary.lookup.no-active-sense` | work | strict | 79.18× | 72.68–110.26 | 58.14 ms | 4.603 s | 5/5 | moderate | ok |
| `glossary.lookup.critical-unknown` | work | strict | 98.80× | 73.78–121.87 | 47.04 ms | 4.648 s | 5/5 | moderate | ok |
| `glossary.lookup.scoped-team` | work | strict | 158.80× | 53.71–194.47 | 12.85 ms | 2.040 s | 5/5 | high | ok |

Geometric mean over 15 strict workload(s): **114.414×** (target ≥ 3.0×).

## Diagnostics (not gate evidence)

| Workload | Class | Equivalence | Ratio | 95% CI | Rust median | Python median | n R/P | Noise | Status |
|---|---|---|---|---|---|---|---|---|---|
| `mission.list.projection` | work | projection | 50.76× | 39.83–55.56 | 49.97 ms | 2.536 s | 5/5 | moderate | ok |
| `mission.graph.projection` | work | projection | 169.81× | 134.58–189.30 | 34.21 ms | 5.809 s | 5/5 | moderate | ok |
| `transition.check.planned-claimed` | micro | projection | 1378.23× | 640.38–2679.35 | 1.87 ms | 2.574 s | 3/3 | high | ok |
| `transition.check.claimed-in_progress` | micro | projection | 1277.48× | 466.09–1485.66 | 2.01 ms | 2.571 s | 3/3 | high | ok |
| `transition.check.for_review-in_review` | micro | projection | 1668.26× | 1083.71–3065.20 | 1.45 ms | 2.415 s | 3/3 | high | ok |
| `transition.check.approved-done` | micro | projection | 1637.12× | 1305.67–1890.90 | 1.59 ms | 2.598 s | 3/3 | moderate | ok |
| `transition.check.planned-done-refused` | micro | projection | 1609.89× | 343.07–2143.72 | 1.49 ms | 2.404 s | 3/3 | high | ok |

Projection pairs compare only facts both implementations expose; micro pairs are launch-dominated. Neither is allowed to stand in for a required workload.

## Startup and pending work

| Workload | Class | Equivalence | Ratio | 95% CI | Rust median | Python median | n R/P | Noise | Status |
|---|---|---|---|---|---|---|---|---|---|
| `startup.version` | startup | contract | 3382.07× | 2395.48–5914.15 | 1.46 ms | 4.955 s | 5/5 | high | ok |
| `startup.help` | startup | contract | 2559.97× | 1713.32–3807.75 | 1.93 ms | 4.945 s | 5/5 | high | ok |
| `mission.inventory.full` | pending | pending | — | — | — | — | — | — | No pinned upstream command emits the Rust inventory document. specify_cli.context.mission_ |
| `mission.show` | pending | pending | — | — | — | — | — | — | No pinned upstream command emits the same per-mission WP summary. agent tasks list-tasks/s |
| `project.root` | pending | pending | — | — | — | — | — | — | Upstream locates the project root internally (locate_project_root) but exposes no CLI cont |
| `git.inspect` | pending | pending | — | — | — | — | — | — | Upstream has Git helpers but no read-only inspection command with an equivalent document.  |
| `capabilities` | pending | pending | — | — | — | — | — | — | Rust-specific machine surface with no upstream counterpart. It is Rust's own migration dis |
| `mission.dependency_planning.full` | pending | pending | — | — | — | — | — | — | Upstream planning surfaces (orchestrator-api mission-state, agent tasks status) reconstruc |
| `status.replay` | pending | pending | — | — | — | — | — | — | Rust sk-status has no canonical reducer. events inspect states it is not a reducer. Upstre |
| `runtime.next.governed` | pending | pending | — | — | — | — | — | — | Rust runtime query is a pure planner over an explicit snapshot. Upstream next reads and wr |
| `events.inspect` | pending | pending | — | — | — | — | — | — | Rust events inspect is a Rust-specific typed projection (its category partition and torn-t |
| `governance.charter` | pending | pending | — | — | — | — | — | — | No pinned upstream CLI emits the Rust charter.inspect document. Upstream charter status/co |
| `glossary.activation` | pending | pending | — | — | — | — | — | — | Rust sk-glossary implements seed validation + lookup only. Upstream glossary event replay/ |
| `lifecycle.mutations` | pending | pending | — | — | — | — | — | — | Rust CLI is read-only: no project init, mission creation, lane/worktree ownership, event w |

## Fixtures

| Fixture | Files | Manifest sha256 | Recipe |
|---|---|---|---|
| `project-large` | 4806 | 6fc296a242c004a79573458ec3f2cbc5 | {"events": 20000, "kind": "project", "missions": 800, "seed": 20261102, "terms_per_scope": 1500, "work_packages_per_mission": 4} |
| `runtime-large` | 13 | 637996f9b0835021361761997d887c21 | {"kind": "runtime", "scenarios": 8, "seed": 20261102, "steps": 900} |

## Method and limits

- Fixtures are generated deterministically from the recorded seed; inputs are never copied from a developer working tree and are never mutated.
- Equivalence is asserted on a preflight run *before* any timing, then re-asserted for every sample by hashing the normalised payload. A payload that drifts invalidates the sample.
- Both sides run in randomised, interleaved order. Raw samples include child CPU time, exit status, order index, and load before/after; medians, CV, MAD, outliers, and a seeded bootstrap 95% ratio CI are reported.
- User state is isolated (HOME/XDG/TMPDIR under the run directory). Python bytecode is written to a run-local cache, so `<repo>/src`, the installed tool environment, and real user state are not modified.
- The Python side is the pinned reference executed with `PYTHONPATH=<repo>/src`; when no equivalent CLI command exists, a development-only adapter calls the upstream library once per process. Adapters are oracle tooling, never a production fallback.
- Floor controls (Rust `--version`, Python import-only adapter) decompose total latency into protocol/launch cost and work excess. The gate uses total end-to-end medians; the work-excess ratio is reported as a diagnostic.
- This is a shared VM. Load is recorded, samples are bounded, and a high-variance or truncated run blocks the gate rather than being rounded into a pass.
