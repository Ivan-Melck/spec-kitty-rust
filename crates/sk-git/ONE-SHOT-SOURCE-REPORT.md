> Historical owner delivery snapshot on dirty `1a7a62f45`, not current bindings.
> Coordinator clarification, tests, final hashes and gates are in
> `docs/rust/reviews/one-shot-source-disposition.md`. Original final bytes/mtime:
> `/tmp/sk-one-shot-source-integration.mHlRUO/owner-delivery-report-original.md`.
> Original Source execution and footprint expectations ran before Native, but
> four exact Source error-class assertions were originally after Native; these
> now run before Native too. Hooks can rescue empty argv: it is not inevitably
> an empty-message failure. The late public wrapper is textually unchanged but
> inherits the new canonical empty-argv behavior. Literal provisional time
> 11:34:30 differs from actual preserved mtime 11:34:19; both precede probes.
> Original chronological red and provisional bytes remain unchanged below.

# Source-ordered existing one-shot Git facades — PROVISIONAL

**STATUS: PROVISIONAL.** Written 2026-11-04T11:34:30Z by Ivan (implementation
sub-agent), **before** any source/API probe or edit for this slice. Timestamp is
preserved; a FINAL section is appended after implementation and verification.

Base/published remote `1a7a62f452499ef0e5d0f51949be8b8a3ff4ee69`, tree
`675367d018f2d3697753c4886227df0da9d4adcd`, worktree clean at start. Prior
Source Op staging slice was narrowly independently reviewed at `f4d662f7…`;
coordinator follow-ups (private-index mtime, foreign-current regression, hook
lock body/mode, child-umask config) are the current published 2432/18/194 state.
Review finding **F1** (prepared-message public route validates before apply) is
the gap this follow-up addresses for the **existing one-shot execution facades**.

## Intended scope (provisional, before code reading)

- `crates/sk-git/src/{safe_commit,safe_commit_physical,safe_commit_transaction,message,error,lib}.rs`
  plus scoped Git/Op tests and this report. No other production file.
- Make the **existing** one-shot Worktree facades execute in Source order:
  a real Source-style stage of the requested paths **before** the argv/message
  payload failure points, using **one** message-free planner/core and the
  canonical late execution. Nothing about the explicit two-step
  `plan`/`prepare` read-only Native construction contracts changes.
- Preserve every request field/policy/capability/raw index/source path/exact
  path/root/captured handle/30s/candidate/evidence and the pinned-parent and
  index-deletion routes with their existing prepared order/signing/payload.
- Do **not** invent a `safe_message_commit` single call or an unused public
  token/alias; bytes stay on the existing physical one-shot facade.
- Empty message is a VALID Source argv: Source runs hooks/artifact and then an
  ordinary empty-commit failure that restores the requested patch. NUL fails
  after the stage but **before** hooks/artifact. The Native 64 KiB cap remains
  a truthful Native budget, not Source E2BIG.
- Keep public `validate()`/`prepare()` exact legacy error values and their
  non-empty requirement. The deferral applies only to execution facades.
- Source-FIRST actual runtime reds before any production fix (legacy UTF-8 NUL,
  physical byte NUL, empty argv, missing second path, guard/shape/path order),
  then the minimal fix, then full scoped gates.

## Planned gates (scoped only)

Source-first reds, then Git+Op scoped tests/doctests, strict scoped Clippy, fmt
check, strict scoped rustdoc, locked/offline, exact CPython 3.11.15/53. Every
whole-Cargo run through `/tmp/sk-focused-cargo.sh`; every command wrapped by
`/tmp/sk-evidence-run.sh` with a unique fresh root. No aggregate/workspace
Clippy/docs/release, no commit/push, no Source/Kernel/Workspace/CLI/Op runtime/
manifest/dependency/corpus/pin/global-env change.

Broader residual requirements (raw index/TREE/refresh, process causality,
platform, CLI/StepContract, upstream 718, measured ≥3×) remain required and are
not claimed by this slice.

---

# FINAL — implemented and verified

