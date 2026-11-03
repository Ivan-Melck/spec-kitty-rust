---
type: reference
updated: 2026-11-03
audience: software-engineer
---
# Rust verification evidence

**Incomplete rewrite.** Local test success, partial library parity and fast
inspection workloads do not satisfy [the completion loop](LOOP.md).

## Custody/resolver Linux checkpoint — 2026-11-03, after `1ad605bb7`

Stable coordinator gates: **2,073 passed / 0 failed / 9 parent-exercised
helpers**, 178 suites including doctests; strict workspace all-target Clippy
and fmt pass. Logs `/tmp/sk-native-custody-stable-{aggregate,clippy,fmt}.log`.
The combined shell hit its 300-second tool budget during release compilation,
after those gates completed. No build/test process remained when inspected;
partial artifacts and the original release log were preserved. A release-only
continuation with a larger budget passes (`…-release-resumed.log`). This is an
explained interrupted build, not a waived failure or an initial release pass.

Included: **80 Op / 199 Kernel / 276 Git** tests, not added again to the total.
Large Op trails close across native/Python, non-object modes refuse, >64 KiB
worktree output is file-captured, and reference resolution is a shared additive
Kernel API with real-loop detection and no total-link cap. Existing containment
and lock authorities remain unchanged. The generic tracked-aware ignore probe
has exact exit/argv/read-only/index tests. Both deliveries await fresh independent
review; no executor/admission/CLI/capability is wired or promoted.

Fresh completion: **599 findings / 27 PERF / false / pending**, accounting
valid (`/tmp/sk-custody-stable-completion-report.json`). Source remains
`9aed3a80`; no workflow, platform, real-fork, campaign, build receipt or speed
acceptance. Prior 90 migration/16 performance-workload omissions and developer-
tool lint findings remain open.

## Previous Op/ownership Linux checkpoint — 2026-11-03, after `bfc79fdfd`

All owners finished before these coordinator gates: **2,042 passed / 0 failed /
8 parent-exercised helpers**, 176 suite results including doctests. Strict
workspace all-target Clippy, full formatting check and both release CLI builds
pass. Final logs (after gating the three Unix-only Op symlink tests):
`/tmp/sk-native-op-ownership-final-{aggregate,clippy,fmt,release}.log`.
The preceding 2,042-pass chain is also preserved.
The new local `sk-invocation` package is the only lockfile addition; no registry
pin changes. No CLI depends on it and no invocation capability is promoted.

- **Op foundation:** 72 tests reproduced, including live lock-exact wire/parse
  comparisons and real bidirectional Python/native lock contention. Typed
  records/writer/custody exist; admission decision, profile execution and Op
  commit/composition do not. Declared divergences remain in its implementation
  report. Independent review rejects stage-1 acceptance for large-record
  completion, malformed mode, pipe-deadlocked lookup and resolver/composition
  defects. Coordinator F1/F2 red-first corrections pass 74 scoped Op tests;
  remaining remediation is recorded in `reviews/op-foundation-disposition.md`.
- **Status F1c:** independent review verified F1a/F1b, two real post-emit OS
  fault shapes, and F2–F6, then found concurrent losers deleting the winner's
  evidence. Strengthened ownership unit + 12-process real CLI test both fail
  red; correction separates create ownership and holds the canonical Mission
  lock before lane observation/allocation even without auto-commit. Scoped
  234 status / 58 CLI status / 9 CLI lib green. Fresh independent review
  approves F1c with library races, six real-CLI rejection/approval races,
  failure retention and prior-artifact controls. No whole-tranche approval;
  see `reviews/artifact-ownership-disposition.md`.
- **Git B1/B2:** complete bounded pre/post histories with exact prior suffix
  replace ambiguous newest-record matching. Duplicate regression fails red,
  now retains created-commit evidence; capped-history normal summary control
  passes. Scoped Git 262; independent recheck accepts B1/B2 at `1ad605bb7`.
  S1's capped-history fallback limitation remains explicit; see the disposition. One initial test fixture assumed same-OID soft reset
  appended entries; its setup assertion failed and was corrected to alternate
  real tips, explicitly recorded in `DUPLICATE-BOUNDARY-REPORT.md`.
