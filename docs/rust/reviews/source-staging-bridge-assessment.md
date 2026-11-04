# Source staging/recovery bridge — design not authorized

Read-only proposal `/tmp/sk-source-staging-bridge-design.md`, preserved provisional
copy, coordinator assessment `/tmp/sk-stage-bridge-coordinator-assessment.md`.
No production staging bridge was authorized from that proposal.

Useful observations: R2/R3 mutate Source requested staging before encoding/NUL
exceptions; R4 stages/fails/restores before encoding; Git cannot add against an
index whose own lock is already held; hooks see a partial index while the real
operator lock body also includes unrelated staging. Existing prospective-body
work may avoid duplicate filter invocations, but observer-inclusive eight-call
counts do not prove a full stateful-filter equivalence matrix.

Rejected as an implementation contract:

- Whole-index pre-image restoration is not Source's requested-path binary-patch
  replay. It can erase other changes, differs if HEAD moves, and bypasses Source
  recovery rejection/error classes. Held Git locks do not prove absence of
  valid hook/out-of-band writes. Recovery needs real ownership and Source phase
  evidence, not a blind finally/Drop rollback.
- Unix-only or symlink-index Unsupported refusals do not close valid Source
  inputs. The separately implemented canonical endpoint fix addresses the newly
  measured symlink lock mismatch instead of refusing it.
- Native64KiB bounds are not Source E2BIG; do not classify unprobed payload sizes
  as argv failures. One Source git-commit process vs many Native subprocesses
  requires explicit phase evidence: a Native hook spawn failure is not
  automatically a never-started Source outer commit.
- A plan does not own its repository handle. Proposed late-message APIs must
  explicitly borrow the existing captured handle, not reopen/recapture it or
  add dead/unproduced readiness variants or extra Source refusal text.

Evidence qualifications retained, not repaired retroactively: the design's
native-build3 log runs raw Cargo outside the focused whole-command lease and
repository target, pipes tail without pipefail, and does not bind the pinned
repository toolchain. Its Native fixture constructor deletes reused PID/branch
fixture roots, so earlier fixture files are lost (remaining logs preserved).
The matching Source-matrix2 ran after Native-matrix4; earlier Source probes cover
related cases, not that pairing's Source-first chronology. Its22-row index has
no standalone exact53 receipt. These are not accepted Cargo/oracle/chronology
claims; later conforming lock-endpoint implementation gates are separate.

Next bridge must implement actual operator staging before encoding and faithful
requested-only patch recovery using the corrected canonical lock endpoint;
preserve unrelated staging, authored/audit/index/ref/candidate evidence and
actual caught/uncaught Source outcomes. Source-valid gaps remain required.
