# Native upgrade migrations — continuation report

**Status: 46 of 136 declared migrations execute natively; 90 remain unsupported and the full rewrite stays blocked.**

Checkpoint: on top of `ff4452b02`, against pinned Python behaviour
`9aed3a8012cb5eb22c0128b14d525039f95fea7a` (the coordinator has since reported fork
main at `8ffeb0180`; the oracle, catalog, fixtures and this report stay on the
pinned baseline until the coordinator reconciles upstream). This report covers
only the migration tranche in `sk-install`; it is not a full-compatibility claim.

Previous tranche: 16 native ids. This tranche adds **30**, and hardens the runner
contracts those migrations depend on.

## What changed

### New migration modules (owned surface)

| module | ids | effect |
| --- | --- | --- |
| `src/migrations/ignore.rs` | — | the shared `.gitignore` effect seam: symlink-refusing read (strict / `errors="ignore"`), `newline=""` vs universal-newline modes, `GitignoreManager.ensure_entries` (marker, one blank line, CRLF preservation, `force_append`), atomic write that keeps an existing mode and gives a new file the umask-respecting mode, `splitlines`/`splitlines(keepends=True)` equivalents, blank-run collapse, and the Git-backed `root_effectively_ignored` + `is_tracked`/`untrack` probes |
| `src/migrations/gitignore_family.rs` | 10 | the frozen ignore-entry backfills and the two stale-rule removals |
| `src/migrations/retired.rs` | 18 | the ids the pinned registry itself registers as recorded no-ops (16 supersession substitutes + 2 inert modules) |
| `src/migrations/legacy_layout.rs` | 2 | the `.specify/`→`.kittify/` rebrand rename and the retired `.kittify/active-mission` file |

### Runner changes in `src/upgrade.rs` (also owned)

1. **`Failed` plan outcome.** A `detect`/`can_apply` that cannot evaluate the
   project (unsafe ignore file, tracked paths under a managed root) is no longer
   an abort of the whole plan pass and no longer a silent skip: it is a plan row
   that the real run records as `failed`, stops on, and leaves the project
   version untouched for — matching the pinned runner. The typed error is
   re-derived under the held lock and returned to the caller.
2. **Supersession stamping.** A recorded row now carries
   `spec.superseded_by` when the catalog declares one, so a superseded
   migration's `skipped` is distinguishable from the shipped migration's own
   `skipped` (ADR 2026-10-17-1, Amendment 2026-10-25). `LedgerRecord::for_spec`
   still invents nothing; the stamp is the runner's explicit act.
3. **Record de-duplication.** A record is appended only when the partition's held
   result differs, and a held `success` is never downgraded to `skipped`
   (upstream `_record_ledger_result`). Without this, every superseded no-op —
   which a `skipped` record never settles — grew the ledger on every run.
4. **HRTB-free count assertions** and doc updates: `NATIVE_MIGRATIONS` is 46.

No production Python process, FFI interpreter, success stub or invented protocol
was added. No `Cargo.toml`/`Cargo.lock` change and no new dependency: the
implementations use `std`, `serde_json`, `sk_kernel` and the existing
`migrations::git` helper.

## Exact catalog ids now supported (46)

