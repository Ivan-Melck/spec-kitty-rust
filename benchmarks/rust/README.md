# Rust vs Python benchmark harness

Development-only measurement tooling for the Spec Kitty Rust rewrite. It compares
the native `spec-kitty-rust` binary with the pinned Python reference at the same
source revision, using deterministic fixtures, **output equivalence assertions
before timing**, interleaved randomized repetitions, and machine-readable
evidence.

This directory is not shipped, not imported by Rust, and never provides a
production Python fallback.

> **Current status: the acceptance gate is `BLOCKED`, not passed.**
> A first bounded baseline covers two genuinely equivalent read-only areas
> (pure runtime planning and glossary lookup) and several explicitly labelled
> projections. The required full-inventory, status-replay, governed-`next`,
> governance, event-inspection and mutating lifecycle workloads are still
> pending. Their absence blocks the performance gate regardless of the measured
> ratios. See [evidence/2026-11-02-initial/README.md](evidence/2026-11-02-initial/README.md).

## Quick start

The oracle must be the repository source on `PYTHONPATH` with dependency
versions pinned by `uv.lock`. The pre-installed `spec-kitty-cli` uv tool
environment on this VM does **not** match `uv.lock` (25 overlapping packages
differ), so do not use it as the reference.

```sh
# 1. Build a lock-exact oracle interpreter (network needed once; isolated /tmp dir)
python3 benchmarks/rust/bench.py oracle-provision --dir /tmp/spk-bench-oracle

# 2. Prove the harness itself rejects the usual greenwashing
python3 benchmarks/rust/bench.py selftest

# 3. List workloads and their status
python3 benchmarks/rust/bench.py list

# 4. Run a bounded measurement
python3 benchmarks/rust/bench.py run \
  --oracle-python /tmp/spk-bench-oracle/venv/bin/python \
  --reps 5 --max-seconds 900

# 4a. Fast smoke check (one strict pair, ~30 s)
python3 benchmarks/rust/bench.py run \
  --oracle-python /tmp/spk-bench-oracle/venv/bin/python \
  --workload 'runtime.query.step' --reps 2 --min-samples 2 --max-seconds 60 \
  --diagnostic --out /tmp/spkbench-smoke

# 5. Re-evaluate a saved report without remeasuring
python3 benchmarks/rust/bench.py gate benchmarks/rust/evidence/<run>/report.json
```

`run` never rebuilds the Rust binary. It measures the existing
`target/release/spec-kitty-rust`, records its sha256 before/after, and blocks if
it changed during the measurement window.

Exit codes: `0` = gate `PASS`, `1` = gate `FAIL`, `2` = gate `BLOCKED`.
`--diagnostic` changes a subset run's exit code to `0` when every selected pair
is equivalent and measured; the written report's full-gate verdict remains
`BLOCKED` if required workloads are absent.

### Why a provisioned oracle is required

The pre-installed `uv` tool environment on this VM is **not** lock-exact: it has
25 version mismatches against `uv.lock` (for example pydantic 2.14.2 vs 2.13.3,
click 8.5.0 vs 8.3.3, python-ulid 4.1.0 vs 3.1.0, packaging 26.3 vs 26.2). `pydantic` alone changes validation behavior, so measuring
against it would invalidate the comparison. `bench.py identity` fails closed on
that drift, and `run` blocks the gate unless every installed distribution
matches the lock. `oracle-provision` builds a `--no-dev`, lock-exact environment
without touching the installed tool.

## What is compared, and what is not

Every registry entry declares its equivalence class. The classes are not
interchangeable, and only `strict` work is gate evidence.

| Class | Meaning | Gate evidence? |
| --- | --- | --- |
| `strict` | Both sides compute the same contract. Every compared field is upstream-defined (or maps 1:1 from an upstream typed object). | **Yes** |
| `projection` | Upstream exposes the same underlying facts but not the Rust document's full shape. Only the shared facts are compared (e.g. dependency edges, identity fields). | No — diagnostic |
| `contract` | Startup only: exit status and non-empty output. | No — reported separately |
| `pending` | No equivalent Python contract and/or no native implementation yet. | **Blocks the gate** |

The original eleven Rust inspection commands are **not** one-to-one ports of
Python CLI commands. Misleading pairings are therefore left pending instead of
being timed.

### Currently measured

