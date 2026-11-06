---
type: reference
updated: 2026-11-02
audience: software-engineer
---
# Full rewrite completion loop

The operator has explicitly requested autonomous continuation until the **full rewrite** and a **substantial measured speed increase** satisfy the original expectations. The foundation milestone is not the finish line. Do not stop merely because a tranche is green, a draft PR exists, or a read-only subset benchmarks well.

## Fixed reference

Upstream `spec-kitty/spec-kitty` at `9aed3a8012cb5eb22c0128b14d525039f95fea7a` is the reference snapshot. Existing Python sources and shared-package fixtures are an oracle for development, not a production fallback. Public delivery remains `Ivan-Melck/spec-kitty-rust`, branch `rust/rewrite`, draft PR #1. Preserve upstream notices and user customizations. Do not merge or publish a release automatically.

## Completion gates (all required)

1. Inventory **every reachable upstream command/subcommand, option, machine contract, and lifecycle mutation**, not just the initial eleven commands or a hand-picked module list. Classify supported behavior with source/test references; absent behavior stays pending. Do not silently exclude hard subsystems.
2. Every required inventory entry has a working native Rust implementation, observable acceptance tests, and applicable differential/conformance evidence. No production Python process, FFI Python interpreter, success stub, or renamed unsupported surface counts as implementation.
3. Exercise complete create/specify/plan/tasks/implement/review/accept/consolidate/merge/recovery flows, including human decision authorization, governance, persistent state, event integrity and worktree ownership. Test refusal, interruption, concurrency, and user-content preservation as well as happy paths.
4. Native consumers preserve external events/tracker/service boundaries and pinned wire contracts. Mutating legacy formats requires the **same** validated contract, identity, lock authority and recovery semantics; do not invent an unannounced replacement protocol or a second uncoordinated writer. The foundation's read-only fence is lifted only for implementations that satisfy these conditions and their tests.
5. Aggregate native tests, strict Clippy, formatting, release builds, packaging and the applicable existing conformance corpora pass. Required cross-platform CI and independent review findings are resolved; a draft cannot declare itself fully compatible from Linux unit tests alone.
6. **Performance target:** at least **3x geometric-mean latency improvement** versus the pinned Python reference across representative equivalent workloads, with no unexplained correctness regression or statistically material >10% slowdown in a required workload. Measure startup separately, but do not let help/version or constant-output commands substitute for real work. Include representative state mutations and complete workflows when implemented. Record fixtures, result equivalence, toolchain/package identities, interleaved repetitions, machine/load context and raw samples. A partial benchmark is not a pass.
7. Publish the actual source, build/install instructions, migration guidance and reproducible evidence in the public fork. All unsupported required entries must be zero before status becomes complete. If a real external blocker prevents progress, record concrete evidence and request only the decision/credential that is actually needed; do not call a blocker completion.

The 3x target is an acceptance target, not a claimed result. Baseline and measurement validity come before optimization.

## Control loop

1. Run the completion/inventory check; identify unsatisfied requirements and ready dependency slices.
2. Dispatch non-overlapping implementation tasks in parallel using **`tokenfactory/deepseek-v4.1-flash-1017`**, preserving clear crate/file ownership and typed shared contracts.
3. On completion notifications, inspect results and code. Integrate real behavior into the native CLI; add/execute acceptance and differential tests.
4. Reproduce and fix findings. Use separate independent review agents for new aggregate surfaces; fold small findings directly. Never poll live delegates or substitute another model.
5. Run bounded aggregate checks, update the machine-readable inventory and evidence, commit/push verified changes, and refresh draft PR #1 accurately.
6. Benchmark equivalent completed behavior, investigate bottlenecks, optimize with regression tests, and remeasure. Keep correctness evidence attached to speed claims.
7. **If any completion gate remains unsatisfied, return to step 1 without treating the intermediate publication or agent-turn boundary as a final delivery.** Async subagent completion resumes this coordinator; do not create a shell polling loop or an unrequested fleet of top-level threads.

## Current execution priority (operator-confirmed 2026-11-05)

Run a continuous dependency-aware implementation loop using parallel
`tokenfactory/deepseek-v4.1-flash-1017` agents for disjoint gruntwork. Prioritize
real functional coverage in idiomatic Rust, with scoped compile and functional
smoke checks while implementing. Perform deep differential/boundary/platform/
workflow/performance campaigns after coverage is broader; do not let exhaustive
primitive probing monopolize implementation. Read actual pinned Source first;
reuse canonical authorities and preserve authored data/error phases. Deferred
verification is explicit debt, not equivalence, readiness or performance approval.

The coordinator integrates completed lanes and fills freed slots with new
independent implementation slices. No polling of running agents, blanket shared
formatting, success stubs, production Python, invented evidence or automatic
mainline merge. All completion gates above still apply to final acceptance.

## Current tranche

Foundation: eight crates, 455 tests at the last verified commit; not complete. Next work: exhaustive inventory and completion check, reproducible Python/Rust benchmark harness, shared transactional kernel primitives, canonical status replay, persistent runtime/decision execution, governance execution, then initialization/install, Git/lane ownership, lifecycle gates and the remaining command surface. Task completion reports must enumerate remaining gaps rather than quietly reducing this scope.
