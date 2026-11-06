# Canonical unborn / exact-physical publication backend — FINAL

**Historical owner snapshot, before coordinator integration.** Current evidence,
357-test scope and changed transaction/test bindings are in
`PHYSICAL-TARGET-INTEGRATION-REPORT.md`. Coordinator corrected a legacy stale-error
identity regression and native-helper/verifier inherited-CWD isolation. Original
354 tests/hashes below do not bind the integrated tree. Compiler/harness/style
failures in §8 WERE fixed and rerun; “none retried to green” below must not imply
otherwise—none was claimed as a business-red/fix sequence. The UTF8 RefName type
also does not represent every legal Git byte name; only the invented component
minimum was removed. Original observations/logs remain historical.

Status at owner delivery: **FINAL** (the PROVISIONAL revision of this file was written before any
production edit; this revision binds the delivered code, the actual commands,
counts, hashes, first-failure classifications, Source controls and limits).

Owner: implementer scope, `crates/sk-git` only. **No commit, push, checkout,
aggregate, release or environment mutation was performed.** The coordinator owns
integration, workspace aggregates, publication and independent review; the Op
wiring is the *next* increment and is **not** in this change.

| Fact | Value |
|---|---|
| Published+remote base | `102f3b81b5f5a76505ef8b2a6f0f5a077d102819` (`rust/rewrite`, clean at start) |
| Pinned Source/oracle | `9aed3a8012cb5eb22c0128b14d525039f95fea7a`, CPython 3.11.15, 53 requirements |
| Delivered tree | working tree of that commit + the files below (uncommitted, by design) |
| Evidence root | `/tmp/sk-physical-102f.9dHNM0/command-log-index.tsv` (51 wrapped commands) |

## 1. What was missing, and what this increment adds

`33a9102d8`/`fc61898c9` closed source-ordered J/H/P adjudication and the *born*
publication path. Two source-allowed shapes still had no backend; the Op reported
them honestly as `SourceAllowedButNativeCannotPublish`, which is disclosure, not
publication parity.

| Gap | Source | Native before | Native now |
|---|---|---|---|
| unborn physical ref (root commit) | `git commit --only` on an orphan `HEAD` creates a **zero-parent** commit; the judged shadow/main stay put | `SafeCommitDestinationMissing` (the legacy planner needs a destination tip) | exact-ref plan records `Unborn`, seeds git's own empty tree, `commit-tree` **without `-p`**, absent-ref CAS create |
| exact refs no `BranchName` can express | commits to `refs/x`, `refs/heads/HEAD`, `refs/heads/-name`, trailing-NBSP branches | unreachable: `BranchName` cannot name them, and `RefName` invented a `refs/<ns>/<name>` minimum | new exact-`RefName` family; `RefName` accepts every Git-legal `refs/…` name |

Both were **missing implementation**, not approved refusals (owner ruling in
`/tmp/sk-op-shadow-coordinator-assessment.md`, Q2/Q4).

## 2. Delivered design (one planner, one transaction, no second writer)

**Types.** `DestinationAuthority` (`Branch` for the legacy API / `ExactRef` for the
new one), `DestinationState` (`Born { head, tip }` / `Unborn`), `DestinationMatch`
(which HEAD authority an apply re-checks), `SafeCommitRequestView` and
`SafeCommitPlanCore` are private; both public plan families are views over one
core. Legacy `SafeCommit{Request,Plan,MessagePlan,Outcome}` keep their public
types, getters and error values byte-for-byte; they are **born-only** and say so
(their `head()`/`destination_tip()` borrow an `Oid`, which cannot represent
absence). The new family is
`SafeCommitPhysical{Request,Plan,MessagePlan,Outcome,GuardVerdict}` with
`plan_safe_physical_commit` / `apply_safe_physical_commit` /
`safe_physical_commit`.

**One planner.** `plan_safe_commit_core` runs
`Environment → Message → RequestShape → paths → destination → expected parent →
guard → operation` once. The destination step is the only kind-specific block
(short label + must-exist vs exact full symbolic ref + born-or-absent); every
other check, including all path policy and the pure guard, is shared.

