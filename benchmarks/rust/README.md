# Rust vs Python certification harness

Development-only measurement tooling for the Spec Kitty Rust rewrite. It pairs the
native binaries with the pinned Python reference on deterministic fixtures,
asserts **non-vacuous result and post-state equivalence before timing**, runs
paired interleaved repetitions, and writes a hash-bound evidence bundle that the
completion checker can re-derive.

This directory is not shipped, is never imported by Rust, and never provides a
production Python fallback.

> **Status: the acceptance gate is `BLOCKED`, not passed.**
> 16 of the 17 strict completion workloads are still absent natively
> (`state.event-append`, `state.move-task`, `state.decision.resolve`,
> `runtime.next.governed`, `governance.charter`, `glossary.activation` and the ten
> `workflow.*` entries). Their absence blocks the gate regardless of any measured
> ratio. No timing campaign has been run against the current source. See
> [CERTIFICATION-REPORT.md](CERTIFICATION-REPORT.md) and
> [evidence/2026-11-02-initial/README.md](evidence/2026-11-02-initial/README.md)
> (historical `BLOCKED` baseline, not re-certified).

## One registry, shared with the completion checker

`docs/rust/inventory/performance-registry.json` is the single source of truth for
the 20 required workload ids, the 17 strict gate ids, the 2 startup ids, and the
frozen floors (20 samples/side, 14 strict workloads, 3.0x geometric mean, 10%
regression). It mirrors `scripts/rust/inventory_model.py` and
`scripts/rust/check_completion.py`; `tests/test_registry_unification.py` asserts
that equality against the live modules.

The harness cannot invent ids and cannot drop them: a run only produces rows for
the frozen registry, a missing row blocks, and a required id that is not in the
registry is a failure. A `--workload` subset still writes a full bundle whose
missing rows block.

## Quick start

```sh
# Lock-exact oracle (network needed once; isolated directory).
python3 benchmarks/rust/bench.py oracle-provision --dir /tmp/spk-bench-oracle

# Harness negative/end-to-end suite (fails, never skips, if the environment is absent).
python3 benchmarks/rust/bench.py selftest

# Fixture portability: every recipe at two different roots.
python3 benchmarks/rust/bench.py fixtures verify

# Workload listing and registry validation.
python3 benchmarks/rust/bench.py registry
python3 benchmarks/rust/bench.py list

# Adapter/fixture/gate tests (pytest + PyYAML are development tools, not oracle deps).
uv venv /tmp/spkbench-pytest --python 3.11
uv pip install --python /tmp/spkbench-pytest/bin/python pytest pyyaml
PYTHONPATH=src /tmp/spkbench-pytest/bin/python -m pytest benchmarks/rust/tests -q

# Untimed mutation conformance for the one measurable mutation workload.
PYTHONPATH=src SPEC_KITTY_PACKS_ROOT="$PWD/packs" .venv-rust-oracle/bin/python \
  benchmarks/rust/tools/mutation_check.py \
  --rust-binary target/release/spec-kitty --oracle-python .venv-rust-oracle/bin/python \
  --packs-root "$PWD/packs" --invocations 2

# Certification (requires a clean commit and a build receipt; not run yet).
python3 benchmarks/rust/bench.py build --receipt target/release/build-receipt.json
python3 benchmarks/rust/bench.py run --oracle-python .venv-rust-oracle/bin/python --reps 20
python3 benchmarks/rust/bench.py gate benchmarks/rust/evidence/<run>
```

`run` never rebuilds the Rust binary. It refuses to certify without a build
receipt, a clean source tree, and a binary newer than every tracked build input;
`bench.py build` is the only supported way to produce that receipt.

## What is measured

| Frozen workload | Kind | Current evidence path |
| --- | --- | --- |
| `status.replay` | mutation | Canonical replay + `status.json` materialization over a 2,800-event / 200-WP coherent mission. Native `agent status materialize` vs the pinned upstream implementation. Result JSON and `status.json` bytes are identical; only implementation-private lock artifacts are excluded (ruling in the registry). |
| `startup.version`, `startup.help` | startup | Process startup. Measured separately, never gate evidence; version strings are compared after extraction, help output must satisfy a non-vacuous stdout schema. |
| `cli.mission.list` | projection | Mission identity projection (`slug`/`mid8`/`friendly_name`). Required by the frozen floor, explicitly excluded from the strict aggregate. |
| `state.event-append`, `state.move-task`, `state.decision.resolve`, `runtime.next.governed`, `governance.charter`, `glossary.activation`, `workflow.*` | **absent** | No native implementation, or an explicit native refusal. Recorded as `absent_native` with the reason; never paired with an invented adapter, never timed as an equal error. |

Diagnostics (never required, never gate evidence) keep the measurement core
honest: nine pure-planner `runtime.query.*` scenarios, six `glossary.lookup.*`
cases, `tasks.list`, and four projections/micro checks. They are real strict
pairs, but `runtime.query` is not `runtime.next.governed` and lookup is not
activation.

