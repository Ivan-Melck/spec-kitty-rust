# Mission-lock review disposition

Reference: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Independent review of frozen `ff4452b02` sources:
`/tmp/sk-rust-mission-lock-review.md`. Verdict: lock mechanics accepted with
required fixes; the three authoring integrations accepted. This disposition
records coordinator corrections, not a fresh independent approval.

## Findings addressed

- **F1:** exhausted legacy coordination identities use the recorded
  `mission_slug`, not a renamed directory. Absent-slug fallback remains the
  directory. A regression reproduces both cases, and real Python/native holders
  now exclude one another in both directions on `042-legacy` while its directory
  is `042-legacy-moved`.
- **F2:** metadata exceeding the existing 1 MiB ceiling yields typed
  `KernelError::TooLarge`. It cannot silently become missing metadata or fall
  back to another key, even when unresolved-key fallback was requested. This is
  a declared bounded-input difference from the reference, which reads unbounded
  metadata; it is not full oversized-input parity.
- **F4:** the cross-language fixture now declares coordination, with a key
  different from its directory. Tests require a verified oracle rather than
  returning success when it is missing. Valid Crockford fixtures are used.
  Three independent authoring probes are now permanent regressions in
  `sk-mission/tests/coordination_write_lock.rs`: actual coordination-key
  contention, a negative bare-key control, dependency-finalization preservation,
  and birth/post-birth convergence.

## Verification

Temporary project/HOME/XDG/TMP roots, existing Cargo target,
`CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`:

- Status Mission-write suite: **17 passed**, including mixed-writer tests.
- Status lock-family suite: **9 passed**.
- New coordination authoring suite: **3 passed**.
- Existing authoring lock suite: **3 passed**.
- Rustfmt and strict targeted Clippy passed on changed sources/tests.

Logs: `/tmp/sk-mission-lock-disposition-status.log` and
`/tmp/sk-mission-lock-disposition-authoring.log`, and
`/tmp/sk-mission-lock-disposition-clippy.log`.
No aggregate, release, platform or performance approval is inferred while other
implementation owners continue editing.

## Not waived

- Verified rollback capture/truncation under registered holds is a separate
  protocol, not supplied merely by taking a lock. Status writers must preserve
  foreign/committed rows and handle indeterminate writes explicitly.
- Rekey extension still needs actual migration-consumer evidence, including
  re-reading observations after acquiring a moved-key hold.
- Flat emit's default wait policy differs from the reference; its owner must
  distinguish unbounded flat writes from bounded coordination transactions.
- Inherited Git-selector mutation support and non-ASCII legacy-prefix semantics
  remain declared compatibility gaps. The review did not certify non-Linux
  fork/PID or holder-sidecar durability behavior.
- Other in-flight status and workspace work was not part of the independent
  lock review and does not inherit its acceptance.