| id | module | pinned source | oracle cases |
| --- | --- | --- | --- |
| `0.2.0_specify_to_kittify` | `legacy_layout` | `m_0_2_0_specify_to_kittify.py` | 5 |
| `0.4.8_gitignore_agents` | `gitignore_family` | `m_0_4_8_gitignore_agents.py` | 5 |
| `0.8.0_remove_active_mission` | `legacy_layout` | `m_0_8_0_remove_active_mission.py` | 3 |
| `0.9.0_frontmatter_only_lanes` | `retired` | `supersession.py` substitute | 2 |
| `0.9.1_complete_migration` | `retired` | `supersession.py` substitute | 2 |
| `0.10.9_repair_templates` | `retired` | `m_0_10_9_repair_templates.py` (inert) | 2 |
| `0.10.12_charter_cleanup` | `retired` | `m_0_10_12_charter_cleanup.py` (stub) | 2 |
| `0.12.1_remove_kitty_specs_from_gitignore` | `gitignore_family` | `m_0_12_1_remove_kitty_specs_from_gitignore.py` | 5 |
| `0.13.1_exclude_worktrees` | `upgrade.rs` | `m_0_13_1_exclude_worktrees.py` | — (earlier tranche) |
| `2.0.0_charter_directory` | `retired` | `m_2_0_0_charter_directory.py` (stub) | 2 |
| `2.0.0_historical_status_migration` | `retired` | `m_2_0_0_historical_status_migration.py` (inert) | 2 |
| `2.0.0a5_remove_wp_status_gitignore_rule` | `gitignore_family` | `m_0_16_2_remove_wp_status_gitignore_rule.py` | 5 |
| `2.0.2_charter_context_bootstrap` | `retired` | `m_2_0_2_charter_context_bootstrap.py` (stub) | 2 |
| `2.0.6_consistency_sweep` | `retired` | `supersession.py` substitute | 2 |
| `2.0.9_state_gitignore` | `gitignore_family` | `m_2_0_9_state_gitignore.py` | 4 |
| `2.1.2_fix_charter_doctrine_skill` | `retired` | `m_2_1_2_fix_charter_doctrine_skill.py` (stub) | 2 |
| `2.1.2_fix_glossary_context_skill` | `retired` | `SupersededMigration` subclass | 2 |
| `2.1.4_enforce_command_file_state` | `retired` | `supersession.py` substitute | 2 |
| `3.0.2_restore_prompt_commands` | `retired` | `supersession.py` substitute | 2 |
| `3.1.1_event_log_merge_driver` | `merge_drivers` | earlier tranche | — |
| `3.1.1_normalize_status_json` | `retired` | `supersession.py` substitute | 2 |
| `3.2.0rc28_github_diff_attributes` | `upgrade.rs` | earlier tranche | — |
| `3.2.0rc35_default_charter_pack` | `retired` | `SupersededMigration` subclass | 2 |
| `3.2.0rc35_kittify_runtime_git_hygiene` | `gitignore_family` | `m_3_2_0rc35_sync_state_gitignore.py` | 4 |
| `3.2.3_encoding_provenance_gitignore_backfill` | `gitignore_family` | `m_3_2_3_encoding_provenance_gitignore_backfill.py` | 2 |
| `3.2.4_derived_mission_views_gitignore_backfill` | `gitignore_family` | `m_3_2_4_derived_views_gitignore_backfill.py` | 3 |
| `3.2.4_runtime_dirs_gitignore_backfill` | `gitignore_family` | `m_3_2_4_runtime_dirs_gitignore_backfill.py` | 3 |
| `3.2.5_agents_skills_gitignore_backfill` | `retired` | `supersession.py` substitute | 2 |
| `3.2.6_decisions_event_log_merge_driver` | `merge_drivers` | earlier tranche | — |
| `3.2.6_gate_artifact_merge_drivers` | `merge_drivers` | earlier tranche | — |
| `3.2.6_issue_matrix_driver_repoint` | `merge_drivers` | earlier tranche | — |
| `3.2.6_meta_traces_merge_drivers` | `merge_drivers` | earlier tranche | — |
| `3.2.6rc3_lint_report_gitignore_backfill` | `gitignore_family` | `m_3_2_6rc3_lint_report_gitignore_backfill.py` | 3 |
| `3.2.6rc3_narrow_cursor_gitignore` | `retired` | `supersession.py` substitute | 2 |
| `3.2.6rc3_worktrees_gitignore_backfill` | `gitignore_family` | `m_3_2_6rc3_worktrees_gitignore_backfill.py` | 5 |
| `3.2.7_review_cycle_merge_driver` | `merge_drivers` | earlier tranche | — |
| `4.0.0rc5_decision_index_merge_driver` | `merge_drivers` | earlier tranche | — |
| `4.0.0rc7_exclude_spec_kitty` | `upgrade.rs` | earlier tranche | — |
| `4.0.0rc7_pack_level_backfill` | `pack_level` | earlier tranche | — |
| `4.0.0rc7_track_charter_pack_yaml` | `charter_pack_gitignore` | earlier tranche | — |
| `4.0.0rc8_narrow_agent_gitignore` | `narrow_agent_gitignore` | earlier tranche | — |
| `4.0.0rc8_retire_stale_mission_template_forks` | `mission_template_forks` | earlier tranche | — |
| `4_0_0rc5_heal_run_index_paths` | `run_index` | earlier tranche | — |
| `4_0_0rc5_install_lane_tip_recorder` | `lane_tip` | earlier tranche | — |
| `normalize_activation_absence` | `retired` | `SupersededMigration` subclass | 2 |
| `unify_charter_activation_promote_answers` | `retired` | `supersession.py` substitute | 2 |

