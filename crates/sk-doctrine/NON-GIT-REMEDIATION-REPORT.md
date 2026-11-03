# sk-doctrine non-Git project-governance parity remediation

**Status: FINAL — localized residual fixed, red-first, scoped evidence only.**
This is implementation evidence, not an independent review or an aggregate,
release, compatibility or performance sign-off.

**Scope owner:** localized doctrine residual recorded as item 1 in
[`REMEDIATION-REPORT.md`](REMEDIATION-REPORT.md). **Owned path:**
`crates/sk-doctrine/**` only. Installer activation, the CLI, workspace
manifests, `docs/rust/**` and runtime/status/workspace/init surfaces are other
owners' and were not modified here. No commit, push, subagent or production
Python change was made.

**Reference:** pinned Python oracle `9aed3a8012cb5eb22c0128b14d525039f95fea7a`,
selected fail-closed from `<repo>/.venv-rust-oracle/bin/python` and verified
lock-exact through `scripts/rust/verify_oracle.py`. Working tree HEAD:
`d64468a7693e4962712cbf3416d767440d22a649`.

## 1. The residual, traced end to end

`src/charter/activation/context_json.py::_load_project_directives_with_diagnostics`
runs **two** charter-config reads, both anchored on
`charter.resolution.resolve_canonical_repo_root`:

1. `load_directives_config(repo_root)` — the project-local `directives:` section;
2. `resolve_project_governance(repo_root)` — the directive set,
   `directives_source` and the resolution diagnostics.

`resolve_canonical_repo_root` delegates to
`kernel.git_topology.git_common_dir` and maps a non-repository to
`NotInsideRepositoryError`. The first `try` therefore **also** raises outside a
Git repository, before any local directive is loaded; the second `try` catches
the resolver failure and returns

- `local_by_id` / `directive_ids` from whatever was loaded before the failure —
  empty on this path, even when `charter.yaml` authors `directives:` locally;
