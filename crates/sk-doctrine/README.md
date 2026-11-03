# sk-doctrine

Native governance execution for the Spec Kitty Rust rewrite.

`sk-doctrine` owns the executable half of the charter/doctrine domain. It
consumes `sk-charter`'s read-only kind, config and pack APIs and `sk-kernel`'s
containment, topology and atomic-write primitives; it never edits those crates
and never invokes Python at runtime.

Contract mirrors (reference only):

- `src/charter/offering/drg/{models,loader,merge,query,reachability,validator,org_pack_loader,project_scan}.py`
- `src/charter/activation/{activation_engine,activation_ledger,pack_context,drg_activation,cascade,context,context_contract,profile_resolution}.py`
- `src/charter/activation/{org_charter,org_charter_loader,org_pack_discovery,org_expected_artifacts,org_requirement_kinds}.py`
- `src/charter/activation/{context_json,progressive_disclosure,resolver}.py`, `context_renderers/artifact_bodies.py`
- `src/charter/activation/synthesizer/synthesis_lock.py`
- `src/charter/activation/{language_scope,language_vocabulary,language_authority}.py`,
  `src/charter/offering/shared/{scoping,language_catalog}.py` (language signal only)
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
  override with a conflict record (`node_override`/`edge_override` plus the
  three layer-declaration fields), kind-drift refusal, one endpoint policy (local
  bare id → qualified URN → unique built-in id), unknown-relation refusal,
  ambiguous endpoint refusal, duplicate `asset:`/`template:` URN refusal, and
  deferred dangling-endpoint **warnings** over the fully assembled graph (the
  reference behaviour `charter lint` depends on; a caller that owns graph
  completeness opts into the hard refusal with `allow_forward_references: false`).
- `merge_topology_artifact`: `overrides` replacement; `enhances` field merge that
  preserves the `steps`/`actions` backbone and refuses a dropped step or a
  stripped `inputs`/`outputs` contract.
- `project_layer` / `scan_project_artifacts` / `walk_project_agent_profile_nodes`:
  project artifact discovery that validates every file against its kind's schema
  (`required` + `extra="forbid"` key surface), refuses a symlink that escapes the
  project pack root, and projects profile references with the extractor's exact
  relations (`directive-references`/`tactic-references` → `requires`,
  `styleguide-references`/`toolguide-references` → `suggests`,
  `collaboration.operating-procedures` → `requires`) and authored rationale.

### Org charter policy import

- `load_org_charter_policy` / `org_charter_documents`: strict `org-charter.yaml`
  parsing (any integer `schema_version` is folded like the reference model rather
  than version-gated, known-field allowlist, `required_*` list shapes,
  governance policies, activations, `extends:` chains with cycle/dangling-base
  refusals; a missing declared pack is a strict-mode refusal).
- `read_org_required_tokens`, `read_org_required_provenance`,
  `org_required_stems`, `required_floor`: the `required_<kind>` union over every
  declared pack and its `extends:` chain, normalized to activation stems, with
  the requiring pack recorded (`'base'` / `'base' (via 'overlay')`). The
  best-effort readers parse the raw mapping per field, so one malformed
  `required_<kind>` value drops only that field and never the pack's other kinds
  or its `extends:` contribution.
- `resolve_org_expected_artifacts`, `resolve_org_requirement_kinds`: the org-tier
  mission overrides, whole-file, last matching org root wins, malformed mapping
  refused.
- `validate_org_required_directive_stems`, `read_org_skill_namespace`.

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
  required floor, and `raw()` round-trip data that keeps unknown
  `deactivated_<kind>` keys **and their values**.
- `plan_activation` / `plan_deactivation`: pure, validation before post-state,
  absent-key materialisation from what is actually in force, the "already in
  force ⇒ write nothing" rule, `--only` narrowing, deny exclusions, ordered
  required-floor and unknown-id refusals.
- `prepare_ledger_write` / `commit_plan`: deny-change guard (`DenyListChoice`),
  unmigrated deny refusal, empty ledger rendering, and a lossless
  `YamlDocument` that preserves comments, quoting and key order.
- **One authoritative activation transaction.** `ActivationTransaction::begin`
  observes the activation authority at plan time; `commit` takes the canonical
  charter synthesis lock, rechecks the observation, writes atomically via
  `sk-kernel`, verifies, and restores the previous content (or absence) on a
  verification failure. A concurrent writer — Python or native — makes the
  commit refuse with `precondition_changed` instead of being overwritten.
  `PreparedWrite::apply_observed` is the same guard for callers holding a
  prepared write.
