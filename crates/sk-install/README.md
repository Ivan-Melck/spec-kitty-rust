# sk-install

Native project **initialization**, **managed-asset installation**, **pinned pack
acquisition** and the **upgrade/migration substrate** for the Rust Spec Kitty
implementation.

This crate replaces the Python `spec-kitty init` delivery path, the command and
charter skill installers, the pinned-pack pipeline, the additive project
metadata/identity writers and the ledger-driven `spec-kitty upgrade` selection.
The Python tree at the pinned reference snapshot is a **reference and test
oracle only**; no production path spawns an interpreter.

```text
init ─┬─ template tree (verified, backed up)          ─┐
      ├─ command surfaces (typed plan → commit)        │
      ├─ charter skills + skills-manifest             │  full delivery
      ├─ .gitignore / .gitattributes                  │  (ProjectAssets)
      ├─ activation + metadata + identity + event     │
      ├─ migration-ledger seed                        │
      ├─ Vibe pointer + surface repair                ─┘
      └─ upgrade: catalog-diff selection → ledger → metadata

pin ─▶ pack(transport → verify → cache → place) ◀─ bounded archive extraction
```

## Public API

### Initialization

```rust
use sk_install::{InitRequest, ProjectAssets, TemplateSource, initialize};

// Legacy/compat path: command files + config/metadata core only.
let request = InitRequest::new(project_root, vec!["claude".into()], "4.0.0rc8")
    .with_templates(TemplateSource::from_pack_root(pack_root));

// Full delivery: explicit verified assets.
let assets = ProjectAssets::discover(checkout_root)?; // templates, missions, skills, preset
let request = InitRequest::new(project_root, vec!["claude".into(), "codex".into()], "4.0.0rc8")
    .with_project_assets(assets)
    .with_core_version("4.0.0rc8");
let (report, details) = sk_install::initialize_with_details(&request)?;
```

`initialize` keeps its previous return type; `initialize_with_details` adds
`InitDetails` (template mode/copies/backups, skill counts, gitignore/attributes,
merge-driver plan, activation, identity, event, ledger seed, surface repair,
scratch diagnostics, security notice). `InitOutcome` is unchanged
(`Initialized`, `AlreadyInitialized`, `ResumedDelivery`).

Fresh full init holds the canonical per-project state-directory lock before
reading pending/config state and choosing fresh, refresh or resume. It preflights
the activation preset only when the activation key is absent, before managed
writes. Delivery then follows the pinned upstream surface:

1. write the pending-delivery record (the command-skill roster in full mode);
2. install the template tree, moving operator `memory/`, `templates/` and
   `missions/` to `.kittify/.backup-<UTC>/` before writing;
3. write `config.yaml` and `.kittify/memory/`;
4. plan/commit command surfaces and the ownership manifest;
5. install the charter skill catalog (native/shared roots, `skills-manifest.json`),
   session orientation and agent-profile projections in the source-defined order;
6. protect runtime/agent paths in `.gitignore`;
7. seed `.gitattributes` and expose the clone-local merge-driver git-config plan;
8. provision mission-type activations from the built-in `default` preset;
9. save/stamp `metadata.yaml` additively (comments preserved);
10. mint/persist project identity and append `ProjectInitialized`;
11. seed the migration ledger (tracked + proven clone-local effects);
12. restore the Vibe pointer and run managed-skill surface repair;
13. remove only proven run-created scratch; clear the pending record last.

Refresh never re-scaffolds: it verifies/repairs the manifest-owned command
surface, restores missing native/shared skill files, refreshes the agent-files
block without creating a missing `.gitignore`, restores a lost Vibe pointer, and
returns the clone-local merge-driver plan. A pending record resumes exactly its
command-skill set and restores the configured agents' skill surface.

### Managed assets

```rust
let plan = InstallPlan::plan(project_root, version, &rendered, &PlanOptions::new())?;
plan.validate()?;
let report = plan.commit(Duration::from_secs(10))?;
```

