# Lifecycle checkpoint — coordinator disposition

2026-11-02. **Not a shipping approval.** This records independent findings and
coordinator remediation. Reviews were read-only; fixes below are coordinator
changes. The complete rewrite, command conformance and performance gates remain
blocked. Re-checks, when needed, must be new independent review tasks.

## Consumer status replay / schema review

Fixed with native regressions and refreshed pinned-oracle fixtures:

- P1: malformed/non-object existing snapshots retain their presence and select
  legacy replay, including absence of the implementer field.
- P2: absent/typeless metadata emits an empty mission type, not software-dev.
- P2: unknown kinds, malformed annotations and wrong scalar field types cannot
  pass a clean structural audit; raw rows pass the reducer decoder too.
- P3: seed canonical-min uses the shared Python-shaped JSON encoder, including
  ASCII escapes and finite-float exponent/threshold formatting.
- P4: invalid calendar dates/lowercase z/leap seconds are rejected; minute-only
  timestamps are accepted. Complete Python ISO grammar remains unproven.
- P4: non-UTF-8 diary errors retain their path; LF/CRLF/CR line numbering is
  covered. Bounds and stricter-input policy are now in VERIFICATION.md.
- API warning: reduce_rows explicitly documents that it is not materialization.

Open: full arbitrary scalar repr/coercion, shell-PID integer forms/ranges,
non-standard JSON numerics, complete validator message/grammar parity and
all multi-timezone/non-ASCII combinations. Bounded read safety does not imply
symlink containment. No 719-archive or overall performance claim is accepted.

## Runtime / Git review

Fixed with targeted regressions:

- D1: new worktree paths compare resolved ancestors against canonical Git dirs.
- D2: explicit ssh GIT_SSH_COMMAND receives BatchMode under the same rule as
  core.sshCommand; subprocess tests pin both paths and no duplicate injection.
- D3: empty prompt falls back; empty action profile inherits the source profile.
- D4: deserialized scores are validated before soft-gate persistence, matching
  the pinned engine's SignificanceScore.model_validate call. Test scores now
  contain valid complete dimensions rather than accidentally relying on the bug.
- D6: MEDIUM re-poll produces reason:null and the original null step title;
  a round-trip regression pins the shape.
- D10: enum name lookup uses each variant's own wire name, not parallel arrays.

D4 nuance: the Python engine's _significance_band reads the declared band without
validating the whole score. Full model validation is on the medium soft-gate
write path, not every low/high authorization. The fix follows that source
boundary rather than introducing an undisclosed stronger policy everywhere.

D5 now has a nondestructive refusal for detected embedded/bare repositories,
including ignored subtrees and expanded bare-repository paths. The bounded scan
runs during classification at plan and apply; 36 lifecycle tests pass, including
real nested histories and histories appearing after planning. This is not a
sandbox against hostile concurrent filesystem swaps.

Open: D7 live protection revalidation at apply; D8 committed-but-unreadable mutation outcomes; D9 stale
Git module documentation; Python-title-case details; CLI template drift/protection
and recovery end-to-end gates; kernel/Git topology agreement and platform CI.

Additional reproduced test instability: ETXTBSY on newly written executables
under concurrent fork/exec. Only pre-exec ExecutableFileBusy is retried, within
the original invocation timeout and a 100 ms cap. A permanently busy executable
regression proves the bound; running/failed mutations are never blindly retried.

## Mission / workspace review

Fixed with regressions:

- F1: lane placement anchors to the primary checkout; linked-CWD prediction and
  existing-lane reuse produce the same path.
- F2: arbitrary config/status.json and similar user files are not residue.
  Retirement refuses and tests prove their bytes survive. Only .spec-kitty
  runtime paths qualify for residue classification.
- F4: artifact/finalization rollback records intent before atomic replacement,
  including a post-rename indeterminate outcome (matching the create writer).
  Existing failure tests pass; a dedicated post-rename cross-crate fault-injection
  regression is still required.
- F6: single-branch recomputation refuses previous code-lane manifests or frozen
  code-lane pins instead of silently relocating started work.
- F7: re-finalization preserves the recorded integration branch.
- F10: abandoned marking cannot authorize a contradictory recorded topology or
  coordination-branch rewrite; planning refuses without modifying the tree.

Still blocking complete mutation support:

- F3: replace the old per-checkout mission-write.lock with the actual keyed,
  common-dir-anchored Mission lock, including aliases/rekey holds. The CLI
  status materializer now uses the canonical status-family door, but the
  authoring writers and alias protocol need integration.
- F5: allocation/journal failures may follow successful Git mutations.
- F8: unknown vs explicitly gathered empty freeze evidence.
- F9: production exhaustiveness guards.
- F11: complete OwnedCheckout branch/ownership/worktree proofs.
- F12/F13: task parser forms and refusal-code/existence policy parity.
- F14/F15: coordination EMPTY reads, planning-commit metadata/ancestry guard,
  coordination materialization, merges, lane tips and occupancy.

Creation receipts remain PlanningArtifactsOnly. Coordination-routed CLI status
materialization refuses before mutation rather than writing the primary copy.
The CLI must not call these partial paths a complete upstream workflow.

## Kernel continuation

The family/topology/path tranche has local differential and cross-language tests,
not a new independent approval. Coordinator additionally PID-fenced StatusLock
sidecar cleanup and status-family TLS state: inherited child bookkeeping must
not become authority or erase the parent's holder. A stale-PID regression is
present; complete native-fork/platform verification is still a review obligation.

## Evidence boundary

The integrated Linux checkpoint passes 1,276 tests and strict Clippy; the replay
corpus is now 38 fixtures / 468 rows. These are regression evidence, not approval
of the unreviewed run store, doctrine, installer, writer or full CLI composition.
Only three of 136 migrations execute natively. Benchmark evidence remains
partial and cannot satisfy the full-loop gate.
