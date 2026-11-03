# Native governed `next`: continuation report

Implementation unit: native governed-`next` functional facade.
Repository: `spec-kitty-rust` at `ff4452b02` (workspace crates under `crates/`).
Upstream reference: `spec-kitty/spec-kitty` `9aed3a8012cb5eb22c0128b14d525039f95fea7a`
(`spec-kitty` 4.0.0rc8), driven through `.venv-rust-oracle/bin/python` with
`PYTHONPATH=src` and `SPEC_KITTY_PACKS_ROOT=$PWD/packs`, verified by
`scripts/rust/verify_oracle.py` (`ok: true`, 53 runtime requirements, no drift).

Owned surface in this unit: `crates/sk-runtime/**`, `crates/sk-run-store/**`,
`crates/sk-cli/src/compat/runtime.rs`, and the new CLI runtime test file
`crates/sk-cli/tests/runtime_next.rs`. `crates/sk-cli/src/compat/mod.rs` and
`crates/sk-cli/tests/compat.rs` were **not** edited (coordinator-owned); the one
required change to the latter is supplied verbatim in
[Coordinator actions](#coordinator-actions).

---

## 1. What this unit actually implements

`spec-kitty next` is not a planner projection. The facade now composes four
authorities in the upstream order and never reports a success it did not
perform.

| Layer | Where | Behaviour |
| --- | --- | --- |
| Pure decision envelope | `sk-runtime/src/compose.rs` | `CliDecisionKind` (`query`/`step`/`decision_required`/`blocked`/`terminal`), the exact upstream `Decision.to_dict()` field set and JSON value shapes (`mission_number: ""`, `options: null`, `guard_failure_paths: {}`, `origin: {}`), the `kind="step"` prompt invariant (`materialize` degrades an unresolvable prompt to `blocked`), the finalized-board derivation, `preview_claimable_wp` with the upstream selection-reason vocabulary (`no_wp_files`, `dependencies_not_satisfied`, `all_wps_in_progress`, `not_finalized`, `no_planned_wps`), lane-weighted progress with Python's ties-to-even rounding, and the state→action alias table. No filesystem access. |
| Pure analysis currency | `sk-runtime/src/analysis.rs` | `check_analysis_report` reproduces `check_analysis_report_current`'s decision for the documented core: missing report, findings-carrier/wrong-artifact-type/invalid-frontmatter staleness, per-input SHA-256 comparison (`spec.md`, `plan.md`, `tasks.md`, `charter`), `tasks.md` status-churn normalization (list markers and pipe-table status cells), and the typed `ANALYSIS_REPORT_MISSING`/`ANALYSIS_REPORT_STALE`/`ANALYSIS_CURRENCY_UNAVAILABLE` codes. `report_transaction` and `material_manifest_version` carriers fail closed as `Unavailable` (never "current"). |
| Durable store | `sk-run-store/**` | Baseline-parity fixes below, plus a public `RunStore::write_derived_prompt` so the CLI can author a composed prompt artifact inside the run directory without inventing a second prompt namespace. |
| Command facade | `sk-cli/src/compat/runtime.rs` | Query mode (no writes, no run creation), advancing mode (answer → board first-contact → guard/dependency gate → durable advance → decision mapping), prompt authoring with governance context, and the native `runtime` routes. |

### Query mode (read-only)

Order follows upstream `query_current_state`: canonical task-surface error →
finalized-board override → untouched-run/first-step preview → planner decision.
With no run it plans from the live template and an empty in-memory cursor. It
writes **nothing** (upstream bootstraps an ephemeral run store and deletes it;
the native facade never needs one), reports `run_id: null`, and keeps the
structural `kind: query` / `is_query: true` invariant — a board `blocked:*`
verdict is reported as `mission_state: "blocked"` with a reason, not as
`kind: blocked`.

### Advancing mode

1. **Argument preflight** before any mutation: `--answer` requires `--result`
   ("`--answer requires --result because query mode is read-only`"), `--result`
   requires `--agent`, and `--result` is one of `success|failed|blocked`.
2. **Answer** the pending decision as the **human** operator (`ActorType::Human`)
   from `--agent`, with the shared 0/1/many decision-id resolution
   (`DECISION_SELECTION_REQUIRED` naming the pending ids).
3. **Finalized-board first contact** on an untouched run (`success` only):
   `implement` dispatch requires a dependency-ready `planned` work package and
   `review` a `for_review` one; the analysis gate runs before an `implement`
   dispatch. Only a dispatch or a `blocked:*` sentinel pre-empts the DAG —
   accept/done and the board's other blocked floors fall through, exactly as
   upstream `_dn_finalized_board_override` does.
4. **Dependency/guard gate** for the issued step: `implement`/`review` hold the
   run while the board still has work for that step; software-dev non-WP steps
   are re-issued with `guard_failures`/`guard_failure_paths` instead of
   advancing.
5. **Durable advance** under the cursor lock, then map the *advance outcome's*
   decision (not a second query — the planner skips the step it just issued).

Guards implemented natively: `specify` (spec.md), `plan` (plan.md), `tasks`
(tasks.md, `tasks/` directory, ≥1 `WP*.md`, `dependencies` frontmatter present),
`analyze` (analysis-report currency), `implement`/`review` (WP advance
readiness), plus the canonical task-read error for a status work package whose
task file disappeared. Guard-diagnostic paths are repository-relative, matching
the reference.

Prompt authoring writes a real, contained artifact at
`<run_dir>/prompts/<step>[--WPxx].prompt.md` containing the mission/run identity,
the resolved lane worktree, the target branch, the action's governance context
from `sk-doctrine` (`build_action_context` over the merged DRG + activation
ledger), the step prompt, the authored work-package body, and the exact advance
command. A missing/!malformed pack is reported **in the prompt text** instead of
being swallowed.

### Store fidelity changes that this facade depends on

| Change | Reason |
| --- | --- |
| `start` writes `mission_id: null` / `mission_slug: null` into the cursor and the `MissionRunStarted` payload; identity lives in the index entry and `inputs.mission_slug` | The pinned engine never writes cursor identity. This makes a start-state byte differential pass and keeps the tolerant reader (`snapshot.mission_slug or inputs["mission_slug"]`) authoritative. |
| Cursor datetimes use the Pydantic `Z` shape (`2026-01-01T00:00:00Z`, six digits when fractional); journal lines keep `+00:00` | One upstream answer transaction writes both formats (`DecisionAnswer.answered_at` vs the event timestamp). |
| `KernelError::TooLarge` maps to `RunStoreError::TooLarge` (`TOO_LARGE`) | The declared code was unreachable; a bounded-read oversize refusal now surfaces typed. |
| Torn-tail repair acquires the journal's own event-log lock and re-reads under it | The kernel contract says repair happens under the log lock; append already did. |
| `RunStore::write_derived_prompt` (new, additive) | The CLI needs one contained place for a composed prompt; recovery never reads it. |
| README/doc corrections | "Nothing is written on a refusal" was inexact (reconciliation repairs first); the unknown-field preservation claim was already corrected by the parent; gap #14 is now fixed and re-documented, with the remaining prompt-path difference recorded. |

Not done deliberately: `sk-mission` was left untouched. The board reads it needs
(lane reduction, work-package frontmatter, dependency readiness, lane manifest)
already exist as public `sk_status` / `sk_mission` / `sk-workspace` APIs, so a
second "legacy runtime view" would have been a duplicate authority.

---

## 2. Differential evidence (real oracle, not structural expectations)

All runs used temporary project roots with their own `HOME`
(`/tmp/cmp*`, `/tmp/io-*`, `/tmp/g*`, `/tmp/n*`) and the verified oracle. The
committed reproduction instructions are in
`crates/sk-run-store/tests/oracle/generate_next_fixtures.py`.

### 2.1 No-run query preview — byte-equal decision

Same project (custom `demo-io` mission type with `requires_inputs`, no
`.kittify/runtime`), one copy per implementation:

```
oracle: next --json  → exit 0, kind=query, mission_state=not_started,
                       preview_step=specify, run_id=null, progress=null,
                       options=null, origin={}, guard_failure_paths={}
native: next --json  → identical JSON except `timestamp`
neither side wrote .kittify/runtime
```

### 2.2 Advancing mode — byte-equal decision and cursor

Same run cursor on both sides (oracle-produced run, `completed=[discovery,
specify]`, `issued=plan`, `plan.md` present):

```
oracle: next --agent claude --result success --json → step "tasks"
native: next --agent claude --result success --json → step "tasks"
decision diff: prompt_file only (see 2.5); origin.mission_path differs by root
state.json  diff: none — full parsed equality, including the RACI records
journal     diff: none (payloads; only timestamps/run ids differ)
```

### 2.3 Guard re-issue — byte-equal decision

Same project with `plan.md` removed; `next --agent claude --result success`:

```
both: kind=step, step_id=plan, mission_state=plan, action=plan,
      guard_failures=["Required artifact missing: plan.md"],
      guard_failure_paths={"plan.md": "kitty-specs/<slug>/plan.md"}
diff: prompt_file only
```

### 2.4 Human answer — byte-equal envelope and cursor

`requires_inputs` mission: `next --result success` then
`next --agent claude --answer alice --result success`:

```
both: decision_required (input:owner, question, input_key, options=null)
      then step specify with answered="input:owner", answer="alice"
decision diff: `warnings` only (see 2.6)
cursor diff: answered_at timestamp, run_id, template_path
answered record: actor_type=human, authority_role=human, answer=alice (both)
```

### 2.5 Finalized board and the analysis-currency gate

A `demo-board` mission (runtime template without an `analyze` step) with WP01
`claimed` and WP02 unseeded:

```
oracle: next --json → mission_state=implement, preview_step=implement,
                      wp_id=null, reason="all_wps_in_progress",
                      progress={total_wps:2, in_progress_wps:1,
                                weighted_percentage:5.0}
native: identical
```

(The weighted percentage comes from the reduced status snapshot — one claimed
work package — not from every task file; the bucket counts still come from the
task files.)

The same board with a **claimable planned** WP and no recorded analysis report
exercises upstream's B3 gate (a `software-dev` template declares `analyze`):

```
query:    both → kind=query, mission_state=analyze, preview_step=analyze,
                 wp_id=null, error_code=ANALYSIS_REPORT_MISSING,
                 guard_failures=["Analysis report missing: …"]
advance:  both → kind=step, step_id=analyze, action=analyze, reason=null,
                 wp_id=null, workspace_path=null,
                 error_code=ANALYSIS_REPORT_MISSING,
                 guard_failures=["Analysis report missing: …"]
          the native cursor is untouched (issued_step_id stays null)
```

A four-work-package board (1 `for_review`, 1 `in_progress`, 2 unseeded) also
matches exactly, including `weighted_percentage: 45.0` (snapshot-weighted over the
two seeded packages) while `total_wps` stays 4.

The advancing board override is intentionally narrower than the query preview,
matching upstream `_dn_finalized_board_override`: it dispatches only a
*dependency-ready planned* `implement` or a `for_review` `review`, and pre-empts
the DAG only for a `blocked:*` sentinel. Every other board answer (including an
in-progress-only board) falls through to the DAG, which is what the reference
does — verified: an in-progress-only board yields `discovery` on both sides.

### 2.6 Committed run-store differential fixture

`crates/sk-run-store/tests/fixtures/next_scenario.json` is the oracle's own
transcript of the scenario above (two invocations, decision, full cursor, full
journal), normalized on `run_id`, `template_path`, `timestamp`, `answered_at`,
`requested_at`, and carrying its provenance (baseline SHA, rc version, Python
3.11.15). `tests/oracle_differential.rs` replays the same two transactions
through `RunStore` and compares the cursor and journal JSON for equality.

Result: **passes** — the native store reproduces the reference cursor and journal
byte-for-byte (parsed) at both points, including the null cursor identity, the
`input:owner` request payload, the RACI record, the `Z`/`+00:00` timestamp split
and the `DecisionInputAnswered`/`NextStepIssued` order.

### 2.7 Known, documented wire differences

| Difference | Status |
| --- | --- |
| `prompt_file` path: reference `<home>/.spec-kitty/spec-kitty-prompts/<hash>/…`; native `<run_dir>/prompts/…` | Deliberate. The native artifact is contained in the run, durable with it, and carries the WP body + governance context. Content/identity preserved; path differs. |
| `warnings` key | Reference only, emitted when its decision-log Git commit fails. Native has no decision-log commit (gap G3). |
| JSON key order | `serde_json` serializes maps sorted; the reference prints insertion order. Parsed values match. |
| `origin.mission_path` | Absolute path, so it differs across checkouts by construction. |

---

## 3. Verification

Scoped, with `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0` and the shared existing
`target/` (peers were compiling concurrently, so wall times are not meaningful):

| Check | Result |
| --- | --- |
| `cargo test -p sk-run-store --locked` | 54 passed (unit 1, advance_answer 22, locking 4, no_stdout 2, oracle_differential 2, recovery 10, review_regressions 2, start_query 11), 0 failed |
| `cargo test -p sk-runtime --locked` | 170 passed across the crate's suites + doctests, 0 failed |
| `cargo test -p sk-cli --test runtime_next --locked` | 11 passed, 0 failed |
| `cargo test -p sk-cli --test compat --locked` | 22 passed, 2 failed — both expected, see below |
| `cargo clippy -p sk-runtime -p sk-run-store --all-targets --locked -- -D warnings` | clean |
| `cargo clippy -p sk-cli --all-targets --locked --no-deps -- -D warnings` | clean |
| `cargo test -p sk-run-store -p sk-runtime --locked` (combined) | 26 suites, all `ok` (224 tests + doctests) |
| `cargo clippy -p sk-cli --all-targets --locked` (with deps) | **blocked** by peer in-flight lints in `sk-status/src/mutation_rollback.rs` and `sk-install/src/init.rs`; not this unit's code |
| `rustfmt --check` on every file this unit touched | clean |
| `scripts/rust/verify_oracle.py` | `ok: true`, 53 requirements, no drift |

Expected failures in `tests/compat.rs`:

1. `no_run_queries_and_invalid_advances_do_not_bootstrap_runtime_files` —
   asserts `next --mission <slug> --json` exits 1. That pinned the parent's
   temporary "refuse instead of mutate" fix. This unit implements the actual
   upstream behaviour: exit 0, one JSON preview document, `run_id: null`, and
   **no writes** (verified: the tree snapshot is unchanged). The other three
   cases in that test (`runtime query` without a run, `--result` without
   `--agent`, `--answer` without `--result`) still exit 1 and write nothing.
   The required test edit is in [Coordinator actions](#coordinator-actions).
2. `init_with_a_slash_command_agent_renders_canonical_prompts_from_the_pack` —
   fails because `init --ai claude` does not generate
   `.claude/commands/spec-kitty.specify.md`. That is `sk-install`/`init`
   work-in-flight by another owner; this unit did not touch `init`, `sk-install`
   or the command renderer.

---

## Coordinator actions

### A1. Update one assertion in `crates/sk-cli/tests/compat.rs` (coordinator-owned)

Replace the loop at `crates/sk-cli/tests/compat.rs:808-820` with:

```rust
    // A bare `next` is a read-only preview: exit 0, one JSON document, no writes.
    let query = run(root, &home, &["next", "--mission", &slug, "--json"]);
    assert_eq!(query.status.code(), Some(0));
    let value = single_json(&query);
    assert_eq!(value["kind"], json!("query"));
    assert_eq!(value["is_query"], json!(true));
    assert_eq!(value["run_id"], Value::Null);
    assert_eq!(tree_snapshot(root), before);

    for args in [
        vec!["runtime", "query", "--mission", &slug, "--json"],
        vec!["next", "--result", "success", "--mission", &slug, "--json"],
        vec!["next", "--answer", "ignored", "--mission", &slug, "--json"],
    ] {
        let output = run(root, &home, &args);
        assert_eq!(output.status.code(), Some(1), "args: {args:?}");
        let _ = single_json(&output);
        assert_eq!(tree_snapshot(root), before, "args: {args:?}");
    }
```

Rationale and the full oracle comparison are in §2.1; the new
`crates/sk-cli/tests/runtime_next.rs::a_bare_next_previews_the_first_step_without_writing_anything`
pins the same contract from this unit's own suite.

### A2. Capability matrix

`crates/sk-cli/src/compat/capabilities.rs` was updated surgically (only the
`next` entry's `evidence`/`gaps`) so the published matrix no longer lists gaps
that are now implemented. Please review in the merge — nothing else in that file
was touched.

### A3. Publication / evidence docs

`docs/rust/VERIFICATION.md` and the inventory still describe `next` as
"not certified". The claims above are scoped to this facade: adopt whichever of
them the coordinator's evidence gate accepts, and keep the gaps in §4 open
regardless.

---

## 4. Unfinished obligations (ordered by blocking severity)

Nothing below is claimed as done, and each one is fail-closed or explicitly
reported rather than silently skipped.

1. **StepContract composition dispatch (G1).** Upstream routes `software-dev`
   step completion through `StepContractExecutor.execute` before advancing
   (invocation records, step-contract artifact authoring/validation, the
   `runtime_bridge_composition` guard surface). The facade advances the DAG
   directly for the non-WP steps. Consequence: invocation lifecycle records are
   absent, and step-contract-specific artifact validation does not run.
2. **Requirement-mapping and occurrence-map gates (G2).** `_check_requirement_
   mapping_ready` needs the injectable requirement grammar
   (`specify_cli.requirement_mapping.grammar`, 478 lines) and
   `ensure_occurrence_classification_ready`; reproducing either by guessing
   would invent governance. The `tasks` step therefore **always** adds the named
   guard failure `REQUIREMENT_GATE_UNAVAILABLE` and never advances past it. In
   practice a finalized board takes over via the board override, so the WP loop
   still works; a mission that never finalizes a board cannot walk the DAG past
   `tasks` natively where upstream could.
3. **Decision-log Git commits (G3).** Upstream wraps the event emitter in
   `DecisionGitLog` (coordination-branch commit of `decisions.events.jsonl`) and
   surfaces `warnings` when the commit fails. Native emits no such commit and
   therefore no `warnings` key.
4. **Lifecycle records and the retrospective completion gate (G4).**
   `next_invocation_lifecycle` (started/completed record pairing,
   `MissionNextInvoked`) and `retrospective_terminus` completion refusal are not
   wired. A terminal advance appends `MissionRunCompleted` and reports
   `kind: terminal` with `mission_state: done`; upstream can additionally refuse
   or defer that transition.
5. **Coordination topology and owned checkouts (G5).** `mission_context_for`,
   `placement_seam`, coordination worktrees, `--owned-checkout`, stale-copy
   reporting, and the coordination-surface blocked verdicts are absent. The
   facade reads the primary mission directory and the lane manifest; a
   coordination-topology mission is treated as a primary-surface mission, which
   is correct only while the coordination surface is a mirror. Also absent:
   the merged-mission terminal short-circuit (`committed_authority
   .mission_terminal_verdict`), so a merged mission is reported from its board
   instead of `done`.
6. **`analyze` currency carriers (G6).** `report_transaction` qualification and
   `material_manifest_version` inputs are refused as `Unavailable`
   (→ `ANALYSIS_CURRENCY_UNAVAILABLE`), never silently "current". Missions that
   use report transactions need that implemented before `analyze` can pass.
7. **Complete artifact-presence manifests (G7).** The guard's presence set is
   the fixed software-dev filenames. Org-pack / built-in
   `expected-artifacts.yaml` projection (`charter.activation.manifest_loader` +
   `project_artifact_name_set`) and the `research`/`documentation`/`plan`
   family guard tables are not implemented; those families get `[]` failures
   instead of their real guards. This must not be mistaken for parity.
8. **Prompt-path and prompt-content parity (G8).** See §2.6. If a consumer needs
   the reference's shared prompt namespace, that is a separate decision (it
   leaks a home-directory path into a machine contract).
9. **Board-detail parity (G9).** `wp_task_surface_error` reproduces the
   reference message and recovery text; the per-WP workspace resolution reads
   `lanes.json` and falls back to the repository root when no lane is recorded
   (upstream additionally consults persisted workspace context records, raises
   `MissingLanesError` through `resolve_workspace_for_wp` in some fixtures, and
   enforces checkout identity on writes — a `next` decision is a read, so the
   fallback is only reachable for an unallocated lane). The board selector also
   does not reproduce the `_wp_task_surface_error` → `blocked_reason` path for
   the *query* preview beyond the canonical message (the advance path matches).
10. **Differential coverage breadth (G10).** Committed oracle evidence exists
    for the no-run preview, the plain advance, the guard re-issue, the human
    answer (cursor + journal + envelope) and the board preview. Still
    uncommitted/fixture-less: `audit:*` decisions (the built-in templates carry
    no audit steps, so they need a synthesized template), blocked board
    advancing verdicts, terminal completion, and a full
    `specify → plan → tasks → analyze → implement → review → accept` walk that
    crosses G1/G2.
11. **Terminal/retrospective and audit decisions (G13).** A terminal advance is
    reported from the planner alone; the retrospective completion gate and audit
    (`audit:*`) decisions need a template that declares audit steps, which no
    built-in runtime template does. Both are unexercised in committed fixtures.
12. **Store performance (G11).** Every mutating transaction replays the journal
    to prove consistency, so mutation cost is O(events). No measurement against
    the reference was taken in this unit, and none is claimed.
13. **Cross-platform (G12).** All evidence here is Linux. Windows lock
    semantics, path length and file-mode behaviour are unverified for the new
    prompt-artifact writer.

## 5. Files changed by this unit

```
crates/sk-runtime/src/analysis.rs            (new)  pure analysis-report currency
crates/sk-runtime/src/compose.rs             (new)  pure CLI decision envelope/board/progress
crates/sk-runtime/src/lib.rs                 (+exports)
crates/sk-run-store/src/model.rs             (+state_now_iso / state_iso)
crates/sk-run-store/src/store.rs             (null cursor identity, index identity from the
                                              request, write_derived_prompt)
crates/sk-run-store/src/advance.rs           (cursor request timestamp shape)
crates/sk-run-store/src/answer.rs            (cursor vs journal timestamp split)
crates/sk-run-store/src/events.rs            (torn-tail repair under the event-log lock)
crates/sk-run-store/src/error.rs             (TooLarge translation)
crates/sk-run-store/src/lib.rs               (scope-boundary doc)
crates/sk-run-store/README.md                (lock/refusal wording, parity rows, gap list)
crates/sk-run-store/tests/oracle_differential.rs        (new)
crates/sk-run-store/tests/oracle/generate_next_fixtures.py (new, quarantined dev tool)
crates/sk-run-store/tests/fixtures/next_scenario.json   (new, oracle transcript)
crates/sk-run-store/NEXT-CONTINUATION-REPORT.md          (this file)
crates/sk-cli/src/compat/runtime.rs          (facade rewrite)
crates/sk-cli/tests/runtime_next.rs          (new, 11 end-to-end tests)
crates/sk-cli/src/compat/capabilities.rs     (next entry evidence/gaps only — see A2)
```