Actions: `Create`, `Update`, `Adopt`, `Keep`, `Regenerate`, `Delete`,
`Preserve`, `Collision`. The manifest wire format is unchanged
(`schema_version: 1`, sorted entries/keys, two-space indent, trailing newline,
lowercase SHA-256). A commit re-checks every observed precondition under the
project lock. An `AtomicCommitIndeterminate` outcome is read back: observed new
bytes count as written (with a durability warning); anything else is a typed
`IndeterminateCommit` that must be recovered, never blindly retried or deleted.

### Charter skills

```rust
let catalog = SkillCatalog::discover(skills_root)?;      // explicit, verified
let report = install_skills_for_agent(root, "claude", &catalog, &options)?;
let summary = run_surface_repair(root, &agents, &catalog, /*repair_drift=*/false)?;
```

`skills-manifest.json` matches upstream: version 1, upstream field/entry order,
provenance defaults omitted, duplicate `(installed_path, agent_key)` owners
rejected, created/installed timestamps retained on a no-op save. Shared-root
`SKILL.md` files get the Agent Skills frontmatter projection (non-spec keys
moved under `metadata`), so shared and native roots hash exactly as the oracle
does. An unowned/modified destination is never overwritten; missing managed
files are restored, drift is reported (and only replaced with `repair_drift`).

### Pinned packs

```rust
let pin = PackPin::load(&repo.join("packs/built-in.source.yaml"))?;
let acquirer = PackAcquirer::new(cache_root, Box::new(HttpsArchiveTransport::default()))
    .with_core_version("4.0.0rc8");
let report = acquirer.acquire(&pin, &repo.join("packs/built-in"), /*offline*/ false)?;
```

`requires_core` is now enforced (PEP 440, prereleases admitted) before cache
verification/placement; `check_requires_core`, `PackPin::requires_core_satisfied`
and `enforce_requires_core` are the reusable predicates. Digests, staging,
bounded extraction, swap recovery and the transports are unchanged.

### Metadata, identity and the project event

* `ProjectMetadata::load/save` merges only model-owned fields
  (`version`, `initialized_at`, `last_upgraded_at`, an existing
  `schema_version`, the environment triple); every other key, comment, quote and
  line ending survives. A masked compare (`last_upgraded_at` neutralized) makes
  a no-op save a no-op.
* `stamp_schema_metadata` inserts `schema_version` and
  `schema_capabilities` only when absent; an authored capability map is kept.
* `ensure_identity` merges into `config.yaml`'s `project:` mapping, preserving
  comments/operator keys; an unparseable UUID is refused rather than re-minted.
  `resolve_identity` never writes.
* `emit_project_initialized` delegates to the canonical `sk-status` lifecycle
  writer for `.kittify/canonical-events.jsonl` (no `O_APPEND`), using its
  project-event lock, validation, whole-file replacement and readback. It is
  idempotent on `project_uuid`; the contract uses `schema_version: 5.0.0`, sorted
  JSON keys and a time/random ULID, not a deterministic event id.

### Upgrade and the migration ledger

```rust
let plan = plan_upgrade(&UpgradeRequest::new(root, cli_version))?; // read-only
if plan.is_blocked() { /* unimplemented ids */ }
let report = run_upgrade(&UpgradeRequest::new(root, cli_version).with_target("4.0.0rc8"))?;
```

* **Selection is declared minus applied** per partition (tracked vs clone-local),
  capped by an optional `--target`; `detect()` only decides `apply` vs
  `not_needed`. Legacy constitution-era ids normalize on read.
* **The ledger is two append-only JSONL partitions**, written by locked
  whole-file atomic replacement (machine-state lock keyed by the resolved ledger
  path). Lines are byte-compatible with `json.dumps(record.to_dict())`;
  `clone_local`/`superseded_by` appear only when set. `seed_for_init` records all
  tracked migrations and only clone-local ones whose effect is provable.
* **Execution is fail-closed.** The embedded catalog has 136 declared
  migrations (`src/migration_catalog.json`, generated from the pinned Python
  snapshot by `tools/generate_migration_catalog.py`). The delivered continuation
  implements **46 natively; 90 remain absent**. A real run whose selection
  contains an unimplemented id refuses with
  `migration_not_implemented` **before the first write** and names every id; a
  dry run reports `would_apply`/`not_needed`/`would_skip_outside_git`/
  `unimplemented`. No migration is silently dropped.
