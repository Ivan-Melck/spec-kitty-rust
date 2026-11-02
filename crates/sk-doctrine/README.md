# sk-doctrine

Native governance execution for the Spec Kitty Rust rewrite.

`sk-doctrine` owns the executable half of the charter/doctrine domain. It
consumes `sk-charter`'s read-only kind, config and pack APIs and `sk-kernel`'s
containment and atomic-write primitives; it never edits those crates and never
invokes Python at runtime.

Contract mirrors (reference only):

- `src/charter/offering/drg/{models,loader,merge,query,reachability,validator,org_pack_loader,project_scan}.py`
- `src/charter/activation/{activation_engine,activation_ledger,pack_context,drg_activation,cascade,context,context_contract,profile_resolution}.py`
- `src/charter/offering/agent_profiles/repository.py`, `src/kernel/activation_ledger_store.py`

## Implemented

### Typed graph and loading

- `NodeKind` (17), `Relation` (15), URN grammar, typed `DrgNode`/`DrgEdge`/`DrgGraph`
  with strict `deny_unknown_fields` deserialization and URN-prefix/kind checks.
- `load_graph`, `load_graph_str`, `load_graph_or_dir` (`graph.yaml`, otherwise
  sorted `*.graph.yaml`), `merge_layers` (additive edges; project may replace the
  label only, never the built-in kind).
- Strict top-level `UnknownTopLevelKey` and nested `UnknownField` errors instead
  of a silently degraded read.

### Three-layer merge and org packs

- `load_org_pack`: strict fragment schema, canonical plural-kind aliases
  (`mission_steps`, `mission_types`), discovered artifact nodes, augmentation
  projection (`enhances`/`overrides`/`specializes_from`), org
  `mission_types/*/governance-profile.yaml` scope edges, `body_path` containment
  and the layer-rule refusal. A missing `pack.yaml` level is a diagnostic, never
  an invented level.
- `merge_three_layers` / `merge_three_layers_with_options`: provenance
  (`built-in`, `org:<pack>`, `project`), later-org-pack-wins, built-in same-kind
  override with a conflict record, kind-drift refusal, one endpoint policy (local
  bare id → qualified URN → unique built-in id), unknown-relation refusal,
  ambiguous/unresolved endpoint refusal (forward references are opt-in and
  reported), duplicate `asset:`/`template:` URN refusal, deferred dangling-edge
  detection over the fully assembled graph.
- `merge_topology_artifact`: `overrides` replacement; `enhances` field merge that
  preserves the `steps`/`actions` backbone and refuses a dropped step or a
  stripped `inputs`/`outputs` contract.
- `project_layer` / `scan_project_artifacts` / `walk_project_agent_profile_nodes`:
  project profile/artifact discovery, fail-loud malformed profiles, profile
  reference projection with unresolved-target diagnostics.

### Traversal and reachability

- `walk_edges`, `deliver_action_context` (scope depth 1 → transitive `requires`
  → bounded `suggests`), `resolve_context`, glossary `vocabulary` edges and
  tension annotations (`in_tension_with` + `reconciles_tension`).
- `resolve_transitive_refs` with a total `NodeKind` bucket map and `unresolved`
  records; `action_channel_reachable`; the distinct
  `profile_channel_reachable` (`requires`/`specializes_from`/`suggests`, never
  `scope`).

### Activation state and mutations

- `ActivationLedger`: vocabulary derived from `ArtifactKind`, strict L1/L2 load,
  ledger-vs-legacy source resolution, stray/legacy deny reporting, three-state
  allow (`None`/`[]`/ids), identity-spaced `DenySet`, effective deny minus the
  required floor, `raw()` round-trip data.
- `plan_activation` / `plan_deactivation`: pure, validation before post-state,
  absent-key materialisation from what is actually in force, the "already in
  force ⇒ write nothing" rule, `--only` narrowing, deny exclusions, ordered
  required-floor and unknown-id refusals.
- `commit_plan` / `prepare_ledger_write`: one locked write, deny-change guard
  (`DenyListChoice`), unmigrated deny refusal, empty ledger rendering, and a
  lossless `YamlDocument` that preserves comments, quoting and key order.
  `PreparedWrite` rechecks observed bytes under the lock, writes atomically via
  `sk-kernel`, verifies, and restores the previous content (or absence) on a
  verification failure.
