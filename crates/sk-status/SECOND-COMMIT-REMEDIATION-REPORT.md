# Second status commit/rollback remediation — FINAL (implementation report)

Status: **FINAL for this task. This is implementation evidence, NOT independent
acceptance.** The independent re-review of these findings is still required; a
green rerun here does not approve the tranche.

> **Coordinator follow-up after delivery:** the emit-error compensator still
> assumed no transition row existed. `emit_status_transition` can instead
> return a post-commit/readback error. The production compensator now calls
> `retire_failed_transition_evidence`, which uses `!error.may_have_mutated()`;
> uncertain writes retain evidence. A production-helper unit test covers
> `CommitIndeterminate`, `PostCommitVerification`, `EventMissing`, and a
> provable pre-write refusal control. Initial test compile failed on a missing
> `std::fs` import (`/tmp/sk-status-emit-boundary-red.log`); after fixing the
> test, the unchanged behavior failed its assertion (0/1, exit 101;
> `/tmp/sk-status-emit-boundary-assertion-red.log`). This is a typed-boundary
> unit regression, not a claimed real-CLI I/O-fault injection. The final
> coordinator aggregate is **1,966/0/8 helpers**, with strict workspace
> Clippy/fmt/release green (`/tmp/sk-native-post-boundary-*.log`). Original
> owner counts/hashes below describe the earlier delivery; `tasks.rs` now also
> includes this correction. Independent acceptance remains pending.

- Role: implementation owner for the six independently reproduced findings.
- Findings: `/tmp/sk-rust-status-commit-final-review.md` F1–F6.
- Evidence: `/tmp/rev-probe/EVIDENCE.md` and the preserved `/tmp/rev-probe/`
  fixtures (unmodified).
- Isolation: `/tmp/sk-native-f42-review-orders.md`, with the implementation
  exception: only `crates/sk-status/**` (never `mutation_lock.rs`),
  `crates/sk-cli/src/compat/tasks.rs`, `crates/sk-cli/src/compat/tasks/commit.rs`
  and the status-only test `crates/sk-cli/tests/status_mutation.rs` were edited.
  No other source, manifest, lockfile, dispatcher, capability, `sk-git`, test,
  agent, commit or push was touched. `sk-git` G1/G2 belong to the coordinator.
- Repository: `spec-kitty-rust`, `rust/rewrite`, frozen review baseline
  `f42d5380b0d3be11773e0b2354eb73e9c2a2c47d`.
- Reference: `spec-kitty/spec-kitty`
  `9aed3a8012cb5eb22c0128b14d525039f95fea7a`; pinned oracle
  `.venv-rust-oracle/bin/python` 3.11.15.
- Every Cargo invocation (tests, doctests, clippy, fmt, build) ran through
  `bash /tmp/sk-focused-cargo.sh` (existing `target/`, two jobs, no incremental,
  shared lease, temporary HOME/XDG/TMP). No aggregate/release gate, no
  source-wide format, no deletion of evidence, no background/poll loop.

---

## 0. Initial red state (before any production edit)

Command:

```sh
bash /tmp/sk-focused-cargo.sh test -p sk-cli --locked --test status_mutation
```

**48 passed; 8 failed** (`/tmp/sk-second-remediation-initial-red.log`). The eight
new regressions (F1a, F1b, F2, F3, F4 topic, F4 missing-meta, F5, F6) failed
against the frozen source on the intended semantics:

| Regression | Observed frozen behavior |
| --- | --- |
| F1a refused rollback | Hook corrupted `.git/HEAD`; rollback refused, but the artifact was **deleted** (`pD` shape). |
| F1b side-committed | Hook `git add -A && git commit --no-verify` then `exit 1`; the committed artifact was **deleted from the worktree** (`fs::read` `NotFound`; `pH` shape). |
| F2 protected `mark-status` | `PROTECTED_BRANCH_COMMIT_REFUSED`, exit 1, **0 annotation rows** on the default project. |
| F3 `lanes` mark-status | A commit was created (`head` moved). |
| F4 topic branch | `AUTO_COMMIT_TARGET_CHECKOUT_REQUIRED` before any mutation. |
| F4 no meta `target_branch` | `auto-commit needs a stored meta.json target_branch` refusal. |
| F5 valued flag | `--no-auto-commit=true` **committed** (exit 0). |
| F6 prior artifact feedback | Accepted; `review-cycle-2.md` wrapped the whole prior artifact. |