| Workload | Rust | Oracle | Class |
| --- | --- | --- | --- |
| `runtime.query.*` (9 scenarios) | `runtime query` | `runtime.next._internal_runtime.planner.plan_next` via a dev adapter | strict |
| `glossary.lookup.*` (6 cases) | `glossary lookup` | `glossary.scope/resolution/conflict` via a dev adapter | strict |
| `mission.list.projection` | `mission list` | `context.mission_resolver.list_missions_for_selection` | projection |
| `mission.graph.projection` | `mission graph` | `core.dependency_graph.build_dependency_graph` + `get_dependents` | projection |
| `transition.check.*` | `transition check` | `status.wp_state.wp_state_for(...).check_transition(...)` | projection, micro |
| `startup.version`, `startup.help` | binary | `python -m specify_cli` | contract |

Projections are deliberately excluded from the geomean. Micro workloads are
launch-dominated and also excluded.

### Pending required workloads (each blocks the gate)

| Workload | Why it is pending |
| --- | --- |
| `mission.inventory.full` | No pinned upstream command emits the full inventory document (identity, number, type, topology, meta). Upstream listing exposes only slug/mid8/friendly name. |
| `mission.show` | No upstream command emits the same per-mission WP summary; `agent tasks list-tasks/status` are lane/state views. |
| `mission.dependency_planning.full` | Upstream planning surfaces reconstruct lanes/readiness; Rust `mission graph` is explicitly read-only. Edge-only comparison is diagnostic. |
| `status.replay` | Rust has no canonical reducer; `events inspect` is explicitly not a reducer. Upstream authority is `status.reducer.reduce` / `agent status materialize`. |
| `runtime.next.governed` | Rust `runtime query` is a pure planner over an explicit snapshot. Upstream `next` owns a persistent run store, workspace resolution, prompt emission and gates. |
| `events.inspect` | Rust's typed category/torn-tail projection has no upstream typed inspector; `events tail` is a line stream. |
| `governance.charter` | No upstream CLI emits the Rust `charter inspect` document; upstream `charter status/context/validate` are different contracts; activation is unimplemented. |
| `glossary.activation` | Rust implements seed validation + lookup only; upstream event replay/extraction/middleware/activation mutations have no native counterpart. |
| `lifecycle.mutations` | Rust is read-only: no init/create/implement/review/accept/consolidate/merge/recovery. |
| `project.root`, `git.inspect` | Upstream has no equivalent CLI contract; discovery/inspection would be startup-class or requires a shared contract. |
| `capabilities` | Rust-specific migration disclosure surface; not rewritten upstream behavior (not required). |

## Equivalence discipline

1. **Preflight before timing.** Each side runs once. Exit codes must match the
   declared contract, and the normalized payloads are compared. A mismatch
   aborts that workload with `equivalence_failed`; no timing samples are kept.
2. **Per-sample re-assertion.** Every timed sample is normalized and hashed.
   If the hash differs from the preflight, the sample is rejected as
   `nondeterministic` and cannot become evidence.
3. **Canonical normalization.** JSON is sorted and compacted; absolute fixture
   paths are replaced with `<FIXTURE>`. Only explicitly declared
   non-upstream fields are dropped:
   - glossary `result.status` (Rust aggregation of upstream active-sense counts)
   - glossary `status` (same field under the top-level match list)
   - Rust metadata `read_only`
   - dependency **order** inside the mission-graph projection (upstream returns
     an unordered adjacency list; Rust preserves declaration order)
4. **No “empty is fine”.** Contract workloads require exit 0 *and* non-empty
   stdout. Non-JSON operation output is marked unparseable and fails.
5. **Negative tests** (`bench.py selftest`, 18 checks) exercise failure,
   empty output, faster-but-wrong, pending-workload greenwashing,
   projection/startup/micro substitution, missing required work, >10%
   regression, sub-3x geomean, low-confidence ratios, insufficient samples,
   truncation, identity drift, and nested-diff detection.

## Measurement protocol

- **Fixtures** are generated from deterministic recipes + seed, never copied
  from a working tree and never mutated. Each report records a per-file sha256
  manifest. Recipes:
  - `project-large`: 800 Missions × 4 WPs, 6 000 glossary senses across 4 scopes,
    20 000 mixed status events.
  - `runtime-large`: 900-step frozen mission template (~600 KiB) plus 9 snapshot
    scenarios (step, blocked, terminal, pending decision, missing input,
    blocking audit, advisory audit, unschedulable, drift).
  - `project-small` is available for quick checks.
