# Coordination transactions — implementation report

> Coordinator integration note: the YAML dependency is now declared. MissionHold
> captures the actual guard's key/path/holder instead of reconstructing them;
> inferred return types already expose these methods, so no Mission-lock type
> re-export is necessary for diagnostics. A regression covers a renamed legacy
> directory. Scoped verification is now **127 passed**, strict Clippy/fmt clean.
> The remaining functional gaps and lack of independent approval below persist.

Checkpoint: `ff4452b02`. Python baseline: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Scope: `crates/sk-workspace/**` only. **This is not an independent review, not an
approval of the safe-commit substrate, and not a claim of production CLI
readiness.** The CLI still routes coordination to refusals; the integration
patches in §6 are proposals, not applied changes.

## 1. What this tick actually added

Before this tick, `sk-workspace` answered placement questions (`artifact`,
`surface`, `coordination`, `policy`) and performed lane allocation, but there was
no coordination **transaction store**: nothing seeded or materialized a
coordination surface, nothing committed bookkeeping, and the protected set was
always the caller's (empty by default at the `sk-git` boundary). The following
are new, executable behaviours with real-repository tests:

| Area | Implementation | Tests |
| --- | --- | --- |
| Live protection policy | `protection::ProtectionPolicy` — canonical config table, explicit list replaces defaults, `[]` unprotects, absent key adds the remote default, malformed document/list refuses, operator hatch, `single_branch` `commit_to_target`, `is_protected_target`, `branch_set()` for `sk-git` | `tests/protection_policy.rs` (15), `protection::tests` (9) |
| Branch/lane tips + occupancy | `topology::mission_branch_tips`, `mission_occupancy`, `record_lane_tip{,_at}`, `read_lane_tip`, `lane_tip_ref` | `tests/topology_occupancy.rs` (6) |
| Coordination surface read | `read_coordination_surface`; `MissionWorkspace::probe_coordination` rewritten to the four-state answer (`Materialized`/`Empty`/`Unmaterialized`/`Deleted`) with husk detection, branch-mismatch refusal and no mutation on read | `tests/coord_transaction.rs` (18) |
| Materialization + seeding | `CoordTransaction::acquire` attaches an existing branch as the canonical worktree, or seeds an `EMPTY` surface from the primary checkout's COORD-partition artifacts with a `Spec-Kitty-Coordination-Seed` trailer | same |
| Transactional commit | `write_artifact` (confined, snapshotted), `stage_path`, `commit` (pinned parent + exact bytes), typed `Landed`/`NoOp`/`NotLanded`/`Indeterminate` | same |
| Ownership receipts/proofs | `CoordReceipt { outcome, proof, state_before, establishment, seed, journal_path }`; `OwnershipProof` minted from observed facts + the two held locks | same |
| Durable store + recovery | `CoordJournal` under `<common>/spec-kitty-workspace/coord/`, `recover_interrupted` proving applied/abandoned/conflict from Git state | same |
| Creation orchestration | `bootstrap_coordination` — mint-or-validate the declared coordination branch at the target tip, then materialize/seed; refuses unaccounted-ahead and diverged branches | same |
| Planning commit | `PlanningCommitPlan::prepare/apply` (partition guard, metadata demotion guard, pin guard, pinned bytes), `record_planning_commit` | `tests/planning_commit.rs` (12) |
| Planning/dependency integration | `integrate_tip`, `integrate_planning_commit`, `integrate_dependency_lanes` — real fast-forward with clean-checkout resync; `MergeRequired` when a merge commit is needed | same |

Locks: every mutating path takes the kernel **checkout-claim** lock first and
then the canonical Mission status lock through `sk_mission::lock_mission`
(claim-before-Mission order, alias holds registered by that door). Coordination
journal writes take their own `sk-kernel` file lock. No second lock protocol was
introduced.

## 2. Transactions covered (exact behaviour)

### 2.1 Reading a coordination surface

`read_coordination_surface` is read-only. States:

