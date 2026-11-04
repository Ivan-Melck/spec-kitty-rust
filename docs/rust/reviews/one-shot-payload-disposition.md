# One-shot review: inclusive payload boundary and complete reflogs

## Independent verdict at `21254a41ed55bf4518b532f092ce1c6ec8c1b157`

**NARROW PASS WITH FINDINGS**, ordinary one-shot ordering on exercised fixtures,
not payload closure or whole process/platform/workflow/performance acceptance.
Final `/tmp/sk-rust-one-shot-source-review.md`, sha256
`fe164cf7462fd4feb972a355a13d99d3b190856abc5d42f23ebf42bd8bc6f5c9`;
provisional `/tmp/sk-review-renata-21254.root/provisional-pristine.md`, sha256
`15c2c01e54e741228ef275dee18ae2ec0ed3bceec04954c7fcbd122e0f34b5a1`.
Reviewer independently reproduces **684/0/14 helpers/40 suites** (Git438+Op246),
strict scoped Clippy/fmt/docs/offline/exact53 and drift/missing/foreign fail-closed,
all six bindings/exact12 files/clean matching remote. Own Source-first probes
confirm pre-staged-requested NUL for both facades, commit-msg rescue of empty
argv (full objects/raw reflogs), and rejecting-precommit recovery. Three payload
probes expose real boundary/budget/reflog differences. Workspace2441/release/
completion are coordinator citations, not reviewer approval.

This review certifies its fixed21254 subject only. Small fixes below were made
by the coordinator without another delegate/re-check or a new independent
verdict. Provisional `/tmp/sk-one-shot-review-followup.md` preceded new probes.

## Findings, actual code and actions

- **N1, inclusive 65536:** Source commits; Native admitted the payload then
  rejected its artifact after hooks because cleanup appended a newline. Fixed:
  artifact budget excludes one final LF, and the reader takes one additional
  sentinel byte so an oversized artifact is detected rather than truncated.
  A raw65536 payload with/without final LF round-trips; oversized non-LF and
  sentinel cases refuse. Public message budget remains65536 inclusive.
- **N2, reflog truncation:** removed the two private 2000-byte truncation
  helpers; existing audit bytes now go directly to the same canonical ref
  transaction/CAS writer. Shared ref syntax validation remains; the internal
  Source commit-audit path has no artificial subject-length budget. Public
  textual ref-plan adapter retains its exact1..=2048 validation contract.
  No lossy decode, escaped Debug, invented subject, second writer or ref rewind.
  Source owns whitespace normalization and pre-conversion byte semantics.
- **N3, Source-valid65537:** still a real required false refusal, NOT blessed by
  disclosure or this review. Native64KiB is not Source E2BIG. Merely widening
  the cap without the real OS/argv failure boundary could create false commits;
  no synthetic readiness subprocess or guessed kernel limit was introduced.
  Full Source payload/process behavior remains unfinished.
- **N4, pinned/deletion:** no new ordering approval. Native prepared priority
  remains unchanged. The review's analogy to Source's stage-then-argv byte path
  must not be generalized to pinned execution: actual Source
  `_safe_commit_with_expected_parent` writes `COMMIT_EDITMSG` as UTF8 before
  `_build_expected_parent_tree` and invokes `commit-tree -F`. That route's
  message is a file, not the ordinary `-m` argv boundary. Broader fidelity is
  still required; unchanged code is not evidence of parity.
- **N5, permanent coverage:** added Source-first65535/65536 full-object/raw
  HEAD+destination-reflog pairs for both facades and a2100-byte nonUTF8 subject
  pair; added rejecting-precommit empty recovery; extended empty rescue to
  BOTH prepare-commit-msg and commit-msg on both facades. The claim that prior
  NUL controls lacked a pre-staged requested path is not supported by the
  actual builder: it already staged v2 over HEADv1 then wrote worktreev3.
  Added explicit pre-call assertions for those distinct bytes and Source's
  full-worktree requested stage after NUL. No duplicate test is presented as
  fixing an absent case, and no passing test ratifies the65537 false refusal.
- **N6/F2:** semantic index/full-object equality does not establish raw index/
  stat/TREE/cache-tree/refresh parity. Remains required.

