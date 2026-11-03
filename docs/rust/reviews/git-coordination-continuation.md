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

After regeneration: **30 exact matches**, **1 explicit literal-path correction**,
**1 open custom-index mismatch**. Case ids remain stable even where an old id
contains the word "divergence". Literal pathspecs commit only requested paths
where the reference can commit unrelated files; this is not counted as equality.
`GIT_INDEX_FILE` still refuses rather than honoring the caller's index, and is
not counted complete. Captured operator-index support remains required.

`cargo test -p sk-git --locked`: **192 passed, 0 failed, 2 ignored** (parent-
exercised subprocess helpers). Strict crate Clippy and formatting pass. Tests
use temporary HOME/XDG/TMP/project roots. Logs:
`/tmp/sk-safe-parity-closeout-{tests,clippy}.log`.

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

`cargo test -p sk-workspace --locked`: **127 passed, 0 failed**; strict Clippy
and formatting pass. Logs: `/tmp/sk-coordination-integration-{tests,clippy}.log`.

## Required follow-through

- Independent coordination transaction review, including rollback/readback,
  claim/Mission ordering, seeding, protected destinations and ownership proofs.
- Native Git blob/tree reads, merge-start and merge-base, guarded pruning,
  sparse checkout, remote HEAD and captured custom operator-index support.
- Actual source-backed restore/merge behavior instead of current explicit
  `RestoreRequired`/`MergeRequired` refusals; complete YAML protection parsing.
- Full CLI coordination/planning/implement composition only after review and
  functional gaps are closed.
- Non-Linux, SHA-256 and submodule evidence; no performance claim is inferred.
- Reconcile newer upstream main after owner edits stabilize. Fork main is
  synchronized separately; the current oracle remains the declared older pin.
