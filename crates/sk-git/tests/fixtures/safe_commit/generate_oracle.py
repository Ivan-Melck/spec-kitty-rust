#!/usr/bin/env python3
"""Generate the committed scoped-safe-commit differential corpus.

This is a development-time oracle generator, never a production dependency.
It drives the pinned upstream Python implementation
(`specify_cli.git.commit_helpers.safe_commit`, baseline
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`) inside a throw-away repository and
records the *complete observable transaction state* before and after the call:

* branch, HEAD, every ref, status, every index entry and index flag;
* every worktree file (path, digest, text when UTF-8);
* the exact commit message bytes, tree, parents and changed-path set;
* the destination ref's reflog (new sha + reflog subject);
* every hook invocation: argument kinds/content digests, the candidate-index
  identity, cwd, Git's environment shape and the index view the hook sees;
* the typed outcome (as far as Python has one), warnings, and, on the
  "landed, but" routes, the landed commit.

The same case file is executed by `tests/safe_commit_conformance.rs` against the
native `sk-git` implementation, and the before/after structures are compared.
Commit and tree ids are deterministic because every fixture pins the branch,
identity and author/committer dates, so they are compared directly rather than
normalised away.

Run from the repository root with the lock-exact interpreter:

    PYTHONPATH="$PWD/src" SPEC_KITTY_PACKS_ROOT="$PWD/packs" \
      .venv-rust-oracle/bin/python \
      crates/sk-git/tests/fixtures/safe_commit/generate_oracle.py

The generator refuses to run when the interpreter is not the lock-exact
`.venv-rust-oracle` one, when `scripts/rust/verify_oracle.py` reports drift,
when the pinned baseline commit is absent, or when the four reference sources
differ from that baseline.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parents[4]
CASES_DIR = HERE / "cases"
BASELINE = "9aed3a8012cb5eb22c0128b14d525039f95fea7a"
BASELINE_FILES = (
    "src/specify_cli/git/commit_helpers.py",
    "src/specify_cli/core/commit_guard.py",
    "src/specify_cli/git/ref_advance.py",
    "src/specify_cli/git/protection_policy.py",
)

FIXED_ENV = {
    "GIT_AUTHOR_DATE": "2020-01-01T00:00:00+0000",
    "GIT_COMMITTER_DATE": "2020-01-01T00:00:00+0000",
    "TZ": "UTC",
    "LC_ALL": "C",
    "LANG": "C",
    "GIT_CONFIG_NOSYSTEM": "1",
}

SELECTOR_ENV = (
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_NAMESPACE",
    "GIT_SHALLOW_FILE",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
)

# ---------------------------------------------------------------------------
# Shared hook scripts (stored verbatim in the fixtures and installed verbatim by
# the Rust harness, so the two sides cannot drift apart).
# ---------------------------------------------------------------------------

HOOK_PRELUDE = """#!/bin/sh
LOG="${SK_SAFE_COMMIT_HOOK_LOG:?}"
{
  echo "hook=%s"
  echo "nargs=$#"
  i=0
  for a in "$@"; do
    i=$((i+1))
    if [ -f "$a" ]; then
      echo "argv$i=file:$(git hash-object --no-filters -- "$a")"
    else
      echo "argv$i=text:$a"
    fi
  done
  if [ -n "${GIT_INDEX_FILE-}" ]; then
    real="$(git rev-parse --absolute-git-dir)/index"
    if [ "$GIT_INDEX_FILE" = "$real" ]; then echo "index=real"; else echo "index=private"; fi
    case "$GIT_INDEX_FILE" in
      "$(git rev-parse --absolute-git-dir)"/*) echo "index_in_git_dir=yes";;
      *) echo "index_in_git_dir=no";;
    esac
  else
    echo "index=unset"
    echo "index_in_git_dir=no"
  fi
  if [ "$(pwd -P)" = "$(git rev-parse --show-toplevel)" ]; then
    echo "cwd_is_worktree=yes"
  else
    echo "cwd_is_worktree=no"
  fi
  echo "git_dir=${GIT_DIR-<unset>}"
  echo "git_prefix=${GIT_PREFIX-<unset>}"
  if [ -n "${GIT_EDITOR-}" ]; then echo "git_editor=set:$GIT_EDITOR"; else echo "git_editor=unset"; fi
  git status --porcelain | sed 's/^/status=/'
  git diff --cached --name-only | sed 's/^/cached=/'
} >> "$LOG"
"""


def hook(name: str, body: str = "") -> dict[str, str]:
    return {"name": name, "script": HOOK_PRELUDE % name + body}


def logging_hook(name: str) -> dict[str, str]:
    return hook(name, "exit 0\n")


POST_CAS_BODY = r"""
# Deterministic post-CAS interposition: a background helper waits until our
# compare-and-swap has moved the branch off the pinned parent, then lands a
# competitor commit on top of ours and signals completion.
if [ -n "${SK_POST_CAS_SENTINEL-}" ]; then
  (
    branch="$(git symbolic-ref --short -q HEAD)"
    parent="$(git rev-parse HEAD)"
    n=0
    while [ "$n" -lt 15000 ]; do
      if [ -f "${SK_POST_CAS_FAILED-}" ]; then exit 1; fi
      cur="$(git rev-parse "refs/heads/$branch" 2>/dev/null || true)"
      if [ -n "$cur" ] && [ "$cur" != "$parent" ]; then
        export GIT_INDEX_FILE="${SK_POST_CAS_INDEX:?}"
        git read-tree "$cur"
        blob=$(printf 'competitor\n' | git hash-object -w --stdin)
        git update-index --add --cacheinfo 100644 "$blob" p.txt
        tree=$(git write-tree)
        c2=$(git commit-tree "$tree" -p "$cur" -m competitor)
        git update-ref "refs/heads/$branch" "$c2" "$cur"
        : > "${SK_POST_CAS_SENTINEL:?}"
        exit 0
      fi
      n=$((n+1))
      sleep 0.002 2>/dev/null || sleep 1
    done
    : > "${SK_POST_CAS_FAILED:?}"
  ) >/dev/null 2>&1 &
fi
exit 0
"""


INIT_FILES = {
    "seed.txt": "seed\n",
    "a.txt": "one\n",
    "other.txt": "base\n",
    "partial.txt": "p0\np1\np2\n",
}
INIT_STEPS = [
    {"argv": ["git", "add", "-A"]},
    {"argv": ["git", "commit", "-q", "-m", "initial"]},
]


def case(
    name: str,
    description: str,
    route: str,
    request: dict,
    *,
    files: dict[str, str] | None = None,
    steps: list[dict] | None = None,
    hooks: list[dict] | None = None,
    expectation: str = "match",
    divergence: dict | None = None,
    native_variant: str | None = None,
    native_landed_kind: str | None = None,
    detail_tokens: list[str] | None = None,
    branch: str = "lane",
) -> dict:
    return {
        "name": name,
        "description": description,
        "route": route,
        "expectation": expectation,
        "divergence": divergence,
        "native": {"variant": native_variant, "landed_kind": native_landed_kind},
        "detail_tokens": detail_tokens or [],
        "setup": {
            "branch": branch,
            "files": dict(files) if files is not None else dict(INIT_FILES),
            "steps": list(INIT_STEPS) + list(steps or []),
            "hooks": list(hooks or []),
        },
        "request": request,
    }


def req(
    message: str,
    paths: list[str],
    *,
    deletions: list[str] | None = None,
    expected_parent: str | None = None,
    custom_index: bool = False,
    wait_for_sentinel: bool = False,
    protection: list[str] | None = None,
    capability: str = "standard",
) -> dict:
    return {
        "message": message,
        "paths": paths,
        "index_deletions": deletions or [],
        "expected_parent": expected_parent,
        "custom_index": custom_index,
        "wait_for_sentinel": wait_for_sentinel,
        "protection": protection,
        "capability": capability,
    }


def unusual_name_steps() -> list[dict]:
    return [{"write": {"path": name, "content": "v1\n"}} for name in UNUSUAL_NAMES]


UNUSUAL_NAMES = [
    "space name.txt",
    "-x.txt",
    "tab\tname.txt",
    "line\nbreak.txt",
    "colon:name.txt",
    'quote"name.txt',
    "hash#name.txt",
    "semi;name.txt",
    "dollar$name.txt",
    "unicode-\u00e9.txt",
]


def build_cases() -> list[dict]:
    cases: list[dict] = []

    cases.append(
        case(
            "preserve_unrelated_staged_partial_untracked",
            "Unrelated fully staged, partially staged and untracked content survives.",
            "worktree",
            req("diff: two", ["a.txt", "new.txt"]),
            steps=[
                {"write": {"path": "a.txt", "content": "two\n"}},
                {"write": {"path": "other.txt", "content": "changed\n"}},
                {"argv": ["git", "add", "other.txt"]},
                {"write": {"path": "partial.txt", "content": "p0\np1-WIP\np2\n"}},
                {"argv": ["git", "add", "partial.txt"]},
                {"write": {"path": "partial.txt", "content": "p0\np1-WIP\np2-WIP\n"}},
                {"write": {"path": "u.txt", "content": "u\n"}},
                {"write": {"path": "new.txt", "content": "new\n"}},
            ],
        )
    )
    cases.append(
        case(
            "rename_pair_as_delete_plus_add",
            "A git-mv rename pair is committed as delete plus add and leaves a clean index.",
            "worktree",
            req("diff: rename", ["old.txt", "new.txt"]),
            files={**INIT_FILES, "old.txt": "old content\n"},
            steps=[{"argv": ["git", "mv", "old.txt", "new.txt"]}],
        )
    )
    cases.append(
        case(
            "noop_runs_hooks_before_classification",
            "A no-op request still runs the commit hooks before the empty-changeset refusal.",
            "worktree",
            req("diff: noop", ["a.txt"]),
            hooks=[logging_hook("pre-commit")],
            native_variant="SafeCommitNothingToCommit",
        )
    )
    cases.append(
        case(
            "hook_formatter_makes_candidate_nonempty",
            "An initially empty candidate runs pre-commit but leaves its reformat uncommitted, matching the baseline.",
            "worktree",
            req("diff: formatted", ["a.txt"]),
            hooks=[
                hook(
                    "pre-commit",
                    "printf 'formatted\\n' > a.txt\n"
                    "git add --force -- a.txt\n"
                    "exit 0\n",
                )
            ],
            native_variant="SafeCommitNothingToCommit",
        )
    )
    cases.append(
        case(
            "prepare_and_commit_msg_rewrite",
            "prepare-commit-msg receives (file, message), commit-msg receives (file), and both rewrites reach the commit after cleanup.",
            "worktree",
            req("subject\n\nbody", ["a.txt"]),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
            hooks=[
                hook(
                    "prepare-commit-msg",
                    "printf '\\nPrepared-by: hook   \\n\\n\\n' >> \"$1\"\n"
                    "exit 0\n",
                ),
                hook(
                    "commit-msg",
                    "printf '\\nSigned-off-by: hook\\n' >> \"$1\"\n"
                    "exit 0\n",
                ),
            ],
        )
    )
    cases.append(
        case(
            "commit_msg_rejection_refused",
            "A rejecting commit-msg hook refuses the commit and leaves the operator index as found.",
            "worktree",
            req("reject me", ["a.txt"]),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
            hooks=[
                hook("commit-msg", "echo 'commit-msg boom' >&2\nexit 1\n"),
            ],
            native_variant="SafeCommitHookRejected",
            detail_tokens=["commit-msg boom"],
        )
    )
    cases.append(
        case(
            "pre_commit_rejection_usage_phrase_not_environment",
            "A rejecting hook whose stderr contains a git usage phrase stays a hook rejection, never an unsupported-environment error.",
            "worktree",
            req("diff: blocked", ["a.txt"]),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
            hooks=[
                hook("pre-commit", "echo 'unknown command: rustfmt' >&2\nexit 1\n"),
            ],
            native_variant="SafeCommitHookRejected",
            detail_tokens=["unknown command: rustfmt"],
        )
    )
    cases.append(
        case(
            "message_cleanup_whitespace",
            "The ordinary route applies git's configured whitespace cleanup to the commit message.",
            "worktree",
            req("subject   \n\n\n\nbody\t \n", ["a.txt"]),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
        )
    )
    cases.append(
        case(
            "message_cleanup_strip_config",
            "commit.cleanup=strip removes comment lines from the ordinary route's message.",
            "worktree",
            req("# c1\nsubject\n\n# c2\nbody\n", ["a.txt"]),
            steps=[
                {"argv": ["git", "config", "commit.cleanup", "strip"]},
                {"write": {"path": "a.txt", "content": "two\n"}},
            ],
        )
    )
    cases.append(
        case(
            "message_cleanup_verbatim_config",
            "commit.cleanup=verbatim preserves the ordinary route's message bytes exactly.",
            "worktree",
            req("subject   \n\n\n\nbody\t \n", ["a.txt"]),
            steps=[
                {"argv": ["git", "config", "commit.cleanup", "verbatim"]},
                {"write": {"path": "a.txt", "content": "two\n"}},
            ],
        )
    )
    cases.append(
        case(
            "whitespace_only_message_refused",
            "A message that cleans to empty is refused and no commit lands.",
            "worktree",
            req("   \n", ["a.txt"]),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
            native_variant="SafeCommitCommitFailed",
            detail_tokens=["empty commit message"],
        )
    )
    cases.append(
        case(
            "pinned_message_verbatim",
            "The pinned-parent route keeps the caller's message verbatim, including trailing whitespace and blank lines.",
            "pinned",
            req("subject   \n\n\n\nbody\t\n", ["a.txt"], expected_parent="head"),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
        )
    )
    cases.append(
        case(
            "pinned_commit_msg_rewrite_reflog_source",
            "Pinned route: hook-rewritten commit bytes and the requested reflog message match the baseline.",
            "pinned",
            req("subject\n\nbody", ["a.txt"], expected_parent="head"),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
            hooks=[
                hook("commit-msg", "printf '\\nSigned-off-by: hook\\n' >> \"$1\"\nexit 0\n"),
            ],
        )
    )
    cases.append(
        case(
            "pinned_signing_failure_typed",
            "A configured signing backend that fails refuses the pinned route before the ref moves.",
            "pinned",
            req("signed", ["a.txt"], expected_parent="head"),
            steps=[
                {"write": {"path": "a.txt", "content": "two\n"}},
                {
                    "shell": (
                        "printf '#!/bin/sh\\necho \"signer unavailable\" >&2\\nexit 1\\n' "
                        '> "$SK_RUN_ROOT/fail-gpg"\n'
                        'chmod +x "$SK_RUN_ROOT/fail-gpg"\n'
                        'git config commit.gpgsign true\n'
                        'git config gpg.program "$SK_RUN_ROOT/fail-gpg"\n'
                    )
                },
            ],
            native_variant="SafeCommitCommitFailed",
            detail_tokens=["signer unavailable"],
        )
    )
    cases.append(
        case(
            "worktree_signing_enforced_divergence",
            "Ordinary route explicitly forces signing off, matching the baseline even with a failing configured signer.",
            "worktree",
            req("signed", ["a.txt"]),
            steps=[
                {"write": {"path": "a.txt", "content": "two\n"}},
                {
                    "shell": (
                        "printf '#!/bin/sh\\necho \"signer unavailable\" >&2\\nexit 1\\n' "
                        '> "$SK_RUN_ROOT/fail-gpg"\n'
                        'chmod +x "$SK_RUN_ROOT/fail-gpg"\n'
                        'git config commit.gpgsign true\n'
                        'git config gpg.program "$SK_RUN_ROOT/fail-gpg"\n'
                    )
                },
            ],
        )
    )
    cases.append(
        case(
            "git_index_file_selector_honoured",
            "A caller-supplied GIT_INDEX_FILE is captured at open: the commit reconciles that index and leaves .git/index untouched.",
            "worktree",
            req("env index", ["a.txt"], custom_index=True),
            steps=[
                {"write": {"path": "real-unrelated.txt", "content": "r\n"}},
                {"argv": ["git", "add", "real-unrelated.txt"]},
                {"write": {"path": "unrelated.txt", "content": "u\n"}},
                {"argv": ["git", "read-tree", "HEAD"], "custom_index": True},
                {"argv": ["git", "add", "unrelated.txt"], "custom_index": True},
                {"write": {"path": "a.txt", "content": "two\n"}},
            ],
        )
    )
    cases.append(
        case(
            "index_only_deletion_keeps_file",
            "An index-only deletion is committed from the private index and the file stays on disk.",
            "index_deletions",
            req(
                "diff: index deletion",
                ["a.txt"],
                deletions=["c.md"],
            ),
            files={**INIT_FILES, "c.md": "c.md\n", "d.md": "d.md\n"},
            steps=[
                {"write": {"path": "other.txt", "content": "operator staged\n"}},
                {"argv": ["git", "add", "other.txt"]},
                {"write": {"path": "d.md", "content": "d.md\noperator unstaged\n"}},
            ],
        )
    )
    cases.append(
        case(
            "worktree_deletion_committed",
            "A tracked file removed from the worktree is committed as a deletion.",
            "worktree",
            req("diff: delete", ["gone.txt"]),
            files={**INIT_FILES, "gone.txt": "gone\n"},
            steps=[{"shell": "rm gone.txt\n"}],
        )
    )
    cases.append(
        case(
            "literal_unusual_filenames",
            "Spaces, leading dashes, tabs, newlines, colons, quotes, hashes and Unicode in paths are data.",
            "worktree",
            req("diff: unusual", list(UNUSUAL_NAMES)),
            files={**INIT_FILES, **{name: "v0\n" for name in UNUSUAL_NAMES}},
            steps=unusual_name_steps(),
        )
    )
    cases.append(
        case(
            "pathspec_magic_literal_name",
            "A filename that looks like pathspec magic: baseline expands it, native commits the literal path.",
            "worktree",
            req("magic", [":(glob)*.txt"]),
            files={**INIT_FILES, "b.txt": "b\n"},
            steps=[
                {"write": {"path": ":(glob)*.txt", "content": "weird\n"}},
                {"write": {"path": "a.txt", "content": "two\n"}},
                {"write": {"path": "b.txt", "content": "b2\n"}},
                {"write": {"path": "other.txt", "content": "o2\n"}},
            ],
            expectation="documented_divergence",
            divergence={
                "kind": "literal_pathspecs",
                "detail": (
                    "Baseline stages/commits the pathspec expansion (a.txt, b.txt, the literal "
                    "name), then reports the extra committed paths after landing. Native passes "
                    "--literal-pathspecs, so only the literal file is committed. Native is "
                    "narrower and safer; recorded, not claimed as baseline equality."
                ),
            },
        )
    )
    sparse_files = {
        **INIT_FILES,
        "cone/keep.txt": "k\n",
        "outside/out.txt": "o\n",
    }
    sparse_steps = [
        {"argv": ["git", "sparse-checkout", "init", "--cone"]},
        {"argv": ["git", "sparse-checkout", "set", "cone"]},
    ]
    cases.append(
        case(
            "sparse_in_cone_preserves_skip_worktree",
            "An in-cone commit lands and the out-of-cone skip-worktree entry survives.",
            "worktree",
            req("in cone", ["cone/keep.txt"]),
            files=sparse_files,
            steps=[*sparse_steps, {"write": {"path": "cone/keep.txt", "content": "k2\n"}}],
        )
    )
    cases.append(
        case(
            "sparse_out_of_cone_refused",
            "An out-of-cone request fails closed before any index entry changes.",
            "worktree",
            req("out cone", ["outside/out.txt"]),
            files=sparse_files,
            steps=[
                *sparse_steps,
                {"write": {"path": "cone/keep.txt", "content": "k2\n"}},
                {"write": {"path": "a.txt", "content": "two\n"}},
            ],
            native_variant="SafeCommitStageFailed",
            detail_tokens=["outside of your sparse-checkout definition"],
        )
    )
    cases.append(
        case(
            "intent_to_add_and_staged_deletion_preserved",
            "Unrelated intent-to-add, staged deletion and partially staged content survive byte-for-byte.",
            "worktree",
            req("ita", ["a.txt"]),
            files={**INIT_FILES, "drop.txt": "drop\n"},
            steps=[
                {"write": {"path": "a.txt", "content": "two\n"}},
                {"write": {"path": "other.txt", "content": "changed\n"}},
                {"argv": ["git", "add", "other.txt"]},
                {"write": {"path": "partial.txt", "content": "p0\np1-WIP\np2\n"}},
                {"argv": ["git", "add", "partial.txt"]},
                {"write": {"path": "partial.txt", "content": "p0\np1-WIP\np2-WIP\n"}},
                {"argv": ["git", "rm", "--cached", "-q", "drop.txt"]},
                {"write": {"path": "fresh.txt", "content": "fresh\n"}},
                {"argv": ["git", "add", "-N", "fresh.txt"]},
            ],
        )
    )
    cases.append(
        case(
            "unmerged_entries_preserved",
            "Three-stage unmerged entries without a *_HEAD marker are preserved while another path commits.",
            "worktree",
            req("conflict keep", ["a.txt"]),
            files={**INIT_FILES, "conf.txt": "c\n"},
            steps=[
                {
                    "shell": (
                        "b1=$(printf 'base\\n' | git hash-object -w --stdin)\n"
                        "b2=$(printf 'ours\\n' | git hash-object -w --stdin)\n"
                        "b3=$(printf 'theirs\\n' | git hash-object -w --stdin)\n"
                        "printf '100644 %s 1\\tconf.txt\\n100644 %s 2\\tconf.txt\\n"
                        "100644 %s 3\\tconf.txt\\n' \"$b1\" \"$b2\" \"$b3\" "
                        "| git update-index --index-info\n"
                    )
                },
                {"write": {"path": "a.txt", "content": "two\n"}},
            ],
        )
    )
    cases.append(
        case(
            "nested_hook_advance_conflict",
            "A commit-msg/pre-commit hook that creates its own nested commit moves the branch; the outer transaction reports an unconfirmed advance.",
            "pinned",
            req("outer", ["a.txt"], expected_parent="head"),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
            hooks=[
                hook(
                    "pre-commit",
                    "git commit --no-verify -q -m 'nested from hook'\nexit 0\n",
                )
            ],
            native_variant="SafeCommitAdvanceUnconfirmed",
        )
    )
    cases.append(
        case(
            "post_cas_competitor_reconciled_to_landed_commit",
            "A competitor that lands on top of our commit after the CAS cannot redirect the index reconciliation away from the landed commit.",
            "pinned",
            req("interpose", ["p.txt"], expected_parent="head", wait_for_sentinel=True),
            files={**INIT_FILES, "p.txt": "ours\n"},
            steps=[{"write": {"path": "p.txt", "content": "ours-v2\n"}}],
            hooks=[hook("pre-commit", POST_CAS_BODY)],
        )
    )
    cases.append(
        case(
            "ordinary_soft_wrapped_reflog_source",
            "Ordinary route reflog uses the first message line, matching git's copy_reflog_msg.",
            "worktree",
            req("subject\nbody", ["a.txt"]),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
        )
    )
    cases.append(
        case(
            "protected_branch_refused",
            "A protected destination without a bookkeeping capability is refused before mutation.",
            "worktree",
            req("gated", ["gated.txt"], protection=["main", "master"]),
            branch="main",
            steps=[{"write": {"path": "gated.txt", "content": "gated\n"}}],
            native_variant="SafeCommitProtectedBranch",
        )
    )
    cases.append(
        case(
            "directory_argument_refused",
            "A requested directory is refused before staging.",
            "worktree",
            req("diff: dir", ["sub"]),
            files={**INIT_FILES, "sub/one.txt": "one\n", "sub/two.txt": "two\n"},
            native_variant="SafeCommitDirectoryArgument",
        )
    )
    cases.append(
        case(
            "dot_relative_path_committed",
            "A ./relative spelling is normalized and committed.",
            "worktree",
            req("dot relative", ["./a.txt"]),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
        )
    )
    cases.append(
        case(
            "hook_stages_extra_paths_landed",
            "A hook that stages a tracked extra lands the commit and reports the unrequested paths as a landed follow-up.",
            "worktree",
            req("diff: sweep", ["spec.md"]),
            steps=[
                {"write": {"path": "other.txt", "content": "operator modified\n"}},
                {"write": {"path": "spec.md", "content": "# spec\n"}},
            ],
            hooks=[hook("pre-commit", "git add -u\nexit 0\n")],
            native_variant="SafeCommitLanded",
            native_landed_kind="UnrequestedPaths",
        )
    )
    cases.append(
        case(
            "pinned_hook_changes_requested_content",
            "Pinned route: a hook that changes requested content between staging and re-isolation is refused.",
            "pinned",
            req("outer", ["a.txt"], expected_parent="head"),
            steps=[{"write": {"path": "a.txt", "content": "two\n"}}],
            hooks=[hook("pre-commit", "printf 'hook changed\\n' > a.txt\nexit 0\n")],
            native_variant="SafeCommitHookChangedPaths",
        )
    )

    return cases


# ---------------------------------------------------------------------------
# Execution
# ---------------------------------------------------------------------------


def run(argv, cwd, env):
    return subprocess.run(
        list(argv),
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        errors="replace",
        check=False,
    )


def require_ok(result, what):
    if result.returncode != 0:
        raise SystemExit(
            f"{what} failed ({result.returncode}):\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def git_text(repo, args, env):
    return require_ok(run(["git", *args], repo, env), f"git {' '.join(args)}").stdout


def git_bytes(repo, args, env):
    result = subprocess.run(
        ["git", *args],
        cwd=repo,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"git {' '.join(args)} failed: {result.stderr.decode('utf-8', 'replace')}"
        )
    return result.stdout


def split_z(raw: bytes) -> list[bytes]:
    return [record for record in raw.split(b"\x00") if record]


def status_entries(repo, env) -> list[str]:
    raw = git_bytes(
        repo,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        env,
    )
    records = split_z(raw)
    entries: list[str] = []
    index = 0
    while index < len(records):
        record = records[index]
        code = record[:2].decode("utf-8", "surrogateescape")
        path = record[3:].decode("utf-8", "surrogateescape")
        if code[:1] in ("R", "C") and index + 1 < len(records):
            old = records[index + 1].decode("utf-8", "surrogateescape")
            entries.append(f"{code} {path} <- {old}")
            index += 2
            continue
        entries.append(f"{code} {path}")
        index += 1
    return entries


def index_entries(repo, env) -> list[str]:
    raw = git_bytes(repo, ["ls-files", "-s", "-z"], env)
    return [
        record.replace(b"\t", b" ", 1).decode("utf-8", "surrogateescape")
        for record in split_z(raw)
    ]


def index_flags(repo, env) -> list[str]:
    raw = git_bytes(repo, ["ls-files", "-v", "-z"], env)
    return [record.decode("utf-8", "surrogateescape") for record in split_z(raw)]


def worktree_files(repo: Path) -> list[dict]:
    entries: list[dict] = []
    for path in sorted(repo.rglob("*"), key=lambda value: str(value)):
        relative = path.relative_to(repo)
        if ".git" in relative.parts or not path.is_file():
            continue
        data = path.read_bytes()
        entry: dict = {
            "path": str(relative),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            pass
        else:
            if len(data) <= 8192:
                entry["text"] = text
        entries.append(entry)
    return entries


def reflog_entries(repo, branch, env) -> list[str]:
    raw = git_text(
        repo,
        ["reflog", "show", "--format=%H%x00%gs", f"refs/heads/{branch}"],
        env,
    )
    entries: list[str] = []
    for line in raw.splitlines():
        if not line:
            continue
        sha, _, subject = line.partition("\x00")
        entries.append(f"{sha}|{subject}")
    return entries


def commit_message(repo, sha, env):
    raw = git_bytes(repo, ["cat-file", "commit", sha], env)
    _, _, message = raw.partition(b"\n\n")
    sha256 = hashlib.sha256(message).hexdigest()
    try:
        text = message.decode("utf-8")
    except UnicodeDecodeError:
        text = None
    return sha256, text


def snapshot(repo: Path, branch: str | None, env: dict, custom_index: Path | None) -> dict:
    base = {key: value for key, value in env.items() if key not in SELECTOR_ENV}
    head = git_text(repo, ["rev-parse", "HEAD"], base).strip()
    symbolic = run(["git", "symbolic-ref", "--short", "-q", "HEAD"], repo, base)
    current = symbolic.stdout.strip() if symbolic.returncode == 0 else None
    refs = sorted(
        line
        for line in git_text(repo, ["for-each-ref", "--format=%(refname) %(objectname)"], base).splitlines()
        if line
    )
    raw_parents = git_text(repo, ["rev-list", "--parents", "-n", "1", head], base).split()
    parents = raw_parents[1:]
    tree = git_text(repo, ["rev-parse", f"{head}^{{tree}}"], base).strip()
    committed = sorted(
        record.decode("utf-8", "surrogateescape")
        for record in split_z(
            git_bytes(
                repo,
                ["show", "--name-only", "--format=", "-z", "--no-renames", head],
                base,
            )
        )
    )
    message_sha, message_text = commit_message(repo, head, base)
    entry = {
        "branch": current,
        "head": head,
        "refs": refs,
        "status": status_entries(repo, base),
        "index": index_entries(repo, base),
        "index_flags": index_flags(repo, base),
        "files": worktree_files(repo),
        "message_sha256": message_sha,
        "message": message_text,
        "tree": tree,
        "parents": parents,
        "committed_paths": committed,
        "reflog": reflog_entries(repo, current or branch or "lane", base),
        "custom_index": None,
    }
    if custom_index is not None:
        custom_env = {**base, "GIT_INDEX_FILE": str(custom_index)}
        exists = custom_index.exists()
        entry["custom_index"] = {
            "exists": exists,
            "entries": index_entries(repo, custom_env) if exists else [],
            "index_flags": index_flags(repo, custom_env) if exists else [],
        }
    return entry


def parse_hook_log(path: Path) -> list[dict]:
    if not path.exists():
        return []
    hooks: list[dict] = []
    current: dict | None = None
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("hook="):
            current = {
                "hook": line[len("hook="):],
                "nargs": 0,
                "argv": [],
                "index": "unset",
                "index_in_git_dir": "no",
                "cwd_is_worktree": "no",
                "git_dir": "<unset>",
                "git_prefix": "",
                "git_editor": "unset",
                "status": [],
                "cached": [],
                "extra": [],
            }
            hooks.append(current)
            continue
        if current is None:
            continue
        if line.startswith("argv") and "=" in line:
            _, _, value = line.partition("=")
            current["argv"].append(value)
        elif line.startswith("status="):
            current["status"].append(line[len("status="):])
        elif line.startswith("cached="):
            current["cached"].append(line[len("cached="):])
        elif "=" in line:
            key, _, value = line.partition("=")
            if key == "nargs":
                current["nargs"] = int(value)
            elif key in current:
                current[key] = value
            else:
                current["extra"].append(line)
        else:
            current["extra"].append(line)
    return hooks


class WarningCollector(logging.Handler):
    def __init__(self):
        super().__init__(level=logging.WARNING)
        self.messages: list[str] = []

    def emit(self, record):
        self.messages.append(record.getMessage())


def warning_tokens(messages: list[str]) -> list[str]:
    tokens = set()
    for message in messages:
        if "produced warnings on a successful commit: " in message:
            tokens.add(
                "hook_stderr:"
                + message.split("produced warnings on a successful commit: ", 1)[1].strip()
            )
        elif "post-commit hook failed after" in message or "post-commit hook could not run" in message:
            tokens.add("post_commit_failed")
        else:
            tokens.add("warning:" + message)
    return sorted(tokens)


def applied_steps(repo: Path, steps: list[dict], base_env: dict, custom_index: Path) -> None:
    for step in steps:
        env = {key: value for key, value in base_env.items() if key not in SELECTOR_ENV}
        if step.get("custom_index"):
            env["GIT_INDEX_FILE"] = str(custom_index)
        if "write" in step:
            target = repo / step["write"]["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(step["write"]["content"].encode("utf-8"))
        elif "argv" in step:
            require_ok(run(step["argv"], repo, env), f"setup step {step['argv']!r}")
        elif "shell" in step:
            require_ok(run(["sh", "-eu", "-c", step["shell"]], repo, env), "setup shell step")
        else:
            raise SystemExit(f"unknown step: {step!r}")


def oracle_commit_attr(error) -> str | None:
    return getattr(error, "commit_sha", None)


def classify(error, case_spec, warnings) -> dict:
    tokens = case_spec["detail_tokens"]
    if error is None:
        raise AssertionError("classify() expects an error")
    kind = type(error).__name__
    text = str(error)
    facts = [f"detail:{token}" for token in tokens if token in text]
    commit = None
    landed_kind = None
    if kind in ("SafeCommitStagedTreeUnchanged",):
        klass = "no_change"
    elif kind in ("SafeCommitDirectoryArgument",):
        klass = "directory_argument"
    elif kind in ("SafeCommitPathPolicyError",):
        klass = "path_policy"
    elif kind in ("SafeCommitPathLoopRefused",):
        klass = "path_loop"
    elif kind in ("ProtectedBranchRefused",):
        klass = "protected_branch"
    elif kind in ("SafeCommitHeadMismatch",):
        klass = "head_mismatch"
    elif kind in ("SafeCommitDestinationNotFound",):
        klass = "destination_missing"
    elif kind in ("SafeCommitEmptyChangeset",):
        klass = "empty_changeset"
    elif kind in ("SafeCommitIndexDeletionConflict",):
        klass = "index_deletion_conflict"
    elif kind in ("SafeCommitIndexResidue",):
        klass = "landed_with_followup"
        landed_kind = "IndexResidue"
        commit = oracle_commit_attr(error)
    elif kind in ("SafeCommitUnrequestedPaths",):
        klass = "landed_with_followup"
        landed_kind = "UnrequestedPaths"
        commit = oracle_commit_attr(error)
    elif kind in ("SafeCommitRecoveryFailed",):
        klass = "landed_with_followup"
        landed_kind = "RecoveryFailed"
        commit = oracle_commit_attr(error)
    elif kind == "RuntimeError":
        lowered = text.lower()
        if "could not create expected-parent commit" in text and "gpg failed to sign" in text:
            klass = "signing_failure"
        elif "empty commit message" in lowered:
            klass = "empty_message"
        elif "hook rejected expected-parent commit" in text:
            klass = "hook_rejection"
        elif "changed requested path contents" in text:
            klass = "hook_changed_paths"
        elif "failed to stage requested files" in text:
            klass = "stage_failure"
        elif "conditional advance of target" in text or "compare-and-swap" in lowered:
            klass = "advance_conflict"
        elif "git commit failed" in text and any(
            token in text for token in case_spec["detail_tokens"]
        ):
            klass = "hook_rejection"
        elif "git commit failed" in text:
            klass = "commit_failed"
        else:
            klass = "other_runtime_error"
    else:
        klass = f"other:{kind}"
    return {
        "outcome": "error",
        "refusal_class": klass,
        "facts": facts,
        "commit": commit,
        "landed_kind": landed_kind,
        "warnings": warning_tokens(warnings),
        "error_detail": text,
    }


def run_case(case_spec: dict, run_root: Path, verifier: dict, git_version: str) -> dict:
    from specify_cli.git.commit_helpers import safe_commit

    repo = run_root / "repo"
    repo.mkdir(parents=True)
    custom_index = run_root / "custom-index"
    hook_log = run_root / "hooks.log"
    for directory in ("home", "tmp", "xdg-config", "xdg-cache", "xdg-data"):
        (run_root / directory).mkdir()

    base_env = {
        key: value
        for key, value in os.environ.items()
        if key not in SELECTOR_ENV
        and key not in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME")
        and not key.startswith("SK_")
    }
    base_env.update(FIXED_ENV)
    base_env.update(
        {
            "HOME": str(run_root / "home"),
            "TMPDIR": str(run_root / "tmp"),
            "XDG_CONFIG_HOME": str(run_root / "xdg-config"),
            "XDG_CACHE_HOME": str(run_root / "xdg-cache"),
            "XDG_DATA_HOME": str(run_root / "xdg-data"),
            "SK_RUN_ROOT": str(run_root),
            "SK_CUSTOM_INDEX": str(custom_index),
            "SK_SAFE_COMMIT_HOOK_LOG": str(hook_log),
            "SK_POST_CAS_INDEX": str(run_root / "competitor-index"),
            "SK_POST_CAS_SENTINEL": str(run_root / "competitor.done"),
            "SK_POST_CAS_FAILED": str(run_root / "competitor.failed"),
        }
    )
    setup_env = {key: value for key, value in base_env.items() if key not in SELECTOR_ENV}

    branch = case_spec["setup"]["branch"]
    require_ok(run(["git", "init", "-q", "-b", branch], repo, setup_env), "git init")
    require_ok(run(["git", "config", "user.name", "Test User"], repo, setup_env), "git config")
    require_ok(
        run(["git", "config", "user.email", "test@example.com"], repo, setup_env),
        "git config",
    )
    for path, content in case_spec["setup"]["files"].items():
        target = repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content.encode("utf-8"))
    applied_steps(repo, case_spec["setup"]["steps"], base_env, custom_index)

    if case_spec["setup"]["hooks"]:
        hooks_dir = repo / ".git" / "hooks"
        hooks_dir.mkdir(parents=True, exist_ok=True)
        for entry in case_spec["setup"]["hooks"]:
            path = hooks_dir / entry["name"]
            path.write_text(entry["script"], encoding="utf-8")
            path.chmod(0o755)

    before = snapshot(repo, branch, base_env, custom_index if case_spec["request"]["custom_index"] else None)

    request = case_spec["request"]
    call_env = {key: value for key, value in base_env.items() if key not in SELECTOR_ENV}
    if request["custom_index"]:
        call_env["GIT_INDEX_FILE"] = str(custom_index)

    logging.root.setLevel(logging.WARNING)
    collector = WarningCollector()
    logging.root.addHandler(collector)
    result = None
    error = None
    previous_env = dict(os.environ)
    try:
        os.environ.clear()
        os.environ.update(call_env)
        kwargs = {
            "repo_root": repo,
            "worktree_root": repo,
            "destination_ref": branch,
            "message": request["message"],
            "paths": tuple(Path(path) for path in request["paths"]),
            "index_deletions": tuple(Path(path) for path in request["index_deletions"]),
        }
        if request["expected_parent"] == "head":
            kwargs["expected_parent_sha"] = git_text(repo, ["rev-parse", "HEAD"], setup_env).strip()
        if request["protection"]:
            from specify_cli.core.commit_guard import GuardCapability

            kwargs["capability"] = GuardCapability.STANDARD
        try:
            result = safe_commit(**kwargs)
        except Exception as exc:  # noqa: BLE001 - the oracle's typed refusals
            error = exc
    finally:
        os.environ.clear()
        os.environ.update(previous_env)
        logging.root.removeHandler(collector)

    if request["wait_for_sentinel"]:
        deadline = 30.0
        waited = 0.0
        sentinel = run_root / "competitor.done"
        failed = run_root / "competitor.failed"
        while not sentinel.exists() and not failed.exists() and waited < deadline:
            import time

            time.sleep(0.01)
            waited += 0.01
        if not sentinel.exists():
            raise SystemExit(
                f"{case_spec['name']}: post-CAS helper did not signal completion "
                f"(failed={failed.exists()}, log={hook_log})"
            )

    after = snapshot(repo, branch, base_env, custom_index if request["custom_index"] else None)
    hooks = parse_hook_log(hook_log)

    if error is None:
        oracle = {
            "outcome": "ok",
            "refusal_class": None,
            "facts": [],
            "commit": result.sha,
            "landed_kind": None,
            "warnings": warning_tokens(collector.messages),
            "error_detail": None,
        }
    else:
        oracle = classify(error, case_spec, collector.messages)
        if oracle["error_detail"]:
            oracle["error_detail"] = oracle["error_detail"].replace(
                str(run_root), "<RUN_ROOT>"
            )

    return {
        **{key: case_spec[key] for key in (
            "name",
            "description",
            "route",
            "expectation",
            "divergence",
            "native",
            "detail_tokens",
        )},
        "setup": {
            "branch": case_spec["setup"]["branch"],
            "files": [
                {"path": path, "content": content}
                for path, content in case_spec["setup"]["files"].items()
            ],
            "steps": case_spec["setup"]["steps"],
            "hooks": case_spec["setup"]["hooks"],
        },
        "request": request,
        "before": before,
        "after": after,
        "hooks": hooks,
        "oracle": oracle,
        "oracle_environment": {
            "python": sys.version.split()[0],
            "git": git_version,
            "baseline": BASELINE,
            "verify_oracle_ok": verifier.get("ok") is True,
        },
    }


def verify_oracle_identity() -> dict:
    expected = REPO_ROOT / ".venv-rust-oracle" / "bin" / "python"
    actual = Path(sys.executable)
    if (
        actual.parent.parent.resolve() != expected.parent.parent.resolve()
        or not (expected.parent.parent / "pyvenv.cfg").is_file()
    ):
        raise SystemExit(
            f"refusing to generate fixtures with {actual}; use {expected} "
            "(the lock-exact uv environment)"
        )
    if require_ok(run(["git", "cat-file", "-e", f"{BASELINE}^{{commit}}"], REPO_ROOT, os.environ), "baseline probe"):
        pass
    diff = run(
        ["git", "diff", "--quiet", BASELINE, "--", *BASELINE_FILES],
        REPO_ROOT,
        os.environ,
    )
    if diff.returncode != 0:
        raise SystemExit(
            "reference sources differ from the pinned baseline; refusing to record oracle evidence"
        )
    env = {
        **os.environ,
        "PYTHONPATH": str(REPO_ROOT / "src"),
        "SPEC_KITTY_PACKS_ROOT": str(REPO_ROOT / "packs"),
    }
    result = require_ok(
        run(
            [
                str(expected),
                str(REPO_ROOT / "scripts" / "rust" / "verify_oracle.py"),
                "--root",
                str(REPO_ROOT),
            ],
            REPO_ROOT,
            env,
        ),
        "verify_oracle.py",
    )
    payload = json.loads(result.stdout)
    if payload.get("ok") is not True or payload.get("drift") or payload.get("missing"):
        raise SystemExit(f"oracle verification failed: {payload}")
    return payload


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    if not (REPO_ROOT / "crates" / "sk-git").is_dir():
        raise SystemExit(f"cannot locate the repository root from {HERE}")
    sys.path.insert(0, str(REPO_ROOT / "src"))
    verifier = verify_oracle_identity()
    git_version = require_ok(run(["git", "--version"], REPO_ROOT, os.environ), "git --version").stdout.strip()

    CASES_DIR.mkdir(parents=True, exist_ok=True)
    for stale in CASES_DIR.glob("*.json"):
        stale.unlink()

    cases = build_cases()
    names = [entry["name"] for entry in cases]
    if len(names) != len(set(names)):
        raise SystemExit("duplicate case name")

    run_root = Path(tempfile.mkdtemp(prefix="sk-safe-commit-oracle-"))
    manifest_cases = []
    try:
        for index, case_spec in enumerate(cases, start=1):
            case_root = run_root / f"{index:02d}-{case_spec['name']}"
            case_root.mkdir()
            payload = run_case(case_spec, case_root, verifier, git_version)
            target = CASES_DIR / f"{case_spec['name']}.json"
            target.write_text(
                json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
                encoding="utf-8",
            )
            manifest_cases.append(
                {
                    "name": case_spec["name"],
                    "file": f"cases/{target.name}",
                    "sha256": sha256_file(target),
                    "route": case_spec["route"],
                    "expectation": case_spec["expectation"],
                }
            )
            if case_spec["request"]["wait_for_sentinel"]:
                (case_root / "competitor.done").unlink(missing_ok=True)
            shutil.rmtree(case_root)
            print(f"[{index:02d}/{len(cases)}] {case_spec['name']}: {payload['oracle']['outcome']}")
    finally:
        shutil.rmtree(run_root, ignore_errors=True)

    manifest = {
        "corpus": "scoped-safe-commit-differential",
        "baseline": BASELINE,
        "generator": "generate_oracle.py",
        "generator_sha256": sha256_file(Path(__file__)),
        "python": sys.version.split()[0],
        "git_version": git_version,
        "verify_oracle": verifier,
        "required_case_names": names,
        "normalizations": [
            "reflog entries are compared as (new commit id, reflog subject); timestamps and "
            "identities are not re-derived",
            "error text is compared as a canonical refusal class plus explicit detail tokens, "
            "never as a raw cross-language message; recorded oracle error text replaces the "
            "throw-away run root with <RUN_ROOT> so the corpus is host-independent",
            "hook arguments are compared as (text | file content sha256); the temporary "
            "message-file path is not compared",
            "hook fields git_editor, git_prefix and index_in_git_dir are recorded but excluded "
            "from the compared hook-environment subset; they are asserted explicitly where "
            "they differ (see CONFORMANCE-REPORT.md)",
        ],
        "cases": manifest_cases,
    }
    (HERE / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    (HERE / "PROVENANCE.json").write_text(
        json.dumps(
            {
                "baseline": BASELINE,
                "generator": "generate_oracle.py",
                "generator_sha256": sha256_file(Path(__file__)),
                "python": sys.version.split()[0],
                "git_version": git_version,
                "verify_oracle": verifier,
                "case_count": len(cases),
                "cases_digest": hashlib.sha256(
                    json.dumps(manifest_cases, sort_keys=True).encode("utf-8")
                ).hexdigest(),
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {len(cases)} cases to {CASES_DIR}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
