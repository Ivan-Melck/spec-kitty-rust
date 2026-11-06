# Op root / exact / non-heads publication — implementation report

**Historical owner delivery snapshot.** Coordinator subsequently clarified
success-only accessor docs, fixed two strict-rustdoc private links (one doc-only
python_json change), and tightened the captured-index child environment/root
requirement. Current six bindings/2358-workspace gates/limits are in
`OP-PHYSICAL-PUBLICATION-INTEGRATION-REPORT.md`; old five bindings below do not
bind those comment/test follow-ups. Actual runtime publisher wiring is unchanged. Coordinator chronology qualification:
the five Native runtime red assertions stop BEFORE the paired Source commit
call; complete Source success/object comparisons execute in later green runs.
They are not same-attempt Source-commit-first counterexamples. Source admission
may run before the red; earlier independent Source evidence is separate. This
qualification preserves the original logs rather than inventing stronger evidence.

Status at owner delivery: **FINAL implementation report (not an independent review, commit, push or
workspace aggregate)** at published base `3fabdf29f96d789658e5220246aa853e5f850e39`
(docs-only over the reviewed Git physical backend
`abd0b5450940f64d44e5d6a2a311cdff4cfdc400`). Source authority remains
`9aed3a8012cb5eb22c0128b14d525039f95fea7a` (CPython 3.11.15 / 53 oracle
requirements). The coordinator owns integration, aggregates, release, commit and
push; this work is intentionally uncommitted.

## Delivered behavior

`OpCommitService` now publishes through **one** exact-physical
planner/transaction for every Source-allowed call:

1. The one per-call Source-ordered judge on `J` is unchanged: shape → roots →
   `H == J` → `refs/heads/J` existence → Source path proof → project OR record
   policy plus the one canonical STANDARD guard. `J` is still judged as the
   Source string; it is never forced through `BranchName` or
   `NativeProtectedBranches`.
2. `P` is now the **exact full symbolic `HEAD` ref** parsed as `RefName`
   (`refs/heads/<P>`, `refs/x`, `refs/heads/HEAD`, `refs/heads/-name`, an
   unborn ref), not a stripped short branch. The request keeps the already
   allowed `J` verdict as an **empty** protected projection, so the Source's
   actual-only-protection quirk is preserved and no P-only hardening is added.
3. Only after the Source guard allows does the same opened repository/index
   (30 s budget) build `SafeCommitPhysicalRequest` and call the existing
   `plan_safe_physical_commit`. The late code-point → native OS encode →
   `CommitMessage` → `prepare` → `apply_safe_physical_commit` order is the
   existing byte bridge; the encoder is unchanged.
4. Removed are only the now-implemented artificial blockers: the short
   `BranchName` parse, the `refs/heads/` prefix requirement, and the unborn
   plan refusal. `SourceAllowedButNativeCannotPublish` remains for a genuine
   inability (an exact ref `RefName` cannot parse, or a real planner failure);
   it is never a Source refusal, fake plan, seeded parent, or guessed OID.
5. Evidence: representable physical branch success keeps the legacy
   `OpCommitEvidence::Committed { destination: BranchName, .. }` shape. An
   exact ref no short branch can represent now returns the typed
   `CommittedPhysical { commit, destination: RefName, was_unborn,
   committed_paths, warnings }`. Additive getters `committed_ref()`,
   `committed_commit()` and `was_unborn()` expose the truth without a lossy
   string or fabricated short name. `AdvanceUnconfirmed` now names the exact
   physical destination (`refs/...`) the backend pinned; this explicit format
   change is documented on the field. Its `observed_tip` remains `Option` and
   is never guessed when the destination cannot be read.
6. Doctor-sweep closes still commit only the closure spine; agent closes still
   commit only their per-record file. Audit rows/index/landed data are never
   rewound or deleted by a failure.

## Source-real cases actually exercised

Every Source/native pair uses the live pinned `ProfileInvocationExecutor`
(`op_write_refusal` / `_commit_op_record`) and real Git, never a monkeypatched
guard. Full-object/native-bound pairs run in a child with fixed author/committer
dates and a real temporary CWD, HOME, every `XDG_*` root and `TMPDIR`; the child
permanently asserts those roots and that its CWD is not the source checkout. The
Python probe support now also asserts the same isolated roots inside the
interpreter.

* Unprotected unborn physical `topic\u{a0}` with the real judged `topic`
  shadow: Source and native both create a **zero-parent root**, advance only
  `P`, leave `refs/heads/topic` and `refs/heads/main` untouched, and match the
  complete commit object, OID, raw branch reflog payload, `P` tip, record bytes
  and committed path set.
