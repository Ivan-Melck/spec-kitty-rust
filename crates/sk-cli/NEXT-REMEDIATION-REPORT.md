# `next` remediation — final report

Status: **complete for the owned slice**, with named residual gaps. This file was
written provisionally first and is now updated with measured results.

Repository: `spec-kitty-rust`, branch `rust/rewrite`, working tree at
`d64468a76` plus the delivery's dirty edits. Reference:
`spec-kitty/spec-kitty` `9aed3a8012cb5eb22c0128b14d525039f95fea7a`, driven
through `.venv-rust-oracle/bin/python` with absolute `PYTHONPATH=$PWD/src`,
`SPEC_KITTY_PACKS_ROOT=$PWD/packs` (the documented parent spelling).

Owned surface: `crates/sk-runtime/**`, `crates/sk-run-store/**`,
`crates/sk-cli/src/compat/runtime.rs`,
`crates/sk-cli/tests/runtime_next.rs` and additive runtime tests.
Excluded and untouched: shared support/dispatcher/matrix, `compat/mod.rs`,
`tests/compat.rs`, workspace manifests, `sk-doctrine`, `sk-install`, `sk-git`.

## 1. Starting state and disposition of the five failing CLI tests

The cancelled owner left no remediation report. Coordinator scoped reruns at
start: `sk-runtime` 181 pass, `sk-run-store` 54 pass,
`crates/sk-cli/tests/runtime_next.rs` 17 pass / 5 fail
(`/tmp/sk-cli-recovery-scoped.log`).

The five failures were resolved **without weakening any assertion**. Oracle
probes (temporary `HOME`/`XDG_*`/`TMPDIR`; exact absolute oracle env) showed
that four tests encoded a cursor/fixture state the reference itself rejects, so
the fixtures were made reproduce the reference path; the fifth asserted the
wrong step's prompt after a second advance.

| Failing test | Real cause | Resolution |
| --- | --- | --- |
| `a_wp_iteration_block_keeps_the_issued_step_id_and_the_actionable_reason` | The cursor was marked only `issued_step_id=implement` with no completed steps, so the reference's `_run_is_untouched` first-contact board override (correctly) emitted `mission_state="blocked"` with `step_id=null`. | Fixture now models the R1 reproduction: a run that walked the DAG (`completed_steps` non-empty) and is inside the WP-iteration gate. Oracle and native both then emit `kind=blocked`, `mission_state=step_id="implement"`, `reason="... review in progress ..."`. |
| `a_missing_canonical_status_log_fails_loud_with_the_bootstrap_remedy` | Same untouched-cursor setup. | Same fixture precondition. Oracle and native both emit `mission_state=step_id="implement"`, `reason` containing `Canonical status not found` + `finalize-tasks`, `guard_failures=[reason]`. |
| `the_tasks_guard_advances_a_mapped_board_to_analyze` | `tasks/` + `tasks.md` existed before the run reached `tasks`, so the untouched-run board override pre-empted the DAG. Oracle proves that state yields `blocked:no_actionable_wp`, not `discovery`. | Board is authored at the `tasks` boundary, as the real walk does. Oracle and native then issue `analyze` with `guard_failures=[]` for a mapped board. |
| `the_tasks_guard_emits_the_actionable_requirement_mapping_failure` | Same board-timing cause. | Board authored at the `tasks` boundary. Oracle and native both re-issue `tasks` (exit 0) with the exact actionable message `Requirement mapping incomplete before finalize-tasks: missing refs for WPs: WP01; unmapped FRs: FR-001. Run \`spec-kitty agent tasks map-requirements --batch ... --mission <slug> --json\` ...`. |
| `both_documented_packs_root_spellings_resolve_the_same_templates` | The second advance issued `specify`, not `discovery`; the test compared a `specify` prompt with a `discovery` assertion. | Each spelling now runs on its own fresh fixture; the two `discovery` bodies are compared after normalising the project root, so both spellings must resolve identical canonical prompts. |

Result: `cargo test -p sk-cli --test runtime_next --locked` → **25 passed,
0 failed** (22 pre-existing + 3 new doctrine/language regressions below).

## 2. Doctrine residual 2 (CLI composition defects) — fixed

Source: `crates/sk-doctrine/REMEDIATION-REPORT.md` residual 2 and F2 note.
All changes are in `crates/sk-cli/src/compat/runtime.rs`; no doctrine edits.

