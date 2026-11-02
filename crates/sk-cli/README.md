# spec-kitty-rust CLI

Native, read-only Rust inspection commands. **Not a drop-in replacement for the Python CLI.** No command creates Missions, advances persistent runs, changes lanes, activates doctrine, allocates worktrees, or merges branches. See `capabilities` and the workspace migration status before scripting it.

```sh
cargo build --release -p sk-cli --locked
./target/release/spec-kitty-rust capabilities --json
./target/release/spec-kitty-rust --project-root . mission list
./target/release/spec-kitty-rust glossary lookup mission --scope spec_kitty_core
./target/release/spec-kitty-rust --json --help
```

## Output and exit codes

Operational commands emit exactly one versioned JSON document on stdout, even without `--json`; diagnostics go to stderr. Help/version are human-readable by default and use the same JSON envelope when `--json` is passed.

- `0`: successful inspection or a runtime step, pending decision, or terminal query.
- `1`: data/domain refusal or I/O failure, including missing/invalid filesystem paths.
- `2`: malformed invocation, unknown command, or invalid scalar option syntax/value.
- `3`: a successful runtime query whose decision is blocked (`ok: true` does not mean permission to advance).

The unsupported-command list is illustrative, not exhaustive: unlisted commands refuse too. `events inspect` reports parsed stream statistics, not current lanes or a clean schema audit. `runtime query` is a pure library query, not the governed `next` operation.

## Roots, paths and limits

`--project-root DIR` is authoritative, not a discovery starting directory: it must contain `.kittify/`, `kitty-specs/`, or `.git`. Without that flag, kernel discovery walks from the working directory and may honor a valid `SPECIFY_REPO_ROOT`. Reported `source: discovery` refers to that kernel operation, not proof that a particular environment override was selected.

JSON path strings are display representations, not byte-preserving filesystem identities. On Unix, Git entries additionally expose `path_bytes_hex` and rename sources expose `original_path_bytes_hex`; decode these rather than reopening a lossy display string. Charter inspection refuses non-UTF-8 roots and sidecar names as domain errors.

Glossary CLI and library loaders share an 8 MiB seed limit. When every selected scope has an explicit seed, no project is required:

```sh
./target/release/spec-kitty-rust glossary lookup mission \
  --scope spec_kitty_core --seed-file spec_kitty_core=/path/to/core.yaml
```

Charter inspection fails closed when declared packs are missing. This source checkout pins but does not bundle the pack trees; `make sync-packs` (upstream build tooling, network required) may be needed first. The Rust binary performs no fetch and never falls back to Python.
