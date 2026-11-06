# sk-doctrine / sk-install activation remediation report

**Scope owner:** doctrine remediation recovery (recovering the cancelled owner
`doctrine9edd1069`'s preserved edits).
**Owned paths:** `crates/sk-doctrine/**` and `crates/sk-install/src/activation.rs`.
Everything else on this branch (installer init/locks/profiles/session/ledger/upgrade,
CLI, workspace manifests, `docs/rust/**`) is another owner's and was not modified here.
**Review under remediation:** `/tmp/sk-rust-governance-migrations-continuation-review.md`
(reviewed HEAD `d64468a7693e4962712cbf3416d767440d22a649`, branch `rust/rewrite`).
**Pinned reference:** upstream `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
**State:** working tree, uncommitted (no commit/push/manifest change made here).

## Verdict (scoped)

The preserved doctrine edits are real and the review's doctrine findings F1, F2, F4,
F5, F6, F7, F8 are fixed at the source and now proven against the live pinned Python
oracle by an exact (not subset) differential comparison. F9's doctrine-side evidence
claims and the F10 documentation hazards inside this scope are corrected. F3 and
`upgrade.rs`/migration-report items are installer/coordinator-owned and were not
touched (F3's normalized-id settlement is already present in
`crates/sk-install/src/ledger.rs`). One residual divergence outside the reviewed
finding list (non-Git project-governance provenance) was recorded here unfixed at
the checkpoint above and has since been fixed in a bounded follow-up; see
[`NON-GIT-REMEDIATION-REPORT.md`](NON-GIT-REMEDIATION-REPORT.md) and the
"Follow-up" section below. The original checkpoint evidence is preserved
unchanged.

This is scoped evidence, not a compatibility, aggregate, release or performance
sign-off.

## Evidence (executed, this working tree)

Log: `/tmp/sk-doctrine-remediation-verification.log` (2026-11-03T13:14Z,
HEAD `d64468a7693e4962712cbf3416d767440d22a649` + uncommitted scoped edits).
Cargo invocations went through `/tmp/sk-focused-cargo.sh` (isolated
HOME/XDG/TMP, shared Cargo lease, `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`).
The oracle is selected fail-closed from the absolute
`<repo>/.venv-rust-oracle/bin/python` and verified lock-exact through
`scripts/rust/verify_oracle.py`; `PYTHONPATH=<repo>/src`,
`SPEC_KITTY_PACKS_ROOT=<repo>/packs`.

| check | result |
|-------|--------|
| `cargo test -p sk-doctrine --locked` | **138 passed, 0 failed** (106 unit + 18 `baseline_fixtures` + 12 `governance_oracle` + 2 doctests) |
| `cargo test -p sk-doctrine --locked --test governance_oracle` | **12 passed, 0 failed** (real oracle subprocesses; 11.5 s) |
| `cargo test -p sk-install --locked --lib activation::` | **7 passed, 0 failed** at 13:11Z; see the cross-owner build note |
| `cargo clippy -p sk-doctrine --all-targets --locked -- -D warnings` | pass (exit 0) |
| `cargo fmt -p sk-doctrine -- --check` | pass (exit 0) |
| `rustfmt --check --edition 2024 crates/sk-install/src/activation.rs` | pass (exit 0) |

New differential evidence added here (both pass): two oracle-settled regressions for
the two context unit expectations, see "Unit expectations" below. Files added/changed
in this recovery: `tests/governance_oracle.rs` (two tests), `README.md` (accuracy),
`REMEDIATION-REPORT.md` (this file). `src/**` and `activation.rs` were preserved as the
cancelled owner left them, after per-finding source verification.

Scope-relevant hashes at report time:

```text
45b17d33…9c1d  crates/sk-doctrine/src/activation.rs   (unchanged vs reviewed hash)
b7d661eb…355a  crates/sk-doctrine/src/org_charter.rs
a9ad216d…6dee  crates/sk-doctrine/src/context.rs
71e3d828…f688  crates/sk-doctrine/src/body.rs
4f5db7a9…02aa  crates/sk-doctrine/src/profile.rs
30471ce6…aae8  crates/sk-doctrine/src/plan.rs
c92f93fe…be89  crates/sk-doctrine/tests/governance_oracle.rs
1bafad7c…ff5f  crates/sk-install/src/activation.rs
```

## Second remediation (N1–N4 + packs-root floor)

An independent review of this remediation (`/tmp/sk-rust-doctrine-remediation-review.md`)
found four residual parity defects — absent-key org/project catalogue defaults
(N1), retired/malformed governance handling (N2), label-first typed titles (N3)
and diagnostics cardinality/wording (N4) — plus a known leaf packs-root floor
normalization risk. All are fixed and pinned by new live-oracle regressions in
[`SECOND-REMEDIATION-REPORT.md`](SECOND-REMEDIATION-REPORT.md). The checkpoint
evidence below is preserved as executed for its own source state.

## Follow-up: non-Git project-governance provenance (fixed)

**Provisional-handoff note:** this section was added after the checkpoint above;
the checkpoint table stays as executed evidence for that source state.

Residual 1 of this report is now fixed, not accepted. Full detail, red-first
history and final hashes: [`NON-GIT-REMEDIATION-REPORT.md`](NON-GIT-REMEDIATION-REPORT.md).

- Source: `crates/sk-doctrine/src/context.rs` gained one canonical-root seam
  (`CharterConfigRead` over `sk-kernel`'s `git_common_dir`, the same primitive
  `charter.resolution.resolve_canonical_repo_root` wraps). When it is
  unavailable, `resolve_directives` returns the reference's documented degrade
  — `all_directives: []` (the loader shares the same gate, so nothing was
  directly loaded), `directives_source: null`, no resolution diagnostics — and
  `governance_reference_status` returns `[]`. The paradigm
  `CATALOG_DEFAULT_APPLIED` diagnostic, which the reference emits from the
  resolver rather than the seed walk, is suppressed on the same path. Nothing
  else changes: typed arrays, links, nested bodies, the org floor and
  `project_charter` are untouched.
- Red-first evidence: two new non-Git cases and a Git control in
  `tests/governance_oracle.rs` failed on the pre-fix source
  (`directives_source` was `"activation"` / `"activation+charter+project_local"`
  where the oracle returns `null`) and pass after the fix. The Git control keeps
  `"activation+charter+project_local"` with the authored local directive.
- Unit fixtures that assert resolution metadata (`all_directives`, `directives_source`,
  governance references) now run in a real Git checkout. No deny or delivery
  assertion was weakened.
- Re-run at the follow-up source state: **141 passed, 0 failed** across
  `cargo test -p sk-doctrine --locked` (106 unit + 18 baseline + 15
  oracle-generated + 2 doctests), with
  `cargo clippy -p sk-doctrine --all-targets --locked -- -D warnings` and
  `cargo fmt -p sk-doctrine -- --check` clean. Log:
  `/tmp/sk-doctrine-nongit-remediation.log`.
- Scope-relevant hashes after the follow-up:

```text
1a006ea0…0283  crates/sk-doctrine/src/context.rs
96ee91d1…73b7  crates/sk-doctrine/tests/governance_oracle.rs
```

This is scoped evidence for one residual, not a new independent review or an
aggregate sign-off.

## Per-finding disposition

### F1 (HIGH) — activated / org-required roots not delivered: **FIXED**

Source: `crates/sk-doctrine/src/context.rs::build_action_context`. The bootstrap
candidate set is now `delivery.all_artifacts ∪ selected_closure(&filtered,
&seeds.roots, action_urn)`, and `selected_closure` starts from the selected roots
(`walk_edges` visits its roots), so a selected root is delivered without any other
artifact pointing at it. `governance_seeds` resolves each kind independently: absent
key → built-in catalogue default, explicit `[]` → empty, `required_<kind>` unioned on
top, effective deny subtracted last (never holding a required id), and the
reference's directive-slot guard is preserved.

Tests (all in `tests/governance_oracle.rs`, exact comparison):

- `selected_non_hub_tactic_is_delivered_inline_like_the_oracle` — the review's
  minimal repro A: a single activated non-hub tactic is delivered `inline` with its
  body.
- `catalogue_fallback_defaults_match_the_oracle_exactly` — the review's repro B
  state (no activation ledger at all): the whole directive and tactic catalogues are
  delivered with model-dump bodies. Probe of the equivalent built-in-only, git,
  ledger-absent project: oracle **115 tactics / 35 directives / 251 links**, native
  identical (the review saw 49 vs 114 before; its fixture carried a charter/ledger,
  hence 114 there — either way the comparison is exact, not a count match).
- `org_required_roots_are_independent_of_the_action_bundle` — repro C plus a control:
  with `required_tactics` the org floor nets the stale deny and the tactic is
  delivered; after removing the requirement the deny stands and it disappears (both
  sides).
- `explicit_empty_directive_allow_keeps_the_independent_tactic_catalogue` and
  `a_denied_tactic_only_removes_itself_from_the_catalogue_default` (new here).

### F2 (HIGH) — 5 built-in profiles silently skipped: **FIXED**

Source: `crates/sk-doctrine/src/profile.rs`. `PROFILE_KEYS` is now the full 40-entry
`extra="forbid"` surface including `self-review-protocol` **and**
`self_review_protocol` (plus the other populate-by-name spellings); `validate_profile`
accepts either spelling of `profile-id` and still refuses a profile declaring both
`role:` and `roles:`.

Tests: `every_pinned_builtin_profile_loads_like_the_oracle` (native ids == oracle ids,
**26** ids, `skipped() == []`, oracle `skipped == []`),
`profile_schema_accepts_alias_and_populate_by_name_spellings` (Python field-name
spellings load with the same `routing_priority`/`max_concurrent_tasks`/
`self_review_protocol.steps[0].gate`/`directive_references[0].code` as the oracle, and
the `role`/`roles` conflict is refused like the oracle).

Residual (other owner, review out-of-scope note): `ProfileRepository::skipped()` is
still not surfaced through the CLI path (`crates/sk-cli/src/compat/runtime.rs`), so a
future skip would remain unannounced on that surface.

### F3 (MEDIUM) — legacy-alias `success` records not settled: **OUT OF SCOPE, already fixed by the coordinator**

`crates/sk-install/src/ledger.rs` (not mine, untouched): `is_settled` compares
`record.id == spec.id` (the normalized id) and `records_for` normalizes legacy aliases
on read; the regression is
`review_regression_legacy_success_settles_by_normalized_id_not_identity`. Ledger and
migration catalog were not modified here, per the coordination instruction.

### F4 (MEDIUM) — one malformed `required_*` field discards the whole pack floor: **FIXED**

Source: `crates/sk-doctrine/src/org_charter.rs`. When strict parsing refuses a pack,
the lenient path re-reads the raw mapping (`raw_org_charter_document` →
`lenient_policy`) and takes each `required_<kind>` field independently
(`lenient_required_values`: absent/null ⇒ empty, a non-list or non-string member drops
**only that field**), while keeping `extends:`, `org_name` and `skill_namespace`, so
the pack's other kinds and its inheritance still contribute.

Tests: `malformed_required_field_keeps_the_rest_of_the_org_floor` — base
`required_directives`, overlay with a malformed scalar `required_directives` **and**
`extends: base` plus valid `required_tactics`; native tokens and per-pack provenance
equal the oracle for both kinds (`directive: ["DIRECTIVE_001"]`,
`tactic: ["acceptance-test-first"]`), and `deactivated_urns_with_floor` shows the
org-required tactic's floor entry nets the stale project deny. Unit coverage:
`lenient_floor_keeps_every_other_kind_and_the_extends_chain`.

### F5 (MEDIUM) — nested `body` was raw YAML: **FIXED**

Source: new `crates/sk-doctrine/src/body.rs::canonical_artifact_body`, consumed by
`context.rs::artifact_document`. It mirrors
`context_renderers/artifact_bodies.py`'s
`model_dump(by_alias=True, mode="json", exclude_none=True)`: default-factory fields
materialise (`[]` / `{}`), defaulted scalars appear, `null` optionals are dropped, and
nested models (tactic/procedure steps, `references[]`) dump recursively with their own
defaults. Untyped kinds return `None`.

Tests: `body::tests::{directive_body_materialises_empty_defaults_and_drops_nulls,
tactic_steps_and_references_are_dumped_as_nested_models,
procedure_step_actor_defaults_to_agent, untyped_kinds_have_no_model_dump}`, plus the
differential comparison, which now compares the **whole nested `body` object** for every
typed entry — the review's list of only-oracle defaults (`explicit_allowances: []`,
`references: []`, `applies_to_languages: []`, `tooling: {}`, `steps[].references: []`)
is now present on both sides, and the old `body.id`-only leniency is gone.

### F6 (LOW–MED) — top-level link set over-named excluded kinds: **FIXED**

Source: `context.rs` delivery table — only the five typed kinds plus `asset` and
activation-gated `glossary_pack` are inserted into `delivered`; `paradigm` (and other
resolved-but-undelivered kinds) is never a link target, while it still contributes
through the traversal roots. `link_references` names only delivered targets.

Test: the top-level `references` array is compared **exactly** (order and content) in
every differential case, so a native-only or missing link fails; the reviewed fixture's
four `{execution-lanes, scope}` / `{git-flow, scope}` / `{shared-branch-ci, scope}` /
`{trunk-based, scope}` extras are gone, and the non-vacuity assertion still requires a
non-empty realistic link set (251 links in the catalogue probe, identical both sides).

### F7 (LOW) — org block `pack_name` fallback: **FIXED**

Source: `context.rs::org_charter_block` falls back to the pack **directory basename**
(`root.file_name()`), never the configured display name, matching
`org_charter_loader.py`.

Test: `org_charter_block_pack_name_and_schema_acceptance_match_the_oracle` — an org
pack with no `org_name` yields `pack_name: "org"` on both sides (the whole block is
compared exactly).

### F8 (LOW) — `schema_version > 2` refused / allowlist strictness: **FIXED**

Source: `org_charter.rs::load_org_charter_policy` reads `schema_version` as an
integer/numeric string and then **folds any value** (`let _ = schema_version;`) instead
of version-gating at `1..=2`, matching the pinned model, which types it as an int and
does not gate on it. Unknown-field refusal is unchanged (the reference model is
`extra="forbid"`), and the lenient floor readers no longer go through the strict
validator (F4). Profile alias/populate-by-name spellings are covered under F2.

Test: same `org_charter_block_pack_name_and_schema_acceptance_match_the_oracle`
(`schema_version: 9` is read, `policy.schema_version == 9`, and the oracle's imported
block equals the native block), plus the F4 lenient cases.

### F9 (LOW) — evidence claims stronger than the fixtures: **DOCTRINE SIDE FIXED; migration side out of scope**

- `crates/sk-doctrine/README.md` verification section now states the real counts
  (138 / 106+18+12+2, 7 activation tests) and that the comparison is exact, listing
  exactly what is compared (typed arrays entered-for-entry including the whole body,
  the top-level link array by order and content, `all_directives`,
  `directives_source`, `project_charter`, `governance_references`, the org block, the
  26-profile catalogue, the floor/provenance, and the lock/contention case). The stale
  "native-only links allowed" / "every oracle link the native set must name" wording is
  gone.
- The `README.md` org-charter section claimed "schema versions 1–2" and did not
  mention the lenient per-field floor read; both corrected from source.
- The migration-report claims the review lists under F9 (`2 oracle cases` for 18
  retired ids, the `42 * 3` ledger floor) are in
  `crates/sk-install/MIGRATIONS-CONTINUATION-REPORT.md` /
  `tests/migrations_gitignore_oracle.rs` — installer-owned, unchanged here.

### F10 (LOW/INFO) — residual API/documentation hazards: **DOCUMENTED (in-scope items)**

- `plan::commit_plan` keeps its stale-write footgun as a compatibility API, but its
  doc comment now says explicitly that preparing from an older read and applying later
  re-applies stale full values over another writer's changes, and that new callers use
  `ActivationTransaction` (observe → plan → commit), which refuses with
  `precondition_changed`. No in-repo production caller besides the transaction; install
  uses the transaction.
- `org_charter::RequiredFloor::tokens` was documented as "raw ids"; it now documents
  normalized activation stems with the raw spelling living in
  `read_org_required_tokens`.
- `upgrade.rs`'s unparseable-version `continue` is installer-owned and unreachable on
  the current catalog; unchanged here.
- The documented lock-taking deviation (`provision_mission_type_activations` holds the
  canonical synthesis lock, as Python `charter activate` does) is intentional and
  covered by `the_write_uses_the_canonical_synthesis_lock_path` and the
  Python/native contention test.

## Unit expectations settled by Python, not by weakened deny rules

`docs/rust/VERIFICATION.md` recorded that adding selected roots exposed two further
unit expectations needing source-backed resolution. Both are now settled against the
live oracle rather than by inspection or by dropping assertions:

1. `context.rs::activation_filter_removes_deactivated_artifacts` — pre-fix it asserted
   `context.tactics.is_empty()` for a one-tactic deny. The oracle delivers the tactic
   catalogue default and only removes the denied id, so the test now asserts the denied
   id is absent. New oracle test
   `a_denied_tactic_only_removes_itself_from_the_catalogue_default` pins **both** halves
   (≥30 catalogue tactics on the oracle side, and the denied id absent on both sides)
   with exact payload equality.
2. `context.rs::explicit_empty_directive_allow_denies_directives_but_not_the_tactic_catalogue`
   — asserts an explicit `activated_directives: []` delivers no directive while the
   absent tactic key still yields the catalogue (`["hard","soft"]` in the unit fixture,
   inline). New oracle test
   `explicit_empty_directive_allow_keeps_the_independent_tactic_catalogue` reproduces
   the same state on a real project: native `directives == []`, the oracle's directive
   branch is the activation branch (not `catalog_fallback`), ≥30 tactics with bodies,
   and the exact payload match.

The coordinator's `all-26-profile`/`self-review` regression and the non-hub-root
regression are preserved and passing.

## Residual risks / open items (honest, unfixed here)

1. **Non-git project governance metadata — FIXED in the bounded follow-up.**
   Originally recorded here as unfixed: for a project root that is not inside a
   Git repository, the reference's `resolve_project_governance` raises
   `NotInsideRepositoryError` and the JSON builder degrades to
   `all_directives: []`, `directives_source: null`, `diagnostics: []`
   (`src/charter/activation/context_json.py::_load_project_directives_with_diagnostics`,
   verified by direct traceback), while native resolved the catalogue fallback
   and reported `catalog_fallback` plus two diagnostics. The follow-up traces
   both reference reads (`load_directives_config` shares the same canonical-root
   gate, so it raises first and no local directive is loaded), probes the
   already-available kernel topology seam, and reproduces the degrade exactly;
   the Git control keeps the resolved provenance. See
   [`NON-GIT-REMEDIATION-REPORT.md`](NON-GIT-REMEDIATION-REPORT.md).
2. **CLI wiring** (other owners): the CLI still passes an empty `required_floor` to
   `build_action_context` and loads profiles without active-language filtering; the
   library-level floor netting proven here is therefore not exercised by that CLI path.
3. **Migration breadth** (installer owner): 46/136 ids implemented, 90 correctly refuse
   before writing. Not re-checked or changed here beyond F3's out-of-scope note.
4. **Cross-owner build state at report time.** `cargo test -p sk-install` currently
   fails to **compile** because of concurrent edits by another owner in
   `crates/sk-install/src/init.rs` (`TemplateTreeReport` has no field `warnings`) and
   `crates/sk-install/src/skills.rs` (if/else branch type mismatch), both modified at
   13:14Z after my activation run; the owner was still editing at the 13:15Z re-check
   (4 errors, all in those files). `crates/sk-install/src/activation.rs` is unchanged
   from the reviewed hash and its 7 tests passed at 13:11Z on the same source; a rerun
   of that subset is pending the init/skills owner's compile fix. I did not edit those
   files.
5. **Single platform / single oracle**: Linux only, one pinned oracle closure; no
   macOS/Windows, aggregate, release or performance claim.

## Files changed by this recovery

- `crates/sk-doctrine/tests/governance_oracle.rs` — +2 oracle differential regressions
  (the two context unit expectations); no leniency added anywhere; the fixture helper
  and exact comparison functions are unchanged.
- `crates/sk-doctrine/README.md` — accurate verification counts and comparison scope;
  org-charter parse/floor description corrected from source.
- `crates/sk-doctrine/REMEDIATION-REPORT.md` — this report.
- Follow-up (non-Git residual): `crates/sk-doctrine/src/context.rs` (canonical-root
  degrade seam), `crates/sk-doctrine/tests/governance_oracle.rs` (+2 non-Git oracle
  cases and a Git control), `crates/sk-doctrine/NON-GIT-REMEDIATION-REPORT.md`
  (new), `crates/sk-doctrine/README.md` (non-Git degrade + 141-count accuracy).
