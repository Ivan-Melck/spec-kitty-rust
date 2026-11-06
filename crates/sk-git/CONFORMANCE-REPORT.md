# Scoped-safe-commit conformance report

> Historical corpus-delivery report. Coordinator oracle-driven corrections
> subsequently raised exact matches to **30/32**: ordinary signing policy,
> both reflog sources, initially-empty candidate handling, and GIT_EDITOR now
> match the reference. One literal-path correction and one unsupported custom
> index remain explicit. See `docs/rust/reviews/git-coordination-continuation.md`;
> hashes and counts below describe the original delivery.

> **Forward note (operator-index continuation).** The open mismatch recorded in
> §5 is closed. `GIT_INDEX_FILE` is captured at repository open and bound to
> every operator-index read, write, lock check and reconciliation; the renamed
> case `git_index_file_selector_honoured` is now an exact oracle match, and the
> corpus is **31 exact matches + 1 documented literal-path divergence + 0 open
> mismatches**. The historical text below is retained unchanged except for this
> note. See `OPERATIONS-CONTINUATION-REPORT.md` for the current evidence and
> `docs/rust/reviews/git-coordination-continuation.md` for the coordinator's
> correction.

Status: **evidence report for the native safe-commit substrate; not an
independent review and not overall safe-commit/CLI signoff.**

This report is bound to the *current source bytes*, not to a commit id and not
to the independent review's earlier hashes. The shared checkout is being edited
concurrently by other tranches, so the table below is the identity of the code
these observations were taken against.

| File | SHA-256 |
| --- | --- |
| `crates/sk-git/src/safe_commit.rs` | `e9b5075929c275d4d771b7c6eacc766acbedbe187972d4fc44fd3bb442295530` |
| `crates/sk-git/src/safe_commit_transaction.rs` | `1e06015ad60408c50124020dbc52c62851a54a8bd2af9978c22b34140f4219cf` |
| `crates/sk-git/src/process.rs` | `8b1863011a9b4b95186a59ad5099a2649884aed1eb659a8703678a43c141273d` |
| `crates/sk-git/src/error.rs` | `73ca4bf2757dcc094914af5a93ad982caecba6c0322beecde661014553c0e0c9` |
| `crates/sk-git/tests/safe_commit.rs` (unchanged by this work) | `a2d66c90ad7938e80c8acfd7b8657d0693ef0ad8586e28d8055100f849e7fa16` |
| `crates/sk-git/tests/safe_commit_concurrency.rs` (unchanged) | `7973fd012c6895d29dad2655cf2411a46ad5820f240b7e5a6f2ef824f69eb847` |
| `crates/sk-git/tests/safe_commit_authority.rs` (unchanged) | `892a5b25a7d3cda18778341e8e7fc6bbf5752f912e26fe444ce33b56e1957f61` |
| `crates/sk-git/tests/safe_commit_differential.rs` (doc header clarified; assertions unchanged) | `c0f6d6efa3a7a99c719a0adb1dd35193289beed7ff83e05ec02ce0f4bd3a223d` |
| `crates/sk-git/tests/fixtures/safe_commit/generate_oracle.py` (new) | `77a3d9c92660ee6fb2641b3f7c3a7bca7e87697b68e435da08860eef725aba52` |
| `crates/sk-git/tests/fixtures/safe_commit/manifest.json` (new) | `7c2e88d434204538e8bf5a7f6258cf2d552fe0e24f9833c0faf4b59df5cb9f8c` |
| `crates/sk-git/tests/safe_commit_conformance.rs` (new) | `e0af9b50ca9a13ff72674123213ad21437cf9bda1ede6cd03b46a4c4f362156f` |
| `crates/sk-git/tests/common/safe_commit_oracle.rs` (new) | `6a5f0c53ba6dbb70a28c27cfa47358b4274d0677e4308c015ad750b6e0717133` |

Only new files were added under `crates/sk-git/tests/`, plus a doc-header
clarification in `safe_commit_differential.rs` that removes its false
"oracle evidence" claim (its eight assertions are unchanged and remain fast
native-only regressions). No production source, `safe_commit.rs` /
concurrency / authority test, manifest, workspace build or release build was
changed by this work. No commit, push or timing campaign was
performed.

## 1. Corpus and provenance

