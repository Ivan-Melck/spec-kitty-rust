# Semantic evidence binding — B2 / B3 / H1 tranche

Status: **B2, B3 and H1 are implemented and covered by red-first controls in
the current working tree. The performance gate remains `BLOCKED` and no speed,
throughput, no-regression or readiness claim is made.** The source tree is dirty
from concurrent owners, the release binaries predate the current build inputs,
there is no `target/release/build-receipt.json`, and 16 of the 20 required
production workloads have no native implementation. No measurement campaign was
run in this tranche.

This report was created provisionally before the implementation and is now
updated in place with the real results.

## Scope and preserved state

Owned and edited: `benchmarks/rust/**`,
`scripts/rust/performance_evidence.py`,
`scripts/rust/tests/test_check_completion.py`,
`scripts/rust/tests/support/**`.

Not edited: `scripts/rust/check_completion.py`,
`scripts/rust/inventory_model.py`, the frozen production registry,
`coverage.json`, docs/manifests, and every Rust crate. No `cargo`, build,
release, timing command, new checkout, commit or push was part of this tranche.

The coordinator's small fixes are preserved and covered:

* **B2b** — parsed `fixtures.json` must equal `bundle["fixtures"]`
  (`evidence._load_fixtures`), with a negative control.
* **J1** — a collector error can never return `verdict=PASS`.
* **I1** — each row's stored `command_argv` must equal the frozen
  `native.argv`/`oracle.argv`; the coverage section must equal the bundle. The
  harness really records the canonical `CommandSpec.template` (the registry
  argv), not an expanded per-run command, so the comparison is exact and no
  expansion was invented.

All prior closures (C1 stored payloads, D1 raw pairs, E1 projection regression,
F1 emit, G1 aggregate projections, B1 live identity) still pass. Policy floors
are unchanged: 20 required ids, 17 strict gate ids, 20 samples per side, 3.0x
geomean, 14 strict workloads, 10% per-workload regression allowance. The
production registry still has 20 required / 17 gate / 16 `measurement:
"absent"` rows, and no absent row was replaced, renamed or given samples. Its
bytes changed after this tranche for the coordinator's R2 written-status
identity correction; the R1 follow-up repaired the then-stale self-digest. The
current registry hash is recorded in
[`INPUT-MANIFEST-REPORT.md`](INPUT-MANIFEST-REPORT.md).

## Finding disposition

| Id | Area | Disposition |
| --- | --- | --- |
| **B2** | Row fixture truth was circular | **Closed in current bytes.** |
| **B2b** | `fixtures.json` content unbound | **Closed (preserved).** |
| **B3** | Post-state was equality/self-hash only | **Closed in current bytes.** |
| **H1** | Chronology/interleaving claim unvalidated | **Closed in current bytes.** |
| **I1** | Row `command_argv` unbound | **Closed (preserved).** |
| **J1** | `verdict=PASS` with `ok=false` | **Closed (preserved).** |

## B2 — the checker regenerates fixtures by default

`performance_evidence._check_bundle` now calls the shared verifier with
`verify_fixtures=True`. `bench.py gate`, `bench.py run`, `emit`/`validate` and
the completion checker therefore run the same fixture observation.

The observation is `certify._verify_fixtures`: every recorded fixture is
regenerated **twice at different audit roots** through the real
`FixtureBuilder` from the frozen recipe + seed. It compares:

* the two-root portable content digest with the recorded `content_digest`
  (so a self-consistent forged table fails),
* the regenerated `recipe` facts with the recorded recipe (including the
  recorded event/WP counts and git version), and
* the regenerated file count.

Unit-scoped synthetic fixtures use an explicit observation seam,
`FIXTURE_OBSERVER` in `scripts/rust/performance_evidence.py` /
`fixture_observer` in `certify.evaluate_bundle`, with the same output shape as
the real regeneration. This is the same principle as `NativeSourceProbe`: only
the *observation* is injected, every comparison still runs. There is **no**
bundle flag (`harness.synthetic`, `environment.synthetic`,
`registry.synthetic`), custom registry or asserted verdict that turns the check
off; a control sets all three flags and still gets blocked.

The checker's synthetic positive control now records the *generated* digests of
the real `compat-project` and `replay-project` recipes plus the synthetic unit
fixture, and the checker re-verifies all three. Evidence:
`fixture_verification: regenerated`, with
`compat-project`, `replay-project` and `synthetic-checker-fixture` all
`portable=True, matches=True, recipe_equal=True`.

## B3 — post-state is re-derived from stored bytes

The writer now stores, per mutation side, a `spkbench.post-state.v1` envelope
(`post-state/*.json`, content-addressed exactly like a payload document):

```json
{
  "schema": "spkbench.post-state.v1",
  "delta":     {"added": [...], "changed": [...], "removed": [...],
                "added_sha256": {...}, "changed_sha256": {...}, "removed_sha256": {...}},
  "before":    {"<path>": "<sha256>"},      // full content manifest
  "after":     {"<path>": "<sha256>"},      // full content manifest
  "params":    {"mission_slug": "..."},     // substituted registry params
  "files":     {"<required path>": {"bytes": N, "sha256": "...",
                                     "canonical_sha256": "...", "content_b64": "..."}}
}
```