Two F1 test expectations were corrected after that run (the row count is
`before + 1` because a refused rollback *retains* the new row; F1b likewise).
F1b had already failed on the deleted artifact itself; F1a's artifact deletion is
independently recorded in the reviewer's preserved `pD` evidence
(`artifact exists? no`). No production behavior was changed before the red run.
The F1 regressions were first driven through the route function the dispatcher
calls; they were then converted to spawn the real `spec-kitty` binary (as F2–F6
already did) and re-verified green, so every final regression is a real-CLI one.

## 1. Oracle grounding (pinned source + pinned runtime)

All probes use the pinned oracle only (`PYTHONPATH=$PWD/src`,
`SPEC_KITTY_PACKS_ROOT=$PWD/packs`). Logs: `/tmp/sk-second-oracle.log`.

**F5 — Typer boolean pair.** A minimal app declaring
`auto_commit: bool | None = typer.Option("--auto-commit/--no-auto-commit")`:

```
--auto-commit                        -> {"resolved": true}
--no-auto-commit                     -> {"resolved": false}
--auto-commit --no-auto-commit       -> false
--no-auto-commit --auto-commit       -> true
--auto-commit --auto-commit          -> true
--no-auto-commit --no-auto-commit    -> false
--auto-commit=true|false             -> exit 2
--no-auto-commit=true|false          -> exit 2
```

This pins every F5 behavior the implementation now reproduces: valued spellings
are parse errors, repeats are last-wins, the pair is last-wins, and the negative
spelling never yields `true`.

**F3 — mark-status commit predicate.** Store `topology` in a real mission and
call the pinned authority:

```
STORED lanes         -> MissionTopology.LANES          is_single_branch False
STORED single_branch -> MissionTopology.SINGLE_BRANCH  is_single_branch True
STORED None          -> MissionTopology.LANES          is_single_branch False
```

The pinned source is
`tasks_mark_status.py::_ms_emit_subtask_state`:
`commit_annotation = st.resolved_auto_commit and is_single_branch(resolve_topology(...))`.
The same probe records that `tasks_mark_status.py` **never calls**
`_protected_branch_status_commit_error(`, while
`tasks_shared._protected_branch_status_commit_error` still defines the
`move-task` refusal text.

**F2 — degrade arm.** `coordination/status_transition.py::_annotation_degrades_uncommitted`
is keyed on `identity.owned is None and annotation_topology == SINGLE_BRANCH and
refusal.verdict.error_code == PROTECTED_BRANCH_REFUSED`; its warning is
`"Status annotation for %s %s was written but not committed: the mission records
directly on a protected branch."` (`tasks_verdict_persistence.py` also pins
`_DURABILITY_REASON_PROTECTED_TARGET_BRANCH = "protected_target_branch"`).

**F4 — destination.** `tasks_shared._ensure_target_branch_checked_out` resolves
`resolve_target_branch(..., respect_current=True)` and returns
`(main_repo_root, resolution.current)`. A real probe with a `topic-branch`
checkout and a `meta.json` **without** `target_branch` returned
`branch=topic-branch` with no checkout and no refusal.

**F1 — deletion condition.** The reference's own compensator
(`review/cycle.py`'s `except ReviewCycleError: artifact_path.unlink(missing_ok=True)`
and `tasks_verdict_persistence.py::revert_committed_verdict_write`) only unwinds
an **uncommitted** write it just made, and explicitly never deletes an adopted
or committed prior record (`#5969`). The implementation applies the same proof
order to the evidence file that the rollback already applies to the log.