- Pinned reference: `spec-kitty/spec-kitty` at
  `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
- Oracle interpreter: `$REPO/.venv-rust-oracle/bin/python` (CPython **3.11.15**),
  proven lock-exact by `scripts/rust/verify_oracle.py`:
  `ok: true`, **53/53** runtime requirements checked, `drift: []`,
  `missing: []`, `duplicate_versions: []`, `unsatisfied: []`.
- The four reference sources (`commit_helpers.py`, `commit_guard.py`,
  `ref_advance.py`, `protection_policy.py`) were verified byte-identical to the
  baseline before generation (`git diff --quiet 9aed3a8 -- …`).
- Git: `git version 2.43.0`, Linux.
- **32 cases**, each checked in as `cases/<name>.json` (working-tree additions;
  the coordinator owns the commit) with a SHA-256 digest in `manifest.json`:
  **26 exact matches**, **5 recorded divergences**, **1 open mismatch**.
  `PROVENANCE.json` records the generator digest, verifier report and case-list
  digest.
- Regeneration is the checked-in generator only. The test fails on a missing
  corpus, a missing case, a digest mismatch, an unexpected expectation, a
  missing required behaviour case, a changed generator without regeneration, or
  a failed recorded oracle verification. There is no skip path.

## 2. Coverage

| Area | Cases |
| --- | --- |
| preservation | `preserve_unrelated_staged_partial_untracked`, `rename_pair_as_delete_plus_add`, `intent_to_add_and_staged_deletion_preserved`, `unmerged_entries_preserved`, `index_only_deletion_keeps_file`, `worktree_deletion_committed`, `sparse_in_cone_preserves_skip_worktree`, `sparse_out_of_cone_refused` |
| message pipeline | `message_cleanup_whitespace`, `message_cleanup_strip_config`, `message_cleanup_verbatim_config`, `whitespace_only_message_refused`, `pinned_message_verbatim`, `ordinary_soft_wrapped_reflog_source` |
| hooks | `prepare_and_commit_msg_rewrite` (`prepare-commit-msg` `(file, "message")`, `commit-msg` `(file)`, both rewrites), `commit_msg_rejection_refused`, `pre_commit_rejection_usage_phrase_not_environment`, `noop_runs_hooks_before_classification`, `hook_formatter_makes_candidate_nonempty`, `hook_stages_extra_paths_landed`, `pinned_hook_changes_requested_content`, `nested_hook_advance_conflict`, `post_cas_competitor_reconciled_to_landed_commit` |
| paths / index | `literal_unusual_filenames` (space, leading dash, tab, newline, colon, quote, hash, semicolon, dollar, Unicode), `pathspec_magic_literal_name`, `dot_relative_path_committed`, `directory_argument_refused` |
| destinations / authority | `protected_branch_refused`, `git_index_file_selector_refused` |
| signing | `pinned_signing_failure_typed`, `worktree_signing_enforced_divergence` |

The literal-filename case commits ten files including `line\nbreak.txt` and
`tab\tname.txt`; the sparse case preserves the out-of-cone `S` bit and the
in-cone commit; the ITA/conflict cases preserve intent-to-add and three-stage
unmerged entries byte-for-byte while an unrelated path commits.

`nested_hook_advance_conflict` proves the F6 remediation against the oracle:
both sides end at the hook's nested commit, native returns
`SafeCommitAdvanceUnconfirmed` with the candidate and observed tip, and the
candidate is verified unreachable from the nested commit.
`post_cas_competitor_reconciled_to_landed_commit` proves the F1 remediation:
the competitor lands directly on our commit, the final HEAD is the competitor,
and the operator index holds the *landed* commit's `p.txt` blob, never the
competitor's. The existing `safe_commit.rs` regression
`reconciliation_uses_the_landed_commit_even_when_head_advances_again` also
passes and covers the ordering window itself.

## 3. What is compared, and the only normalisations

Compared exactly: branch, HEAD, every ref, status, every index entry (mode,
blob, stage), every index flag (`H`, `S`, unmerged), every worktree file
(digest, text), exact commit message bytes, tree, parents, changed-path set,
reflog `(new id, subject)` sequence, hook invocation count and argument
content, the hook's candidate-index view (`git status`, `git diff --cached`),
cwd, `GIT_DIR` shape, and the complete after-state. Commit and tree ids are
deterministic because branch, identity and dates are pinned, so they are
compared directly, not ignored.

Normalised (documented, also in `manifest.json`):

1. reflog entries compare as `(new commit id, subject)`; committer identity and
   timestamp are not observed;
2. error text compares as a canonical refusal class plus explicit detail
   tokens, because Python has no typed taxonomy and native messages differ;
   the recorded oracle detail replaces the throw-away run root with
   `<RUN_ROOT>`, so the committed evidence contains no host path;
3. hook arguments compare as `text:<value>` / `file:<content-sha256>`; the
   temporary message-file *path* is not part of the comparison;
4. `git_editor`, `git_prefix` and `index_in_git_dir` are recorded but excluded
   from hook-evidence equality (the Rust harness asserts the native invariant
   for each; the recorded differences are in §7).

## 4. Results

Commands used (existing `target/`, bounded jobs, no workspace/release build):

```sh
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p sk-git --locked
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 \
  cargo clippy -p sk-git --all-targets --locked -- -D warnings
