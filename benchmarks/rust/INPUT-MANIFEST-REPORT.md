# Recorded input manifest binding — R1 tranche (FINAL)

Status: **R1 is implemented and covered by a red-first, consistently re-sealed
negative control. R2 is preserved and now also exercised through the final
checker. All prior negatives and the frozen policy floors remain unchanged. The
production performance gate is still `BLOCKED`, and no speed, throughput,
no-regression, completeness or readiness claim is made.**

This report was created provisionally before the implementation and is now
updated in place with the real results. The source tree remains dirty from
concurrent owners, the release binaries predate the current harness bytes, there
is no `target/release/build-receipt.json`, and 16 of the 20 production workloads
remain genuinely absent.

## Scope and preserved state

Owned and edited: `benchmarks/rust/**`, `scripts/rust/performance_evidence.py`,
`scripts/rust/tests/test_check_completion.py`,
`scripts/rust/tests/support/**`.

Not edited: `scripts/rust/check_completion.py`, `scripts/rust/inventory_model.py`,
native crates, `coverage.json`, manifests and all other docs/policy. No `cargo`,
native build, release, timing campaign, new workspace, commit, push or subagent
was used.

The production registry has one mechanical edit in this tranche, disclosed
below: its **self-digest field** was stale after the R2 content correction and
had to be repaired because the file is self-digesting and
`PerformanceRegistry.load` refuses a mismatch. No workload row, id, measurement,
expectation, policy value, sample floor or absent backlog was changed.

## R1 — what was actually unbound

The prior tranche recorded a mutation `before` manifest and the `after` entries
for `unchanged_paths`, and verified only their internal consistency
(delta == manifest difference, before == after). The independent review proved
the limit: both sides accepted a forged event-log hash of `9*64`; the synthetic
positive recorded a fabricated before digest (`6ef58784…`, the hash of the path
string) while the regenerated `replay-project` bytes were `e48b5edd…`.

## R1 — implementation

One shared normalisation now exists in `spkbench.fixtures`:

* `canonical_bytes(data, root, roots=...)` replaces every known location root
  (the pristine fixture root **and** the runtime clone root) with `<FIXTURE>`,
  longest root first.
* `content_manifest(root, roots=..., ignored_prefixes=...)` is the single
  manifest definition. It is used by the pristine fixture, by `verify_manifest`
  and by `spkbench.mutation.tree_manifest`.
* Ignored paths are limited to the frozen registry's declared
  `ignored_prefixes` (`.git/spec-kitty-locks/` for `status.replay`). No
  user-observable directory (including the rest of `.git` or `kitty-specs/`) is
  excluded.

Measured mutation manifests are therefore portable and directly comparable with
an independently regenerated fixture. A clone whose bytes still embed the
**original fixture root** (the runtime-snapshot case) normalises to the same
manifest as a freshly generated fixture at an audit root; a raw clone hash does
not. Required post-state file bytes remain byte-exact; their manifest binding is
recomputed with the same normalisation using the recorded `content_roots`.

The default fixture observer (`certify._verify_fixtures`) now returns, in
addition to the digest/recipe/file-count facts, the **actual portable per-file
manifest** of both two-root regenerations and a `manifests_equal` fact. The
synthetic observation seam returns the same shape from the same generator.
Observation is the only injected seam; there is no bundle flag, custom registry
or asserted verdict that turns the checks off.

For every mutation workload the certifier now compares the stored envelope:

* the whole recorded `before` manifest must equal the regenerated fixture
  manifest path-for-path and digest-for-digest (after filtering only the
  registry's declared ignored prefixes);
* every declared `unchanged_paths` `after` entry must equal that regenerated
  fixture digest;
* missing, extra and differing paths are all failures.

The synthetic positive controls record the **real generated fixture manifest**
and the real generated required-file bytes. `status.replay` still uses the real
`replay-project` fixture; the `gate.mutation` scaffold now uses the real
synthetic fixture manifest instead of `keep.json`. No expectation, workload,
sample count or floor was lowered.

## R1 — red-first and genuine-positive evidence

Probe: `/tmp/skr1/probe_r1_redgreen.py`, log `/tmp/skr1/r1-redgreen.log`. It
builds the normal synthetic bundle, rewrites one unchanged fixture file's
`before` **and** `after` digest to `9*64` on both sides, re-seals every envelope
hash, row post-state hash, sample post-state hash, artifact hash, manifest and
bundle self-hash.

```
red_without_binding:  verdict PASS   (the pre-fix behaviour)
green_with_binding:   verdict FAIL   (both sides:
  "recorded before manifest does not match the regenerated fixture bytes for
   ['file-00.txt']")
```

The genuine positive (`test_pretty_printed_post_state_envelope_still_verifies`,
`test_write_then_verify_round_trips`, and the synthetic certification) still
returns `PASS`. A new test
(`test_recorded_input_manifest_is_the_generated_fixture_manifest`) asserts the
positive control's `before`/unchanged `after` entries are exactly the generated
fixture bytes. A clone test
(`test_clone_manifest_binds_to_the_pristine_fixture_manifest`) proves that the
runtime fixture's embedded original root is normalised correctly:
`raw clone manifest != pristine manifest` and
`tree_manifest(clone, roots=(fixture,)) == fixture.expected_content()`.

## Preserved coordinator R2 correction

The frozen `status.replay` written `status.json` contract still pins
`mission_slug`, `mission_number`, `mission_type` and `schema_version` to stdout's
contract. `benchmarks/rust/tests/test_post_state_identity_contract.py` is
preserved with its 1 static contract check and 4 stored-byte identity
negatives; `/tmp/sk-post-state-identity-{red,green}.log` retain the original
red-first evidence.

A new final-checker negative
(`test_bundle_stored_status_identity_forgery_is_rejected`) rewrites the stored
`status.json` to `999-some-other-mission`, re-binds its bytes, after-manifest,
delta, artifact record, per-sample post-state hash and bundle/manifest hashes,
and the final checker rejects it with a `mission_slug` failure. This proves R2
through the complete `check_completion` path, not only the schema unit.

A second final-checker negative
(`test_bundle_unchanged_input_hash_forgery_is_rejected`) forges the
`status.events.jsonl` digest on both `before` and `after` (the exact residual in
the independent review), re-seals every hash, and is rejected with
`regenerated fixture bytes`.

## Registry self-digest repair (disclosed)

The R2 content change left the registry's declared self-digest stale
(`sha256:01841ff…` while the content hashed to `82435983…`), so
`PerformanceRegistry.load` refused the frozen file. The only registry edit made
here is the `digest` field required by the file's own invariant:

* file sha256: `74a0f408dc538c418480d1a88a8d9fb18a1092c039ba4c3cba97e451b3ae5fa4`
* self-digest: `sha256:82435983080c99693f9ce0c977bc7dffd7a4c6e581194e9583c4e7b70114700d`
* still **20 required / 17 gate / 2 startup / 16 absent**; policy still
  **3.0x / 20 samples per side / 14 strict workloads / 10%**.
* `coverage.json` is unchanged:
  `797212bc0229d63df7fef11019a357ae5e5e8dde922679e041fca8ced6b99d98`,
  `performance.status = pending`.

No absent row was replaced, renamed, given samples or promoted, and no
production workload or expectation changed.

## Commands run (fresh isolated roots)

CWD/HOME/XDG/TMP under `/tmp/skr1`; `PYTHONPATH=<repo>:<repo>/src`,
`SPEC_KITTY_PACKS_ROOT=<repo>/packs`, `PYTHONDONTWRITEBYTECODE=1`; oracle
interpreter `.venv-rust-oracle/bin/python` (CPython 3.11.15); benchmark pytest
from `/tmp/spkbench-pytest/bin/python`. No `cargo` was run.

| Command | Result |
| --- | --- |
| `pytest benchmarks/rust/tests -q` | **86 passed** in 39.9 s (was 78 before R2's 5 and this tranche's 3) |
| `python -m unittest scripts.rust.tests.test_check_completion` | **53 passed** in 81.3 s |
| `python -m unittest test_check_completion test_verify_oracle test_evidence_roots` | **61 passed** in 84.8 s |
| `bench.py selftest` | `ok=true total=49 failures=0` |
| `scripts/rust/verify_oracle.py` | `ok=true`, 53 checked, 0 drift/missing/unsatisfied, CPython 3.11.15 |
| `check_completion.py --json-report` | **exit 1**, **599 errors**, **27 `PERF_*`**, `summary.ok=false`, `gates.performance=false`, no performance section |
| production `PERF_*` shape | 17 `PERF_WORKLOAD_NOT_PASSING`, 2 startup, 2 missing bundle, 6 aggregate/status/candidate = 27 |

The check-based test suite is run with the lock-exact oracle interpreter because
`_check_dependencies` compares the recorded oracle package table with the live
interpreter; the benchmark pytest suite runs from the pinned pytest venv and
executes the prebuilt binaries only for functional equivalence, never for
timing.

## Source hashes (sha256)

| Artifact | sha256 |
| --- | --- |
| `benchmarks/rust/spkbench/fixtures.py` | `c1010a70d3976bf58fd4601a883ceacdabf11113e3ab2304a2782482b3f4a2b1` |
| `benchmarks/rust/spkbench/mutation.py` | `b6d1443b328a43e1c28c9ab4bb2305d28acab128acbe2b818c15b727588842df` |
| `benchmarks/rust/spkbench/expectations.py` | `43d81ce280d7f8c376ef1ddf8dbee16b9d1fab0333a86b16ee091e1a4425aba9` |
| `benchmarks/rust/spkbench/runner.py` | `22e34a59244645ab636a24bcbaf3a82ba9fa634394b3047f3e2d8e89fb0c430a` |
| `benchmarks/rust/spkbench/report.py` | `b85ee5a73b75ba33f18d5382c95c230dbd0c7bda3ee9c528d59dd432780c4d04` |
| `benchmarks/rust/spkbench/evidence.py` | `7d0c27fc27f61f26c7ef66eb63df33d0316ce8da86079e75a53b491329d71323` |
| `benchmarks/rust/spkbench/certify.py` | `a0eed52cc9cdfdf2c8d700f1a8f4d1ccba0c378ba84db5a18cd53f1192edaab3` |
| `benchmarks/rust/spkbench/selftest.py` | `73117af4bc146445073537c6bcb2f68dd702f878fbe88c29823bb06d1a228660` |
| `benchmarks/rust/tests/test_evidence_binding.py` | `5553457f44b1ace07d92c9a32cd69f643597b3598f2cf9540870335edc9b3aac` |
| `benchmarks/rust/tests/test_fixtures_portable.py` | `4349ed15cc7de8d660b86581e7f8fcf032a83197227c00ed68fc99ff14063495` |
| `benchmarks/rust/tests/test_post_state_identity_contract.py` | `351c14188049a80d96467ab825d125fde9b46c02533b2cdf2314464787183e21` |
| `scripts/rust/tests/test_check_completion.py` | `4e625f22d9085dfe8705871a9614b410f4e6f7a2fd80935992ede006a9e6b080` |
| `scripts/rust/tests/support/synthetic_certification.py` | `c3e1bc8f0ea68f7206dc59d38971a69f125bfebe2f86374de39d1a8c39688051` |
| `docs/rust/inventory/performance-registry.json` | `74a0f408dc538c418480d1a88a8d9fb18a1092c039ba4c3cba97e451b3ae5fa4` |

`scripts/rust/performance_evidence.py` was not changed in this tranche (the
checker already passes `verify_fixtures=True` and the observation seam is
`None` in production).

## Findings and residuals

| Id | Disposition |
| --- | --- |
| **R1** | **Closed in current bytes** with the shared normalisation, the regenerated per-file manifest binding, the re-sealed red-first probe and a final-checker re-sealed unchanged-hash negative. |
| **R2** | Preserved; the 5 focused tests and logs remain, plus a new final-checker re-sealed wrong-slug negative. |
| **R3** | Unchanged disclosed limit: a caller-supplied audit registry is an explicit trust anchor; production never injects one. Not a new cybersecurity gate. |
| **R4** | Unchanged disclosed limit: chronology is structural, not historical execution. |
| **R5** | Unchanged disclosed limit: binding proves the canonical recipe, not a per-sample executed-argv receipt. |
| **R6** | Unchanged disclosed limit: the lower-level `evaluate_bundle` defaults to `verify_fixtures=False`; every production call site passes `True`. A caller that explicitly disables observation gets the `recorded-only` marker and no regenerated-manifest binding. |
| **R7** | Unchanged disclosed limit: the fixture seed is a declared run parameter, not pinned in the registry. |

**R3, narrowly.** A caller-supplied audit registry remains an explicit trust
anchor for that caller. Production does not inject one: the checker and harness
load the live canonical `DEFAULT_REGISTRY`. This is an audit seam, not a new
cybersecurity boundary or gate, and this tranche adds no policy or security
claim around it. **R4–R7** are carried forward exactly as disclosed: chronology
proves stored structure rather than historical execution; command binding
proves the canonical recipe rather than a per-invocation argv receipt; the
lower-level evaluator still defaults `verify_fixtures=False` while every
production call site passes `True`; and the fixture seed remains a declared run
parameter. No provenance beyond the independently observed facts is claimed.

The production gate remains blocked for honest reasons: 16 absent workloads, no
build receipt, no current-source measurement and no passing campaign. The
historical `evidence/2026-11-02-initial/` bundle remains the old schema and is
still refused; no bundle was retrofitted.

Preserved untouched: `/tmp/skrev3/**`, `/tmp/skrev4/**`, `/tmp/skrev5/**`, and
`/tmp/sk-post-state-identity-{red,green}.log`.

## No claim

No performance claim of any kind is made. This report quotes only test counts,
error counts, source hashes and deterministic control outcomes. It does not
claim native completeness, cross-platform behaviour, a passing gate or that the
current dirty sources build.
