# COMMIT_EDITMSG / ordinary hook artifact — implementation report

**Historical owner delivery snapshot.** Current integration/bindings/gates are
in `COMMIT-ARTIFACT-INTEGRATION-REPORT.md`. Coordinator found a real ownership-
loss publication red, added phase fences/portable same-file identity and explicit
manifest/lock/ADR/SBOM evidence, plus five tests and a root artifact assertion.
Owner368Git/245Op/hash values below are not the integrated373Git/245Op tree.
Artifact matrix below actually has11 passes+1helper (not12passes). Original
report-write event is05:48:53.922Z, before05:48:56Z repo probes; literal05:49Z is
rounded/inaccurate. Unwrapped05:48:44Z clock inspection means “beforeANYprobe” /
“everycommandwrapped” must be qualified. Original statements/logs retained.
Owner Windows identity calls are UNSTABLE in pinned Rust (not merely untested);
size/mtime/fake-zero identities were removed, not certified by disclosure.

Status at owner delivery: **FINAL for this artifact-only increment** (implementation frozen; coordinator owns
integration/aggregate/commit/PUSH/review). Provisional section was written in this file at
**2026-11-04T05:49Z**, before any repository probe, build, or code change. The provisional
wording/timestamp is retained below rather than backdated or reconstructed. Final measurements
were completed 2026-11-04T06:04Z.

## Fixed base

- Repo: `/home/exedev/.t3/scratch/2026-11-02-pull-current-spec-kitty-from-442fd12a/spec-kitty-rust`
- HEAD verified through the evidence wrapper at 05:48:56Z:
  `ea7174dbf9cebe37b83956c725f077770a26b80f`, branch `rust/rewrite`, clean except for this
  increment's untracked report and test file.
