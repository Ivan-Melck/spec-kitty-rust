# Benchmark adapter report — native compatibility workflows

Status: **adapter/equivalence work complete for this tranche; the speed gate is
`BLOCKED`.** No timing campaign was run and no release build was made. This
report records what is provably equivalent, what stays absent, and how the
measurement core now consumes it. Gate design and checker integration are in
[CERTIFICATION-REPORT.md](CERTIFICATION-REPORT.md).

Checkpoint: `ff4452b02` (`feat(rust): integrate native transactions and fix
reproduced parity defects`). The release binaries predate the current source (tracked
build inputs are newer, e.g. `crates/sk-cli/src/compat/runtime.rs`), so nothing
here binds a measurement to the tree. Untimed conformance evidence was produced
with the pre-existing artifacts only:

| Artifact | sha256 |
| --- | --- |
| `target/release/spec-kitty` (compat CLI used here) | `0ae779448f13f41272b60baf92758da4778def4b2b2b0df9a2b33840ef077a26` |
| `target/release/spec-kitty-rust` (inspection CLI) | `0caf5722cb294bdef6547cf0d944683d0289fde96674ab534d5c46e3721ba907` |
| `.venv-rust-oracle/bin/python` (real target) | `0253b01a7f5d96a042a2b9f5e0f5ff5062707a1c75be2b089098e3111cd186d2` |

`bench.py identity` is deliberately **not** `exact` at this checkpoint: the tree
is dirty from concurrent owner work, both binaries predate build inputs, and no
build receipt exists. `bench.py build` on a clean commit is the supported way to
fix that.

## What changed in this tranche

Owned surface only: adapters, fixtures, registry consumption, mutation timing,
evidence writing. No `crates/`, `src/`, checker or inventory files were edited.