- `directives_source = None` (provenance unavailable is itself the documented
  operator signal, per the function's docstring);
- `diagnostics = []`.

The sibling `_load_governance_charter_config` is best-effort with the same broad
catch, so `governance_references` is `[]` too. `project_charter` still describes
the authored file because `_bundle_root_for_json` falls back to `repo_root` and
reads it directly.

Observed oracle output for a non-Git temp root (no Git marker at all), with an
authored local `DIRECTIVE_009`, a `selected_directives` entry and two
`governance_references`:

```json
{"all_directives": [], "directives_source": null, "diagnostics": [],
 "governance_references": []}
```

…while typed delivery was untouched (35 directives, 115 tactics, 12
styleguides, 14 toolguides, 251 links with the same nested bodies). The
pre-fix native payload invented the catalogue resolution instead:
`all_directives` (38 entries), `directives_source: "catalog_fallback"` and two
`CATALOG_DEFAULT_APPLIED` diagnostics.

## 2. Fix (one canonical seam, no second resolver)

`crates/sk-doctrine/src/context.rs`:

- `CharterConfigRead` / `charter_config_read` probe
  `sk_kernel::git_topology::git_common_dir` — the canonical primitive
  `resolve_canonical_repo_root` wraps, already a dependency of this crate (it
  backs `synthesis_lock_path`). The variants keep an ordinary non-repository
  (`NotAGitRepository`) distinct from another discovery failure, but **both**
  take the degrade path, exactly as the reference's broad `except Exception`
  does; neither is treated as a reason to refuse the supported input.
- `resolve_directives` returns the documented degrade when the gate is
  unavailable: empty `all_directives`, `directives_source: None`, no
  diagnostics. It does not read or fabricate a catalogue for that root. The
  comment records that the reference's local loader shares the gate, so the
  directly-loaded set on this path is empty, and that a future native resolver
  failure which leaves the local read successful must return those local ids
  with a `null` source label rather than inventing provenance.
- `governance_seeds` suppresses the paradigm `CATALOG_DEFAULT_APPLIED`
  diagnostic on the same path. That diagnostic is emitted by the reference
  **resolver** (`_resolve_paradigm_base`), not by the seed walk; the seed walk's
  catalogue roots are a separate seam that does not need Git, so typed delivery
  is unchanged.
- `governance_reference_status` returns `[]` when the gate is unavailable,
  matching the best-effort governance read in the reference. This was found by
  the new red test (the authored references would otherwise have been reported
  natively).
- `build_action_context` computes the gate once per build and passes it to the
  directive and seed seams.

No manifest change, no second Git resolver, no new refusals, no speculative
schema validation, no production Python.

## 3. Red-first evidence

`crates/sk-doctrine/tests/governance_oracle.rs` grew three real-oracle
differential cases (the fixture helper now takes `git_repository: bool`):

1. `non_git_project_without_local_directives_degrades_like_the_oracle` — plain
   root, no charter file and no activation ledger. Exact payload comparison plus
   explicit `all_directives == []`, `directives_source == null`,
   `diagnostics == []`, `governance_references == []`, and non-vacuity: the
   typed catalogue is still delivered with bodies and a non-empty link set.
2. `non_git_project_with_local_directives_reports_no_resolution_provenance` —
   plain root with the authored local directive, charter selection and
   governance references present on disk. Exact payload comparison and the same
   degraded fields; the authored activation still delivers `DIRECTIVE_001` with
   its body, and `project_charter.present` stays `true`.
3. `git_control_resolves_the_same_authored_fixture` — the identical authored
   content inside a Git checkout keeps
   `directives_source == "activation+charter+project_local"`, the local
   `DIRECTIVE_009` with `source: "project"`, and both governance references.

The pre-fix run failed cases 1 and 2 exactly at the invented provenance
(`left: "activation"` and `left: "activation+charter+project_local"`,
`right: Null`), and passed the Git control. Post-fix all three pass; the
existing 12 oracle cases (whole nested bodies, top-level links, catalogue
fallback, selected non-hub tactic, org-required roots with a deny control,
explicit-empty allow, single-tactic deny, org-floor width, org-charter block,
26-profile catalogue, alias spellings, cross-language lock contention) are
unchanged and green.

Unit fixtures that assert resolution metadata were made realistic Git
checkouts rather than having their assertions dropped:

- `context::tests::{bare_bootstrap_action_succeeds_without_a_unique_node,
  compact_mode_carries_no_action_grain_but_keeps_governance,
  project_governance_labels_and_local_directives,
  governance_references_report_unsafe_and_missing_paths}` now call the new
  `git_root` helper. Deny/delivery assertions were not weakened; all previous
  full-body, floor and profile fixes are preserved.

## 4. Final verification

Log: `/tmp/sk-doctrine-nongit-remediation.log`
(`2026-11-03T13:27:27Z`, HEAD `d64468a7693e4962712cbf3416d767440d22a649` +
these uncommitted scoped edits). Every Cargo invocation went through
`/tmp/sk-focused-cargo.sh`: temporary HOME/XDG/TMP, shared Cargo lease,
`CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, retained Cargo cache only. No
aggregate, release, new target, extra manifest, background process, sleep or
poll loop.

| check | result |
|-------|--------|
| `cargo test -p sk-doctrine --locked` | **141 passed, 0 failed** (106 unit + 18 `baseline_fixtures` + 15 `governance_oracle` + 2 doctests) |
| `cargo test -p sk-doctrine --locked --test governance_oracle` | **15 passed, 0 failed** (real oracle subprocesses, 11.57 s) |
| `cargo clippy -p sk-doctrine --all-targets --locked -- -D warnings` | pass (exit 0) |
| `cargo fmt -p sk-doctrine -- --check` | pass (exit 0) |

Hashes after the fix:

```text
1a006ea07c18b72738372e40c9d288a3913967972bf0d6f694a86d5c8df00283  crates/sk-doctrine/src/context.rs
96ee91d1931380304b53e18c575e9baa0a915687da35bf956fbb32af6c2c73b7  crates/sk-doctrine/tests/governance_oracle.rs
a7a2a3e30f543c27764b019cfdb253171c55e670f3fafb17ce1c95599f73c79d  crates/sk-doctrine/README.md
79a9650457531783d3378cf08e4c6b6164ebbe1e14220736166cdddfb26db80c  crates/sk-doctrine/REMEDIATION-REPORT.md
```

## 5. Honest boundaries

- **Not covered at this checkpoint; CORRECTED by the second remediation
  pass.** A Git checkout where the *resolver* fails after
  `load_directives_config` succeeded. The provisional text here said the
  reference "returns the directly-loaded local directives with
  `directives_source: null`" for the retired `governance.doctrine` key; that was
  wrong. A direct trace shows `require_canonical_governance` raises
  `ActiveCharterConfigError` / `ACTIVE_CHARTER_CONFIG_INVALID` and the JSON
  build HARD-REFUSES (the governance-reference seam re-raises it). The
  local-only/null-source degrade is the *malformed governance shape* branch
  (`governance: []`, a scalar `charter`/`selected_directives`), which native now
  reproduces exactly. See
  [`SECOND-REMEDIATION-REPORT.md`](SECOND-REMEDIATION-REPORT.md) (N2) and the
  `retired_governance_doctrine_key_is_a_typed_refusal_not_a_degrade`,
  `retired_governance_key_on_a_non_git_root_degrades_like_the_oracle` and
  `malformed_governance_and_directives_shapes_degrade_to_local_only` oracle
  cases.
- Linux only, one pinned oracle closure; no macOS/Windows, aggregate, release,
  packaging or performance claim.
- Other owners were concurrently editing `sk-install`/`sk-cli`/runtime surfaces.
  Only `crates/sk-doctrine/**` was touched by this follow-up, so a whole
  workspace build is the coordinator's to run.
- This fix is implementation work. Whole-doctrine independent review is still
  required before acceptance.

## Files changed by this follow-up

- `crates/sk-doctrine/src/context.rs` — canonical-root gate and the three
  degraded seams; metadata-dependent unit fixtures made Git-realistic.
- `crates/sk-doctrine/tests/governance_oracle.rs` — 2 non-Git oracle cases, 1
  Git control, `fixture_with_git` helper.
- `crates/sk-doctrine/NON-GIT-REMEDIATION-REPORT.md` — this report (new).
- `crates/sk-doctrine/README.md` — non-Git degrade documented; 141-count
  accuracy.
- `crates/sk-doctrine/REMEDIATION-REPORT.md` — residual 1 marked fixed with a
  pointer; prior checkpoint evidence preserved.
