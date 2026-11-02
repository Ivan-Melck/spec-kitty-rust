# Initial bounded baseline — 2026-11-02

**Verdict: `BLOCKED`.** This run is diagnostic evidence, not a completed
performance acceptance.

The measured strict workloads are far above the 3× target, but the required
full-inventory, Mission show, dependency-planning-with-state, status replay,
governed `next`, event inspection, governance, glossary activation and all
mutating lifecycle workloads are still pending. Pending required work blocks the
gate by design. No partial benchmark can declare the performance target met.

## Run metadata

| Fact | Value |
| --- | --- |
| Started (UTC) | 2026-11-02T19:49:45Z |
| Elapsed | 539.5 s (all 220 timed tasks processed, not truncated) |
| Host | `llama-ramen`, AMD EPYC 9554P, 40 visible CPUs, 62.8 GiB |
| OS | Linux 6.12.113 |
| Load average (1 m) | median 8.92 over samples; range ~2.6–23 (shared VM) |
| Repetitions | 5 per side + 1 warmup; seed `20261102` |
| Rust toolchain | `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `cargo 1.99.1` |
| Rust binary | `8c08853d9bbfcf68512ab3ffa488defd25c8811de6968b5f3d975809264be162` (identical before/after) |
| Repo HEAD | `94fc25c48` |
| Oracle source | `<repo>/src`, 1 463 files, tree digest `64a0cbc4dc9043c5be959f4cc7f44be730b7051fe12f6b104fc83fbd74f9dee7` |
| Oracle Python | CPython 3.11.15, lock-exact `--no-dev` environment |
| Dependency identity | **exact** vs `uv.lock` (52 overlapping distributions, 0 mismatches, 0 unlocked) |
| `uv.lock` sha256 | `15f03342f4caac91c67210cb0bb7c3d4357d7a1736c655f24d257afee1c02420` |
| Fixture integrity | **unchanged** (manifests recomputed after the run) |
| `project-large` fixture | 4 806 files, manifest `6fc296a242c004a79573458ec3f2cbc5ca987b27895102908f82c8c64d43de8d` |
| `runtime-large` fixture | 13 files, manifest `637996f9b0835021361761997d887c219435c569dd7487059e84f4f36f1cacf1` |

Command:

```sh
python3 benchmarks/rust/bench.py run \
  --oracle-python /tmp/spk-bench-lockenv/.venv/bin/python \
  --reps 5 --min-samples 5 --max-seconds 900 \
  --out benchmarks/rust/evidence/2026-11-02-initial