- Source oracle: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`, CPython **3.11.15 / 53**
  (`{"ok": true, "python": "3.11.15", "checked_runtime_requirements": 53, "drift": [],
  "missing": [], "unsatisfied": []}`).
- Runtime baseline quoted by the orders: 2358 workspace / 244 Op / 357 Git tests. This increment
  did not run workspace aggregates or release gates.

## Inputs read completely before code

- `/tmp/sk-native-ea717-artifact-orders.md`
- `/tmp/sk-source-commit-phase-coordinator-assessment.md` (mandatory corrections; the design's
  false "Git cannot root" claim, existing operator path/lock authority, lock held through CAS,
  and valid-DIRC prospective lock body were treated as authoritative over the design prose)
- `/tmp/sk-source-commit-phase-design.md`
- `AGENTS.md`, `docs/rust/DECISIONS.md`, `docs/rust/LOOP.md`, `docs/rust/VERIFICATION.md`,
  `.kittify/charter/charter.md`, and the loaded implementer-ivan profile.

## What changed (actual diff)

| Path | Change |
|---|---|
| `crates/sk-git/src/repository.rs` | New private-field `CommitMessagePath` with read-only `spelling()`/`absolute()` and `GitRepository::commit_message_path()` using raw `git rev-parse --git-path COMMIT_EDITMSG` bytes. Existing `operator_index_path()` / `operator_index_lock_path()` reused unchanged. |
| `crates/sk-git/src/safe_commit_transaction.rs` | `SafeCommitRoute::Worktree` now writes/reads the real per-worktree artifact, passes git's exact path spelling to `prepare-commit-msg` and `commit-msg`, writes the artifact only after a successful pre-commit and before the unchanged-changeset refusal, computes object bytes from `cleanup(artifact readback)` without write-back, and feeds the one existing `commit-tree`/CAS authority through a private post-hook object-input file. Adds the owned captured-index `.lock` with a valid prospective DIRC body, PID/identity cleanup, and hold-through-CAS lifetime. PinnedParent and IndexDeletions routes keep their previous private temp-message behavior. |
| `crates/sk-git/src/error.rs` | Typed `SafeCommitIndexLocked`, `SafeCommitIndexLockUnavailable`, `SafeCommitMessageFileUnwritable`. No `Display` string reclassification. |
| `crates/sk-git/src/lib.rs` | Re-export `CommitMessagePath`. |
| `crates/sk-git/tests/safe_commit_artifact.rs` | New live Source-first/native-paired matrix (12 tests, 1 of which is the ignored native child helper): primary OLD artifact, linked absent absolute spelling, hook edit vs object cleanup, pre-commit rejection, unchanged changeset, authored symlink, dangling symlink, read-only, directory, prospective DIRC lock with unrelated staged entry, custom index with default-index byte preservation. |
| `crates/sk-invocation/tests/op_byte_message.rs` | Actual `OpCommitService` R1: source `_commit_op_record` runs first with a hook that reads `$(git rev-parse --git-path COMMIT_EDITMSG)`; native real service must commit with the same artifact/spelling. |
| `crates/sk-git/tests/safe_commit.rs` | One stale native-only expectation corrected to Source lock ownership (see below). |
| `crates/sk-git/tests/safe_commit_concurrency.rs` | Caller retry classification now includes the Source-shaped `SafeCommitIndexLocked`; the test still asserts all retried writers land exactly one commit each and operator staging survives. |

No changes to Op runtime/Kernel/Workspace/CLI/parser/encoder/writer/dependencies/manifests/Source
Python/fixtures/goldens/inventory/benchmarks. No commit or push.

## Implemented Source facts

- Pre-commit runs with the OLD or ABSENT artifact; the cleaned message is written only after a
  successful pre-commit, including for an unchanged changeset before the no-op refusal.
- `prepare-commit-msg` gets `[git-path COMMIT_EDITMSG, "message"]`; `commit-msg` gets
  `[git-path COMMIT_EDITMSG]`. Primary spelling is `.git/COMMIT_EDITMSG`; linked is the absolute
  per-worktree gitdir path. Nothing is re-derived from the common dir or UTF-8-assumed.
- The object body is cleanup(artifact readback), while the artifact keeps hook-written bytes,
  including whitespace edits. No success/failure path deletes, restores, or renames the artifact.
- Artifact writes use ordinary open/truncate: author link followed/preserved, dangling link created
  through, read-only and directory cases fail.
- The ordinary route creates git's canonical captured-index `.lock` with `create_new`, holds it
  through hooks, object creation, explicit CAS and the ref readback, then releases it before the
  existing post-CAS index resync. Foreign existing/replaced files are preserved by PID + file
  identity; no blind unlink/retry.
- The lock body is a **valid prospective DIRC index**: a private copy of the operator index is
  restored/re-staged only inside the private copy, requested blobs are added with the existing
  literal-pathspec helper, unrelated staged entries survive, and only our lock FD is populated.
  The real operator index is not mutated.
- The clean-up authority is one function (`MessageCleanup::render`); the one existing
  `create_commit_object`/ref-CAS helper is reused. No second commit writer.

## First-failure chronology (all retained logs)

Evidence root: `/tmp/sk-commit-artifact-ea717` (fresh logs per attempt; index in
`command-log-index.tsv`).

1. 05:48:56Z — wrapped orientation (`git rev-parse HEAD`, `git status`) confirmed base/clean.
2. 05:49Z — provisional report written before probes/code (original timestamp above).
3. 05:53:48Z — compile failure in the new test (`TempDir` moved before `Io::create`; unused import).
   **Compiler/harness failure, not a business red.**
4. 05:53:54Z — test hook log missing because the harness resolved `hooks` from the private
   gitdir's parent. **Harness isolation/path failure, not a business red.**
5. 05:54:01Z — ignored native child helper did not run (missing `--ignored`).
   **Harness failure, not a business red.**
6. **05:54:08Z — FIRST business red:** the Source side committed; native's pre-commit hook recorded
   `/tmp/.../tmp/.tmpXXXX` as its spelling while the expected Source spelling was
   `.git/COMMIT_EDITMSG`. This is the retained pre-fix chronology for R1. The Op-level R1 test was
   added after the production fix, so **no separate Op-level pre-fix red was captured**; the Git
   live pair is the business red that drove the fix, and the Op test now verifies the real consumer
   Source-first.
7. Post-fix regressions found by scoped suites, all resolved with Source reasoning:
   - rename pair failed because the prospective copy lacked the Source unstage step;
   - `a_commit_that_lands_but_cannot_resync…` expected a foreign lock created by a hook touching the
     lock we now own; corrected to `UnrequestedPaths` + no foreign lock, with the
     `IndexResyncFailed` branch still covered by the unchanged IndexDeletions route;
   - unborn prospective cleanup surfaced `git rm --cached`'s non-zero exit; it is now best-effort
     like the Source and the following `add --force` replaces the entry;
   - strict Clippy flagged one test literal unwrap; `cargo fmt --check` flagged formatting only.

## Verification actually run

Every command ran through `/tmp/sk-evidence-run.sh`; every whole Cargo command additionally through
`/tmp/sk-focused-cargo.sh` (2 jobs, `CARGO_INCREMENTAL=0`, existing `target/`, isolated
HOME/XDG/TMP, external absolute manifest paths).

| Gate | Log | Result |
|---|---|---|
| First business red (Git live pair) | `red-git-primary.lFET3v.log` (and preceding harness attempts) | FAILED as expected; native temp-file spelling |
| New artifact matrix | `test-git-artifact-assertions.2M3c1j.log` | 12 passed / 0 failed / 1 ignored helper |
| Full `sk-git` tests | `test-git-full-final2.JZziQm.log` | **368 passed / 0 failed / 9 ignored** |
| Full `sk-git` offline | `test-git-offline.s0LRS6.log` | 368 passed / 0 failed / 9 ignored |
| Op R1 (Source first) | `green-op-r1.S4BkpV.log` | 1 passed |
| Full `sk-invocation` tests | `test-op-full-final2.1uRDIC.log` | **245 passed / 0 failed / 3 ignored** |
| Full `sk-invocation` offline | `test-op-offline.zbSWZg.log` | 245 passed / 0 failed / 3 ignored |
| Git/Op doctests | `doc-git.CFELvf.log`, `doc-op.aOjSRl.log` | pass |
| Scoped strict Clippy (`sk-git`, `sk-invocation`, all targets, `-D warnings`) | `clippy-final2.JeYyi6.log` | pass |
| Strict rustdoc (`RUSTDOCFLAGS=-D warnings`, no deps) | `doc-strict.rxvZgw.log` | pass |
| `cargo fmt --all -- --check` | `fmt-final2.2Ikrsg.log` | pass |
| Exact 53 oracle | `oracle-exact53.8buz7E.log` | ok, CPython 3.11.15, 53/53, no drift/missing |
| `git diff --check` | `diff-check.u8Y6CK.log` | pass |

No workspace aggregate, release build, commit, push, background poll, or subagent was run.

## Source controls bound by the new tests

- Primary OLD-SENTINEL pre-commit; linked-worktree absent artifact.
- Exact relative vs absolute `COMMIT_EDITMSG` spelling in each real fixture, compared with that
  fixture's own `git rev-parse --git-path` output (not a basename or cross-repo substitution).
- Cleaned artifact bytes, prepare/commit-msg readback, and object-body cleanup without artifact
  write-back.
- Unchanged changeset updates the artifact before refusal; rejecting pre-commit leaves the OLD
  artifact.
- Symlink preserved, dangling link created through, read-only and directory failures preserve
  authored state.
- Hook-visible `.lock` is a valid `DIRC` body containing the requested blob and an unrelated staged
  entry, and the lock is gone after the CAS.
- Captured custom index commits while the default `.git/index` bytes remain unchanged.
- Actual Op consumer R1 (hook reads the real artifact, Source commits first, native matches the
  artifact/spelling and full commit shape via `commit_pair`).
- Both the source driver and the native child assert a real temporary CWD plus HOME/XDG/TMP roots
  before doing any work; `env_clear` alone is not treated as isolation.

## File hashes at freeze (sha256)

```
6dd06f62ce73f6d881f792895778d7326af7f922c3bdad9055df807767d48c5c  crates/sk-git/src/error.rs
e4557536fa251d053287aaf0debbd3ac3f4bafcf9a8bb6fb472e9f6f238b29f4  crates/sk-git/src/lib.rs
9cf36587666a115033a8ea158a2291516994a9f22ca8bce4a44f26bc17682a96  crates/sk-git/src/repository.rs
ec55af8620795fa95dbfc56d3427395ac28fc83fec4efe53e6a334ef89429185  crates/sk-git/src/safe_commit_transaction.rs
2ec344b91e9a066193a39d5137b2b8d16e95f58d5ea49b11e91f553f8bd5e8ff  crates/sk-git/tests/safe_commit.rs
4388fce6fb4ce4a070a4bbde5a0c475ef571317abef5aaeef3568704d4f51dba  crates/sk-git/tests/safe_commit_concurrency.rs
a0e09f37f7d5454962d81c55dc10b7ee84b4f36bda44c44958b14713ac221979  crates/sk-git/tests/safe_commit_artifact.rs
299288b94208b8ae2969f62bdbbf3981314196c1972f3375a26a617a05cd68b4  crates/sk-invocation/tests/op_byte_message.rs
```

`crates/sk-git/COMMIT-ARTIFACT-REPORT.md` is intentionally excluded from its own hash list
(writing the final report changes its bytes).

## Limits / explicitly unwaived

- R2 (`\udc00`), R3 (used NUL), R4 (missing SPINE + high surrogate), Source stage-before-encoding
  ordering, precise Source snapshot/restore/exception precedence, and real operator-index staging
  remain **REQUIRED NEXT**. No staged token, real operator-index stage, or patch restore was added.
- Native early raw-empty/NUL/64 KiB caps and validation position remain unchanged known gaps.
- The lock body is exposed prospectively, but hook environment (`GIT_INDEX_FILE` partial-index
  name), lock file permissions, and broader lock-content parity beyond the tested entries are not
  claimed.
- The post-CAS `reset` resync window remains a pre-existing residual, not a full concurrency or
  "never wrong data" proof.
- Non-Unix lock identity uses the platform file id where available and a length/mtime fallback
  elsewhere; this is not a platform certification. No blanket unsupported branch was added.
- PinnedParent and IndexDeletions behavior is intentionally unchanged; their artifact/hook parity
  remains a later tranche (Source's IndexDeletions route uses a real `git commit`).
- Root/unborn artifact bytes are exercised through the physical Source/native suite but are not
  separately asserted by the new artifact test file; root publication behavior is unchanged.
- Functional port only: this is not a cybersecurity review. Completeness 599 / PERF 27 / 0-of-16,
  full CLI/StepContract/governance/glossary/profile-invocation, upstream 705-commit adoption, and
  >=3x performance remain required.

## Provisional text retained (written 2026-11-04T05:49Z)

> Status: PROVISIONAL — written before any repository probe, build, or code change in this
> increment. Written 2026-11-04T05:49Z (actual clock; not backdated, not reconstructed). This file
> will be finalized in place with real hashes, counts, commands, first-failure chronology, Source
> controls, and limits once the increment is done.
>
> Selected increment: actual Source ordinary-worktree COMMIT_EDITMSG / exact hook argv through the
> same canonical transaction, artifact-first only. R2/R3/R4 remain required next. Scope: sk-git
> canonical transaction/index ownership/minimal message path/errors/exports + Git/Op tests + report.
