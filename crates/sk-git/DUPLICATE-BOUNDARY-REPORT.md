# Duplicate reflog boundary correction — coordinator delivery

Input: `/tmp/sk-rust-git-boundary-recheck.md`, which accepts G1–G4 at published
`bfc79fdfd` but reports B1 (LOW duplicate-record ambiguity) and B2 (README
predicate wording). Provisional record `/tmp/sk-git-duplicate-boundary-plan.md`
predates implementation. This delivery is not independent acceptance.

## Correction

A newest-record byte match is not an ordinal: a hook can append an identical
reset record after creating a same-parent commit. The previous search selected
the duplicate and lost evidence of the intervening creation.

`PublicationMark` now captures complete history **only below the existing
256-entry bound**. The post-history must also be below the bound and contain
the entire previous snapshot as its exact suffix. Only the count difference
before that suffix is eligible evidence. A rewritten, shortened or saturated
history cannot be rebound. This is neither a count-only slice nor an oldest
matching-record search, either of which could admit old history.

Empty/unreadable handling, amendment exclusion, candidate-sensitive retention,
argv/hooks/signing/index and blob/capture bounds are unchanged. A saturated
fallback does not block normal commits: this child's own summary remains an
independent evidence channel. No new generic input refusal, rollback or ref
mutation is introduced.

## Red/green evidence

- Independent public-API assertion `/tmp/sk-duplicate-boundary-red.log`:
  **3 controls passed / 1 B1 failed**, exit 101; retained roots.
- Promoted regression, `/tmp/sk-git-duplicate-cargo-red.log`: same **3/1**.
- Corrected case now returns `Unconfirmed(candidate=created)` with pinned HEAD
  and restored marker, not a misleading ordinary refusal.
- Pure snapshot unit controls cover duplicate records both before/after capture,
  unchanged history, altered suffix, truncation and capped counts.
- Real-Git positive control creates >256 reflog entries and confirms a normal
  conclusion through Git's summary. Its first fixture used same-OID soft resets,
  which do **not** add reflog entries: the setup assertion failed in
  `/tmp/sk-git-duplicate-op-coordinator.log` (81 passes/1 failure before Cargo
  stopped). This was a test-authoring assumption, not a native regression. It
  now alternates two real tips and proves the history count before concluding.

Final coordinator command, through `/tmp/sk-focused-cargo.sh`, existing target,
whole-command lease, two jobs, isolated HOME/XDG/TMP:
`test -p sk-git -p sk-invocation --locked` → **334 passed / 0 failed / 3
parent-exercised helpers**, 25 suites: **262 Git + 72 unwired Op foundation**.
Log `/tmp/sk-git-duplicate-op-coordinator-verified.log`. No aggregate/release or
independent acceptance is inferred from this scoped count. README predicate and
bounded-snapshot documentation are updated with this correction.
