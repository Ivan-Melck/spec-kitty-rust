# Kernel continuation review — coordinator disposition

2026-11-02. Review basis: checkpoint `ab3e0fcc8`, not concurrent uncommitted
Mission-lock/safe-commit work. This is remediation evidence, not shipping approval.

## K1: composed Git/lock selection

Reproduced: raw kernel discovery honors inherited Git selectors while the bounded
Git adapter opens an explicit checkout and strips them. A canonical writer could
therefore lock in one repository while another adapter mutated the original.

**Safety fence applied:** status-family authority and generic-journal authority
now refuse inherited GIT_DIR, GIT_COMMON_DIR, GIT_OBJECT_DIRECTORY, GIT_WORK_TREE,
GIT_CEILING_DIRECTORIES and GIT_DISCOVERY_ACROSS_FILESYSTEM. Raw read-only topology
probes retain Git/Python parity. Explicit low-level lock paths are unaffected.

This closes the silent split by refusing, not by implementing environment-aware
mutations. Complete support still needs one captured repository context shared
by file placement, Git operations and lock selection. It is an explicit supported-
input restriction; do not call it full override parity.

`sk-cli/tests/repository_authority.rs` exercises the composed Git/workspace/status/
generic-journal seam in fresh child environments: the default authorities agree;
each selector refuses actual lock acquisition/append before creating lock dirs or
writing a log. The child helper is ignored in isolation and executed by its parent.
Existing lock-authority tests preserve read-only environment parity while proving
writer refusal.

## K2: non-vacuous oracle evidence

The five kernel differential selectors now share `tests/support/oracle.rs`.
The default is the repository's `.venv-rust-oracle`, with the platform's native
executable layout. Absence or dependency drift panics, rather than returning None
and reporting a zero-comparison pass. SK_KERNEL_PYTHON remains a supported selector,
not an exemption from validation.

`scripts/rust/verify_oracle.py` verifies Python 3.11, all installed versions against
uv.lock, duplicate-version shadowing, the root distribution and complete active
runtime requirements (including transitive dependencies/extras and constraints).
An empty or partial environment cannot pass merely because its overlap is exact.
Five unit tests cover genuine green, missing/empty/transitive closure, wrong or
unlocked/duplicate versions and unsatisfied requirements. A Rust subprocess test
proves a missing interpreter fails the actual shared selector. The preinstalled
uv-tool interpreter was also checked and correctly rejected for drift.

CI now explicitly provisions and verifies the frozen oracle before native tests.
No new remote CI result is claimed. Historical fixtures/evidence retain their
original provenance; production binaries do not import or execute this tooling.

## K3: canonical status-family identity

StatusLock and checkout-claim ordering now use FileLock's canonical PathBuf key,
not lossy caller spelling. A Unix symlink-alias nesting regression proves the
original holder record survives the inner drop, mutual exclusion remains held,
and final release clears the holder. Authority no longer depends on UTF-8 display
strings; lossy rendering remains only in diagnostics.

## K4–K8: scope and disclosures

- Without inherited selectors, baseline status-family corrupt-topology degrade
  remains distinct from generic-journal strict refusal. Both are now documented.
- Committed fork evidence is PID/TLS simulation only. An earlier /tmp experiment
  is not reproducible regression evidence; the stronger documentation was removed.
- Windows/macOS CI, Windows read-only-relax crash windows, mandatory-lock details,
  owner/ACL/xattr/hard-link preservation and exotic topology remain open.
- No-follow helpers without callers are library coverage, not completed workflows.

## Checks run for these fixes

- Seven targeted kernel suites: 35 passing tests, using the verified oracle.
- Oracle negative subprocess precondition: parent passes, child fails as required.
- Composed CLI authority test: parent passes all seven environments; its subprocess
  helper is ignored only when selected without its parent.
- Full kernel rerun: 189 passed, one negative subprocess helper ignored in
  isolation; strict kernel Clippy passed. The broader workspace is still in flight.
- Oracle/completion-checker unit tests: 39 passed (five new verifier tests);
  pinned verifier succeeds and the old
  tool environment fails; missing-oracle cargo comparison exits 101 as required.

Whole-workspace/release/platform/performance gates must be repeated after the
other active owners finish. The full rewrite remains incomplete.
