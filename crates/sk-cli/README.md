# `sk-cli`: native Spec Kitty command-line adapters

Two thin binaries share this library. Neither invokes Python at run time.

| Binary | Purpose |
| --- | --- |
| `spec-kitty` | Upstream-shaped compatibility surface. Canonical generated prompts (`.claude/commands/`, `.agents/skills/`, ...) invoke this name. Implemented routes execute real domain behavior; every other route is refused before mutation. |
| `spec-kitty-rust` | The original native inspection surface: a versioned JSON envelope plus `project`, `git`, `mission`, `charter`, `glossary`, `runtime query`, `events` and `transition` reads. The compatibility binary also serves it as `spec-kitty native ...`. |

```sh
cargo build --release -p sk-cli --bins
./target/release/spec-kitty capabilities --json
./target/release/spec-kitty init --ai claude --non-interactive
./target/release/spec-kitty native --project-root . mission list
```

`SPEC_KITTY_PACKS_ROOT` selects the built-in pack tree used for canonical
mission and command templates (default: the checkout's `packs/built-in`).

## Implemented compatibility routes

End-to-end binary tests live in `tests/compat.rs`; every case runs the real
executable against a temporary project.

| Route | State | Real behavior |
| --- | --- | --- |
| `init` | implemented | `sk-install` scaffold: config, metadata, agent command/skill assets, ownership manifest, idempotent refresh |
| `agent status validate` | implemented | raw event schema audit + lane-FSM walk + continuity + canonical materialization drift |
| `agent status materialize` | partial | canonical replay under the Mission status lock; coordination-routed writes refuse before mutation pending canonical write-surface integration |
| `agent tasks list-tasks` | implemented | frontmatter + canonical lanes, exact `{tasks,count}` payload |
| `agent tasks status` | partial | kanban rows, lane counts, weighted readiness, done percentage (stale/stalled flags and config-driven `auto_commit` pending) |
| `agent tasks list-dependents` | implemented | `sk-mission` dependency graph |
| `agent mission branch-context` | implemented | deterministic branch contract; never creates or switches a branch |
| `agent mission create` / `specify` | partial | `sk-mission` transactional scaffold with canonical templates; reports `planning_artifacts_only` (no lane/worktree allocation). Coordination topologies are refused before mutation. No Git commit. |
| `agent mission setup-plan` / `plan` | partial | canonical plan template resolution and scaffold; no substantive-content gate or Git commit |
| `agent mission finalize-tasks` / `tasks` | partial | `sk-mission` tasks.md dependency parse and frontmatter patch preserving user content; no planning commit or requirement gates |
| `implement` | partial | `sk-workspace` lane allocation and protected-branch refusal; real branch + worktree creation |
| `next` | partial | `sk-run-store` durable start/query/advance/answer with governed authorization and an upstream-shaped decision payload; mission-board composition fields remain `null` |
| `runtime start/query/advance/answer/recover` | implemented | native extension over the `sk-run-store` contracts |
| `workspace recover` | implemented | native extension over the `sk-workspace` allocation journal |

Every other route (`upgrade`, `agent status emit`, `agent status lifecycle`,
`move-task`, decision ledgers, accept/consolidate/review, charter mutation,
doctor families, tracker/service surfaces) refuses with a typed
`NOT_IMPLEMENTED` or `UNSUPPORTED_REQUIRED_OPTION` payload and a non-zero exit.
`capabilities --json` is the authoritative matrix; it never lists a route as
implemented without evidence.

## Contracts

- Compatibility routes use upstream-shaped JSON: a single document on stdout,
  `--json` where upstream offers it, human output otherwise. Domain refusals
  exit 1; malformed invocations, unsupported required options, and unknown
  routes exit 2. `next` exits 1 only for an advancing blocked decision.
- Unsupported required options are refused **before** any filesystem or Git
  mutation. Tests snapshot the tree to prove it.
- `spec-kitty-rust` keeps its versioned envelope (`schema_version`, `ok`,
  `command`, `exit_code`, `data`), its exit classes (0/1/2/3), and its
  `--project-root`/path policies unchanged.
- Real mutations only happen in the caller's project. Tests use `tempfile`
  fixtures and temporary `HOME`s; neither binary writes to its own checkout or
  the user's home.

## Performance entry points

`PERF.md` names equivalent workloads for the Python-vs-Rust comparison and the
automated conformance commands. Help/version timings are recorded separately
and never stand in for real work.
