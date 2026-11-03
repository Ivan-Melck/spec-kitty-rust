# Benchmark adapter report — native compatibility workflows

Status: **adapter/equivalence work complete for this tranche; speed gate remains
`BLOCKED` and `false`.** No timing campaign was run and no release build was
made. This report records what is now provably equivalent, what stays pending,
and the exact measurement-core edits still required.

Checkpoint: `ab3e0fcc8` (`feat(rust): checkpoint native lifecycle substrates with
blocked completion gates`). The working tree contains uncommitted source changes
outside `benchmarks/rust/` that were already in flight; this tranche did not
touch them, did not build a release, and binds every result below to the
pre-existing artifacts:

| Artifact | sha256 |
| --- | --- |
| `target/release/spec-kitty` (compat CLI used here) | `5fa476982dc097026e584e419e72adef61c7088ff6c73b660df271f91c672861` |
| `target/release/spec-kitty-rust` (inspection CLI, unchanged) | `7bea8ccd046b32df3f5299b7689c1dd6bf025f2e3c515053061319b3f887cad5` |
| `.venv-rust-oracle/bin/python` | `0253b01a7f5d96a042a2b9f5e0f5ff5062707a1c75be2b089098e3111cd186d2` |

`bench.py identity` is `exact` for the inspection binary and the oracle source /
`uv.lock` dependency set at this checkpoint (source imported from `<repo>/src`,
no dependency drift). The compat binary is not yet attested by the identity
core; see **Core edits needed**.

## What changed

Owned surface only: adapters, fixture/workload registration, mutation executor
and their tests. `spkbench/stats.py`, `gate.py`, `runner.py`, `identity.py` and
`execute.py` are **untouched**.

| Path | Change |
| --- | --- |
| `benchmarks/rust/spkbench/fixtures.py` | New `compat-project` recipe: 12 Missions × 6 WPs, an **unborn `main`** branch (deterministic `.git`, no index/objects) and a coherent 36-event mission-002 history. Manifest includes `.git`; two builds produce identical manifests. |
| `benchmarks/rust/spkbench/workloads.py` | Registry now has 46 entries: new strict `tasks.list`; new diagnostic `tasks.list_dependents.projection` and `mission.branch_context.projection`; new `status.materialize` mutation entry; stale `status.replay` / blanket `lifecycle.mutations` removed and replaced by precise entries (`status.validate`, `tasks.status`, `tasks.list.target-branch-guard`, `lifecycle.init`, `lifecycle.plan`, `mission.create`, `tasks.finalize`, `lane.implement`); `rust_payload` accepts bare compat documents as well as the versioned envelope; `MutationSpec` added additively; compat routes resolve the sibling `spec-kitty` binary and stay pending when it is absent. |
| `benchmarks/rust/adapters/py_tasks_list.py` | Calls the pinned upstream `agent tasks list-tasks` implementation directly. |
| `benchmarks/rust/adapters/py_tasks_list_dependents.py` | Calls the pinned upstream `agent tasks list-dependents` implementation directly. |
| `benchmarks/rust/adapters/py_branch_context.py` | Calls the pinned upstream `agent mission branch-context` implementation directly. |
| `benchmarks/rust/adapters/py_status_materialize.py` | Calls the pinned upstream `agent status materialize` implementation directly. |
| `benchmarks/rust/spkbench/mutation.py` | Additive mutation executor: fresh clone per invocation, result + post-state comparison, declared internal-artifact capture, pristine-fixture check. |
| `benchmarks/rust/tools/mutation_check.py` | No-timing proof entry point for mutation workloads. |
| `benchmarks/rust/tests/` | 16 adapter/registration/fixture/mutation tests in temporary fixtures. |

No production Python fallback was added. No `src/`, manifest or repository-root
documentation was edited. No commits or pushes were made.

## Reproducible entry points

All commands run from the repository root. The oracle must remain the
lock-exact environment; the installed `spec-kitty-cli` tool is not a valid
oracle.

```sh
# Oracle (already provisioned in this VM; network needed once otherwise)
UV_PROJECT_ENVIRONMENT="$PWD/.venv-rust-oracle" \
  uv sync --frozen --no-dev --python 3.11

# 1. Harness negative self-tests (unchanged core): expect 19/19
python3 benchmarks/rust/bench.py selftest

# 2. Registry listing (no fixture trees are written)
python3 benchmarks/rust/bench.py list

# 3. Adapter / equivalence / mutation self-tests in temporary fixtures.
#    Pytest is a dev tool only; install it outside the lock-exact oracle env.
uv venv /tmp/spkbench-pytest --python 3.11
uv pip install --python /tmp/spkbench-pytest/bin/python pytest
PYTHONPATH=src /tmp/spkbench-pytest/bin/python -m pytest benchmarks/rust/tests -q
# observed here: 16 passed in ~26 s

# 4. List mutation workloads and their declared exclusions
PYTHONPATH=src .venv-rust-oracle/bin/python \
  benchmarks/rust/tools/mutation_check.py --list

# 5. Prove a mutation pair without timing it (fresh clone per invocation)
PYTHONPATH=src .venv-rust-oracle/bin/python \
  benchmarks/rust/tools/mutation_check.py \
  --rust-binary target/release/spec-kitty \
  --out /tmp/spkbench-mutation --invocations 2
# observed: ok=true, equivalent=true, post_state_equal=true,
#           master_unchanged=true, ignored_artifacts_differ=true
```

