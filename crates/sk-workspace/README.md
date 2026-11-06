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

### Live protection policy (`protection`)

- `ProtectionPolicy::resolve` / `resolve_with_hatch` — the live contract from
  `.kittify/config.yaml`: an explicit `protection.protected_branches` list
  replaces the name defaults exactly, `[]` unprotects everything, an absent key
  adds the remote default branch to `{main, master}`, and a malformed document
  is a typed `ProtectionConfig` refusal. `SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS`
  is the operator hatch.
- `ProtectionPolicy::scoped_to_mission` / `resolve_for_mission` /
  `for_mission` — a `single_branch` mission's `commit_to_target: true`
  un-protects exactly its own `target_branch`. A non-boolean value, an
  unstored/other topology, or a missing target grants no bypass.
- `is_protected_target(branch, primary_branch)` — the primary branch counts as
  protected even when an explicit list replaced the defaults.
- `branch_set()` — the resolved set as `sk-git`'s `ProtectedBranches`, with the
  hatch/bypass already applied, so the adapter never sees an empty default.

### Branch tips and occupancy (`topology`)

- `mission_branch_tips` — target/mission/coordination tips, the recorded
  planning commit, and every code lane's live tip plus recorded lane-work tip.
- `mission_occupancy` — which registered checkouts hold each lane branch,
  their dirty entries, unregistered stray directories at predicted paths, and
  the shared repository-root checkout with its planning-lane work packages.
- `record_lane_tip` / `record_lane_tip_at` / `read_lane_tip` / `lane_tip_ref` —
  `refs/spec-kitty/lane-tip/<branch>` recording through a compare-and-swap
  plan. Recording is monotonic: a tip unrelated to the record refuses
  (`LANE_TIP_HISTORY_REWRITTEN`) instead of overwriting the only surviving
  record of a destroyed lane.

### Coordination transactions (`coord`)

- `read_coordination_surface` — the read-only four-state answer
  (`Materialized` / `Empty` / `Unmaterialized` / `Deleted`) with a declared
  primary fallback. It never creates a worktree, so reading an `Empty` surface
  cannot mutate the repository.
- `MissionWorkspace::probe_coordination` — registry-verified state: an
  unregistered directory at the canonical path is a reported husk, never a
  materialized surface; a checkout on another branch is a typed refusal.
- `CoordTransaction::acquire` — claim-lock-then-Mission-lock, journaled
  materialization (attach an existing branch) and seeding (carry
  COORD-partition artifacts from the primary checkout with a
  `Spec-Kitty-Coordination-Seed` trailer), then a pinned-bytes commit path.
  Seeding refuses (`COORD_SEED_FORK_REFUSED` / `COORD_SURFACE_RESTORE_REQUIRED`)
  when the branch already carries coordination-only commits this store cannot
  account for.
