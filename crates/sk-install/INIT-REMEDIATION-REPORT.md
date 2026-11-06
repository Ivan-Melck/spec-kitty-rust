# Init remediation report — N1–N13 (final)

Owner: init remediation sub-agent. Date: 2026-11-03. Repo:
`/home/exedev/.t3/scratch/2026-11-02-pull-current-spec-kitty-from-442fd12a/spec-kitty-rust`
(branch `rust/rewrite`, dirty shared tree; INIT tranche uncommitted).

Inputs read in full: `AGENTS.md`, `.kittify/charter/charter.md`,
`docs/rust/{DECISIONS,LOOP,VERIFICATION}.md`,
`crates/sk-install/INIT-CONTINUATION-REPORT.md`, and the complete independent
review `/tmp/sk-rust-init-continuation-review.md` (N1–N15, F0–F16).
Reproductions in `/tmp/initprobe` (native/oracle trees, agent-set lists,
concurrency rounds) are treated as the red cases.

Status: **FINAL for this owner slice** (the provisional plan below was written
before the first edit, so a cancelled turn would still have left the intended
scope on disk; the final section records what actually ran). Scoped evidence is
green; this is **not** an "init complete" or parity claim.

## Ownership fence (as dispatched)

Owned: `crates/sk-install/src/{init,lock,plan,skills,profiles,session,
template_tree,lifecycle,upgrade,error}.rs`, init-specific tests
(`crates/sk-install/tests/{init_delivery,init_lifecycle,full_init_baseline,
lock_contract}.rs` + new init tests), `crates/sk-cli/src/compat/init.rs`,
`crates/sk-cli/tests/{init_delivery,init_oracle_diff}.rs`.

Excluded: `activation.rs` (doctrine owner), `ledger/catalog/migrations/manifests`,
other `compat/*`, `support.rs`, docs, benchmarks. One explicit exception granted
by the coordinator: the single-line lock-authority change in `upgrade.rs`
(migrations implementation inactive), so `init` and `upgrade` share one lock.

No commits, pushes, background jobs, extra Cargo targets, or aggregate builds.
All Cargo runs use `/tmp/sk-focused-cargo.sh` (temp HOME/XDG/TMP, shared
`flock`, `CARGO_BUILD_JOBS=2`, exact oracle supplied explicitly).

## Planned remediation

| id | defect (review) | fix direction |
| --- | --- | --- |
| N1 | Concurrent fresh inits corrupt templates and leave `.backup-*` residue; delivery is unlocked before `InstallPlan::commit` | Take one canonical `ProjectLock::for_project` for the *whole* delivery and decide fresh/refresh/resume only after it is held (nested plan/identity/event locks stay same-thread reentrant). New two-process CLI concurrency test asserting one `initialized`, one `already_initialized`, no backup dirs, no pending record. |
| N2 | `init` (state-dir lock) and `upgrade` (`.kittify/.install.lock`) no longer exclude each other | Switch the single `upgrade.rs` lock call to `ProjectLock::for_project` (coordinator-permitted one-liner); evidence via source + lock-path unit coverage. |
| N3 | Required activation preset rejects a legitimate already-activated refresh; check runs after writes | Preflight: resolve the activation key first; require/validate the preset only when the key is absent, and do it before the first managed write. `provision` is skipped when the key is present. |
| N4 | Symlinked `AGENTS.md`/`.claude/CLAUDE.md`/`.claude` root abort init; oracle warns and completes | Treat an unowned-symlink/elevated-escape destination as a preserved+warned optional surface for session, skills install and profile projection; never write outside, never weaken the fail-closed manifest-restore boundary. |
| N5 | Empty `.kittify/agent_profiles_manifest.json` for the 11 agent keys with no profile renderer | Do not create a manifest when nothing was projected/kept/pruned and no manifest existed. |
| N6 | copilot-only init writes `.github/copilot-instructions.md` the oracle does not | Add upstream's `check_dir` guard for copilot orientation and run session presence before profile projection (upstream ordering). |
| N7 | Misleading error taxonomy / success text | Dedicated agent-profile-catalog error/code; accurate refresh success text; warnings name the failing surface. |
| N8 | Refresh success text overstates (`verified`) while deleted template trees stay absent silently | Honest refresh text; warn when the template tree is absent on resume/refresh. |
| N9 | `full_init_baseline` oracle normalization omits `agent_key`; CLI oracle diff rewrites substrings of full hashes as ULIDs | Add `agent_key` to the tuple; make ULID normalization boundary-aware so 64-hex hashes compare byte-for-byte. |
| N10 | Crate tests are not HOME-isolated | Not fixed: `std::env::set_var` is `unsafe` in this edition and unsafe Rust is forbidden workspace-wide. Documented as a residual with the coordinator's temp-HOME wrapper as the mitigation. |
| N11 | `SPEC_KITTY_PACKS_ROOT` documented as `$PWD/packs` but native requires `packs/built-in` | Accept both shapes in pack discovery and in the CLI checkout resolution (no new refusals; strictly more accepted layouts). |
| N12 | Docs/evidence claims (crate README, oracle normalizations) | Docs are excluded from this task; the oracle-diff/test normalizations this owner writes are named and tightened (see N9). |
| N13 | Absent `memory/` source is silent | Report a warning from `install_template_tree` and surface it through `InitReport.warnings`. |

