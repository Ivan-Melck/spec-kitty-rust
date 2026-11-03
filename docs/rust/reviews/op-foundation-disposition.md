# Op foundation independent review disposition

Checkpoint: `1ad605bb7744b8cae1d1a95b6bf449b7d1b92c9a`.
Independent report: `/tmp/sk-rust-op-foundation-review.md` (FINAL).

**Original 1ad review: NOT APPROVED for stage 1.** The 72-test run is not acceptance. Reviewer adds
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

## Independent 388 re-review and narrow follow-up

`/tmp/sk-rust-op-remediation-review.md` independently reproduces279/0/1,
strict scoped Clippy/fmt, exact oracle53/53 and missing-oracle failure; 9MiB
cross-closes are byte-identical. It accepts F1/F2/F3, authority consolidation
without widened containment, local-reader contracts, F6 assertions and F7
float fidelity. This is genuine scoped acceptance, not a whole stage approval.

It rejects N1 loop-plus-parent normalization fidelity and N2 writable-TMP
capture dependence, and notes N3 stale bindings/manifest wording and N4 integer
-0 repr. Coordinator reproduces all three semantic cases red-first and fixes
them directly: lexical loop tail plus final stat, concurrently drained pipe
without a temp-file dependency, integer -0 canonicalization. Reports now
separate historical388 bindings from current sources; tempfile is dev-only
again with Cargo.lock unchanged. Scoped **281/0/1**, strict Clippy/fmt pass;
`crates/sk-invocation/NARROW-REMEDIATION-REPORT.md` holds exact current bindings.
These corrections are not new independent acceptance, nor a workspace total.

Stage-2 admission/catalog/routing/governance/glossary/executor/commit/lifecycle
and StepContract remain absent. New fixes require inclusion in the next
independent integration review before any whole-stage approval; no CLI wiring,
performance or inventory promotion follows from this bounded remediation.