`mutation_check.py` writes `mutation-report.json` (schema
`spkbench.mutation-check.v1`) under `--out`. It removes clones unless
`--keep-clones` is passed.

For a future `bench.py run`, keep the primary `--rust-binary` as
`target/release/spec-kitty-rust`: inspection workloads still need that CLI, and
the compat routes auto-resolve the sibling `target/release/spec-kitty`. Passing
`spec-kitty` as the primary would break the inspection routes (they would need
the `native` prefix). Attesting both artifacts is core item 3 below.

## Eligibility matrix

Only `strict` + `required` + `work` counts toward the 3× gate. Projections and
micro results are diagnostics. Pending entries block the gate.

| Workload | Class | Evidence | Gate |
| --- | --- | --- | --- |
| `tasks.list` | strict, required, work | Parsed JSON identical to pinned upstream `agent tasks list-tasks` (6 rows, same ids/titles/lanes/paths). Read-only; fixture manifest unchanged. | **Eligible** once the 20-sample floor and dual-binary identity are met |
| `tasks.list_dependents.projection` | projection | `wp_id` and sorted `dependents` identical (`WP02 → WP04, WP05`). Exposes a real gap: upstream `depends_on` is raw scalar text (`["[WP01]"]`), native is parsed (`["WP01"]`). | No — diagnostic |
| `mission.branch_context.projection` | projection, micro | All branch-contract fields identical; `repo_root` path normalised; wall-clock `NOW_UTC_ISO` / `runtime_vars.now_utc_iso` declared nondeterministic. Native omits upstream's `spec_kitty_version`. | No — launch-dominated |
| `status.materialize` | pending, required | Fresh-clone pair: result JSON and `status.json` bytes identical (sha256 `e1b40807…`), all user-facing files identical; pristine fixture unchanged. Only lock artifact differs. | **Pending** (see blockers) |
| `status.validate` | pending, required | On the same 240-event fixture upstream reports 21 errors + 156 warnings (clock-skew ordering, done-without-evidence); native reports 145 errors + 0 warnings (schema/FSM-edge class only). Different finding contracts. | Pending |
| `tasks.list.target-branch-guard` | pending, required | Upstream refuses detached HEAD / resolves the target branch; native reads without a branch guard. Happy path is `tasks.list`; the guard is absent. | Pending |
| `tasks.status` | pending, required | Native emits extra fields (`auto_commit`, `done_count`, `progress_percentage`, stale/workspace verdicts) and derives agent/workspace kind from events; upstream omits them and reports empty/unknown. Different result shape and data. | Pending |
| `lifecycle.plan` | pending, required | Upstream blocks with `SPEC_NOT_SUBSTANTIVE_OR_UNCOMMITTED` and writes nothing; native scaffolds `plan.md` without the substantive-spec gate or planning commit. | Pending |
| `lifecycle.init` | pending, required | Same empty-dir, `init . --ai codex --non-interactive` inputs: upstream writes 100+ files (full skills/commands/hooks/charter assets), native writes 14. Output and asset set differ. | Pending |
| `mission.create` | pending, required | Upstream refuses with `ACTIVE_CHARTER_CONFIG_INVALID` when no mission type is activated; native proceeds. Native appends a ULID and does not allocate `mission_number`; upstream does. | Pending |
| `tasks.finalize` | pending, required | Both exit 1 for different contracts: upstream validates requirement mapping from `spec`/`tasks.md`; native reports `tasks.md not found`. Planning commit, lanes refresh and lifecycle events absent. | Pending |
| `lane.implement` | pending, required | Both exit 1 differently: upstream refuses protected `main` before mutation; native refuses `MISSING_LANES`. Branch/dependency preflight, workspace persistence and auto-commit absent. | Pending |
| `runtime.next.governed` | pending, required | Durable native run store exists (`runtime start/query/advance/answer/recover`), but upstream `next` still owns mission-board progress, WP workspace resolution, decision-log commits, analysis currency, lifecycle records and retrospective hooks. | Pending |
| `mission.inventory.full`, `mission.show`, `mission.dependency_planning.full`, `project.root`, `git.inspect`, `events.inspect`, `governance.charter`, `glossary.activation` | pending | Reasons unchanged and still accurate (no equivalent upstream/native contract). | Pending |
| `capabilities` | pending, not required | Rust-specific migration disclosure; no upstream counterpart. | Not required |

