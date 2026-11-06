# sk-doctrine second remediation report (N1–N4 + packs-root floor)

**Status: FINAL for this scoped pass.** Implementation evidence only — not an
independent review, not an aggregate/release/compatibility/performance sign-off.

**Scope owner:** the second-remediation findings in
`/tmp/sk-rust-doctrine-remediation-review.md` (N1 HIGH, N2 MEDIUM, N3 LOW, N4
LOW) plus the leaf-pack packs-root normalization risk.
**Owned path:** `crates/sk-doctrine/**` only. Installer activation/ledger/
migrations, the CLI, runtime/next, workspace manifests and `docs/rust/**` are
other owners' and were not modified here. No commit, push, subagent, background
process, `nohup`, sleep/poll loop, aggregate/release run or new target was used.
**Reference:** pinned Python oracle
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`, selected fail-closed from
`<repo>/.venv-rust-oracle/bin/python` and verified lock-exact by
`scripts/rust/verify_oracle.py` inside each oracle test.
**Working tree HEAD:** `d64468a7693e4962712cbf3416d767440d22a649`
(`rust/rewrite`), uncommitted scoped edits.
**Logs:** `/tmp/sk-doctrine-second-remediation.log` (final green evidence) and
`/tmp/sk-doctrine-second-red.log` (red/green cycles).

## Verdict (scoped)

All four review findings are fixed at the source and pinned by live-oracle
regressions that failed on the pre-fix code and pass now. The additional
leaf-pack packs-root normalization risk is fixed behind one shared helper, with
both spellings covered by deterministic (env-free) tests.

| id | sev | disposition |
|----|-----|-------------|
| **N1** | HIGH | **FIXED** — absent-key catalogue default is the merged graph's ids for the kind (built-in ∪ org ∪ project), unioned with the built-in catalogue; diagnostic count still reports the built-in catalogue. |
| **N2a** | MEDIUM | **FIXED** — retired `governance.doctrine` is a typed HARD REFUSAL (`ACTIVE_CHARTER_CONFIG_INVALID`), never a degrade and never read. |
| **N2b** | MEDIUM | **FIXED** — malformed `governance`/`directives`/charter-YAML shapes are the reference's resolver-failure degrade: directly-loaded local directives, `directives_source: null`, no resolution diagnostics, no governance references. |
| **N3** | LOW | **FIXED** — typed entry `title` comes from the artifact model body (`title`→`name`), never the DRG node label; proven for all five typed kinds with labels that differ. |
| **N4** | LOW | **FIXED** — one `UNKNOWN_SELECTION_DROPPED` for one unknown *charter selection* (unknown activation tokens are not payload diagnostics), and the reference project-local `SELECTION_MERGED` wording. Compared exactly, not as a code subset. |
| leaf floor | risk | **FIXED** — one shared packs-root normalization (parent or `built-in` leaf) used by the org floor, the context roots and the identity index; explicit `*_with_packs_root` APIs added. |

No new refusal was added on a supported-input happy path: N2a is a ported
reference refusal, N2b is a degrade, and the rest are deliveries/diagnostics.

## Provisional checkpoint (written before the red/green runs)

At the provisional point the source edits were complete and the newly added
oracle regressions passed, but the red-first reversion evidence and the final
hashes/counts had not yet been produced. The final verification below replaced
that checkpoint; the checkpoint's counts were 149 tests / 21 oracle tests and
are unchanged by the red-cycle restores (the source is byte-identical before and
after, proven by the equal hash blocks in the final log).

## Source changes

### N1 — absent-key catalogue default (`src/context.rs::governance_seeds`)

The per-kind default is now `_graph_and_catalog_default_ids`: every node of the
kind the merged graph declares (built-in, `org:<pack>`, `project`) is a seed;
provenance never excludes a node. The built-in catalogue is still what the
paradigm `CATALOG_DEFAULT_APPLIED` message counts (`(14 paradigms)`), exactly as
the reference's `_resolve_paradigm_base` counts `offering_catalog`. Explicit
`activated_<kind>` lists, explicit empty lists, org `required_<kind>` unions,
the effective-deny subtraction and the project-local directive roots keep their
previous three-state semantics. `all_directives`/`directives_source`
(`resolve_directives`) intentionally stay catalogue/activation + charter +
project-local — org/project graph nodes are NOT unioned into `all_directives`,
matching `_resolve_directive_base`'s `offering_catalog.directives` base.

### N2a — retired `governance.doctrine` (`src/context.rs`, `src/error.rs`)

`require_canonical_governance` mirrors `sync.require_canonical_governance`: a
`governance:` MAPPING that carries `doctrine` (even `doctrine: null`) raises
`DoctrineError::RetiredGovernanceSelection { path, runbook }`. The message names
the charter path, `governance.doctrine`, `spec-kitty upgrade` and
`docs/migrations/charter-pack-cutover.md`; `reference_code()` returns
`Some("ACTIVE_CHARTER_CONFIG_INVALID")`. The `governance.doctrine` fallback in
`selected_directives` is deleted, so no reader can resolve the retired
selection. The refusal is raised from `charter_sections`, the same read the
reference's governance-reference seam re-raises from, and is gated on the
canonical-root probe: a non-Git root degrades (the reference's governance read
raises `NotInsideRepositoryError` before it can see the key), which is covered
by its own oracle case.

### N2b — malformed charter shapes (`src/context.rs`)

`CharterConfigSections` models the two reference reads:

* `governance_section_valid` — structural `GovernanceConfig` surface: a
  non-mapping section, a scalar/null `charter`, non-`list[str]` `selected_*`/
  `available_tools`/`authority_paths`/`governance_references`, a non-string
  `template_set` (`null` stays valid), non-mapping block models, non-mapping
  `activations` entries, or a non-mapping `retrospective`. Unknown keys are
  ignored, exactly as Pydantic's default `extra="ignore"`.
* `directives_section_valid` — structural `DirectivesConfig` surface: the
  section must be a mapping, `directives` a list, each entry a mapping with the
  required string `id` and `title`, optional `description`/`severity` strings
  and a `references` list of strings. A bare-sequence `directives:` section
  (previously accepted by a native leniency) now fails exactly as
  `DirectivesConfig.model_validate` does.
* `CharterDocument` distinguishes absent from unparseable `charter.yaml`; an
  unparseable file makes BOTH reads fail (the reference raise is caught by the
  same best-effort catches), so it degrades instead of inventing catalogue
  provenance.

On either failure `resolve_directives` returns only the directly-loaded local
directives (`source: "project"`, authored order), `source: None`, no
diagnostics; the seed walk's paradigm `CATALOG_DEFAULT_APPLIED` is suppressed
because the reference emits it from the resolver, and
`governance_reference_status` returns `[]` because the best-effort governance
read collapses to the default selection. Typed delivery, links and the other
governance blocks are unchanged.

### N3 — typed entry title (`src/context.rs::artifact_entry`)

`title` is now the body's `title`, then `name`; the DRG node label is no longer
consulted. An unresolvable artifact therefore has no title (matching
`collect_typed_artifacts`' plain `{id, source}` DTO) instead of a label-derived
one.

### N4 — diagnostics (`src/context.rs`)

* The `ActivationState::unknown_allow_tokens` → payload conversion is removed;
  the field stays API data. The unknown-activation-token warning collected in
  `resolve_directives`' activation branch is no longer pushed as a diagnostic.
  The reference logs those at WARNING level and reserves
  `UNKNOWN_SELECTION_DROPPED` for charter `selected_*` tokens (`_drop_unknown`).
* `charter_selection_merged_diagnostic` mirrors `_report_charter_selection`:
  de-duplicated resolution in token order; the "none added" branch names the
  SELECTION.
* `project_local_merged_diagnostic` mirrors `_resolve_directives_selection`:
  `Merged N project-local directive(s) onto the {source} set: …` /
  `All N project-local directive(s) already present in the {source} set; none added: …`.

### Packs-root normalization (`src/org_charter.rs`, `src/context.rs`, `src/identity.rs`)

`SPEC_KITTY_PACKS_ROOT` is documented as the parent of `built-in`, but the CLI
adapter also accepts the built-in leaf (`<packs>/built-in`). Previously
`required_floor` (through `configured_org_roots`) and the crate's
`context_roots`/`IdentityIndex::from_project` read the raw env value, so a leaf
spelling made `BuiltInSearch` fail; in the lenient floor posture
`PackRoots::resolve`'s error became an EMPTY floor (org floor drop) while the
CLI's normalized read still saw the packs.

One shared seam now normalizes it: `normalize_packs_root` (parent unchanged,
`built-in` leaf → parent, any other name unchanged), `packs_root_override`, and
`built_in_search`. `configured_org_roots`, `context_roots`,
`IdentityIndex::from_project` all use it. Precise deterministic API for other
owners (no environment mutation needed):

```rust
normalize_packs_root(path) -> PathBuf
packs_root_override() -> Option<PathBuf>
configured_org_roots_with_packs_root(repo_root, Option<&Path>)
org_charter_documents_with_packs_root(repo_root, strict, Option<&Path>)
read_org_required_tokens_with_packs_root(repo_root, kind, strict, Option<&Path>)
read_org_required_provenance_with_packs_root(repo_root, kind, identity, strict, Option<&Path>)
required_floor_with_packs_root(repo_root, identity, Option<&Path>)
context_roots_with_packs_root(project_root, Option<&Path>)
```

The existing functions delegate to these with the normalized environment value,
so the CLI needs no edit and no call site changes.

## Red-first evidence (`/tmp/sk-doctrine-second-red.log`)

Each cycle patched only the named seam back to its pre-fix behaviour, ran the
new regression, restored the file from a byte-exact backup, and re-ran it. The
source was compared to the backup after the last cycle (`cmp`: identical).

| cycle | red result | green result |
|-------|------------|--------------|
| N1 absent-key org/project default | FAILED at `directives: exact nested entries` (org/project graph artifacts dropped) | 21 passed |
| N2a retired `governance.doctrine` | FAILED (`native_payload_result` returned a payload instead of the typed refusal) | 21 passed |
| N2b malformed governance degrade | 4 FAILED (the malformed case plus the two non-Git degrades and the non-Git retired-key control — the invented catalogue provenance is the shared seam) | 21 passed |
| N3 typed title from model | 2 FAILED at `directives: exact nested entries` (the N3 case and the N1 org title case) | 21 passed |
| N4 diagnostics cardinality/wording | FAILED at `exact governance-resolution diagnostics` (3 unknowns instead of 1) | 21 passed |
| leaf floor normalization | FAILED at the leaf floor assertion (empty floor without normalization) | 1 passed |
| leaf context-root normalization | FAILED at `context_roots_with_packs_root(leaf)` (built-in tier unresolved) | 1 passed |

## Final verification (executed, this source state)

Log `/tmp/sk-doctrine-second-remediation.log` (2026-11-03T14:2xZ, HEAD
`d64468a7…` + scoped edits). Every Cargo call went through
`/tmp/sk-focused-cargo.sh` (temporary HOME/XDG/TMP, shared Cargo lease,
`CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, existing target, no aggregate/release/
new target/extra manifest). Oracle reads use the absolute lock-exact
`.venv-rust-oracle/bin/python`, `PYTHONPATH=<repo>/src`,
`SPEC_KITTY_PACKS_ROOT=<repo>/packs`.