**STATUS: FINAL.** Appended 2026-11-04T11:43:30Z. The provisional section above
is preserved byte-for-byte; its original bytes and mtime are also kept at
`/tmp/sk-one-shot-1a7-ivan.20261104T113542/ONE-SHOT-SOURCE-REPORT.provisional.md`
(sha256 `592e36591663f89eb98467e0bbea5573757002218f247f0ca074d275da8bb87e`,
mtime 2026-11-04 11:34:19Z), written before any source/API probe.

## Frozen subject

- Repo/HEAD `1a7a62f452499ef0e5d0f51949be8b8a3ff4ee69`, tree
  `675367d018f2d3697753c4886227df0da9d4adcd` (unchanged; no commit/push).
  `origin/main` is still `15ab392163b82741638d45eed20036216cc6c260`; local
  `refs/heads/main` is still the stale `61882b8eaff89f1c46111135bae5e7a6c52731e9`.
  No fetch/push/checkout/rebase was performed.
- Worktree at freeze: exactly 6 modified files + this report (untracked).
  Nothing outside `crates/sk-git/{src,tests}` and this report changed. No
  Source/Kernel/Workspace/CLI/Op-runtime/parser/encoder/locale/writer/
  admission-policy/manifest/dependency/lock/inventory/corpus/goldens/pins/
  global-env/cache change. No `git status` of other trees, no stash/reset/
  checkout/rebase.
- No cargo/rustc/git process was running at freeze (`freeze-pids`).
- Evidence root: `/tmp/sk-one-shot-1a7-ivan.20261104T113542`
  (`command-log-index.tsv`). Every command, including orientation, status,
  probes, Git inspection and whole Cargo invocations, has its own fresh log; all
  whole Cargo runs went through `/tmp/sk-focused-cargo.sh` (lease, 2 jobs, no
  incremental, existing repository target, isolated HOME/XDG/TMP).
- Diffstat: **684 insertions(+), 30 deletions(-)** over 6 files
  (`message.rs 92`, `safe_commit.rs 64`, `safe_commit_physical.rs 20`,
  `safe_commit_transaction.rs 7`, `tests/safe_commit.rs 72`,
  `tests/safe_commit_source_stage.rs 459`).

## Frozen hashes

```text
ea5f50ba07e8e9bbb21fea970a89dee1d386ec5c3f59fcbf2c70092ac1283e28  crates/sk-git/src/message.rs
bd1995406b45e608c1cbad016fb7219a4c400b2759e2c3fe08a7f22d648eaad6  crates/sk-git/src/safe_commit.rs
8aca964ce9d3bbdfe03891ae18055717910f31f6989d2a51e638cf3cb258757e  crates/sk-git/src/safe_commit_physical.rs
a7196d43c97f24a49ef3f50bebbbf5876856ad9d79e65f74b774251dc198dec3  crates/sk-git/src/safe_commit_transaction.rs
fce473185843923765377b8b136b4105040ff18481a1a061cabb2159271ca73c  crates/sk-git/tests/safe_commit.rs
585ec984cce337611c0a8afbca10c165c863057d9301fec65e36225436ceaf37  crates/sk-git/tests/safe_commit_source_stage.rs
```

These are the frozen worktree hashes; this report's own hash necessarily changes
when the FINAL section is appended, so the provisional bytes are hashed above
instead.

## Source facts actually probed first (live pinned Source)

Probe script `/tmp/sk-one-shot-source-probe.py`, oracle CPython 3.11.15,
isolated HOME/all XDG/TMP/CWD, real `safe_commit` from
`src/specify_cli/git/commit_helpers.py`:

- **Empty message** (`-m ""`): the requested path is staged, `pre-commit` sees
  the **old** `COMMIT_EDITMSG` (`seed\n`), the artifact is then written
  **empty**, `prepare-commit-msg` sees the empty artifact, and the commit fails
  with `Aborting commit due to empty commit message.`; the captured
  requested-only patch is restored (`MM` requested stage, unrelated/partial
  stage untouched). Logs `probe-source-empty.*`, `probe-source-empty-preartifact.*`.
