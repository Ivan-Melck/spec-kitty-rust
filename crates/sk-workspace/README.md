# sk-workspace

Workspace, lane, coordination, and artifact-placement facts for the Rust Spec
Kitty implementation.

This crate is the native projection of the upstream bounded context spanning
`src/mission_runtime/{context,artifacts,lifecycle_phase,owned_checkout,resolution}.py`
and `src/specify_cli/{workspace,lanes,coordination,policy}`. It depends on
`sk-mission` for Mission identity/metadata/topology, on `sk-kernel` for
validated identifiers, bounded reads, atomic writes, and the cross-process file
lock, and on `sk-git` for read-only inspection and typed mutation plans. The
dependency direction is `sk-workspace -> sk-mission`; nothing in `sk-mission`
depends on this crate. There is no Git subprocess seam here: every Git
operation goes through `sk-git`.

## Implemented APIs

### Placement and artifacts (`artifact`, `surface`)

- `MissionArtifactKind` — all 23 upstream kinds, wire strings, serde, and the
  exhaustive `ALL` array.
- `ArtifactPartition` and `assert_partition_invariant()` — the PRIMARY (17) and
  COORD (6) partitions are disjoint and jointly cover every kind.
- `artifact_home_for`, `declared_read_surface`, `kind_is_coordination_residue`
  — total projections over `(kind, stored topology)`.
- `kind_for_mission_file`, `kind_for_basename`, `kind_for_directory`,
  `MissionArtifactKind::basenames` — the single file→kind classifier, including
  the filename-anchored `review-cycle-*.md` leg and the `issue-matrix.{md,json}`
  and `wps.yaml` entries.
- `TopologySurface` (5 members), `SurfaceLocations`, `translate_surface`,
  `assert_surface_totality` — the total surface→filesystem translation; an
  unresolved member refuses instead of guessing.
- `resolve_artifact_surface` with `PlacementInputs` / `CoordState` — the
  materialization-aware read projection: PRIMARY-partition kinds never probe
  coordination; coord-partition kinds on a routing topology refuse `DELETED`
  and `UNMATERIALIZED`, resolve `MATERIALIZED`, and take the declared PRIMARY
  create-window answer for `EMPTY`/`NONE`.
- `CommitTarget`, `PlacementRef` — validated single-destination ref carriers.

### Coordination writes and policy (`coordination`, `policy`)

- `coordination_write_surface` — the write-side counterpart of the read
  projection: primary-partition and coord-less missions write primary, the
  exact published/E2 short-circuit set writes primary without probing a
  torn-down coordination surface, and every other coord write requires a
  materialized coordination surface. `DecisionLog` deliberately stays
  in-mission.
- `MutationPolicy::evaluate_destination` — the pure protected-destination
  refusal, with the coordination redirect, the coord-less remedy, an explicit
  (non-blank) override reason, and a typed refusal for an unusable ref
  spelling.

### Lifecycle (`lifecycle`)

- `LifecyclePhase` — `pre_consolidation`, `consolidated`, `published`.
- `Probe<T>` — `Present`, `Absent`, `Failed`. A failed probe is never an
  absence; `derive_lifecycle_phase` returns a typed error for it.
- `baseline_merge_commit_from_meta`/`_from_mission`, `probe_target_ref`,
  `terminal_completion`, `derive_lifecycle_phase`.

### Identity and naming (`naming`)

- `MissionIdentity` derives `mid8` exactly once from the declared ULID.
- `mission_branch_name`, `mission_branch_name_required`, `coord_branch_name`,
  `coord_reconstruct_branch`, `mission_dir_name`, `coord_mission_dir_name`,
  `coord_dir_name`, `code_lane_branch_name`, `lane_branch_name`,
  `worktree_dir_name`, `worktree_path`, `strip_numeric_prefix`,
  `mid8_from_slug`.
- `PLANNING_LANE_ID` resolves to the target branch / repository-root checkout
  and has no worktree.

### Lanes (`lanes`)

- `compute_lanes` — union-find write-scope collapse, surface inference only
  when ownership is not provably disjoint, lane-level dependency edges (never
  collapses), cycle refusal, deterministic parallel groups, the planning lane,
  stable lane-id read-back from a previous manifest, and frozen started-lane
  membership with conflict refusals.
- `globs_overlap`, `infer_surfaces`, `lane_dependency_readiness`.
- `LanesManifest`/`ExecutionLane`/`CollapseReport` serde with the upstream
  `lanes.json` field names.