- **Canonical lock authority.** `synthesis_lock_path` /
  `acquire_synthesis_lock` place the lock exactly where Python's
  `charter.activation.synthesizer.synthesis_lock` does
  (`<git-common-dir>/spec-kitty-locks/charter-synthesis/<sha256[0..32]>.lock`,
  else `<root>/.kittify/runtime/locks/charter-synthesis.lock`) through the
  kernel's `anchored_lock_path`. Activation writes take it, never a private
  sidecar.
- `filter_graph_by_activation`: mission-step ownership, kind gate, per-artifact
  canonical-URN gate, effective deny first, edges only when both endpoints
  survive. `ActivationState::unknown_allow_tokens` keeps dropped selections
  visible; the fallible `try_filter_graph_by_activation` fails closed rather than
  admitting the unfiltered graph.
- `CascadeScope` parsing, `cascade_activation_targets` /
  `cascade_activation_targets_for_state` (in-scope/skipped/kind-filtered/
  deactivated/unreached), `referenced_but_not_cascaded(_for_state)`, and
  shared-reference-safe `deactivation_plan`. The state-based entry points use the
  **effective** deny, so a stale deny of an org-required artifact cannot stop a
  cascade.

### Identity

- `IdentityIndex`: stem-first across all layers (project, org packs
  last-to-first, built-in), then declared id, then the canonical directive-id
  normaliser (`1`/`001`/`directive:1` → `DIRECTIVE_001`;
  `use-c4-model-techniques` → `USE_C4_MODEL_TECHNIQUES`). A shared stem resolves
  by layer precedence instead of failing; `declared_id_in_layer` asks one layer;
  `stored_spelling` raises `ActivationIdCollision` only when no stem can
  represent a declared id; `selection_id`, `lookup_key`, `activation_id`,
  `source_layer` match the reference helpers.

### Context and profiles

- `ProfileRepository`: layered loading where a same-id org/project profile is
  **deep-merged into** its built-in base (nested mappings recursively, scalars/
  lists replaced), schema validation with skip-and-record diagnostics (no
  file-stem renaming), `applies_to_languages` filtering with family expansion
  and recorded scope-filtered ids, `specializes_from` lineage composed from the
  DRG (root → leaf union of list fields, one-level mapping merge), missing
  parent/cycle/ambiguous-parent refusals, leaf `excluding` declarations (whole
  fields or named values), and profile-channel deliveries with `suggests` `when`
  clauses.
- `build_action_context`: action normalization (bare or full URN, explicit or
  unique mission type; a bootstrap action still delivers when it has no unique
  node), activation filter, selected-doctrine closure (activated + org-required
  directives/tactics/paradigms, ownership-gated so another action's scoped
  doctrine does not leak), typed delivery with real artifact bodies, `inline` vs
  `link` cadence, `include_all`, the complete top-level link set
  (`{id, relation, when, reason}`), glossary references, deterministic ordering,
  and the versioned `CONTEXT_SCHEMA_VERSION = "1.4.0"` top-level key set.
- The nested entries match the reference shape exactly (`{id, source, title?,
  summary?, references[], delivery, body?}`), `all_directives` is the plain
  provenance DTO, `directives_source` uses the resolver's real labels
  (`catalog_fallback` / `activation` plus `+charter` / `+project_local`),
  `project_charter` always carries its four core keys plus byte count and
  metadata fields, `org_charter` carries the imported policies, and
  `governance_references` carries the per-path status (with `warning: null` when
  clean).
- **Non-Git project roots keep the reference's documented degrade.** The
  reference's `load_directives_config`, `load_governance_config` and
  `resolve_project_governance` anchor on
  `charter.resolution.resolve_canonical_repo_root` (`git_common_dir`), so a root
  outside a Git repository fails those reads and
  `context_json._load_project_directives_with_diagnostics` catches it. Native
  probes the same `sk-kernel` topology seam (no second Git resolver) and then
  reports the contract instead of inventing one: `all_directives: []` (nothing
  was loaded before the failure), `directives_source: null` (the documented
  provenance-unavailable signal), no resolution diagnostics and no
  `governance_references`. Typed delivery, links and `project_charter` are
  unchanged, so this is a degrade, not a refusal of the supported non-Git
  input. The `directives_source` labels above still apply to Git checkouts, and
  the catalogue-default/selection diagnostics are only emitted where the
  reference emits them.
