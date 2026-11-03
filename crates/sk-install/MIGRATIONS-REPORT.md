# Native upgrade migrations — tranche report

> **Superseded by [`MIGRATIONS-CONTINUATION-REPORT.md`](MIGRATIONS-CONTINUATION-REPORT.md),
> which records 46 of 136 native ids on top of `ff4452b02` and the runner
> contracts (recorded `failed` rows, supersession stamping, record de-duplication)
> this report predates.** Kept as the historical checkpoint record for the
> `ab3e0fcc8` tranche; its unsupported-id list and dependency notes below are
> accurate for that checkpoint only.

**Status: 16 of 136 declared migrations execute natively; 120 remain unsupported and the full rewrite stays blocked.**

Checkpoint `ab3e0fcc8`, pinned Python behavior `9aed3a8012cb5eb22c0128b14d525039f95fea7a`. This report covers only the migration tranche in `sk-install`; it is not a full-compatibility claim.

## What changed

- Added `sk-install/src/migrations/` with seven migration modules plus two shared helpers (`git`, `json`): merge-driver seeding, run-index healing, lane-tip hooks, project pack descriptors, charter-pack ignore rules, narrow agent ignores and stale mission-template forks.
- Extended `sk-install/src/upgrade.rs` with a per-migration `can_apply` seam. A `detect()==true` migration whose `can_apply` refuses the project is now a skip that is **not** recorded, so it is re-evaluated after the project changes — matching the pinned runner.
- Fixed `skip_outside_git` to record nothing (the source's `_not_inside_repository_skip` contract), rather than settling a migration that a later run inside a repository must apply.
- No production Python process, FFI interpreter, success stub or invented protocol was added. No `Cargo.toml`/`Cargo.lock` edits and no new dependencies.

## Implemented ids

| id | Native module | Pinned source evidence | Native test evidence |
| --- | --- | --- | --- |
| `0.13.1_exclude_worktrees` | `crates/sk-install/src/upgrade.rs` | `m_0_13_1_exclude_worktrees.py` | `upgrade.rs` unit tests |
| `3.1.1_event_log_merge_driver` | `crates/sk-install/src/migrations/merge_drivers.rs` | `_merge_driver_seeding.py`, `m_3_1_1_event_log_merge_driver.py` | `tests/migrations_merge_drivers.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `3.2.0rc28_github_diff_attributes` | `crates/sk-install/src/upgrade.rs` | `m_3_2_0rc28_github_diff_attributes.py` | `upgrade.rs` unit tests |
| `3.2.6_decisions_event_log_merge_driver` | `crates/sk-install/src/migrations/merge_drivers.rs` | `_merge_driver_seeding.py`, `m_3_2_6_decisions_event_log_merge_driver.py` | `tests/migrations_merge_drivers.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `3.2.6_gate_artifact_merge_drivers` | `crates/sk-install/src/migrations/merge_drivers.rs` | `_merge_driver_seeding.py`, `m_3_2_6_gate_artifact_merge_drivers.py` | `tests/migrations_merge_drivers.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `3.2.6_issue_matrix_driver_repoint` | `crates/sk-install/src/migrations/merge_drivers.rs` | `_merge_driver_seeding.py`, `m_3_2_6_issue_matrix_driver_repoint.py` | `tests/migrations_merge_drivers.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `3.2.6_meta_traces_merge_drivers` | `crates/sk-install/src/migrations/merge_drivers.rs` | `_merge_driver_seeding.py`, `m_3_2_6_meta_traces_merge_drivers.py` | `tests/migrations_merge_drivers.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `3.2.7_review_cycle_merge_driver` | `crates/sk-install/src/migrations/merge_drivers.rs` | `_merge_driver_seeding.py`, `m_3_2_7_review_cycle_merge_driver.py` | `tests/migrations_merge_drivers.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `4.0.0rc5_decision_index_merge_driver` | `crates/sk-install/src/migrations/merge_drivers.rs` | `_merge_driver_seeding.py`, `m_4_0_0rc5_decision_index_merge_driver.py` | `tests/migrations_merge_drivers.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `4_0_0rc5_heal_run_index_paths` | `crates/sk-install/src/migrations/run_index.rs` | `m_4_0_0rc5_heal_run_index_paths.py`, `runtime/next/run_index.py` | `tests/migrations_run_index.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `4_0_0rc5_install_lane_tip_recorder` | `crates/sk-install/src/migrations/lane_tip.rs` | `m_4_0_0rc5_install_lane_tip_recorder.py`, `policy/lane_tip_recorder.py`, `lanes/branch_naming.py` | `tests/migrations_lane_tip.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `4.0.0rc7_exclude_spec_kitty` | `crates/sk-install/src/upgrade.rs` | `m_4_0_0rc7_exclude_spec_kitty.py` | `upgrade.rs` unit tests, `tests/upgrade_migration_paths_4x.rs` |
| `4.0.0rc7_pack_level_backfill` | `crates/sk-install/src/migrations/pack_level.rs` | `m_4_0_0rc7_pack_level_backfill.py`, `tests/upgrade/test_pack_level_backfill.py` | `tests/migrations_pack_and_gitignore.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `4.0.0rc7_track_charter_pack_yaml` | `crates/sk-install/src/migrations/charter_pack_gitignore.rs` | `m_4_0_0rc7_track_charter_pack_yaml.py`, `_charter_pack_gitignore.py`, `tests/upgrade/test_track_charter_pack_yaml.py` | `tests/migrations_pack_and_gitignore.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `4.0.0rc8_narrow_agent_gitignore` | `crates/sk-install/src/migrations/narrow_agent_gitignore.rs` | `m_4_0_0rc8_narrow_agent_gitignore.py`, `agent_utils/ignore_entries.py`, `tests/specify_cli/upgrade/migrations/test_m_4_0_0rc8_narrow_agent_gitignore.py` | `tests/migrations_narrow_agent_gitignore.rs`, `tests/upgrade_migration_paths_4x.rs` |
| `4.0.0rc8_retire_stale_mission_template_forks` | `crates/sk-install/src/migrations/mission_template_forks.rs` | `m_4_0_0rc8_retire_stale_mission_template_forks.py`, `tests/upgrade/migrations/test_m_4_0_0rc8_retire_stale_mission_template_forks.py` | `tests/migrations_mission_forks.rs`, `tests/upgrade_migration_paths_4x.rs` |

### Coherent reachable paths

1. **4.0.0rc6 → 4.0.0rc7 is a complete native path.** A ledger settled through rc6 and an rc7 target selects exactly `4.0.0rc7_exclude_spec_kitty`, `4.0.0rc7_pack_level_backfill` and `4.0.0rc7_track_charter_pack_yaml`; all three apply, record and advance metadata, and a second run is a no-op (`tests/upgrade_migration_paths_4x.rs::rc6_to_rc7_is_a_complete_native_path`).
2. **The pre-rc8 clone-local family is complete.** A fresh clone of a 4.0.0rc7 project whose tracked ledger is settled but whose per-clone ledger is absent selects the ten implemented clone-local migrations through rc7 (merge drivers, exclude, index heal, lane-tip hooks); all apply and record in the clone-local partition (`a_fresh_clone_with_only_clone_local_state_missing_applies_the_whole_family`).
3. **Unsupported selections still refuse before mutation.** A realistic rc8 ledger naming the six remaining rc8 paths refuses with `migration_not_implemented` before metadata, ledger or project files change, and the names are exact (`a_realistic_rc8_ledger_still_refuses_every_unimplemented_id_before_mutation`).
4. **A `can_apply` refusal is retried.** A malformed project `pack.yaml` leaves the rc7 backfill unrecorded and re-selected on the next run; the file is untouched (`a_can_apply_refusal_is_a_skip_that_is_not_recorded_and_is_retried`).

## Source-defined behavior covered

- Merge drivers: exact `.gitattributes` entry, exact `merge.<key>.name`/`driver`, append-preserving `ensure_line`, non-git warning, idempotent detection.
- Run index: in-tree absolute paths relativized with subpath preserved; out-of-tree/cross-OS paths re-anchored to `.kittify/runtime/runs/<run_id>`; relative tokens untouched; Python `json.dumps(indent=2, sort_keys=True, ensure_ascii=True)` bytes; missing/corrupt index no-op; non-object and non-UTF-8 refused.
- Lane-tip: exact `sh` hook body and signature, `post-commit` + `post-rewrite`, executable mode, foreign hook preserved, `core.hooksPath` scope/in-tree refusals, linked-worktree/common-dir resolution.
- Pack descriptor: never creates the pack directory, fresh ULID descriptor, authored comments/`pack_id` preserved, `pack_level` added only when absent, malformed/non-mapping refusal routing.
- Charter-pack ignore: blanket and anchored spellings, sibling allow-rule grouping, CRLF preservation, non-fatal unsafe-file warning, still-ignored warning.
- Narrow agent ignores: frozen legacy entry list, marker/0.4.8 run classification, cursor neighbour proof, operator-rule preservation, backfill rows, explicit empty agent list, built-in skill names, session files, pack-skill manifest and profile manifest entries, visibility warning, CRLF, idempotence.
- Stale forks: full frozen historical SHA-256 table, CRLF folding, override removal vs legacy replacement, fork-only removal, hand-edit preservation, symlink refusal, missing-pack refusal routing, idempotence.

## Verification executed

Commands (no release/aggregate build, no new target directory):

```sh
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p sk-install --locked
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo clippy -p sk-install --all-targets --locked -- -D warnings
cargo fmt -p sk-install -- --check
```

- `cargo test -p sk-install --locked`: **228 passed, 0 failed, 0 ignored** across the crate's unit and integration suites (16 suites). The new migration integration test files account for 53 of those tests.
- `cargo clippy -p sk-install --all-targets --locked -- -D warnings`: passed.
- `cargo fmt -p sk-install -- --check`: passed. Workspace-wide formatting was **not** run because this shared checkout has unrelated in-progress edits in other crates.
- All migration tests use temporary projects/homes only. The fork tests use the verified `packs/built-in/missions` tree in this checkout; no canonical template or agent copy is overwritten.
- The Python oracle was not used as a test runner (the pinned `.venv-rust-oracle` has no `pytest`); source parity is pinned by reading the baseline modules and their checked-in tests. No Python production path is invoked by the Rust code.

## Remaining blockers / required dependencies

- **`4_0_0_retired_hosted_target` is not ported.** The source performs a `toml` load → `tomli_w` dump. The workspace has no direct TOML parser/serializer dependency, and adding one requires a `Cargo.toml`/`Cargo.lock` edit that this task forbids. Needed: a direct TOML writer dependency (for example `toml`/`toml_edit`) with write support, or an approved manifest change.
- **Command-skill/antigravity migrations are not ported.** `4.0.0rc8_command_skill_hyphen_names` and `4.0.0rc8_retire_antigravity_workflows` depend on the command-skill installer, which sits in the frozen skills surface and has no native command-skill install seam in this crate.
- **Charter activation/cutover migrations are not ported.** `4.0.0rc8_activation_ledger`, `4.0.0rc8_promote_interview_selections`, `4.0.0rc8_project_intent_directive_rename`, `charter_pack_cutover`, `4_0_0rc5_heal_template_set_provenance` and the rc8 ledger adoption step need the charter activation/offering/YAML round-trip authorities and the canonical Mission write lock.
- **Dashboard/topology cleanups are not ported.** `4.0.0rc5_retire_bundled_dashboard` needs a native process probe/termination contract; `4_0_0rc5_single_branch_code_lanes_restamp` needs the canonical Mission write lock and completion predicate. Neither is simulated.
- **Everything else remains pending.** The exact unsupported list below is the `unimplemented` set the runner will name and refuse before mutation.

The crate `README.md` and `docs/rust/STATUS.md` still describe the earlier three-migration checkpoint; this report is the tranche's count for the coordinator to fold into those surfaces.

## Exact remaining unsupported ids (120)

```text
4.0.0rc8_backfill_migration_ledger
3.2.0rc35_charter_bundle_v2
charter_pack_cutover
0.2.0_specify_to_kittify
0.4.8_gitignore_agents
0.6.5_commands_rename
0.6.7_ensure_missions
0.7.2_worktree_commands_dedup
0.7.3_update_scripts
0.8.0_remove_active_mission
0.8.0_worktree_agents_symlink
0.9.0_frontmatter_only_lanes
0.9.0_residue_safe_frontmatter_only_lanes
0.9.1_complete_migration
0.9.1_residue_safe_complete_migration
0.9.2_research_mission_templates
0.10.0_python_only
0.10.1_populate_slash_commands
0.10.2_update_slash_commands
0.10.6_workflow_simplification
0.10.8_fix_memory_structure
0.10.9_repair_templates
0.10.12_charter_cleanup
0.10.14_update_implement_slash_command
0.11.1_improved_workflow_templates
0.11.1_update_implement_slash_command
0.11.2_improved_workflow_templates
0.11.3_workflow_agent_flag
0.12.0_documentation_mission
0.12.1_remove_kitty_specs_from_gitignore
0.13.0_research_csv_schema_check
0.13.0_update_charter_templates
0.13.0_update_research_implement_templates
0.13.5_add_commit_workflow_to_templates
0.13.8_target_branch
0.14.0_centralized_feature_detection
2.0.0a5_remove_wp_status_gitignore_rule
2.0.0_charter_directory
2.0.0_historical_status_migration
2.0.0_retire_git_hooks
2.0.1_fix_generated_command_templates
2.0.1_tool_config_key_rename
2.0.2_charter_context_bootstrap
2.0.6_consistency_sweep
2.0.6_residue_safe_consistency_sweep
2.0.7_fix_stale_overrides
2.0.9_state_gitignore
2.0.11_install_skills
2.0.11_remove_clarify_command
2.1.1_repair_skill_pack
2.1.2_fix_charter_doctrine_skill
2.1.2_fix_glossary_context_skill
2.1.2_fix_orchestrator_api_skill
2.1.2_fix_runtime_next_skill
2.1.2_install_git_workflow_skill
2.1.2_install_mission_system_skill
2.1.2_remove_release_skill
2.1.3_fix_planning_repository_terminology
2.1.3_restore_prompt_commands
2.1.4_enforce_command_file_state
2.2.0_profile_context_deployment
3.0.0_canonical_context
3.0.2_restore_prompt_commands
3.0.3_globalize_skill_pack
3.1.1_charter_rename
3.1.1_direct_canonical_commands
3.1.1_normalize_status_json
3.1.1_residue_safe_normalize_status_json
3.1.2_globalize_commands
3.2.0a4_normalize_mission_lifecycle
3.2.0a4_safe_globalize_commands
3.2.0rc30_fix_runtime_next_result_default
3.2.0rc35_activate_builtin_mission_types
3.2.0rc35_charter_manifest_defaults_repair
3.2.0rc35_codex_to_skills
3.2.0rc35_default_charter_pack
3.2.0rc35_fix_prompt_file_workaround
3.2.0rc35_kittify_profile_handoff
3.2.0rc35_pi_letta_agent_backfill
3.2.0rc35_repository_root_checkout_terminology
3.2.0rc35_spk_skill_pack
3.2.0rc35_strip_selection_config
3.2.0rc35_kittify_runtime_git_hygiene
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
3.2.3_encoding_provenance_gitignore_backfill
3.2.4_derived_mission_views_gitignore_backfill
3.2.4_runtime_dirs_gitignore_backfill
3.2.5_agents_skills_gitignore_backfill
3.2.6_retire_rtk_search_tooling
normalize_activation_absence
unify_charter_activation_promote_answers
consolidate_charter_bundle_fold
runtime_state_backfill
verdict_provenance_backfill
3.2.7_heal_provenance_paths
3.2.8_provision_kitty_env
migrate_lifecycle_envelope
3_3_1_context_sources_consolidation
3.2.6rc3_lint_report_gitignore_backfill
3.2.6rc3_narrow_cursor_gitignore
3.2.6rc3_worktrees_gitignore_backfill
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
