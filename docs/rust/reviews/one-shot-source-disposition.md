# Existing one-shot commit facades: Source ordering

> Historical `21254a41e` checkpoint below. Independent narrow review and the
> coordinator's subsequent inclusive-boundary/full-reflog fixes, current four
> overrides and gates: [one-shot-payload-disposition.md](one-shot-payload-disposition.md).
> The original review does not certify the later small fixes.

## Subject and actual behavior

Follow-up to published `1a7a62f452499ef0e5d0f51949be8b8a3ff4ee69`.
**Coordinator verified; fresh independent review pending.** This does not extend
an earlier narrow review to new code. The prior Op staging review and timestamp
follow-up remain documented in `source-staging-disposition.md`.

Existing `GitRepository::safe_commit` ordinary Worktree calls and
`safe_physical_commit` now use the same message-free planner and canonical late
transaction: requested patch capture/current-HEAD unstage/per-path real staging,
THEN payload handling. Request fields, policy/capability/index binding/exact ref/
unborn state/outcome evidence stay with the existing core. One private
Infallible adapter flattens errors without replacing real candidate evidence.
No second planner/writer, new public API, or unused byte facade was introduced.

- NUL is rejected after real staging, before hooks/artifact; staging remains.
- Empty is valid argv. It reaches hooks/artifact; if the message stays empty,
  completed failure restores only requested staging. A hook may instead supply
  the message and successfully commit. Both existing facades now have live
  Source-FIRST rescue controls binding full objects AND raw HEAD/destination
  reflogs, artifact, index, refs and worktree, not merely a failure expectation.
- One typed byte inspection (`Empty`, `TooLong`, `Nul`) serves construction and
  argv policies. Native 64 KiB remains an operational budget, NOT Source E2BIG
  or acceptance of all Source-valid payloads.
- Pinned-parent/index-deletion one-shot routes retain their existing prepared
  path and validation priority. Explicit two-step plan/prepare APIs remain
  read-only and payload-validated. Those Native construction contracts are not
  claims of Source execution-footprint equivalence.
- The existing late-supplier public wrapper keeps its signature/body, but its
  shared transaction now also lets empty argv reach hooks. It is not accurate
  to call that wrapper's empty-message behavior unchanged.

No Source, Op runtime/parser/encoding/policy, Kernel, Workspace, CLI, manifest,
dependency/lock, pins, corpus, inventory or golden data changed.

## Chronology and corrections

Owner evidence `/tmp/sk-one-shot-1a7-ivan.20261104T113542/command-log-index.tsv`.
Original final report bytes/mtime preserved at
`/tmp/sk-one-shot-source-integration.mHlRUO/owner-delivery-report-original.md`;
`crates/sk-git/ONE-SHOT-SOURCE-REPORT.md` now has a historical qualification.
Provisional copy sha256
`592e36591663f89eb98467e0bbea5573757002218f247f0ca074d275da8bb87e`, actual
mtime 11:34:19Z; its literal 11:34:30Z differs by eleven seconds. Both precede
Source probes at11:35:49; no timestamp was backdated or replaced.

Original **11:37:08 Source-first business red**
`red-one-shot-source-first.LMxxDd.log`: six tests fail before production edits.
Source ran first and its hook/artifact/stage expectations passed before Native.
Four exact Source error-class assertions originally appeared after the Native
comparison, despite the owner's broader ordering prose. Coordinator moved them
before Native too; `source-class-before-native-positive.zzD6KU.log` is an
inspection-driven positive, not a new original business red. Source execution
was never suppressed by a Native failure.

The reds bind legacy UTF8/physical byte NUL stage, empty hook/artifact/recovery,
missing-second-path failure before payload, and directory shape before NUL.
E0615, formatting and rustdoc first-fails are not business reds. No HEAD restore
manufactured red-first chronology. Existing prepared validation controls and
all prior stage/artifact/physical/Op/corpus controls remain enabled.

Coordinator also clarified conditional empty-message docs and added
`one_shot_empty_argv_can_be_rescued_by_a_message_hook_source_first` (both facades).
`empty-hook-rescue-source-first-positive.ykDqfC.log` passes on the delivered
implementation; it is new coverage, not a claimed pre-fix failure. Every Source
success/object/artifact/raw-reflog assertion precedes Native execution.

