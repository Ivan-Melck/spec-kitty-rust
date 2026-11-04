# Ordinary commit artifact — coordinator integration

**Historical `5afb1b018` integration snapshot.** Independent review now gives
NARROW PASS WITH FINDINGS. Current follow-up bindings/gates (Git376/Op245,
workspace2378) and F1 partial mitigation/F2 corrected citation/F3 required
staging phase/F4 added controls are in
`docs/rust/reviews/commit-artifact-disposition.md`. The12 hashes/counts below
remain the reviewed5afb snapshot, not silently updated hashes.

Status at integration: **coordinator verified; fresh independent review pending**. Base
`ea7174dbf9cebe37b83956c725f077770a26b80f`; Source/oracle remains
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`. Provisional coordinator plan
`/tmp/sk-commit-artifact-integration.md` preceded integration. Owner
`COMMIT-ARTIFACT-REPORT.md` is a historical pre-integration snapshot.

## Real ordinary-route change

Worktree routes (legacy UTF8/byte and physical/root) now use the real per-worktree
COMMIT_EDITMSG, exact raw Git-path spelling for message-hook argv, OLD/absent file
at pre-commit, cleaned initial file after pre-commit, and file update even before
an unchanged-changeset refusal. Hook-edited artifact bytes remain untouched;
OBJECT bytes are cleanup(readback) through the same cleanup authority and ONE
existing commit-tree/CAS helper, via an owned post-hook private object-input file.
PinnedParent and IndexDeletions keep their prior temporary-message behavior.
Ordinary writes follow/preserve Source symlinks/dangling links, with read-only/
directory failures; no authored artifact deletion/restore/atomic replacement.

Shared artifact ownership uses the EXISTING captured operator-index lock path.
Its prospective DIRC body is built in the existing scoped index from the captured
operator index, reset/re-stage only requested paths privately, preserve unrelated
entries, copy bytes into the owned lock FD. REAL operator staging is not changed.
The lock spans hooks/object/CAS/readback, then releases before existing resync.
No staged-token/Source patch-restorer or second publisher was added.

## Integration corrections — before publication

1. **Actual ownership runtime red:** owner checked file identity only in Drop.
   A pre-commit replacement of index.lock still let Native PUBLISH, then returned
   SafeCommitLanded(IndexResyncFailed). Retained
   `ownership-loss-runtime-red.ISM7sS.log` proves the head moved. This is an
   explicit native ownership-invariant red, NOT an upstream Source business red.
   Added checks after hook boundaries/before artifact and object, and before CAS.
   Detected pre-object loss refuses without writing/publishing under a foreign
   lock. After an object exists, loss uses the SAME actual candidate/observed-tip/
   reachability uncertainty path; never a fabricated no-candidate error.
2. **Known Windows compile regression removed:** owner's std volume_serial_number/
   file_index calls are `#[unstable(windows_by_handle)]` in PINNED Rust source
   commit58cf8f9c3, fetched/inspected as `rust-windows-fs.rs`. Removed that code,
   fake-zero IDs and size/mtime identity heuristic. Coordinator explicitly adds
   exact same-file1.0.6 and Windows-only locked winapi-util0.1.11, safe held-file
   handles/PID fence for checks AND cleanup. Existing windows-sys0.61.2 unchanged;
   no unsafe workspace code or Kernel/advisory lock duplication. Manifest/lock
   extension belongs to coordinator, not silent owner scope expansion. See ADR
   in DECISIONS and `docs/rust/dependencies/README.md` for registry/archive/lock
   checksums, freshness, licenses/no-build-script, OSV exact-version checks and
   regenerated164-package lock SBOM. This removes a KNOWN unstable-call defect;
   it is NOT a real Windows/ReFS certification. Only Linux target is installed.
3. Added four private guard positives: mutable body keeps identity; existing
   foreign file preserved; simulated inherited PID cannot release; identical
   replacement contents/length cannot establish ownership. These are controls,
   not invented business reds or real-fork/platform certification.
4. Public ownership regression test covers loss at pre-commit and commit-msg;
   foreign bytes survive and no ref moves. Root service's existing full-object
   Source pair now ALSO asserts exact Source/native COMMIT_EDITMSG bytes. The
   root test retains its existing Native-first ordering, not a new red-first
   claim. No Op runtime/encoder/parser/Source changes.

## Evidence chronology and accounting corrections

Owner's initial run ended without a delivery after provisional/test scaffolding;
parent checked actual tree/processes before resuming the SAME implementation.
No production edit/build was then live. Its first compiled red had earlier
compiler/harness/missing-ignored-helper failures, correctly retained, not business
reds. Git-level Source-first R1 business red at05:54:08Z:
`/tmp/sk-commit-artifact-ea717/red-git-primary.1TJv6T.log`. Review F2 corrected
this filename: previously cited `lFET3v.log` is the05:54:01 harness NotFound,
not the05:54:08 business red; both original logs remain. Actual Op R1 test was
added AFTER the fix: consumer-positive proof, not a separately captured Op red.

Exact original report-write tool event is05:48:53.922Z; wrapped repository probe
starts05:48:56Z. Its literal “05:49Z” is rounded/inaccurate, NOT an exact clock
receipt. An unwrapped clock check at05:48:44Z also preceded the report, so the
absolute “before ANY probe / every command wrapped” claim is qualified. Original
text/logs retained, not backdated. Coordinator inspected completed child activity,
not inferred chronology from file names.

