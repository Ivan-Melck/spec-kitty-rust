# fb19 — Source-ordered operator staging / faithful requested-patch recovery

**Historical first delivery only, superseded by remediation and coordinator
integration.** Current gates/bindings/chronology/limits are in
`SOURCE-STAGING-INTEGRATION-REPORT.md`. Original unaccepted/dirty-tree wording
below remains historical evidence, not a statement about later publication.

**UNACCEPTED historical first delivery; not published.** Coordinator preserved
original report bytes at `/tmp/sk-source-stage-integration.CSHjFu/owner-delivery-report-original.md`
(sha256 `de7233d25eb33acc75f827f896db0f0083072201598675b5f56fec7e35e6c151`).
At09:42 current Source-first controls FAIL: core.sharedRepository0660 Source mode
0660 vs Native0664, and stateful-filter Source requested index residue vs Native
forced clean state. Three fresh repeats also expose DIFFERENT full commit trees
when Source executes3 filter passes. Pass-number data is authored data, not an
ignorable invariant. Existing masking/normalization tests do not establish
acceptance. Current strict tests retain these reds before remediation.
Chronology correction: RED-prefix logs at09:08 restore HEAD production AFTER
stage-suite green at09:05, so they are **later baseline reproductions**, not
original chronological pre-fix captures. Source-first within those later runs
remains valid. Original report wording/logs preserved, not backdated.
Owner ran forbidden aggregate/workspace/release gates despite scoped orders;
section5/8 “no aggregate/release” implications are false. Parent has not accepted
those as authorized gates. Additional publisher ownership/process-phase/recovery
and actual Source root coverage checks remain open. See coordinator plan
`/tmp/sk-source-stage-integration.md` and evidence root there. Published remains
`fb19c5c52`, not this dirty tree.

**STATUS: FINAL — implemented and verified in the working tree, not committed or
pushed (coordinator owns integration/publication).** FINAL written
2026-11-04T09:33Z (UTC). The provisional report written at
2026-11-04T08:51Z before any probe/code analysis is preserved verbatim at
`/tmp/sk-fb19-source-staging-report.PROVISIONAL.md`
(sha256 `1312a5bf043745bd11ea3d86146c5a42e9a5b61d9541dfd4160b567221859dec`).
No content in this file was backdated; the chronology below is the actual order
of work.