- `LanesStore` — fail-closed read (missing is `None`, corrupt is an error) and
  an atomic, cross-process-locked write.

### Ownership and workspace (`owned`, `workspace`)

- `OwnedCheckout` — mint-only, private fields. Minting requires a loaded
  manifest whose canonical directory is exactly
  `<owned_root>/kitty-specs/<slug>`, a non-root owned checkout, a stored
  topology, and a usable write branch. `files()` validates a whole batch for
  traversal and containment, resolving parents but keeping a symlink leaf
  (or following it on request).
- `ProjectWorkspace` — repository discovery from any worktree, canonical
  primary root, shared common dir, mission inventory/selection, and the status
  lock root (always the primary checkout).
- `MissionWorkspace` — stored topology, target branch, coordination branch,
  protected-target-aware write branch, coord worktree path, and
  `probe_coordination`.

### Allocation and recovery (`allocation`, `journal`)

- `LaneAllocator::allocate` — fresh (coord/legacy), reuse, and crash-recovery
  routes with explicit preconditions: code lane only, protected/target branch
  refused, `single_branch` + code lanes refused, unhonorable explicit bases
  refused, plan-then-apply CAS through `sk-git`.
- `LaneAllocator::recover_interrupted` — reconciles pending intents:
  `Completed`, `Reattached`, `Cleared`, or reported conflicts. Unregistered
  content is reported, never deleted.
- `AllocationJournal` — per-mission durable intent/applied/abandoned records
  under the Git common dir, written under a kernel lock with atomic replacement.
- `classify_retention` / `retire_worktree` / `retire_branch` — custom state is
  retained (retirement refuses), only untracked `.spec-kitty/` runtime residue
  may be discarded, branch deletion refuses unique commits and protected
  branches.

## Verification

`cargo test -p sk-workspace` (65 tests at the time of writing) covers:

- all 23 artifact kinds, partitions, 4 topologies, 5 surfaces, and every
  `(kind, topology, coord-state)` cell;
- real temporary Git repositories for lifecycle present/absent/failed probes,
  CWD-invariant workspace resolution, ownership minting/refusals, manifest
  containment and symlink screens;
- real lane allocation: fresh, reuse, dirty refusal, crash recovery, journal
  repair of interrupted creation, unhonorable bases, protected-branch refusal,
  stale plans after a ref moves, and custom-file retention with guarded branch
  deletion.

`cargo clippy -p sk-workspace --all-targets -- -D warnings` and
`cargo fmt -p sk-workspace -- --check` are clean.

## Deliberate differences and remaining gaps

These are real gaps, not hidden fallbacks:

1. **`EMPTY` vs `UNMATERIALIZED` coordination states.**
   `MissionWorkspace::probe_coordination` reports `Unmaterialized` for a
   declared branch with no registered worktree, because the distinction
   upstream draws depends on coordination-seeding state
   (`specify_cli/coordination/coord_seed.py`, `surface_resolver.probe_coord_state`)
   that `sk-git` does not expose. Reads therefore refuse rather than take the
   upstream `EMPTY` primary degrade. Needs a narrow `sk-git` read for
   "coordination surface seeded/empty".

2. **`merge-base` for detached-base refusal.**
   `LaneAllocator::guard_planning_ancestry` uses direct `is_ancestor` in both
   directions as a fail-closed approximation of upstream
   `git merge-base <base> <planning_sha>`
   (`specify_cli/lanes/worktree_allocator.py::_guard_base_honorable`). A shared
   but unrelated ancestor is refused rather than probed. Needs a narrow
   `sk-git` `merge-base` read.

3. **Content-presence at the primary tip.**
   Upstream
   `mission_runtime/lifecycle_phase.content_present_at_primary_tip` uses a
   two-step `rev-parse --verify` + `cat-file -e <rev>:<path>` probe to be
   squash-robust. This crate implements the phase derivation and the target-ref
   probe, but not the `<rev>:<path>` blob probe; `sk-git` has no
   path-at-commit read. Needs `GitRepository` support for
   `path_exists_at_commit`.

4. **Stale worktree registry pruning on crash recovery.**
   Upstream prunes a stale registration before re-attaching
   (`guarded_worktree_prune` then `worktree add <path> <branch>`). `sk-git`'s
   `plan_worktree_add` refuses a path git still registers, and there is no
   prune API, so recovery only re-attaches when the path is genuinely
   unregistered. A crash that left a registry entry pointing at a deleted
   directory is reported as a conflict. Needs a guarded prune plan in
   `sk-git`.

