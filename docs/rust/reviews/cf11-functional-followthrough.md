# CF11 handoff and full-init phase correction (unpublished)

CF11 delivered seeded-list promotion hints through canonical effective sets and
exact spelling, corrected dry-run wording/warning order and caught-failure result
carriers, and added the provenance leak authority. Actual report:
`/tmp/sk-cf11-charter-migration-report.md`; bounded parent report:
`/tmp/sk-cf11-parent-integration-plan.md`.

## A real coordinator regression, not a foreign-test waiver

CF11's scoped Install run exposed two `full_init_baseline` reds. The coordinator's
CF6 change correctly bound templates to the actual project but incorrectly
consulted that binding before full initialization published configuration.
Source `init.py` installs final command skills **after** `save_agent_config`.
The selector's missing-config=false rule therefore does not imply fresh full
initialization should render inactive.

Full-delivery rendering now sees actual published config. The separate Native
no-assets API retains early validation and is explicitly labelled as such. No
hardcoded active override, golden regeneration, or relaxed byte/retry assertion.
A new full-delivery test checks conditional output and byte-identical manifest
retry. Other init ordering/failure boundaries are not globally approved by this fix.

## Actual bounded evidence

Index: `/tmp/sk-vertical-integration.TwDlpw/command-log-index.tsv`.

- `.8kOxIP`: original report/engines/shared/init files preserved in
  `/tmp/sk-cf11-parent-original.iU97g6`; command subsequently exited2 on guessed
  evidence globs. `.CZCbMN` located actual failures, then exited2 on guessed
  Source globs. Neither attempt was hidden or reconstructed.
- `.XRY1Ok`: parent PRE-edit standard-eight schema/structural/admissibility valid.
- `.MjtEyX`: owned formatting pass.
- `.YjFuHU`: **17 pass** = CF11-5 + parent rendering5 + unchanged full-init6 +
  A5-1. Both previously failing full-init checks pass with recorded hashes and
  no-op retry assertions unchanged. Native/recorded tests, not new isolated pairs.
- `.xH5AlE`: actual delivered Source-recorded fixtures/foreign test preimages and
  current bindings retained; tracked Source/packs diff clean. The producer changed
  one existing test branch from old Err to the fixture's failed-result payload;
  parent inspected that stronger payload/unchanged-tree assertion. No producer
  occurrence map found; valid parent map is not retrospective producer approval.

## Remaining functionality, not merely proof debt

Six charter migrations remain unregistered. Four preview doors and actual
`MigrationStateUnreadable` mapping still need implementation. Main and worktree
error catch/ledger phases differ; a universal record-all-errors arm is wrong.
Other documented promotion warnings, mission-token/provider resolution, model
coercions and cutover effects remain qualified. Four rescue callbacks likewise
remain withheld under their separate known gaps. Central **126/136** unchanged.

CF16 is a new bounded implementation for real preview dispatch and typed failure
execution, explicitly allocating corresponding shared upgrade/error/CLI functions
and the completed charter-migration subtree. Static registration/catalogue/root
capabilities stay coordinator-owned. CF12–CF15 remain disjoint active producers.
No polling/restarting completed agents, aggregate/release, commit/push, global
accounting, independent fidelity acceptance or measured performance promotion.