Marking the 18 no-op ids native is a port of **source behaviour, not a shortcut**:
`supersession.py` registers a substitute whose `detect()` is always `False`, and
three modules are `SupersededMigration` subclasses kept only so upgrade history
stays meaningful; the two inert modules (`0.10.9`, `2.0.0_historical_status`)
declare `detect()` false with a fixed `apply()` result in their own source. A
test asserts every note in the native table equals the catalog's
`superseded_by`, and the oracle fixture captures the real `detect`/`can_apply`/
`apply` verdicts per id, including the two that keep an allowing `can_apply`.

## Behaviour pinned against the pinned source

- **`.gitignore` write shape**: exact marker line, one blank line before the
  marker, exactly one trailing newline, CRLF preserved through the manager path
  and normalised to LF through `Path.read_text`/`write_text` call sites, whole-line
  membership (not substring) in `ensure_entries` and substring presence where the
  source uses `in content` (`0.4.8`, `2.0.9`).
- **Removals**: anchored whole-line match sets for `0.12.1` (the four exhaustive
  `kitty-specs` spellings, subpath rules kept) and `2.0.0a5` (two rules + two
  headings), the source's per-line `Line N: Removed '…'` messages, `\n{3,}`→`\n\n`
  collapse, `errors="ignore"` decoding, and a symlinked `.gitignore` that
  `detect()` reports as needing work while `can_apply()` refuses (so the runner
  skips rather than writes through it).
- **Git-backed guards**: `.worktrees` coverage requires an equivalent whole-root
  rule, rejects any later negation, refuses tracked paths under the root with the
  source's message, and `force_append`s the rule at EOF before re-verifying.
- **Hygiene**: `git rm --cached` of the three local-runtime paths, with the
  source's `Added gitignore entries: …` / `gitignore entries already present` /
  `Untracked local runtime file: …` message shapes.
- **Layout**: `.specify/`→`.kittify/` and `specs/`→`kitty-specs/` rename with the
  source's "manual merge required" refusals; `active-mission` removal including a
  dangling symlink, and the source's warn-only "not found" path.
- **Ledger wire shape**: `identity`, `content_hash`, `clone_local` (omitted when
  false), `superseded_by` (omitted when empty) and JSON spelling are compared
  byte-for-byte for every declared id and every result.

## Verification executed

Commands (bounded jobs, no release build, no new target directory, no timing):

```sh
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test  -p sk-install --locked --no-fail-fast
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo clippy -p sk-install --all-targets --locked -- -D warnings
cargo fmt -p sk-install -- --check     # my files only; see note
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo check -p sk-install --locked   # in the live shared checkout
```