**One transaction.** `execute` re-checks the operator-index authority, the
destination (exact or label, per the pinned identity), `HEAD`, the destination
value and the operation markers, then the shared body:
`read-tree <parent>` **or `read-tree --empty`** → stage exactly the requested paths
(`--literal-pathspecs`, `add --force`) → `git hook run` (`pre-commit`,
`prepare-commit-msg`, `commit-msg`) against the private index → cleanup/signing →
`write-tree` → `commit-tree` (with `-p` only when a real parent exists) → **one**
`update-ref --stdin` compare-and-swap (`RefChange::set` with the observed old
object, or `RefChange::create` = "must be absent") → `post-commit` → index
resynchronisation for the requested paths only → changed-path readback and the
existing landed/unconfirmed evidence.

**Reflog.** Git's own subjects are reproduced: `commit: <subject>` for a born
target and `commit (initial): <subject>` for the root commit; the pre-conversion
payload bytes are recorded (never the object body, never a lossy rendering). A
non-`refs/heads/` destination such as `refs/x` gets no reflog of its own, exactly
as `git commit` leaves it, while `HEAD`'s reflog entry is still written by the
update-ref transaction (probed, see §4).

**No invented state.** No seed/shadow/`main`/zero/sentinel parent, no hardcoded
empty-tree id (it is captured from `read-tree --empty` + `write-tree`), no fake
getter, no `Ready` without a plan, no widened `BranchName`, no new authorization
boolean, no second guard, no re-derivation of J/H/P.

## 3. Files (all inside `crates/sk-git`)

| File | Change |
|---|---|
| `src/refname.rs` | removed the invented `refs/<namespace>/<name>` minimum; kept `refs/` prefix and every syntax/NUL/control/option/revspec rule and the no-invented-length-cap stance |
| `src/safe_commit.rs` | private `DestinationAuthority`/`DestinationState`/`DestinationMatch`/`SafeCommitRequestView`/`SafeCommitPlanCore`/`SafeCommitCoreOutcome`; one `plan_safe_commit_core`; legacy plans delegate and keep their public shapes; `SafeCommitExecution` carries the exact ref, the pinned match, and `Option` head/tip; exact-ref guard verdict via the one decision core |
| `src/safe_commit_physical.rs` (new, 473 lines) | the exact-ref request/plan/message-plan/outcome/guard-verdict family and its entry points |
| `src/safe_commit_transaction.rs` | dynamic baseline tree, `commit-tree` without `-p`, `(initial)` reflog subject, absent-ref CAS create, exact-ref/unborn re-checks, core outcome |
| `src/lib.rs` | one private module + five re-exports + capability/limit docs |
| `tests/safe_commit_physical.rs` (new, 1206 lines) | the source-paired matrix and the in-process controls |
| `tests/symbolic_head.rs` | **one** pre-existing expectation corrected (see §5) |
| `src/error.rs`, `src/refs.rs`, `src/objects.rs`, `src/message.rs`, `src/process.rs` | **unchanged** — `RefChange::create`, `RawRefTransactionPlan`, `read_ref`, `read_ref`/`ref_exists`, `symbolic_head_ref`, `CommitMessage`, `ScopedIndex` were reused as they are |

No other crate, manifest, lock, fixture, corpus, Source file, pin, inventory or
benchmark was touched. Diff: `+889 / −245` over 5 tracked files, plus 2 new files.

## 4. Actual Git controls (run before any production edit)

Every probe through `bash /tmp/sk-evidence-run.sh <root> <label> …` with a fresh
`mktemp`; isolated `HOME`/`ALL XDG`/`TMPDIR`; fixed identity and dates.

- `probe-p1-git-legal` — `git check-ref-format` accepts `refs/x`,
  `refs/heads/HEAD`, `refs/heads/-name`, `refs/heads/-x`, `refs/heads/refs/x`,
  `refs/heads/topic<NBSP>`. The deeper minimum was **ours**, not Git's.