| Path | Change |
| --- | --- |
| `spkbench/registry.py` | **New.** Loads/validates the frozen registry shared with `check_completion.py`; rejects id/floor drift and vacuous schemas. |
| `spkbench/expectations.py` | **New.** Declarative payload/post-state schema evaluator used by preflight, every sample and the gate. |
| `spkbench/certify.py` | **New.** Builds the checker-shaped bundle, recomputes every statistic, enforces the fixed gate, projects the `coverage.json` performance section. |
| `spkbench/build_receipt.py` | **New.** `bench.py build` writes a source→binary receipt (clean commit, build-input digest, toolchain, command, hashes); the gate requires it. |
| `spkbench/workloads.py` | Registry-driven ids; 16 absent frozen workflows recorded with reasons; diagnostics are never required or gate-eligible; versioned `--json` envelope unwrapped. |
| `spkbench/fixtures.py` | Root-canonicalised portable content digests; new `replay-project` recipe (2,800 events / 200 WPs); sealed build-time manifests. |
| `spkbench/runner.py` | Paired randomized interleaving, per-sample payload **and** post-state hashing, clone-per-invocation mutation path with clone/post-state outside the timer, schema re-assertion. |
| `spkbench/stats.py` | Paired ratio bootstrap, aggregate geomean CI with pair resampling, uncertainty-aware regression verdict. |
| `spkbench/identity.py` | Rust build-input/toolchain/binary provenance, build-input freshness, fail-closed oracle probe, oracle baseline + `verify_oracle.py`, repo `packs`, dual-binary attestation. |
| `spkbench/report.py` | Evidence bundle writer (samples, payloads, identity, fixtures, manifest, bundle) and disk re-verification. |
| `spkbench/gate.py` | Only the fixed-policy entry point; the old `GateOptions` relaxation surface is gone. |
| `spkbench/evidence.py` | **New.** Re-derives payload canonical hashes, schemas, equivalence, post-state equality, per-sample fields and pairs from the stored files. |
| `spkbench/live.py` | **New.** Observes the live commit/build inputs/`Cargo.lock`/toolchain/binaries/receipt/oracle/packs and hashes the harness + adapter code. |
| `spkbench/selftest.py` | 49 end-to-end controls on written bundles (positive control + the review's negative probes); the synthetic probe is a test seam, never an acceptance switch. |
| `bench.py` | `registry`/`fixtures verify`/`build`/`gate`/`run`; no threshold flags; old evidence stays `BLOCKED`. |
| `tools/mutation_check.py` | Registry-aware; untimed conformance only (venv interpreter no longer symlink-resolved). |
| `tests/` | 60 tests; missing oracle/binaries **fail** (autouse fixture), never skip. |

`evidence/conformance/status-replay-mutation.json` is the untimed proof for the
one measurable mutation workload.

## Reproducible entry points

```sh
# 1. Harness self-tests: expect 49/49.
python3 benchmarks/rust/bench.py selftest

# 2. Portable fixtures at two roots.
python3 benchmarks/rust/bench.py fixtures verify

# 3. Adapter / registry / gate suite (pytest + PyYAML are dev tools only).
env PYTHONPATH=src SPKBENCH_ORACLE_PYTHON="$PWD/.venv-rust-oracle/bin/python" \
  /tmp/spkbench-pytest/bin/python -m pytest benchmarks/rust/tests -q

# 4. Untimed mutation conformance (fresh clone per invocation).
PYTHONPATH=src SPEC_KITTY_PACKS_ROOT="$PWD/packs" .venv-rust-oracle/bin/python \
  benchmarks/rust/tools/mutation_check.py \
  --rust-binary target/release/spec-kitty \
  --oracle-python .venv-rust-oracle/bin/python --packs-root "$PWD/packs" --invocations 2
# observed: equivalent=true, post_state_equal=true, master_unchanged=true,
#           ignored_artifacts_differ=false
```

## Eligibility matrix

Only `strict` + `required` + `work` + **frozen registry** rows count toward the
3× gate. Projections and micro results are diagnostics. Absent entries block.

| Workload | Kind | Evidence | Gate |
| --- | --- | --- | --- |
| `status.replay` | strict mutation | Fresh-clone pair on `replay-project`: `event_count=2800`, 200 WPs, result JSON identical, `status.json` byte-identical (`e1b40807…` on the smaller fixture; the replay fixture's own digest is recorded per run), pristine fixture unchanged. Only `.git/spec-kitty-locks/` is excluded, with a recorded ruling; the lock namespace itself now agrees. | **Eligible** once 20 pairs/side + build receipt + clean commit exist |
| `startup.version` / `startup.help` | contract startup | Version strings equal after extraction; help output must contain `Usage:`/`spec-kitty`, ≥6 lines, ≥200 bytes. | Separate; never gate evidence |
| `cli.mission.list` | projection | `count`/`missions` with slug/mid8/friendly_name. | Required row, **not** in the strict aggregate |
| `state.event-append`, `state.move-task` | absent | Native routes are explicit refusals (`agent status emit`, `agent tasks move-task`). | Blocked, never paired |
| `state.decision.resolve`, `runtime.next.governed`, `governance.charter`, `glossary.activation` | absent | Partial or unrelated native surfaces; see registry `blocked_by`. | Blocked |
| `workflow.*` (10) | absent | Create/specify/plan/tasks/implement/review/accept/consolidate/merge/recovery are partial or explicit refusals. | Blocked |
| `runtime.query.*` (9), `glossary.lookup.*` (6) | strict diagnostics | Real planner / lookup equivalence. | Diagnostic only |
| `tasks.list` | strict diagnostic | Parsed JSON identical (6 rows). | Diagnostic only |
| `tasks.list_dependents.projection`, `mission.branch_context.projection`, `mission.list/graph.projection`, `transition.check.*` | diagnostics | Declared projections/micro. | Never gate evidence |

### Lock-artifact ruling (status.replay)

Status lock artifacts are implementation-private, ephemeral and named differently
by each implementation. They are captured per invocation under
`ignored_artifacts`, never asserted equal, and never counted as observable
post-state. Every user-facing byte — including `status.json` — is compared
exactly. The ruling is recorded in the frozen registry next to the workload, so
it cannot be widened silently.

## Evidence binding (follow-up tranche)

The gate no longer takes stored payloads, pair arrays or a recorded native
identity on trust: it re-parses every stored payload/post-state delta against its
declared canonical hash and schema, reconstructs the pairs from each sample's
`pair_index`, re-derives every statistic from them, re-observes the live native
source (including the oracle/packs/`uv.lock`) and hashes the harness/adapter code.
Details, controls and remaining obligations:
[EVIDENCE-BINDING-REPORT.md](EVIDENCE-BINDING-REPORT.md).

## Why nothing is certified

- 16/17 strict completion workloads are absent natively → the gate blocks by
  construction; no measured ratio can rescue it.
- The current release predates the source; there is no build receipt.
- No timing campaign has been run (the committed initial bundle is historical and
  stays `BLOCKED`; it has a different schema and is refused by `bench.py gate`).

## Guardrails observed

- Fresh fixture clone per mutation invocation; clone/post-state outside the timer;
  pristine fixture re-verified.
- Fixtures are generated in a temp work directory, never in the checkout; the
  repo `src` tree and real user state are never written.
- Oracle is `.venv-rust-oracle/bin/python` with `PYTHONPATH=<repo>/src` and
  `SPEC_KITTY_PACKS_ROOT=<repo>/packs`; no installed-tool fallback, no
  production Python path in either native binary.
- Only declared nondeterministic fields and the internal lock artifact are
  excluded; every other difference is recorded.