## Equivalence discipline

1. **Non-vacuous schemas.** Every runnable workload declares expected exit code,
   required/non-null/non-empty payload keys, exact dotted-path values, minimum
   collection sizes and row keys — plus, for mutations, the exact allowed
   post-state additions/changes/removals, minimum file sizes and a JSON schema
   for the written artifact. Two empty documents, two unparseable blobs, two
   `{}` payloads and two no-op mutations fail the schema before timing.
2. **Preflight, then per-sample re-assertion.** Each side runs once; exit codes,
   normalized payloads and post-state deltas must match. Every timed sample is
   re-hashed against the preflight (payload and post-state); drift makes the
   sample `nondeterministic`, and nondeterminism fails the gate.
3. **Mutation isolation.** Every mutating invocation runs in a fresh clone of the
   pristine fixture. Clone and post-state scan are outside the timed window, so
   clone cost is credited to neither side. The pristine fixture is re-verified
   after the run.
4. **Declared exclusions only.** Normalization steps are named in the registry
   (`unwrap_rust_data`, `project_mission_list`, …) and drop only declared
   non-upstream fields. Ignored post-state artifacts are captured per invocation
   and reported; they are never silently equalized.
5. **Portable fixtures.** Fixture content is hashed with the fixture root replaced
   by `<FIXTURE>`, so the same recipe + seed regenerates the same digest at any
   audit path. `fixtures verify`, the gate **and the completion checker** rebuild
   two roots from the frozen recipes and compare the digest, recipe facts and file
   count with the recorded table. A unit-scoped synthetic fixture is checked
   through an explicit observation seam of the same shape; there is no bundle flag
   or asserted verdict that skips the comparison.

## Measurement protocol

- Fixtures are generated from recipes + seed in a temporary work directory, never
  copied from a working tree and never written into the checkout.
- Repetition *i* runs the Python and Rust side adjacent in randomized order and
  records `pair_index`; samples store order, wall time, child CPU, load before/after,
  exit status, normalized payload hash and post-state hash.
- Statistics: median, CV, and a seeded **paired** bootstrap of per-pair ratios;
  the aggregate interval resamples pairs within every workload.
- Floors: Rust `--version` and a Python import-only adapter measure launch/import
  cost. Work-excess is recomputed by the gate, and the gate requires a work-excess
  geometric mean ≥3x as well as total-latency ≥3x.
- Isolation: `HOME`, `XDG_*`, `TMPDIR`, `PYCACHEPREFIX`, `PYTHONHASHSEED=0`,
  `TZ=UTC`, `NO_COLOR`; the repository `src` tree and real user state are never
  written.
- A truncated run, a loaded machine (median load1m above the frozen floor), a
  noisy side (CV above the floor) or any nondeterministic sample blocks rather
  than being averaged.
- Identity is re-collected **after** the measurement window and the bundle is
  verified against that fresh snapshot, so a mid-run edit, rebuild or oracle
  change blocks instead of being certified from the start snapshot.

## Gate rules (fixed, not configurable)

`PASS` requires all of:

1. the frozen registry digest matches and its ids equal the checker's floor;
2. recorded identity is exact: clean commit, build-input digest, `Cargo.lock`,
   toolchain, **verified build receipt**, both binary hashes, pinned oracle
   baseline, repo `packs` digest, oracle `verify_oracle.py` pass;
3. every required workload row is present, has the registry's class and
   measurement kind, and is either `absent_native` (**which blocks**) or passes;
4. every required measured workload (strict, projection and startup) has ≥20
   raw samples **per side** and ≥20 pairs;
5. every sample carries its `pair_index`, `payload_sha256`, post-state hash,
   exit code and timeout flag, and each of those fields equals the proved
   preflight value and the stored artifact;
6. the paired arrays are reconstructed from those `pair_index` values in the
   recorded global `order` (the executor counter, including its floor/preflight
   offset): orders are unique across the run, adjacent within each pair, one rust
   and one python record per pair, and increasing with `pair_index`; speedup,
   paired CI, work-excess ratio, aggregate CI and geomean, and the regression
   verdict are derived **only** from the reconstructed samples;
7. the normalized payload of every measured side is stored, its declared
   canonical hash is recomputed from the stored document, its frozen schema is
   re-applied, and equivalence is re-derived by comparing the two stored
   payloads (or the stored stdout contract) rather than reading the recorded
   boolean;
8. the geometric mean is ≥3.0x and its 95% CI does not reach below 3.0x; the
   work-excess geometric mean is also ≥3.0x; the recorded CI, work geomean and
   counts must equal the recomputed ones;
9. no gate workload is a startup command, a stdout-only contract workload or
   startup-dominated (work window <5 ms or work-excess ratio <1.25x);
