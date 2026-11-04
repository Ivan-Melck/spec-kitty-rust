---
type: explanation
updated: 2026-11-04
audience: software-engineer
---
# Canonical Git byte-message and target-preflight increment

**Historical design, not acceptance.** First Git-only increment now implemented
and coordinator-tested; fresh independent review pending. Current code/evidence/
source corrections: `crates/sk-git/BYTE-MESSAGE-TARGET-INTEGRATION-REPORT.md`.
Original plan follows. Final architect proposal
`/tmp/sk-git-byte-message-preflight-design.md`; immutable provisional retained;
coordinator amendments `/tmp/sk-byte-message-coordinator-assessment.md`.
Source remains9aed3a8012cb5eb22c0128b14d525039f95fea7a.

## First bounded implementation: Git only

- Opaque exact-byte CommitMessage with escaped Debug, no lossy/re-encoding or
  Python-specific knowledge in Git. Existing UTF8request/plan constructors AND
  public message()->&str getters preserved by strict legacy wrappers/default
  payloads; no panic or fake UTF8 for byte plans.
- Message-free target request/plan plus typed failure stage/progress. One planner
  body, legacy Environment→Message→Shape→paths→HEAD/tip→guard→operation order/error
  values unchanged; target route omits payload. Only a prepared actual payload
  yields an applicable byte plan; same canonical transaction/index/CAS/evidence.
- Exact bytes written through current commit-tree/file/cleanup/hook/signing path;
  successful Git stderr warnings retained. Preserve existing UTF8reflog/data;
  Debug is not wire/audit data. Raw reflog capability needs exact typed bytes or
  explicit residual—not silent replacement/blanket escaped-data change.
- Promote/reuse existing full symbolic HEAD read in place with RefName result;
  no widening RefName to HEAD, generic normalization or second writer.

No Op wiring/claim of F4 or encoding-phase closure in this first Git increment.
Actual byte-commit API must be exercised against real Git/source ordinary routes,
including default/ISO8859-1 encoding, hooks, alternate/dirty index, warnings, actual
OIDs, unchanged UTF8corpus and landed/uncertain evidence. NewAPI absence compile
reds are distinct from behavioral reds; existing UTF8request+hook injecting raw
bytes exposes dropped-warning/reflog behavior through the pre-existing entry point.

## Amended later composition, not waived

J(source-judged)/P(physical) source resolver and policy projection remain one Op
authority. Explicitly distinguish SourceRefuses from SourceAllowsButNativeCannotPublish
(no plan)—never Ready without a plan or falsely label a native-only limitation as
source refusal. Unavailable WorkspaceError/GitError keep typed provenance.

Canonical phase progress alone does not prove source path-semantic equivalence.
D1 unrepresentable P still needs source path proof before protected/invalid verdict.
Actual source probes `/tmp/sk-byte-target-coordinator.7XCyGi`:
protected/invalid refs/x with missing sentinel refuses; sentinel DIRECTORY admits
(4 status-read-only cases). Proposed early D1 guard would be wrong in directory
cases. Actual doctor close with missing spine+high surrogate fails staging BEFORE
encoding, not UnicodeEncodeError (HEAD/record unchanged).

Source encoding-stage index residue is observable, regardless of whether intended.
Encoding after target but before mutation is NOT source7→8 order and does not close
stage-failure/encoding/index-on-error footprint. Canonical prepared/staging lifecycle
is required for that later increment; no Op raw add/commit or guessed rollback.
Unborn publication must be root/no parent+absent-ref CAS; never seed P from J.
Exact-existing refs/Windows/fsencoding/error/reflog limits stay explicit until built.

Real-Git design route probes show equal objects only with matching metadata,
cleanup/newline handling. Retained first log has stdout/stderr/command harness
errors, run2 empty, run3 missing-LF object mismatch; these are not behavioral reds
or blanket route-equivalence proof. Later run4/5 matching cases establish the useful
byte-file route capability. Historical errors/data retained, not retried to green
without diagnosis. Full design/probe references are in coordinator assessment.

Implementation/review sequence: Git capability → reviewed stable publication →
source-grounded Op J/P/path/guard/late payload composition → failure footprints/
unborn/exact refs/platforms. Each step remains incomplete where source differs.
All aggregates/publication coordinator-only; PR draft; no workflow/performance or
whole rewrite approval follows from a plan.