- `Materialized` — the canonical `.worktrees/<slug>-<mid8>-coord` is registered
  on the declared branch **and** `<worktree>/kitty-specs/<slug>-<mid8>` is a
  directory → reads resolve the coordination directory.
- `Empty` — the same worktree is registered but the Mission directory is absent
  → reads resolve the **primary** directory (`degraded_to_primary = true`) and
  nothing is created. Verified by
  `reading_an_empty_surface_degrades_to_primary_without_mutating`.
- `Unmaterialized` — no worktree is registered; the branch exists locally, or
  only on a remote (`RemoteBranchLookup::Hit`/`Error` fail closed toward
  present), or a stray directory exists at the canonical path
  (`stray_directory`).
- `Deleted` — the declared branch is absent locally and on every reachable
  remote.
- `None` — the stored topology is coordination-less.

A registered worktree on a *different* branch is a
`COORDINATION_WORKTREE_BRANCH_MISMATCH` refusal; it is never auto-switched.

### 2.2 Acquire → materialize/seed → commit

`CoordTransaction::acquire(workspace, operation, policy)` on a
coordination-routed mission:

1. resolves the surface read-only (above);
2. refuses a protected destination, a deleted branch, an undeclared branch, and
   a remote-only branch, all **before** any mutation;
3. takes the checkout claim for the write checkout (the coord worktree path,
   even before it exists) and then the canonical Mission lock;
4. journals an intent, then:
   - `Unmaterialized` local head → `plan_worktree_add(ExistingBranch)` +
     apply, journaled as `Materialize`;
   - `Empty` registered worktree → seed: carry `kitty-specs/<dir>/**` files
     whose kind is COORD-partition, refuse symlinks, refuse to overwrite a file
     that appears mid-seed, commit with the seed trailer;
   - `Materialized` → reuse, verifying `HEAD` is the destination branch;
5. re-probes, opens the write checkout, pins the observed tip, journals the
   `Commit` intent, and mints the `OwnershipProof`.

`commit(message)` builds a `SafeCommitRequest` with
`with_protection(policy.branch_set(), Standard)`, `with_expected_parent(tip)`
and `with_expected_bytes(...)` for every artifact this transaction wrote. Outcome
mapping:

- `Ok` → `Landed` (commit object recorded as applied in the journal);
- `SafeCommitNothingToCommit` → `NoOp { head }` (proved idempotent);
- `SafeCommitLanded { recovery, detail }` → `Landed` with
  `index_repair = detail`; nothing is undone;
- `SafeCommitAdvanceUnconfirmed { candidate, .. }` → re-read the branch:
  candidate == tip → `Landed`; tip == observed tip → `NotLanded` and the
  transaction's own writes are restored from snapshots; anything else →
  `Indeterminate`, journaled as such, nothing undone;
- any other Git error → journal `Abandoned`, snapshots restored, error returned.

Dropping an uncommitted transaction restores its writes and resolves the intent
as abandoned; `abort(detail)` does the same explicitly; `seal(detail)` resolves
an establishment-only transaction (used by bootstrap) without inventing a
commit.

### 2.3 Seeding safety (no fork, no silent restore)

Seeding is allowed only when the coordination branch carries **no
coordination-only commits** (`unique_commits(branch, [], None)` is empty), i.e.
the branch is at/behind its creation base. Otherwise:

- if this store recorded an applied seed whose commit is reachable from the tip
  → `COORD_SURFACE_RESTORE_REQUIRED`;
- else → `COORD_SEED_FORK_REFUSED`.

Both refuse rather than fork the log or overwrite committed coordination state.
Verified by `seeding_after_a_lost_directory_refuses_rather_than_forking`.

### 2.4 Journal and recovery

`CoordJournal` records `{kind, phase, mission_slug, operation, branch, worktree,
observed_tip, candidate_commit, recorded_at, detail}` under
`<git-common-dir>/spec-kitty-workspace/coord/<digest>.json`, written under a
kernel lock with atomic replacement. `recover_interrupted`:

