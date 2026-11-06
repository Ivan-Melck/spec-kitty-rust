# Performance / conformance certification infrastructure

Status: **infrastructure complete for this tranche; the performance gate is
`BLOCKED` and no speed claim is made.** This document records what is now
genuinely checkable, the exact integration the coordinator must apply, and every
required workload that is still absent. It is an engineering report, not an
acceptance result.

Checkpoint: `ff4452b02`. Working tree is dirty from concurrent owner work
(`crates/sk-*`), the release binaries predate tracked build inputs (the count changes as owners edit the crates), and there
is no build receipt. Nothing in this report binds a measurement to the current
tree; every probe was untimed conformance in temp directories.

## Verdict summary

| Question | Answer |
| --- | --- |
| Can a filtered subset, an edited report or two degenerate outputs pass? | No — see the negative matrix below; each case is exercised end-to-end. |
| Do the harness ids and floors match `check_completion.py`? | Yes, byte-for-byte; `tests/test_registry_unification.py` asserts it against the live modules. |
| Is a passing bundle possible today? | No. 16 of 17 strict completion workloads are absent natively; the gate blocks by construction. |
| Is any performance result certified? | **No.** The committed initial bundle is historical evidence with a superseded schema and stays `BLOCKED`; `bench.py gate` refuses it. |
| Is the one implemented mutation workload equivalent? | Yes, untimed: identical result JSON, identical `status.json` bytes, identical post-state delta on a fresh clone per invocation (`evidence/conformance/status-replay-mutation.json`). |

Observed in this tranche:

```
bench.py selftest                        49/49 passed
pytest benchmarks/rust/tests             60 passed
bench.py fixtures verify                 4/4 recipes portable, 0 failures
mutation_check status.replay             equivalent=true post_state_equal=true
                                         master_unchanged=true ignored_artifacts_differ=false
bench.py identity                        exit 1, status=drift (dirty tree, stale binaries)
bench.py gate evidence/2026-11-02-initial exit 2, BLOCKED (schema predates the bundle)
check_completion.py                      exit 1, performance gate False (unchanged, 27 perf findings)
```

## What the review asked for, and where it now lives

