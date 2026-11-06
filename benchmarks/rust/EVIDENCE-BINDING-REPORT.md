# Evidence-binding tranche — end-to-end verification layer

Status: **infrastructure tranche complete; the performance gate remains
`BLOCKED` and no speed, throughput or no-regression claim is made.** This report
records what changed relative to the independent review
(`/tmp/sk-rust-benchmark-certification-review.md`), the controls that now exist,
the exact integration the separate test owner must apply, and what is still
outstanding.

Checkpoint: `HEAD = c60ff00b461a5f289b1a61b7dfaa2a6e683bcda3`. The working tree is
dirty from concurrent Rust/harness owner work, both release binaries predate the
tracked build inputs, no `target/release/build-receipt.json` exists, and 16 of
the 17 strict completion workloads have no native implementation. No timing
campaign, native build, Cargo target, manifest or commit was produced by this
tranche. Every probe ran with `HOME`/`XDG_*`/`TMPDIR` under `/tmp` and wrote only
to temporary directories.

## What the review asked for, and what now enforces it

The review's verdict was "not yet certification-ready, independent of the 16
absent workloads", with six infrastructure blockers. Each is closed by deriving
the fact from stored bytes or from a live observation, not by adding a boolean.

| Finding | Mechanism now | Positive control | Negative controls |
| --- | --- | --- | --- |
| **P0-1** `check_completion` accepts evidence from a stale/different source; `require_live_identity=False` | The checker always observes the live checkout through `spkbench.live.NativeSourceProbe`: HEAD (or a proven evidence/docs-only descendant), build-input digest, `Cargo.lock`, toolchain text, both binary hashes, a re-verified build receipt, the frozen registry file, `uv.lock`, the repo `packs` digest and the live oracle (`verify_oracle.py` + pinned `src` baseline). A recorded `live_identity` is ignored with a warning. | 20-id synthetic positive control (below) and `test_positive_control_passes` | `test_stale_candidate_commit_fails`, `test_descendant_changing_build_inputs_blocks`, `test_dirty_measured_input_blocks`, `test_dirty_docs_do_not_block`, `test_harness_code_change_blocks`, `test_checker_adapter_binds_live_native_source`, `test_live_source_mismatch_fails` |
| **P1-2** missing/rewritten normalized payload artifacts still pass | Every payload is stored; its declared `canonical_sha256` is recomputed from the *parsed* document, its frozen schema re-applied, and equivalence re-derived by comparing the two stored payloads (or the stored stdout contract). `manifest.json` and its payload/post-state/artifact tables are verified against the files. | `test_pretty_printed_payload_still_verifies`, 20-id control | `test_deleted_payload_artifact_blocks`, `test_rewritten_payload_hash_fails`, `test_missing_payload_declaration_blocks`, `test_manifest_payload_table_tamper_fails`, `test_missing_manifest_blocks` |
| **P1-3** paired arrays that drive CI/regression are unbound; per-sample fields never read | Each bundle sample is a record with `pair_index`, duration, exit code, timeout flag and payload/post-state hashes. Pairs *and* all statistics are reconstructed from those records; `samples.jsonl` must reproduce them exactly. `paired`, `speedup`, medians, CV, pair ratios and CI are compared with the reconstruction. | `test_positive_control_passes`, 20-id control | `test_forged_paired_arrays_fail`, `test_forged_sample_fields_fail`, `test_mutation_sample_post_state_hash_is_bound`, `test_extra_raw_sample_row_fails`, `test_tampered_raw_samples_artifact_fails` |
| **P1-4** a required workload's >10% regression is not checked (projection exempt) | Every required measured row — strict, projection and startup — must clear the raw-sample floor, be equivalent, and pass the same uncertainty-aware >10% regression verdict. The projection's recorded launch/import floor table is also checked against the measured table. Only the geomean population excludes projection/startup. | `test_positive_control_passes` | `test_projection_regression_fails`, `test_projection_sample_floor_applies`, `test_startup_only_apparent_win_fails` |
| **P1-5** `emit` labels a recorded PASS; `aggregate.status` can invert a gate verdict | `performance_evidence emit/validate` re-derive the verdict from artifacts + live probe before projecting. `certify.performance_section` derives status from the recomputed verdict only; a non-PASS gate can never produce `status: pass`, and a section without a verification is emitted as `blocked`. | `test_emit_verifies_before_projecting_a_pass`, 20-id control | `test_emit_refuses_a_forged_bundle`, `test_emit_never_projects_pass_from_a_non_pass_gate`, `test_checker_adapter_rejects_a_blocked_bundle` |
| **P2-6** projected CI / work geomean / counts unverified | `evaluate_bundle` compares `geometric_mean_ci95`, `work_excess_geometric_mean`, `strict_workload_count`, `sample_count`, `minimum_speedup` and the regression allowance with the recomputation; `check_bundle` repeats the comparison for the `coverage.json` section. | 20-id control | `test_checker_adapter_rejects_projection_drift` |
| **P2-7** bundle does not bind its producer; manifest unverified | `certify_run` records `harness.code` = sha256 of `bench.py`, `spkbench/**`, `adapters/**`, `tools/**` and `scripts/rust/performance_evidence.py`; the verifier compares those hashes to the live files and requires the measured-input set to be unchanged across a descendant commit. `manifest.json` is mandatory. | `test_harness_manifest_covers_producer_not_tests`, 20-id control | `test_harness_code_change_blocks`, `test_missing_manifest_blocks` |
| **P2-8** malformed evidence crashes instead of blocking | `evidence.load_evidence` raises typed `EvidenceError`; `evaluate_bundle` catches `KeyError`/`TypeError`/`AttributeError`/`ValueError`/`OSError` and blocks with the exception type; `check_bundle` catches everything at the evidence boundary and reports `PERF_EVIDENCE_MALFORMED`. | — | `test_malformed_evidence_blocks_without_crashing`, `test_checker_adapter_reports_malformed_evidence` |
| **P2-10** doc drift / compat fallback / mtime message | `workloads._resolve_context` no longer substitutes the inspection binary for the compatibility CLI (the guard now checks a real file); the receipt mtime message names the real condition (older-than-build file, sha256 authoritative); counts below are corrected. | — | `ruff`-visible, no behaviour change to passing paths |