- `Materialize`: registered → applied; path absent → abandoned; path present but
  unregistered → **conflict** (never deleted or adopted);
- `Seed`/`Commit`/`Integration`: candidate == tip → applied; source reachable
  from tip (integration) → applied; tip unchanged from the observation →
  abandoned; anything else → conflict naming both tips and the candidate.

Nothing is undone; a landed commit is never rolled back.

### 2.5 Planning commits

`PlanningCommitPlan::prepare` refuses, in order: missing target, primary
checkout not on the target branch (`PLANNING_COMMIT_HEAD_MISMATCH`), protected
target, empty path set, traversal/escape paths, COORD-partition artifacts
(`PLANNING_PARTITION_REFUSED`), an uncommitted `meta.json` that drops
`coordination_branch` relative to the caller's observed baseline
(`META_DEMOTION_REFUSED`), and an orphaned/foreign recorded pin
(`PLANNING_PIN_UNUSABLE`). `apply` commits with the observed target tip as the
pinned parent and each file's exact bytes asserted. `record_planning_commit`
writes the landed SHA into `lanes.json` through the locked store.

`classify_recorded_pin` mirrors the upstream four-way classification (verified
against the oracle, §5).

### 2.6 Integration

`integrate_tip` fast-forwards the destination when the destination tip is an
ancestor of the source tip (plan → apply with clean-checkout resync). It reports
`AlreadyIntegrated` when the source is already reachable, and `MergeRequired`
(naming both tips, mutating nothing) when the histories diverged.
`integrate_dependency_lanes` walks `depends_on_lanes` in
`(parallel_group, lane_id)` order, stops at the first `MergeRequired`, and
returns the real advances it already performed. A missing dependency branch is a
typed `DEPENDENCY_BRANCH_MISSING` refusal (no zero-OID placeholders).

## 3. Invariants honoured

- **mid8 identity.** Every name comes from `MissionIdentity`/`MissionId::mid8`;
  the journal digest, Mission-lock key and coordination Mission directory use
  the canonical `<slug>-<mid8>` name. No legacy tail heuristic was added.
- **No invented identities or permissions.** The protected set, destination
  branch and worktree branch are read, never guessed. The mission lock is the
  canonical door; the claim lock is the kernel family.
- **Claim before Mission.** Enforced structurally (`CoordLocks::acquire` claims
  first; the kernel itself refuses a claim taken under a Mission lock).
- **Landed commits are immutable.** No `git checkout --`, no reset, no branch
  move backwards; `NotLanded` restores only bytes this transaction wrote.
- **No production Python.** No fixture or module invokes Python; the oracle is
  development tooling used to derive expectations (§5).

## 4. Verification performed in this tick