10. no required workload — including the projection — has a statistically
    material >10% regression (`speedup < 0.909` with the interval entirely
    below the floor); an uncertain regression blocks;
11. each mutation's stored post-state envelope carries the full before/after
    content manifests, the delta and the actual bytes of every schema-constrained
    file; the registry's `expected_additions/changes/removals`, `required_paths`,
    `min_bytes`, `json_payload` schema and `unchanged_paths` are re-derived from
    those stored bytes with the same evaluator the preflight uses, the delta must
    be exactly the manifest difference, each stored file must hash to its
    after-manifest entry, and the two sides must agree, with a recorded exclusion
    ruling;
12. fixtures re-verify from their recipes at two roots, every artifact hash and
    `manifest.json` match the files on disk, and the bundle's self-hash matches;
13. the live checkout is observed, not read back from the bundle: HEAD (or a
    proven evidence/docs-only descendant whose every measured input is
    unchanged), build-input digest, `Cargo.lock`, toolchain, both binary hashes
    and a re-verified build receipt must be the measured ones, and no measured
    input may be dirty;
14. the harness, adapters, diagnostic tools and the checker adapter are hashed
    into `harness.code`, and the live files must still hash to the same values.

Exit codes: `0` PASS, `1` FAIL, `2` BLOCKED (missing/unverifiable evidence).
`--diagnostic` only changes the exit code for a subset run; the bundle verdict
stays authoritative and can never become `PASS`.

## Evidence bundle

`benchmarks/rust/evidence/<run>/`:

| File | Purpose |
| --- | --- |
| `performance-evidence.json` | Checker-shaped, self-hashed bundle: registry, baseline/candidate identity, environment/load, portable fixtures, per-workload raw samples (ms), paired arrays, statistics, work-excess, post-state, aggregate, startup, artifacts, gate verdict. |
| `manifest.json` | sha256 of every artifact and of the bundle. |
| `samples.jsonl` | One row per timed invocation (order, pair index, duration, CPU, load, exit, payload/post-state hashes). |
| `report.json` / `summary.md` | Human/audit report, including the exact gate reasons. |
| `identity.json`, `environment.json`, `fixtures.json` | Source/binary/toolchain/oracle/packs identity, machine/load context, portable fixture manifests. |
| `payloads/*.json` | Normalized preflight payloads for every measured side. Pretty JSON is allowed: `canonical_sha256` is the hash of the canonical JSON of the parsed document, so a reformat is not an edit. |
| `post-state/*.json` | Mutation post-state envelopes: full before/after content manifests, the delta, the substituted params and the base64 bytes of every schema-constrained file. Content-addressed the same way as payloads, so a re-serialised envelope is not an edit; the raw file bytes are still bound to their manifest hash. |

`bench.py gate` reloads the bundle, observes the live source (never a recorded
identity), rebuilds the fixtures, re-hashes the artifacts and manifest, re-parses
the payloads, re-derives the post-state contract from the stored envelopes and
re-derives every statistic from the reconstructed pairs. The completion checker
runs the same verifier with the same fixture regeneration by default.
Editing a number without resealing breaks the self-hash; resealing does not help
because the number is recomputed from the stored evidence.

## Layout

```text
benchmarks/rust/
  bench.py                    CLI (registry/list/identity/build/fixtures/selftest/run/gate)
  spkbench/
    registry.py               frozen registry loader/validator
    evidence.py               derives facts from stored payloads/samples/manifest
    live.py                   live native-source + harness-code probe
    expectations.py           declarative payload/post-state schema evaluator
    fixtures.py               deterministic + portable fixture recipes
    workloads.py              registry workloads + non-gating diagnostics
    execute.py                isolated subprocess execution
    mutation.py               clone-per-invocation executor (untimed conformance)
    runner.py                 paired measurement, mutation hook, schema re-assertion
    stats.py                  paired bootstrap, geomean CI, regression/uncertainty
    certify.py                bundle construction, recomputation, gate, coverage projection
    build_receipt.py          source→binary build receipt
    identity.py               source/binary/toolchain/oracle/packs identity
    report.py                 bundle writer + rendered report
    gate.py                   frozen-policy entry point (no relaxation knobs)
    selftest.py               49 end-to-end negative/positive controls
  adapters/                   development-only Python oracle adapters
  tools/mutation_check.py     untimed mutation conformance
  evidence/                   committed evidence (historical BLOCKED + conformance)
```

## Known limitations

- The VM is shared. A certification run must observe the frozen load floor
  (median load1m ≤6, spread ≤4); otherwise the gate blocks.
- The Python oracle adapters call pinned library/CLI implementations directly, so
  the comparison is not merely Python interpreter startup. Work-excess is
  recomputed and gated anyway.
- 16 frozen workloads have no native implementation yet. That is the honest
  blocker; this harness does not manufacture coverage for them.
- Only Linux has been measured. Cross-platform evidence remains an open delivery
  gate.
