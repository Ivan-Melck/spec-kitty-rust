# Binary conformance and performance entry points

These are the reproducible entry points this crate contributes to the loop's
performance gate. They exercise real work, not `--help`/`--version`.

Environment used by every command below:

```sh
export CARGO_BUILD_JOBS=2
export SPEC_KITTY_PACKS_ROOT="$PWD/packs/built-in"
cargo build -p sk-cli --bins --release
BIN="$PWD/target/release/spec-kitty"
```

Fixtures must live under a temporary directory (`mktemp -d`) with a temporary
`HOME`; never point them at this checkout or the user's home.

| Workload | Command | What it measurably does |
| --- | --- | --- |
| cold start | `$BIN native project root --json` | discovery + process start; measured separately, never a substitute for work |
| initialization | `$BIN init --ai claude --non-interactive` | renders and commits ~12 managed asset files |
| status replay | `$BIN agent status materialize --mission <slug> --json` | parses the event log, reduces it, writes `status.json` |
| status audit | `$BIN agent status validate --mission <slug> --json` | schema audit + FSM walk + drift comparison |
| task board | `$BIN agent tasks status --mission <slug> --json` | frontmatter reads + canonical lane reduction + weighted progress |
| mission create | `$BIN agent mission create <slug> --json` | ULID mint, inventory scan, transactional multi-file scaffold |
| dependency finalize | `$BIN tasks --mission <slug> --json` | `tasks.md` parse, frontmatter patch, write-back |
| lane allocation | `$BIN implement WP01 --mission <slug> --json` | Git branch + worktree creation through `sk-workspace` |
| governed run query | `$BIN runtime query --mission <slug> --json` | journal replay, cursor consistency, planner decision |
| governed run advance | `$BIN runtime advance --mission <slug> --agent claude --result success --json` | transactional event append + cursor write + prompt artifact |
| `next` loop | `$BIN next --mission <slug> --json` | mission resolution + run start/query + decision envelope |

Conformance entry points (already automated):

```sh
CARGO_BUILD_JOBS=2 cargo test -p sk-cli --test cli
CARGO_BUILD_JOBS=2 cargo test -p sk-cli --test review_regressions
CARGO_BUILD_JOBS=2 cargo test -p sk-cli --test compat
```

`compat.rs` is the end-to-end binary corpus: every assertion runs the real
`spec-kitty` executable against a temporary fixture and covers human/`--json`
modes, help/version, exit classes, refusal-before-mutation, and actual lane
worktree allocation.

The Python-vs-Rust benchmark harness belongs to the coordinator; this file
names equivalent workloads so the comparison uses the same commands on both
implementations.