| Review finding | Mechanism | Test |
| --- | --- | --- |
| P0-1 strict equivalence vacuous for two-sided degenerate output | Registry-declared payload schemas (required/non-empty/equals/size/row keys) evaluated in `spkbench/expectations.py`; contract workloads also need a stdout schema. `_unparseable`, empty stdout and `{}` fail. | selftest `test_degenerate_payloads_are_rejected`, `test_two_sided_empty_output_is_not_equivalence`; pytest `test_degenerate_payload_fails_the_declared_schema` |
| P0-2 report forgeable, fail-open, subset can PASS | Gate recomputes every statistic from raw samples; bundle is self-hashed and artifact-hashed; frozen registry ids/classes/floors; fixed policy (no CLI knobs); a subset leaves missing rows and blocks. | 20+ selftests (`filtered_subset`, `missing_raw_samples`, `forged_speedup`, `forged_geomean`, `tampered_self_hash`, `registry_digest_tamper`, …) |
| P0-3 harness disconnected from checker | `docs/rust/inventory/performance-registry.json` is the shared registry; bundle is the checker's shape; `scripts/rust/performance_evidence.py` verifies and projects the `coverage.json` section. | `tests/test_registry_unification.py` (7 assertions against `inventory_model` and `check_completion`), `tests/test_gate_bundle.py::test_checker_adapter_*` |
| P0-4 stale/unverifiable binary provenance | `Cargo.lock`, build-input digest, toolchain, binary hashes and a **build receipt** are required; binaries older than any build input block; dirty trees block; oracle baseline (`9aed3a80…`), `verify_oracle.py` and repo `packs` are gated. | selftest `test_missing_build_receipt_blocks`, `test_identity_drift_blocks`; `bench.py identity` reports all three problems today |
| P1-4b/portable fixtures | Root-canonicalised content digests; two-root regeneration in `fixtures verify` and in `bench.py gate`. | pytest `test_fixtures_portable.py` (5 tests, incl. the absolute-`template_path` case) |
| P1-5 five samples vs 20 | Floor is 20 in the registry, enforced on both raw and paired counts; no flag can lower it. | selftest `test_insufficient_samples_blocks` |
| P1-6 startup substitution | Gate workloads cannot be startup commands; work-excess is recomputed and gated (≥1.25x per workload, ≥3x geomean, ≥5 ms window). | selftest `test_startup_only_apparent_win_fails`, `test_startup_dominated_work_blocks` |
| P1-7 no mutation/post-state/isolation | Runner mutation path: fresh clone per invocation, timer wraps only the command, per-sample post-state hash, pristine fixture verified, declared additions/changes/removals. | pytest `test_mutation_timing.py` (incl. a clone that sleeps; the sample must stay fast), `test_status_replay_mutation_equivalence` |
| P1-8 no uncertainty control | Paired randomized interleaving; paired bootstrap CI per workload; aggregate CI resamples pairs within workloads; uncertainty-aware regression verdict; load/CV floors block. | selftest `test_uncertain_regression_blocks`, `test_noisy_bundle_blocks`, `test_geomean_ci_below_target_blocks` |
| P1-9 raw-sample binding is attestation | `samples.jsonl` is hash-bound and re-read; every stored payload and post-state delta is re-parsed, canonical-hash-checked and schema-checked; the paired arrays are reconstructed from `pair_index`; `manifest.json` is verified. | selftest `test_deleted_payload_artifact_blocks`, `test_rewritten_payload_hash_fails`, `test_forged_paired_arrays_fail`, `test_forged_sample_fields_fail`, `test_missing_manifest_blocks`; pytest `test_evidence_binding.py` |
| P1-10 tests do not cover real failure modes | The 49 selftests drive `write_evidence`→stored artifacts→`evaluate_bundle`, not hand-built dicts; the positive control stores real payloads and uses a synthetic probe at the live-observation seam only. | `bench.py selftest` |
| P2-11 adapter field fidelity | Registry names each normalizer; `cli.mission.list` is explicitly a projection; versioned `--json` envelope is unwrapped rather than misread as a bare document. | pytest `test_rust_payload_accepts_all_documented_envelopes` |
| P2-13 identity checks fail-open | Oracle distribution probe raises instead of falling back to the harness interpreter; `PYTHONNOUSERSITE`; build provenance symmetric for both binaries. | `bench.py identity` output; selftest `test_missing_build_receipt_blocks` |
| P2-14 configurable gate / diagnostic exit | Floors are constants in the registry; the old `GateOptions` surface is removed from `spkbench/gate.py`; `--diagnostic` never changes the bundle verdict. | selftest positive/negative controls |

## Frozen registry

`docs/rust/inventory/performance-registry.json`

- 20 required ids, 17 strict gate ids, 2 startup ids — exactly the checker's sets.
- Policy: 3.0x, ≥20 samples/side, ≥14 strict workloads, ≤10% regression, 95% CIs,
  4000 bootstrap iterations, CV ≤0.30, median load1m ≤6 / spread ≤4, work-excess
  ≥1.25x per workload and ≥3x geomean, portable fixtures, build receipt, clean
  source, recomputation.
- Per workload: category, required/strict/gate flags, measurement kind, native
  route (`spec-kitty` or `spec-kitty-rust`), oracle adapter, declared normalizer
  and comparator, non-vacuous payload schema, and (for mutation) the post-state
  schema with an exclusion ruling.
- 16 entries are `measurement: "absent"` with a specific `blocked_by`. They have
  no argv, no adapter and no expectations, so they cannot be timed as equal
  refusals.
- The file is self-digesting; editing a floor or dropping an id fails
  `PerformanceRegistry.load`.

Registry digest after the R2 written-status identity correction and the R1
self-digest repair:
`sha256:82435983080c99693f9ce0c977bc7dffd7a4c6e581194e9583c4e7b70114700d`.
See [`INPUT-MANIFEST-REPORT.md`](INPUT-MANIFEST-REPORT.md); the earlier
`sha256:01841ff…` line referred to a previous checkpoint.

## Evidence bundle (checker-compatible)

`benchmarks/rust/evidence/<run>/performance-evidence.json`:

```
schema: spkbench.performance-evidence.v1
registry           {path, sha256, digest, workload_ids, gate_workload_ids, startup_workload_ids}
checker_contract   {min_samples_per_side, min_strict_workloads, min_speedup,
                    max_regression_fraction, required/gate/startup ids}
policy             the frozen policy
baseline           {commit, python_version, uv_lock_sha256, oracle_python_real_sha256,
                    packs_digest, oracle_verified}
candidate          {commit, source_clean, source_digest, cargo_lock_sha256, toolchain,
                    binaries{sha256,size,mtime,predates_source}, build_receipt}
identity_status    "exact" | "drift" (+ problems)
fixtures           {name: {recipe, file_count, content_digest, portable}}
environment        {machine, load, load_context, isolation, seed, repetitions, pairing}
workloads[20]      {id, category, required, strict_equivalence, equivalence, gate_eligible,
                    measurement, status, result_equivalent, fixture_id,
                    fixture_content_digest, command_argv, environment,
                                        samples{python[],rust[]},
                    sample_records{python[],rust[]},   # pair_index, duration, exit, timeout,
                                                      # payload/post-state hashes
                    paired{python[],rust[]}, sample_count, medians_ms, speedup,
                    paired_ci95, pair_ratios, cv, noise, work_excess, payload_sha256,
                    schema_problems, post_state, evidence_refs, floor_modules}
aggregate          {status, geometric_mean_speedup, geometric_mean_ci95,
                    work_excess_geometric_mean, strict_workload_count, sample_count,
                    evidence{path,sha256}}
startup            {excluded_from_gate, workload_ids, status, measurements}
artifacts          {report, samples, identity, environment, fixtures, summary: {path,sha256}}
payload_artifacts  {workload.side: {sha256, canonical_sha256, bytes, stored, path}}
post_state_artifacts {workload.side: {...}}
harness            {name, version, ..., code{repo-relative path: sha256}}
self_sha256
gate               {verdict, reasons, blockers, failures, recomputed counters}
```

`samples` are milliseconds (the unit `check_completion.py` already uses).
`sample_records` carry `pair_index` and the per-sample payload/post-state/exit
fields; the gate reconstructs the pair-aligned arrays from them and refuses a
`paired` array that disagrees. `manifest.json` hashes every artifact, payload and
post-state file; `summary.md`/`report.json` are the human view. Payload files may
be re-serialized: the declared hash is over canonical JSON of the parsed
document, not over the bytes.

## Exact completion-checker integration

The checker is owned by the coordinator; these are exact proposals, not applied
edits.

### 1. `scripts/rust/check_completion.py` — add the bundle cross-check

At the end of `_check_performance` (after the existing
`if performance.get("status") != "pass": report.fail(...)`), append:

```python
    # The bundle must actually exist, hash-match, re-derive these numbers and
    # carry a live-verifiable identity/build binding. Additive: it never relaxes
    # a rule above.
    import performance_evidence  # nearby module; HERE is already on sys.path

    performance_evidence.check_bundle(report, performance, root=root, gate=gate)
```