| check | result |
|-------|--------|
| `cargo test -p sk-doctrine --locked` | **149 passed, 0 failed** (108 unit + 18 `baseline_fixtures` + 21 `governance_oracle` + 2 doctests) |
| `cargo test -p sk-doctrine --locked --test governance_oracle` | **21 passed, 0 failed** (real oracle subprocesses, ~27 s) |
| `cargo clippy -p sk-doctrine --all-targets --locked -- -D warnings` | pass, exit 0 |
| `cargo fmt -p sk-doctrine -- --check` | pass, exit 0 |
| source hashes before vs after every check | identical |

The full-payload comparison is the `assert_payload_matches_oracle` used by every
oracle case: all five typed arrays entry-for-entry including the whole
model-dump `body`, the top-level link set by order and content,
`all_directives`, `directives_source`, `project_charter`,
`governance_references`. Diagnostics are compared exactly (order, code,
message) for every entry the native contract models; the only filtered oracle
entries are the declared-unported tools/template-set `FALLBACK_APPLIED` pair,
and the helper asserts native never fabricates them.

New oracle cases (all live-oracle, exact):

1. `absent_key_catalogue_default_includes_org_and_project_graph_artifacts` —
   org fragment directive/tactic plus a project pack graph directive/tactic,
   no activation ledger; exact payload, explicit delivery of all four, the
   `(14 paradigms)` catalogue count preserved, and an explicit-activation
   control.
