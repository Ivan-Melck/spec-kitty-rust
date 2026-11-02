---
type: reference
updated: 2026-11-02
audience: software-engineer
---
# Transactional kernel review checkpoint

Independent review found normal local-repository interop: Python/Rust lock paths agreed, cross-language holders contended in both directions, and mixed writers preserved complete ordered lines. This is not full topology/family or service-handover approval.

## Coordinator fixes after review

- Guards are statically `!Send`/`!Sync` and `must_use`; same-thread reentrancy cannot be invalidated by moving a guard.
- Per-process ownership checks reject inherited TLS reentrancy after fork; inherited guards do not truncate/unlock their parent's holder. A real fork probe now reports child refusal; a safe unit test pins stale-process rejection and non-truncation.
- Unimplemented `GIT_DIR` / `GIT_COMMON_DIR` / `GIT_OBJECT_DIRECTORY` handling refuses rather than choosing a different lock from Git/Python. This fence is pending full environment-aware topology support, not full parity.
- Claimed Git markers validate HEAD/objects/refs; a bogus nested HEAD-only marker refuses rather than silently winning. A marker with no HEAD entry is ignored as Git ignores it. Symlinked Git directories are followed for directory detection.
- `atomic_write` reports `AtomicCommitIndeterminate` after a successful rename followed by failed directory fsync. `KernelError::mutation_may_have_committed()` exposes this explicitly; a fault-injection test confirms the new data is visible. Consumers must recover/read back, not assume error means no mutation.

## Remaining kernel completion work

Explicit blocking/nonblocking/unbounded and reentrant/non-reentrant policies per family; name-keyed status/project locks and checkout-claim SHA1/prefix/suffix rules; canonical Git environment/ceiling/bare/worktree/submodule discovery and cross-language differential tests; Windows mandatory-lock diagnostics; broader clock/parser/no-follow/home/path contracts; append precondition size policy, exact short-write/repair behavior and new-log directory durability. Intentional permission-preserving/no-op behavior differs from Python kernel.atomic and needs consumer guidance. See the kernel README for remaining platform and metadata limits.

**Do not route status.events.jsonl or canonical-events.jsonl through the generic O_APPEND event-log helper.** They require their own exact status-lock authority, atomic whole-file batch replacement, verification and recovery protocol (or the pinned external writer service). The generic helper is for the baseline's interim run/decision/retrospective log families.

The initializer's LockTimeout mapping must also be corrected: map contention to its advertised LockUnavailable, not generic I/O. Installer/run-store callers must handle indeterminate commit outcomes. Full completion remains blocked until these contracts and their required callers are verified.
