# Scoped-safe-commit differential corpus

Committed oracle evidence for `sk_git::GitRepository::safe_commit`, compared
against the pinned upstream Python baseline
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`.

- `manifest.json` — required case names, per-case file + SHA-256, generator
  digest, oracle-verification report, and the comparison normalisations.
- `PROVENANCE.json` — generation-time identity summary.
- `cases/*.json` — one complete observation per case (setup, request, before
  snapshot, oracle outcome, after snapshot, hook evidence).
- `generate_oracle.py` — the only writer of these files.

The Rust harness is
[`crates/sk-git/tests/safe_commit_conformance.rs`](../../../safe_commit_conformance.rs);
its shared runner/capture/comparison code is
[`crates/sk-git/tests/common/safe_commit_oracle.rs`](../../../common/safe_commit_oracle.rs).

## Regeneration

From the repository root, with the lock-exact oracle environment:

```sh
PYTHONPATH="$PWD/src" SPEC_KITTY_PACKS_ROOT="$PWD/packs" \
  .venv-rust-oracle/bin/python \
  crates/sk-git/tests/fixtures/safe_commit/generate_oracle.py
```

The generator refuses to run unless all of the following hold:

- the interpreter is the repository's `.venv-rust-oracle/bin/python` (the
  check uses `pyvenv.cfg`, not a resolved symlink);
- `scripts/rust/verify_oracle.py` reports `ok: true` with no drift/missing
  requirements;
- the four reference sources are byte-identical to the pinned baseline commit
  (`commit_helpers.py`, `commit_guard.py`, `ref_advance.py`,
  `protection_policy.py`);
- the baseline commit exists locally.

It deletes and rewrites `cases/*.json`, then writes `manifest.json` and
`PROVENANCE.json`, so a removed case cannot linger.

## What is captured

Both the Python generator and the Rust runner build the *same* scenario from
the committed `setup` block (files, shell/argv/write steps, hook scripts) and
apply the same request. For each side the snapshot records:

| Field | Detail |
| --- | --- |
| `branch`, `head`, `refs` | symbolic HEAD, tip, every ref |
| `status` | `git status --porcelain -z --untracked-files=all`, rename-aware |
| `index`, `index_flags` | `git ls-files -s -z` and `-v -z` (stage, mode, blob, skip-worktree, unmerged) |
| `files` | every non-`.git` file: path, SHA-256, small UTF-8 text |
| `message_sha256`, `message` | exact commit message bytes from `git cat-file commit` |
| `tree`, `parents`, `committed_paths` | commit-object identity and changed set |
| `reflog` | new commit id + reflog subject for the destination branch |
| `custom_index` | entries/flags of a caller-designated `GIT_INDEX_FILE` when the case uses one |
| `hooks` | per invocation: feature name, argument kinds (file content digest / literal text), candidate index identity and flags, cwd, `GIT_DIR`/`GIT_PREFIX`/`GIT_EDITOR` shape, the hook's `git status`/`git diff --cached` view |

Every fixture pins the branch, identity and author/committer dates, so commit
and tree ids are deterministic and are compared directly. Nothing depends on
wall-clock time, the process id, or a temporary path.

## Normalisations

Only these comparisons are normalised (also listed in `manifest.json`):

1. reflog entries compare as `(new commit id, reflog subject)`; committer
   identity and timestamp are not part of the observation;
2. errors compare as a canonical refusal class plus explicit detail tokens,
   never as a raw cross-language message string; the recorded oracle detail
   replaces the throw-away run root with `<RUN_ROOT>`;
3. hook arguments compare as `text:<value>` or `file:<content-sha256>`; the
   temporary message-file path itself is not compared;
4. hook fields `git_editor`, `git_prefix` and `index_in_git_dir` are recorded
   but excluded from equality for the candidate-index/editor invariants
   documented in `crates/sk-git/CONFORMANCE-REPORT.md` (the Rust harness asserts
   the native invariant instead of silently ignoring the field).

Nothing else is normalised. In particular, index entries, flags, worktree
bytes, commit message bytes, trees, parents, refs, reflog subjects, hook
argument content and the after-state are compared exactly.