cargo fmt -p sk-git -- --check
```

- `cargo test -p sk-git --locked`: **192 passed (189 tests + 3 doctests),
  0 failed, 2 ignored** across the crate's ten suites. The two ignored tests are
  the subprocess helpers `redirected_safe_commit_child` and
  `native_case_helper`; both are exercised by their parents.
- `cargo test -p sk-git --test safe_commit_conformance --locked
  oracle_corpus_matches_native_transactions`: passed **5 consecutive times**
  (4.5–5.3 s each), including the post-CAS background interposition.
- `cargo clippy … -D warnings`: clean.
- `cargo fmt -p sk-git -- --check`: clean.
- One earlier full run exited 1 in *doctests only* with
  `can't find crate for 'tempfile'`: a concurrent cargo process in the shared
  `target/` replaced the exact `libtempfile-*.rlib` mid-invocation. The doc
  suite was rerun immediately afterwards (3 passed), and the clean full rerun
  above then passed end to end. The failure was build-artifact churn in the
  shared target, not a source or test failure.

These results are scoped to `sk-git` on Linux/SHA-1. They are not a
whole-workspace gate, not a CLI wiring claim and not a performance claim.
The older `SAFE-COMMIT-REPORT.md` suite arithmetic and its "pinned oracle
tests" description are superseded for the differential scope by this corpus.

## 5. Open mismatch (recorded, not counted as conformance)

### `GIT_INDEX_FILE` — unsupported parity gap

The corpus case `git_index_file_selector_refused` records the one known
unsupported input. It is **not** normalised into equality and is excluded from
the 26 matches.

- Oracle: honours the caller's index, commits into it, leaves `.git/index`
  untouched (`after.head != before.head`; `custom_index` entries change).
- Native: `GitError::RepositoryEnvironmentRefused { variable: "GIT_INDEX_FILE" }`
  before any mutation (`after == before`; real and custom index, ref and
  worktree unchanged).

Minimal repro (temporary project; `$IDX` outside the worktree):

```sh
git read-tree HEAD                      # GIT_INDEX_FILE="$IDX"
git add unrelated.txt                   # into the custom index
echo two > a.txt
GIT_INDEX_FILE="$IDX" <oracle safe_commit paths=("a.txt")>
#   oracle => Ok(<sha>)
GIT_INDEX_FILE="$IDX" <native GitRepository::safe_commit paths=("a.txt")>
#   native => RepositoryEnvironmentRefused { variable: "GIT_INDEX_FILE" }
```

This is a deliberate fail-closed boundary (`docs/rust/DECISIONS.md`: writer
authority refuses inherited repository selectors until a captured context owns
both lock and mutation namespaces), but it is still an input the baseline
supports. It must remain an open mismatch until captured-environment support
lands.

## 6. Recorded divergences (all visible, none green-washed)

1. **Ordinary-route configured signing is enforced by native.**
   `worktree_signing_enforced_divergence`: with `commit.gpgsign=true` and a
   failing `gpg.program`, the baseline's ordinary route lands an *unsigned*
   commit (`git -c commit.gpgsign=false commit`), while native honours signing
   and refuses with `SafeCommitCommitFailed`; HEAD/index unchanged. The pinned
   route already agrees on refusal (`pinned_signing_failure_typed`). This is the
   stated remediation ("both routes honor signing"), not a parity match.
2. **A hook can rescue an initially empty candidate in native; the baseline
   cannot.** `hook_formatter_makes_candidate_nonempty`: a `pre-commit` hook
   rewrites and re-stages an unchanged requested file. Baseline: hooks run, then
   `SafeCommitStagedTreeUnchanged`. Native: the reformatted candidate lands
   (`diff: formatted`, `a.txt` reformatted). This is the deliberate
   "hooks before no-op" ordering.
3. **Literal pathspecs.** `pathspec_magic_literal_name`: a file literally named
   `:(glob)*.txt`. The baseline expands the pathspec, lands `a.txt`, `b.txt`,
   `other.txt` and the literal file, then reports
   `SafeCommitUnrequestedPaths`. Native passes `--literal-pathspecs` and commits
   only the literal path; the other modifications stay unstaged.