`check_bundle` reports through the checker's own `Report.fail` surface with these
codes (in addition to the checker's existing ones):

`PERF_REGISTRY_UNREADABLE`, `PERF_REGISTRY_DRIFT`, `PERF_EVIDENCE_SCHEMA`,
`PERF_BUNDLE_SELF_HASH`, `PERF_BUNDLE_REGISTRY`, `PERF_EVIDENCE_INCOMPLETE`,
`PERF_EVIDENCE_UNEXPECTED`, `PERF_BUNDLE_NOT_CERTIFIED`, `PERF_POST_STATE`,
`PERF_WORK_EXCESS`, `PERF_STATUS_NOT_BACKED`, `PERF_SECTION_BUNDLE_MISMATCH`.

It also cross-checks every coverage row's samples/status against the bundle row,
so the checker's own arithmetic loop cannot be fed numbers the bundle does not
contain. A test exercised this through a real `check_completion.Report()`:
clean section → 0 errors; tampered section → `PERF_SECTION_BUNDLE_MISMATCH`
(tagged `gate=performance`).

Note: `check_bundle` runs the full `certify.evaluate_bundle` with
`require_live_identity=False` (the checker already pins `uv.lock`, the oracle
environment and dependency drift). Live identity is re-verified by
`bench.py gate`, which the certification runbook uses before publishing evidence.

### 2. `docs/rust/inventory/coverage.json` — replace the performance section once

After a real campaign on a clean commit:

```sh
python3 scripts/rust/performance_evidence.py emit \
  --bundle benchmarks/rust/evidence/<run>/performance-evidence.json \
  --out /tmp/performance-section.json
```

Apply `/tmp/performance-section.json` as the `performance` object (its `policy`
is already `inventory_model.PERF_POLICY`). `build_inventory._preserve_performance`
then keeps the measured rows across later rebuilds. Verify with:

```sh
python3 scripts/rust/performance_evidence.py review \
  --coverage docs/rust/inventory/coverage.json \
  --bundle benchmarks/rust/evidence/<run>/performance-evidence.json
```

Until a bundle's verdict is `PASS`, `emit` produces a non-passing section
(`status: blocked|failed|pending`), never `pass`.

### 3. `docs/rust/VERIFICATION.md` — count corrections (documentation owner)

- "Benchmark harness self-tests: **19 passed**" → **49 passed**
  (`python3 benchmarks/rust/bench.py selftest`, 0 failed; the
  evidence-binding tranche expanded the matrix).
- "untimed adapter/fixture tests: **46 passed**" → **60 passed**
  (`pytest benchmarks/rust/tests -q`; needs `pytest` + `pyyaml` in a dev venv).
- The benchmark performance paragraph points at this report and at
  [EVIDENCE-BINDING-REPORT.md](EVIDENCE-BINDING-REPORT.md), and keeps the
  explicit "no timing campaign on the current source" caveat.

### 4. Optional: `inventory_model.PERF_WORKLOADS` notes

The registry already mirrors ids/flags; the coordinator may copy the
`blocked_by` text into the curated notes for `state.event-append`,
`state.move-task`, `state.decision.resolve`, `runtime.next.governed`,
`governance.charter`, `glossary.activation` and `workflow.*` so the inventory
and the registry tell the same story.

## Remaining absent workload adapters

Each of these must be implemented natively **and** given a shared contract before
it can be registered. None is faked here.

| Workload | Missing capability | What a real adapter requires |
| --- | --- | --- |
| `state.event-append` | Native `agent status emit` is an explicit refusal (no canonical writer) | Canonical event writer under the mission lock, exact upstream envelope, durable append+readback, and an upstream adapter that appends the same event |
| `state.move-task` | Native `agent tasks move-task` refuses | Canonical status mutation through the CLI (same envelope/lock/readback) |
| `state.decision.resolve` | No native decision-ledger mutation/repair | Decision ledger write path + repair semantics |
| `runtime.next.governed` | Native `next` lacks mission-board progress, workspace resolution, decision-log commits, lifecycle records, retrospective hooks | Full governed `next` on both sides, including prompt files and gates |
| `governance.charter` | No native charter activation/pack acquisition | Activation ledger + pack acquisition execution with an upstream-equivalent contract |
| `glossary.activation` | Native glossary is validate+lookup only | Event replay/extraction/middleware/activation mutations |
| `workflow.mission-create` | Native skips charter activation, `mission_number`, planning commit | Full create flow (identity, numbering, commit) |
| `workflow.specify` | Partial scaffold, no refusal/commit contract | Full specify flow |
| `workflow.plan` | No substantive-spec gate or planning commit | Full plan gate |
| `workflow.tasks` | Different refusal contract; no requirement mapping/lanes refresh/events | Full finalize flow |
| `workflow.implement` | `MISSING_LANES`; no branch/dependency preflight, workspace persistence, auto-commit | Full lane allocation + workspace ownership |
| `workflow.review` | No native review workflow | Review claim/result/approval-stamp/evidence-window flow |
| `workflow.accept` | Explicit native refusal | Acceptance gate engine |
| `workflow.consolidate` | Explicit native refusal | Lane merge + rollback + preflight engine |
| `workflow.merge` | Explicit refusal (renamed to consolidate) | Same engine with the merge entry point |
| `workflow.recovery` | No native recovery workflow | Interrupted consolidation / stale worktree / unexplained-move recovery |

`status.replay` is the only strict gate workload with a native implementation; it
is proven equivalent and registered as a timed mutation workload. It becomes gate
evidence only after ≥20 paired samples, a clean commit and a build receipt.

## Mutation post-state ruling (recorded, narrow)

Status lock artifacts are implementation-private and ephemeral; upstream uses
`.git/spec-kitty-locks/<slug>.status.lock`, native uses a hashed name in the same
namespace. They are captured per invocation under `ignored_artifacts`, never
asserted equal, and excluded from the equality manifest **only**. Every
user-facing byte, including `status.json`, is compared byte-for-byte; declared
additions/changes/removals are exact; the pristine fixture is re-verified. The
ruling text lives in the registry next to the workload so it cannot be widened
silently.

Conformance proof: `evidence/conformance/status-replay-mutation.json`
(2 invocations/side, `event_count=2800`, 200 WPs, `equivalent=true`,
`post_state_equal=true`, `master_unchanged=true`,
`ignored_artifacts_differ=false`). It is explicitly **not certification** and is
bound to the stale binary with that caveat recorded.

## Negative / end-to-end test matrix

`python3 benchmarks/rust/bench.py selftest` (49):

positive control; absent required workflow; filtered subset; missing raw samples;
n=19; missing build receipt; identity drift; degenerate payloads; two-sided empty
contract; schema-failed row; forged speedup; forged geomean; tampered self-hash;
tampered raw-samples artifact; startup command as work; startup-dominated work;
projection-only; projection promoted to gate; material regression; uncertain
regression; noisy load; sub-3x geomean; geomean CI reaching below 3x; registry
digest tamper; vacuous schema; extra required workload; absent workload with
samples; mutation without post-state; recorded equivalence diff; plus
comparator/normalizer unit checks and the required-environment assertion (fails
when the oracle or binaries are missing).

`pytest benchmarks/rust/tests` (60): adapters and equivalence
(`test_compat_workflows.py`, 17), registry/checker unification
(`test_registry_unification.py`, 10), fixture portability
(`test_fixtures_portable.py`, 7), bundle write/verify/adapter and stored-evidence
binding (`test_gate_bundle.py`, 14; `test_evidence_binding.py`, 8), mutation
timing isolation (`test_mutation_timing.py`, 4).

## Reproduction runbook for the coordinator

```sh
# 0. Clean commit; no concurrent owner edits.
git status --porcelain        # must be empty
# 1. Build both executables and bind them to the source.
python3 benchmarks/rust/bench.py build --receipt target/release/build-receipt.json
# 2. Prove the environment and the harness.
python3 benchmarks/rust/bench.py identity      # must exit 0 with status=exact
python3 benchmarks/rust/bench.py selftest      # 49/49
python3 benchmarks/rust/bench.py fixtures verify
# 3. Campaign on an idle machine (frozen load floor).
python3 benchmarks/rust/bench.py run --oracle-python .venv-rust-oracle/bin/python --reps 20
# 4. Independent re-verification (live identity + fixture rebuild).
python3 benchmarks/rust/bench.py gate benchmarks/rust/evidence/<run>
# 5. Publish the coverage section / checker integration only if step 4 is PASS.
python3 scripts/rust/performance_evidence.py emit --bundle <run>/performance-evidence.json
```

Expected today: step 1 succeeds only after a rebuild; step 3 exits 2 (`BLOCKED`)
because 16 required workloads are absent natively, even if every other condition
is met. That is the correct behaviour, not a harness defect.

## Residual risks / honest limits

- **No acceptance claim.** No geometric mean, speedup or no-regression result is
  claimed for the current source.
- Workflow workloads have no adapters at all; when they land, each needs a
  declared payload schema (not just exit codes) and, if mutating, a post-state
  schema with a written exclusion ruling.
- Work-excess uses recorded floor measurements; the *use* is recomputed from the
  samples, but the floor values themselves are harness-recorded metadata. The
  same is true of `environment.isolation`: it states the intended isolation, it
  does not measure it.
- The remaining infrastructure gaps closed by the follow-up tranche are listed in
  [EVIDENCE-BINDING-REPORT.md](EVIDENCE-BINDING-REPORT.md): stored-payload
  binding, pair reconstruction, live-native binding in the checker (no
  `require_live_identity=False` path), harness/adapter hashing, projection
  recomputation and typed malformed-evidence handling.
- The gate re-verifies live identity, artifacts and fixtures, and `run` re-collects
  identity after the measurement window (the bundle is verified against that
  fresh snapshot). It still cannot prove a human did not edit code between the
  end snapshot and the later `bench.py gate`; the build receipt, binary-source
  freshness, start/end stability and the independent gate re-check bound that
  window.
- Only Linux has been exercised. macOS/Windows equivalence remains a delivery
  gate.
- Historical `evidence/2026-11-02-initial/` is retained unchanged and stays
  `BLOCKED`; it is not retrofitted into the new format.
