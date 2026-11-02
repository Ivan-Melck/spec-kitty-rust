# sk-git

Read-only Git adapter for the Spec Kitty Rust workspace.

`sk-git` answers a small, fixed set of questions about one checkout. It does
not write to the repository, does not contact a remote, and does not expose a
generic "run git" surface. The Python implementation in
`src/kernel/git/{runner,listing,remote}.py` is the migration reference; this
crate is an independent Rust contract, not a line-for-line translation.

## Public API

| Item | Purpose |
|------|---------|
| `GitRepository::open(path)` | Resolve a path (directory or file) to its work-tree root. |
| `GitRepository::open_with_options(path, &GitOptions)` | Same, with a pinned git program and timeout. |
| `GitRepository::root()` | Canonicalised work-tree root of the checkout. |
| `GitRepository::current_branch()` | Short branch name, or `None` on a detached HEAD. |
| `GitRepository::head_commit()` | HEAD commit id, or `None` in an unborn repository. |
| `GitRepository::is_dirty()` | Any staged, modified, or untracked path. |
| `GitRepository::dirty_paths()` | Paths from the default status query. |
| `GitRepository::status()` / `status_with(&StatusOptions)` | Typed porcelain-v1 status entries. |
| `GitRepository::worktrees()` | `worktree list --porcelain` entries. |
| `GitPath` | Repository-relative path with raw `OsString` bytes retained. |
| `StatusEntry` | Index/worktree letters, path, rename source, directory flag. |
| `Worktree` | Path, HEAD, short branch, detached/bare/locked/prunable state. |
| `GitOptions` | `program` (default `git`) and per-invocation `timeout` (default 10s). |
| `GitError` | Typed failures: unavailable/spawn/timeout/command-failed/not-a-repo/bare/invalid output/non-UTF-8. |

Absence is not an error: detached HEAD and an unborn HEAD return `Ok(None)`.
Every other inability to answer returns `Err(GitError)`; a failed status probe
is never reported as a clean repository.

## Guarantees

- **Single bounded subprocess seam.** Every invocation goes through one internal
  `GitProcess::run`. Arguments are passed as argv entries to `Command::new`,
  never through a shell. No public method accepts revisions, pathspecs, remote
  names, or arbitrary arguments.
- **Timeout with kill + reap.** A finite bound applies to discovery and every
  query. On expiry the direct git child is killed and `wait`ed (reaped) before
  `GitError::Timeout` is returned.
- **No interactive prompts.** Children get `GIT_TERMINAL_PROMPT=0`, null stdin,
  and `GIT_PAGER=cat`. This crate never runs a remote-contacting command, so
  there is no credential path at all.
- **Read-only probes.** Status uses `--no-optional-locks`, `GIT_OPTIONAL_LOCKS=0`,
  and `-c core.fsmonitor=false`, so it does not refresh/write the index and does
  not execute a repository-configured fsmonitor hook.
- **Explicit repository identity.** Inherited `GIT_DIR`, `GIT_WORK_TREE`,
  `GIT_COMMON_DIR`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, and related
  redirecting variables are removed from every child.
- **Raw paths.** Porcelain status and worktree listings are parsed from `-z`
  records. On Unix, paths are kept as exact `OsString` bytes and never round-trip
  through a lossy `String`.
- **Typed absence vs failure.** Only git's confirmed "not present" answers are
  mapped to `None`/`false`; a command failure is always an error.

## Limits and non-goals

- No push, fetch, clone, commit, checkout, reset, ref update, stash, worktree
  add/remove/prune, or any other mutating or network operation.
- Requires a work tree. Bare repositories are rejected with
  `GitError::BareRepository`; a path inside `.git` is not a work tree.
- Worktree listing prefers `git worktree list --porcelain -z` (git 2.36+) and
  falls back to newline-delimited porcelain output on older git. A worktree path
  containing a newline (or a trailing carriage return) cannot be represented by
  the older format.
- Repository root discovery uses `git rev-parse --show-toplevel`, which cannot
  represent a root path ending in a newline.
- Branch and ref names are exposed as `String` and must be UTF-8. Paths are not
  subject to that restriction on Unix.
- The timeout bounds the direct git child only. Descendants (for example a remote
  helper) are not signalled; no API here starts one.
- User/system git configuration and attributes still affect results (for example
  `core.autocrlf`, `status.showUntrackedFiles`, `safe.directory`). The adapter
  does not rewrite configuration or claim parity across every configuration.
- Passing tests establish the covered contracts, not blanket compatibility with
  the Python implementation.

## Tests

The tests use real temporary Git repositories and the real `git` binary, plus a
fake `git` shell script for the bounded-process tests. They need Git >= 2.28
(for `git init -b`) and a Unix host for the fake-executable tests. The
timeout/reap assertion reads `/proc` and is therefore Linux-only.

Run only this crate (not the whole workspace):

```bash
CARGO_BUILD_JOBS=2 /home/exedev/.cargo/bin/cargo test -p sk-git
```

Current count: **48 tests** — 13 unit tests, 27 acceptance tests, 6 bounded
subprocess tests, 2 compile-only doctests.

Coverage highlights:

- repository discovery from a nested directory, a file path, a linked worktree,
  and paths containing spaces, shell metacharacters, and non-UTF-8 bytes (Unix);
- `main`, feature, detached, and unborn HEAD;
- clean, modified, staged, untracked, ignored, renamed, and collapsed-directory
  status entries, including non-UTF-8 path bytes;
- worktree listing with a linked checkout, detached HEAD, and a lock reason;
- not-a-repository, missing path, bare repository, `.git` interior, and
  command-failure classification;
- bounded timeout with child kill/reap verified through `/proc`, missing git
  executable, direct (non-shell) argv delivery, and hardened child environment.