* Born physical `refs/x` with `refs/heads/x`, `refs/heads/refs/x` and the real
  loose `.git/x` disambiguator (`J == H == refs/x`): both commit the same full
  object/OID, `P` advances, the shadow/main refs do not, no non-heads branch
  reflog is invented, and HEAD reflog bytes match.
* Unborn physical `refs/x` with the same disambiguator and `refs/heads/refs/x`
  shadow: both create a zero-parent root on `refs/x`; `was_unborn == true`,
  HEAD reflog bytes and the full object match.
* Born branch `-name` (`refs/heads/-name`): both commit; typed full-ref
  evidence names `refs/heads/-name`, the branch reflog payload matches.
* Branch literally `refs/x` (`refs/heads/refs/x`): both commit; typed evidence
  names `refs/heads/refs/x` (no `refs/x` short-name fabrication).
* Protected and invalid-config non-heads `J = refs/x` on born and unborn
  physical targets: admission and close refuse before any physical plan, the
  reason matches the live Source, and no ref moves. A linked record worktree
  proves both project and record policies participate (either side alone
  refuses); an invalid record policy refuses with the canonical detail.
* Branch literally `HEAD` and the tag-disambiguated `heads/topic` case stay
  Source step-4 refusals with audit admission; the Git helper's manual
  `destination=HEAD` success is not copied into the Op.
* Unshadowed unborn destination stays a Source step-5 refusal with audit
  admission even though the native backend can publish roots; no ref is
  created.
* Doctor-sweep root close on unborn `topic\u{a0}` commits only
  `kitty-ops/op-closures.jsonl`; a pre-existing archive record (committed on
  `main`) stays byte-identical, and the full object/OID/reflog match Source.
* Root commit whose message uses `\udc80`: the raw branch reflog retains the
  surrogateescape byte `0x80`, Git's own conversion produces the same complete
  commit object as Source, and the record bytes are unchanged.
* Root CAS race through a real pre-commit hook publishing a foreign seed:
  `OpAdvanceUnconfirmed` retains a real **zero-parent** candidate, the exact
  `P`, the observed foreign tip, `last_op_commit`, and the authored record
  bytes; nothing is rewound. A second control makes the destination unreadable:
  `observed_tip` stays `None` (never guessed), the candidate/detail remain, and
  the foreign malformed ref is retained.
* Root close with a captured `GIT_INDEX_FILE`: the operator index receives the
  record, unrelated staged content survives, the default `.git/index` raw bytes
  are unchanged, and the commit is a zero-parent root carrying exactly the
  requested path.
* Source-relative record/spine input is preserved: a linked record worktree
  commits exactly its relative `kitty-ops/<id>.jsonl`, never an absolute
  project-root projection.
* Ordinary born-branch, byte/locale, legacy JSON, doctor, index, hook and
  evidence controls remain green.

## First failures and classification

Genuine CURRENT-service Source false-refusal business reds, before the
publication wiring (the existing 233-test baseline was green; these tests were
written to assert the Source-real publication):

| Log | Observed current-service failure |
| --- | --- |
| `red-shadow_branch_unborn_unprotected_root_pu.hfk8oG.log` | unborn NBSP: `native publisher cannot prepare a plan: safe commit destination branch "topic\u{a0}" does not exist` |
| `red-non_heads_born_refs_x_publication_matche.TK4T5C.log` | born `refs/x`: `unsupported ... non-branch symbolic HEAD target` |
| `red-non-heads-unborn-retry2.C3LtC0.log` | unborn `refs/x`: same non-branch refusal |
| `red-dash_prefixed_branch_publication_matches.x2eig0.log` | `-name`: `not a valid branch name ... starts with '-'` |
| `red-judged_refs_prefixed_branch_publication_.8XxyS7.log` | `refs/heads/refs/x`: `not a valid branch name ... must not start with 'refs/'` |

Non-business failures encountered and fixed (retained, not counted as Source
reds): a fixture helper called `rev-parse HEAD` before publishing an unborn
destination (`red-non_heads_unborn_refs_x_root_publication.Q0MYxZ.log`); the new
test compared unbound OIDs before adding the fixed-metadata child
(`green-judged_refs_prefixed_branch_publication_.2hZ0eG.log`); a test expected
the legacy `Committed` shape to carry `was_unborn`
(`op-admission-full2.e7uu6a.log`); a byte-window length was off by one
(`green3-root_raw80_surrogateescape_message_match.4IArLw.log`); a compile
`E0277` on comparing `&Option<Oid>` with `Option`
(`cas-none-test.50GebR.log`). Format/clippy gates also failed once each
(`op-fmt-check.rMzhXZ.log`, `op-clippy1.MX9I9U.log`). `cargo doc` with
`RUSTDOCFLAGS=-D warnings` reports two **pre-existing** private-item links
(`op_commit::with_git_options`, `python_repr`; `op-doc.nmIWIQ.log`); plain
`cargo doc` is green (`op-doc-final.rY9aT7.log`). None of these is presented as
a Source business red or as a green-washed release gate.

