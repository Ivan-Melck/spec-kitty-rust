# JP target adjudication / F4 admission + born publication — FINAL

Status: **historical owner-final snapshot**, Op232/Git338 before coordinator
path/observation/classification/isolation corrections. Current bindings/counts/
Source qualifications: `JP-TARGET-INTEGRATION-REPORT.md`. The
pre-code PROVISIONAL revision of this report was written before any source edit;
this final replaces it with the actual implementation, real first failures, and
verification bindings.
Repository `spec-kitty-rust` published `33a9102d8f23b35a91cdb9d3cdd6bc4149d9d9b8`,
clean tree; source/oracle pinned `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Orders: `/tmp/sk-native-33a-jp-target-orders.md`; adopted designs
`/tmp/sk-op-shadow-branch-design.md`, `/tmp/sk-op-shadow-coordinator-assessment.md`,
`/tmp/sk-git-byte-message-preflight-design.md`,
`/tmp/sk-byte-message-coordinator-assessment.md` (read together, amendments
included). This report is not a review verdict and waives no residual gap.

## Three source identities (actual source, not labels)

- **J** — `executor._current_branch`: `git symbolic-ref --quiet --short HEAD`,
  Python `stdout.strip()`. Judged destination for shape/HEAD/existence/
  protection/refusal/admission.
- **H** — `commit_helpers._read_worktree_head`: full `git symbolic-ref HEAD`,
  Python `stdout.strip()` of the **whole** stdout, then
  `removeprefix("refs/heads/")`. Compared with J (source step 4). The strip
  happens **before** prefix removal, so `topic\u{a0}` matches `topic` while
  `\u{a0}topic` does not.
- **P** — exact physical suffix of the full symbolic `HEAD` ref
  (`refs/heads/<P>`), taken from `GitRepository::symbolic_head_ref()` and used
  only as the native publication/CAS/index/evidence destination.

J and H are never derived by stripping P's short name; P is never re-derived
from `--short`.

## Source target order implemented

`preflight_commit` actual order:

1. `J.startswith("refs/heads/")` refusal **before** roots/HEAD/existence.
2. non-empty paths (one Op path always supplied).
3. `_is_worktree_of(project_root, record_root)`.
4. `H == J` (full symbolic ref, strip then remove prefix).
5. `refs/heads/J` exists: `rev-parse --verify --quiet`; any completed non-zero
   is absent (not the commit reachable from `HEAD`).
6. source path normalisation, 6a `.worktrees/`, 6b directory argument with the
   symlink/missing/gitlink exemptions.
6c. project-root OR record-root `ProtectionPolicy` judged on **J**, then the
   canonical STANDARD guard.

The Op evaluator (`sk-invocation/src/op_target.rs`) reproduces that order once
per call for admission and for the close. No authorization is cached; the source
primary-tracking cache is unchanged. Source preflight failures admit the audit
write but refuse the actual commit — the two verdicts stay separate.

## Verdicts

`OpTargetVerdict`: `SourceRefuses` (steps 1/3/4/5), `SourcePathRefuses` (2/6a/6b
and absolute path loop), `Protected { judged }`, `InvalidConfig { detail }`,
`Repository(GitError)`, `Protection(WorkspaceError)`, `Ready { target, plan }`,
and explicit
`SourceAllowedButNativeCannotPublish { judged, cause }` (unborn P, unrepresentable
P, or any other native-only non-publishable shape). There is no `Ready` without a
prepared native target plan and no fabricated parent/seed/`BranchName`.

Admission maps `SourceRefuses`/`SourcePathRefuses`/`SourceAllowedButNativeCannotPublish`/
`Ready` to `Ok(None)`, `Protected`/`InvalidConfig` to the existing Op write
refusals, and probe failures to the existing typed errors. Close maps the same
verdict to a best-effort refusal or to the one canonical commit.

## Canonical shared seams (no second validator/writer)

- `sk-git/src/safe_commit_paths.rs` (new, authorized): one read-only path
  observation/shape/gitlink authority used by the native planner **and** the
  source-compatible Op projection (`source_preflight_path`). The native
  planner's default order/errors are unchanged; moved helpers keep their exact
  argv/status facts.
- `sk-git/src/safe_commit.rs`: planner delegates to the shared path helpers;
  `evaluate_commit_guard` delegates to one pure
  `evaluate_commit_guard_decision(protected: bool, capability)` core.
- `sk-git/src/objects.rs`: the already-promoted full `HEAD` read is reused
  unchanged; `RefName` is not widened to `HEAD` and `BranchName` is not widened.
- `sk-invocation`: `op_target` (identity/order/verdict/projection),
  `op_commit`/`op_admission` wiring; `has_destination_commit`,
  `ProtectionRead::BranchName`, and the admission "reached-guard" heuristic are
  deleted rather than duplicated.

Only `apply_safe_message_commit` mutates. The projected protected set is the
verdict decided on J mapped 1:1 onto P, so the source actual-only-protection
quirk is preserved (an unprotected J commits even when only raw P is configured
protected). Refusals name J; typed evidence names P.

## ATDD red-first matrix (actual service + live pinned source)

Six core rows: physical `topic\u{a0}` with existing `topic` shadow, positive
primary, born/unborn × unprotected/protect-J/invalid-config. Four D1
counterexamples: branch `refs/x`, protected/invalid config × missing
admission-sentinel path (source refuses at 6c) / sentinel directory (source
refuses at 6b, so the write is admitted). Admission snapshots cover
HEAD/refs/status/index/record bytes.

Expected after the fix:

| row | source | native |
|---|---|---|
| born unprotected | commits on `refs/heads/topic\u{a0}` | commits, same full object/reflog/paths/P tip |
| born protect-J / invalid | refuses protected / invalid | same refusal, nothing moves |
| unborn unprotected | root commit, no parent | admission admits; close truthfully `SourceAllowedButNativeCannotPublish`; P stays absent |
| unborn protect-J / invalid | refuses | same refusal (guard decided before the native plan) |
| D1 missing sentinel + protect/invalid | refuses at 6c | same refusal |
| D1 sentinel directory + protect/invalid | admits (6b) | admits |

## Residuals kept explicit (not waived)

Unborn publication, exact existing-ref publication (`refs/x`, `HEAD`, `-name`),
and any other source-commits/native-refuses case remain required increments.
The close reports them as native non-publishable, never as source refusals or
fake succeeds. Source quirks F4-P1/P2/P3, wording/`repr` (F5), `--abbrev-ref`
fallback, staging-before-encoding footprint, cross-platform, full executor/CLI/
StepContract/workflow, aggregate gates, and the 599/PERF27/0-of-16 completion
accounting remain open. No manifest, lock, kernel, backend writer, parser,
locale, CLI, fixture/golden, or docs/rust change is in scope.

## Implemented files (scope only)

| File | Change |
|---|---|
| `crates/sk-git/src/safe_commit_paths.rs` (new) | one read-only path observation/shape/gitlink authority + `source_preflight_path` |
| `crates/sk-git/src/safe_commit.rs` | planner delegates to the shared path helpers; `evaluate_commit_guard` delegates to the one `evaluate_commit_guard_decision` core |
| `crates/sk-git/src/lib.rs` | `mod safe_commit_paths`, exports `SourcePathRefusal`, `source_preflight_path`, `evaluate_commit_guard_decision` |
| `crates/sk-invocation/src/op_target.rs` (new) | J/H/P identities, source-ordered `judge_record_target`, verdicts, guard-before-plan |
| `crates/sk-invocation/src/op_commit.rs` | close consumes the verdict, names J on refusal, evidence names P; `has_destination_commit`, `resolve_protected`, `ProtectionRead::BranchName` removed |
| `crates/sk-invocation/src/op_admission.rs` | admission consumes the same verdict; "reached-guard" heuristic deleted |
| `crates/sk-invocation/src/lib.rs` | private `mod op_target` (no new public type) |
| `crates/sk-invocation/tests/op_admission_commit.rs` | 6-row shadow matrix, 4 D1 admission counterexamples, exact-ref close, tag mismatch, actual-only-protection quirk |
| `crates/sk-git/tests/source_path_preflight.rs` (new) | real-Git source path controls (missing/regular/directory/`.worktrees`/symlink/loop/lexical `..`/gitlink-probe failure/absolute in-out) |

No manifest, lock, pin, dependency, kernel, backend writer, parser, locale,
CLI, fixture/golden, `safe_commit_transaction`, or `docs/rust` change.

## RED first — real first failures against published `33a9102d8`

Evidence root `/tmp/sk-jp-target.3V0XmP` (unique `mktemp` log per attempt,
`command-log-index.tsv`):

- `red-shadow.FWTFhT.log`: `0 passed; 4 failed`.
  - born protected/invalid and **unborn protected/invalid** admission
    `expect("the judged name refuses")` returned `None` → the four native
    false-admits.
  - born unprotected close refused with
    `safe commit worktree … has HEAD "topic\u{a0}", expected branch "topic"`
    instead of publishing P.
  - unborn unprotected close refused the same way instead of reporting
    source-allowed/native-nonpublishable.
- `red-d1.IeOtVe.log`: `0 passed; 2 failed`.
  - `refs/x` + protected/invalid with a **missing** sentinel was admitted
    (early `BranchName` fallback) although the source refuses at 6c.
  - the `refs/x` close returned
    `not a valid branch name: "refs/x" (must not start with refs/)`
    before any source path/guard proof.
- `red-tag.IcciIj.log`: passed before the change; the tag-disambiguation
  refusal is a preserved source fact, not a new behavior.
- Source admission/commit decisions in every RED run came from the live pinned
  `ProfileInvocationExecutor`, never from a stub.

## Green verification (final state)

| Gate | Result | Log |
|---|---|---|
| `sk-invocation` package (10 suites incl. doctests) | **232 passed, 0 failed, 2 ignored** (was 224; +8) | `final-test-op.oEWACi.log` |
| `op_admission_commit` file | 54 passed, 0 failed, 1 ignored | inside the above |
| `sk-git` package (26 suites incl. doctests) | **338 passed, 0 failed, 7 ignored** (was 332; +6 new path controls) | `final-test-git.KSd9nc.log` |
| strict scoped Clippy, all targets | green (`-D warnings`, `--locked --offline`) | `final-clippy2.PyfMUU.log` |
| scoped `fmt --check` | green | `final-fmt2.aKoOjE.log` |
| exact oracle closure | `{"ok": true, "python": "3.11.15", "installed_count": 53, "missing": [], "drift": [], "unsatisfied": []}` in an isolated CWD/HOME/all-XDG/TMP | `final-oracle-exact53.nxagzr.log` |

The born-unprotected pair runs in a child process carrying the fixture's fixed
author/committer dates (no process-global mutation), so the complete commit
object, OID, reflog payload, paths, and physical P tip are compared, not just
the decision.

## Changed-file bindings (sha256)

```
0a282ccd6f6427fc39c527610c51b95e09f6dfde8306c585bdb0147812b4b9cd  crates/sk-git/src/lib.rs
53d4e063dcef04a37ca08a2899746c792b07c9bd28ad9888859e288bbebc25dd  crates/sk-git/src/safe_commit.rs
8dc2dad41870abb8c6920929df77c3c62dcc515f8ec4da937d50749a6deac37a  crates/sk-git/src/safe_commit_paths.rs
f239a4992f903c37b989c2e36c345826d9343cf1f3aa4a37ddff77df17b3b7d3  crates/sk-git/tests/source_path_preflight.rs
3b1ef9c65b5a03afc6cf668ef8ae94f641999971cc5fa167591acc5549951ffc  crates/sk-invocation/src/lib.rs
8ebdbf17eed3cb3083a0ca3d9fce55935be694e3b0c1b7b425e7c84f155b3943  crates/sk-invocation/src/op_admission.rs
392714b0a1caea29ad40d2b5864c5fafbbbea688934deb7e82995403db5fc97f  crates/sk-invocation/src/op_commit.rs
8890240b455a13e14e4db0356123f463d2ef8982822a4345855649d3678447b2  crates/sk-invocation/src/op_target.rs
d1e48da4525260218289f0997d71c6ace10bd19f68a90834c1af4fa94861754d  crates/sk-invocation/tests/op_admission_commit.rs
```

## What is now source-parity on the exercised rows

- The four shadow-family native false-admits (born/unborn × protect-J/invalid)
  now refuse with the source code and reason; admission snapshots
  HEAD/refs/status/index/record bytes are unchanged 6/6 on both sides.
- Born unprotected `refs/heads/topic\u{a0}` publication is one canonical
  `SafeCommitTargetPlan`/apply on P; source and native bind the same full
  commit object, OID, reflog payload, paths and P tip, with `topic` and `main`
  seeds unchanged.
- Unborn unprotected is source-allowed and reported natively as exactly
  `SourceAllowedButNativeCannotPublish` (no plan, no evidence, P absent). The
  live source root commit has one parent token (no fabricated parent).
- The D1 `refs/x` family now distinguishes the source path order: a missing
  sentinel reaches 6c (protected/invalid refusal), a sentinel directory refuses
  at 6b and admits; both before any `BranchName`/P fallback. The close reports
  the unrepresentable publication truthfully.
- The actual-only-protection quirk (config protects only the raw P spelling)
  still commits on P, as the source does.
- The tag-disambiguated `heads/topic` vs `topic` mismatch still refuses on both
  sides.

## Residuals (required next increments, not waived)

- Unborn publication backend (absent-ref CAS, root commit, no parent/seed).
- Exact existing-ref publication for short names `BranchName` cannot represent
  (`refs/...`, `HEAD`, `-name`).
- Source wording/`repr` (F5), staging-before-encoding footprint, `--abbrev-ref`
  fallback, non-UTF-8/Windows argv, archive/other-codec/locale settings.
- Full executor/CLI/StepContract/workflow/platform/performance gates and the
  aggregate completion accounting (**599 / PERF 27 / 0 of 16**) are unchanged
  and were not run by this owner. No aggregate/release/commit/push was made;
  the coordinator owns gates, commit, push, and fresh independent review.

