# Continuation review disposition — not a parity approval

Behavior reference: upstream `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
The reviewed sources were dirty deliveries on `d64468a76`; reports record their
hashes. Duplicate completion notifications are the same reviews, not new passes.

## Governed next

Report: `/tmp/sk-rust-next-continuation-review.md`.

**Accepted as reviewed:** run-store interop in both directions, cursor/journal
fidelity, human answers, read-only queries, legacy run tokens, contained/hash
prompt filenames and indeterminate-start preservation. Runtime/store tests were
224 green; these do not certify the CLI composition.

**Facade rejected:** blocked-iteration sentinels leak into public step/state;
missing canonical status loses diagnostics; unresolved custom mission prompts
can produce false step success. Ordinary prompts are placeholders rather than
the canonical pack content, board prompts omit the step, and review guidance
uses the wrong completion operation. Preview WP/tasks eligibility, origin tier,
pack-root interpretation and corrupt-journal envelopes also differ.

Prompt *content/identity preserved* in the implementation report is retracted.
The prompt path alone is not proof of executable instructions. Requirement and
occurrence gates, StepContract composition, decision-log commits and lifecycle /
retrospective behavior remain requirements, not harmless deferrals. Those findings required behavior and full-prompt corrections, not weaker tests.

Recovered implementation (`crates/sk-cli/NEXT-REMEDIATION-REPORT.md`) now uses
canonical prompts, requirement/occurrence guards, origin tiers and board/query
semantics. It also supplies the real org-required floor, active-language profile
filtering and skip diagnostics. Coordinator reproduces **25 next CLI passes**
and **235 runtime/store passes**, including doctests. Independent report
`/tmp/sk-rust-next-remediation-review.md` confirms the five source-backed fixture
corrections and approves the store seam, R1/R4–R8, mapping/occurrence guards and
CLI floor/language composition within scope. It still rejects full replacement:
the unknown-type check wrongly blocks valid non-WP custom steps; absent
StepContract execution lets protected-main composition falsely issue `analyze`;
WP prompt frontmatter is dropped. Decision-log warnings, rich/compact rendering
and prompt-location fidelity also remain gaps. Governance evidence is conditional
on the changing doctrine dependency, not a frozen full-facade approval.

## Status / task facades

Report: `/tmp/sk-rust-status-continuation-review.md`.

**Preserve:** typed lifecycle payloads/deltas, in-lock readiness, unbounded flat
waits, byte-local mirroring, verified rollback protocol and real dispatch.
Lifecycle matched four oracle fixtures. The shells are **not behavior-preserving**.

Required corrections include reviewer independence, explicit self-review
fallback validation/event persistence, actual for-review implementation-commit
gates, substantive feedback on rejection and rejection of inappropriate feedback
edges, default reasons, approval provenance, mark-status identity/format/exit
semantics, legacy nullable identity, dependency YAML comments and mirror model
validation. Numeric legacy slug lane naming belongs to the shared workspace
branch-name authority; no local alternative belongs in status.

Recovered implementation evidence (`crates/sk-status/REMEDIATION-REPORT.md`)
is freshly reproduced by the coordinator: **220 library passes / 1 helper** and
**39 CLI passes**. Existing assertions now follow legal seed edges, canonical
feedback text and transition-before-lifecycle ordering. Separate regressions
cover numbered-slug lane heads, nullable legacy IDs and corrupt committed-log
errors. The lane reader's four omitted-field defaults are fixed red-first by
the coordinator (5 persistence tests pass), rather than treating accepted Python
input as a bad fixture. None of this is independent facade approval.

Strict delta refusals and UTC offset spelling remain declared differences, not
claims of equality. Coordination, projections, commit/review/rollback composition,
valid YAML mirror forms, owned-checkout behavior and service delivery are still
incomplete at that checkpoint. The subsequent commit/rollback tranche is now
wired: coordinator reproduces **231 status library passes / 1 helper**, **48
status CLI passes**, plus **7 CLI library / 26 compatibility passes** in
`/tmp/sk-status-commit{,-cli}-coordinator.log`. Source config/flags, protected
preflight, real review-cycle files/commits and a 64 MiB committed-log rollback
probe are exercised; independent review is pending. The valid-YAML mirror now
uses a real parser and byte-local edits. No supported route becomes complete
from these counts; coordination, owned checkout and merge-ancestry remain gaps.

## Governance / migrations

Report: `/tmp/sk-rust-governance-migrations-continuation-review.md`.

**Verified:** canonical synthesis lock/path/real mixed-language exclusion,
inside-lock stale-plan guard, preserved empty/null/comment YAML, activation
floor-netting substrate, 136 catalog rows, 61 effects cases and 136×3 ledger
vectors. The 46 native migration registrations are real; 90 remain absent.

**Findings in the original review:** selected/org-required roots disappearing from context;
five skipped built-in profiles; legacy migration settlement; one malformed
org-floor field dropping other fields/inheritance; raw YAML bodies omitting model
defaults; extra excluded-kind links; org naming/schema/alias fidelity; overly
permissive body/link tests and overstated per-id oracle case counts.

Coordinator red-first corrections:

- Added the real `self-review-protocol` profile field. The regression now loads
  all 26 pinned built-in profiles without skips, including the five lost ones.
  Full populate-by-name/schema fidelity remains with the remediation owner.
- Settlement compares the normalized migration **id**, not the historical
  identity/drift key. The alias regression fails before the fix and passes
  after, without changing the original identity. All 136 installer lib tests
  pass (`/tmp/sk-ledger-alias-green.log`).
- Added an oracle-selected non-hub tactic regression and seeded selected roots
  in context. The two subsequently exposed unit expectations are now settled by
  exact live-oracle regressions: an explicit empty directive allow does not
  suppress the independent tactic catalogue, and a tactic deny removes that
  tactic rather than the whole catalogue.

Recovered doctrine remediation (`crates/sk-doctrine/REMEDIATION-REPORT.md`) covers
selected roots, all 26 profiles/aliases, independent lenient org-floor fields,
Pydantic nested defaults, exact typed arrays/link sets, org naming and schema
acceptance. Coordinator reproduced **138 passes, 0 failures**, including 12
live-oracle cases and 2 doctests (`/tmp/sk-doctrine-remediation-coordinator.log`).
Owner Clippy/fmt passed. This is scoped remediation evidence, not independent
approval or installer verification during concurrent init edits. The non-Git
provenance/reference degradation is subsequently fixed without changing typed
catalogue delivery: two non-Git oracle cases and a Git control bring coordinator
verification to **141 passes** (`/tmp/sk-doctrine-nongit-coordinator.log`).
Independent re-review (`/tmp/sk-rust-doctrine-remediation-review.md`) confirms
the original fixes but still rejects full context parity: absent-key defaults
omit org-graph artifacts; retired governance keys must hard-refuse rather than
succeed, while malformed governance shapes must degrade correctly; model titles
and diagnostic cardinality/text also differ. Those four findings remain open,
not carved out. The CLI floor/language/skip wiring is now implemented and has
real CLI regressions with the runtime facade, pending its independent review.

## Verification / publication boundary

The completion checker still refuses production coverage: 599 errors, 16 absent
strict benchmark workloads and no full passing workflow evidence. The synthetic
20-row positive control is test scaffolding, never a measurement. Independent
semantic review now reproduces **59 checker/oracle tests, 78 benchmark pytest
tests and 49 self-tests**, including real fixture regeneration, stored mutation
bytes/schema, command templates and structural sample chronology. Recorded
before/unchanged manifests still need binding to regenerated per-file bytes.
Coordinator has additionally strengthened written-status identity/schema parity
with stdout (5 focused tests pass); full suites are pending after that remaining
manifest correction. See [the semantic disposition](evidence-semantic-disposition.md).
No production coverage or performance status is promoted.

Git continuation supplies captured operator-index and execution primitives.
Coordinator reproduced 228 native passes but hit a doctest compile failure;
whole Cargo invocations now serialize on `/tmp/spec-kitty-rust-cargo.lock`.
No fresh full aggregate/release/benchmark acceptance follows from these checks.
Fork `main` is synchronized separately; rewrite semantic reconciliation remains
pending. Keep PR #1 draft and do not merge automatically.