Also in scope: the dead `lifecycle::ProjectEventAppendPlan::commit` still takes
`.kittify/../.install.lock`; it will use the canonical project lock (still not
called by init, which uses the canonical `sk-status` writer).

## Test contract additions (red-first)

1. CLI: two simultaneous `init --ai claude,codex,vibe` runs → both exit 0, exactly
   one `Initialized`, no `.kittify/.backup-*`, no pending record, 124 ledger rows.
2. Library: refresh of a fully initialized project with the preset **removed**
   from the asset set succeeds and leaves activation bytes/semantics unchanged.
3. Library: fresh init with the preset removed fails **before** `config.yaml` /
   template tree / skills are written (existing test strengthened).
4. Library: `AGENTS.md` symlink (codex), `.claude/CLAUDE.md` symlink (claude) and
   `.claude -> outside` (claude) init succeeds with a warning, writes no byte
   outside the project, and produces no manifest claiming the refused outputs.
5. Library: nine non-renderer agent keys write no profile manifest; claude does.
6. Library: copilot-only init does not write `.github/copilot-instructions.md`
   but does project `.github/agents/*.agent.md`.
7. CLI oracle post-state: `claude,codex,vibe`, plus `gemini` and `copilot` runs,
   compared byte-for-byte with a boundary-correct ULID normalizer and exact
   profile-manifest hashes.
8. Baseline fixture: skills-manifest tuple includes `agent_key`.
9. Unit: pack-subdir resolution accepts `<root>/<name>` and `<root>/built-in/<name>`.
10. Unit: absent memory source yields a warning instead of silence.

## Non-goals / residual gaps to keep honest

* N10 (test-process HOME isolation) and N12's crate-README wording: not fixed
  here (unsafe env mutation forbidden; docs excluded). Reported to the
  coordinator.
* No new supported-input refusals, no speculative hardening, no production
  Python, no upgrade-migration implementation.
* No claim of whole-workspace green, release build, clippy/fmt aggregate,
  Windows/macOS, packaging, or performance.

## Final evidence

Final status: **fixed, scoped evidence green, not an "init complete" or parity
claim.** Six review defects N1–N6 are closed by code + red-first tests; N7–N9
and N11/N13 were folded; N10/N12 documentation items and F15/F16 remain as
stated below. The tree is dirty and shared; nothing was committed.

### Exact commands and counts

All Cargo invocations used `/tmp/sk-focused-cargo.sh` (temp `HOME`/XDG/TMP,
shared `flock`, `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`) with `--locked`
passed explicitly, the shared `target/`, and no manifest or lockfile changes.

