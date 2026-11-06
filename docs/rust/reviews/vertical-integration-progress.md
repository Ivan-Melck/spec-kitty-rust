---
type: reference
updated: 2026-11-04
audience: software-engineer
---
# Vertical delivery integration — unpublished work in progress

The operator authorized broad fork changes and larger coherent delivery slices.
Four implementation tracks now supply real profile/catalogue, invocation/StepContract/
next, installation/ledger, and Mission acceptance/review/consolidation adapters.
Root CLI wiring is real (`dispatch`, `profile-invocation complete`, `profiles list`,
`upgrade`, `migrate`, `review`, `accept`, `consolidate`, Source rename-only `merge`).
The CLI gains one local Invocation dependency and its required lock dependency row;
no external package/version/pin changes.

**This is not a release, complete rewrite or full Source-parity verdict.** Source
9aed and remote-main semantic-adoption boundaries remain unchanged. The current
working tree is intentionally unpublished pending further completion and independent
review. Earlier e38b075 H1a review is NARROW PASS WITH FINDINGS at that fixed
subject only; it does not automatically approve these new surfaces.

## Actual coordinator evidence so far

After all four original producers froze: **2652 passed /0 failed /21 parent-exercised
helpers /213 suites**, including real CLI next/Op and acceptance/default-squash/
review/resume exercises. Strict workspace all-target Clippy, full fmt, strict
workspace rustdoc, all-target offline check and both release CLIs/optimized
Invocation build pass. These covered gates do not certify untested Source inputs.
Evidence `/tmp/sk-vertical-integration.TwDlpw/command-log-index.tsv`, notably:
`aggregate-fourth-integration.EWXs7h.log`, `strict-second-integration.lgcSua.log`,
`fmt-integrated.9oP2eE.log`, `docs-integrated-fifth.MCY2g7.log`,
`offline-integrated.T39jS2.log`, `release-integrated.zFc3jH.log`.

First integration failures remain retained: a moved generated-data test's Kernel
self-import (fixed test-only self-alias, not numeric facts), the obsolete native
unsupported-upgrade expectation (replaced with real conflicting Source flags),
and two Source/native non-Git lock comparisons using DIFFERENT TMPDIR authorities.
The latter now run both Source and Native in matching isolated subprocess roots;
no production lock path was changed. Strict docs found new redundant/broken links
and old private links; corrected comments, no warning suppressions. A's unused
test helper and C's unused mut were removed; D's unused recursion argument removed.
No compiler/style/harness failure is represented as a business red.

## Five real profile counterexamples caught and fixed by coordinator

Original all-Source-first comparisons at
`/tmp/sk-profile-handoff-counters.N8vIXj/source-first-profile-countercases.8ZDX80.log`
fail all five checks on A's delivered transport:
1. valid literal marker-shaped authored name rejected;
2. a literal marker silently turned into a surrogate when another field produced
   that marker;
3. trailing spaces after a surrogate lost;
4. invalid YAML backslash-single-quote accepted after the shadow hid it;
5. a Source-valid custom Greek role lowercased with ordinary sigma instead of final.

Fixed directly: marker-shaped string DATA is encoded too (no reserved authored
noncharacters), trailing pending whitespace retained, invalid quote escape not
hidden, block-scalar structural terminator retained, and half-open roles use the
shared EXACT CPython3.11.15/UCD14 lower authority. Fresh Source-first five-pair
positive `/tmp/sk-profile-handoff-fixed.0yaKHD/source-first-profile-positive.3ehV8H.log`.
Expanded permanent transport matrix28 cases preserves original20 plus literal
marker styles/collision/trailing spaces/custom Unicode roles, with Source and
Native isolated child assertions. A block replacement newline defect was found
by this expansion and fixed; original failure retained.

Existing lower/word/FinalSigma algorithm and byte-identical facts now live in
Kernel `python_text`; Invocation retains private callable aliases and original
unit/exhaustive oracles. Data SHA256 remains
`8d2f6e9c25c5f7efcdaa0d2e9dab145c80f00fdd0c0656bca5056ada73041df1`.
Generator destination only follows the move, all numeric facts/pins/digests intact;
--check/11 Python controls pass. Pre-edit eight-category classification and exact
move: `occurrence-maps/python-text-kernel.yaml`, schema validated in isolated roots.

## Required continuation, not accepted by disclosure

Installation is **55/136**, not complete:81 actual migration effects remain;
compat-planner/preview envelope, administrative backfills, worktree/auto-commit/
finalizer paths and other Source contracts remain required. Effects are being
split by catalogue target-version solely for non-overlapping implementation
ownership, never runtime version-window selection.

Profile conformance needs full quoted/typed merge-key, constructor-error/order,
YAML/Pydantic and explicit non-scalar DRG/identity/lineage coverage. Native String
limitations or Source graph-with-no-edge tests do not justify guessed ancestry.
Invocation still has declared non-scalar/wire/model/glossary/contract-binding
residuals; typed unsupported is not parity. Lifecycle still lacks exact Source
squash merge-driver/hook protocol, rebase and some real gate/evidence/bookkeeping
semantics. Post-publication tree comparison is not proof of correct Source
publication and must preserve actual landed/candidate uncertainty.

Completion/performance inventories are not promoted from library counts or a
happy-path CLI chain. Full workflows/platform/real-fork/required upstream adoption
and genuine ≥3× equivalent-work campaign remain open. Draft PR only; no merge/release.