1. **Canonical org required floor.** The CLI passed an empty
   `BTreeMap` to `ContextInputs.required_floor`, so the library-proven
   floor/deny netting was unenforceable on the prompt surface. It now calls the
   canonical `sk_doctrine::required_floor(repo_root, &identity)?.into_map()`.
2. **Active-language profile filtering.** `ProfileRepository::load` (no filter)
   is replaced with `load_with_languages(..., active_languages.as_deref())`.
   Active languages are resolved with the reference's tier order:
   `.kittify/config.yaml` `languages` (catalog-validated, deduped) →
   `.kittify/charter/charter.yaml` `catalog.languages` (present-but-empty is an
   explicit empty answer) → `.kittify/charter/interview/answers.yaml`
   (`languages_frameworks` real declaration first, then all answers; the
   shipped default answer and placeholder/sentinel answers declare nothing).
   The declared-token table mirrors
   `charter/offering/shared/language_catalog.py` (10 languages + `jvm` family
   and the Gradle/Maven/JUnit family rule).
3. **Skipped profiles surfaced.** `ProfileRepository::skipped()` is now rendered
   in the governance `Diagnostics` line as
   `Agent profile source is invalid: <id> (<summary>)`. The repository-recorded
   language drop is surfaced too as
   `Agent profile <id> was excluded by the active language scope.`, so a future
   skip cannot remain unannounced on this surface.

### Focused real-CLI regression

`an_org_required_tactic_survives_a_deny_and_the_active_language_scope` in
`crates/sk-cli/tests/runtime_next.rs` drives the actual `spec-kitty next` binary
against a real project with:

- an org pack whose `org-charter.yaml` has
  `required_tactics: [aggregate-boundary-design]`;
- `.kittify/charter/activation.yaml` with
  `deactivated_tactics: [aggregate-boundary-design]` (a deny);
- `.kittify/config.yaml` `languages: [python]` (active-language scope);
- an org profile `rust-specialist` scoped to `rust` (must be excluded) and an
  invalid `broken-profile` (must be surfaced).

Assertions: the issued `implement` prompt carries `Tactic IDs:` and
`aggregate-boundary-design` (the floor nets the deny while the language scope is
active); the prompt diagnostics carry the `rust-specialist` exclusion and the
`broken-profile` skip. The test passes.

Two further real-CLI regressions pin the language-source tiers with the same
org-scoped `rust-specialist` profile: `a_compiled_charter_language_scope_filters_a_language_specific_profile`
(`.kittify/charter/charter.yaml` `catalog.languages: [python]`) and
`an_interview_language_declaration_filters_a_language_specific_profile`
(`.kittify/charter/interview/answers.yaml` `languages_frameworks: Python and pytest`).
Both assert the surfaced exclusion diagnostic. The configured tier is covered by
the org-floor test (`languages: [python]`). All three tiers pass.

## 3. Verification (scoped, temp HOME/XDG/TMP, shared Cargo lease)

