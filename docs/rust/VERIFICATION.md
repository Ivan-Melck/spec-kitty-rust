---
type: reference
updated: 2026-11-02
audience: software-engineer
---
# Rust verification evidence

## Aggregate local verification

Linux, Rust 1.99.1, bounded compilation (`CARGO_BUILD_JOBS=2`):

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo build --release -p sk-cli --locked
cargo test -p sk-charter --no-default-features --locked --test review_regressions
```

All pass locally. The aggregate run has **455 passing tests**, including 3 compile-only doctests:

| Crate | Passing tests |
| --- | ---: |
| sk-kernel | 66 |
| sk-git | 49 |
| sk-mission | 84 |
| sk-charter | 77 |
| sk-glossary | 41 |
| sk-status | 33 |
| sk-runtime | 76 |
| sk-cli | 29 |

The FSM test reads all 2,057 upstream corpus rows: 1,700 non-genesis rows match exact acceptance/refusal messages; 357 obsolete genesis expectations are covered by separate current-semantics assertions. This is not a claim that all 2,057 legacy messages still apply. Two upstream runtime next-decision fixtures reproduce exactly; lifecycle fixture parity is explicitly not claimed.

The release binary was also exercised against this checkout for `capabilities`, `project root`, `mission list`, `glossary lookup mission`, `git inspect`, and `transition check planned claimed --actor smoke-test`. Each returned exit 0 and exactly one parseable JSON document with `ok: true`. The Mission crate's real-repository test indexes 723 Missions.

macOS and Windows CI are configured but not claimed locally verified. No speedup, startup-latency, coverage percentage, or full upstream Python-suite pass is claimed. Workspace crates forbid unsafe code; third-party dependency internals are outside that lint's scope.

## Independent review and remediation

All research, implementation, and review subagents used `tokenfactory/deepseek-v4.1-flash-1017`. The coordinator performed integration and fixes. Three independent reviews covered kernel/Git/Mission, charter/glossary, and runtime/status. A fourth, final CLI/integration review approved publication as an incomplete draft with fixes or explicit disclosures; its root/provenance, non-UTF-8, output, path-fidelity, and seed-budget findings were then addressed.

Fixed findings have regression tests in the owning crates:

- **Special-file hangs:** reproduced the FIFO read hang, then the discovery-override and atomic-equality variants. Every file-read boundary now uses the shared nonblocking-on-Unix, regular-file, bounded kernel reader. Atomic equality reads at most the proposed payload length plus one, rather than reading arbitrary destination contents.
- **Duplicate safety authority:** removed the optional charter fallback reader. Disabling default Cargo features cannot bypass kernel I/O policy or change path-separator interpretation.
- **Glossary library bypass:** the public seed loader now uses the same bounded primitive as the CLI, with an 8 MiB limit.
- **Asset root escape:** all three asset tiers validate the assets directory against its owning pack before adopting it as a containment anchor.
- **Partial asset overlays:** absent fields inherit the lower-precedence entry; explicit null title removes the title. Invalid/ambiguous sidecars still fail closed rather than being skipped with warnings.
- **Inventory omissions:** non-UTF-8 names are refused rather than silently lost; symlink checks occur before resolution; only filenames claiming a WP id enter the dependency graph.
- **Event tail loss:** a complete final JSON object without LF is consumed. Only incomplete JSON at EOF is reported as a torn tail; malformed complete tails fail.
- **Audit wire parity:** audit decisions emit/read upstream `reason: null` while retaining Rust's internal enum discriminant.
- **Skipped policy:** declared audit significance now produces a typed unsupported-policy error, not an invented approve/reject gate.
- **Workflow compatibility:** implicit terminal actions and empty descriptions are accepted; workflow ids reject traversal/path spellings.
- **Git captures:** stdout/stderr have a 16 MiB per-stream memory bound; active captures are checked and an oversized child's direct process is killed/reaped. This is not a strict disk quota or descendant-process sandbox.
- **Strict CI gates:** fixed Git module item ordering, formatting, redundant closures, and a large CLI enum variant without suppressing lints. CI also builds release binaries and runs the six smoke commands; the declared Rust minimum now matches the tested 1.99 toolchain rather than an untested 1.85 promise.
- **CLI root/provenance:** explicit roots require project/repository markers and cannot silently treat an arbitrary subdirectory as an empty project. Kernel discovery is reported as discovery, not inferred from the presence of an ignored environment override.
- **CLI data/error fidelity:** non-UTF-8 charter roots/sidecars are typed domain failures, not internal serialization errors. Unix Git paths expose raw-byte hex companions alongside display strings.
- **CLI output policy:** help/version honor `--json`, usage messages do not duplicate clap's error prefix, filesystem data failures are explicitly exit 1, and the unsupported list is explicitly non-exhaustive. CLI and glossary library share one 8 MiB seed limit.

## Deliberate differences and remaining risks

- This command surface is read-only. No canonical event reducer/writer, full governed `next`, worktree allocator, activation, migration, acceptance or consolidation is claimed.
- Event inspection is not a schema audit: timestamps, execution modes, force reasons, review-reference consistency and evidence semantics are not certified by a successful inspection. It does not compute current lanes.
- Templates with duplicate/unknown/cyclic dependencies fail schema validation instead of loading and later returning upstream's blocked decision. Pending-decision map-key mismatches are also refused. These are deliberate fail-closed differences.
- Missing declared org packs make charter inspection fail rather than warn/skip; this checkout's unsynced `packs/internal` demonstrates that policy (also disclosed in the root/CLI READMEs and capabilities). No implicit network fetch occurs.
- Pack core-version constraints are not evaluated. A parsed descriptor is not a compatibility approval. Path expansion supports a narrower syntax than every Python/platform form.
- Path containment is verified for static trees; resolve-then-open is not a capability-based, race-proof filesystem sandbox. Concurrent hostile symlink swaps remain outside the guarantee.
- Atomic replacement does not preserve modes/owner/ACLs/xattrs/hard links or fsync the parent directory; concurrent writes are last-writer-wins. It is not a transactional event-store primitive.
- Git kills only the direct child. User/system Git configuration still affects unpinned behavior. Large graph recursion, exhaustive resource budgets and more adversarial cross-platform cases remain hardening work.
- Minor legacy input/diagnostic differences remain (non-string actors, scalar YAML version formatting, unknown-lane diagnostics, optional pending-request metadata). The scope matrix deliberately does not promise blanket wire parity.

These limits must remain visible in the draft PR and capability matrix. Green local Rust tests do not establish a complete rewrite.
