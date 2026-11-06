# Rust rewrite coverage inventory

> Generated evidence, not a completion claim. The checker currently **fails**:
> 2 of 488 required entries are recorded as natively implemented, no required workflow or
> performance gate has passed.

This directory holds the exhaustive, machine-readable coverage inventory for the
Rust rewrite of Spec Kitty. It exists so the coordinator cannot mistake a tranche,
a file count, or the eleven foundation commands for the full upstream surface.

| File | Purpose |
| --- | --- |
| `python-surface.json` | Dynamic discovery of the pinned Python CLI: every reachable group, leaf, option, argument, alias, hidden flag and source reference. |
| `coverage.json` | The inventory: one entry per CLI surface element, migration, wire contract, governance/asset/state/external contract, plus workflows, performance gate, dependency pins and delivery slices. |
| `completion-report.json` (generated) | Latest checker output; regenerate with `check_completion.py --json-report`. |

## Provenance and regeneration

Baseline source: upstream `spec-kitty/spec-kitty` commit
`9aed3a8012cb5eb22c0128b14d525039f95fea7a` (2026-11-02). The oracle interpreter is
`.venv-rust-oracle/bin/python`, provisioned using
`UV_PROJECT_ENVIRONMENT="$PWD/.venv-rust-oracle" uv sync --frozen --no-dev --python 3.11`,
with `PYTHONPATH=<checkout>/src` and `SPEC_KITTY_PACKS_ROOT=<checkout>/packs`.
The preinstalled uv-tool environment is **not** lock-exact and must not be used
for conformance or performance certification. Python is development-only.

```sh
# 1. Discover the live CLI tree and the registered migration set.
SPEC_KITTY_PACKS_ROOT="$PWD/packs" PYTHONPATH=src .venv-rust-oracle/bin/python \
  scripts/rust/discover_python_surface.py \
  --output docs/rust/inventory/python-surface.json

# 2. Merge discovery with the curated contracts/workflows/performance floor.
.venv-rust-oracle/bin/python \
  scripts/rust/build_inventory.py

# 3. Check completion (exit 0 only when every gate passes).
#    Use the verified, offline-synced pack trees and lock-exact oracle.
SPEC_KITTY_PACKS_ROOT="$PWD/packs" PYTHONPATH=src .venv-rust-oracle/bin/python \
  scripts/rust/check_completion.py \
  --json-report docs/rust/inventory/completion-report.json

# 4. Prove the checker cannot be greenwashed.
.venv-rust-oracle/bin/python \
  -m unittest scripts/rust/tests/test_check_completion.py -v
```

The discovery step isolates `HOME`/XDG/cache in a temporary directory, sets
`PYTHONDONTWRITEBYTECODE=1`, and runs from an empty working directory: importing the
baseline application must not touch the checkout or the user's global `~/.kittify`
state. It resolves the built-in pack tree from the installed oracle environment (or
an existing `SPEC_KITTY_PACKS_ROOT`) and never fetches from the network.

## What "reachable" means

Discovery walks the actual Typer/Click command tree after full command registration.
It does not infer scope from filenames.

- **58** top-level registrations, **41** group objects (including the root),
  **244** reachable leaf commands, **888** options and **84** positional arguments.
- **13** reachable leaves are hidden (`merge`, the seven `merge-driver-*` drivers,
  `commit-guard-hook`, deprecated aliases, the internal sentinel).
- **82** Python files sit directly under `src/specify_cli/cli/commands/`; **181**
  include nested packages. Neither number is treated as the command count. The
  module census records all 181 with a SHA-256 and status: **89**
  registered command-source modules, **4** dynamically registered modules,
  **67** statically imported helpers and **21** helpers not reachable by static
  import (recorded, not silently dropped). All reachable registrations are
  captured with their callback source reference.
- **136** upgrade migrations are loaded from the live `MigrationRegistry` (not counted
  from migration files); each becomes a required `MIG:<id>` entry.
- Deprecated spellings are preserved as hidden options/aliases, including
  `--x/--no-x` pairs, `--feature`/`--mission`-style aliases and the injected
  accept-and-ignore `--mission` flag.

The only non-required CLI row is the internal
`__force_multi_command_mode__` sentinel. The checker pins that exclusion and rejects
any other skip.

## Entry contract

Each entry carries a stable ID (`CLI:<path>`, `MIG:<id>`, `WIRE-*`, `GOV-*`,
`ASSET-*`, `STATE-*`, `EXT-*`, `WF-*`), the argv/option surface, source and test
references, dependencies, and four independent status axes:

- `implementation`: `absent` / `partial` / `implemented`, with Rust-only refs.
- `conformance`: `none` / `partial` / `pass`, with evidence refs.
- `review`: `none` / `pending` / `approved`, with independent-review refs.
- `performance`: `not_measured` / `measured` / `pass`, with workload IDs.

Mutating entries additionally carry a documented contract: write targets, locks, Git
effects, authority functions, rollback, atomicity, user-content preservation and
recovery. Read-only entries record the authority they read through. An entry whose
family cannot be classified stays `unresolved`; the builder never drops it and the
checker refuses to complete while one remains.

## Current coverage (conservative inventory snapshot)