## Stable coordinator gates

Provisional integration `/tmp/sk-one-shot-source-integration.md` preceded
integration probes/code. Evidence
`/tmp/sk-one-shot-source-integration.mHlRUO/command-log-index.tsv`.
All owners stopped before workspace/release gates. Every probe uniquely wrapped;
entire Cargo commands use focused lease/2jobs/no incremental/existing repository
target/isolated roots. Live binaries use actual temporary CWD/HOME/allXDG/TMP
with assertions and the exact oracle, not env_clear alone.

| Gate | Actual receipt |
| --- | --- |
| Workspace including doctests | **2441/0/19 parent-exercised helpers/194 suites**, `stable-aggregate-final-bindings.NSxwDK.log` |
| Git+Op scoped | **684/0/14/40**, Git438 + Op246, `scoped-publish.e4HKl5.log` |
| Strict all-target workspace Clippy | `clippy-publish-final.8LrHdP.log`, pass |
| Full fmt check | `fmt-publish-final.ut04e1.log`, pass |
| Strict Git+Op rustdoc | `docs-publish-final.3gOZFH.log`, pass |
| All-target locked/offline workspace | `offline-publish-final.yjTDOC.log`, pass |
| Both release CLIs + optimized Op library | `release-publish-final.yPJUSM.log`, pass |
| Exact CPython3.11.15/53 | `oracle-publish-final.1tCgtS.log`, pass |
| Completion | expected exit1 `completion-publish-final.WWBo1L.log`, **599/PERF27/pending/accounting valid/0-of-16** |

Earlier aggregate2440 preceded the added rescue test. A 300s chained gate ran
and completed aggregate2441/scoped684, then timed out during Clippy without its
receipt (`strict-clippy-publish.iqa9eY.log`). One actual process inspection found
no surviving cargo/rustc/Clippy; fresh standalone Clippy and final aggregate pass.
The interruption is not counted as a successful gate. Coordinator's first fmt
failure for the new test was corrected manually; no foreign formatting or
warning suppression. Release/build/test counts are not performance acceptance.

## Current six source/test SHA256 bindings

Owner hashes are historical where comments/test assertions changed. All other
`1a7a62f45` source/test bytes outside these six files remain unchanged.

```text
a798c079d329d4649a84c99b6505a480976f1f45b33359c9e1555cac95f0bfe1  crates/sk-git/src/message.rs
d124a09c12943b040ed0b5adb01afc68e03d5ac238040ca93164accf7ede50c6  crates/sk-git/src/safe_commit.rs
5f0be10ac526ab4bdd550e1959f421e7424b9a0e46e3037ca26671c80fe0c342  crates/sk-git/src/safe_commit_physical.rs
a7196d43c97f24a49ef3f50bebbbf5876856ad9d79e65f74b774251dc198dec3  crates/sk-git/src/safe_commit_transaction.rs
fce473185843923765377b8b136b4105040ff18481a1a061cabb2159271ca73c  crates/sk-git/tests/safe_commit.rs
bbe831d77f037471418a67c77ade1361945f3fb1fd92829e2d4a0527176b0791  crates/sk-git/tests/safe_commit_source_stage.rs
```

## Required, not waived

F1's ordinary one-shot ordering corrected on exercised valid Source cases; NOT
all payloads, prepared execution, pinned/index-deletion ordering or full Source
process equivalence. Raw index/stat/TREE/cache-tree/refresh parity (F2), broader
timestamps/dynamic filters/config/ACL/gid/hooks/foreign or missing index,
spawn/capture/timeout/recovery-causality, fstat partial acquisition, retarget/
rename/unlink/post-CAS/six-link windows, nonUTF8/codec/wideargv/version/platform
breadth; full executor/catalogue/router/CLI/StepContract (protected-main false
advance), workflows, upstream718 semantic adoption and genuine measured >=3x.
599 completion findings and0-of-16 workflows remain. Remote main synchronization
is not Source adoption; PR stays draft, no merge/release.