- `cargo test -p sk-install --locked --no-fail-fast`: **264 passed, 0 failed,
  1 ignored** across 20 suite results (the ignored case is a subprocess helper
  exercised by its cross-language parent), run in a checkout of `ff4452b02` plus
  exactly this tranche's files, with `SK_KERNEL_PYTHON` pointing at the verified
  oracle. Two new integration suites contribute 11 tests
  (`migrations_gitignore_oracle.rs` 5, `migrations_gitignore_paths.rs` 6); the
  lib target contributes 22 new unit tests (`ignore` 7, `gitignore_family` 7,
  `retired` 4, `legacy_layout` 4), which is 102 pre-existing + 22 = 124.
- `cargo clippy -p sk-install --all-targets --locked -- -D warnings`: passed.
- `rustfmt --check` over the six changed/added source files and both new test
  files: passed. The crate-wide `cargo fmt -p sk-install -- --check` currently
  reports diffs in files this tranche does not own (`activation.rs`,
  `attributes.rs`, `identity.rs`, `init.rs`, `ledger.rs`, `lib.rs`, `lifecycle.rs`,
  `lock.rs`, `plan.rs`, `skills.rs`, `profiles.rs`, `session.rs`,
  `template_tree.rs`) because those are mid-edit by other owners; the same shared
  checkout briefly failed to compile `sk-doctrine` mid-run for the same reason.
- `cargo check -p sk-install --locked` in the live shared checkout: **passed**,
  i.e. this tranche integrates with the other owners' current work.
- The two new integration suites also pass **in the live shared checkout**
  against the other owners' current work:
  `cargo test -p sk-install --locked --test migrations_gitignore_oracle
  --test migrations_gitignore_paths` → **11 passed, 0 failed** (5 + 6).
- One pre-existing, load-sensitive flake was observed in a file this tranche does
  not own: `sk-install/src/lock.rs::lock_is_exclusive_then_released_on_drop`
  failed once while several other agents were compiling in the shared target
  directory (`LockUnavailable { waited_ms: 0 }` on re-acquire after drop) and
  passed in the `--no-fail-fast` run of the identical sources. Reported, not
  changed: it is in the lock owner's surface.

### Oracle evidence (this tranche)

- **61 differential cases** in `crates/sk-install/tests/fixtures/migrations/gitignore_oracle.json`,
  generated by `crates/sk-install/tests/fixtures/migrations/generate_migration_oracle.py`
  under the lock-exact oracle (`.venv-rust-oracle`, Python `3.11.15`,
  `verify_oracle.py` ok, `PYTHONPATH=src`, `SPEC_KITTY_PACKS_ROOT=$PWD/packs`).
  Each case records the project tree before, the pinned `detect`/`can_apply`/`apply`
  verdicts, the tree after, the git index contents and the post-apply `detect`.
  Cases cover: absent/partial/present/CRLF/slash-less/comment variants, a
  substring-only match, a later negation, tracked paths under a managed root
  (guard refusal), a symlinked ignore file (refusal), a Git-tracked local-runtime
  file, a dangling `active-mission` symlink, the `.specify/` layouts that must be
  refused (pinned `detect`/`can_apply` only, tree untouched), and both inert
  modules.
- **Reproducibility test.** `the_committed_fixture_is_reproducible_with_the_lock_exact_oracle`
  re-runs the generator with the verified oracle into a temporary directory and
  requires the regenerated fixtures to equal the committed ones, so a drifted
  fixture or a drifted oracle fails instead of passing on stale evidence.
- **Ledger oracle**: `ledger_oracle.json` holds `build_record(...)` output for all
  136 declared ids × `success|skipped|failed` at a fixed timestamp;
  `replay_ledger_lines_match_the_oracle_wire_shape` compares the native line for
  every id and requires at least `46 × 3` of the rows to belong to native ids.
- Every project, `HOME`, XDG directory and `TMPDIR` used by the generator and the
  tests is a fresh temporary directory; nothing reads or writes this checkout's
  runtime state.

### End-to-end runner evidence