2. `retired_governance_doctrine_key_is_a_typed_refusal_not_a_degrade` — oracle
   raises `ActiveCharterConfigError` / `ACTIVE_CHARTER_CONFIG_INVALID`; native
   returns `RetiredGovernanceSelection` with that `reference_code()`, the
   upgrade/runbook message, and a canonical-key control.
3. `retired_governance_key_on_a_non_git_root_degrades_like_the_oracle` — the
   refusal is gated on the canonical-root read; non-Git degrades exactly.
4. `malformed_governance_and_directives_shapes_degrade_to_local_only` — seven
   shapes: `governance: []`, scalar `charter`, scalar
   `selected_directives`, non-string selected item, bare-sequence `directives`,
   directive without `title`, unparseable `charter.yaml`; each exact with
   `directives_source: null`, `diagnostics: []`, `governance_references: []`
   and local-only (or empty) `all_directives`.
5. `typed_entry_titles_come_from_the_artifact_model_not_the_graph_label` —
   valid org artifacts of all five typed kinds whose graph labels all differ
   from the body titles; exact payload plus per-entry title assertions.
6. `unknown_selection_and_project_local_merge_diagnostics_match_the_oracle_exactly`
   — one `UNKNOWN_SELECTION_DROPPED` (the unknown charter selection, not the
   unknown activation token) and the reference project-local wording.

