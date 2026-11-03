# Init continuation report — full delivery, recovery and CLI wiring

Owner: init continuation sub-agent (recovery of task `268f400b`, cancelled by T3
restarts). Date: 2026-11-03. Repo:
`/home/exedev/.t3/scratch/2026-11-02-pull-current-spec-kitty-from-442fd12a/spec-kitty-rust`.

This is a **continuation**, not a fresh start. The interrupted owner's uncommitted
work is preserved in place (no revert, no re-scaffold). The review this report
answers is [`/tmp/sk-rust-install-review.md`](/tmp/sk-rust-install-review.md)
(findings F0–F16), read in full together with `AGENTS.md`, the charter and
`docs/rust/{DECISIONS,LOOP,VERIFICATION}.md`.

## Coordinator recovery addendum (after the owner was cancelled)

The owner applied two further fixes before cancellation: a full asset set cannot
silently omit its activation preset, and first-install skill destinations use
`managed_skill_destination`. Fresh coordinator runs now cover **137 library +
7 agent-surface + 6 full-init baseline + 6 delivery + 20 lifecycle + 1 lock tests
= 177 passed**, with one parent-exercised helper ignored. The actual CLI delivery
suite passes **5 tests**, and `init_oracle_diff` passes **1** with the lock-exact
oracle explicitly supplied. Logs: `/tmp/sk-init-residual-verification.log` and
`/tmp/sk-cli-init-resume-verification.log`.

The old lock-contract test used external process-group signalling. Its exact
relationship to the harness cancellations is not proven. The coordinator
removed that dependency: an ignored native helper announces lock acquisition,
blocks on its stdin pipe, and is killed/reaped by direct child handle only.
The contention and release test now completes normally (0.11 s); no negative
PGID signal or external `flock` remains in the test.

These results supersede §3/§4/§6's stale missing-run statements and F9's
first-install gap, not the remaining concurrency, packaging, option coverage or
independent-review obligations. The body below is the preserved owner report;
a new independent review is in progress.

## 1. Honest status (owner report before the addendum)

