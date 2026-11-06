# Artifact ownership / Mission-lock review disposition

Checkpoint: `1ad605bb7744b8cae1d1a95b6bf449b7d1b92c9a`.
Independent report: `/tmp/sk-rust-artifact-ownership-recheck.md` (FINAL).

**Scoped APPROVAL of F1c**, not an approval of the status tranche, other
unported gates, Op execution, platforms, workflows, release or parity.

The reviewer independently verifies:

- Failed exclusive create owns no path: Existing/symlink/EACCES preserve prior
  bytes; five 12-writer library races preserve the one winner; an owned partial
  write alone is cleaned up, with its handle dropped first.
- The canonical Mission hold precedes lane observation and cycle allocation,
  even without auto-commit, and covers create/emit/commit/retirement. The writer
  re-enters that same authority; rollback uses the same hold. Protected commit
  preflight is byte-identical and remains before status reads.
- Three no-auto rejection races, one auto rejection race and both approval
  modes each have exactly one successful transition, one artifact and live
  evidence/review references; auto commits contain exactly the expected paths.
- Existing artifacts, no-ops, invalid/forced edges, F2–F6 and post-emit/rollback
  retention remain correct. Side-commit operator-index lag is observed and
  unrepaired, not hidden by the approval.
- 234 status / 58 CLI status / 9 CLI lib / 26 compat, strict scoped Clippy/fmt
  and build reproduce. Source hashes: mutation_commit `861f8cdb…`, tasks
  `3986fb07…`, status_mutation `87122387…`. Old 57-test/hash tables are historical.

The serialized CLI race no longer reaches Existing; that cleanup branch is
proven separately at the library boundary, not falsely claimed by the CLI
race. Mission-wide serialization can make invalid calls wait; no speed claim.
Probe setup errors and corrected review-local fault instrumentation remain in
the report and `/tmp/f1c-review/`, not erased or counted as product failures.

Remaining gaps include lanes destinations, coordination/owned/done gates,
side-commit index-state interpretation and native payload-key differences.
No inventory/whole-command/performance gate is promoted by this disposition.