* Version checks refuse a newer project (`project_version_newer`), an older
  target (`target_version_older`) and a newer schema (`schema_version_newer`).
* `AgentUpgradeChoice`/`plan_agent_check` are the pure `--agent-check` decision
  surface (`none`/`auto_upgrade`/`guidance`/`prompt`); no network call or nag
  cache write happens here.

### Git boundaries

The crate owns file-level `.gitattributes`/`.gitignore`/`.git/info/exclude`
edits it can prove safe. A linked worktree's common-dir exclude is resolved
through `sk_git::GitRepository::git_dirs()` (the Git owner's captured
repository state) and written by this crate's atomic writer, never by a second
git seam. A Git **config** write (merge-driver wiring), a git-remote-derived
slug and any `git fetch <sha>` transport are returned as typed plans
(`MergeDriverWiringPlan`, `GitDirectoryUnsupported`,
`GitOperationDelegated`) for the Git owner rather than duplicated as a second
subprocess seam. `sk-git` has no config/remote-write API yet.

## Compatibility actually checked

* **Full-init differential baseline**: `tests/full_init_baseline.rs` compares a
  full `claude,codex,vibe` init against
  `tests/fixtures/oracle_init_claude_codex_vibe.json` produced by the isolated
  Python oracle at the pinned snapshot: all 222 charter-skill manifest entries
  (paths/classes/agents/hashes), all 14 command-skill entries with hashes, the
  exact `.gitattributes` and activation/Vibe files, the runtime/agent
  `.gitignore` block, the 124 seeded ledger ids, and the schema/config stamp.
* **Migration catalog** ids/order/partition/git requirements/content hashes are
  generated from the pinned Python registry, not hand-written.
* **Manifest bytes** follow the upstream command-skills manifest shape.
* **Rendering** follows `template/asset_generator.py` and
  `skills/command_renderer.py`, including the `## User Input` rewrite, command
  invocation rewrite, `$ARGUMENTS` guard, startup upgrade-check block, and the
  SPDD/REASONS conditional-block renderer (markers stripped when active).
* **Skills** follow the current upstream surface: full init installs command
  skills for the command-skill roster (`codex`, `vibe`, `pi`, `letta`,
  `antigravity`) and no longer writes slash-command files; legacy init keeps the
  previous 12-family command-file behavior for API compatibility.
* **Repeats/interruption**: a second full init leaves skills/command manifests,
  `.gitignore`, `.gitattributes`, the event log and the ledger byte-identical; a
  pending record resumes and restores missing skill files; a partially recorded
  upgrade resumes only unrecorded migrations.
* **User content**: comments/quotes in `metadata.yaml` and `config.yaml`,
  operator YAML subtrees (backed up), unowned skill files, damaged `.gitignore`
  blocks, symlinked ignore/config files and encoded/zip/tar archive escapes are
  all fail-closed and preserved.
* **Locking**: a contended project lock maps to `LockUnavailable` (kernel
  `LockTimeout` included), not generic I/O.

## Remaining gaps to full `init`/`upgrade` parity

Deliberately visible, not stubbed:

1. **Only 46 of 136 migrations are native.** `upgrade` refuses the rest before
   mutating; there is no autocommit, worktree upgrade, doctor/config-sync
   repair, downgrade-confirmation flow or legacy `metadata.migrations` mirror
   retirement (the ledger adoption backfill is not ported).
2. **Profile/session breadth is not blanket agent parity.** Supported native
   projections and orientation blocks are installed; renderer-less agent sets
   do not acquire empty manifests. Optional symlinked surfaces are preserved and
   warned about rather than written through or made fatal. The expanded oracle
   comparison covers `claude,codex,vibe`, `gemini` and `copilot`, not every layout.
3. **Pack transports** remain `file://` and the `curl` HTTPS archive fetch.
   Artifactory AQL, ETag/If-None-Match, `ApiSource`, authenticated zip bundles
   and `GitSource` (`git fetch <sha>`) are not implemented; no native TLS client
   is available offline. `git`-sourced fetch/merge-driver config needs the Git
   owner.
