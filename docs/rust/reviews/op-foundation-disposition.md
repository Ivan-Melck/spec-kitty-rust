# Op foundation independent review disposition

Checkpoint: `1ad605bb7744b8cae1d1a95b6bf449b7d1b92c9a`.
Independent report: `/tmp/sk-rust-op-foundation-review.md` (FINAL).

**NOT APPROVED for stage 1.** The 72-test run is not acceptance. Reviewer adds
22 bespoke byte vectors, 42 validation vectors, 17 writer scenarios, actual
Git-placement bidirectional lock contention, missing-oracle negatives and
real-checkout custody probes. Tested record/model/JSON behavior has partial
scoped acceptance; no whole command/executor/admission/CLI is approved.

Required corrections:

- F1 HIGH: start accepts a 9 MiB request but completion's 8 MiB precondition
  strands it. No new start-side refusal is permitted.
- F2 MEDIUM: non-object first rows become evidence-permitting Absent mode;
  they must error, while absent/unknown object fields stay distinct.
- F3 MEDIUM: custody waits for Git to exit before draining its pipe, so large
  worktree listings deadlock and silently miss records.
- F4/F5: writer-local read is not cross-checkout claim composition; correct the
  declaration. Consolidate reference resolution in Kernel with actual Python
  tolerance/acyclic-chain semantics, not an unchanged strict containment
  resolver or another copy labelled equivalent.
- F6/F7: correct unsupported assertion claims, historical hashes/test names
  and message-detail limitations. No broad approval of declared divergences.

Coordinator red `/tmp/sk-op-foundation-f1-f2-red.log`: **23 passed / 2 failed**
for real >8 MiB completion and malformed first-row mode. Small F1/F2 corrections
now pass, preserving start behavior and object-mode distinctions. Scoped
`/tmp/sk-op-f1-f2-git-coordinator.log`: **74 Op + 262 Git = 336 passed / 0
failed / 3 parent-exercised helpers**; scoped strict Clippy clean. This is not
independent approval. Runtime `tempfile` availability is coordinator-provided
for file-backed custody output; no registry dependency pin changed.

Subsequent implementation delivers F1–F7 remediation: 80 Op + 199 Kernel
scoped tests, file-backed large custody listings, shared reference-profile API
(no total-link cap) and corrected local-reader/report contracts. Coordinator
reproduces these within 2,073 workspace passes, strict Clippy/fmt and completed
release continuation; prior release tool timeout remains documented. Fresh
independent acceptance is still required; the original rejection is historical
input, not silently erased. See `crates/sk-invocation/REMEDIATION-REPORT.md`.

Stage-2 admission/catalog/routing/governance/glossary/executor/commit/lifecycle
and StepContract remain absent. Large-input, resolver and custody defects must
close before wiring; performance and inventory gates remain unchanged.