- `probe-p2-git-shapes` — `--short` prints `refs/x` only with the real `.git/x`
  ref; `refs/heads/HEAD` is a real branch whose `--short` is `HEAD`;
  `refs/heads/-name` shortens to `-name`; `git commit --only` on an unborn `HEAD`
  writes the `commit (initial): …` subject, a **zero-parent** commit whose tree
  holds **only** the requested paths, and leaves unrelated staged content out; the
  canonical empty tree is reproduced dynamically by `read-tree --empty` +
  `write-tree`.
- `probe-p3-mechanics` — `read-tree --empty` → `add --force` → `write-tree` →
  `commit-tree <tree>` (no `-p`) → `update-ref --stdin` `update <ref> <new> <zero>`
  reproduces the root object, the `(initial)` subject and the absent-ref CAS;
  `refs/x` gets no reflog of its own; a stale born CAS fails at `update-ref`.
- `probe-p7-nonheads-headlog` — with `HEAD → refs/x`, `update-ref refs/x` still
  appends the `commit: …` entry to `.git/logs/HEAD` and creates no `refs/x` log:
  the same footprint `git commit` leaves, so no second mutation path is needed.

## 5. Actual Source controls (live pinned oracle)

`src6-*` probes (isolated CWD/`HOME`/`ALL XDG`/`TMPDIR`, fixed
`GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE`, `PYTHONPATH=<repo>/src`,
`SPEC_KITTY_PACKS_ROOT=<repo>/packs`), fixture `work` + seed `cc570dcf…`, requested
`kitty-ops/rec.jsonl`, message `physical probe`:

| Control | Source result | Bound facts |
|---|---|---|
| `born_work` | ok | born commit `de22b47c…`, 1 parent, tree `1ed1aa8b…`, `commit: physical probe` |
| `born_nbsp` | ok | same object; `refs/heads/topic<NBSP>` advanced, shadow `topic` unchanged |
| `unborn_nbsp` | ok | **root** `830d6238…`, **0 parents**, tree `7de22fe5…` (only the requested path), `commit (initial): physical probe` on `HEAD` and the NBSP ref |
| `born_refsx` | ok | commits to `refs/x`; no `refs/x` reflog; `HEAD` reflog gets the subject |
| `unborn_refsx` | ok | root `830d6238…` on `refs/x`, shadow unchanged |
| `born_head` | ok | commits to `refs/heads/HEAD`; `--short` reports `heads/HEAD` |
| `born_dash` | ok | commits to `refs/heads/-name` |
| `born_main` | `ProtectedBranchRefused` | default protection still covers `main` |
| `unborn_plain` | `SafeCommitDestinationNotFound` | the source's step 5 refuses an unborn destination with **no shadow** |

The last row is why the source-reachable unborn family is exactly "orphan physical
ref + existing judged shadow": step 5 (`rev-parse --verify --quiet refs/heads/<J>`)
is what permits the orphan.

## 6. Delivered tests

`crates/sk-git/tests/safe_commit_physical.rs` — one fixture is built once, copied,
and then the **live** pinned `safe_commit` runs on one copy and
`safe_physical_commit` on the other (child process, `env_clear`, fixed identity and
dates). Both copies are snapshotted by the same code — refs, the full commit
object, parents, tree, **every raw reflog byte**, worktree file digests, index
entries and porcelain status — and asserted equal, plus
`native commit == source HEAD`, the exact parent count, the destination ref now
carrying that commit, and the absence of a `parent` header on a root object.

Paired matrix (8 cases, all equal): `born_work`, `born_nbsp`, `unborn_nbsp`,
`born_refsx`, `unborn_refsx`, `born_head`, `born_dash`, and
`unborn_nbsp_custom_index` (a captured `GIT_INDEX_FILE` holding an unrelated
staged entry; the default `<git-dir>/index` is asserted byte-identical afterwards
on both sides, which is the positive proof that the captured authority — not the
default one — was staged and reconciled).

Native-only controls (same file):