| Surface | Status |
| --- | --- |
| `sk-install` library (init/assets/skills/identity/lifecycle/templates/acquisition/profiles/session) | **Green in fresh scoped runs** — see §3 |
| Full native `spec-kitty init` via `sk-cli` (F0) | **Wired, not yet certified end-to-end** — see §4 |
| Workspace aggregate / Clippy / fmt / release / timing | **Not run** (out of this task's remit; no claim) |
| Commit/push/PR | **Not done** (this tranche is uncommitted work in a shared tree) |

No completion claim is made from partial direct-binary runs. Every number below
comes from a fresh `cargo test -p …` invocation under the shared Cargo lease,
with isolated `HOME`, `XDG_STATE_HOME` and `TMPDIR` (Cargo/rustup caches kept, as
in `VERIFICATION.md`).

## 2. Scope and ownership

Owned and preserved/finished here:

* `crates/sk-install/src/{init,skills,identity,lifecycle,template_tree,profiles,session,attributes,activation}.rs`
  (activation delegates to the doctrine write authority; no migration execution).
* `crates/sk-install/src/lock.rs` (state-dir lock path only).
* `crates/sk-install/tests/{init_delivery,full_init_baseline,init_lifecycle}.rs`
  (new/updated init tests).
* `crates/sk-cli/src/compat/init.rs` and the new CLI init tests
  `crates/sk-cli/tests/{init_delivery,init_oracle_diff}.rs`.

Explicitly **not touched** (other owners):

* `crates/sk-install/src/{upgrade,version,error}.rs`, `src/migrations/**`,
  ledger semantics owned by the coordinator.
* Workspace manifests, `Cargo.lock`, `crates/sk-cli/src/compat/{mod,capabilities}.rs`,
  `crates/sk-cli/tests/compat.rs`, `sk-doctrine`, `sk-status`, `sk-run-store`,
  `sk-runtime`, `sk-workspace`, `sk-git`, benchmarks.

## 3. Evidence: fresh scoped `sk-install` runs

All runs: `flock /tmp/spec-kitty-rust-cargo.lock env CARGO_BUILD_JOBS=2
CARGO_INCREMENTAL=0 cargo test -p sk-install --locked …`, temp `HOME`/XDG/TMP.

| Targets | Result | Log |
| --- | --- | --- |
| `--lib` | **136 passed, 0 failed** | `/tmp/sk-init-scoped-1.log` |
| `--test full_init_baseline` | **6 passed, 0 failed** | `/tmp/sk-init-scoped-1.log` |
| `--test init_delivery` | **5 passed, 0 failed** | `/tmp/sk-init-scoped-1.log` |
| `--test init_lifecycle` | **20 passed, 0 failed** | `/tmp/sk-init-scoped-1.log` |
| `--test agent_surfaces` | **7 passed, 0 failed** | `/tmp/sk-init-scoped-2.log` |
| `--test ledger_lock_interop` | **1 passed, 0 failed, 1 subprocess helper ignored** (real Python/native contention) | `/tmp/sk-init-scoped-2.log` |
| `--test manifest_contract` | **9 passed, 0 failed** | `/tmp/sk-init-scoped-4.log` |
| `--test plan_contract` | **7 passed, 0 failed** | `/tmp/sk-init-scoped-4.log` |
| `--test render_families` | **11 passed, 0 failed** | `/tmp/sk-init-scoped-4.log` |
| `--test pack_acquisition` | **14 passed, 0 failed** | `/tmp/sk-init-scoped-4.log` |
| `--test lock_contract` | **Not completed** (see §6) | `/tmp/sk-init-lock-contract.log` |

The `init_delivery` suite exercises the real checkout assets (template tree,
charter skills, profile catalog, built-in preset) in temp projects with temp
`HOME`, and covers fresh delivery, required-failure/keep-pending, resume of
every missing required effect, refresh without re-scaffolding, user-content
preservation, and the 8 proven clone-local ledger records inside a git repo.

## 4. CLI end-to-end status (F0)

`crates/sk-cli/src/compat/init.rs` is now the **full** delivery path: it
discovers the verified asset set (`ProjectAssets::discover`) and calls
`initialize_with_details` with the core version, environment and lock budget.
It refuses a build that cannot locate the assets instead of silently
scaffolding less. The new tests `crates/sk-cli/tests/init_delivery.rs` drive the
real binary in isolated temp projects, and `init_oracle_diff.rs` compares the
complete post-state (every file, every byte, with timestamp/UUID/ULID
normalization and the documented metadata/provenance and skill-order
exceptions) against the pinned Python oracle when
`SPEC_KITTY_ORACLE_PYTHON` is set.

**However, these two test targets could not be compiled in this session.** The
workspace is being rewritten concurrently; at 11:53 the build failed in a
foreign crate:

```
crates/sk-run-store/src/query.rs:71:18 error[E0425]: cannot find type `RunState` in this scope
```

(`/tmp/sk-cli-init-scoped-1.log`). `sk-run-store` is outside this task's
ownership, and the owner is actively editing it, so it was not patched here.
Subsequent attempts waited on the shared Cargo lease and were cut before the
build completed; another agent's `cargo test -p sk-install` held the lease at
the time of writing. **The CLI init surface is therefore wired but not
certified by a fresh run, and no parity claim is made for it.** It must be
re-run by whoever owns the next green workspace state.

## 5. Review remediation (what the preserved work now does)

* **F0 — CLI reachability.** `compat/init.rs` calls
  `initialize_with_details` with `ProjectAssets`; asset discovery is explicit
  (checkout offering templates/skills, pack missions/presets/profiles, optional
  `SPEC_KITTY_PACKS_ROOT`), and absence is a refusal.
* **F1 — recovery no longer lies.** One `run_delivery` performs the whole
  sequence for fresh, `ResumeFresh`, `Resume` and `Refresh`; template tree is
  re-installed when absent, and skills/profiles/session/gitignore/attributes/
  activation/identity/event/ledger are all completed idempotently on resume and
  refresh. The pending record is written before the first managed byte and
  cleared only after the whole delivery is durable. Tests:
  `a_required_failure_keeps_the_pending_record_and_resume_completes_it`,
  `resume_restores_every_missing_required_effect`,
  `refresh_completes_missing_effects_without_re_scaffolding`.
* **F2 — manifest paths are validated.** Every skills-manifest path is checked
  by `manifest::validate_relative_posix` **and** `sk_kernel::resolve_within`
  (absolute/traversal/symlink escape refused) on load and before every restore
  write (`skills::managed_skill_destination`). Profile manifest paths get the
  same validation. Test: `skills`/`profiles` unsafe-path unit tests.
* **F3 — ledger lock directory.** `ledger_lock_path` uses the shared kernel
  state-directory resolver (`~/.cache` fallback), matching the pinned writer;
  the cross-language contention test (`ledger_lock_interop`) passes for unset,
  empty and relative XDG state. (Coordinator-owned code; verified here, not
  modified.)
* **F4 — `ProjectInitialized`.** `lifecycle::emit_project_initialized`
  delegates to the canonical `sk-status` lifecycle writer (same
  `project.status.lock`, Python-compatible JSON, `...Z` payload timestamp,
  readback) instead of a second writer. The oracle post-state comparison in
  `init_oracle_diff` covers the event bytes when it can run.
* **F5 — seeded `superseded_by`.** `LedgerRecord::for_spec` no longer copies
  the catalog's supersession note; only the runner stamps an outcome.
* **F6 — `memory/` installed.** `TemplateTreeSource::discover` resolves the
  checkout's `.kittify/memory` (the absent `src/specify_cli/memory` path is no
  longer used), and the delivery asserts `memory/README.md` plus operator
  files surviving refresh.