- **Retired and malformed governance shapes are refused or degraded, never
  guessed.** `governance.doctrine` is the retired selection key: the reference
  raises `ActiveCharterConfigError` / `ACTIVE_CHARTER_CONFIG_INVALID` from
  `require_canonical_governance`, and native raises the typed
  `DoctrineError::RetiredGovernanceSelection` (`reference_code() ==
  ACTIVE_CHARTER_CONFIG_INVALID`) with the `spec-kitty upgrade` and
  `docs/migrations/charter-pack-cutover.md` remediation. A malformed shape
  (`governance: []`, a scalar `charter`/`selected_directives`, an unparseable
  `charter.yaml`, a bare-sequence/malformed `directives:` section) is the
  reference's resolver-failure DEGRADE: the directly-loaded local directives
  are returned with `directives_source: null`, no resolution diagnostics and no
  `governance_references`. Neither path reads the retired selection or reports
  an invented provenance.
- **The absent-key catalogue default is `_graph_and_catalog_default_ids`.**
  When an `activated_<kind>` key is absent, the seed set is every node of that
  kind the merged graph declares — built-in, ORG and PROJECT — not just the
  built-in catalogue, so an org pack's directives/tactics are delivered in the
  default project state. The diagnostic still reports the built-in catalogue
  size, exactly as the reference's `_resolve_paradigm_base` counts it.
- **Packs-root spelling is normalized once.** `SPEC_KITTY_PACKS_ROOT` is
  documented as the parent of `built-in`, but the CLI adapter also accepts the
  built-in leaf (`<packs>/built-in`). `normalize_packs_root`,
  `packs_root_override`, `configured_org_roots_with_packs_root`,
  `org_charter_documents_with_packs_root`,
  `read_org_required_{tokens,provenance}_with_packs_root`,
  `required_floor_with_packs_root` and `context_roots_with_packs_root` apply the
  same normalization, so a leaf value cannot silently empty the org required
  floor while the CLI's own reads see the packs.
- **Typed entry titles come from the artifact model.** `title` is the body's
  `title` (then `name`) exactly as `artifact_to_dict` reads it; the DRG node
  label is graph metadata and is never substituted (an org fragment label that
  differs from the body title is a real repro).
- `CharterContext::to_json` emits only contracted top-level keys; profile links
  and tension annotations are typed API fields, deliberately outside the JSON
  contract.

### Language scope (read-only provider; not CLI-bound yet)

The doctrine-side half of the language signal, ported from
`charter/activation/{language_vocabulary,language_scope,language_authority}.py`,
`charter/offering/shared/{scoping,language_catalog}.py` and the interview
defaults. It is a library seam: **no CLI detector, dispatcher or runtime path is
bound to it**. The CLI retains its separate catalog/configured/compiled/interview
implementation, which lacks the doctrine vocabulary and differs in other tier details
(see the remaining gap below).

- `scoped_language_vocabulary(repo_root, packs_root)` is a lightweight YAML scan
  of `applies_to_languages` over exactly the five scoped kinds (tactics,
  styleguides, toolguides, procedures, agent profiles), from the built-in tier,
  the project overlay (`.kittify/charter-packs`, then the code-local layer
  candidate) and every existing configured org pack root in both directory
  spellings. Only single `\w`-word string items count; tokens are normalized and
  `unknown`/`any`/`all` are stripped. Malformed, non-mapping, unreadable and
  non-UTF-8 artifacts are skipped exactly as the reference skips them — no graph
  is built, no sole-door activation gate is applied and no confinement or
  refusal is invented.