- `CoordTransaction::write_artifact` / `stage_path` / `commit` / `abort` /
  `seal` — confined writes with byte snapshots, exact-bytes commit assertions,
  and typed outcomes: `Landed` (immutable), `NoOp`, `NotLanded` (provably
  nothing landed; the transaction's own writes are restored) and
  `Indeterminate` (nothing undone).
- `CoordReceipt` + `OwnershipProof` — the landed commit plus the evidence that
  the write owned the checkout it names: surface, destination, worktree branch,
  observed tip, claim-lock path, and the canonical Mission lock key/holder.
- `CoordJournal` / `recover_interrupted` — a durable per-mission store under
  `<git-common-dir>/spec-kitty-workspace/coord/`. Recovery proves each pending
  record's outcome from Git state, marks landed candidates applied, clears
  provably-unlanded ones, and reports the rest as conflicts without touching
  anything.
- `bootstrap_coordination` — mint-or-validate the declared coordination branch
  at the target tip, then materialize/seed its surface. An unaccounted ahead
  branch or a genuinely diverged branch refuses; this is the workspace-owned
  half of a coordination-topology `mission create`.

### Planning commits and integration (`planning`)

- `classify_recorded_pin` — `Advanced` / `Orphaned` / `Foreign` /
  `Indeterminate` for a recorded `planning_commit_sha` against the target tip.
- `PlanningCommitPlan::prepare` / `apply` — the destination is the mission
  target branch of the primary checkout; COORD-partition artifacts are refused
  (`PLANNING_PARTITION_REFUSED`), a `meta.json` that drops `coordination_branch`
  is refused (`META_DEMOTION_REFUSED`), an unusable recorded pin is refused,
  and the commit pins the observed target tip plus every file's exact bytes.
- `record_planning_commit` — persist the landed commit into `lanes.json`
  through the locked store.
- `integrate_planning_commit` / `integrate_dependency_lanes` / `integrate_tip`
  — real fast-forward integration with clean-checkout resync, and
  `MergeRequired` (naming both tips, changing nothing) when a merge commit is
  needed.

## Verification

`cargo test -p sk-workspace` (126 tests at the time of writing) covers:

- all 23 artifact kinds, partitions, 4 topologies, 5 surfaces, and every
  `(kind, topology, coord-state)` cell;
- real temporary Git repositories for lifecycle present/absent/failed probes,
  CWD-invariant workspace resolution, ownership minting/refusals, manifest
  containment and symlink screens;
- real lane allocation: fresh, reuse, dirty refusal, crash recovery, journal
  repair of interrupted creation, unhonorable bases, protected-branch refusal,
  stale plans after a ref moves, and custom-file retention with guarded branch
  deletion;
- real protection-policy resolution: absent/explicit/empty/malformed configs,
  the remote default branch, the operator hatch, and `commit_to_target`;
- real coordination transactions: `EMPTY` reads that do not mutate,
  materialization and seeding with the seed trailer, protected/deleted refusals
  before mutation, proven no-ops, aborted-write restoration, a blocked ref
  advance reported as `NotLanded`, journal recovery of landed/unlanded/
  ambiguous records, and creation bootstrap;
- real planning commits: partition refusal, metadata demotion refusal, pin
  classification, landed-byte verification, manifest pin recording, and
  fast-forward/`MergeRequired` integration of planning and dependency tips.

`cargo clippy -p sk-workspace --all-targets -- -D warnings` and
`cargo fmt -p sk-workspace -- --check` are clean.

## Deliberate differences and remaining gaps

These are real gaps, not hidden fallbacks:

1. **`EMPTY` on a *foreign* branch (no tree read).**
   `probe_coordination` now distinguishes `EMPTY` (registered worktree, mission
   directory absent) from `UNMATERIALIZED` without mutating, and reads of both
   resolve the declared primary directory. Seeding an `EMPTY` surface is refused
   when the coordination branch already carries coordination-only commits this
   store cannot account for. Upstream distinguishes pre-fix from post-fix by
   reading a seed trailer and the COORD-kind paths *at the branch tip*
   (`git log --format=%(trailers:...)`, `git ls-tree`), which `sk-git` does not
   expose; this crate uses its own journal plus `unique_commits` instead and
   refuses where a tip read would be required.

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

5. **Lane sparse-checkout policy.**
   `CoordTransaction::acquire` creates the coordination worktree (journaled,
   registry-verified, branch-verified) and seeds it. The upstream *lane*
   sparse-checkout policy (`register_lane_sparse_checkout`: `sparse-checkout
   init --no-cone`, a per-worktree `info/sparse-checkout`, `read-tree -mu HEAD`)
   is not installed: `sk-git` exposes no sparse-checkout operation, and writing
   another checkout's gitdir files directly would bypass that seam.

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
   `mission_occupancy` now reports the primary checkout's branch/HEAD, its dirty
   entries, and the manifest's planning-lane work packages. Upstream's
   `checkout_occupancy.py` additionally folds *status-event* in-progress WP
   claims into that decision; the status projection is not consumed here, so a
   caller that needs the claim-based refusal must supply it.

9. **Lane-tip recording hooks.**
   `record_lane_tip`/`record_lane_tip_at` write `refs/spec-kitty/lane-tip/<branch>`
   through a compare-and-swap plan, and `mission_branch_tips` reads them back so
   a destroyed lane is never reported empty. The upstream *hook* that records a
   tip after every lane commit (`policy/lane_tip_recorder.py`) is not installed
   here: installing hooks into a user checkout is an application-layer action.

10. **Protected-branch configuration reader scope.**
    `protection::ProtectionPolicy` reads `.kittify/config.yaml` live (explicit
    list / empty list / absent-key remote default), the operator hatch and a
    mission's `commit_to_target`, and hands `sk-git` the resolved set. It reads
    only the `protection.protected_branches` shape with a narrow, non-reserializing
    scanner: anchors, tags and multi-document streams are refused rather than
    guessed, and the remote default is read from the local
    `refs/remotes/origin/HEAD` file (upstream additionally asks the remote with
    `ls-remote --symref` when that file is absent).

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
