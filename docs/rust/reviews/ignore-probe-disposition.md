# Tracked-aware ignore probe review disposition

Checkpoint: `38834239dcccf6171f7f56c0d62036117d11cb4c`.
Independent report: `/tmp/sk-rust-ignore-probe-review.md` (FINAL).

**Scoped PASS**, Linux/git 2.43, for the literal, read-only Git query; not Op
admission/commit policy, CLI, platforms, workflows or performance acceptance.

Reviewer reproduces 276/0/4 parent-exercised helpers, 20 suites/five doctests,
strict scoped Clippy/fmt. Independent 46 byte cases match real Git; Python
matches 42/44 UTF-8 cases, with two literal-operand normalization caveats; two
non-UTF-8 cases are Git-only controls. Tracked-but-ignored/custom-index controls
flip under ordinary check-ignore and remain true with --no-index. Signal,
timeout, missing executable, non-1 exit and capture overflow stay typed errors,
never quiet false. Dirty worktree/HEAD/index/staging/environment are unchanged.

Coordinator closes the review's small follow-ups:

- Adds the already-proven allowed-exit-1 stdout overflow and signal cases to
  the suite. They pass immediately: coverage additions, not claimed red-first
  bug fixes. Git now **278/0/4**, strict Clippy/fmt pass in
  `/tmp/sk-ignore-review-followup-*.log`; no aggregate while doctrine owner edits.
- Updates current source/test hashes in `IGNORE-PROBE-REPORT.md`, retaining
  original delivery/format-failure history. Method body is unchanged.
- Documents literal operand semantics: Python PurePath maps empty to dot and
  strips a trailing separator; native deliberately forwards raw GitPath bytes.
  Such spellings are not Op-record paths. No broader operand parity is claimed.

This review does not promote any command or change the completion/performance
verdict. The original 276-pass/2073-workspace checkpoint remains historical;
278 is a later scoped count, not a freshly certified workspace aggregate.