- `filter_graph_by_activation`: mission-step ownership, kind gate, per-artifact
  canonical-URN gate, effective deny first, edges only when both endpoints
  survive. `ActivationState::unknown_allow_tokens` keeps dropped selections
  visible.
- `CascadeScope` parsing, `cascade_activation_targets` (in-scope/skipped/
  kind-filtered/deactivated/unreached), `referenced_but_not_cascaded`, and
  shared-reference-safe `deactivation_plan` (exclusive vs shared, naming the
  still-referencing source; non-activatable nodes reported, never dropped).

### Context and profiles

- `ProfileRepository`: layered loading, `specializes_from` lineage composed from
  the DRG (root → leaf union of list fields, one-level mapping merge), missing
  parent/cycle/ambiguous-parent refusals, profile-channel deliveries with
  `suggests` `when` clauses.
- `build_action_context`: action normalization (bare or full URN, explicit or
  unique mission type), activation filter, eager vs `suggests` delivery,
  `include_all`, glossary references, deterministic ordering, project-charter
  presence, diagnostics, and the versioned `CONTEXT_SCHEMA_VERSION = "1.4.0"`
  top-level key set (`CONTEXT_CONTRACT_TOP_LEVEL_KEYS`).
- `CharterContext::to_json` emits only contracted top-level keys; profile links
  and tension annotations are typed API fields, deliberately outside the JSON
  contract.

## Verification

- `CARGO_BUILD_JOBS=2 cargo test -p sk-doctrine` — 69 unit + 18 baseline
  conformance tests + 1 doctest.
- Baseline conformance consumes the checked-in upstream fixtures
  (`tests/architectural/_fixtures/org_packs/example_org`,
  `tests/charter_offering/fixtures/minimal_org_pack_2`,
  `tests/charter_offering/fixtures/org_pack_template_asset/*`,
  `tests/charter_offering/fixtures/valid-profile.agent.yaml`) and pins the
  exact reference URN grammar, ledger key order, diagnostic strings, activation
  warnings, tension annotation shape and context contract key set.
- `CARGO_BUILD_JOBS=2 cargo clippy -p sk-doctrine --all-targets -- -D warnings`
  and `cargo fmt -p sk-doctrine -- --check` pass.
- No real project governance is mutated by tests: every write uses a temp
  fixture root.

## Remaining full-charter gaps

These are real missing behaviours, not aliases for a successful stub.

1. **Org-charter policy import and required floors.** No reader for
   `<pack>/org-charter.yaml`, `required_*` unions, expected artifacts or
   revocation checks. Source: `src/charter/activation/org_charter.py`,
   `org_charter_loader.py`, `org_pack_discovery.py`,
   `org_expected_artifacts.py`, `org_requirement_kinds.py`. Consequence:
   callers must supply `required_floor` / `required_by` data themselves; the
   ledger cannot derive it.
2. **Full project governance resolution.** `all_directives` currently covers the
   action grain plus project-tier directives; catalog fallback, project-local
   charter extraction, selection merging and `directives_source` branch values
   are not ported. Source: `src/charter/activation/resolver.py`,
   `context_json.py`, `selection.py`, `scope.py`, `charter_md_parsing.py`,
   `governance_references.py`.
3. **Progressive disclosure and text/JSON bodies.** The payload carries URN/id/
   title/source/body_path links; it does not inline artifact bodies, render
   profile sections, token-budget sections, template includes, or the text
   `charter context` surface. Source:
   `src/charter/activation/progressive_disclosure.py`,
   `context_renderers/*`, `context_result_builders.py`,
   `context_renderers/artifact_bodies.py`.
4. **Mission-type profiles and action-index fallback.** Action context comes
   from DRG `action:` nodes; `governance-profile.yaml` type grains and
   `actions/*/index.yaml` loading/union are not ported. Source:
   `src/charter/activation/mission_type_profiles.py`,
   `mission_type_profile_repository.py`, `action_grain.py`,
   `src/charter/offering/missions/action_index.py`.
5. **Interview, synthesis and compiler.** No interview question flow, artifact
   synthesis, project DRG emission, charter compilation or synthesis manifest
   stamp. Source: `src/charter/activation/interview.py`,
   `synthesizer/**`, `compiler.py`, `manifest_loader.py`.