1. `flock … cargo test -p sk-install --lib --test full_init_baseline --test
   init_delivery --test init_lifecycle --test agent_surfaces --test
   lock_contract --test ledger_lock_interop --test manifest_contract --test
   plan_contract --test render_families --test pack_acquisition`
   → **227 passed, 0 failed, 2 ignored** (both ignored are parent-exercised
   subprocess helpers). Log: `/tmp/sk-init-remediation-final-install.log`.
   Breakdown: lib 140 · agent_surfaces 7 · full_init_baseline 6 · init_delivery
   11 · init_lifecycle 20 · lock_contract 1 (+1 ignored) · ledger_lock_interop
   1 (+1 ignored) · manifest_contract 9 · plan_contract 14 · render_families 7 ·
   pack_acquisition 11.
2. `flock … cargo test -p sk-cli --test init_delivery`
   → **8 passed, 0 failed** (was 5). Log:
   `/tmp/sk-init-remediation-final-cli.log`.
3. `SPEC_KITTY_ORACLE_PYTHON="$PWD/.venv-rust-oracle/bin/python" flock … cargo
   test -p sk-cli --test init_oracle_diff -- --nocapture`
   → **1 passed** (three complete post-state comparisons: `claude,codex,vibe`,
   `gemini`, `copilot`), 43.08 s. Log:
   `/tmp/sk-init-remediation-final-oracle.log`. Native env now uses the
   documented `SPEC_KITTY_PACKS_ROOT=<repo>/packs`.
4. `rustfmt --edition 2024 --check` over every file this owner touched → clean.
5. `flock … cargo test -p sk-cli --test compat` → **23 passed, 2 failed**:
   * `init_with_a_slash_command_agent_renders_canonical_prompts_from_the_pack`
     — the expected, coordinator-owned stale expectation (see below);
   * `status_mutation_dispatch_executes_all_four_registered_facades`
     (`mark-status` now needs `tasks.md`) — outside init ownership, status
     surface; the fixture does not call `init`. Not introduced here.

No aggregate build, Clippy/fmt pass, release build, packaging, timing campaign,
commit, push, background job, `sleep`/poll loop, extra Cargo target, or
process-group signal was used. All concurrency tests use native child handles
and channels only.

### Red-first evidence (fix temporarily reverted, test red, file restored)