`spkbench.runner` records the full before/after manifests, the delta, the params
and the base64 bytes of every schema-constrained file
(`expectations.required_post_state_paths`, i.e. `required_paths` plus
`json_payload_key`). `spkbench.evidence` decodes them and binds each file's
stored bytes to its declared hash, byte length and canonical JSON hash.
`spkbench.certify` then calls the **single** evaluator
`spkbench.expectations.validate_post_state` with the stored files instead of a
live tree, so the measurement path and the evidence path apply the same rules:

* `required_paths` must be present **and** their stored content must hash to the
  after-manifest entry (naming a path is not evidence of what was written);
* `min_bytes` is measured on the actual stored bytes;
* the `json_payload_key` document must satisfy `required_keys`, `equals`
  (e.g. `event_count: 2800`, `summary.done: 200`), `min_items`
  (e.g. `work_packages >= 200`) and the rest of the frozen payload schema;
* `unchanged_paths` must exist on both sides with equal hashes;
* the delta must be **exactly** the difference between the stored manifests
  (no missing, extra or duplicated paths, and the `*_sha256` tables must match);
* every delta path must still be an allowed `expected_additions` /
  `expected_changes` / `expected_removals` entry, no ignored artifact may leak
  into the equality manifest, and every manifest value must be a real content
  digest (or a recorded symlink target);
* the stored `params` must equal the frozen registry params, the two sides must
  agree on the delta, and each sample's `post_state_sha256` must equal the
  canonical hash of that stored delta.

The synthetic positive control did **not** lower the registry expectations. For
`status.replay` it writes real generated `status.json` bytes (≥ 2000 bytes,
`event_count 2800`, 200 work packages, `summary.done 200`) and re-derives them
through the unchanged production `post_state` schema. A test asserts those
properties and the manifest binding directly.

Reformat tolerance is preserved: the stored envelope is a document whose
declared hash is over parsed canonical JSON, so pretty-printing it is not an
edit (control `test_pretty_printed_post_state_envelope_still_verifies`). The raw
required-file bytes are still bound to the exact after-manifest hash the
implementation produced.

## H1 — global observation chronology

`order` is the `Executor` invocation counter. It increments for floor probes,
preflights, warmups and measurements, so valid values are a **global** counter
with an offset — not `0/1` per workload. The verifier now requires, per measured
workload:

* one integer, positive `order` per sample, unique **across the whole run**;
* exactly one rust and one python record per `pair_index`;
* the two orders of a pair are adjacent (difference 1);
* pair order strictly increases with `pair_index`;
* each raw `samples.jsonl` row's `order` equals its bundle record;
* the run declares `pairing: interleaved-randomized-within-pair`.

Pairs are reconstructed from `pair_index` **ordered by the recorded global
`order`**, never by the position a row happens to have in `samples.jsonl` (that
file groups rows by side). The synthetic scaffolds now allocate a global,
gap-aware counter starting after a floor/preflight offset, so the positive
controls exercise a valid chronology instead of resetting `order` per workload.

## Controls run (real results)

Environment: fresh temp CWD/HOME/XDG/TMP under `/tmp/skimpl1`,
`PYTHONPATH=<repo>:<repo>/src`, `SPEC_KITTY_PACKS_ROOT=<repo>/packs`,
`PYTHONDONTWRITEBYTECODE=1`, oracle
`.venv-rust-oracle/bin/python` (CPython 3.11.15), pytest from
`/tmp/spkbench-pytest/bin/python`. Reviewed tree: `HEAD d64468a76` (`rust/rewrite`)
plus the concurrent owners' uncommitted work; no commit or push was made.