- **NUL message**: `ValueError: embedded null byte` at argv conversion after the
  requested stage, **before** any hook/artifact write; the requested stage stays
  (not restored), the pre-existing artifact is untouched.
  `probe-source-nul.*`.
- Successful control: artifact `subject\n`, both hooks ran.
  `probe-source-ok.*`.

All Source-first tests below assert the reference class/HEAD/artifact/hook/stage
classification **before** the native one-shot facade is launched, and compare
the full index/HEAD/refs/artifact/hook/raw-file footprint afterwards.

## Source chronology and business reds

- 11:34:19 provisional report written (before probes).
- 11:35:49 live Source probes.
- **11:37:08 — business red**, before any production edit:
  `red-one-shot-source-first.LMxxDd.log` (exit 101), 6 new tests failed. The
  Source side had already run and its assertions passed; the failures were the
  native one-shot facades. Representative first failures:
  - legacy UTF-8 NUL / physical byte NUL: native `SafeCommitInvalidRequest`
    before staging, no requested stage vs Source's staged/kept stage;
  - legacy/physical empty: native left the old artifact and ran no hook vs
    Source's empty artifact + hooks + requested-patch restore;
  - missing second path + NUL/empty: native refused the payload
    (`SafeCommitInvalidRequest`) instead of the Source stage failure
    (`SafeCommitStageFailed`);
  - directory argument + NUL: native refused the payload instead of the path
    shape (`SafeCommitDirectoryArgument`).
- 11:37:47 `green-one-shot-attempt1` exit 101 is a **compiler** first-fail
  (`E0615`, wrong field spelling), not a business red;
  `fmt-check-attempt1`/`strict-git-op-doc` later first-fails are formatting and
  rustdoc style, not business reds.
- 11:37:52 the same 6 Source-first tests green (`green-one-shot-attempt2`).
- HEAD was never restored or rewritten to manufacture a red, and no old
  evidence/log was deleted or rewritten.

## Implementation (actual, in-scope)

One message-free planner + the existing canonical late transaction, no second
planner/transaction/writer/alias:

1. **`message.rs`** — the one byte inspection is now a typed
   `pub(crate) enum MessageDefect { Empty, TooLong, Nul }` behind
   `message_defect()`. `validate_message_bytes` (used by public `validate()` and
   prepared `plan`/`prepare`) keeps the exact legacy errors and the non-empty
   requirement. New crate-private `validate_argv_message_bytes` shares that one
   inspection, **defers Empty** (valid argv; the commit classifies it), and
   refuses NUL/over-long with the same typed errors. No `Display`-string
   matching and no second NUL catalogue.
2. **`safe_commit_transaction.rs`** — the late supplier boundary now calls
   `validate_argv_message_bytes` instead of `validate()`. Order is unchanged:
   real requested stage (capture patch, unstage, `add --force` per path,
   publish) → late payload validation → hooks → artifact → empty-commit
   classification → requested-patch restore. NUL therefore leaves the Source
   stage and writes no hook/artifact; empty reaches hooks, writes the empty
   artifact, and fails as a completed commit.
3. **`safe_commit.rs`** — added `SafeCommitTargetPlan::execution_late` and a
   crate-private `execute_source_message(repo, execution, message)` that reuses
   `execute_late` with an `Infallible` supplier and flattens the typed
   `TransactionFailure` without losing `GitError`/candidate evidence. The
   existing `safe_commit` now:
   - dispatches **first** on the route: pinned-parent or index-deletion keeps
     the exact old prepared path (`plan_safe_commit` → `apply_safe_commit`,
     message validated first, signing/payload unchanged);
   - ordinary worktree plans message-free (`plan_safe_commit_target`, the same
     planner/core) and executes late on the same request fields.
4. **`safe_commit_physical.rs`** — `safe_physical_commit` plans message-free
   and routes through the same `execute_source_message`/`execution_late`
   instead of `prepare(message)`; the exact-ref outcome fields
   (`was_unborn`, destination ref, full `P`, evidence) are preserved.