All Cargo work used `/tmp/sk-focused-cargo.sh` (read first; `/tmp` sandbox,
`CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, existing `target/`). No aggregates,
releases, manifests, cargo-clean, nohup, sleep/poll loops, subagents, commits or
pushes.

- `cargo test -p sk-cli --test runtime_next --locked` → **25 passed, 0 failed**.
- `cargo test -p sk-runtime -p sk-run-store --locked` → **28 green suite
  results**; per-crate counts reproduce the coordinator's **181** (sk-runtime,
  including doctests) and **54** (sk-run-store, including doctests).
- `cargo clippy -p sk-runtime -p sk-run-store --all-targets --locked -- -D
  warnings` → clean.
- `cargo clippy -p sk-cli --all-targets --no-deps --locked -- -D warnings` →
  `compat/runtime.rs` is clean; the remaining 6 lints are in excluded, other-owner
  files (`compat/init.rs:270`, `compat/status.rs:831,854`, `compat/mod.rs:48,274,287`).
  A package-wide `clippy -p sk-cli` (without `--no-deps`) is additionally blocked
  by concurrent `sk-git` owner edits (`result_large_err`), not by this slice.
- `rustfmt --edition 2024 --check` on every owned `.rs` file → clean.

## 4. Per-review disposition (R1–R8, G2, G3)

- **R1 (HIGH) WP-iteration sentinel / status-log guard.** Source already maps
  the internal `blocked:*` sentinel back to the issued `step`, and already has
  the fail-loud `canonical_status_missing_error`. Oracle re-verified on the
  walked cursor; both sides agree. Failing fixtures corrected (§1). **Resolved.**
- **R2 (HIGH) prompt-resolution fail-closed.** Activated mission type with no
  profile → `kind=blocked`, `prompt_file=null`, `UnknownMissionTypeError`
  reason. **Resolved** (test passes).
- **R3 (HIGH) real prompt content.** `discovery` prompt is byte-identical to the
  oracle except one advisory governance diagnostic; `tasks` carries the pack
  `prompt_template` body in full; WP review prompts carry the `move-task`
  approve/reject guidance and no bogus `--result success`. **Resolved for
  content**; residual: rich-vs-compact governance rendering per action (§5) and
  the prompt path (run-dir artifact vs HOME hashed dir).
- **R4 (MEDIUM) review preview `wp_id`.** Query fills `wp_id` only for
  `implement`; review/accept leave it null. **Resolved.**
- **R5 (MEDIUM) board override needs `tasks.md`.** `BoardView::can_override`
  requires `tasks.md` and a task-error-free board; test covers the absent case.
  **Resolved.**
- **R6 (MEDIUM) `origin` authority/tier.** `origin` is resolved from
  `mission.yaml` tiers (override → legacy → org → global → package) and
  canonicalised; test covers the legacy tier. **Resolved.**
- **R7 (MEDIUM) one packs-root convention.** `packs_root` normalises a
  `built-in` leaf to its parent and shares it with the doctrine context;
  templates resolve identically under both spellings (test). The org required
  floor uses doctrine's canonical parent spelling (see §5 residual).
  **Resolved for the adapter's reads.**
- **R8 (LOW) torn journal / envelope / exit.** Query emits a `query` decision
  with exit 0; advance emits `kind=blocked` with reason prefixed
  `Runtime engine error: ` and exit 1; nothing is appended. **Resolved.**
- **G2 (requirement/occurrence gate).** Implemented in `sk-runtime`
  (`requirements.rs`, `occurrence.rs`) and wired into the `tasks` guard.
  Oracle-backed tests cover mapped → `analyze` and unmapped → actionable
  re-issue. **Resolved.**
- **G3 (decision-log commit `warnings`).** **Residual, not implemented.** No
  native `kitty-specs/<slug>/decisions.events.jsonl` writer/committer exists in
  this slice; the reference's `warnings` comes from
  `DecisionGitLog`/`safe_commit` on the coordination/decision-log seam.
  Inventing a second decision-log protocol would violate the migration-safety
  contract, so it is left to the decision-log/coordination writer tranche
  instead of being faked.

## 5. Residual gaps (honest)

1. **G3 decision-log commit/warnings** — see §4.
2. **`StepContractExecutor` composition dispatch** (invocation records,
   step-contract artifact authoring, retrospective completion gate,
   coordination-topology workspaces) remains absent; `capabilities` records it.
3. **Prompt governance rendering** — the legacy-DAG rich block is reproduced
   (directive/tactic IDs, template set, tools, diagnostics) but the reference's
   compact render for `specify/plan/tasks/implement/review` in a charter-less
   project is always rendered rich here. Content is canonical; byte-shape is not.
   Prompt artifact location remains inside the run dir (`prompts/<step>.md`)
   rather than the reference's HOME hashed namespace.
4. **Org-floor + leaf packs-root spelling** — the canonical
   `sk_doctrine::required_floor` search reads the documented parent
   `SPEC_KITTY_PACKS_ROOT=<repo>/packs`; the adapter's leaf normalisation covers
   templates/governance but not doctrine's internal org search. Tests and the
   oracle use the parent.
5. **Active-language inference** carries a local read-only mirror of the shipped
   language catalog and the three-tier resolution; moving it into a shared
   charter-language module would remove the second table.
6. **Non-git project governance metadata** (doctrine report residual 1) is
   separately assigned; not touched here.

## 6. Files changed by this remediation

- `crates/sk-cli/src/compat/runtime.rs` — canonical required floor,
  active-language profile loading, skipped/scope-filtered diagnostics,
  stale module docs, two clippy fixes.
- `crates/sk-cli/tests/runtime_next.rs` — source-backed fixture corrections for
  the five failures; new org-floor/active-language CLI regressions (25 tests).
- `crates/sk-cli/NEXT-REMEDIATION-REPORT.md` — this report.

No `sk-doctrine`, `sk-install`, `sk-git`, manifest, `compat/mod.rs`,
`tests/compat.rs` or shared-support edits; no commits or pushes.