| Command | Result |
| --- | --- |
| `pytest benchmarks/rust/tests -q` | **78 passed** in 31.6 s (was 61 before this tranche; `test_evidence_binding.py` 22, `test_gate_bundle.py` 18) |
| `python -m unittest scripts.rust.tests.test_check_completion` | **51 passed** in 67.2 s (was 47 + the coordinator's coverage test) |
| `python -m unittest scripts.rust.tests.test_verify_oracle scripts.rust.tests.test_evidence_roots` | **8 passed** |
| checker/oracle surface | **59 passed** (51 + 5 + 3) |
| `bench.py selftest` | `ok=true total=49 failures=0` |
| `scripts/rust/verify_oracle.py` | `ok=true`, 53 checked, 0 drift, 0 missing, CPython 3.11.15 |
| `git diff --quiet 9aed3a80… -- src` | exit 0 (the oracle `src` is the pinned baseline) |
| `check_completion.py --json-report` | **exit 1**, 599 errors, **27 `PERF_*`**, `summary.ok=false`, no performance section |
| frozen production registry | 20 required / 17 gate / 16 absent; policy 3.0x / 20 / 14 / 10% (bytes changed by R2; hash in the R1 report) |

New red-first controls (all failed before the implementation and pass now):

* `tests/test_evidence_binding.py`
  * `test_consistent_fixture_table_forgery_is_rejected` (B2)
  * `test_fixture_verification_cannot_be_disabled_by_bundle_flags` (B2)
  * `test_post_state_delta_is_re_derived_from_the_registry_contract` (B3:
    empty and wrong-path deltas)
  * `test_post_state_path_without_content_is_rejected` (B3)
  * `test_post_state_unexpected_addition_is_rejected` (B3)
  * `test_post_state_required_file_bytes_are_bound_to_the_manifest` (B3)
  * `test_pretty_printed_post_state_envelope_still_verifies` (reformat rule)
  * `test_sample_order_must_be_a_global_unique_counter` (H1)
  * `test_pair_observation_orders_must_be_adjacent` (H1)
  * `test_observation_orders_are_unique_across_workloads` (H1)
  * `test_raw_sample_order_must_match_the_bundle_record` (H1)
* `tests/test_gate_bundle.py`
  * `test_checker_adapter_verifies_fixtures_by_default` (B2; the real
    `FixtureBuilder` runs when no seam is injected and blocks the unit recipe)
  * `test_checker_adapter_rejects_a_consistent_fixture_forgery` (B2)
  * `test_checker_adapter_rejects_post_state_contract_forgery` (B3)
* `scripts/rust/tests/test_check_completion.py`
  * `test_bundle_fixture_table_forgery_is_rejected` (B2)
  * `test_bundle_post_state_contract_forgery_is_rejected` (B3)
  * `test_bundle_sample_chronology_forgery_is_rejected` (H1)

### Independent bounded probe

`/tmp/skimpl1/probe_tranche.py` builds the 20-id synthetic certification through
the real `certify_run` → `write_evidence` → `check_bundle` path (unit-scoped
registry at the load seam, synthetic probe/fixture observation only) and then
forges the bundle consistently. Observed output:

```
base verdict PASS
fixture verification regenerated
fixture outcomes {'compat-project': (True, True, True),
                  'replay-project': (True, True, True),
                  'synthetic-checker-fixture': (True, True, True)}
green ok True errors 0
B2 ok False  -> fixture compat-project/replay-project/synthetic-checker-fixture
                do not match their recorded content digest
B3 ok False  -> status.replay: <side> post-state delta added does not re-derive
                from the stored manifests (missing ['…/status.json'])
H1 ok False  -> <wid>: <side> pair N order 0 is not a positive executor counter;
                … is already used by …; the executor observation order is global
default-fixture-check ok False -> fixture regeneration failed:
                KeyError: "unknown fixture recipe 'synthetic-checker-fixture'"
```

The last line is the positive proof that the checker regenerates by default: with
no observation seam the real `FixtureBuilder` runs and refuses the unit-scoped
recipe, so the bundle cannot pass silently.

## Remaining obligations / disclosures

1. **No production implementation or measurement.** 16 strict workloads remain
   `absent` and block the gate by construction; no fake adapter or equal
   refusal was introduced. There is still no build receipt and no current-source
   measurement, so the performance gate is `BLOCKED` for those honest reasons.
2. **Synthetic controls are not production evidence.** The unit-scoped registry
   keeps the same 20/17/2 ids and frozen policy but relabels only the 16 absent
   rows as *simulated* read workloads (`SYNTHETIC TEST SCAFFOLDING` notes). It is
   never written to the production registry or `coverage.json`, and it cannot
   produce a production verdict.
3. **Fixture regeneration is now on every write/verify path.** `bench.py run`,
   `bench.py gate`, `emit`/`validate` and the checker all rebuild fixtures at two
   roots, so a run takes longer than before. That cost is outside the timed
   window and is the price of not trusting a recorded table.
4. **No valid bundle exists yet.** The historical
   `evidence/2026-11-02-initial/` bundle stays `spkbench.report.v1` and is
   refused; no bundle was retrofitted to the new post-state envelope.
5. **Lint was not run in this environment** (`ruff` is not installed in the
   available interpreters). The changed files compile (`py_compile`). The
   `validate_post_state` split into helpers was done to keep the complexity
   contribution small, but a ruff/CI lint pass is still owed.
6. **Cross-platform and delivery gates are untouched.** Only Linux-Python
   harness logic was exercised; macOS/Windows, native builds, release packaging
   and cross-platform evidence remain delivery work.
7. **Recorded-but-not-measured metadata is still disclosed.** `isolation`,
   `load_context` and the floor table remain harness-recorded claims; their *use*
   is re-derived from samples, but a consistently fabricated bundle could still
   choose consistent values for the parts that are not re-observable. The live
   source, binaries, receipt, registry, oracle, packs, harness code and now
   fixtures are re-observed.

## No claim

No performance claim of any kind is made. There is no speedup, geometric mean,
confidence interval, no-regression result, native-completeness statement or
release-readiness statement in this report. The only quoted numbers are test
counts, error counts and deterministic synthetic control outcomes.