| fix | temporary revert | red test result |
| --- | --- | --- |
| N1 whole-delivery lock | early `ProjectLock::for_project` replaced by `None` | `concurrent_fresh_inits_serialize_without_backup_residue` → exit 1, `initialization failed: precondition changed for …/.agents/skills/…` (the review's exact failure class). Restored; green. |
| N2 shared upgrade lock | `for_project` → legacy `.kittify/.install.lock` | `upgrade::tests::upgrade_takes_the_same_canonical_lock_as_init` → upgrade succeeded under the canonical lock (no `lock_unavailable`). Restored; green. |
| N3 conditional preset | preset requirement forced on every mode | `a_refresh_of_an_activated_project_needs_no_preset` → `ActivationPresetMissing`. Restored; green. |
| N4 tolerant optional surfaces | destination-refusal warning path reverted to `?` | `symlinked_optional_surfaces_are_preserved_with_warnings_instead_of_aborting` → `SkillsManifestInvalid … resolves outside root`. Restored; green. |
| N5 no empty profile manifest | manifest save condition forced true | `an_agent_set_without_a_profile_renderer_writes_no_profile_manifest` → manifest present for `gemini`. Restored; green. |
| N6 session-before-profiles | the two delivery blocks swapped back | `copilot_only_init_does_not_write_orientation_before_the_harness_root_exists` → `.github/copilot-instructions.md` created. Restored; green. |

Additional red evidence for N5/N6 ordering at CLI level: the expanded
`init_oracle_diff` fails on `gemini`/`copilot` without those fixes (the first
run after adding the agent sets failed on `spec_kitty_version` and then on the
copilot file before the ordering fix, matching `/tmp/initprobe/agentsets/*`).

### Per-finding disposition

| id | status | evidence |
| --- | --- | --- |
| N1 | **fixed** | `initialize_with_details` takes `ProjectLock::for_project` *before* loading the pending record/config and therefore before choosing fresh/refresh/resume. The second concurrent process waits then refreshes; nested plan/identity/event locks re-enter the same thread-local lock. Two-process CLI test: both exit 0, exactly one `Initialized` + one `AlreadyInitialized`, no `.backup-*`, no pending record, 1 event, 124 ledger rows. Library test proves init blocks on `ProjectLock::acquire(project_lock_path(..))`. |
| N2 | **fixed** | Single permitted line in `upgrade.rs` now uses `ProjectLock::for_project`; `lifecycle::ProjectEventAppendPlan::commit` (dead exported plan) uses it too. Red-first test holds the canonical lock and requires `lock_unavailable` from `run_upgrade`. |
| N3 | **fixed** | `preflight_activation` resolves the activation key via the doctrine ledger and validates the preset only when the key is absent, before the template tree/config/skills/metadata. Refresh with the preset removed succeeds (`activation_provisioned == false`, activation bytes unchanged); fresh without the preset fails with `DEFAULT_PRESET_MISSING` and now leaves no config/templates/memory/skills. |
| N4 | **fixed** | Session presence refuses any symlinked path component *and reports it*, never aborts; Claude settings and copilot orientation follow the same path; skills install and profile projection skip a refused optional destination with a warning/preserved entry; the strict manifest-owned restore boundary is unchanged. Tests cover `AGENTS.md`, `.claude/CLAUDE.md`, and a `.claude -> outside` root: exit success, no byte outside, no ownership manifests created. |
| N5 | **fixed** | A brand-new profile manifest is written only when something was claimed; the 11 renderer-less keys now leave no file (tested with `gemini`, `windsurf`, `pi`; `claude` control). The same rule is applied to a fresh skills manifest when every optional skill destination was skipped. |
| N6 | **fixed** | Orientation now runs before profile projection (upstream order) and copilot's `.github` guard is modelled explicitly (`check_dir`). Copilot-only init writes `.github/agents/*.agent.md` but no `.github/copilot-instructions.md`; certified by the `copilot` oracle post-state run. |
| N7 | **fixed (scoped)** | New `AgentProfileCatalogMissing` / `agent_profile_catalog_missing` replaces the misleading `skills_catalog_missing` for profiles; skipped-surface warnings name the skill/profile and path; refresh output no longer claims a template verification. |
| N8 | **fixed** | Refresh success text only claims skill-surface restoration. Resume/refresh of a project whose `.kittify/memory` **and** `.kittify/missions` are both gone emits a warning that init does not re-scaffold. |
| N9 | **fixed** | `full_init_baseline` now includes `agent_key` in its skills-manifest tuple. `init_oracle_diff` ULID normalization is boundary-delimited, so 64-hex `file_hash`/`source_hash` values (skills and profiles manifests) are compared in full. A further fidelity defect found by the expanded comparison — native stamped `spec_kitty_version` for shared-root-only agent sets where the oracle leaves it empty — is fixed by stamping only when a native-root agent is selected. |
| N10 | **not fixed (reported)** | Crate tests (`init_delivery`, `init_lifecycle`, `full_init_baseline`, unit lock tests) still take project locks in the ambient user state dir. `std::env::set_var` is `unsafe` in edition 2024 and unsafe Rust is forbidden workspace-wide, so the owner did not introduce process-global env mutation from parallel tests. Mitigation in every delivered run (and the shared helper) is a temp `HOME`/`XDG_STATE_HOME`; the coordinator should either accept that harness property or add a safe per-test lock-root injection API. |
| N11 | **fixed** | Pack discovery accepts both source-backed shapes (`<root>/<name>` and `<root>/built-in/<name>`, built-in preferred) and the CLI checkout resolver derives the checkout from either `SPEC_KITTY_PACKS_ROOT` shape; the redundant/misleading second refusal was removed (strictly more accepted layouts, no new refusal). Unit test + CLI test with a checkout that has no `packs/` and `SPEC_KITTY_PACKS_ROOT=<repo>/packs`. |
| N12 | **partially fixed (code/test only)** | This owner fixed its own module doc (`.claudeignore`/step order) and the test normalizations above. Crate `README.md`, capabilities matrix and other documentation are excluded from this task; the remaining stale claims listed by the review are unchanged and stay with the coordinator/docs owner. |
| N13 | **fixed** | `install_template_tree` reports an absent `memory`/`templates`/`missions` source in `TemplateTreeReport.warnings`, surfaced through `InitReport.warnings`; unit test covers the absent memory source. |
| N14 | **fixed** | The dead `ProjectEventAppendPlan::commit` now takes the canonical project lock (init still uses the canonical `sk-status` writer and never calls it). |
| N15 | **unchanged** | `HttpsArchiveTransport`/archive hardening and the README "deterministic ULID" wording are untouched, as before; no new input refusals were added in that substrate. |

### Coordinator-owned `tests/compat.rs`: exact expectation fix

The failing test asserts the pre-migration slash-command surface. The pinned
source contract (`9aed3a80`, `init.py`) installs **command skills** only for
`codex/vibe/pi/letta/antigravity`, installs the charter skill catalog into each
selected agent's native skill root, and generates **no** `.claude/commands/*`
(those are `upgrade` migration output). Precise replacement for
`init_with_a_slash_command_agent_renders_canonical_prompts_from_the_pack`:

```rust
#[test]
fn init_with_a_slash_command_agent_installs_the_charter_skill_catalog() {
    let (temp, home) = project();
    let root = temp.path();
    let output = run(
        root,
        &home,
        &["init", "--ai", "claude", "--non-interactive"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Pinned source contract: init does not generate slash-command files; they
    // are upgrade-migration output. Claude receives the charter skill catalog
    // in its native root, agent profiles and the orientation block.
    assert!(root.join(".claude/skills/spec-kitty/SKILL.md").is_file());
    assert!(root.join(".claude/agents/analyst-annie.md").is_file());
    assert!(root.join(".claude/CLAUDE.md").is_file());
    assert!(!root.join(".claude/commands/spec-kitty.specify.md").exists());
    assert!(!root.join(".claude/commands").exists());
}
```

If the coordinator still wants the prompt-content assertions, drive them through
a command-skill agent instead: `.agents/skills/spec-kitty-specify/SKILL.md`
contains the `spec-kitty agent mission create` / `spec-kitty next` text. A copy
of this corrected contract is executed in the owned
`crates/sk-cli/tests/init_delivery.rs` (`a_slash_command_agent_receives_the_
charter_skill_catalog_not_command_files`).

### Honest residuals / notes

* The compat run also showed one **other-owner** failure
  (`status_mutation_dispatch_executes_all_four_registered_facades`, `tasks.md
  not found` from `mark-status`); it does not call init and is not attributable
  to this remediation.
* One unexplained transient: `lock::tests::lock_is_exclusive_then_released_on_drop`
  (pre-existing, unmodified) failed in two combined invocations immediately after
  the red-demo file restores (`/tmp/sk-init-remediation-scoped.log`), then passed
  in four subsequent identical full-suite runs, including the final
  certification. The test uses a 50 ms contention window and a zero-wait
  re-acquire; the coordinator should treat the final green log as the evidence
  and re-run it in the aggregate.
* No Windows/macOS evidence, no whole-workspace aggregate, Clippy/fmt aggregate,
  release build, packaging, migration breadth, transport or performance claim.
  The fixture-based `full_init_baseline` remains a subset baseline; the CLI
  `init_oracle_diff` is the complete post-state comparison.
* Native still does not implement the interactive agent picker, installed-layout
  asset discovery, or the remaining upgrade/transport/global-runtime surface
  (unchanged from the continuation report).