P3 nits: the per-workload median CI still uses `Random(0)` (display-only, not
gated) and is left unchanged; the estimand mix (point = median-of-samples ratio,
interval = paired ratio) is now stated in the method text; the checker resolves
the registry relative to the audited checkout when it is not the default one.

## One verifier, derived evidence

The gate is now three small layers instead of a chain of booleans:

| Module | Responsibility |
| --- | --- |
| `spkbench/evidence.py` | Loads the artifact table, `manifest.json`, `samples.jsonl`, every normalized payload and every mutation post-state delta; verifies file hashes, **declared canonical hashes over parsed content**, table equality and coverage. A payload may be pretty JSON: reformatting keeps the canonical hash, editing changes it. |
| `spkbench/live.py` | `NativeSourceProbe.observe()` reports the live commit, dirty paths, build-input digest, `Cargo.lock`, toolchain, both binaries, the re-verified build receipt, the registry file, `uv.lock`, the repo `packs` digest, the live oracle verification/pinned `src` baseline, and the hash of every harness/adapter file. `is_ancestor`/`changed_measured_inputs` decide the evidence-only descendant allowance. |
| `spkbench/certify.py` | One `evaluate_bundle`: registry/self-hash, recorded identity *shape*, stored evidence derivation, per-sample binding, pair reconstruction, statistics, work excess, aggregate, startup, live-native comparison, projection comparison, verdict. `certify_run` only *constructs* a bundle; it never stamps a verdict. |

`report.write_evidence` runs the verifier twice: a provisional pass once the
measurement artifacts exist (so the rendered report can state the verdict) and an
authoritative pass over the final bytes including `manifest.json`. If the two
verdicts differ, the write is refused. `bench.py gate`, the completion checker,
`emit` and `validate` all call the same function; there is no
`require_live_identity=False` path in the checker.