Unit regressions: `normalize_packs_root` parent/leaf/pack spellings plus the
floor and org-registry reads under the leaf spelling
(`org_charter::packs_root_tests`), and `context_roots_with_packs_root`
parent-vs-leaf (`context::tests`). Both use a `packstore` directory that is NOT
discoverable as `<ancestor>/packs/built-in`, so the leaf case cannot pass via
the built-in ancestor walk.

## Hashes at the verified state

```text
# changed by this pass
c5a0dc80c7f64ec6ed4d61dcb2d1421858f4fff0e1147ccb4060437837321447  src/context.rs
9735a413eebd14140b74533069ba553aa4aac678067bbacda2ca2889778b466f  src/error.rs
8830279eee5259863e15eac5dc8001873e21c0e94631b31d5c59fcf2f05d9dc3  src/identity.rs
6b0a70bf3f0a8e57a443e4c370874a3b30c86dc1f543a9972ba1901b94952ba9  src/lib.rs
5303d47a720bfd27cad5fb11fe4102e95ca0ff2b18f61f2ef329153f6a55cdb0  src/org_charter.rs
93cdfa46241348320cd21fcb05350eb7a5222cffca2124c4e8f67bafbffb3051  tests/governance_oracle.rs
79b456bf4e125f7b6135362280e87e6475d887786f76ae364438aa8162048034  README.md
# unchanged by this pass
45b17d333f2b0efd48ac331d995c282991e088c99bc2cb24c53c1089c8f9cc1d  src/activation.rs
71e3d828748441ff298f62ae5b066520a04a983b83b679e107146e73c592f688  src/body.rs
6348c8f34d0b00b68751cb112cd3ef25226fda8612e79e783fb0e26996040353  src/cascade.rs
1c9e9cb31eba5d3e9107fda6c2784c3f129cd6b6a13d988f5965ed2edf9d9dfb  src/graph.rs
969c95dbdcd78b76cd222bbf7ef6e78137cf9354c6a02272a792e6c613bbc82e  src/ledger.rs
db2a6e12d78ff38188c49415d53fd1843bcbc1562203666917031f72c5f7a787  src/loader.rs
c3242ef676ef1f3b5a7ba98d4f1912b265a61484b666a2ff1da3b93700a72838  src/lock.rs
87de80bd6406d1dab3eafdf781d5de0c4216ef77133ed65724bf13e96223938b  src/merge.rs
ed43ebfa539de59a4bb6d5b5d967d207757fd9799271ee98a2fc1a11e3715a03  src/org.rs
30471ce6da499da9f42b638c4eb712913a7908f03c7d82ed170385b28f0aaae8  src/plan.rs
4f5db7a988eeb8974c7154dfece86e601d874e1d4ba2621736721764b94702aa  src/profile.rs
85dc10eb14442062f85fbfdf6195eadf5245437ee2d92f1570d17d407fea9144  src/query.rs
0078eaca5b6a82b8e9a38fc9bdb323f41d6a0a46893f44bc7d0cfe62b6f6f5f2  src/validate.rs
654cb2d99e4a74aae26d24b220a40aba2d42d8bd2f62e6cb222ab730a5c00ec9  src/yaml.rs
54bc8c415c17fa59e4a676e71ae98e954d33f1f4a5c4d3f5a143b117f40148c7  tests/baseline_fixtures.rs
37802f3679f210f2468072d4190c5554da3c11f5c6a56bf3038fa0ba3141ec05  tests/support/mod.rs
```

## Preserved behaviour (not re-litigated, re-verified green)

The first remediation's body/profile/floor/link and transaction-lock fixes are
unchanged: 26/26 built-in profiles, nested model-dump bodies, org-floor field
independence with `extends`, the exact link set, the canonical synthesis lock
and the activation transaction still pass their existing oracle/unit and
cross-language tests. The non-Git provenance degrade keeps `all_directives: []`,
`directives_source: null`, `diagnostics: []` and `governance_references: []`
with typed delivery intact. The `all_directives` catalogue base remains
built-in-only (org/project ids are deliberately not unioned there). Git-local
controls (lock path, refusals, activation writes) are untouched.

