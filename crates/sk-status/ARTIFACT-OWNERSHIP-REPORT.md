# Review-artifact ownership/concurrency correction — coordinator delivery

Baseline `bfc79fdfd413e69059b6a1475d31e70317f8b6b8`; independent input:
`/tmp/sk-rust-status-second-recheck.md`, F1c BLOCKING. Provisional work record:
`/tmp/sk-status-f1c-remediation.md`, written before changes. Implementation is
not independent approval.

Subsequent independent review at `1ad605bb7` **approves F1c only**:
`/tmp/sk-rust-artifact-ownership-recheck.md`. It independently verifies the
library ownership/error controls, six real-CLI rejection/approval races,
canonical hold scope and retained evidence under fault/commit failures.
See `docs/rust/reviews/artifact-ownership-disposition.md`. The review explicitly
does not approve the whole tranche, unported routes, platforms or performance.

## Defect and correction

The old `write_new` cleanup included failed `create_new`: an `Existing` error
unlinked a file this call never created. A concurrent successful transition
could then retain a dangling `review_ref`. In addition, no-auto-commit cycle
allocation happened before any enclosing Mission hold.

- Exclusive create now completes **before** entering the fallible body-write
  phase. `Existing`/open errors return without cleanup. Only the file this
  call actually created can be removed after a write/sync failure; its handle
  is dropped before removal (including Windows handle semantics).
- `move_task` takes the canonical Mission hold after protected commit preflight
  and before current-lane observation, feedback/provenance decisions and cycle
  allocation. It holds through evidence create, status append, commit/rollback
  and retirement regardless of auto-commit. The writer re-enters this same
  authority; no private lock or collision retry was invented.
- Rollback-point capture is still auto-commit-only and uses that existing hold.
  No other ownership, durability, protected-refusal or flag policy is changed.

## Red first and coordinator verification

Strengthened the existing unit test: a losing create with different feedback
must leave the first artifact byte-identical. Added a bounded 12-thread launcher
of real CLI `--no-auto-commit` rejection processes, checking every successful
`evidence_ref`, every persisted cycle pointer and unchanged Git HEAD.

Both fail on the intended behavior before production edits:
`/tmp/sk-f1c-ownership-red.log`, `/tmp/sk-f1c-concurrent-red.log` — each **0
passed / 1 failed**, exit 101; the real-CLI failure is a successful call naming
a deleted artifact. Neither is a compile failure or manufactured verdict.

Green, via whole-invocation `/tmp/sk-focused-cargo.sh` lease and isolated
HOME/XDG/TMP: **234 status passed / 1 parent-exercised helper; 58 CLI status
passed; 9 CLI lib passed**, zero failures. Logs
`/tmp/sk-f1c-{status-green,cli-green}.log`. All prior named F1/F2–F6 tests
remain. No broader approval, source-baseline change, real-fork, workflow,
platform or performance claim.

The prior reports' original hashes/counts remain historical delivery inputs;
they are not current hashes for `tasks.rs`/`mutation_commit.rs`. Existing review
`/tmp/sk-rust-status-second-recheck.md` independently proves F1a/F1b and the
coordinator emit-error guard (two real OS-level faults) and F2–F6, but rejects
the tranche for this F1c defect. A fresh review of this correction is required.
Side-commit operator-index lag, lanes destinations and the stated unported
coordination/owned/done gates are not silently repaired or waived.