5. **Unchanged:** `plan_safe_commit`, `plan_safe_commit_target`,
   `plan_safe_physical_commit`, `SafeCommitTargetPlan::prepare`,
   `SafeCommitPhysicalPlan::prepare`, `apply_safe_commit`,
   `apply_safe_message_commit`, `apply_safe_physical_commit`,
   `apply_safe_physical_commit_with_message`, message getters/origin/reflog,
   pinned/index-deletion signing and payload. No new public API; no
   `safe_message_commit` alias; bytes remain on the physical facade.

## Tests (scoped only)

- New Source-first controls in `tests/safe_commit_source_stage.rs` (6 executed
  + 1 ignored subprocess helper): legacy/physical empty message, legacy UTF-8
  NUL, physical byte NUL, missing second path before NUL/empty, directory shape
  before NUL. Hooks record the reached boundary and the artifact bytes; every
  comparison includes index (`ls-files -s`), `diff --cached`, refs, HEAD, raw
  worktree files, `COMMIT_EDITMSG` and the hook log.
- `tests/safe_commit.rs`: the old pre-mutation empty-message assertion is
  reworked to the Source-ordered stages-then-restores behavior (same test
  count); new shape/guard-before-payload one-shot control.
- `message.rs` unit test for the argv rule set.
- Counts: **Git 437 passed / Op 246 passed / 683 total, 0 failed, 14 ignored
  helpers, 40 suites**. Previous 675/0/13/40 (Git 429 + Op 246) → +7 Git
  tests (6 one-shot + 1 shape/guard) and +1 message unit test = +8; helpers
  13→14; suites unchanged. Source corpus 32 unchanged; no performance
  promotion.

## Gates actually run (all through the wrappers)

| Gate | Actual | Log (evidence root) |
| --- | --- | --- |
| Scoped Git+Op tests/doctests | **683/0/14/40** (Git 437, Op 246) | `scoped-git-op-final2.IjpC6s.log` |
| Strict scoped Clippy all-targets | pass | `strict-scoped-clippy-final.DMl5Rs.log` |
| fmt check | pass | `fmt-check-final.vM2GLa.log` |
| Strict Git+Op rustdoc (`-D warnings`) | pass | `strict-git-op-doc-final.cGisLL.log` |
| Locked/offline scoped all-target check | pass | `offline-scoped-check-final.DModpo.log` |
| Exact oracle CPython 3.11.15 | 53/53, no drift/missing/duplicate | `exact-oracle.m8NBMu.log` |
| Oracle fail-closed foreign root | exit 1, `ok:false` (missing `uv.lock`), no fallback | `exact-oracle-foreign2.TjKYQd.log` |

No workspace test aggregate, workspace Clippy/docs, release build, or foreign
formatting was used as evidence. The Source-first suite re-runs each test with
the real pinned interpreter; the full scoped suite includes all prior
stage/artifact/byte/physical/Op tests (pinned/index-deletion, 2/2 old,
3/3 future, private-index mtime boundary, hook lock body/mode, child-0077,
foreign-current-body, Op late R2/R3/R4).

## Limits (required, not waived)

- F1 is closed only for the **existing one-shot execution facades**
  (`safe_commit`, `safe_physical_commit`) and the already-existing late
  supplier path. The explicit two-step `plan`/`prepare` construction APIs
  intentionally still validate the payload read-only and are **not** a Source
  execution-footprint claim.
- Empty-message artifact/hook parity is proven on Linux with Git 2.43 for the
  tested fixtures; the Native 64 KiB bound is still a truthful Native budget,
  not Source `E2BIG`, and valid 65 KiB Source commit parity is not claimed.
- Raw index bytes/stat/TREE/refresh parity (F2), broader racy/concurrent timing,
  full process outer-one/causality/IO, foreign/missing-index breadth, dynamic
  config/ACL/gid/hook/filter/non-UTF-8/platform/version cases, CLI/StepContract,
  protected-main false-advance, upstream 718 adoption and measured ≥3× remain
  required. No performance claim is made from tests.
- Pinned-parent/index-deletion one-shot calls intentionally retain their
  prepared-message priority; this report does not claim Source argv ordering for
  those routes.