## Coordinator handoff notes (API and behaviour deltas)

* **Additive public API only**: `normalize_packs_root`, `packs_root_override`,
  `configured_org_roots_with_packs_root`,
  `org_charter_documents_with_packs_root`,
  `read_org_required_tokens_with_packs_root`,
  `read_org_required_provenance_with_packs_root`,
  `required_floor_with_packs_root`, `context_roots_with_packs_root`,
  `RETIRED_GOVERNANCE_RUNBOOK`, `DoctrineError::reference_code()`.
  `ContextInputs`, `ContextRequest`, `build_action_context`, `context_roots`,
  `required_floor` and the activation-transaction signatures are unchanged, so
  the runtime-next/CLI owners need no code change; their existing
  `required_floor(...)` call now also succeeds when `SPEC_KITTY_PACKS_ROOT`
  names the built-in leaf.
* **New `DoctrineError` variant** `RetiredGovernanceSelection`: dependent crates
  match `DoctrineError` with wildcard arms (`sk-install::activation::target_error`,
  CLI error mappers), so their builds are unaffected; they receive the
  reference code via `reference_code()`.
* **`build_action_context` can now return an error on a legacy project** that
  still authors `governance.doctrine` (the reference refuses it). Any CLI
  surface that used to print a payload for that project will now surface the
  upgrade refusal — intended parity, but a snapshot/test owner should expect it.
* **Diagnostics payload deltas**: unknown activation tokens are no longer
  echoed as `UNKNOWN_SELECTION_DROPPED`, and the project-local
  `SELECTION_MERGED` message is reworded to the reference's text. Any external
  expectation pinned to the old native wording is stale.
* **No installer activation/ledger/migrations/manifests/CLI/docs outside this
  crate were edited**, and no commit/push was made.

## Residuals and boundaries

* **No independent review yet.** These results are owner evidence; the
  coordinator/independent reviewer must re-run and accept them.
* **Diagnostics breadth.** Tools/template-set `FALLBACK_APPLIED` is still not
  emitted (declared README gap, unchanged). The exact diagnostic tests enforce
  everything the native contract models and explicitly refuse to fabricate the
  missing pair. `available_tools`/`template_set` resolution is likewise still
  unported.
* **Validation depth.** `governance_section_valid`/`directives_section_valid`
  are structural surfaces (the shapes the resolver rejects), not a full
  Pydantic port: malformed nested block models (e.g. an invalid `testing`
  field type) can still be read leniently. The named review shapes are covered.
* **Graph input.** The catalogue-default union uses the graph's declared ids;
  the production graph's built-in layer contains the real catalogue. A
  hand-built graph that omits a whole built-in kind would not widen to the
  on-disk catalogue in native (the reference's injected-graph union would).
  No production caller passes such a graph.
* **Packs-root env read.** The crate normalizes `SPEC_KITTY_PACKS_ROOT`
  internally; the CLI adapter still has its own equivalent normalization, which
  now agrees. No CLI/installer edit was made (out of scope).
* **Platform/scope.** Linux only, one pinned oracle closure; no macOS/Windows,
  aggregate, release, packaging or performance claim. Other owners were
  concurrently editing `sk-install`/`sk-cli`/runtime surfaces, so a whole
  workspace build remains the coordinator's.

## Files changed by this pass

- `crates/sk-doctrine/src/context.rs` — catalogue default, charter-section
  model (retired/malformed handling), typed-entry title, diagnostics wording
  and cardinality, `context_roots_with_packs_root`, unit regressions.
- `crates/sk-doctrine/src/error.rs` — `RetiredGovernanceSelection`,
  `RETIRED_GOVERNANCE_RUNBOOK`, `reference_code()`.
- `crates/sk-doctrine/src/org_charter.rs` — shared packs-root normalization and
  the `*_with_packs_root` floor/registry API, unit regressions.
- `crates/sk-doctrine/src/identity.rs` — normalized built-in search in
  `from_project`.
- `crates/sk-doctrine/src/lib.rs` — re-exports for the new API.
- `crates/sk-doctrine/tests/governance_oracle.rs` — six new live-oracle cases,
  exact-where-ported diagnostics comparison, project-layer payload helper,
  body/id fixtures.
- `crates/sk-doctrine/README.md` — verification counts, new comparison scope and
  the N1/N2/N3/N4/packs-root contracts.
- `crates/sk-doctrine/SECOND-REMEDIATION-REPORT.md` — this report.