- **Next:** independent review accepts both prompt corrections with live oracle
  decisions, exact authored WP bytes and 148 Mission tests. No broad next
  acceptance: protected-main composition still falsely advances without the
  missing executor.

Current full checks are not parity/workflow/platform/performance acceptance.
Fresh completion remains **599 findings/27 PERF/false/pending**, accounting
valid (`/tmp/sk-op-ownership-completion-report.json`); source
baseline is still `9aed3a80`. No measured campaign, current build receipt or
policy lowering; prior Python-tool lint findings remain unresolved.

## Previous post-review Linux checkpoint — 2026-11-03, after `f42d5380b`

All owners finished before the coordinator's final gate chain:
**1,966 passed / 0 failed / 8 parent-exercised helpers**, 170 suite results;
strict workspace all-target Clippy, full format check and both release CLIs
pass. Logs: `/tmp/sk-native-post-boundary-{aggregate,clippy,fmt,release}.log`.
This replaces the intermediate 1,965-pass run after one more evidence-retention
boundary was corrected; both sets of logs are preserved.

- Git: missing reflog watermark is validated before candidates; amend entries
  excluded. Three real-Git tests fail red then pass; scoped **259/0/3 helpers**.
  Independent re-review accepts G1–G4 and resolves the scoped workspace
  attribution condition. A duplicate-boundary ambiguity (B1, LOW) remains;
  coordinator reproduces it with 3 controls passing / 1 assertion failing.
  See `reviews/git-boundary-disposition.md`; no complete-attribution claim.
- Status: six independently found defects corrected (**234 status / 57 CLI
  status / 9 CLI lib**). Coordinator additionally fixed emit-error evidence
  cleanup to respect `MutationError::may_have_mutated`; its initial missing-
  import compile failure and subsequent true assertion-red are distinguished
  in `SECOND-COMMIT-REMEDIATION-REPORT.md`. Acceptance pending re-review.
- Next: activated custom non-WP `specify`/`plan` prompts and full authored WP
  frontmatter/trailing whitespace now pass two red-first regressions;
  **27 runtime_next** tests. StepContract false advance remains open.
- Doctrine: independent acceptance of N1/N2a/N2b(structural)/N3/N4 and packs-root
  normalization: **149 scoped tests + 52 independent oracle checks**. Nested
  validation and custom-language vocabulary remain open in the durable README.

Fresh lock-exact-interpreter completion check stays **599 findings / 27 PERF
errors / false / pending**, counting valid. The initial system-Python attempt
failed setup (`typer` missing), not a completion result; preserved at
`/tmp/sk-post-boundary-completion.log`. Actual report:
`/tmp/sk-post-boundary-completion-oracle-report.json`. Python-tool Ruff's
90 findings, 90 missing migrations, 16 absent performance workloads, source
reconciliation and all full-workflow/platform/performance obligations remain.
No coverage promotion or new acceptance policy.

## Previous frozen Linux checkpoint — 2026-11-03, published in `f42d5380b`

All implementation owners had finished before these coordinator gates; no
source changed during the runs. Rust 1.99.1, existing target directory,
`CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, temporary HOME/XDG/TMP and one lease
covering each entire Cargo invocation, including doctests:

- `cargo test --workspace --locked --no-fail-fast`: **1,947 passed, 0 failed,
  8 ignored subprocess helpers**, 169 suite results including doctests. Parents
  exercise all eight helpers; no required acceptance case is waived.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass.
- `cargo fmt --all -- --check`: pass.
- `cargo build --release -p sk-cli --locked`: pass, both CLI binaries.
- Independent harness review reproduces **86 benchmark pytest / 61 checker-
  oracle tests / 49 self-tests**. These are correctness controls, not timings.
- Fresh production completion checker: **exit 1, 599 findings, 27 PERF errors**.
  No workflow or performance gate is promoted; 90 migrations remain absent,
  and the registry still declares 16 performance workloads absent.

Logs: `/tmp/sk-native-frozen-required-oracle-{aggregate,clippy,fmt,release}.log`,
`/tmp/sk-frozen-completion{.log,-report.json}`. The additional development-tool Ruff check **fails with 90 findings**
(`/tmp/sk-frozen-python-lint.log`), including complexity, domain-inapplicable
charter-hash rules needing justified treatment, imports and unused names. No
Python lint waiver or passing-lint claim. No clean-source build receipt or
measurement campaign exists; a release build is not performance acceptance.
The source/oracle baseline remains `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Fork main was separately fast-forwarded and remotely rechecked at
`7c50fd38bec0fca4483bf8c11da8471e91652acd`; the rewrite is not yet reconciled.