New implementation tranches are not promoted merely because their Rust tests
pass. The counts below remain conservative until command-specific conformance,
independent review and workflow evidence are reconciled into the inventory.

| Measure | Count |
| --- | ---: |
| Inventory entries | 489 |
| Required entries | 488 |
| Required CLI leaves | 243 (+1 frozen sentinel) |
| Reachable CLI leaves | 244 |
| CLI groups | 40 |
| Registered migrations | 136 |
| Contract entries (wire/governance/asset/state/external/migration/perf) | 52 |
| Required full workflows | 16 |
| Required performance workloads | 20 (17 strict gate + 2 startup + 1 projection) |
| Documented mutation contracts | 302 of 302 mutating entries |
| Unresolved classifications | 0 |
| Natively implemented required entries | **2** (`mission list`, `mission show`) |
| Entries with passing conformance | **0** |
| Entries with independent review | **0** |
| Passing workflows | **0 of 16** |
| Performance gate | **blocked** (initial harness evidence is partial; required workflows remain pending) |

The eleven foundation commands are not coverage. Two are upstream parity
(`mission list`, `mission show`) and count only as those entries. Nine are
Rust-authored read surfaces with no upstream argv contract (`capabilities`,
`project root`, `git inspect`, `mission graph`, `charter inspect`, `glossary lookup`,
`runtime query`, `events inspect`, `transition check`). They are recorded in
`native_extensions` with their projection/parity relationship; projections never
count toward upstream completion.

## Completion gates enforced by the checker

1. **Surface completeness.** Every live command path, option, argument, alias and
   hidden flag must exist in the inventory with the exact discovered signature.
   The committed surface hash must match the pinned source hash. Deleting a row,
   removing an option or marking a reachable command optional fails.
2. **Structural non-vacuity.** The checker rebuilds the inventory from the curated
   model and compares IDs, required flags, mutation classification, contract classes,
   migration IDs, workflow IDs and policy floors. Hand-editing scope out of
   `coverage.json` fails.
3. **Contract completeness.** All 52 required contract IDs exist; every mutating entry
   has writes/Git effects, locks or authority, rollback, atomicity, user-content and
   recovery semantics; no unresolved entry remains.
4. **Native implementation.** Every required entry is `implemented` with refs under
   `crates/` ending in `.rs`; no Python runtime/FFI (`pyo3`, `Command::new("python")`)
   is allowed. There are no success stubs.
5. **Conformance and independent review.** Every required entry has passing
   conformance evidence and an independent approval with existing refs.
6. **Full workflows.** All 16 workflows need acceptance, differential, refusal,
   interruption, concurrency and user-content tests, independent review, passing
   performance workloads and all step entries complete.
7. **Performance.** At least 14 strict-equivalence work workloads with ≥20 raw
   samples per side; geometric mean ≥3.0x; no required workload more than 10% slower;
   result equivalence true; startup measured separately and excluded; fixture,
   toolchain, machine/load and interleaved-repetition metadata present; the evidence
   bundle reloads and its hash matches.
8. **Dependency pins.** Python pins match `uv.lock`, service pins match
   `pins/*.source.yaml`, and the Rust toolchain matches `rust-toolchain.toml`.

`completion-report.json` is grouped by gate and lists every blocking finding. The
checker exits non-zero until all of them are resolved.

## Prioritized next slices

The computed `ready_work` currently contains **S2** (benchmark harness with strict
equivalence classes) and **S3** (transactional kernel/Git primitives). S1 (this
inventory and checker) is done.

1. **S2 — performance harness.** Replace pending workloads with strict-equivalence
   adapters; keep startup and projection workloads out of the aggregate; record raw
   samples, fixture identity, toolchain, machine/load and interleaved repetitions.
2. **S3 — kernel/Git primitives.** Locking, durable event append/readback, CAS ref
   transactions, worktree ownership and canonical replay with differential tests.
3. **S4 — status writes.** Native `move-task`/`mark-status`/`status emit` under the
   mission lock with the exact event envelope and verified rollback.
4. **S5 — persistent next/decisions.** Run store, cursor CAS, prompt files, pending
   decisions and human authorization.
5. **S6 — create/specify/plan/tasks.** Mission identity, planning commits, task
   finalization and lane/worktree allocation.
6. **S7 — governance/assets.** Activation ledger, DRG cascade, pack acquisition,
   skills/manifest ownership.
7. **S8–S11 — implement/review, accept/consolidate/merge/recovery, init/upgrade/
   migrations/services/tracker, and the remaining diagnostics.**
8. **S12 — cross-platform CI and 3x proof.** No completion claim without macOS/Windows
   CI, strict-equivalence measurements and independent review.

## Deliberate limits

- The inventory is a snapshot of the working tree while other agents land native
  crates. Status axes are evidence points, not predictions; regenerate them as work
  lands. A `partial` Rust module does not make its CLI entry implemented.
- Discovery cannot execute every conditional branch of the Python CLI. Branches that
  are not reachable without a project/fixture are represented by their command
  entries and contract references; they are never silently omitted. Mutating
  contracts are derived from the owning Python modules and must be confirmed by
  differential conformance, not assumed from a docstring.
- Performance evidence is not yet produced. The harness under `benchmarks/rust/` is
  incomplete and its pending workloads block the gate.
