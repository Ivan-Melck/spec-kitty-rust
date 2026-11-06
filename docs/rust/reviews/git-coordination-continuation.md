# Scoped-commit corpus and coordination integration

Pinned Python reference: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
This is coordinator verification, not independent approval or whole-project
completion. Other implementation owners are still editing the shared checkout.

## Generated scoped-commit evidence

The delivered reproducible corpus has 32 digest-checked cases with input,
ref/index/worktree/commit/hook observations and a verified dependency oracle.
The coordinator corrected four prior assumptions using those observations:

- Ordinary safe commits explicitly force unsigned commits, including the hook
  config context; only pinned-parent commits honor configured signing.
- Ordinary hooks receive `GIT_EDITOR=:`; pinned hooks retain their inherited
  context. This is now compared with the oracle, not excluded from equality.
- Ordinary reflog uses the first message line, not the full first paragraph;
  pinned reflog uses the requested message, not its hook-rewritten body.
- Ordinary pre-commit runs for an initially unchanged snapshot, but its
  formatter changes remain uncommitted rather than rescuing that no-op.

At the historical `d64468a76` checkpoint: **30 exact matches**, **1 explicit
literal-path correction**, **1 open custom-index mismatch**. The subsequent
captured-operator-index delivery resolves the mismatch: independent review now
verifies **31 exact / 1 explicit correction / 0 open**. Case ids remain stable
even where an old id contains "divergence". Literal pathspec behavior is not
counted as equality. Captured `GIT_INDEX_FILE` now binds safe commits, their
private-index overrides, locks and reconciliation.

Historical `cargo test -p sk-git --locked`: **192 passed, 0 failed, 2 ignored** (parent-
exercised subprocess helpers). Strict crate Clippy and formatting pass. Tests
use temporary HOME/XDG/TMP/project roots. Logs:
`/tmp/sk-safe-parity-closeout-{tests,clippy}.log`.

## Git operations review and subsequent correction

Independent report `/tmp/sk-rust-git-operations-review.md` reproduces **232
passes / 0 failures / 3 parent-exercised helpers**, including 4 doctests; scoped
Clippy and formatting pass. The earlier shared-target doctest failure is not a
pass, but the dated fresh run now covers those same Git sources. Old raw probes
under `/tmp/sk-git-review2` no longer exist; findings not rebound from fresh
executions remain explicitly qualified in the report.

The review rejects merge conclusion as-is: its plan omitted captured-index
identity and its outcome read current HEAD rather than identifying the commit
created before a post-commit hook moved the ref. Coordinator reproduced the
first defect through a native regression, added `MergeConclusionPlan`'s index
binding, and verified refusal before either index, HEAD or MERGE_HEAD changes.
Red/green logs: `/tmp/sk-merge-index-binding-{red,green}.log`. The full scoped Git
rerun was **233 passes / 0 failures / 3 helpers**, including doctests, in
`/tmp/sk-merge-index-binding-scoped.log`. The subsequent owner remediation pins
created-commit evidence, returns typed unconfirmed outcomes, classifies corrupt
HEAD and supports explicit 64 MiB blob reads (8 MiB default retained).
Coordinator reproduces **246 Git passes / 0 failures / 3 helpers**, including
doctests (`/tmp/sk-runtime-git-remediation-coordinator.log`). Independent
report `/tmp/sk-rust-git-remediation-review.md` now reproduces 246 tests and
accepts the required matrix, but finds **F2-R**: unbounded historical reflog and
HEAD fallback can name a pre-existing same-parent commit. A rejected pre-commit
with a prior identical merge is misclassified as unconfirmed; a destructive
hook demonstrates false created-commit identity. Creation evidence must be
bounded to the call, not inferred from parents alone. That finding remains open.
Workspace consumers must not blindly abort an unconfirmed conclusion or discard
a possibly landed candidate.

## Coordination substrate

`sk-workspace` now executes coordination bootstrap/materialization/seeding,
transactional writes/commits, journal recovery, planning pins and fast-forward
integration. It also resolves protection, branch/lane tips and occupancy.
The CLI is deliberately not rewired merely because these APIs exist.

The coordinator found that `MissionHold` boxed the real guard but guessed its
key/path for the receipt. It now captures the real guard's key, path and holder
before boxing. Tests cover the recorded legacy key differing from the directory,
and the coordinate-less receipt reports the directory key actually held.
No concrete guard re-export was needed to call its public inferred methods.
The requested `serde_yaml_ng` dependency is declared; replacing the narrow
protection scanner is still outstanding.

Coordinator's initial scoped run: **127 passed, 0 failed**; strict Clippy
and formatting pass. Logs: `/tmp/sk-coordination-integration-{tests,clippy}.log`.
The independent review of `d64468a76` reproduced **129** tests, including the
receipt regression, and found the blockers below. Neither count is approval.

## Independent coordination review — original required fixes

Report: `/tmp/sk-rust-coordination-review.md`, frozen workspace sources at
`d64468a76`. Real temporary-Git oracle probes verified protection, divergence,
state classification, demotion and seeding behavior. The transaction core is
accepted for further integration only after correcting landed-outcome reporting;
bootstrap, planning and integration are **rejected for CLI wiring**.

- **P1-1:** bootstrap mints a branch before claim/Mission locks and protection;
  a refused protected destination leaves a branch behind.
