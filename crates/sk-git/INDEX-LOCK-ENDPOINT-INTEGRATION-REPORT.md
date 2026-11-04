# Captured operator-index lock endpoint — coordinator integration

**Verified checkpoint; fresh independent review pending.** Base
`27e350dcbff372c89c2d15e666a6c6137a061db6`; Source/oracle unchanged at
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`. Owner report is a historical
392-Git/3-binding snapshot; this integration has **393 Git / 245 Op / 4 bindings**.
No real operator staging/restoration or new publication route was added.

## Actual fix

ONE final-component resolver in `index_authority.rs` derives the endpoint used
by Git's index-lock setup. Existing `operator_index_lock_path()` now uses that
resolver at observation/apply time, not at repository capture. Raw inherited
GIT_INDEX_FILE and public spelled getters remain unchanged. All Native manual
lock checks and artifact acquisition use the existing repository getter;
operator Git commands still receive the raw selector and do Git's own chase.
No new resolver namespace, advisory lock, Git invocation, error variant,
Unsupported refusal, dependency or Source/pin change.

Host Git2.43.0/strace establishes final-component-only chasing, relative targets
joined to the current parent, five readlink attempts, and stop-on-readlink-error.
The surprising long-chain/loop endpoints are measured Git behavior, not invented
hardening. Parent links still resolve through the OS. Dangling targets, custom
and default-index links, linked checkouts, bytes and legal retarget-before-apply
remain supported. A Native-owned lock now excludes Git at the actual target,
not the spelled alias. This does NOT prove retarget-during-hook or post-CAS
reconciliation ownership, or other Git/platform versions.

## Source-first red and coordinator additions

Owner Source `safe_commit` positives at07:49:10 (relative/absolute link and
foreign target lock) precede actual Native business reds at07:51:32:
`/tmp/sk-evidence-27e-indexlock-074449/sourcefirst-index-lock3.iYJb2g.log` and
`red02-native-business-mismatch.jY6RZr.log`. Native let the hook's explicit
operator-index Git add succeed and change staging; under a foreign target lock
it even landed a real commit then failed resync. Source excluded the hook write
or refused staging, preserving foreign bytes. The original Source/native
fixtures are the same business shape, not identical paths/payloads/full objects.

Owner's permanent regressions are Native controls bound to those earlier Source
observations; they do **not** rerun live Source themselves. Coordinator adds one
permanent live Source-first paired test (relative and absolute link targets) in
`safe_commit_artifact.rs`: Source commit/lock observation asserted FIRST, then
Native. Both exclude the attempted write (128, target bytes unchanged), preserve
both symlink and unrelated partial staging/default-index bytes, and produce
identical full commit objects and COMMIT_EDITMSG bytes. Positive after-fix
consumer evidence, not a new pre-fix red (`source-first-permanent-pair.vCcuTY.log`,
strengthened final `source-first-pair-final.TAmuFZ.log` checks actual captured
requested/unrelated blobs and unstaged worktree bytes after each side).

Coordinator also retains the original `Path::join` capture behavior: join("")
already preserves the OS-string trailing separator; manual root+separator
concatenation was unnecessary and could duplicate an existing root separator.
Byte-level control passes (`captured-spelling-byte-control.7XoLDk.log`), unlike
Path equality which masks that distinction. This is inspection-driven correction
with positive controls, not an invented Source business red. Extra non-UTF8
symlink-target coverage and checked object-reader exit status strengthen the
existing tests without adding counts. Count/size alone are not parent identity
or full object equivalence. The test helper's error `commit=<none>` is only an
absent SUCCESS observation, **not** proof of no land; the original red retains
the actual landed OID/error and current refusal tests also inspect HEAD.

## Chronology / evidence qualifications

Owner provisional text/byte copy retained exactly. Its literal07:45:40 and
“before any probe” wording are not absolute pre-probe evidence: wrapped
orientation00 started **07:44:49**, before that report. It precedes Source
behavior probes and production edits, not all orientation. No backdating or
reconstructed first-red log. Fixture/setup failures, first getter-only assertion,
new-harness failures and strict-doc private-link failure are retained and are
not business reds (owner section14). All owner Cargo gates use whole focused
helper; earlier rejected architectural probe shortcuts do not certify this work.
Coordinator provisional `/tmp/sk-index-lock-endpoint-integration.md` precedes
integration probes/code. Every coordinator command here is uniquely wrapped;
all entire Cargo commands use the lease/2 jobs/no incremental/existing target
and isolated roots. No competing producer or evidence deletion.

## Stable coordinator gates

Evidence `/tmp/sk-index-lock-integration.wI8Ac2/command-log-index.tsv`.

| Gate | Actual |
| --- | --- |
| scoped Git+Op/doctests | **638/0/12 helpers/39 suites**, Git393 + Op245; `scoped-git-op.hICcqR.log` |
| workspace/doctests | **2395/0/17 helpers/193 suites**; `stable-aggregate-final-exact.a74JKr.log` |
| strict workspace all-target Clippy | `final-exact-clippy.knDtWR.log`, pass |
| full fmt | `final-exact-fmt.IG3rDR.log`, pass |
| strict Git/Op rustdoc | `strict-git-op-doc.47Q8pu.log`, pass |
| locked/offline workspace all-target check | `offline-all-targets.3eEFsV.log`, pass |
| both release CLIs + optimized Op library | `stable-release.KaxG6e.log`, pass |
| exact CPython3.11.15/53 oracle | `oracle-final.nJcT6H.log`, pass |
| completion | expected exit1 `completion-final.OFCSsA.log`: **599/PERF27/pending/accounting valid/0-of-16** |

The first chained aggregate was interrupted by the outer240s timeout during Op
oracle_vectors (`stable-aggregate.0aULRx.log`), no terminal receipt. A one-shot
process inspection found no remaining builds/tests (`inspect-after-outer-timeout.mo7RO0.log`);
fresh standalone aggregate above completed. The partial observed1354/85-suites
is NOT an aggregate success or final count. Original log retained.

Arithmetic: previous2378 + owner16 + coordinator1 =2395. Helpers17/suites193
unchanged. Exact nonscope freeze verified against27e: transaction/CAS/message/
physical planner/paths/process/refs, all Op runtime/tests, Kernel/Workspace/CLI,
Source, manifests/locks/SBOM/packs/pins/corpus untouched. Repository code/index
module and two Git test files alone change runtime/test behavior.

## Current bindings

```text
e1d7b62826a984f93f27d820cff4894b26c75d2b50bb1c4a14cdbd7c5209b91a  crates/sk-git/src/index_authority.rs
257d8365ee96854fac600bcdb0527153e56234925dfc64a14f5307850a3f4b3f  crates/sk-git/src/repository.rs
4a8c360e7a621ad08c2b922b06a28d274101afe0a02f806200cf2c93d2a0fde5  crates/sk-git/tests/index_authority.rs
9dc3c6989183cb0d199032f3a7c2def30f58e1f236075e325d4730faac5a3bef  crates/sk-git/tests/safe_commit_artifact.rs
```

## Required, not waived

Fresh independent endpoint review; real Source stage-before-encoding/R2/R3/R4,
faithful requested-path patch capture/restore/error precedence; rare fstat partial
acquisition; retarget during hooks/post-CAS/check-unlink windows; nonUTF8/path/
permissions/filter/hook breadth; other Git/platform/Windows/ReFS acceptance;
message bounds, full executor/CLI/StepContract, workflows and measured≥3×.
The proposed whole-preimage/Unix-only/symlink-refusal staging bridge remains
**not authorized**. Main remotes both15ab39216 rechecked; localmain stale,
9aed behavior reference unchanged,718 historical commits unadopted.