4. **Ordinary-route reflog of a soft-wrapped one-paragraph message.**
   `ordinary_soft_wrapped_reflog_source`: message `subject\nbody`. Baseline
   reflog `commit: subject` (git's reflog takes the first line); native
   `commit: subject body` (reads the commit back with `--format=%s`, the first
   paragraph). Commit object, tree and sha agree. Native preserves more audit
   text; this is an intentional superset, not claimed equality.
5. **Pinned-route reflog source after a message rewrite.**
   `pinned_commit_msg_rewrite_reflog_source`: `commit-msg` appends a trailer.
   Baseline writes the *requested* message to `git update-ref -m`; native writes
   the *final* rewritten message. The commit bytes/tree/sha agree; only the
   reflog subject differs (native contains `Signed-off-by: hook`).

For 4 and 5 the reflog difference is asserted field-by-field: all other
snapshot fields are proved equal, and the two subjects are proved different.

## 7. Hook-environment differences (recorded, invariant-asserted)

The Rust harness asserts the native invariant for each excluded field instead
of silently dropping it:

| Field | Baseline (ordinary / pinned) | Native | Meaning |
| --- | --- | --- | --- |
| `index` | `private` / `private` | `private` | both keep the operator index out of the candidate; compared equal |
| `index_in_git_dir` | `yes` / `no` (pinned index in `TMPDIR`) | `yes` (scoped index in `.git`) | candidate-index location; not semantically required |
| `git_editor` | `set::` / `unset` | `unset` | baseline hooks run under `git commit` and see `GIT_EDITOR=:`; native hooks run via `git hook run` |
| message-file argument | relative `.git/COMMIT_EDITMSG` / a temp file | absolute temp file | compared by content digest and argument position, never by path |
| `git_prefix` | `""` / `""` | `""` | compared equal |

`index_in_git_dir` and the message-file path are unavoidable
implementation/volatile differences of the private-index design. `GIT_EDITOR`
is a real, low-risk environment difference: a hook that reads `GIT_EDITOR` (or
tries to launch an editor) observes a different value on the ordinary route.
It is recorded here and in every hook fixture rather than normalised into
equality.

## 8. Does this close F10?

**F10 is closed for the Linux/SHA-1 scoped-safe-commit surface, and not
beyond it.** The former F10 gap was: no committed regeneration, no
input/output provenance or drift check, narrow coverage, and no independent
re-derivation. This work now provides:

- a committed generator that refuses a non-lock-exact interpreter, oracle
  drift, a missing baseline or modified reference sources;
- 32 checked-in cases with per-case SHA-256 digests, a generator digest and a
  recorded verifier report, all checked by the test;
- a Rust runner that re-executes the same setup and compares real oracle
  observations with real native observations (26 exact matches, 5 recorded
  divergences, 1 open mismatch);
- the older `safe_commit_differential.rs` frozen constants are now explicitly
  labelled as non-evidence; its eight fast native-only checks remain, while the
  required oracle comparisons live in the corpus suite;
- the previously missing coverage: `prepare-commit-msg`/`commit-msg` argument
  passing, rewriting and rejection; cleanup configuration and empty-message
  refusal; the real hook/editor environment shape; sparse, intent-to-add,
  unmerged and deletion index states; literal and unusual filenames; post-CAS
  interposition; `GIT_INDEX_FILE`; nested-hook history and outcome
  classification; typed signing/commit failures.

Not closed by F10 here: Windows/macOS, SHA-256 repositories, real submodule
staging, CLI wiring, and the `SafeCommitRecovery::repair_argv` path (no
resync-failure was induced; the landed-sha *target* is proved by the post-CAS
case). The corpus also does not exercise every removed repository selector,
only the `GIT_INDEX_FILE` one named by the review.

## 9. Remaining defects and disposition

- **No defect was found in the covered contracts.** The open item is the
  explicit `GIT_INDEX_FILE` refusal (§5), which is a recorded parity gap, not a
  silent divergence.
- The two reflog-source differences (§6.4/§6.5) and the ordinary-route signing
  difference (§6.1) are intentional/stronger behaviour against an older
  baseline; they are recorded so no one reads the corpus as blanket equality.
- The `GIT_EDITOR` hook-environment difference (§7) is the only environment
  finding that could surprise a hook author. A minimal repro is any
  `pre-commit` hook that logs `$GIT_EDITOR` on the ordinary route: `set::`
  under the baseline, unset under native.
- Overall safe-commit and CLI signoff is **not** claimed. This is a differential
  evidence artifact for the current source bytes; an independent reviewer still
  has to accept the remaining product decisions (selector support, hook
  environment, signed ordinary commits).

## 10. Reproduction

```sh
# regenerate the committed oracle evidence (development only)
PYTHONPATH="$PWD/src" SPEC_KITTY_PACKS_ROOT="$PWD/packs" \
  .venv-rust-oracle/bin/python \
  crates/sk-git/tests/fixtures/safe_commit/generate_oracle.py

# manifest/integrity + full differential
export PATH="$HOME/.cargo/bin:$PATH"
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 \
  cargo test -p sk-git --test safe_commit_conformance --locked -- --nocapture

# scoped gates
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p sk-git --locked
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 \
  cargo clippy -p sk-git --all-targets --locked -- -D warnings
cargo fmt -p sk-git -- --check
```

Each test invocation creates only temporary repositories, `HOME`, `XDG_*`,
`TMPDIR`, hook logs and competitor state under the test's temporary roots. No
probe runs in the repository checkout and no production Python is invoked by
the native binary.
