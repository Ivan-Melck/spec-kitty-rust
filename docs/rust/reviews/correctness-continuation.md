# Native correctness continuation — 2026-11-02

This is a checkpoint disposition, **not completion or independent approval**.
The purpose is functional parity and preservation during the native rewrite.
Pinned reference: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.

## Integrated and reproduced

- Mission-write alias/rekey/registered-hold protocol now backs the three native
  authoring doors. Implementation report: `crates/sk-status/MISSION-LOCK-REPORT.md`.
  Independent protocol review remains required.
- Upgrade execution increased from 3 to **16/136** migrations. Unsupported
  selections still refuse before mutation; 120 implementations are outstanding.
- Run-store nullable baseline cursor identities reconcile through validated
  references/index/input identity, without rewriting nullable fields. Repeated
  open MEDIUM audit answers replay correctly. In-repo absolute legacy run paths
  are accepted; nonempty step ids retain their identity while derived prompt
  filenames remain portable. An indeterminate index commit never deletes a run
  that the visible index may name.
- CLI queries no longer create runs. No-run preview is still unimplemented and
  explicitly refused. Human `next --answer`, answer/result validation, agent
  preflight, literal `native` argument handling, canonical protection config and
  the primary-checkout coordination default now have regressions. Omitted
  topology cannot silently turn into lanes; coordination remains unimplemented.
- Empty/null/comment-only doctrine YAML now becomes a real mapping; scalar and
  sequence roots refuse. Graph-only anti-pattern ids no longer require files.
  Shared ledger transaction, profiles and full context rendering remain work.
- Managed-skill manifest destinations are validated before repair/save.
  Initializer recovery/full delivery remains incomplete. Memory discovery now
  reads the actual checkout source. Seeded ledger rows do not invent supersession
  outcomes. Ledger locks use the kernel's reference-compatible state directory
  and non-strict path resolution; real Python/native contention covers unset,
  empty and relative XDG state with a nonexistent ledger parent.
- Canonical batch transitions do not invent workspace evidence. A failed
  post-commit read/check retains an explicit potentially-committed outcome.
  UUID-envelope migration matches ten verified-oracle cases, including accepted
  alternate spellings and a numeric legacy value, preserving original fields.
  Numeric rendering matches **2,056 finite IEEE vectors**, including the reproduced
  decimal tie; this is not exhaustive floating-point or nonstandard-JSON parity.

## Independent scoped-commit review disposition

Full original report: `/tmp/sk-rust-safe-commit-review.md`, bound to file hashes
before these fixes. The review is not approval of the subsequently changed bytes.

| Finding | Coordinator disposition |
| --- | --- |
| F1 moving-HEAD reconciliation | Reset and residue verification target the immutable landed commit, including extras. Deterministic post-CAS competing-commit regression covers ordinary and pinned routes. |
| F2 `./` path refusal | CurDir components normalize away; actual dot-relative commit covered. Existing parent-directory restrictions remain explicit. |
| F3 message bytes / empty message | Ordinary route uses configured git stripspace cleanup before message hooks and after their rewrites; cleaned-empty messages refuse. Pinned route retains verbatim semantics. |
| F4 ignored `GIT_INDEX_FILE` | All thirteen removed selectors now explicitly refuse at plan/apply boundaries. Both indexes/ref remain unchanged in subprocess tests. Full captured-environment support is still outstanding. |
| F5 hook text mistaken for old Git | Version is probed independently; rejecting hook name/output survive usage phrases. |
| F6 ambiguous post-hook ref failures | New typed `SafeCommitAdvanceUnconfirmed` carries candidate and observed tip on both failure paths. Reachable candidates are reconciled after readback. Nested-hook commit regression retains evidence rather than claiming nothing happened. |
| F7 reflog | Ordinary subject uses final committed message with `commit:`; pinned message retains its body through git's reflog normalization. |
| F8 no-op skips hooks | Hooks now precede no-op classification. |
| F9 unreachable commit failure | Real commit-tree/signing failures construct the documented typed variant. |
| F10 weak differential artifacts | Still open: require committed regeneration, input/output provenance and broader independent recheck before CLI wiring. |

Repair argv also includes `--literal-pathspecs`, matching the live writer rather
than exposing a different path interpretation to the operator. Later generated
oracle evidence corrected the signing assumption in this checkpoint: ordinary
upstream routes explicitly force unsigned commits; only pinned routes honor
configured signing. See `git-coordination-continuation.md` for that correction
and exact hook/reflog/no-op comparisons.

## Actual aggregate evidence

- **1,449 passed, 0 failed, 5 ignored** in 141 suite results; ignored subprocess
  helpers are exercised by their parents. Temporary HOME/XDG/TMP roots were used.
- Strict whole-workspace Clippy, formatting, and fresh release build passed.
- 39 Python checker/oracle tests; 19 harness self-tests and 16 untimed
  compatibility adapter/fixture tests passed.
- Completion checker still exits **1**. Benchmark methodology review found real
  gaps; no new timing campaign or overall speed acceptance is claimed.

Still required: full init recovery/CLI delivery, shared activation transactions,
complete governance/context/profile behavior, coordinated Mission execution,
full governed next, payload/projection composition, remaining migrations,
accept/consolidate/merge, decision/services/tracker contracts, platform gates,
and full-equivalence performance workloads with at least 20 samples per side.