Toolchain: Rust 1.99.1, `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, existing
`target/`, `--locked`, temporary repositories/HOME/XDG/TMP only. No aggregate
workspace build, no release build, no timing run (coordinator-owned).

```
cargo test  -p sk-workspace --locked            → 126 passed, 0 failed
cargo clippy -p sk-workspace --all-targets --locked -- -D warnings → clean
cargo fmt    -p sk-workspace -- --check         → clean
```

Suite totals: 45 lib (9 protection unit tests included) + 10 allocation + 7
artifact_home + 18 coord_transaction + 4 lanes_store + 7 lifecycle_workspace + 4
owned + 12 planning_commit + 15 protection_policy + 6 topology_occupancy, plus 0
doctests. New coverage is listed in the README.

Not exercised by fault injection in this crate: the `Landed (index repair)` and
`Indeterminate` arms (they need substrate-level fault injection, which the
`safe-commit` owner owns); the `NotLanded` arm **is** exercised with a failing
`reference-transaction` hook (`a_blocked_ref_advance_is_not_landed_and_the_write_is_restored`).
The journal side of all four arms is covered by recovery tests.

## 5. Differential checks against the pinned Python oracle

`.venv-rust-oracle/bin/python`, `PYTHONPATH=src`,
`SPEC_KITTY_PACKS_ROOT=$PWD/packs`, throwaway repositories under a temporary
HOME/XDG/TMP. Recorded outputs (`/tmp/sk-ws-oracle-*.txt` at the time of the
run):

Protection (`ProtectionPolicy.resolve`):

```
absent:            ['main', 'master'] True False False
explicit [release]:['release'] False True False
empty []:          [] False False False
flow {..[trunk]}:  ['trunk'] False False True
protection: yes:   ['main', 'master'] True False False
malformed list:    ProtectionConfigError ... must be a list, got str
top-level sequence:ProtectionConfigError ... must be a YAML mapping at the top level
remote default:    ['main', 'master', 'trunk']
```

All eight are encoded as native tests in `tests/protection_policy.rs`.

Coordination surface (`probe_coord_state`):

```
unmaterialized → unmaterialized
worktree added, no mission dir → empty
mission dir added → materialized
mission dir removed → empty
worktree removed + branch deleted → deleted
```

Native `probe_coordination` matches these states; the one deliberate difference
is that a stray/unregistered directory at the canonical path is reported as
`Unmaterialized { stray_directory }` instead of upstream's path-only
`Empty`/`Materialized` answer, consistent with this workspace's existing
"name proposes, registry disposes" husk policy.

Pin classification (`classify_recorded_pin`) and lane-tip naming:

```
advanced / orphaned / foreign / indeterminate
tip_ref("kitty/mission-demo-01KNXQS9-lane-a") =
  refs/spec-kitty/lane-tip/kitty/mission-demo-01KNXQS9-lane-a
