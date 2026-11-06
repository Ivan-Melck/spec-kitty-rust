# Git conclusion-boundary review disposition

Published checkpoint: `bfc79fdfd413e69059b6a1475d31e70317f8b6b8`.
Independent report: `/tmp/sk-rust-git-boundary-recheck.md` (FINAL).

**Scoped PASS:** G1–G4 discharged. Reviewer independently reproduced 259 tests,
0 failures, 3 parent-exercised helpers, 19 suites including four doctests;
forced fresh strict scoped Clippy and format checks pass. Public-API probes
confirm missing/selectively rewritten/out-of-window boundaries cannot bind a
foreign historical commit; hook amendments are not the original candidate;
original argv/hooks/signing/index/bounds/corpus remain intact. Historical
uncommitted-d644/published-f42 evidence identity is accurately preserved.

The previous workspace attribution condition is resolved **for this scope**.
The unconfirmed arm still retains state; no functional workspace change was
needed. This is not approval of coordination CLI wiring, Op invocation,
platforms, full parity, workflows or performance.

## Residual at bfc79fdfd; correction delivered, not yet re-reviewed

**B1 (LOW): duplicate boundary bytes.** The newest matching `%H\0%gs` record
can be a post-capture duplicate, hiding an intervening created commit. A hook
creates a same-parent commit, resets to the pinned HEAD with the same reset
subject, restores the marker, then fails: candidate is lost and the result is
ordinary `CommandFailed`. No false `Ok` or observed data deletion, but this is
not complete evidence attribution. **B2:** README should include the
candidate-is-absent condition when describing ordinary failure.

Coordinator independently reproduced B1 with an asserting public-API test:
`/tmp/sk-duplicate-boundary-red.log`, **3 controls passed / 1 B1 failed**, exit
101. Source `/tmp/sk-git-duplicate-boundary-regression.rs`; fixture roots retained
under `/tmp/sk-duplicate-boundary-red.Oroq8P`. Existing production dependencies
remain frozen while peer status/prompt reviews finish. Follow-up design is in
`/tmp/sk-git-duplicate-boundary-plan.md`: bounded pre-snapshot/count plus suffix
validation, never blindly choose the oldest matching bytes or count-only slices
that could re-admit history. That bounded snapshot/suffix correction is now
implemented with a pure ambiguity control and a real-Git saturated-history
positive. Scoped Git is 262/0/3; see
`crates/sk-git/DUPLICATE-BOUNDARY-REPORT.md`. Subsequent independent report `/tmp/sk-rust-git-snapshot-recheck.md`
**accepts B1/B2 at `1ad605bb7`**, with 262/0/3, fresh Clippy/fmt and real duplicate,
rewritten/count-growth/cap/amend controls. The low S1 limit remains: fallback is
disabled at the 256-entry cap, so a nested-create/restore shape on a saturated
history can still have no identified candidate. Ordinary long-history commits
confirm through their summary; no unconditional “no object existed” claim.
The module predicate text is now corrected; cost remains unmeasured. This is
not full attribution, CLI/Op/coordination wiring or performance acceptance.