**Oracle gate correction:** the first 1,946-pass attempt silently returned from
the init differential test when its optional override was unset. That attempt
is not init-oracle evidence. A new subprocess regression reproduces that false
pass, then the test adopts the shared required, lock-exact selector (including
verification of explicit overrides). With no init-specific override, the final
aggregate actually performs all three init post-state comparisons. Missing
oracle is now a failing precondition. Red/green:
`/tmp/sk-init-required-oracle-{red,green}.log`. The single test-only needless
borrow and two Mission-file formatting failures are also corrected, not waived.

**Review boundary:** init preservation/concurrency, the kernel release fix and
bounded workspace mechanisms have scoped independent acceptance. New status
commit/rollback, doctrine-default/validation and invocation-bound Git evidence
still need final scoped reviews. `next` still rejects valid non-WP custom steps
and can falsely advance a composition-backed action on protected main because
StepContract execution is absent. Those are open implementation defects, not
harmless deferred labels. No full parity, platform, workflow or speed claim.

## Previous full Linux aggregate — historical, `ff4452b02`

Later deliveries are not covered by this aggregate. Rust 1.99.1, `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`. The latest aggregate
used temporary HOME/XDG/TMP roots, retaining only Cargo/rustup build caches:

- `cargo test --workspace --locked`: **1,449 passed, 0 failed, 5 ignored** across
  141 suite results, including doctests. All five ignored cases are subprocess
  helpers exercised by their parents, not waived acceptance tests. Cargo.lock
  was refreshed for coordinator-added native dependencies before this run.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo build --release -p sk-cli --locked`: passed (both CLI executables).
- Inventory/checker/oracle-verifier regression tests: **39 passed**.
- Then-current benchmark self-tests: **19 passed**; untimed adapter tests:
  **16 passed**. Later evidence-verifier checks are listed separately below.
- `check_completion.py`: **exit 1, expected**. Required implementation,
  conformance, independent review, full workflows and performance remain open.

These are checkpoint results, not independent approval of every new module.
The release build covers that checkpoint's lock, run-recovery and scoped-commit
changes, not the current dirty sources. No timing campaign has been run on it. Repeat gates after further edits
and bind any new performance evidence to the exact binary/source.
Logs: `/tmp/sk-native-reviewed-integration-{tests,clippy}.log` and
`/tmp/sk-native-reviewed-release.log`. The 455- and 1,276-test checkpoints are
historical. No full upstream Python-suite pass or macOS/Windows success is claimed.
Required kernel comparisons verify the lock-exact oracle and fail on absence/drift.

## Subsequent scoped checks — not a new aggregate

- Coordinator: actual binary dispatch for status emit/lifecycle and task move/mark
  passes an end-to-end post-state test. The fixture now includes the upstream-
  required `tasks.md` roster; its fresh focused rerun passes in
  `/tmp/sk-status-dispatch-roster-green.log`. Coordinator also reproduces **220
  status library passes / 1 parent-exercised helper** and **39 status CLI passes**
  (`/tmp/sk-status{,-cli}-remediation-coordinator.log`). Reviewer independence,
  explicit self-review, evidence/commit/feedback gates and legacy identities have
  scoped regressions, not independent approval. Rollback/commit orchestration,
  mirror YAML fidelity and coordination remain incomplete.
- Coordinator: checker/oracle **55/55 tests** after fixing evidence references to
  honor the audited `--root` and binding projected fixture identities to their
  bundle; this includes three root-isolation regressions.
  A prior temporary-CWD attempt omitted the repository from `PYTHONPATH` and
  failed importing the oracle tests; the corrected environment is documented.
- Independent semantic-evidence review reproduces **78 benchmark pytest tests,
  59 checker/oracle tests and 49 self-tests**, substantiating real fixture
  regeneration, stored mutation-byte/schema checks, command-template binding and
  structural sample chronology. It still finds an unbound observable: recorded
  before/unchanged hashes were not compared with regenerated per-file bytes.
  The owner has now delivered that binding, a shared pristine/clone normalizer,
  real synthetic input manifests and final-checker forgery controls, reporting
  **86 benchmark pytest / 61 checker-oracle / 49 self-tests**; independent
  re-review is pending. Coordinator's written-status identity/schema correction
  has **5 focused passes**. The registry self-digest is rebound and loads cleanly;
  independent re-review now reproduces all **86 / 61 / 49** results and closes
  the scoped input/identity binding findings. These changes do not promote
  workloads or lower floors. The source is dirty, binaries stale,
  and no build receipt or passing campaign exists. See the
  [semantic disposition](reviews/evidence-semantic-disposition.md).
- Delivered owner reports: doctrine **123** tests; runtime/store **224** tests;
  status **211 passed, 1 subprocess helper ignored**; migrations **46/136**
  implemented with 61 effects cases and 136×3 ledger vectors. These counts are
  scoped owner evidence, not coordinator aggregate certification.
- Earlier coordinator Git verification stopped at **228 native passes / 3
  helpers** followed by a doctest `E0463`; that was not a clean 232-test run.
  Independent review now reproduces **232 passes / 0 failures / 3 helpers**,
  including all 4 doctests, with scoped Clippy/fmt clean. It rejects two merge-
  conclusion defects. Coordinator fixed captured-index binding red-first (**233
  passes**), followed by an owner correction for immutable created-commit
  evidence, typed unconfirmed outcomes, corrupt-HEAD taxonomy and explicit
  64 MiB blob reads (8 MiB default retained). Coordinator now reproduces **246
  Git passes / 0 failures / 3 helpers**, including doctests, together with **235
  runtime/store passes** in `/tmp/sk-runtime-git-remediation-coordinator.log`.
  Independent review reproduces the 246 tests and required matrix but finds
  one remaining identity gap: historical same-parent reflog/HEAD candidates are
  not proof of creation during this call. That correction is pending. Workspace
  consumers must not blindly abort possibly landed conclusions. Every Cargo
  invocation shares a lease.
- Independent next/status/governance continuation reviews found blocking facade,
  prompt, context and profile defects. Store interoperability and the new lock
  transactions held up; those scoped approvals do not approve the facades.
- Coordinator red-first fixes restore all **26** pinned built-in profiles and
  settle legacy migration successes by normalized id (identity remains intact).
  **136 installer library tests passed** at that checkpoint. The two context
  expectations exposed by selected-root delivery are now settled by new exact
  Python comparisons, not weakened deny rules. Coordinator freshly reproduced
  **138 doctrine passes** at the first remediation checkpoint, then **141
  passes** (106 unit, 18 baseline, 15 live-oracle, 2 doctests) after matching
  non-Git provenance/reference degradation without dropping typed delivery
  (`/tmp/sk-doctrine-nongit-coordinator.log`). The recovered remediation also
  covers nested model defaults, org-floor field independence, links and aliases;
  independent review confirms those fixes but finds absent-key org defaults,
  retired/malformed governance behavior, model-title and diagnostic discrepancies.
  Those remain open. CLI required-floor/language/skip composition is now wired
  with real CLI regressions and is awaiting its own independent re-review.
- Recovered init remediation is freshly reproduced: **227 scoped installer
  passes / 0 failures / 2 helpers**, **8 CLI delivery passes**, and **1 oracle
  test comparing three complete post-states** (`claude,codex,vibe`, `gemini`,
  `copilot`). Two-process init exclusion, shared init/upgrade lock authority,
  conditional preset preflight and tolerated optional symlinks have regressions.
  Logs: `/tmp/sk-init-remediation-coordinator.log` and
  `/tmp/sk-cli-init-next-remediation-coordinator.log`. The latter also has **25
  runtime-next passes** and **26 compatibility passes**. The stale Claude slash-
  command expectation is replaced with actual skill/profile/orientation checks;
  a separate Codex test retains the canonical prompt-content assertions.
- Coordinator deterministically reproduces an owning-guard release failure with
  a duplicated file descriptor, then explicitly unlocks only in the owning PID.
  **190 kernel passes / 0 failures / 1 helper**, including mixed-language and
  reentrancy tests, pass in `/tmp/sk-lock-duplicate-release-green.log`.
  Independent review reproduces the 190 tests, scoped Clippy/fmt and public
  reentrancy/descriptor probes, and accepts this correction within scope. It
  could explain init's intermittent post-drop reacquire failures but does not
  prove their causal history; this is descriptor simulation, not a real fork
  test. See [the release correction](reviews/lock-release-duplicate-description.md).
  The native READY/stdin lock helper remains in place; no process-group signals.
- CLI library Clippy with `--no-deps -- -D warnings` passes after boxing the two
  large argument variants and collapsing one init condition. The subsequent
  compatibility suite passes **26/26** (`/tmp/sk-cli-compat-recovered-green.log`).
  This is not workspace/all-target Clippy or independent approval.
- Independent coordination review reproduced **129** workspace tests, but
  rejected bootstrap/planning/integration for CLI wiring. The recovered owner
  reports corrections for locks/protection, seed/root restore, pins, demotion,
  journal outcomes, integration rollback and numeric-slug lane names. Coordinator
  freshly reproduced **162 passes / 0 failures** including the doctest target
  (`/tmp/sk-workspace-remediation-coordinator.log`). Two earlier failures were
  real defects; two were stale fixture expectations corrected to the actual
  seeded JSONL bytes. This remediation still needs independent re-review before
  CLI wiring. Independent review then reproduced **163 passes** but found
  coordination directory naming, planning-lane dependencies, untracked root-copy
  loss, split lanes locks, unlocked recovery and dropped journal faults. The
  second owner correction addresses those findings and handles possibly landed
  merge conclusions without abort/rewind. Coordinator additionally corrects the
  Unix undecodable-path fallback lock digest red-first: **183 passes / 0 failures**
  and scoped Clippy pass (`/tmp/sk-ws-second-remediation-coordinator{,-clippy}.log`).
  Independent review reproduces **183 passes** and accepts those corrected
  library mechanisms. Merge-conclusion attribution stays conditional on Git's
  creation-evidence correction. Merge-driver parity, remote-default fallback and
  platform/concurrency gaps remain. CLI wiring is still unapproved.
- Fork `main` is current at upstream `8ffeb01805c7f11ccdcccc6fd607b29091bc36be`.
  The rewrite/reference source still uses the older baseline below. The
  556-commit reconciliation changes path authority, status and governance;
  unchanged dependency and pack pins do not make old source fixtures current.

## Oracle and committed conformance evidence

Source baseline: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Use `.venv-rust-oracle/bin/python`, provisioned with:

```sh
UV_PROJECT_ENVIRONMENT="$PWD/.venv-rust-oracle" uv sync --frozen --no-dev --python 3.11
```

Set `PYTHONPATH="$PWD/src"` and `SPEC_KITTY_PACKS_ROOT="$PWD/packs"`.
The installed uv-tool interpreter is not dependency-identical to `uv.lock` and
is not a certification oracle. Python is development tooling only; neither
native binary invokes it as a production fallback.

- Consumer replay: **38 fixtures / 468 rows**, regenerated under the lock-exact
  oracle. Includes 12 bounded real archives, malformed/array/null snapshots,
  absent/typeless metadata, Unicode and floating-point seed arbitration, and
  legacy/current/future schema-generation cases. Input/output digests and
  provenance are checked. No uncommitted 719-archive sweep is claimed here.
- Writer: five committed oracle-generated differential cases plus native
  refusal, interruption, readback and thread/process-concurrency tests. The
  independent writer review found payload and composition gaps. Coordinator
  regressions now cover batch workspace semantics, post-commit verification,
  UUID spellings and 2,056 finite IEEE vectors generated by the verified oracle;
  these fixes are not a new independent approval.
- Runtime: 34 differential goldens were independently regenerated identically;
  targeted review fixes have additional native regressions. Re-polled MEDIUM
  gates now clear the pending reason, preserving the upstream null title.
- Kernel: native Git-topology, lock-path, home/path and clock differential tests;
  cross-language lock contention and mixed generic-journal writer exercises.
- Installer: a pinned init oracle fixture and native differential checks for
  generated skills/assets, activation, metadata, ledger and preservation.
  **46 of 136** upgrade migrations have native execution in the delivered
  continuation; 90 remain absent and independent review is pending.
  Ledger locks now use the shared kernel state-directory/path resolver, with
  actual Python/native contention tests for unset, empty and relative XDG state.
  Init-seeded records no longer invent supersession outcomes.
- FSM: all 2,057 corpus rows are consumed; 1,700 non-genesis expectations match
  exact messages, while 357 obsolete genesis expectations have separate
  current-semantics assertions. This is not blanket legacy-message parity.

## Independent review and remediation

Implementation/review delegates use `tokenfactory/deepseek-v4.1-flash-1017`.
The coordinator integrates and reproduces findings. See
[the lifecycle checkpoint disposition](reviews/lifecycle-checkpoint.md) and
[the earlier kernel checkpoint](reviews/kernel-transaction-checkpoint.md) and
[the current correctness disposition](reviews/correctness-continuation.md).

Recent regressions cover corrupt-snapshot schema promotion, invented mission
types, clean audits of undecodable rows, Unicode seed ordering, empty prompt
fallback, malformed recorded soft-gate scores, symlink-hidden Git-directory
allocation, SSH BatchMode pins, CWD-dependent lane placement, user `status.json`
retention, recorded branch preservation, topology-changing repair and inherited
status-lock bookkeeping. Passing those tests is not a new independent approval.

## Input policy and remaining risks

- Replay bounds: diary **64 MiB**, snapshot **32 MiB**, metadata **1 MiB**;
  glossary seeds **8 MiB**. Oversized, special, unreadable and non-UTF-8 replay
  inputs refuse with paths. Present but malformed snapshot JSON selects legacy
  replay; it is not classified as absent or silently promoted.
- Bounded replay reads follow symlinks to regular files; canonical log writers
  additionally refuse final symlinks. Static containment is not a capability-
  based sandbox against hostile concurrent ancestor swaps.
- Standard JSON only: NaN/Infinity/overflowing floating-point tokens refuse.
  Arbitrary-precision integer tokens are preserved. Python accepts additional
  non-standard numeric inputs; shell-PID coercion and non-string scalar repr
  still need complete conformance. ISO validation does not certify all Python
  `fromisoformat` extensions.
- Atomic replacement now preserves file mode and syncs the parent on Unix;
  post-rename sync failure is indeterminate, not evidence of no mutation.
  Owner/ACL/xattr/hard-link and Windows durability equivalence remain open.
  Correct family locks are required for multi-file transactions; Mission
  alias/rekey integration and recovery are not certified by atomic replacement.
- Generic kernel O_APPEND is **not** used for either canonical status log.
  Status/lifecycle writers use whole-file replacement and readback. External
  event/tracker service delivery, handover and fan-out are still incomplete.
- Raw topology reads honor Git selectors, but writer authority boundaries now
  refuse inherited repository-selection variables until one captured context
  owns both lock discovery and the explicitly opened Git adapter. This prevents
  split-authority writes; it is not full override-mutation parity. Status-family
  corrupt-topology fallback and generic-journal strict refusal remain distinct.
- Git bounds captures and kills/reaps the direct child, not an entire remote
  helper process tree. Protection is supplied by the application; safe commit,
  complete live protection resolution and all recovery paths remain gates.
- Native initialization is not all upgrades, transports, profile projection or
  global-runtime provisioning. Explicit verified pack sources are still needed.
- Current CLI routes are a mixture of real but partial mutations, read surfaces
  and explicit refusals. Coordination-routed CLI materialization refuses rather
  than writing primary data. Creation/finalization have a new Mission-lock
  protocol awaiting review and complete coordination/commit orchestration.
  Full governed `next`, acceptance, consolidation,
  merge, decision ledger and service/tracker workflows are not certified.

## Performance

`benchmarks/rust/evidence/2026-11-02-initial/` is a **partial, older-binary
baseline**, not certification of the current source. Required equivalent
workflows are pending and five samples per side do not meet the inventory's
20-sample floor. Startup/projection/micro measurements cannot rescue the gate.
No overall ≥3× improvement or no-regression result is claimed.