| Test | Contract |
|---|---|
| `the_exact_backend_is_a_capability_not_a_copy_of_the_source_shadow_rule` | the source refuses `unborn_plain` (no shadow); the backend still publishes a real root commit — source adjudication stays in the Op, the library does not copy a quirk into a refusal |
| `a_plan_records_the_real_destination_state_and_the_exact_ref` | `destination_ref()`/`is_unborn()`/`head()`/`destination_tip()`/`route()`/`paths()`/`guard_verdict()`/`index_authority()`; `refs/heads/HEAD` resolves although `--short` is `heads/HEAD`; planning is read-only (full snapshot equality) |
| `an_unborn_destination_that_appeared_is_a_stale_plan_not_an_overwrite` | absent → present race is `PlanStale`, the other writer's tip survives |
| `a_born_destination_that_moved_is_a_stale_plan` | born race is `PlanStale`, no overwrite |
| `switching_head_after_planning_is_refused` | exact `HEAD` identity re-check is `SafeCommitHeadMismatch`, refs untouched |
| `the_exact_guard_is_the_one_decision_core` | protected `refs/heads/topic<NBSP>` refused under `Standard`, allowed under a booking capability; a destination no `BranchName` can express is reported as *not in the set*, never silently widened |
| `an_empty_exact_request_is_refused_before_any_destination_probe` | shared `RequestShape` and `Message` stages |
| `a_requested_path_outside_the_policy_is_refused_by_the_shared_path_rules` | shared `PathPolicy` stage (`.worktrees/`) |
| `a_rejecting_hook_leaves_an_unborn_destination_absent` | `pre-commit` rejection on a root commit: no object, no ref, `HEAD` still unborn |
| `physical_native_helper` (ignored) | the subprocess helper the matrix spawns |

Plus one corrected pre-existing expectation in `tests/symbolic_head.rs`: it
asserted that `symbolic_head_ref()` rejects `ref: refs/x`. That assertion existed
**because** of the invented namespace minimum this task removes, so it now expects
`Some(RefName("refs/x"))` (with `git check-ref-format refs/x` as the control) and
still expects `UnreadableHead` for a non-`refs/` target. This is the only
pre-existing test expectation changed, and it is a correction of an invented rule,
not a red turned green.

## 7. Verification (all wrapped; `/tmp/sk-focused-cargo.sh` for every Cargo run)

| Command | Result |
|---|---|
| `cargo check -p sk-git --all-targets --offline --locked` | green (`check-final2-locked`) |
| `cargo clippy -p sk-git --all-targets --offline -- -D warnings` | green (`clippy-final2`) |
| `cargo fmt --all -- --check` | green, no diff (`fmt-check-final`) |
| `cargo test -p sk-git --offline` | **354 passed / 0 failed / 8 ignored**, 28 targets incl. doctests (`test-final2-sk-git`); the 32-case conformance corpus (`oracle_corpus_matches_native_transactions`, `corpus_manifest_is_complete_and_undrifted`) is green and **unmodified** |
| `cargo check -p sk-invocation --offline` (blast radius: the only consumer) | green (`check-5-sk-invocation-blast`) |
| `verify_oracle.py --root .` with the pinned interpreter | `{"ok": true, "python": "3.11.15", "checked_runtime_requirements": 53, "missing": [], "drift": []}` (`oracle-exact53`) |

Before this change the same suite was 344 passed / 0 failed; the ten additional
passing tests are this file's ten runnable tests.

## 8. First failures, classified honestly

Every first failure is retained. None was a behavioural (business) red, and none
was retried to green.

| First failure | Classification |
|---|---|
| `check-1-sk-git` — `E0277` (`SafeCommitPlanCore` lacks `Debug`/`Clone`), `?` could not convert `SafeCommitPlanFailure` | **compiler / API plumbing**, not a business red |
| `test-1-physical-only` — every case `NotARepository` | **test harness** bug (the fixture builder appended `repo/` twice), not a business red |
| `test-2-physical-only` — (a) `symbolic-ref HEAD` compared without its NBSP, (b) snapshot used a non-tolerating `rev-parse` on an unborn `HEAD` | **test harness** bugs: the helper applied `str::trim`, which strips NBSP — precisely the normalization the product must not do; the snapshot needed a tolerating read. Not a business red |
| `test-4-sk-git-all` — `symbolic_head.rs::completed_nonzero_observation_…` | **test expectation of the invented rule** (§6), corrected deliberately — not a product red |
| `clippy-1-sk-git` — `clippy::err_expect` ×3 | **style** lint in the new test file |
| `fmt-check-1` | **formatting** of the three files this change edits (verified: only those three appeared in the diff) |

