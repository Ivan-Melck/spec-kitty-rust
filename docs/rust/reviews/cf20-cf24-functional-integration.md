# CF20–CF24 bounded functional integration

This is an implementation checkpoint, **not** Source-equivalence, workflow,
release, independent review or performance approval. Published HEAD remains
`e38b075eebc6ceffb2adb96b1b1735acfd815da7`; shared work is uncommitted.
Source/packs/pins are unchanged. Callable migrations remain **132/136**; four
rescue callbacks remain withheld.

## Real reachable behavior

- `doctor ops --close-stale` now calls the canonical Invocation sweep after
  root resolution. It records real abandoned closures on the append-only
  spine, leaves record bytes intact, attempts the real custody-bound spine
  commit and reports its actual outcome. Threshold usage validation precedes
  root resolution. Listing and sweep share one non-record filename authority.
- `doctor command-files`, `skills` and `tool-surfaces` reach the actual CF23
  read-only diagnostics. Missing surfaces/uninstalled tools are real findings;
  filters work; unsupported repairs still refuse. The capability list is
  **11 reachable / 8 refused**, not eleven fully implemented diagnostics.
  Doctor correctly advertises mutation because an Op sweep writes/commits.
- Tool errors preserve Source's `ok=false/error.code/error.message` envelope
  and command-specific exit: command-files not-in-project is **1**, the other
  two are **2**. `CompatError::envelope_with_exit` keeps old result-envelope
  callers at exit1. Unsupported repair errors retain their coverage contract.
- CF18/24 finalization establishes COORD status placement while scanning
  PRIMARY tasks, validates owned globs, publishes coordination status seeds
  per WP and reads branch-ref-only logs in validate-only mode. PRIMARY status
  seeds remain uncommitted. Completed planning writes remain on later failure;
  a proven-unlanded seed rollback is separate from whole-command rollback.
- CF20 accept gates use actual planning-to-lane owned-file evidence and review
  independence, explicit attestation and lazy non-WP consultation. The four
  CF7 fixtures now supply an observed planning OID and real subsequent owned
  change on a canonical lane branch. All four old test assertions were retained;
  no evidence guard was weakened to accommodate the old null/no-branch premise.

## Actual parent verification

All command receipts are under `/tmp/sk-vertical-integration.TwDlpw/`, indexed
by `command-log-index.tsv`; all Cargo runs used focused offline/locked wrapper.

| Receipt | Scoped outcome |
|---|---|
| `cf2223-parent-final-diagnostic-binary-smoke.tn2AMC.log` | CF14 15 + new binary4 + CF22 runner4 + CF23 20 + root3 + CF9 13 = **59 pass** |
| `cf2223-parent-doctor-inline-contracts.xi9FMh.log` | **15 pass** |
| `cf20-cf24-parent-acceptance-and-finalize-functional-smoke.1h11Xn.log` | CF7 4 + CF20 8 + CF13 6/3 + CF15 8 + CF18 10 + CF24 6 + compat26 = **71 pass** |
| `cf20-cf24-parent-functional-tests-scoped-strict.uqwlh5.log` | CLI lib + selected CF7/compat/doctor test targets strict clean |
| `cf20-cf24-and-cf2223-final-owned-format-and-result-index.yUIpKq.log` | owned-only format clean, unchanged HEAD, no Source/packs diff |

These are separate overlapping scoped gates, **not** a new aggregate total.
Native binary smoke is not paired Source-child isolation proof.

Originals: `/tmp/sk-cf2223-parent-original.cxO8Tl`,
`/tmp/sk-cf24-parent-original.YTluIB`,
`/tmp/sk-cf20-parent-fixture-original.IhZURo`. Parent PRE-edit maps:
`cf2223-parent-cli-integration.yaml` (`.ZS7KFr`) and
`cf20-cf24-parent-functional-fixtures.yaml` (`.TPhE8S`), both schema,
structural and admissibility validated. They certify **parent edits only**.

Failed parent attempts are retained: `.HYMfQK` assumed a nonexistent flat
spine-path export (compiler failure, fixed via existing path owners);
`.sAxc6b` falsely expected manifest gaps in a no-manifest fixture (Source
verify reserves gaps for missing recorded entries; now assert exact uninstalled
agent/no-folder facts); `.tIVQut` found an old useless format! in the now-owned
CF14 test (byte-identical literal fix). One rejected exact edit changed nothing.
Later greens do not erase them.

## Open functionality and proof

- Op SaaS propagation, non-ULID/corrupt record behavior, Python repr, per-sweep
  spine snapshot/races, human output, platform and exact closure/commit proof.
- Tool session-presence/plugin probes, expected profile/pack catalogs, legacy
  agent config fallback, canonical-render collision/legacy-owned classification,
  shared global-location authority, repairs and Source command-shape policy.
  `research-gap-surface` is not a completed probe. Source caller policy wins over
  Native policy; a reported internal split does not authorize choosing Native.
- Finalization published/E2 successful write vs Source emitter failure,
  ambiguous read-probe refusals, bounds/encoding, annotation-only behavior,
  indeterminate/HEAD/protected/nothing-to-commit recovery, advisory warnings,
  validation ordering and exact payload/error phases remain unapproved.
  The E2 difference is a **defect**, not anti-refusal strengthening permission.
- Acceptance still has wire extras (including `required_artifacts_source`),
  canceled-membership approximation, duplicated status-only exclusion policy,
  adapter-specific attestation publication and missing other evidence consumers.
  Extra fields are **not** an approved additional compatibility exception.
- CF21 full parent code review, owned birth-cutover production callers, owned
  seed/verify lock-root composition and registration remain open. The actual
  Source migrate command has no invented owned-mode flag.
- Producer CF23's post-edit classification/unwrapped attempts remain debt.
  CF24's dependency-hygiene link was not followed at its actual architecture/
  path; its missing-file claim is not initialization completion. Red-first,
  wrapper and cumulative-diff disclosures are not backdated.

The sole semantic exception remains lossless surrogate **keys/associations**
in DecisionIndex summary JSON. No other difference above is waived. Deep paired
Source differential, error/IO/concurrency/platform/workflow campaigns, coherent
aggregate/release gates and genuine ≥3× equivalent-work measurements remain owed.
No commit/push/publication was performed.