* **F7 — skills provenance.** `SkillInstallOptions::with_version` stamps
  `spec_kitty_version` from the running CLI version on fresh install and
  refresh; the install and CLI tests assert `4.0.0rc8`.
* **F8 — clone-local ledger set.** `clone_local_effect_in_place` recognises the
  8 effects the oracle settles at init (including
  `4.0.0rc8_retire_antigravity_workflows` and the merge-driver rows) and
  `MergeDriverWiringPlan::apply_git_config` writes the real clone-local
  `merge.<key>.*` config (keeping operator values, reporting customisations).
  Result: 124 tracked + 8 clone-local in a git checkout, 124 outside.
* **F10 — lock residue.** `ProjectLock::for_project` uses a state-dir lock keyed
  by the resolved project root; no `.kittify/.install.lock` is created, and the
  tests assert its absence.
* **F11 — config order.** `write_fresh_config` writes `vcs` before `agents`, so
  `ensure_identity` yields upstream's `project` / `vcs` / `agents` order.
* **F12 — `.claudeignore`.** The copy is gated on
  `TemplateTreeSource::with_package_mode`; a checkout-mode init does not create
  it (unit test covers both modes).
* **F9 (partial).** Profile destinations, orientation/settings files and
  manifest-controlled skill restores refuse symlinks/escapes via
  `resolve_within`. The catalog-driven first write in
  `skills::install_managed_file` still joins a catalog-derived path under the
  project root and inspects only the final component; a symlinked agent root
  can therefore still escape on the *initial* install. Tracked as an open gap
  (§6), not claimed fixed.
* **F13 (partial).** The new CLI oracle test compares the full file set and
  bytes (including `agent_key` in a full manifest multiset), which closes most
  of the weak-evidence finding; the older crate fixture test still compares
  sets/substrings and is retained as an asset-level baseline.