**F6 — provenance guard.** `review/cycle.py::_guard_feedback_source_provenance`
has two independent legs: strict path identity
(`_REVIEW_CYCLE_FILE_RE = ^review-cycle-(?P<cycle>[1-9][0-9]*)\.md$` inside
`sub_artifact_dir`) and content identity (`ReviewCycleArtifact.from_file`
parses). `_resolve_review_body` runs it after the read/empty validation for
every `feedback_source`, including the approval leg
(`_approval_feedback_source`, #6652).

## 2. Fixes delivered

### F1 — evidence deletion only after proof

`crates/sk-cli/src/compat/tasks.rs`:

- New `retire_uncommitted_evidence(authority, path, status_proven_uncommitted)`.
  It removes the artifact only when **both**:
  1. the status tail is provably absent/uncommitted (no row persisted, or the
     verified rollback returned `rolled_back: true`); and
  2. the destination branch has no blob at the artifact path.
- `crates/sk-cli/src/compat/tasks/commit.rs::CommitAuthority::path_committed`
  reads `refs/heads/<destination>` explicitly (no `HEAD` guess). A missing ref is
  provably absent; an unreadable ref is **not** proof and retains the file.
- `NotLanded`: refused rollback (`HeadUnreadable`, `TailAlreadyCommitted`,
  `TailNotOwned`, I/O) and unknown/indeterminate outcomes retain rows **and**
  artifact, and the refusal text now names the retention. A successful rollback
  deletes the artifact only when the destination does not already hold it.
- The transition-emit failure arm uses the same guard (`status_proven = true`
  because no row exists) instead of an unconditional `remove_file`.
- No deletion path runs for `Indeterminate` (already true) — rows and evidence
  stay for inspection.

### F2 — protected `mark-status` degrades; `move-task` still refuses

- `commit::prepare` no longer refuses a protected destination; it reports
  `protected_destination`.
- `move_task` refuses immediately after `prepare` with the canonical
  `PROTECTED_BRANCH_COMMIT_REFUSED` message/remedy (reference
  `_protected_branch_status_commit_error`); existing regression preserved.
- `mark_status` commits only for `single_branch`; if that destination is
  protected it writes the annotation **locally**, prints the canonical
  `_annotation_degrades_uncommitted` warning on stderr, and reports the additive
  native trace `durability_classification: "persistence_failed"`,
  `durability_reason: "protected_target_branch"`, `destination_ref`.
- Wired the previously unused `DURABILITY_REASON_PROTECTED_TARGET_BRANCH`.

### F3 — `mark-status` auto-commit is `single_branch` only

- New `commit::annotation_commit_topology` = `topology == "single_branch"`.
- `mark_status` takes the transactional lock/commit only then; `lanes` and
  flat/legacy annotations are the uncommitted local write. `move-task` keeps its
  existing `transactional_topology` (`lanes | single_branch`), whose `lanes`
  destination remains a recorded residual (not re-litigated here).

### F4 — canonical `respect_current` destination

- `prepare` resolves the destination from `GitRepository::current_branch`
  (the reference's `resolution.current`), never from `meta.json`.
- A missing stored `target_branch` is not a refusal; no checkout, no
  `AUTO_COMMIT_TARGET_CHECKOUT_REQUIRED`. A detached HEAD is the reference's
  `Detached HEAD — checkout a branch before continuing`.
- `destination_ref` is reported for landed status-only commits too; a landed
  review-cycle verdict continues to report its own `evidence_ref` +
  `destination_ref`. One commit remains the composition; the traced destination
  is the respected current branch, not the stored target.

### F5 — exact Typer boolean pair

- Both options are bare flags (`ArgAction::Count`, `overrides_with` the sibling).
  Valued spellings (`--auto-commit=false`, `--no-auto-commit=true`, …) are parse
  errors (exit 2) before any route work.
- `explicit_auto_commit` is presence-only: the negative flag can only select
  `Some(false)`; repeats and the pair are last-wins. Unit tests pin the parser;
  real-CLI tests pin commit/no-commit outcomes.

### F6 — prior-artifact provenance guard

- `crates/sk-status/src/mutation_commit.rs` adds:
  - `is_review_cycle_filename` (strict `[1-9][0-9]*`, no leading zero);
  - `parses_as_review_cycle_artifact` (the pinned `from_dict` required fields and
    types, non-mapping/unparseable refused);
  - `review_feedback_provenance` (independent path + content legs, resolved
    paths);
  - the canonical refusal strings.
- `move_task` runs the guard after the feedback read/empty check for every edge
  that accepts a feedback `feedback_source` (rejection **and** approval/done),
  before the artifact is allocated or written. Plain reviewer prose that does
  not parse is still admitted (FR-004), even if byte-identical to a prior body.

## 3. Regressions added (all red-first, real `spec-kitty` binary + real Git)

`crates/sk-cli/tests/status_mutation.rs` (+9 tests, 48 → 57):

| Finding | Test |
| --- | --- |
| F1 | `f1_a_refused_rollback_retains_the_transition_row_and_its_review_cycle_evidence` (real binary, corrupted-`HEAD` hook, verifies retained row + pointer + artifact) |
| F1 | `f1_a_hook_side_commit_keeps_the_committed_evidence` (real binary, hook side-commits via the safe-commit temp index, verifies `HEAD:path` bytes == worktree bytes and the retained pointer) |
| F2 | `f2_mark_status_degrades_to_an_uncommitted_annotation_on_a_protected_branch` (real binary, default project) |
| F3 | `f3_mark_status_leaves_lanes_annotations_uncommitted` (real binary, `lanes` and flat) |
| F4 | `f4_move_task_commits_on_the_respected_current_branch_not_the_meta_target` (real binary, `topic-branch`, meta still `main`) |
| F4 | `f4_move_task_commits_when_meta_has_no_stored_target_branch` (real binary) |
| F5 | `f5_auto_commit_flags_reject_valued_spellings_and_resolve_polarity_last_wins` (real binary; 4 valued spellings × no-write/no-commit, 6 bare-flag precedence cases) |
| F6 | `f6_move_task_refuses_a_prior_review_cycle_artifact_as_review_feedback` (real binary; path leg, content-copy leg, plain-prose positive control) |
| F6 | `f6_move_task_refuses_a_prior_review_cycle_artifact_on_an_approval_edge` (real binary, `feedback_source` on approval) |

`crates/sk-status/src/mutation_commit.rs` (+3 unit tests, 231 → 234):
strict filename grammar; canonical-artifact parse vs prose/missing-field;
path/content/prose provenance matrix.

Retained unchanged: configuration precedence/refusals, artifact byte fidelity,
pointer/numbering, operator-index path scope, no-op flattening, flat topology,
64 MiB committed-log probe, YAML mirror fidelity, all prior rollback/probe
tests.

## 4. Verification (final; current combined working tree)

| Command | Result | Log |
| --- | --- | --- |
| `test -p sk-status --locked` | **234 passed, 0 failed, 1 ignored**, 20 suites (helper parent-exercised; includes doctests) | `/tmp/sk-second-final-sk-status.log` |
| `test -p sk-cli --locked --lib` | **8 passed, 0 failed** | `/tmp/sk-second-final-cli-lib.log` |
| `test -p sk-cli --locked --test status_mutation` | **57 passed, 0 failed** | `/tmp/sk-second-final-status-mutation.log` |
| `test -p sk-cli --locked --test compat` | **26 passed, 0 failed** (real-binary dispatch, bare `--no-auto-commit`) | `/tmp/sk-second-final-compat.log` |
| `clippy -p sk-status --all-targets --locked -- -D warnings` | clean | `/tmp/sk-second-final-clippy-status.log` |
| `clippy -p sk-cli --lib --test status_mutation --no-deps --locked -- -D warnings` | clean | `/tmp/sk-second-final-clippy-cli.log` |
| `fmt -p sk-status -p sk-cli -- --check <4 owned files>` | clean | `/tmp/sk-second-final-fmt.log` |
| `build -p sk-cli --locked` | up to date, exit 0 | `/tmp/sk-second-final-build.log` |

Oracle probe logs: `/tmp/sk-second-oracle.log` (pair, stored topology,
`respect_current`/missing meta, source predicate greps).
Initial red: `/tmp/sk-second-remediation-initial-red.log` (48 passed / 8 failed).

## 5. Source hashes of the changed owned files

```
076bbe2a81f2f61a43e668000a7204546da4008fa8668a18fd2fb746a488fb5b  crates/sk-status/src/mutation_commit.rs
e03aba0165d16e520c560e6d4faa0d9a595181ce2a2d3dc22c9607dbcd5dd278  crates/sk-cli/src/compat/tasks.rs
31284daf9ef745c9912857dead550d8a9bc9e7c70738a9bf78d58c63086c5c86  crates/sk-cli/src/compat/tasks/commit.rs
f55c9735679d3e8ccdbb42a01c5f41ada6ed1c565abb3d2e48a97ca065e21d1c  crates/sk-cli/tests/status_mutation.rs
```

Untouched but part of the reviewed owned surface (same as the frozen review):

```
59130f0d2b8ae587ce0b84d9729c1688d49a86adeeeef47bd5695c27107cbe80  crates/sk-status/src/mutation.rs
fb1303276c69ad53d1453a38e99fbc712118bc6dd90126cfb1c0d60f9a5d6513  crates/sk-status/src/lib.rs
```

`mutation_lock.rs` was not edited.

## 6. Working-tree / dependency caveats

- The frozen review baseline was `f42d5380b`; this run happened while other
  owners' uncommitted work was present in the same tree. In addition to these
  four files and this report, `git status` shows concurrent edits by others:
  `crates/sk-git/src/merge.rs`, `crates/sk-git/{README.md,CONCLUSION-EVIDENCE-REPORT.md}`,
  `crates/sk-git/tests/conclusion_boundary.rs` (new),
  `crates/sk-workspace/src/planning.rs`, `crates/sk-doctrine/README.md`.
  None was read/edited by this task; `sk-cli` links `sk-git`/`sk-workspace`, so
  the final counts are of that combined tree, not of `f42d5380b` alone.
  `sk-git` G1/G2 are the coordinator's separate work.
- No manifest/lockfile changed; `--locked` held throughout.
- No source-changing dependency was added. The provenance guard uses the
  already-declared `serde_yaml_ng` and `std::path::absolute`; no new crate.

## 7. Residuals and remaining gaps (not waived)

1. **`move-task` on `lanes` still commits status + evidence** to the respected
   current branch. F3 was scoped to `mark-status`; the reference's
   `commit_status` destination for a `lanes` `move-task` was not traced to a
   conclusion here. Recorded, not claimed equal.
2. **Coordination materialisation remains refused**
   (`COORDINATION_SURFACE_UNAVAILABLE`); the review-cycle sibling-dir union is
   still not exercised.
3. **Owned checkout** (`--owned-checkout`) remains refused.
4. **`--to done` merge-ancestry** still requires `--done-override-reason`.
5. **Consumed draft retirement (#6632)** is still not implemented: the user's
   feedback draft is preserved. It remains a documented native difference, not
   parity.
6. **F1 index caveat for a hook side commit.** The route no longer deletes the
   committed artifact. A hook that commits through the safe-commit transaction's
   temporary index can still leave the operator's real index lagging `HEAD`
   (the review's `pH` `D`/`??` observation). Repairing that index is the
   `sk-git` safe-commit contract, outside this ownership; this route performs no
   index write or deletion.
7. **Provenance content leg parser difference.** The guard parses frontmatter
   with `serde_yaml_ng` rather than ruamel. Canonical artifacts and plain prose
   are pinned by tests; an exotic, non-canonical YAML spelling could parse
   differently. The guarantee (a re-submitted verdict record is refused) is not
   weakened for the writer's own artifacts.
8. **Allocation error wording** for an unparseable sibling and the **1 MiB
   config read bound** remain native differences recorded by the first review.
9. **`mark-status` protected-degrade payload keys are additive native trace**
   (`durability_classification`, `durability_reason`, `destination_ref`); the
   reference's only signal there is the stderr warning. Likewise
   `destination_ref` for a status-only `move-task` commit is additive. These are
   not byte-for-byte envelope parity claims.
10. **No aggregate/release/performance run** and no whole-workspace clippy; the
    scoped commands above are the evidence. `sk-cli --all-targets` still carries
    the foreign `init_oracle_diff.rs` lint outside this ownership.
11. **This is implementation, not independent acceptance.** The reviewer must
    independently reproduce F1–F6 and the retained matrix before the tranche's
    verdict can change.

## 8. Non-negotiables observed

No guessed/faked commit; durability still requires landed commit + exact byte
readback; no fabricated verdict; no new generic refusal for valid reference
input (the two removed refusals are exactly F2/F4); `move-task`'s protected
refusal preserved; no deletion on refused/unknown/side-committed state; the
Mission lock and `StatusHeadCommittedProbe` remain the authorities; no Python
production path; no edits outside the allowed files.