`runtime.query.*`, `glossary.lookup.*`, `mission.list.projection`,
`mission.graph.projection`, `transition.check.*`, `startup.*` are unchanged.

### Why `status.materialize` is still pending

Two independently actionable items, neither of which is hidden:

1. **Lock artifact ruling.** After materialization the canonical
   `status.json` is byte-identical, but the lock file is implementation-private
   and differs:
   - upstream: `.git/spec-kitty-locks/<mission-slug>.status.lock` (or
     `.kittify/spec-kitty-locks/...` outside git)
   - native: `.git/spec-kitty-locks/status-<hash>.lock` (hash varies per run)

   `mutation_check` records these under `observations[].ignored_artifacts` and
   reports `ignored_artifacts_differ=true`. They are excluded from the equality
   manifest only because lock naming is plausibly internal; that exclusion
   needs an explicit reviewer/operator decision. If lock naming is part of the
   post-state contract, this workload must stay pending.
2. **Runner hook.** `spkbench/runner.py` has no fresh-clone/post-state path, so
   a timed run would compare a mutated fixture to a pristine manifest and would
   also include clone cost. The executor exists and is proven; the frozen core
   is not patched.

## Core edits needed (not made here)

These are the additive changes the measurement core needs before mutation
workloads may be timed. `stats.py`, `gate.py`, `identity.py` and `runner.py`
were deliberately left untouched.

1. **Runner — mutation execution path.**
   - In `_run_once` / `_verify_sample`, when `workload.mutation is not None`:
     clone `fixture_paths[workload.fixture]` to a unique run-local directory,
     rewrite argv with `MutationSpec.rewrite`, run with `cwd=clone`, and attach
     `post_state_sha256 = sha_payload(diff_manifests(before, tree_manifest(clone,
     ignored)))`.
   - The per-sample verifier must require both the normalized-payload hash and
     the post-state hash to equal the side's preflight. A changed post-state is
     `nondeterministic`, never a valid sample.
   - Timing must start **after** the clone and stop **before** the post-state
     scan (or measure clone cost as a separate floor), so clone overhead is not
     credited to either side.
   - Keep `_cwd_for` for read-only work; mutation work must never use the
     shared fixture cwd.
2. **`bench.py run` — fixture lifecycle.** Build `compat-project` for selected
   mutation workloads, keep the pristine tree read-only, put clones under the
   run directory, and verify the pristine manifest afterwards. The current
   `fixtures_unchanged` check should apply to the pristine tree, not to clones.
3. **Identity — dual binary attestation.** Compat workloads resolve
   `target/release/spec-kitty` while inspection workloads use
   `target/release/spec-kitty-rust`. `identity.collect()` records one binary.
   Record both sha256 values, verify both are stable across the window, and
   block when either changes or is missing.
4. **Gate eligibility.** After 1–3, set `MutationSpec.timing_ready=True` on
   `status.materialize` and `equivalence="strict"` (retaining `required=True`,
   `klass="work"`) only once the lock-artifact ruling is resolved. Until then
   the entry remains `pending` and blocks the gate by design.

`rust_payload()` now accepts the compat CLI's bare document as well as the
versioned `{ok,data}` envelope; this is a workload-layer change, not a core
edit, and is covered by a test.

## Remaining blockers to a true speed result

- Required pending workloads above are still pending; the gate is `BLOCKED`.
- No timing campaign was run, so the ≥20 samples/side floor is not met. The
  frozen gate's command-line default is still `--min-samples 5`; a final
  campaign must pass `--reps 20 --min-samples 20` explicitly. A default-5 run
  must not be accepted as satisfying the operator's 20-sample floor.
- Source is in flight: the release artifacts predate uncommitted `crates/sk-*`
  changes. Rebuild both executables after that work settles, then rerun
  `mutation_check` and the equivalence tests before any timed campaign.
- The compat binary needs identity attestation (item 3).
- The lock-artifact ruling (item 1) must be recorded before
  `status.materialize` can enter the gate.
- Projections/micro results must not be used to rescue the geomean; they remain
  excluded.

## Guardrails observed

- Fresh fixture clone per mutation invocation; pristine manifest re-verified.
- `HOME`, `XDG_*`, `TMPDIR`, `PYTHONHASHSEED=0`, `TZ=UTC`, `NO_COLOR` and
  upgrade/nag/banner suppression come from the existing `Executor`.
- Only declared nondeterministic wall-clock fields and the internal lock
  artifact are excluded; every other behavioral difference is recorded (raw
  scalar `depends_on`, missing `spec_kitty_version`, validate warning classes,
  init asset counts, create gating, finalize/implement refusal contracts).
- Oracle is `.venv-rust-oracle/bin/python` with `PYTHONPATH=<repo>/src` and
  `SPEC_KITTY_PACKS_ROOT=<repo>/packs`; no installed-tool fallback and no
  production Python path in either native binary.
- `bench.py selftest` remains 19/19; tests ran only in temporary fixture trees.