Owner artifact suite log `test-git-artifact-assertions.2M3c1j.log` actually says
**11 passed + 1 helper**, NOT12 passes. Owner Git368 =357+11; Op245=244+1.
Coordinator adds5 runnable ownership tests =Git373; root artifact assertion adds
no test count. Workspace2358+11+1+5 = **2375**, helpers16→17, suites192→193.
Existing native-only touch-index.lock expectation now matches the actual
Source-owned lock (touch is not creating a foreign file). The distinct existing
IndexDeletions resync-failure control remains; no goldens/corpus edits.

## Stable gates

Evidence root `/tmp/sk-commit-artifact-integration.96Bf02/command-log-index.tsv`.
All probes use unique evidence logs; all WHOLE Cargo commands use focused lease,
2 jobs/no incremental/existing target/isolated roots. Producer stopped first.

| Gate | Actual result/log |
| --- | --- |
| Git+Op scoped/doctests | **618/0/12 helpers**,39 suites (Git373 + Op245); `scoped-git-op.kfVnet.log` |
| final workspace/doctests | **2375/0/17 helpers**,**193 suites**; `stable-aggregate-final.3Rh3RQ.log` |
| strict workspace all-target Clippy | green `stable-clippy-final.QxJTCM.log` |
| full fmt | green `stable-fmt-final.DSYb2z.log` |
| both release CLIs + optimized Op library | green `stable-release.R9GqjE.log` (product code unchanged by later test-only root assertion) |
| strict Git/Op rustdoc | green `strict-git-op-doc.0bdhBU.log` |
| locked/offline all-target check | green `offline-alltarget-final.8hJe9a.log` |
| exact53 oracle | green `oracle-final.FPABn2.log` |
| completion | expected exit1 `completion-final.BqCysT.log`: **599/PERF27/pending/accounting valid/0-of-16** |

Earlier aggregate completed, but its chained Clippy was interrupted by the
240-second outer timeout (`stable-clippy.Nkz6OK.log`, no terminal receipt).
Inspection `inspect-after-aggregate-chain-timeout.oQiI5s.log` found no live builds;
fresh standalone Clippy succeeded, then final gates above. Neither partial log
nor timeout is counted as success. Package metadata API403 and first environment
probe outside the repo (wrong stable sysroot) are retained as probe/setup limits,
not business reds; pinned-repo Rust1.99.1 and official sparse records were used.

## Current bindings / frozen surfaces

`final-counts-bindings-freeze.fOuy53.log` checks all Source/Kernel/Workspace/CLI/Op
runtime/OS-encoder/parser/writer/packs/uv/old32corpus/shared planner/ref backends
unchanged. Rust dependency addition is deliberate; Python/pack/service pins are
NOT changed. Upstream advanced one docs-index commit while checking: fork remote
main fast-forwarded/rechecked to **4d9c95313f7a355cbb2179ca61ab6045d7c7529b**;
local main unchanged/stale. New tail1file+2lines docs/development/docs-retrieval-
index.yaml, no source/pins; total historic semantic gap **706**, not adoption.
Evidence `upstream-tail-refresh.wiAVFZ.log`.

```text
ec422682aa56a9ec12215610fcdd48ca4a8c1673d6ebd53dc890d28ae5ed32c7  Cargo.toml
301a5bb2561633654cbe5a4a29818f73b31fb2f7a8b430c64cfeca3fa517b094  Cargo.lock
a70128245b2cce09d270a44d60bff8f1d90d57cec03019da6850acaf32a8256f  crates/sk-git/Cargo.toml
6dd06f62ce73f6d881f792895778d7326af7f922c3bdad9055df807767d48c5c  crates/sk-git/src/error.rs
e4557536fa251d053287aaf0debbd3ac3f4bafcf9a8bb6fb472e9f6f238b29f4  crates/sk-git/src/lib.rs
9cf36587666a115033a8ea158a2291516994a9f22ca8bce4a44f26bc17682a96  crates/sk-git/src/repository.rs
c6a6e6b13abb0c1cac66f2d9c050437c34c2205f7e495118f1c9fb1abaf003a7  crates/sk-git/src/safe_commit_transaction.rs
2ec344b91e9a066193a39d5137b2b8d16e95f58d5ea49b11e91f553f8bd5e8ff  crates/sk-git/tests/safe_commit.rs
4388fce6fb4ce4a070a4bbde5a0c475ef571317abef5aaeef3568704d4f51dba  crates/sk-git/tests/safe_commit_concurrency.rs
93455db293d73aec7387280b9ff313e1c31a00e593eb1c01df36d09020a7b98c  crates/sk-git/tests/safe_commit_artifact.rs
299288b94208b8ae2969f62bdbbf3981314196c1972f3375a26a617a05cd68b4  crates/sk-invocation/tests/op_byte_message.rs
569f26fd02f8fae0b1fdbeb79ec2647f4b9f51e8d5bad4add9d1b887e7b8ce07  crates/sk-invocation/tests/op_admission_commit.rs
```

## Still REQUIRED

Fresh independent artifact/ownership/dependency review. R2/R3/R4 highsurrogate/
NUL/missing-spine/index-on-error and real captured operator staging/precise Source
restore/exception precedence are NEXT, not fixed here. No speculative global
finally rollback; old patch alone does not establish current foreign staging
ownership. Post-CAS resync window, check/unlink races/platform/filesystem-ID limits,
lock-body/filter/environment breadth beyond tested facts, native64KiB/empty/NUL
prevalidation, IndexDeletions/pinned artifact gaps, wider ref/path/IO/F5/abbrev/
nonUTF8/locale/archive/codecs/nontext/wide-argv/context, full executor/CLI/catalogue/
governance/glossary/StepContract/protected-main advancement/migrations/workflows,
upstream semantic adoption and ≥3× measured acceptance remain required. These
library/artifact gates are not full parity/platform/performance certification;
Source-valid unsupported inputs stay required, and possibly-landed data is never
guessed away.