Obsolete unit assertions that enforced truncation were replaced by artifact
boundary/sentinel and shared public-vs-Source ref-message validation tests;
full-byte integration comparisons are stronger, not relaxed/truncated equality.
No Source, corpus, manifest/dependency/lock, pins, Op runtime, Kernel, Workspace,
CLI, strict writer/model or inventory mutation.

## Immutable red chronology

Evidence `/tmp/sk-one-shot-review-followup.CHOOIe/command-log-index.tsv`.
First orientation's guessed external filename failed rg (setup, not business).
`boundary-and-reflog-source-first-red.1IOMkA.log` is E0308 (test passed String
where &str required), NOT a business red despite its label.

**12:44:41 Source-first business red**
`boundary-reflog-source-first-business-red.IAM05R.log`: before production edits,
Source commits and asserts its full artifact/reflogs first; Native65536 refuses,
Native65535 commits with matching object but truncates raw HEAD reflog. Exactly
2 failed /8 passed /1 helper. Original large assertion bytes retained; compact
`red-classification.W2hHe0.log` is a view, never a replacement. First failures
are legacy facade; physical/raw expansions pass after the shared fix and are
not claimed as additional original pre-fix reds.

Green `boundary-and-full-reflog-green.3Niogp.log`. Expanded Source controls
`source-controls-expanded.0gUXPm.log` pass. All Source class/state/artifact/
object/reflog premises precede Native execution. Formatting first-fail is style,
not a business red; scoped owned Git formatting changes only the four intended
source/test files. Earlier review/harness/provisional logs remain untouched.

## Stable coordinator gates

All producers stopped; every probe uniquely wrapped, whole Cargo focused/
leased2jobs/no incremental/existing target/isolated roots. Actual binary Source/
Native children assert temporary CWD/HOME/allXDG/TMP; exact interpreter never
skips/falls back. No interruption or partial receipt counted as success here.

| Gate | Actual receipt |
| --- | --- |
| Workspace including doctests | **2444/0/19 parent-exercised helpers/194 suites**, `stable-aggregate.M8pyVj.log` |
| Git+Op scoped | **687/0/14/40**, Git441 + Op246, `scoped-git-op.tpfdTi.log` |
| Strict workspace all-target Clippy | `workspace-strict.itXaKm.log`, pass |
| Full fmt check | `fmt-final.tAHUTj.log`, pass |
| Strict Git+Op docs | `strict-docs.Mhsznp.log`, pass |
| All-target locked/offline workspace | `offline-all-targets.NMSJ3k.log`, pass |
| Both release CLIs + optimized Op | `release-final.UTg4aM.log`, pass |
| Exact CPython3.11.15/53 | `oracle-final.7LEz9B.log`, pass |
| Completion | expected exit1 `completion-final.FGLOXu.log`, **599/PERF27/pending/accounting valid/0-of-16** |

## Current four changed source/test SHA256 bindings

All other21254 source/test bytes remain unchanged. The six prior hashes remain
historical where overridden here. Removal of private helpers changes no public
API shape or immutable prepared contract.

```text
5c1b27767d4c254d96e7d81418b4b27a816ca88ce3c9f0a2b9164e56d9d7b515  crates/sk-git/src/refs.rs
ee1e57b24a48ac42947e5350634d45d8acfe6374080e76bb6bf1dc00037ed508  crates/sk-git/src/safe_commit.rs
0a83be7de992ae8bcbc7d116bd1b293993940f4089c7072545f04568cb090eb7  crates/sk-git/src/safe_commit_transaction.rs
87aa4bfb15df22fda1d0636bd63ed86a5cd5447049a2325ebc1130e430cfc93c  crates/sk-git/tests/safe_commit_source_stage.rs
```

## Required, not waived

N3/all payload/process/argv and prepared/pinned-route fidelity; raw stat/TREE/
refresh, broad filters/config/ACL/gid/hooks, foreign/missing-index recovery and
causal graphs, partial fstat, retarget/rename/unlink/post-CAS/six-link windows,
codec/nonUTF8/wideargv/other Git/OS/FS platforms; full executor/catalogue/router/
CLI/StepContract/protected-main false advance,90 migrations,16 workloads and
0-of-16 workflows, upstream718 semantic adoption and genuine measured>=3x.
Remote mains15ab39216 still match; localmain/activeSource9aed unchanged.
PR stays draft; no merge/release or performance promotion from these gates.