```

`oracle-provision --dir /tmp/spk-bench-oracle` produces an equivalent
lock-exact environment; the interpreter path is recorded, not required.

## Strict, gate-eligible workloads (15)

Ratio = median(Python) / median(Rust); >1 means Rust is faster. CI is a seeded
percentile bootstrap over 2 000 resamples. "Work-only" subtracts the measured
import/launch floor and is diagnostic, not gate evidence.

| Workload | Ratio | 95% CI | Rust median | Python median | Work-only ratio | Noise |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| `runtime.query.step` | 111.8× | 97.4–162.0× | 30.9 ms | 3 454 ms | 36.9× | moderate |
| `runtime.query.blocked` | 126.2× | 88.3–152.5× | 27.4 ms | 3 457 ms | 42.1× | moderate |
| `runtime.query.terminal` | 109.0× | 75.1–169.2× | 32.6 ms | 3 549 ms | 38.0× | high |
| `runtime.query.pending-decision` | 124.2× | 98.5–197.4× | 27.7 ms | 3 441 ms | 41.0× | high |
| `runtime.query.missing-input` | 120.7× | 68.8–149.3× | 29.6 ms | 3 576 ms | 43.0× | high |
| `runtime.query.audit-blocking` | 133.4× | 108.7–163.9× | 26.5 ms | 3 533 ms | 46.9× | moderate |
| `runtime.query.audit-advisory` | 137.8× | 92.0–160.7× | 25.3 ms | 3 487 ms | 47.3× | moderate |
| `runtime.query.unschedulable` | 134.9× | 125.9–170.6× | 24.4 ms | 3 287 ms | 40.2× | moderate |
| `runtime.query.drift` | 140.1× | 105.1–168.4× | 24.3 ms | 3 400 ms | 45.5× | moderate |
| `glossary.lookup.resolved` | 89.2× | 72.2–110.9× | 52.7 ms | 4 702 ms | 67.2× | moderate |
| `glossary.lookup.ambiguous` | 85.1× | 72.2–100.6× | 56.1 ms | 4 774 ms | 64.3× | moderate |
| `glossary.lookup.unknown` | 99.7× | 73.7–109.4× | 46.8 ms | 4 670 ms | 75.4× | moderate |
| `glossary.lookup.no-active-sense` | 79.2× | 72.7–110.3× | 58.1 ms | 4 603 ms | 58.9× | moderate |
| `glossary.lookup.critical-unknown` | 98.8× | 73.8–121.9× | 47.0 ms | 4 648 ms | 74.5× | moderate |
| `glossary.lookup.scoped-team` | 158.8× | 53.7–194.5× | 12.8 ms | 2 040 ms | 68.8× | high |

Geometric mean: **114.4×** over 15 strict, required, work-class workloads.
Every ratio CI lower bound is above 3×, so the gate's confidence test is not
the blocker. The blockers are the pending required workloads.

### What the strict set actually proves

- The pure planner port (`runtime.query`) matches the pinned Python
  `plan_next` decision document exactly for step, blocked, terminal, pending
  decision, missing input, blocking audit, advisory audit, unschedulable and
  drift scenarios, over a 900-step template.
- Glossary seed loading, scope-order resolution, ambiguity/unknown
  classification and severity match the pinned Python primitives exactly, apart
  from the explicitly derived `result.status` aggregation and Rust's constant
  `read_only` metadata.

It does **not** prove Mission inventory parity, status replay, governed runtime
behavior, governance, or any mutation.

## Diagnostics (explicitly not gate evidence)

| Workload | Ratio | Rust median | Python median | Why excluded |
| --- | ---: | ---: | ---: | --- |
| `mission.list.projection` | 50.8× | 50.0 ms | 2 536 ms | Upstream exposes only slug/mid8/friendly name, not the full inventory |
| `mission.graph.projection` | 169.8× | 34.2 ms | 5 809 ms | Edge/dependent subset only; upstream does not publish a topological order |
| `transition.check.*` (5 cases) | 1 278–1 668× | 1.4–2.0 ms | 2.4–2.6 s | Micro: process launch dominates; upstream has no pure CLI check |
| `startup.version` | 3 382× | 1.5 ms | 4 955 ms | Startup measured separately; contract-only |
| `startup.help` | 2 560× | 1.9 ms | 4 945 ms | Startup measured separately; contract-only |

The startup ratios illustrate why startup must not be allowed to represent real
work: Python's CLI bootstrap (typer/rich/pydantic imports and global asset
checks) is ~5 s, while the Rust binary starts in under 2 ms. This is a real
user-facing improvement, but it says nothing about planning or state work.

## Pending required workloads (the gate blockers)

1. `mission.inventory.full` — no upstream full-inventory command.
2. `mission.show` — no upstream equivalent per-mission WP summary.
3. `project.root` — internal upstream helper, no CLI contract; startup-class.
4. `git.inspect` — upstream has helpers but no read-only inspection command.
5. `mission.dependency_planning.full` — upstream includes lane/readiness state;
   Rust is read-only.
6. `status.replay` — no Rust canonical reducer.
7. `runtime.next.governed` — Rust planner is pure; upstream `next` owns the run
   store, workspaces, prompts and gates.
8. `events.inspect` — Rust-specific typed projection; upstream `events tail`
   is a line stream.
9. `governance.charter` — no equivalent upstream document; activation absent.
10. `glossary.activation` — upstream replay/extraction/middleware absent.
11. `lifecycle.mutations` — init/create/implement/review/accept/consolidate/
    merge/recovery absent (Rust is read-only).

`capabilities` is pending but not required: it is Rust's own migration
disclosure, not rewritten upstream behavior.

## Bottleneck candidates

These are measurement leads, not accepted optimizations.

1. **Python import/launch floor dominates wall time.** The runtime planner
   import-only floor is ~2.7 s and the glossary floor is ~1.1 s. The Python CLI
   bootstrap is ~5 s. Any Python-side end-to-end comparison is mostly startup.
   The harness therefore reports work-only ratios as well; those are still
   37–75× in Rust's favor.
2. **Python glossary seed loading** (`ruamel.yaml` + per-term pydantic models)
   costs ~3.4–3.6 s beyond the floor for 6 000 senses. Rust parses the same four
   seeds in 47–58 ms (12.8 ms when only one scope is requested). This is the
   largest measured *work* gap.
3. **Python runtime template parsing** (PyYAML + pydantic) costs ~0.7–0.9 s for
   the 900-step, ~600 KiB template. Rust spends 24–33 ms. Rust's remaining time
   is mostly YAML parsing plus full template validation on every invocation.
4. **Rust planner per-invocation validation.** A frozen template is re-read and
   re-validated for each query. If callers can supply a validated/cached
   template (or a content-addressed cache), latency could drop further, but
   this must not weaken drift detection or validation.
5. **Rust glossary does full-scope loading for a single-term lookup.** All four
   seed files are parsed and all senses materialized. Lazy per-scope loading or
   a bounded single-term path could help, but conflict/ambiguity semantics
   require counting active senses across the requested scopes.
6. **Mission listing (Rust 50 ms, Python 2.5 s).** Rust reads and parses 800
   `meta.json` files. This is already fast; the missing full-equivalence oracle,
   not performance, is the gap.
7. **Transition micro (Rust 1.4–2.0 ms).** Launch-bound. Do not optimize the FSM
   based on this; it is not a throughput measurement.

## Noise and validity caveats

- The VM is shared. Load 1 m ranged from ~2.6 to 23 during the run; median was
  ~8.9. Rust medians of 24–58 ms show relative jitter; four strict workloads (plus
  micro/startup entries) were classified "high" noise. Ratio CIs remain above 3× in every case, but the
  `glossary.lookup.scoped-team` lower bound (53.7×) and
  `runtime.query.missing-input` lower bound (68.8×) are the widest relative
  intervals. A final acceptance run needs a quieter machine or more
  repetitions.
- Five repetitions are enough to detect gross instability, not to certify
  sub-10% differences. The gate blocks when a point estimate clears 3× but its
  CI falls below 3×; it does not block merely because a small absolute sample is
  jittery.
- Floor subtraction is approximate. It is reported as a diagnostic only.
- The Python adapters are development-only oracle tooling. They call pinned
  upstream library functions once per process; they are not a production
  fallback and are never used by Rust.
- Other agents were editing workspace crates during the measurement window. The
  measured binary is identified by sha256 and was byte-identical before and
  after; it is not claimed to represent later source edits. The report records
  the repository dirty state for that reason.
- No mutating workload ran. No post-state comparison exists yet because Rust
  cannot produce one. The harness did verify that the read-only fixtures were
  unchanged after the run.

## Files

| File | Contents |
| --- | --- |
| `report.json` | Full machine-readable report: run, identity, gate, fixtures, per-sample data |
| `summary.md` | Human-readable version of the same report |
| `samples.jsonl` | One JSON object per raw sample (wall/CPU time, order, load, exit) |
| `identity.json` | Source tree, dependency-vs-lock, binary, packs identities |
| `environment.json` | Machine, toolchain, and load context |
| `fixtures.json` | Deterministic fixture recipes and per-file manifests |

The generated fixture trees were removed after measurement to keep the
repository small; the manifests above are sufficient to rebuild them
byte-for-byte from the recipe and seed.