Descendant rule: if `HEAD != candidate`, the candidate must be an ancestor of
HEAD and `git diff candidate..HEAD` over the measured inputs
(`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `crates/**`,
`benchmarks/rust/{bench.py,spkbench,adapters,tools}`, the checker adapter and the
frozen registry) must be empty. Uncommitted changes are allowed only outside
those paths. Evidence and documentation commits after a measurement are
therefore accepted; a foreign commit, a rebuilt binary, a changed harness or a
changed measured crate is not.

## Controls actually run

All commands below used the pinned oracle interpreter
(`.venv-rust-oracle/bin/python`, CPython 3.11.15) with `PYTHONPATH=src` and
`SPEC_KITTY_PACKS_ROOT=$PWD/packs`, and `HOME`/`XDG_*`/`TMPDIR` under
`/tmp/sk-bench-dev`.

```
scripts/rust/verify_oracle.py            ok=true  53 checked, 0 drift, 0 missing
git diff --quiet 9aed3a80… -- src        exit 0 (oracle src is the pinned baseline)
bench.py selftest                        49/49 passed
pytest benchmarks/rust/tests             60 passed
bench.py identity                        exit 1, status=drift (dirty tree, stale binaries, no receipt)
bench.py gate evidence/2026-11-02-initial exit 2, BLOCKED (spkbench.report.v1 is not re-certifiable)
check_completion.py --json-report        exit 1, 599 errors, 27 performance, summary.ok=False
```

Positive controls:

* `test_positive_control_passes` — a 3-strict-workload synthetic run written by
  the real writer (stored payloads, post-state delta, 20 pairs/side, manifest)
  reaches `PASS` only when re-derived from the stored bytes with a probe.
* **Full 20-id positive control.** The separate test owner's
  `scripts/rust/tests/support/synthetic_certification.py` was copied out of tree,
  given the small fixture/probe patch listed below, and exercised end to end:
  `build_certification` → written bundle → `check_bundle`, with 20 required ids,
  17 strict ids, the frozen policy and 16 simulated rows.

  ```
  bundle verdict: PASS
  rows: 20   simulated: 16
  check_bundle ok: True   verdict: PASS   errors: 0
  ```

  This is a measurement of nothing; it exists to prove the full acceptance path
  (including live-source binding) is exercisable without disabling a check. The
  positive control fabricates *observations* at the probe seam only — every
  comparison still runs against them.
* `test_dirty_docs_do_not_block` and `test_evidence_only_descendant_is_allowed`
  — legitimate evidence/docs work after the measured commit still certifies,
  while any measured-input change blocks.

Negative controls (all fail closed with a named reason):

| Area | Controls |
| --- | --- |
| Completeness | absent required workflow; filtered subset; missing raw samples; n=19; projection without samples |
| Identity / native source | missing build receipt; recorded identity drift; stale/foreign candidate; descendant touching `Cargo.lock`; uncommitted measured input; harness code change; missing/stale registry file |
| Stored evidence | deleted payload files; rewritten payload content; missing payload declaration; manifest payload tamper; missing manifest; edited `samples.jsonl`; extra raw-sample row |
| Projections | forged speedup; forged geomean; forged aggregate CI/work geomean/counts; forged `paired`; forged per-sample payload/post-state hash |
| Equivalence / mutation | degenerate payloads; two-sided empty stdout; schema-failed row; recorded equivalence diff; mutation without post-state; stored post-state inequality |
| Submission | startup command as work; startup-dominated work; projection-only; projection promoted to gate; material and uncertain regression; noisy load; sub-3× geomean; CI reaching below 3× |
| Emit / adapter | unsealed bundle; recorded PASS over failed recomputation; forged `aggregate.status`; section/bundle sample mismatch; projection drift; live-source mismatch; malformed evidence |

## Integration required from the separate test owner

`scripts/rust/tests/**` is owned elsewhere and was **not** edited. Its current
`build_certification` fails against the strengthened verifier for four reasons
(by design), and the exact fixes were validated on an out-of-tree copy:

1. pass the probe seam to the writer and the section builder:

   ```python
   from spkbench.selftest import synthetic_probe
   written = write_evidence(..., live_probe=synthetic_probe(synthetic_identity(), registry_path))
   section = certify.performance_section(bundle, bundle_path, verification=written["verification"])
   ```

   `synthetic_probe(identity, registry_path)` is a new additive helper: it
   builds the same-shaped probe over the declared synthetic environment and
   hashes the real registry file. Every acceptance check still runs.

2. every run record must carry `"measurement"` and `"equivalence"` (the real
   runner already does) and the `preflight` block must include the per-side
   observations (`exit_code`, `timed_out`), so exit codes can be tied to the
   preflight. `"expected_exit": 0` may be recorded; otherwise the registry's
   declared exit is used.
3. **all** required rows need real paired samples (the real runner measures
   startup and projection rows with 20 pairs too); the previous fixture returned
   `samples: {"rust": [], "python": []}` for non-gate rows, which now correctly
   blocks on the sample floor. `floors_seconds.python_by_modules` must also hold
   the import floor of every runnable row, not only gate-eligible rows — the
   projection's floor table is verified too.
4. stored content must be self-consistent: a mutation row's post-state delta must
   be stored and its per-sample `post_state_sha256` must be the canonical hash of
   that delta (not `"8" * 64`); contract rows (`startup.*`) store
   `{"_raw": stdout}` and must satisfy the registry's stdout schema (≥200 bytes,
   ≥6 lines for `startup.help`); payload-schema validation applies only to
   non-contract rows.

The validated diff (minus the out-of-tree `ROOT` override) is ~90 lines; the
essential parts are the two lines in (1), the two record fields plus preflight
observations in (2), replacing `if entry.get("gate_eligible"):` with unconditional
sample generation in (3), and:

```python
def _post_state_for(entry):
    if entry.get("measurement") != "mutation":
        return {}
    return {"added": ["out/state.json"], "changed": [], "removed": []}

post_state = _post_state_for(entry)
post_state_sha256 = _delta_sha256(post_state) if post_state else None
...
record["preflight"]["post_state_sha256"] = {"rust": post_state_sha256, "python": post_state_sha256}
record["preflight"]["post_state"] = {"rust": {"delta": post_state}, "python": {"delta": post_state}}
```

Until that lands, their existing positive control reports
`synthetic certification bundle did not pass: … BLOCKED/FAIL …` — an honest
refusal, not a crash.

## Actual remaining obligations

Nothing below is claimed as done, and none of it changes the fact that the
performance gate is `BLOCKED`.

1. **16 strict workloads have no native implementation.** `state.event-append`,
   `state.move-task`, `state.decision.resolve`, `runtime.next.governed`,
   `governance.charter`, `glossary.activation` and the ten `workflow.*` rows are
   `absent` in the frozen registry and block the gate by construction. They must
   not be replaced by fake adapters or equal refusals.
2. **No measurement campaign, no receipt.** The release binaries predate the
   tracked build inputs and there is no `target/release/build-receipt.json`, so
   `bench.py identity` is `drift` and no bundle can bind today.
3. **The historical bundle stays `BLOCKED`.** `evidence/2026-11-02-initial/` uses
   `spkbench.report.v1` and is refused; it was not retrofitted.
4. **Recorded-but-not-measured metadata remains disclosed.** `environment.isolation`
   is a constant claim; `environment.floors_seconds` and `load_context` are
   harness-recorded. The gate re-derives *their use* from samples and checks them
   against the recorded table, but a fabricated bundle can choose consistent
   values. `baseline.python_version` and `packs_digest` are re-observed live;
   `isolation` is not.
5. **The oracle interpreter path comes from the bundle's harness block.** The
   verifier re-runs `verify_oracle.py` and the pinned-`src` check with it and
   compares its real-file hash, so a swapped interpreter is visible; it still
   cannot prove the recorded path was the one that ran during the campaign (the
   binary/source and start/end stability checks bound that window).
6. **Lint/complexity.** The new modules add `C901` findings (`_derive_row`,
   `_judge_row`, `_compare_live_native`, `evaluate_bundle`,
   `_verify_recorded_identity`, `_verify_aggregate`, and
   `performance_evidence._check_bundle`). The surrounding `scripts/rust` tree
   already carries `C901`/`E501` findings (`check_completion.py`,
   `inventory_model.py`), so the tree is not ruff-clean at this checkpoint; the
   new files are otherwise clean apart from justified `TID251` noqa on
   file-integrity hashing. This is delivery-lint work, not an evidence gap.
7. **Every harness edit invalidates prior evidence.** `harness.code` binds the
   producer; a committed harness change makes an old bundle block until it is
   re-measured. That is deliberate.
8. **Cross-platform.** Only Linux has been exercised; macOS/Windows execution,
   filesystem semantics and toolchain identity remain delivery gates.
9. **A green `coverage.json` has not been produced.** `emit`/`validate` now
   refuse anything that does not recompute to `PASS`, so there is no path to a
   curated section without a real passing bundle.

## Files in this tranche

| Path | Change |
| --- | --- |
| `benchmarks/rust/spkbench/evidence.py` | **New.** Stored-evidence derivation, canonical payload hashing, manifest verification. |
| `benchmarks/rust/spkbench/live.py` | **New.** Live native-source, oracle/packs/`uv.lock` and harness/adapter probe; measured-input history. |
| `benchmarks/rust/spkbench/certify.py` | Rewritten as one verifier; per-sample records, pair reconstruction, projection recomputation, live binding, typed malformed-evidence handling. |
| `benchmarks/rust/spkbench/report.py` | Stores payloads + post-state deltas with declared canonical hashes; verifies twice and stabilises the on-disk verdict; harness code manifest. |
| `benchmarks/rust/spkbench/selftest.py` | 49 controls on written bundles; synthetic probe seam and `synthetic_probe()` helper. |
| `benchmarks/rust/spkbench/workloads.py` | Compatibility-binary fallback no longer substitutes the inspection binary. |
| `benchmarks/rust/spkbench/build_receipt.py` | Correct mtime provenance message (older-than-build file; sha256 authoritative). |
| `benchmarks/rust/bench.py` | `gate`/`run` observe the live native source through the probe; post-run oracle/dependency drift merged into the recorded identity. |
| `scripts/rust/performance_evidence.py` | Checker always binds live; row and aggregate projection comparisons; `emit`/`validate` verify before projecting; malformed evidence is a reported failure. |
| `benchmarks/rust/tests/test_gate_bundle.py` | 14 end-to-end bundle/adapter tests (was 8). |
| `benchmarks/rust/tests/test_evidence_binding.py` | **New.** 8 focused stored-evidence/probe tests. |
| `benchmarks/rust/{README,ADAPTER-REPORT,CERTIFICATION-REPORT}.md`, `docs/rust/VERIFICATION.md` | Corrected gate rules, evidence layout and test counts. |

## Verification commands (reproducible)

```sh
export HOME=/tmp/sk-bench-dev/home XDG_CACHE_HOME=/tmp/sk-bench-dev/xdg \
       XDG_CONFIG_HOME=/tmp/sk-bench-dev/xdgconf XDG_DATA_HOME=/tmp/sk-bench-dev/xdgdata \
       TMPDIR=/tmp/sk-bench-dev/tmp PYTHONDONTWRITEBYTECODE=1 \
       PYTHONPATH="$PWD/src" SPEC_KITTY_PACKS_ROOT="$PWD/packs"

.venv-rust-oracle/bin/python scripts/rust/verify_oracle.py
.venv-rust-oracle/bin/python benchmarks/rust/bench.py selftest
/tmp/spkbench-pytest/bin/python -m pytest benchmarks/rust/tests -q -p no:cacheprovider
.venv-rust-oracle/bin/python scripts/rust/check_completion.py --json-report /tmp/report.json
```

No `cargo` command, native build, extra Cargo target or timing campaign was run
in this tranche, and the `src`/`packs` baseline was not repinned.

## No claim

This tranche makes no performance claim of any kind: no speedup, no geometric
mean, no confidence interval, no no-regression result and no readiness statement.
The only numbers quoted are test counts, artifact hashes and deterministic
synthetic controls. The gate is `BLOCKED` for the honest reason that 16 required
native workloads do not exist and no current-source measurement has been made.