`tests/migrations_gitignore_paths.rs`:
- a selection of only implemented ids applies in catalog order, settles both
  partitions, advances `metadata.yaml`, and a re-run applies nothing and appends
  no ledger record;
- a superseded id is recorded `skipped` once with its successor note, is
  re-selected on the next run, and still appends nothing; a settled record stays
  note-free;
- an ordinary `skipped` is recorded once and is not duplicated on a re-run;
- an unevaluable project (symlinked `.gitignore`) is a recorded `failed` that
  stops the run, leaves the version untouched, and never writes through the link;
- a selection containing an unimplemented id still refuses before any write.

## Deliberate deviations (fail-closed, documented)

1. **Symlinked ignore files are never written through.** The pinned `0.4.8`
   migration's `apply()` uses `Path.write_text`, which follows a symlink. Native
   code refuses with `ignore_file_unsafe` (`docs/rust/DECISIONS.md`: untrusted
   paths are rejected at the owning boundary). `detect`/`can_apply` still answer
   exactly as the source does, so the runner's verdict/skip behaviour is
   unaffected; only the unsafe write is refused.
2. **A cross-device layout rename is refused** instead of `shutil.move`'s
   copy-then-delete, because a half-copied `.kittify/` is worse than a retry. No
   real project layout puts `.specify/` and `.kittify/` on different filesystems.
3. **`.kittify/active-mission` being a directory is a typed refusal**, not an
   `unlink` attempt: that is not the retired shape, and recursing into it would
   destroy work.
4. Path-bearing change messages are normalised to `<project>` in the fixture and
   in the replay comparison, so the evidence is machine-independent.

## Coordinator mission-lock instruction (recorded, still open)

The coordinator reported that the independent Mission-lock review accepted the
`sk-status` mechanics with fixes, and requires that a native port of
`src/specify_cli/migration/mission_state.py` (the baseline rekey
consumer around line 2064) must:

1. perform the whole repair inside the enclosing Mission-lock hold,
2. call `sk_status::mutation_lock::extend_mission_lock_to_moved_key(mission_dir, Some(repo_root), wait)`
   **while that hold is still open** after the meta repair may have minted
   `mission_id` (which moves the lock key), and
3. **re-read the observed rows after `extension.moved()` is true**, before the
   status/quarantine phases, because another writer holding only the new key may
   have appended in between.

Nothing in this tranche claims that integration: `extend_mission_lock_to_moved_key`
still has **no native migration consumer**, and the four residue-safe mission
migrations that would be its first consumer
(`0.9.0_residue_safe_frontmatter_only_lanes`,
`0.9.1_residue_safe_complete_migration`,
`2.0.6_residue_safe_consistency_sweep`,
`3.1.1_residue_safe_normalize_status_json`) remain `unimplemented` and refuse
before any write. No `mutation_lock`/`mutation`/mission-state file was touched.
The instruction is therefore carried forward as an acceptance condition for that
family, not silently dropped.

## Remaining work and exact cross-owner dependencies

`unimplemented` ids stay `unimplemented`, so a selection containing one refuses
before the first write. The remaining 90 group into families, each blocked on a
seam this tranche does not own:

| family | ids | exact dependency |
| --- | --- | --- |
| Mission-state repair (residue-safe successors + envelope + provenance) | `0.9.0_residue_safe_frontmatter_only_lanes`, `0.9.1_residue_safe_complete_migration`, `2.0.6_residue_safe_consistency_sweep`, `3.1.1_residue_safe_normalize_status_json`, `3_3_0_op_record_schema_v2`, `runtime_state_backfill`, `verdict_provenance_backfill`, `3.2.7_heal_provenance_paths`, `migrate_lifecycle_envelope`, `3_3_1_context_sources_consolidation` | a native `migration/mission_state.py` repair core (ledger replay, quarantine, lane rebuild, status projection) **including** the `extend_mission_lock_to_moved_key` + re-read protocol above; today only `sk-status`'s lock API and status replay exist |
| Command/skill surface materialisation | `0.6.5_commands_rename`, `0.10.1_populate_slash_commands`, `0.10.2_update_slash_commands`, `0.10.14_update_implement_slash_command`, `0.11.1_update_implement_slash_command`, `2.0.1_fix_generated_command_templates`, `2.1.3_restore_prompt_commands`, `3.0.2_restore_prompt_commands`(retired), `3.1.1_direct_canonical_commands`, `3.1.2_globalize_commands`, `3.2.0a4_safe_globalize_commands`, `4.0.0rc8_command_skill_hyphen_names` | a native command/skill install seam over a template source (`render`/`skills`/`assets` owners). The migrations must also call `guard_destructive_removal` with the canonical-content prover |
| Skill pack repair/install | `2.0.11_install_skills`, `2.1.1_repair_skill_pack`, `2.1.2_fix_orchestrator_api_skill`, `2.1.2_fix_runtime_next_skill`, `2.1.2_install_git_workflow_skill`, `2.1.2_install_mission_system_skill`, `2.1.2_remove_release_skill`, `2.2.0_profile_context_deployment`, `3.0.3_globalize_skill_pack`, `3.2.0rc35_spk_skill_pack`, `3.2.0rc35_codex_to_skills`, `3.2.0rc45_retire_standalone_skill_surface`, `3.2.0rc43_retire_profile_context_command`, `2.0.11_remove_clarify_command`, `3.2.6_retire_rtk_search_tooling`, `0_9_3_surface_repair_wiring`, `0_9_4_roo_deprecation` | the same canonical-content prover **plus** the shipped skill/template tree as an explicit template source (the migrations replace files with `charter.offering` / shipped-skill bytes; `sk-install` has no bundled pack content and must not invent it) |
| Asset-preservation prover | the delete-shaped rows above (`2.1.2_remove_release_skill`, `2.0.11_remove_clarify_command`, `3.2.0rc43_retire_profile_context_command`, `3.2.0rc45_retire_standalone_skill_surface`, `3.2.6_retire_rtk_search_tooling`, `0_9_4_roo_deprecation`, `0.7.2_worktree_commands_dedup`) | **exact proposal:** add `sk-install::preservation` (owner: assets) exposing `ManifestProver`/`CanonicalContentProver` equivalents and a `guard_destructive_removal(path, project_root, prover, is_tree, backup_parent)` that archives unproven content under `.kittify/` before removing it. `template_tree::guard_destructive_removal` is init-run-scratch-specific (a `run_created` set) and cannot prove a pre-existing asset, so it must not be reused |
| Charter activation/offering/bundle | `3.2.0rc35_charter_bundle_v2`, `charter_pack_cutover`, `consolidate_charter_bundle_fold`, `3.2.0rc35_activate_builtin_mission_types`, `3.2.0rc35_charter_manifest_defaults_repair`, `3.2.0rc35_strip_selection_config`, `2.0.1_tool_config_key_rename`, `3.1.1_charter_rename`, `3.0.0_canonical_context`, `4.0.0rc8_project_intent_directive_rename`, `4.0.0rc8_activation_ledger`, `4.0.0rc8_promote_interview_selections`, `4.0.0rc5_retire_single_owner_doctrine_ids` | the charter activation/offering authorities (`charter.activation.api`) and a **round-trip YAML writer** with ruamel-equivalent formatting. `YamlEditor` is additive-only (insert/upsert one mapping level, no key rename/delete), so a `2.0.1` rename or a `selection`-block delete cannot be reproduced byte-faithfully today; adding delete/rename there is a `yaml_edit` owner change, not a migration change |
| `meta.json` / mission metadata | `0.13.8_target_branch`, `0.9.2_research_mission_templates`, `0.10.8_fix_memory_structure`, `0.13.0_research_csv_schema_check`, `0.14.0_centralized_feature_detection`, `3.2.0rc30_fix_runtime_next_result_default`, `4_0_0rc5_heal_template_set_provenance` | the mission-metadata reader/writer (`load_meta_fail_closed`, `locked_update_meta`, JSON formatting) and the Mission write lock owned by `sk-mission`/`sk-status` |
| Template-set / command-body rewrites | `0.6.7_ensure_missions`, `0.7.3_update_scripts`, `0.8.0_worktree_agents_symlink`, `0.10.6_workflow_simplification`, `0.11.1_improved_workflow_templates`, `0.11.2_improved_workflow_templates`, `0.11.3_workflow_agent_flag`, `0.12.0_documentation_mission`, `0.13.0_update_charter_templates`, `0.13.0_update_research_implement_templates`, `0.13.5_add_commit_workflow_to_templates`, `2.0.7_fix_stale_overrides`, `2.1.3_fix_planning_repository_terminology`, `3.2.0rc35_repository_root_checkout_terminology`, `3.2.0rc35_update_planning_templates`, `3.2.0rc35_fix_prompt_file_workaround`, `3_2_0rc39_refresh_orientation_block`, `3_3_0_session_presence_all_harnesses`, `3_3_0_session_presence_claude_code`, `0.10.0_python_only` | the shipped template/skill content as a template source (the migrations write canonical bodies, not derived ones) |
| Ledger adoption / hosted / env / dashboard | `4.0.0rc8_backfill_migration_ledger`, `4_0_0_retired_hosted_target`, `3.2.0rc35_unified_bundle`, `3.2.8_provision_kitty_env`, `3.2.0rc35_pi_letta_agent_backfill`, `3.2.0rc35_kittify_profile_handoff`, `4.0.0rc5_retire_bundled_dashboard`, `4_0_0rc5_single_branch_code_lanes_restamp`, `4.0.0rc8_retire_antigravity_workflows`, `2.0.0_retire_git_hooks` | ledger adoption semantics + `toml_edit` (now available) for `4_0_0_retired_hosted_target`; a native process probe/termination contract for the dashboard; the canonical Mission write lock + completion predicate for the lane restamp; the command-skill installer for antigravity |

