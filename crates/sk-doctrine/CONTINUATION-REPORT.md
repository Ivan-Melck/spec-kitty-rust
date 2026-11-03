# sk-doctrine continuation report

**Scope owner:** `crates/sk-doctrine/**` + `crates/sk-install/src/activation.rs`
**Checkpoint started:** `ff4452b02` (after the parent's YAML/anti-pattern fixes)
**Reference baseline:** upstream `9aed3a8012cb5eb22c0128b14d525039f95fea7a`
**Oracle:** `.venv-rust-oracle/bin/python` 3.11.15, `PYTHONPATH=src`,
`SPEC_KITTY_PACKS_ROOT=$PWD/packs`, verified by `scripts/rust/verify_oracle.py`
(`{"ok": true, ...}`).

No commits, pushes, Cargo/manifest/lock edits, workspace/release builds, timing
campaigns, or edits outside the owned paths. The only produced artifacts are the
owned source, its README, the new fixture/support files, and this report.

## What was implemented (behaviour, not DTO scaffolding)

### 1. One authoritative activation transaction (plan observation → commit)

`crates/sk-doctrine/src/plan.rs`, `src/yaml.rs`

- `ActivationObservation::observe(repo_root)` captures the activation authority's
  exact bytes (ledger or the legacy target, resolved through the one source
  resolver) at plan time.
- `ActivationTransaction::{begin,observation,ledger,commit}` is the endorsed
  write path. `commit`:
  1. prepares the write through the deny guard,
  2. refuses if the resolved target changed,
  3. takes the canonical synthesis lock,
  4. rechecks the plan-time observation **inside** the lock,
  5. writes atomically, verifies, restores on failure.
  A concurrent writer therefore causes `precondition_changed`, never a silent
  overwrite. This closes the reproduced loss where `commit_plan` re-applied a
  stale full post-state over another writer's `[a, c]`.
- `PreparedWrite::apply_observed` exposes the same guard for callers holding a
  prepared write; `PreparedWrite::apply` keeps its old signature and semantics.
- `commit_plan(repo_root, plan)` still works (compatibility), but now uses the
  canonical synthesis lock. New code should use the transaction.
- `LockScope` on `PreparedWrite` selects the lock family; activation and legacy
  charter writes use `Synthesis`.
- The parent's empty/null/comment-only/scalar YAML regressions are retained
  (tests still cover `""`, `null`, `~ # comment`, `# note`, scalar roots and
  sequence roots).

### 2. Canonical Python synthesis lock authority + real contention

`crates/sk-doctrine/src/lock.rs`

- `synthesis_lock_path` mirrors `charter.activation.synthesizer.synthesis_lock`:
  `<git-common-dir>/spec-kitty-locks/charter-synthesis/<sha256[0..32]>.lock`
  through `sk_kernel::anchored_lock_path`, else
  `<root>/.kittify/runtime/locks/charter-synthesis.lock`; a git probe that cannot
  run is `SynthesisLockUnavailable`, never a different home.
- `acquire_synthesis_lock` is same-thread reentrant and maps a timeout to
  `SynthesisLockHeld` (nothing written).
- **Actual cross-language test** (`tests/governance_oracle.rs`): a Python child
  holds `synthesis_lock(repo)`, and:
  - Python and Rust compute the **same** absolute lock path;
  - native acquisition times out with `SynthesisLockHeld`;
  - a native `ActivationTransaction::commit` refuses while Python holds it;
  - after release the commit succeeds and the ledger readback is correct.

### 3. Installer delegates to the writer

`crates/sk-install/src/activation.rs`

- `provision_mission_type_activations` now begins an `ActivationTransaction`,
  reads the key through it, and commits one plan. It holds the same lock as
  Python, refuses a concurrent change, and preserves comments.
- `resolve_activation_target` uses `sk_doctrine::resolve_activation_source` /
  `activation_write_target`, so a pointed-at legacy `charter.yaml` and an
  unmigrated `config.yaml` resolve exactly as every other activation writer
  resolves them.
- Preserved semantics: copy the shipped preset verbatim, never create a ledger
  beside legacy keys, never rewrite an authored `[]`/list, fail closed with
  `DEFAULT_PRESET_MISSING`.
- `sk-install` already declared the `sk-doctrine` dependency, so **no manifest
  edit was needed**.

### 4. Required-floor / org-charter import and effective deny throughout cascade

`crates/sk-doctrine/src/org_charter.rs` (new), `src/ledger.rs`,
`src/activation.rs`, `src/cascade.rs`

- Strict `org-charter.yaml` import: schema 1–2, known-field allowlist,
  `required_*` list shapes, `interview_defaults`, `governance_policies`,
  `activations`, `extends:` chains (declared-first, nearest-base order;
  cycle/dangling-base refusals; strict declared-pack existence).
- Floor readers: raw required tokens, normalized stems, provenance
  (`'base'` / `'base' (via 'overlay')`), `required_floor`, unresolved-id report,
  required-directive revocation check, org `expected-artifacts.yaml` /
  `requirement-kinds.yaml` (whole-file, last matching org root wins).
- `deactivated_urns_with_floor` and `ActivationState::deactivated_urns` are
  floor-netted; `cascade_activation_targets_for_state` and
  `referenced_but_not_cascaded_for_state` use that effective deny. A stale deny
  of an org-required artifact no longer stops a cascade (unit test).
- `check_deactivation_permitted` now produces the reference refusal text with
  the real requiring packs instead of the previous dead-code path.

### 5. Reference token normalization and layer precedence

`crates/sk-doctrine/src/identity.rs`

- Directive normalization (`1`/`001`/`directive:1` → `DIRECTIVE_001`, slug ids →
  upper snake) is applied to lookups and `stem_for_declared_id`.
- Stem-first across **all** layers, then declared id, then normalized id; layer
  order project → org packs last-to-first → built-in. A shared stem now resolves
  by precedence (the reproduced `AmbiguousIdentity` hard failure that broke valid
  project activations is gone). `declared_id_in_layer` answers per-layer
  questions; `stored_spelling` raises `ActivationIdCollision` only when a
  declared id has no representable stem.
- Added the reference helpers used by resolution: `selection_id`,
  `has_declared_id`, `names_a_file`, `stem_for_declared_id`, `lookup_key`,
  `activation_id`, `source_layer`, `IdentityLayer`.
- `ActivationState::from_ledger` keeps unknown allow tokens visible and
  anti-pattern tokens as graph-only ids (parent regression retained).

### 6. Same-id deep profile merging, exclusion, schema and language rules

`crates/sk-doctrine/src/profile.rs`

- A same-id org/project profile is deep-merged **into** its built-in base
  (nested mappings recursively; scalars/lists replaced), then validated, so a
  partial overlay keeps built-in fields (the reproduced `name=None`,
  `capabilities=0` loss is fixed).
- Schema validation mirrors the reference load contract: required
  `profile-id`/`name`/`purpose`/`roles` (or `role`, never both)/
  `specialization.primary-focus`, `extra="forbid"` key surface, routing-priority
  and max-concurrent-task bounds. Invalid files are **skipped and recorded**
  (with the file and reason) instead of blanking the layer; a missing
  `profile-id` is no longer renamed after the file stem.
- `applies_to_languages` filtering with family expansion (`java`/`kotlin` →
  `jvm`), reserved `unknown`, `any`/`all` sentinels, and
  `scope_filtered_ids()` recording.
- The resolving leaf's `excluding` (list of fields or mapping of field → values)
  is applied to the merged body, exactly as `repository._apply_excluding`.
- Lineage still comes from DRG `specializes_from` edges with
  missing-parent/cycle/ambiguous-parent refusals, root→leaf union of the five
  list fields and one-level nested mapping merge.

### 7. Exact project-reference relations

`crates/sk-doctrine/src/org.rs`

- `project_reference_edges` now matches the extractor:
  - `directive-references[].code` → `requires`, **no** reason;
  - `tactic-references[].id` → `requires` + authored `rationale`;
  - `styleguide-references[].id` → `suggests` + rationale;
  - `toolguide-references[].id` → `suggests` + rationale;
  - `collaboration.operating-procedures[]` → `requires`;
  - the invented `procedure-references` family is removed.
  This restores eager-vs-link delivery, cascade closure and profile-channel
  semantics.
- `scan_project_artifacts` validates each file against its kind's reference
  schema (required + allowed key surface) and refuses a symlink that escapes the
  project pack root. Reproduced cases now behave like the reference:
  `id: 001-arch` alone is refused, a symlink to `/tmp/outside-directive.yaml`
  is refused.

### 8. Real nested action-context rendering

`crates/sk-doctrine/src/context.rs`

- Entry shape is exactly `{id, source, title?, summary?, references[], delivery,
  body?}`; the URN/kind/`when` are API-only (`#[serde(skip)]`), and
  `references[]` is `{id, relation, when, reason}` with `null`s where the
  reference emits them.
- `delivery` is `inline` for the `requires` closure of the action + selected
  roots, `link` for `suggests`-reached artifacts, and everything inline under
  `--include-all`. `body` is the real artifact document (null-stripped), not a
  path or a fabricated stub.
- Selected project doctrine widens delivery: activated + org-required
  directives/tactics/paradigms seed a `requires`/`suggests` closure (with the
  reference ownership gate against another action's `scope`), which is what
  makes activated styleguides/procedures appear exactly as Python does.
- Top-level `references` is the complete link set (`{id, relation, when,
  reason}`), with the stated default `when` for uncovered `suggests` edges.
- `all_directives` + `directives_source` come from one resolution:
  three-state activation base (`catalog_fallback`/`activation`), charter
  `selected_directives` (`+charter`), project-local `directives:` (`+project_local`),
  unknown selections dropped with a diagnostic, effective deny applied last.
- `project_charter` always carries `present`/`path`/`charter_md_present`/
  `charter_md_path`, plus `bytes` and `metadata.yaml` fields when present.
- `org_charter` is the imported policy block (`org_charter_block`).
- `governance_references` is the per-path status list (including
  `warning: null` for a clean path).
- Bare actions: a bootstrap action (`specify`/`plan`/`implement`/`review`) still
  produces a bootstrap payload with governance blocks when it has no unique
  action node (the reproduced `review` ambiguity failure); non-bootstrap
  actions keep the strict unique-node refusal.
- `org.rs` bodies and all nested entries are oracle-compared (below).

### 9. Merge/lint compatibility

`crates/sk-doctrine/src/merge.rs`

- Forward references are a **warning by default** (the reference
  `_warn_dangling_org_endpoints` / `charter lint` behaviour); the hard refusal is
  now the opt-in `allow_forward_references: false` for callers that own graph
  completeness.
- `MergeConflict` carries `built_in_value`, `org_value`, `project_value` and the
  reference resolution vocabulary; restated built-in/org edges record
  `edge_override`; project overrides use `project_wins`.

### 10. Smaller correctness fixes retained/added

- `ActivationLedger::raw()` keeps unknown `deactivated_<kind>` keys **and their
  values** (round-trip test).
- `filter_graph_by_activation` no longer falls back to the unfiltered graph; the
  fallible variant returns an error and the infallible one fails closed.
- `check_deactivation_permitted` and `RequiringPack` match the reference shape.
- `org::project_layer` / `scan_project_artifacts` schema+containment (above).

## Evidence

All commands run in this checkout with `CARGO_BUILD_JOBS=2
CARGO_INCREMENTAL=0`, the existing `target/`, and temporary project/HOME/XDG/TMP
roots for every fixture.

| Gate | Command | Result |
| --- | --- | --- |
| Doctrine tests | `cargo test -p sk-doctrine --locked` | **123 passed**: 100 unit, 18 baseline conformance, 3 oracle fixtures, 2 doctests |
| Doctrine clippy | `cargo clippy -p sk-doctrine --all-targets --locked -- -D warnings` | pass |
| Doctrine fmt | `cargo fmt -p sk-doctrine -p sk-install -- --check` | pass |
| Oracle verification | `.venv-rust-oracle/bin/python scripts/rust/verify_oracle.py --root .` | `ok: true`, 53 requirements checked |
| Installer activation | `cargo test -p sk-install --locked activation` | 7 passed |
| Installer clippy | `cargo clippy -p sk-install --all-targets --locked` | 0 errors; 2 warnings in `sk-install/src/init.rs` (other owner) + 1 in `sk-status` (other owner); none in `activation.rs` |

Oracle-generated fixtures (`crates/sk-doctrine/tests/governance_oracle.rs`, with
`tests/support/mod.rs`):

1. `nested_context_matches_the_oracle_generated_fixture` — builds a temporary
   git project from the repository's `packs/built-in` plus an org pack
   (`org-charter.yaml` with `required_directives`, a policy, an `extends`-free
   `drg/fragment.yaml`), a ledger with activation + a stale deny, a charter with
   `selected_directives`, a project-local `directives:` section and
   `governance_references`. It then compares against
   `build_charter_context_json` (action `implement`, mission type
   `software-dev`, first-load depth 2):
   - `context_schema_version`, `action`, `mode`, `directives_source`
     (`activation+charter+project_local`);
   - `all_directives` entry-for-entry;
   - every typed array's ids, entry key set, `source`, `title`, `summary`,
     `delivery`, real `body.id`, and sorted `references[]`;
   - every oracle top-level link is present natively (native-only links are
     allowed only for a root the native set reaches; nothing oracle-named is
     dropped);
   - `project_charter` exactly (four core keys + `bytes`);
   - `governance_references` exactly (including `warning: null` and the
     missing-file warning);
   - `org_charter` exactly against `load_org_charter_json_block`.
2. `synthesis_lock_path_and_contention_match_python` — path equality and the
   live Python-holder contention sequence described above.
3. `the_oracle_is_verified_and_the_fixture_is_not_vacuous` — fails if the oracle
   is missing/drifted and asserts the fixture actually produced directives,
   tactics and `1.4.0`.

The fixture is not key-count based: a contract drift in the nested entry shape,
delivery cadence, body, `directives_source`, `project_charter`, `org_charter`,
`governance_references`, or the link set reddens the test.

## API changes (all additive except where noted)

- New: `ActivationObservation`, `ActivationTransaction`,
  `commit_plan_observed`, `activation_write_target`.
- New: `synthesis_lock_path`, `acquire_synthesis_lock`,
  `SYNTHESIS_LOCK_SUBDIRECTORY`, `LockScope`, `observe`.
- New: whole `org_charter` module exports (`load_org_charter_policy`,
  `required_floor`, `org_required_stems`, `read_org_required_provenance`,
  `read_org_required_tokens`, `resolve_org_expected_artifacts`,
  `resolve_org_requirement_kinds`, `org_charter_documents`,
  `declared_policies`, `configured_org_roots`, `validate_org_required_directive_stems`,
  `org_charter_block`).
- New: `deactivated_urns_with_floor`, `try_filter_graph_by_activation`,
  `ActivationState::deactivated_urns`, `cascade_activation_targets_for_state`,
  `referenced_but_not_cascaded_for_state`.
- New: `IdentityLayer`, identity helpers listed in §5.
- New: `ProfileLayer`, `SkippedProfile`, `applies_to_languages_match`,
  `normalize_languages`, `ProfileRepository::{skipped, scope_filtered_ids,
  layer_of, provenance, load_with_languages}`.
- New context types: `Delivery`, `EdgeReference`, `DirectiveEntry`,
  `OrgCharterPack`, `OrgGovernancePolicy`, `GovernanceReferenceStatus`,
  `STATED_DEFAULT_WHEN`.
- Changed shape (no other crate consumed them): `MergeConflict` gained
  `built_in_value`/`org_value`/`project_value`; `MergeOptions` default is now
  `allow_forward_references: true`; `ArtifactEntry`/`CharterContext.references`
  follow the reference nested contract; `ProfileRepository::load` now skips and
  records invalid profiles instead of aborting the layer (reference behaviour).

## Cross-owner notes / proposed integrations (not edited)

1. **`crates/sk-install` is already wired to `sk-doctrine`** in its manifest;
   `src/activation.rs` now uses it. No further integration needed.
2. **Blocked aggregate clippy:** `cargo clippy -p sk-install --all-targets
   --locked -- -D warnings` currently fails in another owner's crate:
   `crates/sk-status/src/mutation_rollback.rs:288` `clippy::needless_borrow`
   (`require_hold(lock, &point.events_path.parent().unwrap_or(...))`). Removing
   that `&` clears it. I did not edit it.
3. **`sk-install/src/init.rs` clippy warnings** (other owner):
   `clippy::question_mark` at line 993 and `clippy::collapsible_if` at line
   1016. Not edited.
4. **Coordinator wiring proposal:** the CLI can now replace any direct ledger
   write with
   `ActivationTransaction::begin(repo)? -> ledger() -> plan_* -> commit(&plan)`;
   activation writes no longer need a private sidecar lock. `charter context`
   can call `build_action_context` directly (it already returns the nested
   contract); `charter list`/`doctor` can read the floor from
   `required_floor(repo, &identity)`.

## Genuinely unfinished obligations

These remain out of scope for this tranche and are stated, not disguised:

1. Mission-type profiles / `actions/*/index.yaml` grains.
2. Project-governance breadth beyond directives (templates/tools/paradigm
   selection, `template_set`, interview/charter.md overlays are not emitted).
3. Text `charter context` renderers, compact-mode prose, token-budget/template
   includes, and the activation render block.
4. Interview → synthesis → compiler → manifest stamping.
5. Pack fetch/pin/preset/sync/versioning lifecycle.
6. Ledger migration / legacy rename writer / bundle restamp.
7. `replaceable-builtins.yaml` adjudication, asset-sidecar escape diagnostics,
   org governance-profile revocation completeness, topology schema validation.
8. Project-charter canonical bundle-root refresh (the block is exact for the
   in-tree bundle only).
9. Stale-lock cleanup and Windows lock-identity equivalence (kernel contract).
10. YAML reorder comment reattachment and alias/anchor edge cases.
11. Full cross-platform (`macOS`/`Windows`) verification: only Linux was run
    here, and no workspace/release/benchmark campaign was run by this owner.

No claim of full-charter parity or performance is made.