Base `fb19c5c52d92c21d71e55610cc3448b5edc1b544` (CLEAN, PUBLISHED, REMOTE).
HEAD stayed at that commit; the tree is dirty with only the files listed in
§7. Source/oracle baseline `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Evidence root for every command in this report:
`/tmp/sk-evidence-fb19-085122/` (84 files, `command-log-index.tsv`, one fresh
`mktemp` log per attempt; no log was overwritten or deleted).

## 1. What was implemented

One canonical order, in the existing single transaction
(`crates/sk-git/src/safe_commit_transaction.rs`), replacing the previous
"private prospective body only" behaviour for the ordinary worktree route:

1. **Capture** the requested-only staged patch
   (`git diff --cached --binary --no-ext-diff --no-renames -- <paths>`) against
   the **current** operator index. This is the pre-mutation gate: a present but
   corrupt index fails here and nothing is staged.
2. **Unstage** only the currently staged requested paths against the **current**
   `HEAD` (`git --literal-pathspecs restore --staged`, or
   `git rm --cached --ignore-unmatch` when unborn). The staged-path probe is the
   reference's `changed_paths(cached=True, pathspecs=...)`; a failed probe is
   swallowed exactly like the reference.
3. **`git add --force` each requested path**, one at a time, and **publish the
   private body into the real operator index after every step** (and after the
   unstage), so a clean filter or observer running for path *n* sees path
   *n-1* already staged. Publication is a same-directory create-new + rename to
   the single resolved physical endpoint, under the existing owned
   `OperatorIndexLock`. Unrelated entries, partial stages and flags are carried
   through untouched.
4. **Only then** is the message payload produced. A new late-message entry point
   (`GitRepository::apply_safe_physical_commit_with_message`) borrows the same
   captured `GitRepository` and the same transaction; an unencodable code point
   or a NUL therefore leaves the requested paths staged, releases the owned lock,
   and runs no restore.
5. The **candidate tree** is still built in a separate private index seeded from
   the pinned parent (the existing `--only`-equivalent that keeps unrelated
   staging out of the commit), then the existing hooks / `commit-tree` / one
   ref CAS / readback / post-landing reconciliation run unchanged.
6. **Faithful recovery**: a completed *non-landing* failure (hook rejection,
   unchanged changeset, cleaned-empty message, artifact I/O, object creation)
   restores from the **current** index — unstage the requested paths against the
   **current** `HEAD`, replay the captured requested-only patch with
   `git apply --cached --whitespace=nowarn -`, publish that body. Never a
   whole-index pre-image. Possibly-landed/unconfirmed CAS, landed repair, a lost
   lock or an unconfirmed install never rewrite the index.
7. The `OpCommitService` consumer (`op_commit.rs`) now uses that one path with
   its existing `OsEncodingContext` as the supplier; the encoding refusal text is
   unchanged (no invented "paths remain staged" sentence).

### Files

Production (7):

| file | change |
|---|---|
| `crates/sk-git/src/safe_commit_transaction.rs` | Source-ordered stage/publish/recovery, `StagingContext`, `TransactionFailure<E>`, `execute_late`, `OperatorIndexLock::publish`, `install_index_body`; module/route docs rewritten |
| `crates/sk-git/src/safe_commit_physical.rs` | `SafeCommitLateMessage<E>` + `apply_safe_physical_commit_with_message` (borrows the repository; no handle in the plan) |
| `crates/sk-git/src/safe_commit.rs` | `SafeCommitExecution` carries `Option<&[u8]>` (prepared vs late) + `execution_late` |
| `crates/sk-git/src/error.rs` | `SafeCommitPatchCaptureFailed`, `SafeCommitIndexPublishFailed`, `SafeCommitRecoveryFailed` (carries `original_failure`) |
| `crates/sk-git/src/process.rs` | raw `run_with_stdin_index_scoped` (reused by `checked_…`) |
| `crates/sk-git/src/lib.rs` | export `SafeCommitLateMessage`; crate docs corrected |
| `crates/sk-invocation/src/op_commit.rs` | consumer switch to the late-message path |

Tests: new `crates/sk-git/tests/safe_commit_source_stage.rs` (23 tests +
1 ignored subprocess helper), `crates/sk-invocation/tests/op_admission_commit.rs`
(R2 pair corrected source-first, new R4 service pair), and
`crates/sk-invocation/tests/op_byte_message.rs` (R3 footprint now asserted as
equal to the source instead of as a measured difference).

No dependency, manifest, lockfile, Source, pin, corpus, golden, Kernel,
Workspace or CLI change. No commit, no push, no subagent, no evidence deletion.

## 2. Required Source-first reds, measured BEFORE the fix

The reds were produced by temporarily restoring the seven production files to
`HEAD` while keeping the new/updated tests, running the **public service**
tests, then restoring the fixed files and verifying their sha256
(`sha256sum -c` all OK). So the reds are real runtime reds of the pre-fix
behaviour, not compile failures, and the source side was asserted first in each.

| red | source (asserted first) | pre-fix native | log |
|---|---|---|---|
| R2 used `\ud800`/`\udc00`/`\udc7f` profile | record **staged** (`A  kitty-ops/<id>.jsonl`), `UnicodeEncodeError`, HEAD unchanged | `?? kitty-ops/<id>.jsonl` (nothing staged) | `RED-prefix-service.Y5AK9T.log` |
| R3-forced-ASCII `\u00e9`/`\U0001F600`/`\u00a0` | record staged | staged set empty | `RED-prefix-byte.iUiRHy.log` |
| R4 missing doctor spine + used `\ud800` | `RuntimeError … failed to stage … pathspec did not match` (encode never reached) | `… not encodable … U+D800 at index 3` (encode first) | `RED-prefix-service.Y5AK9T.log` |

Exact pre-fix failure texts:

* `01ARZ3NDEKTSV4RRFFQ69G5FCK: identical staged status` — left
  `"?? kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FCK.jsonl\n"`, right
  `"A  kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FCK.jsonl\n"`.
* `01ARZ3NDEKTSV4RRFFQ69G5FBE: the native route stages before the same refusal`
  — left `""`, right `"kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FBE.jsonl\n"`.
* `the native route fails at staging, like the reference: Op commit message is
  not encodable by the source's filesystem encoding (utf-8, surrogateescape):
  code point U+D800 at index 3`.

After the fix the same tests pass (`op-tests-4.4ZHJil.log`, `op-tests-3`,
`workspace-final2.p2zNF8.log`).

## 3. Acceptance matrix (source first, then native, same Git state)

`crates/sk-git/tests/safe_commit_source_stage.rs` builds two identical fixtures,
runs the live pinned `safe_commit` first and asserts its result/footprint, then
runs the native route in a child with temporary CWD + isolated HOME/all XDG/TMP,
and requires the **same Git state** (HEAD, refs, `ls-files -s`, index-vs-HEAD
path set, and every worktree byte; no stat/racy-clean-sensitive `status` text).

23 passing, stable over three consecutive full runs (`stage-filterH-{1,2,3}`:
`23 passed; 0 failed; 1 ignored` each) plus the whole-workspace run above and the
`filter-measure*` subset runs:

* argv-boundary failure leaves the stage: plain, linked worktree, symlinked
  captured index, unborn-destination native control (see §5), two-path stage.
* missing second path → requested-only restore **before** encoding.
* normal commit / unrelated+partial staging preserved.
* hook rejection; unchanged changeset; `prepare-commit-msg` empties the artifact;
  read-only `COMMIT_EDITMSG` (artifact I/O).
* staged binary change (with NUL byte), staged rename pair, staged deletion,
  staged mode change — all captured and restored byte-identically.
* corrupt present index → capture gate (`SafeCommitPatchCaptureFailed`), index
  left exactly as found; source class `SafeCommitRecoveryFailed`.
* `HEAD` moved by a rejecting hook → recovery unstages against the **current**
  head and replays the patch.
* recovery rejection (replayed patch cannot apply) → `SafeCommitRecoveryFailed`
  carrying the original failure, identical residue.
* foreign `index.lock` never taken or removed (native ownership control; the
  reference has no equivalent refusal class).
* symlinked captured index retargeted **during** staging by a clean filter →
  refused (`SafeCommitIndexPublishFailed`, "refusing to publish another target's
  index under this lock"), never published under the old lock.
* clean-filter measurement (idempotent and stateful), §4.

Existing controls stay green: 32-case `fixtures/safe_commit` corpus unchanged
(`safe_commit_conformance`, binds baseline `9aed3a80`, generator sha, exact53
receipt), pinned-parent/index-deletion routes untouched, all 393 pre-existing
Git tests and 245 pre-existing Op tests green.

## 4. Measured filter/pass facts (measured, not assumed)

Instrumented clean filter counting its own invocations (no external observer;
the filter writes its own counter/log):

* idempotent filter: `source=2..3 native=3..4` (`filter-measure*.log`,
  `filter-measure-{1,2,3}.log` for the stateful pair).
* stateful filter: `source=2..3 native=3..4`, `native_adds=2`.
* The ordered route's **staging** passes are exactly two per path (the real
  operator stage and the candidate-tree `add`); a third staging pass never
  occurs (`stage_add_passes` bound asserted).
* The extra invocation relative to the reference is the pre-existing
  post-landing `git reset --quiet <landed-sha> -- <paths>` reconciliation, which
  re-hashes the worktree through the clean filter. It runs *after* the commit, so
  it cannot change the committed object. It is disclosed as a measured
  difference, not hidden.
* A **stateful** filter (output changes on every invocation) makes the reference's
  own `git commit --only` run the filter twice internally and commit the second
  pass's content, while the ordered route commits the content its candidate pass
  staged; the committed path contents differ only by that embedded pass number.
  The same test also measures that the reference can leave its *own* index one
  internal pass behind its landed object (`M kitty-ops/rec.jsonl`), while the
  ordered route always reconciles its index onto its landed blob. Both facts are
  asserted as invariants, not assumed away.

An intermediate attempt to skip the post-landing `reset` when the index already
matched was **reverted**: it removed the stimulus of the existing
`reconciliation_uses_the_landed_commit_even_when_head_advances_again` control
(that control interposes on the reset to prove the reconciliation uses the
landed sha, not a re-read HEAD). The landed-reconciliation semantics were left
unchanged.

## 5. Explicit differences and limits (not waived)

* **Unborn destination.** The reference's ordinary `safe_commit` refuses an
  unborn destination (`SafeCommitDestinationNotFound`) *before* staging, so
  there is no Source stage footprint to pair with. The native exact-ref route
  does support an unborn destination; its test asserts the source refusal first
  and then the native staged footprint, labelled as having no Source equivalent.
* **Shared permissions.** The published body is created with Git's own
  create-new mode (`0666 & ~umask`), not an authored mode, not a copy of the old
  index mode, and not `NamedTempFile`'s `0600`. `core.sharedRepository`
  group/world adjustment is **not** implemented: a measurement with that config
  set was not taken, so no claim is made about it.
* **Non-Unix.** `install_index_body` is plain `create-new` + `fs::rename` and
  compiles for non-Unix targets, but only Linux behaviour was executed. The
  pre-existing `#[cfg(not(unix))]` UTF-8 reflog check still runs before mutation
  for prepared payloads and after the stage for late payloads.
