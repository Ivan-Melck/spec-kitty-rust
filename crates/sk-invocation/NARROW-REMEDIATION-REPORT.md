# Op reference/capture narrow follow-up — implementation evidence

Baseline `38834239dcccf6171f7f56c0d62036117d11cb4c`; input independent
`/tmp/sk-rust-op-remediation-review.md` (403 lines). Provisional plan written
before changes: `/tmp/sk-op-narrow-remediation.md`.

The review independently accepts F1/F2/F3, shared Kernel authority without
widened containment, local-reader contract, F6 assertions and F7 floats. It
rejects narrower N1/N2 fidelity guarantees and notes N3/N4. Coordinator fixes
below are **not fresh independent acceptance**, whole invocation or CLI approval.

## Reproduced defects and changes

- **N1:** live lock-exact oracle test red **1 pass / 1 failure**:
  `self/..` stays raw natively but Python resolves it. Added loop/parent vectors,
  surviving-loop controls, and `self/../dir-link/leaf.md`: the last distinguishes
  CPython's stopping expansion from incorrectly continuing filesystem walking.
  On detecting an in-progress link, the reference helper now carries the entire
  remainder lexically (ignoring internal memo markers), normalizes `..`, then
  applies the final stat. Only loop-shaped final errors refuse; other errors
  remain tolerated. Later symlinks in the lexical tail are not expanded. Existing
  41/42/200-link, missing/dangling/EACCES, finite-reuse, URL/NUL and bare-loop
  controls remain. This does not change containment, lock paths or their budget.
- **N2:** real child test red **0/1**, not a compiler/setup failure. Its isolated
  TMPDIR is mode0500, the child cannot write it, 800 registered worktrees produce
  >64KiB of listing, and the record in the final worktree is missed. The fixture
  and initial log survive under `/tmp/sk-focused-cargo.lRzcbh/tmp/` (path in red
  log). Capture now drains a pipe on a reader thread, not via a temp file. The
  same 30s budget covers child completion and receipt of stdout; no output cap,
  process-group signal, environment mutation or wait-before-drain is introduced.
  Direct child is killed/reaped on timeout/error. On timeout the reader handle
  is dropped rather than joined indefinitely; a descendant retaining the pipe
  can keep that reader alive until EOF. No process-tree termination guarantee.
  Native readonly-TMP test now passes; it explicitly diagnoses permission-bypass
  privileges instead of pretending that case is discriminating. Linux only.
- **N3:** implementation §3.1 now explicitly binds the published 388 snapshot
  (`c3b39d76…`, not unpublished `bd7488fe…`) and includes writer-filesystem tests.
  Deletion is identified as committed. Original implementation-owner scope is
  separated from coordinator manifest changes. At 388 tempfile was promoted
  dev→runtime; with pipe capture it is now dev-only again. Cargo.lock unchanged.
- **N4:** integer -0 repr red **0/1**, corrected to CPython 0; floating -0.0 keeps
  its sign. Huge arbitrary-precision integers stay exact, with a positive unit
  control. Prior report's f64-coercion residual was wrong and is corrected.

Red logs: `/tmp/sk-op-{loop-parent,readonly-tmp,negative-zero}-red.log`.
No interruption, ignored oracle, fabricated red or production refusal is used.

## Scoped verification

Through `bash /tmp/sk-focused-cargo.sh`, existing target and whole-command lease:

- `test -p sk-invocation -p sk-kernel --locked --no-fail-fast`: **281/0/1**,
  29 suites including doctests; Op82 + Kernel199. The ignored Kernel helper is
  parent-exercised. No aggregate while doctrine owner edits.
- Strict scoped all-target Clippy and scoped fmt check: clean.
- `/tmp/sk-op-narrow-{green,clippy,fmt,fmt-check}.log` retained.
- Frozen containment/locks/fs_resolve/event-log/paths/discovery/topology,
  Cargo.lock and Python src equal 388 (`git diff --exit-code`); diff check clean.

These are scoped implementation checks, not a replacement for independent
review, workspace/release gates, workflow coverage or performance measurements.

## Current changed-file bindings

```text
3344c29f378151e9891213bae547eabc6624e72061207dc22f0abe369b644310  crates/sk-kernel/src/reference_path.rs
331bad9251f098964fce18270c4c24c6f2ed6c8249a165ca8da187431e051cf4  crates/sk-invocation/src/op_custody.rs
74263ecd0a48f284d3f1c03cdaf36cb8db46e15de7bd164e677358730a48bc38  crates/sk-invocation/src/python_json.rs
954b2e43807108f13a54455c8b6fa7625914b27f613772e6b8620d66b5db14a0  crates/sk-invocation/tests/reference_path_oracle.rs
8cc5e9ee503f42c781cd8a5720636f88aa32b0aa53849ec1ccea499f2c71e66b  crates/sk-invocation/tests/op_custody_roots.rs
d65a460707e6de3d1da6b9a59e63714f4ea3fb9c9b13636617b27c020c21ca63  crates/sk-invocation/Cargo.toml
```

Remaining typed-model/Pydantic message, printability, root-error, cross-checkout
claim-composition, executor/admission/commit-policy/lifecycle/StepContract and
platform boundaries remain as disclosed. No CLI/capability/accounting promotion.