* **F14/F15/F16 — open/deferred**, see §6.

## 6. Open gates and honest gaps

1. **CLI init not yet certified** (§4): blocked by the concurrent
   `sk-run-store` rewrite, not by this crate. Needs a fresh
   `cargo test -p sk-cli --locked --test init_delivery --test init_oracle_diff`
   with `SPEC_KITTY_ORACLE_PYTHON` once the workspace compiles.
2. **`lock_contract` not completed.** Three attempts were cut while the
   process-group lock-contention test was executing (the harness terminates the
   turn when the test's `kill -TERM -<pgid>` runs); no failure was observed and
   no pass is claimed. Run it outside this harness or accept it as unverified.
3. **F9 initial-install symlink ancestor** (above). Fix belongs in
   `skills::install_managed_file` (route the destination through
   `managed_skill_destination`).
4. **F14 locking breadth.** Only `InstallPlan::commit`, identity, event and
   ledger take locks; skills/profile/session writes are separate critical
   sections. Two concurrent inits can still interleave those surfaces.
5. **F15 transport hardening.** `HttpsArchiveTransport` still lacks
   `--proto-redir =https` (integrity is digest-pinned; no credential at risk).
6. **F16 wording.** `crates/sk-install/README.md:135` still says
   "deterministic ULID event id"; the ULID is time+random (only the rest of the
   event is deterministic).
7. **Documented environment gap.** `metadata.yaml` records an empty
   `python_version` and the native platform string, not the oracle's
   interpreter provenance. The oracle diff strips only that block and asserts
   everything else byte-for-byte.
8. **Scope fences unchanged.** 90 of 136 upgrade migrations, transports,
   global-runtime provisioning, profile/context doctrine content (another
   owner), and full governed `next`/accept/merge flows remain outside this
   tranche. No newer-upstream parity is claimed: the reference is
   `9aed3a80`, fork `main` is at `8ffeb018` and is not merged.
9. **Capabilities matrix.** `crates/sk-cli/src/compat/capabilities.rs` still
   lists `init` as `partial`. That file is coordinator-owned; the init evidence
   string should be updated only after the CLI test above is green.

## 7. API additions (exports in `sk-install/src/lib.rs`)

New public surface used by the CLI and tests (additive; no existing signature
was narrowed): `ProjectAssets` (with `discover`, `with_*` builders and
`missions_pin`/`environment`), `InitRequest::with_project_assets` /
`with_core_version` / `with_environment`, `initialize_with_details`,
`InitDetails`, `InitOutcome::ResumedDelivery`, `PendingDelivery`
(load/write/clear), `ProjectLock::for_project` + `project_lock_path`,
`MergeDriverWiringPlan::apply_git_config` + `MergeDriverWiringState`/
`MergeDriverCustomization`, `profiles::*` (catalog, formats, renderers,
manifest, `project_agent_profiles`), `session::*`
(`apply_session_presence`/`apply_healthy_orientation`, orientation constants),
`lifecycle::now_utc_z`, `SkillInstallOptions::with_version`, and
`template_tree::{discover_pack_missions,discover_pack_presets,
discover_pack_agent_profiles}`.

## 8. Proposed coordinator wiring / follow-ups

* No `compat/mod.rs` edit is needed for routing: `pub mod init` and the
  `Command::Init` dispatch already exist (coordinator-owned). `compat/init.rs`
  only consumes `super::CORE_VERSION` (`4.0.0rc8`, distinct from package
  `0.1.0`).
* After the CLI tests are green, update the `init` row in
  `compat/capabilities.rs` (evidence: full template tree + charter skills +
  profiles + session + activation + identity + `ProjectInitialized` + ledger;
  gaps: F9/F14/F15 and the remaining upgrade/transport/global-runtime surface).
* Sequence the next workspace check after the `sk-run-store` owner lands; the
  current tree cannot produce a CLI certificate.