4. **Global runtime mode** writes only the minimal project skeleton; it does not
   provision `~/.kittify` (`ensure_runtime`) or install global command skills.
5. **Template/skill sources must be explicit** (`ProjectAssets::discover` reads a
   checkout layout). The binary does not locate a bundled/installed template
   tree, and there is no fallback when a source is absent.
6. **Provenance honesty**: `python_version` is empty (no interpreter), and
   `platform_version` is `os-<kernel release>` rather than Python's
   `platform.platform()`. Project slug uses the directory name only (no git
   remote lookup).
7. **Activation target resolution is simplified**: only
   `charter/activation.yaml`, `charter/charter.yaml` and `config.yaml` are
   considered; the full charter pack/ledger/legacy-scope resolver is not ported.
   Pack-provided (non-built-in) skills and the staged pack-skill renderer are
   absent.
8. **Skill ownership policy is conservative**: unrecorded legacy dotted skills
   are never deleted, a collision is preserved and reported (not resolved), and
   global skill roots are untouched.
9. **Lifecycle breadth remains partial.** Init uses the canonical `sk-status`
   project-event writer and installs session presence. Fan-out, external service
   delivery and the remaining lifecycle command surface are not complete.
10. **`--agent-check` does not fetch a latest version or persist a nag record**;
    `--agent-choice` values are parsed/validated but no user-state file is
    written. The CLI supplies those inputs.
11. **Scratch/prover conservatism**: `.kittify/command-templates`, `.scratch`,
    `.resolved-*` and `.merged-*` are preserved unless the caller proves this
    run created them. Init/upgrade use the same machine-state project lock and
    do not leave `.kittify/.install.lock` in the checkout.
12. **Cross-platform CI**: verified on Linux only. Windows mode/mtime and macOS
    are not claimed.

## Dependencies

Declared only in this crate's manifest: `tar`, `flate2`, `zip` (deflate),
`getrandom` (UUIDv4) and the workspace `ulid`. No crypto/TLS or HTTP crate is
added; the HTTPS transport remains the bounded `curl` adapter. Locking uses
`sk-kernel::FileLock`.

## Verification

```sh
CARGO_BUILD_JOBS=2 cargo test -p sk-install
CARGO_BUILD_JOBS=2 cargo clippy -p sk-install --all-targets -- -D warnings
cargo fmt -p sk-install -- --check
```

The latest coordinator init-focused set passes **227 tests, 0 failures, 2
parent-exercised helpers**, plus **8 CLI delivery tests** and **1 oracle test
comparing three complete post-states**. These are scoped checks, not a full
installer/upgrade or workspace gate. See `INIT-REMEDIATION-REPORT.md` and
`docs/rust/VERIFICATION.md` at the repository root for commands and boundaries.
Covered behavior includes every renderer family and
placeholder, SPDD block rendering, determinism, every configured agent surface,
manifest corruption/duplication/round-tripping, command and charter skill
installation/hashes, shared-frontmatter projection, native-root restore,
drift reporting, collision preservation, full-init oracle parity, template-tree
copy/backup/global-runtime mode, scratch proving, `.gitignore` block repair and
symlink refusal, attribute seeding and merge-driver plans, metadata/identity
comment preservation, activation provisioning, project-event idempotence/torn
tails, ledger partition/seed/lock/legacy-id normalization, catalog integrity,
upgrade selection/refusal/resume/version/schema guards and `requires_core`.

The full-init baseline fixture was generated by the isolated Python oracle at
the pinned reference snapshot; regenerate the migration catalog with
`tools/generate_migration_catalog.py` (development only). Project fixtures use
`tempfile`, but library tests inherit the ambient machine-state directory: run
Cargo under temporary HOME/XDG/TMP roots while retaining Cargo/rustup caches,
and serialize the entire invocation including doctests. This isolation is a
harness property, not an intrinsic guarantee of every test. No live credentials,
global Git configuration changes or hosted service are required.