- `LanguageVocabularyCache` is the explicit memo: a caller-owned LRU bounded to
  **8** roots (the reference's `lru_cache(maxsize=8)`), with `clear()`, `len()`,
  `capacity()` and `keys()`. The explicit packs-root seam is part of the native
  key (the reference keys on the repo root alone, because its packs root is
  ambient). A failed scan is never cached; no performance claim is made.
- `LanguageScopeService::infer` is the single inference authority, with the
  three-state result typed as `InferredLanguages::{NoSignal, Empty, Declared}`.
  Tier order: operator `languages` in `.kittify/config.yaml` (wins over
  everything, `prefer_interview` included, and is validated against the catalog
  so a confirmed list is never widened), then the compiled
  `.kittify/charter/charter.yaml` `catalog.languages` (present-but-empty is an
  authoritative `[]`; missing/null/non-list falls back; items are `str()`-
  converted, never silently filtered), then the interview (`prefer_interview`
  resolves from it alone and may return no signal, and a supplied interview that
  is only the shipped `defaults.yaml` text does not mask the on-disk
  transcript). There is no `charter.md` fallback.
- A real `languages_frameworks` declaration decides alone: built-in detector hits
  in that answer UNION doctrine-vocabulary whole-word hits in that answer, else
  `["unknown"]`. Placeholder, default or absent declarations scan the other
  answers with the built-in detector only, and return no signal when nothing is
  found. The build-token rule (`"Gradle"` → `jvm`), the `word-`-compound guard
  (`librespot-java` names no language) and the `C#`/`C++` word exclusion are
  ported.
- `lacks_specialist_guidance` is the D2 advisory: no signal and explicit-empty
  are never advisory, reserved tokens are stripped, families are expanded, and
  the remainder is intersected with the scoped vocabulary.
- Declared representation limits (no false equality): `str.casefold()` is
  approximated with `to_lowercase()` (ASCII-identical; `ß`/final-sigma edges
  differ), Python `\w` with `char::is_alphanumeric() || '_'`, and the
  `ast.literal_eval` subset covers list/tuple/scalar answer shapes (float and
  exotic reprs are best-effort; unparsable shapes fall back to the raw answer
  text exactly where the reference falls back). Reference logging warnings are
  value-identical but are not emitted natively.

The independent 170-test service review is scoped PASS with findings, not
end-to-end approval. Coordinator P2–P4 corrections now cover BOM-prefixed
multi-key YAML, whole-file unknown-tag skip and org-only reads that ignore
unrelated invalid config fields (strict whole-config validation unchanged).
Unicode casefolding, numeric exponent repr, and transcript encoding recovery
remain real differences; artifacts are strict UTF-8 in the reference, but
interview transcripts use the broader charter ingestion authority. The CLI
binding must remove its existing implementation rather than add another path.
See `LANGUAGE-REMEDIATION-REPORT.md`.

### Installer integration

`crates/sk-install/src/activation.rs` no longer writes the activation authority
itself: `provision_mission_type_activations` delegates to the same
`sk-doctrine` transaction (target resolution, observation, synthesis lock,
precondition refusal, lossless write). Additive-only and preset-copy semantics
are unchanged.

## Verification

- `CARGO_BUILD_JOBS=2 cargo test -p sk-doctrine --locked`: **174 passed,**
  **0 failed** (120 unit + 18 checked-in baseline conformance + 21
  oracle-generated governance fixtures/regressions + 13 oracle-generated
  language-scope differential tests + 2 doctests). The pre-existing 149 all
  still pass; the language-scope tests are additive.
- Every fixture in `tests/governance_oracle.rs` fails closed on a missing or
  non-lock-exact `.venv-rust-oracle` (`scripts/rust/verify_oracle.py`). They
  generate, from the pinned Python oracle at test time:
  - the full nested `charter context --json` payload. The comparison is
    **exact, not a subset**: all five typed arrays entry-for-entry (id, source,
    title, summary, `references[]`, `delivery` and the whole model-dump `body`),
    the top-level link array (order and content, so an excluded-kind or extra
    link fails), `all_directives`, `directives_source`, `project_charter` and
    `governance_references`. Dedicated cases pin the catalogue-default delivery
    of the whole tactic catalogue, a selected non-hub tactic delivered from its
    own root, org-required roots independent of the action bundle (with a deny
    control), the explicit-empty directive allow list keeping the independent
    tactic catalogue, a single denied tactic removing only itself, the
    absent-key default delivering ORG and PROJECT graph artifacts, the
    malformed-`required_*` floor width, typed titles from the model for all five
    kinds (graph labels deliberately differ), the retired
    `governance.doctrine` refusal and its non-Git degrade control, the
    malformed governance/directives shape degrades, the diagnostics parity case
    (one `UNKNOWN_SELECTION_DROPPED` for one unknown activation token; exact
    project-local `SELECTION_MERGED` wording), and the non-Git degrade (with and
    without an authored local directive/selection) against its Git control.
    Resolution `diagnostics` are compared exactly (order, code and message) for
    every entry the native contract models; the only declared breadth gap is
    the tools/template-set `FALLBACK_APPLIED` pair, removed from the oracle side
    and never fabricated natively;
  - the `org_charter` block (`load_org_charter_json_block`), including the
    directory-basename `pack_name` fallback and a `schema_version` above the
    scaffolded one;
  - the profile catalogue: all **26** pinned built-in ids with **no** skips,
    plus alias/`populate_by_name` spellings and the reference's `role`/`roles`
    conflict refusal;
  - the `org_charter` required floor/provenance for a malformed overlay field;
  - the synthesis lock path, plus **actual cross-language contention**: a Python
    process holds the canonical lock and the native transaction refuses with
    `SynthesisLockHeld`, then commits after release.
- Baseline conformance consumes the checked-in upstream fixtures
  (`tests/architectural/_fixtures/org_packs/example_org`,
  `tests/charter_offering/fixtures/minimal_org_pack_2`,
  `tests/charter_offering/fixtures/org_pack_template_asset/*`,
  `tests/charter_offering/fixtures/valid-profile.agent.yaml`) and pins the
  exact reference URN grammar, ledger key order, diagnostic strings, activation
  warnings, tension annotation shape and context contract key set.
- `cargo clippy -p sk-doctrine --all-targets --locked -- -D warnings` and
  `cargo fmt -p sk-doctrine -- --check` pass.
- `cargo test -p sk-install --locked activation`: 7 passed (fresh ledger,
  authored lists preserved, legacy target, preset failures, concurrent-change
  refusal, canonical lock path).
- No real project governance is mutated by tests: every write uses a temp
  fixture root and a temporary HOME/XDG/TMP.

## Remaining full-charter gaps

These are real missing behaviours, not aliases for a successful stub.

1. **Mission-type profiles and action-index fallback.** Action context comes
   from DRG `action:` nodes; `governance-profile.yaml` type grains and
   `actions/*/index.yaml` loading/union are not ported. Source:
   `src/charter/activation/mission_type_profiles.py`,
   `mission_type_profile_repository.py`, `action_grain.py`,
   `src/charter/offering/missions/action_index.py`.
2. **Project-governance breadth beyond directives.** `all_directives` and
   `directives_source` are ported; the payload's `templates`/`tools`/paradigm
   resolution, `template_set` selection, interview/selection overlays and
   `charter.md`-frontmatter selection are not emitted (the contracts do not
   currently expose those keys, so nothing is fabricated).
3. **Text `charter context` renderers and compact-mode prose.** JSON bodies and
   delivery are real; the plain-text renderer family
   (`context_renderers/*`, `context_result_builders.py`,
   `_activation_render.py`, token-budget/template-include sections) is not
   ported, and compact mode intentionally carries no action grain.
4. **Interview, synthesis and compiler.** No interview question flow, artifact
   synthesis, project DRG emission, charter compilation or synthesis manifest
   stamp. Source: `src/charter/activation/interview.py`, `synthesizer/**`,
   `compiler.py`, `manifest_loader.py`.
5. **Pack acquisition and lifecycle.** No fetch/pin verification/download, pack
   manager preset application, activation sync, provenance/versioning, or
   core-version compatibility evaluation. Source:
   `src/charter/packs.py`, `provenance.py`, `versioning.py`,
   `activation/pack_manager.py`, `preset_application.py`, `sync.py`;
   `sk-charter` covers the read-only descriptor/root checks used here.
6. **Ledger migration and upgrade.** `plan_ledger_migration` /
   `apply_ledger_migration`, divergent legacy copies, manifest restamping and
   the project-intent rename writer are not ported; the deny guard recognises
   the rename shape but does not perform the migration. Source:
   `src/charter/activation/activation_ledger.py` (migration section),
   `synthesizer/legacy_rename.py`, `bundle_restamp.py`.
7. **Replaceable-builtins adjudication.** The merge reports same-kind org
   overrides but does not read `replaceable-builtins.yaml` sanctions or
   produce `OverrideAdjudication`. Source:
   `src/charter/offering/drg/override_policy.py`.
8. **Asset sidecar containment/reporting.** Org asset sidecars are labelled by
   `sk-charter`'s read-only catalog, but the merge does not re-verify each
   sidecar's `path` containment or produce `AssetPathEscape` diagnostics.
   Source: `src/charter/offering/assets/repository.py`,
   `find_asset_path_escapes`.
9. **Org governance-profile completeness.** Only the documented `selected_*`
   fields are projected; revocation/expected-artifact semantics and
   org-charter merge parity are not. Source:
   `src/charter/offering/drg/org_governance.py`,
   `src/charter/activation/org_charter_merge_parity` tests.
10. **Topology schema validation.** `merge_topology_artifact` enforces the
    ordering/step-I/O invariants but does not validate the merged mission-type
    or step-contract schema. Source:
    `src/charter/offering/missions/models.py`, step-contract models.
11. **Project-charter bundle-root refresh.** `project_charter` describes the
    repository root; the reference can refresh/redirect to a canonical bundle
    root (`charter/activation/sync.py`). Byte count and metadata are exact for
    the in-tree bundle.
12. **Lock cleanup.** The kernel keeps the lock inode (never unlinks it) and is
    same-thread reentrant; stale-lock cleanup is deliberately not attempted.
    Windows identity equivalence is the kernel's contract, not re-tested here.
13. **YAML edge cases.** Lossless mutation preserves top-level comments,
    quoting and order, and append/suffix-removal keeps item comments. A reorder
    or disjoint replacement of a list replaces that value (inner item comments
    are not reattached), and YAML alias/anchor write-back beyond what
    `yaml-edit` round-trips is not separately verified.
14. **Nested governance/directive model validation.** The canonical-root
    retired-key refusal and structural malformed-shape degrade are independently
    verified. Nested constraints are incomplete: invalid integer coverage or
    approval fields, and malformed activation entries (missing/extra fields or
    unknown mission type), can still resolve where the reference degrades to
    directly loaded locals. The 149-test / 52-check review does not approve
    those cases.
15. **Scoped language vocabulary (provider landed, CLI binding open).** The
    doctrine-side provider now exists (`scoped_language_vocabulary`,
    `LanguageScopeService`, oracle-verified), but the CLI's `compat/runtime.rs`
    catalog/tier/interview implementation does not call it, so the
    consumer-side behaviour is unchanged: a scoped custom `zig` declaration can
    still exclude a profile that the reference admits. Binding it — and the
    prompt/registry paths that consume the signal — is a separate, unreviewed
    step. No N5 end-to-end fix is claimed here.

## API map for the coordinator CLI

```rust
use sk_doctrine::{
    // graph
    load_graph_or_dir, validate_graph, resolve_context, deliver_action_context,
    // org/merge
    load_org_pack, merge_three_layers, merge_topology_artifact,
    // org charter / floors
    load_org_charter_policy, required_floor, org_required_stems,
    read_org_required_provenance, resolve_org_expected_artifacts,
    // activation
    load_activation_ledger, resolve_activation_source, plan_activation,
    plan_deactivation, commit_plan, prepare_ledger_write, apply_ledger_write,
    ActivationState, ActivationObservation, ActivationTransaction,
    filter_graph_by_activation, deactivated_urns_with_floor,
    synthesis_lock_path, acquire_synthesis_lock,
    // cascade
    CascadeScope, cascade_activation_targets_for_state, deactivation_plan,
    // context/profiles
    build_action_context, ContextRequest, ContextInputs, ProfileRepository,
    org_charter_block,
    // language scope
    scoped_language_vocabulary, LanguageVocabularyCache, LanguageScopeService,
    InferRequest, InferredLanguages, InterviewAnswers, configured_languages,
    extract_declared_languages, lacks_specialist_guidance,
    // identity/yaml
    IdentityIndex, YamlDocument, PreparedWrite, LockScope,
};
```

Typical CLI flow:

1. `IdentityIndex::from_project(repo_root)` (or `from_roots`), load the merged
   graph (`load_org_pack` per configured pack → `merge_three_layers`).
2. `ActivationTransaction::begin(repo_root)`; read the ledger through it,
   compute the org floor with `required_floor`, then
   `ActivationState::from_ledger` and `filter_graph_by_activation`.
3. `plan_activation`/`plan_deactivation` (pure), then
   `transaction.commit(&plan)` (one locked, observation-checked, lossless write).
4. Cascade with `cascade_activation_targets_for_state`, which uses the effective
   deny.
5. `build_action_context` for `charter context --json`; use the typed
   `profile_links`/tension fields for surfaces outside the JSON contract.