An earlier exploratory stage also has two **unwrapped** orientation commands
(`git status --short`, `git rev-parse HEAD`, `git rev-parse --abbrev-ref HEAD`,
`git --version` at orientation, and one `git status --short` after formatting).
They are disclosed, carry no evidence weight, and are not cited; the identity and
file state used in this report come from `bindings-final2` (wrapped).

## 9. Bindings at delivery (SHA-256)

```text
a0afe48a298dd000710928becd303249f3c99153fde20ddf0e650e5df87c3970  crates/sk-git/src/refname.rs
6b15a44eb02da149a96623c566d70e451e0fe253701828fb969f0b8a82b89405  crates/sk-git/src/safe_commit.rs
f805b5046bd920cac1f78c98931b61865c11efb0f74157f9efaa873f74043821  crates/sk-git/src/safe_commit_transaction.rs
2976bf487385f71b3cb98de84608136ad9617638d08ceb4835c606c67a9d2022  crates/sk-git/src/safe_commit_physical.rs
0148fa7768cf955efbad0cd2705cdc71da6fec7cd5682ab386a98c4b4934517e  crates/sk-git/src/lib.rs
6e3befefd0a93674856fb374253fb50f62ca5fea4437f92aaae624eef30c6db4  crates/sk-git/tests/safe_commit_physical.rs
bb6f6428cc341f103351abd23651f059ca68fc1063682e2e3aaa863b9d441ff1  crates/sk-git/tests/symbolic_head.rs
```

(`PHYSICAL-TARGET-REPORT.md` hashes itself, so no digest is recorded for it.)

## 10. Limits and what is **not** claimed

1. **No Op wiring.** `sk-invocation` is unchanged and still returns
   `SourceAllowedButNativeCannotPublish` for these shapes; it compiles against the
   new API. Wiring J→P into `plan_safe_physical_commit` is the next increment.
   This is a Git capability, **not** Op, CLI, workflow, StepContract, platform or
   performance parity, and not a claim that Op publication is closed.
2. **Path-only exact route.** Index-only removals, a pinned parent and exact
   raw-byte assertions stay on the legacy short-branch API. They are refused for an
   unborn destination (`SafeCommitInvalidRequest`) instead of being faked; an
   exact-ref *unborn index deletion* is therefore unsupported rather than wrong.
   The shared planner already carries the branches, so extending the request type
   later needs no second planner.
3. **Exact-ref protection shape.** `ProtectedBranches` holds `BranchName`s, so a
   destination no `BranchName` can express (`refs/x`, `refs/heads/HEAD`,
   `refs/heads/-name`) can never be a member of that set. The guard decision is
   still the one `evaluate_commit_guard_decision`; policy resolution for those
   names stays in the application layer (the Op decides on the source-judged
   destination and passes the projection, exactly as today). No capability is
   added and `Standard` still authorizes nothing.
4. **`HEAD`'s own reflog** is written by git's `update-ref`, which matches the
   source for `refs/heads/*` **and** non-heads targets (probed, and asserted
   byte-for-byte in the matrix). It remains git's write, not an assertion this
   adapter owns.
5. **Not exercised here, shared-code paths:** a post-hook `AdvanceUnconfirmed` on
   the *unborn* route (the evidence path is the shared one the born corpus covers
   with a real competitor), signing (the exact route forces unsigned like the
   ordinary route; only the pinned route honours a signer, and that is corpus
   covered), and non-`refs/heads` refs on a `core.logAllRefUpdates=always`
   configuration. Named, not claimed.
6. **Unchanged boundaries:** non-UTF-8 ref bytes, non-Unix platforms, Source
   staging-before-encoding/index-on-error footprints, locale/archive semantics,
   full invocation/workflow parity and the ≥3× performance campaign stay exactly
   as the last disposition left them. Source-valid behaviour that this increment
   does not implement remains a required gap, not a waiver.