## Final verification (actual, scoped)

Unique evidence root: `/tmp/sk-op-physical.dgPlMU/`
(`command-log-index.tsv`). One pre-order orientation (`ls`, reading the orders
file, raw `git status/log`) ran before the order was loaded; it made no
decision. After that, every verification attempt — including the recorded
orientation/status/hash/exploratory Git probes — used `/tmp/sk-evidence-run.sh`
with a fresh `mktemp` log, and every whole Cargo command also used
`/tmp/sk-focused-cargo.sh` (lease, 2 jobs, no incremental, existing target,
isolated roots).

| Check | Actual result / log |
| --- | --- |
| `sk-invocation` tests incl. doctests | **244 passed / 0 failed / 3 ignored**, 10 suites; `op-package-final2.ZfGK5t.log` (`op_admission_commit`: 66 passed / 2 ignored) |
| strict all-target Clippy (`-D warnings`) | green `op-clippy-final2.ilDbKk.log` |
| `fmt --check` (scoped) | green `op-fmt-final2.YgCTpQ.log`; own files written by `op-fmt-write2.6pR7tu.log` |
| locked/offline all-target check | green `op-offline-final2.bGw8Em.log` |
| docs | green `op-doc-final.rY9aT7.log` (plain; see pre-existing `-D warnings` links above) |
| exact-53 oracle (CPython 3.11.15, isolated) | green `oracle-final2.V7fccC.log`: 53 checked, no missing/drift/duplicate/unsatisfied |
| `sk-git` blast (unchanged crate) | **357 passed / 0 failed / 8 ignored**, 28 suites; `git-blast.XsX9tP.log` |
| unchanged frozen Git bindings | all seven SHA-256 values match the accepted backend report; `final-bindings.5O91VU.log` |
| scope | only `crates/sk-invocation/src/{op_admission,op_commit,op_target}.rs`, `crates/sk-invocation/tests/{op_admission_commit.rs,support/mod.rs}` and this report; no Kernel/Workspace/CLI/Git/Source/parser/encoder/writer/manifest/lock/fixture edits |

New/changed file SHA-256 bindings:

```text
78a5d7ead31d3b3c15f39c106385fe5763e476ba68cfed8568f3f75162a3dccf  crates/sk-invocation/src/op_target.rs
5b58e2769d74ccbc93227dda3bad5b1e2a022f78c4e348a0ec068ecfa45c4229  crates/sk-invocation/src/op_commit.rs
a933502c4f56a7cab07090dbbbd7bccdc685d00893049ee327931e4252114e72  crates/sk-invocation/src/op_admission.rs
3bd05dee4ad8d607b20ac2792cacb9e72783ca562cf49e971a62fde097b51c13  crates/sk-invocation/tests/op_admission_commit.rs
da04777c23f9b1b1830f03f44333462ead2fe9692fcf11dc8160fc4e3777cf93  crates/sk-invocation/tests/support/mod.rs
```

The report's own bytes are not self-hashed; recompute the file hash after any
edit. The code/test bindings above are the implemented-source bindings.

Count arithmetic: baseline **233 / 0 / 2 ignored / 10 suites** + 11 new passing
tests = **244 passed**; the new ignored captured-index child makes ignored
**3**. The unchanged `sk-git` binding set keeps its accepted **357 / 0 / 8**
result. No workspace aggregate, release build, subagent, commit, push, global
environment mutation, cache clean or evidence deletion ran in this task.

## Remaining required (not waived)

Source-valid behavior still outside this increment: `COMMIT_EDITMSG`/hook
argument artifacts, the non-commit error phase, Source stage 7-before-8 and
index-on-encoding-error/missing-spine/NUL footprints, broader ref/path/IO/probe/
error/F5/abbrev/non-UTF-8/platform behavior, message/reflog/process bounds,
locale archives and other codecs/non-text `LC_ALL`/wide argv, the full
catalogue/router/governance/glossary/ProfileInvocation/StepContract and
CLI/lifecycle/migration/workflow surface, upstream `705`-commit semantic
adoption, and the real ≥3× performance campaign. Completion accounting remains
`599 / PERF27 / pending / valid 0-of-16`. This report proves the implemented Op
root/exact/non-heads publisher and its scoped tests; it is not full rewrite,
platform, workflow or performance certification, and a source-valid unsupported
input stays required rather than waived.