6. **Pack acquisition and lifecycle.** No fetch/pin verification/download,
   pack manager preset application, activation sync, provenance/versioning, or
   core-version compatibility evaluation. Source:
   `src/charter/packs.py`, `provenance.py`, `versioning.py`,
   `activation/pack_manager.py`, `preset_application.py`, `sync.py`;
   `sk-charter` covers the read-only descriptor/root checks used here.
7. **Ledger migration and upgrade.** `plan_ledger_migration` /
   `apply_ledger_migration`, divergent legacy copies, manifest restamping and
   the project-intent rename writer are not ported; the deny guard recognises
   the rename shape but does not perform the migration. Source:
   `src/charter/activation/activation_ledger.py` (migration section),
   `synthesizer/legacy_rename.py`, `bundle_restamp.py`.
8. **Replaceable-builtins adjudication.** The merge reports same-kind org
   overrides but does not read `replaceable-builtins.yaml` sanctions or
   produce `OverrideAdjudication`. Source:
   `src/charter/offering/drg/override_policy.py`.
9. **Asset sidecar containment/reporting.** Org asset sidecars are labelled by
   `sk-charter`'s read-only catalog, but the merge does not re-verify each
   sidecar's `path` containment or produce `AssetPathEscape` diagnostics.
   Source: `src/charter/offering/assets/repository.py`,
   `find_asset_path_escapes`.
10. **Org governance-profile completeness.** Only the documented `selected_*`
    fields are projected; revocation/expected-artifact semantics and
    org-charter merge parity are not. Source:
    `src/charter/offering/drg/org_governance.py`,
    `src/charter/activation/org_charter_merge_parity` tests.
11. **Topology schema validation.** `merge_topology_artifact` enforces the
    ordering/step-I/O invariants but does not validate the merged mission-type
    or step-contract schema. Source:
    `src/charter/offering/missions/models.py`, step-contract models.
12. **Lock sidecar cleanup.** Writes use `sk_kernel::FileLock` through a thin
    typed-error wrapper. The kernel keeps the lock inode (never unlinks it) and
    is same-thread reentrant; stale-lock cleanup is deliberately not attempted.
    Windows identity equivalence is the kernel's contract, not re-tested here.
13. **YAML edge cases.** Lossless mutation preserves top-level comments,
    quoting and order, and append/suffix-removal keeps item comments. A reorder
    or disjoint replacement of a list replaces that value (inner item comments
    are not reattached), and YAML alias/anchor write-back beyond what
    `yaml-edit` round-trips is not separately verified.
14. **Context compact-mode parity.** JSON top-level keys match the 1.4.0 ledger,
    but compact-mode `all_directives`/org-charter blocks are intentionally
    minimal until gaps 1–3 close.

## API map for the coordinator CLI

```rust
use sk_doctrine::{
    // graph
    load_graph_or_dir, validate_graph, resolve_context, deliver_action_context,
    // org/merge
    load_org_pack, merge_three_layers, merge_topology_artifact,
    // activation
    load_activation_ledger, resolve_activation_source, plan_activation,
    plan_deactivation, commit_plan, prepare_ledger_write, apply_ledger_write,
    ActivationState, filter_graph_by_activation,
    // cascade
    CascadeScope, cascade_activation_targets, deactivation_plan, referenced_but_not_cascaded,
    // context/profiles
    build_action_context, ContextRequest, ContextInputs, ProfileRepository,
    // identity/yaml
    IdentityIndex, YamlDocument, PreparedWrite,
};
```

Typical CLI flow:

1. `IdentityIndex::from_project(repo_root)` (or `from_roots`), load the merged
   graph (`load_org_pack` per configured pack → `merge_three_layers`).
2. `load_activation_ledger`, then `ActivationState::from_ledger` with any
   caller-supplied required floor; `filter_graph_by_activation`.
3. `plan_activation`/`plan_deactivation` (pure), then `commit_plan` and
   `PreparedWrite::apply` (single locked, lossless write).
4. `build_action_context` for `charter context --json`; use the typed
   `profile_links`/tension fields for surfaces outside the JSON contract.
```