* **30 s.** The native git budget stays at the existing 30 s; the reference's
  ordinary `safe_commit` has no timeout. Not exact exception parity.
* **Phase model.** Source runs one `git commit`; Native runs several git
  processes. A native hook spawn failure is therefore not automatically "the
  source's outer commit never started": hook/artifact/object failures restore,
  the argv-boundary failure does not, and a possibly-landed CAS restores nothing.
  A hook that corrupts the index file out of band is not prevented by the lock;
  when the reference's own probe/replay cannot proceed, both sides end in the
  same non-landing class and the same residue (covered by the `HEAD`-moved and
  recovery-rejection controls).
* **Patch text.** The captured patch is read with Python's `text=True,
  errors="replace"` semantics plus universal-newline translation before it is
  replayed (that is what the reference feeds to `git apply`). Invalid UTF-8 in a
  captured patch therefore becomes U+FFFD on both sides; no raw-byte parity is
  claimed where the reference itself replaces.
* **No aggregate/release/commit/push, no performance claim, no full-platform or
  full-command acceptance.** Completion accounting (599 findings / 27 PERF /
  0-of-16 workflows), upstream reconciliation and StepContract gaps are
  untouched by this increment.

## 6. Gates actually run (all via `/tmp/sk-focused-cargo.sh`, fresh leases)

| gate | result | log |
|---|---|---|
| `cargo fmt --all -- --check` | pass | `fmt-final.rHARU4.log` |
| `clippy --workspace --all-targets --locked -- -D warnings` | pass | `clippy-final2.pl7JZ2.log` |
| `test --workspace --locked --no-fail-fast` | **2419 passed / 0 failed / 18 ignored, 194 suite results** (includes doctests) | `workspace-final2.p2zNF8.log` |
| `test -p sk-git -p sk-invocation` | 662 passed / 0 failed / 13 ignored (Git 416 = 393+23, Op 246 = 245+1) | `git-op-count.RWONIL.log` |
| `test -p sk-git --test safe_commit_conformance` (32-case corpus) | pass, baseline/generator/verify pins intact | `corpus32.7NArrV.log` |
| `doc --workspace --no-deps` | pass; 3 pre-existing warnings (sk-workspace ×2, sk-status ×1), none in sk-git/sk-invocation | `docs.Btp688.log`, `docs2.NjHiCs.log` |
| `build --release -p sk-cli --locked` | pass | `release-build.OHqdlh.log` |
| `verify_oracle.py --root .` | `ok: true`, 53 requirements, no drift/missing, CPython 3.11.15 | `oracle53.nQjcsC.log` |

No compiler/harness/setup failure from this increment is retained as a business
red; the one clippy round (`clippy.cVrHgf.log`: collapsible-if,
unnecessary-to-owned, too-many-arguments) and the two compile rounds
(`build-check5`, `clippy2`/`clippy3`: brace/impl restructuring) are style or
structure failures, not business reds, and are preserved as such.

## 7. Frozen bindings

* HEAD `fb19c5c52d92c21d71e55610cc3448b5edc1b544`, HEAD tree
  `01925e8a85632515f8ce2b944646b4b80e6cd904` (`freeze-tree.FMeMZw.log`),
  branch `rust/rewrite`, upstream `origin/rust/rewrite` at `+0 -0`
  (`freeze-status.mbsZkx.log`). No build/test processes remain; every command was
  a foreground invocation with a bounded timeout and no PID was left behind.
* Working-tree sha256 (`freeze-hashes2.*.log`):
  * `crates/sk-git/src/error.rs` `bfb3d7332022eb5ec0a46408474deced22236de9fa879a12a5a9cdd4edcc2021`
  * `crates/sk-git/src/lib.rs` `7fc49222da29fa6cd22d7da0f44e37f0663a05e8072b03dea355062927e77998`
  * `crates/sk-git/src/process.rs` `00b6e4e8130558aa2063411f6f17b088dcdc2d216a5aecbb6dc49a3a5c82af57`
  * `crates/sk-git/src/safe_commit.rs` `aebd572a05856cd1ec3e37c3e8703807c5e0ae69517d6ee92f9f407c215ecd3b`
  * `crates/sk-git/src/safe_commit_physical.rs` `4344c69b7f0c5a623d1d655b017f425c54107997429700f5a15fbebfc88852a1`
  * `crates/sk-git/src/safe_commit_transaction.rs` `a6857e86d89139ad552354b69be125b7319a441f3bc4f8486e2540a6b539b867`
  * `crates/sk-invocation/src/op_commit.rs` `e5dab3ed33c944c589a4cc9d3de864d5b51ddb29cd17b8b3d7ad4f1f45e96884`
  * `crates/sk-git/tests/safe_commit_source_stage.rs` `ca1258ba64c0b1920a2ec09ca38dcdac8e72b5c17bf4198049b25e6568db4dcc`
  * `crates/sk-invocation/tests/op_admission_commit.rs` `ba7328d20844f7e3267d9a7fe1c1e1ef9c6ad63ee85bc3445a1e6e3e83dda2d5`
  * `crates/sk-invocation/tests/op_byte_message.rs` `7d809378d3eb2c3eebfd1ef11e577db3d80fc39356c15531f71df3e18630fcc9`
  * preserved provisional copy
    `/tmp/sk-fb19-source-staging-report.PROVISIONAL.md`
    `1312a5bf043745bd11ea3d86146c5a42e9a5b61d9541dfd4160b567221859dec`
    (this FINAL report's own hash is recorded outside itself, in
    `freeze-final-report3.UvhBXI.log`:
    `3356a1a5789972fa690a8dd7690b4e91fc08737805b97d5c4bc21677b5d51c80` before
    this parenthetical was added)
* Diff size: 9 tracked files changed (+1158/−366) plus the new test file and this
  report; no manifest/lock/dependency/corpus byte changed.

## 8. What the coordinator still owns

Commit + push of this dirty tree, fresh independent review, aggregate/release
gates, and any further scope (performance campaign, platform, full CLI/workflow,
StepContract, upstream reconciliation). Nothing in this report claims those.