Two of these deserve an explicit note:

- `4_0_0_retired_hosted_target`'s last blocker is gone: `toml_edit` is now a
  `sk-install` dependency, so its `[sync].server_url` cleanup is portable. It was
  not ported here only because this tranche stopped at the families above; it is
  the cheapest next id.
- `2.0.0_retire_git_hooks` needs the Git adapter's hook-removal seam; `0.13.1`
  and the lane-tip recorder already show the shape, but the migration also
  removes `.git/hooks/*` files it must prove it owns.

## Exact remaining unsupported ids (90)

```text
4.0.0rc8_backfill_migration_ledger
3.2.0rc35_charter_bundle_v2
charter_pack_cutover
0.6.5_commands_rename
0.6.7_ensure_missions
0.7.2_worktree_commands_dedup
0.7.3_update_scripts
0.8.0_worktree_agents_symlink
0.9.0_residue_safe_frontmatter_only_lanes
0.9.1_residue_safe_complete_migration
0.9.2_research_mission_templates
0.10.0_python_only
0.10.1_populate_slash_commands
0.10.2_update_slash_commands
0.10.6_workflow_simplification
0.10.8_fix_memory_structure
0.10.14_update_implement_slash_command
0.11.1_improved_workflow_templates
0.11.1_update_implement_slash_command
0.11.2_improved_workflow_templates
0.11.3_workflow_agent_flag
0.12.0_documentation_mission
0.13.0_research_csv_schema_check
0.13.0_update_charter_templates
0.13.0_update_research_implement_templates
0.13.5_add_commit_workflow_to_templates
0.13.8_target_branch
0.14.0_centralized_feature_detection
2.0.0_retire_git_hooks
2.0.1_fix_generated_command_templates
2.0.1_tool_config_key_rename
2.0.6_residue_safe_consistency_sweep
2.0.7_fix_stale_overrides
2.0.11_install_skills
2.0.11_remove_clarify_command
2.1.1_repair_skill_pack
2.1.2_fix_orchestrator_api_skill
2.1.2_fix_runtime_next_skill
2.1.2_install_git_workflow_skill
2.1.2_install_mission_system_skill
2.1.2_remove_release_skill
2.1.3_fix_planning_repository_terminology
2.1.3_restore_prompt_commands
2.2.0_profile_context_deployment
3.0.0_canonical_context
3.0.3_globalize_skill_pack
3.1.1_charter_rename
3.1.1_direct_canonical_commands
3.1.1_residue_safe_normalize_status_json
3.1.2_globalize_commands
3.2.0a4_normalize_mission_lifecycle
3.2.0a4_safe_globalize_commands
3.2.0rc30_fix_runtime_next_result_default
3.2.0rc35_activate_builtin_mission_types
3.2.0rc35_charter_manifest_defaults_repair
3.2.0rc35_codex_to_skills
3.2.0rc35_fix_prompt_file_workaround
3.2.0rc35_kittify_profile_handoff
3.2.0rc35_pi_letta_agent_backfill
3.2.0rc35_repository_root_checkout_terminology
3.2.0rc35_spk_skill_pack
3.2.0rc35_strip_selection_config
3.2.0rc35_unified_bundle
3.2.0rc35_update_planning_templates
3_2_0rc39_refresh_orientation_block
3_3_0_session_presence_all_harnesses
3_3_0_session_presence_claude_code
3_3_0_op_record_schema_v2
3.2.0rc43_retire_profile_context_command
0_9_3_surface_repair_wiring
0_9_4_roo_deprecation
3.2.0rc45_retire_standalone_skill_surface
3.2.6_retire_rtk_search_tooling
consolidate_charter_bundle_fold
runtime_state_backfill
verdict_provenance_backfill
3.2.7_heal_provenance_paths
3.2.8_provision_kitty_env
migrate_lifecycle_envelope
3_3_1_context_sources_consolidation
4_0_0_retired_hosted_target
4_0_0rc5_heal_template_set_provenance
4.0.0rc5_retire_bundled_dashboard
4.0.0rc5_retire_single_owner_doctrine_ids
4_0_0rc5_single_branch_code_lanes_restamp
4.0.0rc8_project_intent_directive_rename
4.0.0rc8_activation_ledger
4.0.0rc8_promote_interview_selections
4.0.0rc8_command_skill_hyphen_names
4.0.0rc8_retire_antigravity_workflows
```

## Files owned and changed

- `crates/sk-install/src/upgrade.rs` (modified: `Failed` outcome, supersession
  stamping, record dedupe, 46 registrations, docs, test count)
- `crates/sk-install/src/migrations/mod.rs` (modified: modules + family table)
- `crates/sk-install/src/migrations/ignore.rs` (new)
- `crates/sk-install/src/migrations/gitignore_family.rs` (new)
- `crates/sk-install/src/migrations/retired.rs` (new)
- `crates/sk-install/src/migrations/legacy_layout.rs` (new)
- `crates/sk-install/tests/migrations_gitignore_oracle.rs` (new)
- `crates/sk-install/tests/migrations_gitignore_paths.rs` (new)
- `crates/sk-install/tests/fixtures/migrations/generate_migration_oracle.py` (new)
- `crates/sk-install/tests/fixtures/migrations/gitignore_oracle.json` (new, 61 cases)
- `crates/sk-install/tests/fixtures/migrations/ledger_oracle.json` (new, 136 ids × 3)

No other crate or file was edited; no manifest, no CLI, no `init`/`assets`/
`skills`/`activation`/`ledger`/`error`/`lib`, no `mutation_lock`/`mutation`/
mission files, no commit, push or tag.