- **Isolation.** `HOME`, `XDG_*`, `TMPDIR`, bytecode cache and cwd are under the
  run directory. `PYTHONHASHSEED=0`, `TZ=UTC`, `NO_COLOR`, upgrade/nag/banner
  checks disabled. The repository `src/` tree and installed packages are not
  written.
- **Ordering.** After one warmup per side, all `(workload, side)` tasks are
  shuffled with a recorded seed and executed interleaved. Order index, load
  before/after, exit status, wall time, and child user/sys CPU time are stored
  per sample.
- **Statistics.** Median, mean, min/max, stdev, CV, MAD, modified z-score
  outliers, seeded 2 000-iteration percentile bootstrap 95% CI for the ratio of
  medians. Raw samples are emitted to `samples.jsonl`.
- **Floor controls.** Rust `--version` and a Python import-only adapter measure
  protocol/launch cost. `work_excess_ratio` subtracts those floors and is a
  diagnostic only (null when either side's excess is < 5 ms).
- **Bounded runs.** `--max-seconds` stops the measurement; a truncated run is
  reported and **blocks** the gate rather than being silently averaged.

## Gate rules

`PASS` requires all of the following:

1. Oracle identity is exact: source imported from `<repo>/src`; every installed
   distribution present in `uv.lock` matches its locked version and none is
   unlocked; Rust binary sha256 recorded and stable.
2. No required workload is `pending`.
3. Every runnable workload passed preflight equivalence and every timed sample
   kept the same normalized payload.
4. At least five samples per side for every gate workload.
5. Geometric mean of `median(python) / median(rust)` over **strict, required,
   work-class** workloads is ≥ 3.0.
6. No required strict workload is slower than 0.909× (an unexplained >10%
   regression).
7. No gate workload's ratio 95% CI falls below 3.0 while its point estimate is
   above 3.0 (an uncertain pass), and no run is truncated.

Projections, startup and micro results cannot satisfy any of these conditions.
High CV is reported as a warning; the decision-relevant test is the ratio CI.

## Adding a paired workload

1. Add a deterministic recipe or input to `spkbench/fixtures.py` if needed.
2. Add a `Workload` entry in `spkbench/workloads.py` with an explicit
   `equivalence` class, `required` flag, and a comparator/normalizer.
3. If no CLI equivalent exists, add a small development adapter under
   `adapters/` that calls the pinned upstream library. Document which fields are
   upstream-defined and which (if any) are derived.
4. Run `bench.py selftest`, then a targeted `--workload '<id>' --reps 1` pass to
   prove equivalence before any longer measurement.
5. Do not mark a workload `strict` until every compared field maps to upstream
   behavior; prefer `pending` over a flattering but unequal comparison.

## Layout

```text
benchmarks/rust/
  bench.py                 CLI entrypoint
  spkbench/                harness (stdlib only)
    fixtures.py            deterministic fixture recipes
    workloads.py           equivalence registry + projections
    execute.py             isolated subprocess execution
    runner.py              preflight, interleaved measurement, floors
    stats.py               medians, bootstrap CIs, noise metadata
    gate.py                acceptance-gate evaluation
    report.py              JSON + Markdown reports
    selftest.py            negative tests
  adapters/                development-only Python oracle adapters
  evidence/                committed measurement reports
```

## Known limitations

- The VM is shared. The baseline ran at load average ~7–23 and shows relative
  jitter on 20–60 ms Rust samples. Ratios are reported with bootstrap CIs; the
  gate blocks on decision-relevant uncertainty, not on raw CV alone.
- Five repetitions are enough to expose gross instability, not to certify
  sub-10% differences. A final claim needs a quieter machine or more repetitions.
- The Python adapters are library-level oracles invoked once per process. They
  avoid the ~2.5 s Python CLI bootstrap so the comparison is not merely a
  startup measurement; the startup cost is measured separately as its own
  workload class.
- Floor subtraction is approximate: the import-only adapter may not cover every
  module a workload would touch. It is reported, never used to pass the gate.
- No mutating workload is measured yet. The Rust CLI cannot perform one. Read-only
  workloads are checked for fixture integrity (the fixture manifest is recomputed
  after the run and any change fails the gate). When mutating workloads are
  added they must run on a fresh fixture clone per invocation and compare
  post-state manifests, not just stdout; that clone-per-invocation contract is
  not exercised by this baseline.
