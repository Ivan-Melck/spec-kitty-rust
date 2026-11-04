# Lossless profile transport: coordinator decisions and counterexamples

Design `/tmp/sk-profile-lossless-transport-design.md`, original evidence
`/tmp/sk-ea81-p7-loader.LFZ7Uo/command-log-index.tsv`, remains unchanged.
Coordinator provisional `/tmp/sk-profile-transport-coordinator-assessment.md`
preceded its own probes. No profile transport has been implemented/approved.

## Useful evidence, not full closure

Source ruamel preserves each `\uD83D\uDE00` escape as two code points, not one
astral scalar. The reader-level serde refusal is real; changing only an id type
cannot repair it. yaml-edit raw tokens/byte ranges are useful; its decoded
surrogate-as-six-ASCII-chars value is NOT Source data. Terminal aliases require
actual alias resolution.19 selected cases provide useful scalar transport
facts, not a full-profile, mapping-key or layered-merge conformance proof.

## Two new Source-first POC failures

Fresh evidence `/tmp/sk-profile-transport-assessment.fjBja7/command-log-index.tsv`.
Exact3.11.15/53: `exact-oracle.HR9MnO.log`; outside copied POC build
`build-shadow-countercases.Ajdbs0.log`, absolute repository target/focused lease.
Original probe/scripts/fixtures were not rewritten. Unique binary name avoids
overwriting the original POC binary. Source and Native assert temporary CWD/
HOME/allXDG/TMP; Source runs/asserts each case before the Native counterpart.

`source-first-shadow-countercases.zq6ZoF.log` fails BOTH actual comparisons:

1. A real Source-accepted tactic-reference merge inherits `rationale: why` into
   the second entry. The POC's direct serde parse has **None** there. Explicit
   `Value::apply_merge()` produces `why`; that existing library operation is
   available but is NOT automatic. The original POC only compared tactic ids,
   not rationales, so the broad “field-for-field / merge already handled” claim
   exceeds its assertions. No new merge implementation is needed, but correct
   use and exact overlay carriage still need proof.
2. A Source-valid `excluding` mapping has distinct keys `"\ud800"` and
   `"\u0000"`, each with an authored list. Source accepts/preserves both. The
   same-width surrogate-to-NUL shadow coalesces them before the semantic parse,
   which then rejects a duplicate key. Its replacement value IS consumed for
   key equality, despite the claim that values are discarded. This shadow is
   not production-ready or injective over Source-valid mappings.

These are POC/design failures, not regressions introduced into production.
They do not invalidate the useful19-case facts. No lossy escaped/PUA/fake id,
silent skip or new unsupported-input class is adopted as a solution. Any future
private transport must preserve exact key equality/types/alias+merge behavior
before model validation, with structured paths, and full authored-field tests.

## Decisions

**D-A: H1a chosen for a FIRST, SEPARATE foundation slice.** Move the ONE existing
PythonString/InvalidCodepoint type, canonical repr formatter and existing
CPython14 printability facts to sk-kernel. Keep `sk_invocation::PythonString`,
`InvalidCodepoint`, `python_repr`, all current methods INCLUDING inherent
`PythonString::repr()` unchanged through aliases/re-exports. No duplicate type
or formatter, no dependency cycle/new dependency. The relevant facts file is
**789 lines /701 ranges**, not the2974-line ROUTER lower/word table. The router
facts stay private in invocation. H1b's public repr removal is NOT approved.

**D-B: M2 direction only, not authority to implement a loader yet.** Prefer one
canonical rich parse/model with an explicit scalar compatibility projection,
not a breaking `AgentProfile.id/body` change, second loader or fake scalar id.
Catalogue must eventually consume the exact rich model. Merge/union/excluding/
lineage must retain every exact field/key; a path overlay silently lost on any
composition is not acceptable. This needs a corrected POC and exact contracts.

**D-C: no invented non-scalar DRG refusal.** Source identity/node/URN/ancestor
behavior must be established before any such policy. Inability of a current
Native String-keyed graph is not a Source refusal and cannot be blessed as
parity. No graph code or guessed empty lineage is authorized in H1a.

**D-D:** H1a requires no manifest/lock/external package/version/pin change.
**D-E:** a private scalar decoder may be justified with full pinned Source
conformance; current19 cases and colliding shadow are insufficient. Source
valid profiles remain required, not declared unsupported until upstream fixes.
All2048 surrogates, escaped literal/pair/astral/PUA, names/roles/unused fields,
keys, full references, aliases/merges, line ending/folding/tags/errors and real
layer/lineage cases remain necessary for later transport implementation.

D2 strict-vs-lenient org consumers must remain distinct through one resolver;
D3 legacy flat overlay reuses the existing loader and builtin merge, ungated
AND language-unfiltered, collision wins. Neither is implemented by this plan.

## Pre-edit occurrence classification

The move is structural, not a public terminology/API rename. Loaded the
bulk-edit skill and actual template/schema; classified all eight categories
and exact move plus per-file exceptions BEFORE any production move:
`docs/rust/occurrence-maps/python-string-kernel.yaml`.
Inventory `primitive-occurrence-inventory.KV37eP.log` separates live imports/
implementations/generator destinations from immutable historical reports and
existing public oracle callers. Serialized keys/CLI/logs/errors/fixtures/public
names stay pinned; no sed-everywhere or report rebinding. The schema validates
(`occurrence-map-isolated-validation.uz4c1z.log`). This rewrite is not advancing
an active host Mission, so no historical/fake Mission metadata is mutated and
no runtime Bulk Edit Gate pass is claimed. First validator invocation used repo
CWD; it is administrative schema checking, not accepted Source behavior proof;
repeat uses asserted isolated roots. Original log retained.

At this design checkpoint, only the H1a primitive relocation was next authorized.
It is subsequently implemented and coordinator-verified, with independent review
pending: [current H1a disposition](shared-python-string-disposition.md). The
following pre-implementation counts and unchanged-production statement are
historical. This does not approve or fix the transport POC. No catalogue, reader,
profile API retype, graph, activation, global serde, Source, corpus or pin edit.
Production behavior/data remain unchanged so far; current router NARROW PASS,
2472 coordinator tests,599/PERF27/0-of-16 and full workflow/platform/performance
requirements remain. PR stays draft; no merge/release.