- **P1-2:** planning commits/integrations lack the claimed Mission/claim holds
  and recovery journal; the integration recovery record has no writer.
- **P1-3:** planning uses the target instead of the canonical single-branch
  mission write branch, permitting a wrong destination or refusing valid work.
- **P2-4/5:** planning pin read/write loses concurrent updates and clobbers
  existing pins; demotion lacks authoritative HEAD bytes and misses empty/null.
- **P2-6:** seed lacks source-backed single-home restore and log validation;
  partial failure can leave an authoritative-looking empty directory.
- **P2-7:** recovery needs reachability for landed seeds/commits; a newer attempt
  must not label an unproven older intent abandoned. Journal failure after a
  landed commit must return landed evidence, not an ordinary failure.
- **P2-8:** permissive and strict read surfaces disagree on deleted/unmaterialized
  coordination; undeclared branches and stray directories need canonical rules.
- **P2-9:** dependency integration differs from upstream all-or-nothing behavior,
  loses earlier mutation receipts on error, and orders planning dependencies
  incorrectly. Missing branches are warn/skip upstream, not hard failure.
- **P2-10:** replace the YAML scanner: anchors/aliases can silently remove
  explicit protected branches; duplicate/multi-document configs are mishandled.

Verified correct: claim→Mission order *inside* `CoordTransaction`, actual guard
key/path/holder receipts, immutable landed commits, safe NotLanded classification,
foreign/untracked preservation, pin classification and seed trailer interop.
The blanket statement that every mutation holds both locks is not accepted;
lane-tip bookkeeping itself matches the reference's lock-free CAS behavior.

## Recovered coordination remediation — independent review still rejects CLI wiring

Owner report: `crates/sk-workspace/REMEDIATION-REPORT.md`. Coordinator freshly
reproduced **162 passes / 0 failures** (12 suite results, including the empty
doctest target) in `/tmp/sk-workspace-remediation-coordinator.log`. Owner scoped
Clippy/fmt pass. The cancelled partial tranche's four failures were classified:

- Failed bootstrap rollback left the minted branch checked out. The correction
  re-takes claim/Mission holds and removes only a clean, registered, tip-pinned
  worktree before guarded branch deletion, retaining tip evidence.
- Root-copy restoration compared status paths against a composed directory name
  instead of the actual legacy Mission directory. It now uses the real path.
- Two rollback assertions still expected Markdown after the fixture switched to
  validated JSONL. They now compare against the shared original seed bytes.

The owner reports source-backed fixes for the original findings, including
locks/journals on planning/integration, pin/demotion guards, single read authority,
all-dependency rollback/receipts, YAML aliases/merge keys and canonical lane names.
Independent report `/tmp/sk-rust-workspace-remediation-review.md` now reproduces
**163 passes / 0 failures**, including the coordinator's lane-reader defaults
correction. It approves the original lock/protection/bootstrap rollback,
pin/demotion, read-authority and naming mechanisms only within the reviewed
scope, and **still rejects CLI wiring** for these remaining defects:

- Coordination Mission directories use the prefix-stripping composer, creating
  a second home for legacy numeric slugs; use the distinct coordination grammar.
- A planning-lane dependency is passed to the code-lane branch resolver instead
  of resolving to its target branch.
- An untracked root copy differing from the coordination tip can be deleted
  without its bytes being committed, while its receipt says preserved.
- Lanes writes use an in-tree lock rather than the reference common-directory
  lock: Python/native writers do not contend, and lock residue dirties later
  integration checkouts.
- Recovery resolves records without the claim/Mission holds, and integration
  drops journal-resolution errors instead of retaining fault evidence.

The second remediation (`crates/sk-workspace/SECOND-REMEDIATION-REPORT.md`)
addresses these findings with shared directory/branch composers, byte-proven
root-copy removal, real mixed-language canonical lanes locks, locked recovery,
explicit journal faults and strict UTF-8 log decoding. Possibly landed merge
conclusions are retained without abort or rewind. Coordinator adds a real-oracle
Unix undecodable-path lock-digest correction and reproduces **183 passes / 0
failures**, scoped Clippy clean. This is pending independent re-review, not CLI
approval. Merge-driver composition, remote-default fallback and platform/
concurrency coverage remain real gaps, not waived by the fixes or test count.
Git dependencies have their own review boundary above.
The report's old `merge_base`-missing claim is incorrect: the public primitive
exists and is already used. Do not add a second implementation.

## Required follow-through

- Correct these reproduced coordination findings before CLI integration; retain
  independent-review rejection until fixes and re-check evidence exist.
- Review and compose the delivered native Git blob/tree reads, merge-start and
  merge-base, guarded pruning, sparse checkout, remote HEAD and captured
  operator-index support; close the merge-conclusion and blob-bound findings.
- Actual source-backed restore/merge behavior instead of current explicit
  `RestoreRequired`/`MergeRequired` refusals; complete YAML protection parsing.
- Full CLI coordination/planning/implement composition only after review and
  functional gaps are closed.
- Non-Linux, SHA-256 and submodule evidence; no performance claim is inferred.
- Reconcile newer upstream main after owner edits stabilize. Fork main is
  synchronized separately; the current oracle remains the declared older pin.
