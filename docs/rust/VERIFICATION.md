---
type: reference
updated: 2026-11-02
audience: software-engineer
---
# Rust verification evidence

**Incomplete rewrite.** Local test success, partial library parity and fast
inspection workloads do not satisfy [the completion loop](LOOP.md).

## Latest integrated Linux checkpoint

Rust 1.99.1, `CARGO_BUILD_JOBS=2`:

- `cargo test --workspace --locked`: **1,276 passed, 0 failed, 1 ignored** across
  124 suite results, including doctests. The ignored test is a subprocess child
  exercised by the concurrency parent, not a waived acceptance test.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo build --release -p sk-cli --locked`: passed (both CLI executables).
- Inventory/checker regression tests: **34 passed**.
- Benchmark harness self-tests: **19 passed**.
- `check_completion.py`: **exit 1, expected**. Required implementation,
  conformance, independent review, full workflows and performance remain open.

These are checkpoint results, not independent approval of every new module.
The release build precedes subsequent CLI lock integration; rerun all gates and
bind benchmark evidence to the resulting binary/source before publication.
The previously published 455-test foundation remains historical evidence only.
No full upstream Python-suite pass or macOS/Windows success is claimed.

## Oracle and committed conformance evidence

Source baseline: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Use `.venv-rust-oracle/bin/python`, provisioned with:

```sh
UV_PROJECT_ENVIRONMENT="$PWD/.venv-rust-oracle" uv sync --frozen --no-dev --python 3.11
```

Set `PYTHONPATH="$PWD/src"` and `SPEC_KITTY_PACKS_ROOT="$PWD/packs"`.
The installed uv-tool interpreter is not dependency-identical to `uv.lock` and
is not a certification oracle. Python is development tooling only; neither
native binary invokes it as a production fallback.

- Consumer replay: **38 fixtures / 468 rows**, regenerated under the lock-exact
  oracle. Includes 12 bounded real archives, malformed/array/null snapshots,
  absent/typeless metadata, Unicode and floating-point seed arbitration, and
  legacy/current/future schema-generation cases. Input/output digests and
  provenance are checked. No uncommitted 719-archive sweep is claimed here.
- Writer: five committed oracle-generated differential cases plus native
  refusal, interruption, readback and thread/process-concurrency tests. The
  writer still has explicit gaps and awaits independent review.
- Runtime: 34 differential goldens were independently regenerated identically;
  targeted review fixes have additional native regressions. Re-polled MEDIUM
  gates now clear the pending reason, preserving the upstream null title.
- Kernel: native Git-topology, lock-path, home/path and clock differential tests;
  cross-language lock contention and mixed generic-journal writer exercises.
- Installer: a pinned init oracle fixture and native differential checks for
  generated skills/assets, activation, metadata, ledger and preservation.
  Only three of 136 upgrade migrations have native execution.
- FSM: all 2,057 corpus rows are consumed; 1,700 non-genesis expectations match
  exact messages, while 357 obsolete genesis expectations have separate
  current-semantics assertions. This is not blanket legacy-message parity.

## Independent review and remediation

Implementation/review delegates use `tokenfactory/deepseek-v4.1-flash-1017`.
The coordinator integrates and reproduces findings. See
[the lifecycle checkpoint disposition](reviews/lifecycle-checkpoint.md) and
[the earlier kernel checkpoint](reviews/kernel-transaction-checkpoint.md).

Recent regressions cover corrupt-snapshot schema promotion, invented mission
types, clean audits of undecodable rows, Unicode seed ordering, empty prompt
fallback, malformed recorded soft-gate scores, symlink-hidden Git-directory
allocation, SSH BatchMode pins, CWD-dependent lane placement, user `status.json`
retention, recorded branch preservation, topology-changing repair and inherited
status-lock bookkeeping. Passing those tests is not a new independent approval.

## Input policy and remaining risks

- Replay bounds: diary **64 MiB**, snapshot **32 MiB**, metadata **1 MiB**;
  glossary seeds **8 MiB**. Oversized, special, unreadable and non-UTF-8 replay
  inputs refuse with paths. Present but malformed snapshot JSON selects legacy
  replay; it is not classified as absent or silently promoted.
- Bounded replay reads follow symlinks to regular files; canonical log writers
  additionally refuse final symlinks. Static containment is not a capability-
  based sandbox against hostile concurrent ancestor swaps.
- Standard JSON only: NaN/Infinity/overflowing floating-point tokens refuse.
  Arbitrary-precision integer tokens are preserved. Python accepts additional
  non-standard numeric inputs; shell-PID coercion and non-string scalar repr
  still need complete conformance. ISO validation does not certify all Python
  `fromisoformat` extensions.
- Atomic replacement now preserves file mode and syncs the parent on Unix;
  post-rename sync failure is indeterminate, not evidence of no mutation.
  Owner/ACL/xattr/hard-link and Windows durability equivalence remain open.
  Correct family locks are required for multi-file transactions; Mission
  alias/rekey integration and recovery are not certified by atomic replacement.
- Generic kernel O_APPEND is **not** used for either canonical status log.
  Status/lifecycle writers use whole-file replacement and readback. External
  event/tracker service delivery, handover and fan-out are still incomplete.
- Git bounds captures and kills/reaps the direct child, not an entire remote
  helper process tree. Protection is supplied by the application; safe commit,
  complete live protection resolution and all recovery paths remain gates.
- Native initialization is not all upgrades, transports, profile projection or
  global-runtime provisioning. Explicit verified pack sources are still needed.
- Current CLI routes are a mixture of real but partial mutations, read surfaces
  and explicit refusals. Coordination-routed CLI materialization refuses rather
  than writing primary data. Creation/finalization still require canonical
  Mission lock integration. Full governed `next`, acceptance, consolidation,
  merge, decision ledger and service/tracker workflows are not certified.

## Performance

`benchmarks/rust/evidence/2026-11-02-initial/` is a **partial, older-binary
baseline**, not certification of the current source. Required equivalent
workflows are pending and five samples per side do not meet the inventory's
20-sample floor. Startup/projection/micro measurements cannot rescue the gate.
No overall ≥3× improvement or no-regression result is claimed.
