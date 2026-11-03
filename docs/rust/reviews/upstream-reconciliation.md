# Upstream reconciliation: current fork, older rewrite baseline

Fork `main` and upstream `main` both resolve to
`4a5264c4081fe0be604319d371c902ecc22231e1` (fork fast-forwarded and remotely
rechecked on 2026-11-03). The single additional commit after `7c50fd38b`
changes neither production source nor pins; the total gap is now 642 commits. The rewrite and
active development oracle source still use
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`. Synchronizing fork `main` did not
silently rebase or certify the rewrite.

Read-only investigation: `/tmp/sk-rust-upstream-reconciliation-plan.md`.
That investigation covers 556 linear commits through `8ffeb0180`, with 3,230
changed files but only 103 source files. The refreshed tip adds 85 commits
(**641 total** from the behavior reference). `uv.lock`, both pack source pins and the read-service pin are
byte-identical (coordinator reproduced the empty diff). Existing locked Python
runtime dependencies remain usable; source/provenance/fixtures still need
rebinding and a fresh oracle verification. No dependency or pack repin is
justified by this range.

## Required semantic adoption, before regenerating expected results

1. **Kernel path authority:** `platformdirs`-compatible app-specific cache,
   state and config roots, including startup legacy-location cleanup. Linux
   state changes to `~/.local/state/spec-kitty`; caller suffixes and ledger-lock
   contention tests must move together. macOS/Windows behavior also changes.
2. **Status:** migrated legacy lifecycle rows use `lamport_clock: null`, not
   zero. Forced `approved -> in_progress` needs the review feedback reference.
   The upstream FSM corpus changes two expectations; do not merely rewrite
   native goldens to conceal the missing rule.
3. **Governance identity:** canonical step-contract URNs must come from the
   identity authority, including deny sets; add/remove org-pack registration
   and context/activation changes remain required capabilities.
4. **Mission/Git:** own-mid8 human-slug selection, branch-independent managed
   lane classification/adoption and protected target override rules change.
5. **Other contracts:** orchestrator contract 1.22.0, service `/api/v1` prefixes,
   PRIMARY-routed decisions ledger, stated-empty init agents and default doctor
   behavior. These remain obligations even where the native subsystem is absent.
6. **Migrations:** count/order remain 136; one migration source/content hash and
   centralized-runtime detection change. Regenerate the catalog and full ledger
   oracle after implementing the corresponding native behavior.

## Refreshed upstream tail: `8ffeb0180..7c50fd38b`

Coordinator inspected the additional 85 commits without changing checkout
sources: 318 files, +6,792/-55 lines, mostly Mission/Op records and CI/docs.
The production-source delta is confined to task move pre-review gates,
`review/pre_review_gate.py`, `review/test_timeout.py` and `status/wp_metadata.py`.
Review-budget scope/shape and diagnostics, timeout handling and WP validation
need semantic adoption; do not assume the first investigation covers this tail.
`uv.lock`, `pyproject.toml`, pack pins, service pins, Cargo lock/toolchain have
no tail changes. No dependency/pack repin is justified by the refreshed range.
The source baseline and oracle remain `9aed3a80` until actual reconciliation.

## Safe integration sequence

- Finish or explicitly freeze active owners; commit verified integrated work.
  Do not merge/rebase beneath dirty shared-tree implementations or reviewers
  using the live older Python source.
- Merge current upstream on `rust/rewrite`, preserving its existing history and
  native sources. The investigation predicts no textual conflicts; that is not
  a semantic acceptance result.
- Adopt the path/lock authority and status/governance changes first. Reverify
  the unchanged dependency closure and packs; never use the installed uv-tool
  interpreter as a substitute for the lock-exact oracle.
- Refresh source baseline constants, dynamic CLI discovery/inventory, migration
  catalog, and oracle fixtures. Static discovery predicts two additional CLI
  leaves; regenerate absolute option/group counts rather than inventing them.
- Run scoped differential checks, then coordinator aggregate/Clippy/fmt/release
  gates on stable sources. Keep unresolved review findings and unsupported
  entries open.
- Bind any future benchmark campaign to the new source, binary, harness and
  reference identities. Old timings cannot certify the reconciled branch.

No merge into the rewrite, new-baseline certification or speed acceptance is
claimed by this plan. The public PR remains draft.