```

Both match `tests/planning_commit.rs` and `tests/topology_occupancy.rs`.

## 6. Exact remaining integration needs

### 6.1 `sk-git` (blocks upstream-exact cases)

1. `GitRepository::read_blob_at_ref(rev: &str/&RefName, path: &GitPath) ->
   Option<Vec<u8>>` and `tree_paths_at_ref(rev, pathspec) -> Vec<GitPath>`
   (one bounded `git show <rev>:<path>` / `git cat-file --batch` and one
   `git ls-tree -r --name-only <rev> -- <pathspec>` behind the existing seam).
   Unblocks: (a) post-fix `EMPTY` restore from the coordination tip
   (`_coord_tip_relpaths` / `_restore_coord_kind_paths_from_tip`), replacing the
   current `COORD_SURFACE_RESTORE_REQUIRED` refusal; (b) the upstream
   `HEAD:meta.json` demotion baseline (see 6.2); (c) a seed-trailer probe that
   does not depend on this crate's journal.
2. `plan_merge` / `apply_merge`: a pinned-HEAD three-way merge start with typed
   conflict evidence (path list), plus reuse of the existing
   `plan_merge_conclusion`/`abort` as the conclusion leg. Unblocks non-fast-forward
   dependency and planning merges, which today return `MergeRequired`.
3. `merge_base(a, b) -> Option<Oid>` (or a `probe_merge_base`), so
   `LaneAllocator::guard_planning_ancestry` stops approximating with
   `is_ancestor` in both directions.
4. `plan_worktree_prune` (guarded: only registrations Git already reports
   `prunable`, and only when no other registration loses a live directory), so
   interrupted materialization can be repaired instead of reported as a conflict.
5. Sparse-checkout operations (`init --no-cone`, per-worktree pattern write
   resolved via `rev-parse --git-path`, `read-tree -mu HEAD`) for the lane
   isolation policy.
6. A remote default-branch read (`ls-remote --symref <remote> HEAD` or
   `symbolic-ref refs/remotes/<remote>/HEAD`): today `ProtectionPolicy` reads the
   loose `refs/remotes/origin/HEAD` file and returns no augmentation when it is
   absent, whereas upstream then asks the remote.

### 6.2 `sk-mission` (small, unblocks exact metadata baseline)

- Re-export the concrete Mission lock guard (or add a `MissionLockView` trait
  with `path()`, `key()`, `holder()`) from `sk_mission`. `MissionHold` currently
  type-erases `sk_status::mutation::MissionLock` to avoid a dependency on
  `sk-status`, and derives the diagnostic lock path with
  `sk_kernel::feature_status_lock_path(common_dir, canonical_dir_name)`.
  Re-exporting removes the derivation and lets `OwnershipProof.mission_lock_path`
  report the actual held file.
- Optionally expose the canonical `read_primary_meta` selector so
  `ProtectionPolicy::for_mission` reads `meta.json` through the same door instead
  of `list_missions` + `select_mission`. Behaviour is equivalent for
  non-ambiguous selectors; ambiguous selectors currently grant no bypass (fail
  closed).
- The planning-commit demotion guard currently compares the working `meta.json`
  with the caller's observed baseline mapping. Switch its baseline to
  `sk_git::read_blob_at_ref("HEAD", path)` once 6.1(1) exists.

### 6.3 `crates/sk-workspace/Cargo.toml` (coordinator-owned)

- Add `serde_yaml_ng.workspace = true` and replace the narrow
  `protection` scanner with the same lossless YAML reader `sk-doctrine` uses (or
  extract that reader into `sk-kernel::config` so both crates share it). The
  scanner is documented as refusing anchors/tags/multi-document streams; a real
  reader removes that narrowness. This tick did not edit the manifest.

### 6.4 CLI patches (proposed, not applied)

`crates/sk-cli/src/compat/*` still contains the independent review's P1
findings. The intended wiring:

1. `mission create` (coordination topology): after
   `MissionCreatePlan::commit`, before the target scaffold commit, call
   `sk_workspace::bootstrap_coordination(&workspace, &ProtectionPolicy::resolve_for_mission(root, slug)?)`.
   On success emit `{coordination_branch, coordination_branch_created,
   coordination_worktree, seed_commit}` instead of `PlanningArtifactsOnly`; on a
   typed refusal leave the scaffold uncommitted and surface the code. This is
   the "real executable orchestration API" the refusals must be replaced with.
2. `agent status materialize` / status emit on a coordination-routed mission:
   replace the pre-mutation refusal with
   `CoordTransaction::acquire → write_artifact → commit`; map
   `CoordCommitOutcome::Indeterminate` to a non-zero exit naming candidate and
   observed tip, and `NotLanded` to a retryable failure.
3. `plan` / `finalize-tasks`: `PlanningCommitPlan::prepare` →
   `apply` → `record_planning_commit`, deleting the local "spec exists" gate and
   the local HEAD/protection logic. This closes review P1-5 and P1-1 at the
   planning seam.
4. `implement` / lane allocation: build `LaneAllocator` with
   `ProtectionPolicy::resolve_for_mission(root, mission)?.branch_set()` instead
   of the hardcoded `main`/`master` default, closing P1-1 for lanes and
   `workspace recover`.
5. `workspace recover` / `implement --recover`: call
   `sk_workspace::recover_interrupted` for coordination records and
   `LaneAllocator::recover_interrupted` for lanes, then render both reports.
   The upstream `--recover` JSON shape stays an `sk-status` projection.
6. `next`/`runtime query`: use an existing-run lookup only (out of this crate's
   scope; listed because the review ties it to the same mutation boundary).

### 6.5 Explicitly not claimed

- No independent review of this tick or of the safe-commit substrate.
- No production CLI parity: the CLI has not been rewired, so creation still
  reports `PlanningArtifactsOnly` and coordination writes still refuse.
- No Windows/macOS verification; path identity is byte-exact/case-sensitive.
- No performance claim; the pure-path benchmark in the README is unchanged and
  does not cover these transactions.
- The coordination worktree is created without the upstream lane sparse-checkout
  policy (see 6.1(5)); lane worktrees are still created by the existing
  allocator, which this tick did not change.