5. **Coordination-worktree creation and sparse checkout.**
   Upstream `specify_cli/coordination/workspace.py` /
   `worktree_allocator._register_sparse_checkout_if_coord` create the
   coordination worktree and register the status-file sparse-checkout policy.
   This crate resolves and probes coordination surfaces and refuses a
   non-materialized one; it does not yet create the coordination worktree or
   install the sparse-checkout policy. That belongs with the coordination
   transaction work and needs the same journaled allocation discipline.

6. **Frozen-membership provenance sources.**
   `LanePlannerInput::frozen_lane_bindings` accepts already-resolved started
   WP→lane bindings and enforces pinning, lane-mate union, removal and
   kind-change conflicts. It does not read the status event log or the
   cancellation projection itself; the caller must supply
   `specify_cli/lanes/frozen_membership.build_frozen_membership`'s equivalents.
   The bindings are consequently not certified against durable status here.

7. **Workspace context records (`.kittify/workspaces/`).**
   `specify_cli/workspace/context.py` persists a `WorkspaceContext` record per
   lane and maintains the WP/branch index used by occupancy and stale-context
   guards. This crate does not yet persist those records, so
   `_backfill_context_if_missing` / `find_context_for_wp` semantics are absent.

8. **Repo-root checkout occupancy.**
   `specify_cli/lanes/checkout_occupancy.py` inspects in-progress WPs and dirty
   paths for the shared single-branch write checkout. Only the manifest-derived
   `repo_root_lane_wps` helper is present; status-based occupancy is not.

9. **Lane-tip recording and retention policy.**
   Upstream records `refs/spec-kitty/lane-tip/*` and installs a hook
   (`specify_cli/policy/lane_tip_recorder.py`) so a destroyed lane is detectable.
   This crate's journal covers interrupted allocation, and guarded branch
   deletion refuses unique commits, but it does not install hooks or write
   lane-tip refs.

10. **Protected-branch policy sources.**
    The protected set is supplied by the caller, matching `sk-git`'s boundary.
    This crate does not read `.kittify/config.yaml`, the
    `SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS` escape hatch, or the commit-guard
    hook configuration (`specify_cli/policy/{config,commit_guard}.py`).

11. **Brand-new-name heuristics.** `mid8_from_slug` remains only as a legacy
    reconstruction fallback and is never used for a correctness path; all
    canonical names derive from `MissionIdentity`/`MissionId::mid8`.

## Local throughput measurement

`cargo run --release -p sk-workspace --example projection_bench` measures the
pure native paths and checks determinism. One recorded run on this machine
(Linux, Rust 1.99.1, release):

| Path | Raw |
| --- | ---: |
| Resolve the complete 460-cell `(kind × topology × coord-state)` matrix | 8,598 ns/op (18.7 ns per resolved cell) |
| `compute_lanes` for 24 work packages (276 overlap pairs, dependency chain) | 196,292 ns/op |
| Classify the 28-entry basename table | 2,580 ns/op (92.1 ns per path) |
| Geometric mean across the three normalised paths | 696.6 ns/op |

These are crate-local native numbers, **not** a Python/Rust ratio and not a
claim against the 3× aggregate gate. The differential harness that measures
equivalent completed workflows against the pinned Python reference belongs to
the workspace-level benchmark tooling; this crate adds no Python interpreter,
startup process, or Git subprocess to the measured pure paths. Lane planning is
`O(n²)` in work packages by construction (every pair is tested for write-scope
overlap); the per-pair glob intersection allocates the split segments.

Additional deliberate differences:

- `MissionWorkspace::probe_coordination` refuses a stored coordination-routing
  topology that records no `coordination_branch` as `Deleted`, where upstream
  `probe_coord_state` can return `NONE` and fall back to the primary checkout.
  A corrupt topology should not silently redirect reads.
- `OwnedCheckout::mint` requires the mission directory to be exactly
  `<owned_root>/kitty-specs/<slug>` (a direct child), which is stricter than
  upstream's "anywhere inside `kitty-specs`" containment.
- Path identity comparisons are byte-exact on Unix and case-sensitive
  everywhere; upstream's Windows `ntpath.normcase` identity is not modelled.
- Journal records serialize worktree paths as UTF-8; recording a non-UTF-8
  worktree path fails with a typed journal error rather than a lossy encoding.
