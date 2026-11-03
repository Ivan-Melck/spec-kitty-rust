# `sk-invocation` — Op-foundation remediation report (FINAL)

Baseline `1ad605bb7744b8cae1d1a95b6bf449b7d1b92c9a`. Inputs:
`/tmp/sk-native-remediation-orders.md`, `/tmp/sk-rust-op-foundation-review.md`
(stage-1 NOT APPROVED, F1–F7), `/tmp/sk-op-foundation-remediation-coordinator.md`.

This is implementation evidence, **not approval**. No capability, executor,
admission decision, CLI wiring, dispatcher, coverage, manifest/lockfile,
`Cargo.toml`, pin or doc-gate was changed; the only other crate changes are the
authorized additive Kernel module/export below. No agents, commits or pushes. Status: F1–F7 addressed in the scope below; F8's uncertainty rules
retained.

## Coordinator stable integration (after owner delivery)

Coordinator inspected the new path walker and updated only its introductory
wording to describe the retained 40-step containment contract accurately; no
behavior changed (current module hash in §2). Existing lock/containment/private
resolver files remain byte-identical. Stable workspace verification reproduces
**2,073 passed / 0 failed / 9 parent-exercised helpers**, 178 suites, strict
workspace Clippy and fmt (`/tmp/sk-native-custody-stable-*.log`). The combined
shell exceeded its 300-second tool budget during release compilation, after
tests/Clippy/fmt completed; process inspection found no live builds. This was
not a passing release or native failure. The release-only continuation with a
larger tool budget passed in `/tmp/sk-native-custody-stable-release-resumed.log`.
All interrupted artifacts/logs are retained. Independent Op/Kernel acceptance
is still required; these counts do not promote invocation or workflow parity.

## 1. What was preserved (coordinator's F1/F2) and what changed

**Preserved unchanged, independently re-exercised:**

- Op-only `max_precondition_bytes = u64::MAX` in `append_validated`
  (`writer.rs`), with the coordinator's 9 MiB close/read regression
  `a_record_larger_than_the_kernel_default_remains_completable`; generic kernel
  default (8 MiB) untouched in `sk-kernel`.
- `read_started_mode` requires a JSON **object** first row; `[]`, `"bogus"`,
  `5`, `null` are `Invalid invocation record: <id>` (never `Absent`), `{}` is
  `Absent`, `{"mode_of_work": false}` is `Undetermined`
  (`malformed_first_rows_are_not_absent_evidence_modes`).
- That test, its fixture builder, and the coordinator's writer edits were not
  modified beyond rustfmt.

**Changed by this round:** F3 (custody Git stdout), F4 (reader docs/D9), F5
(shared kernel reference-path authority, duplicate resolver deleted), F6
(report/message claim corrections + missing assertions), F7 (float/number repr,
declared residuals), plus additive regression/oracle tests.

## 2. Source binding (sha256)

Pinned Python authority (verified byte-identical to the review; unchanged):
```
978e890fa22ab0651af76badfa6d3610b9746bf78748784a413ea4f9961947e9  src/specify_cli/invocation/record.py
d9b7787f03ef7ba8a21c56e955892fff7f859b43498410241b3b3952bcc34eff  src/specify_cli/invocation/writer.py
45867b93587fee8232ce495bc30bc65abe9d6488b07c6ead0c22b5acbc22f94d  src/specify_cli/invocation/modes.py
997be074fda4a70d279a01ccb7c4579e752d4d0ebd4a739b45073a752fef8c75  src/specify_cli/invocation/errors.py
465abfd972c6c8352235b18e2c348c7d4e5a8860c8285ea50a4145b189b28f40  src/specify_cli/invocation/op_custody.py
c5b8092a29231bfbf60cb29859dc5cae0d2154a79c80b6d6030a0b83a55d14c1  src/specify_cli/invocation/lifecycle.py
23a0aab143de3b3972acf09a41b5a6a95ca2d5e216f0cf2062e37b0d2d9b0a4c  src/kernel/event_log_append.py
8c31578b8c492d2da5045e0c7161f7ee30ae5f798a8eafd1e8116c32642f21e9  src/kernel/resolution.py
15772f225425a8d2714d713007ed1fa10a36c74c86756a90acdd52ffd62131ae  src/kernel/git_topology.py
78d622455564205959d4b881489afa3f6514fa0f7ca24f6529a1a738924b84c1  src/specify_cli/cli/commands/agent/workflow.py
```

Final native state (this task's inputs plus outputs):
```
843f1c3766d57516a54098542b92a4bf08621b27b89b70a3c756dc440a3e819e  crates/sk-invocation/src/error.rs
833a971b33a0340b2c35a120398efadfb16a8ff8daf3cb9278c62ce14f165bc5  crates/sk-invocation/src/lib.rs
cc2b8be5b60c95965d788db398a2ca423707d0e5d005c5cf261754e68ff862cb  crates/sk-invocation/src/modes.rs
7694f7bff9f5c8258e020cc2ce7a4059126d12a318dad2bae8ef00b46a161da4  crates/sk-invocation/src/op_custody.rs      (F3)
53cad7da714c82bc956e9fe166e8a22b608badf333c61fe1a97316a72960e4b9  crates/sk-invocation/src/python_json.rs     (F7)
0170e3a2152edad0dcfa702d4a3a291965c001f004c76ae3ce7487a84e335671  crates/sk-invocation/src/record.rs
7d5cd86a6895b5b98bc51ca93d165fe043a712d354916e79b8ded759bdd2fef5  crates/sk-invocation/src/writer.rs          (F4 docs, F6 seams, coordinator F1/F2)
c24a6fd64dfc79347a314e83cf9486f42fec8c4d9e1ccaedd65f92f840c634c5  crates/sk-invocation/tests/lock_contention.rs
8ff3ef84d1966362624645fdbefd3f28b426ccd65fd900e214fe780b0f474107  crates/sk-invocation/tests/op_custody_roots.rs  (F3 red-first)
17513e96a55c53bb0a45a8d42217e4c1930ca99f26452cd5a41be232439f7e64  crates/sk-invocation/tests/oracle_vectors.rs    (F1 closure)
e5cf9cf85683d1562761674e276c11bfbe0d10412437fc5c49923d95529b3201  crates/sk-invocation/tests/reference_path_oracle.rs (NEW, F5)
3514559efaa6edb366284d0d325249e4a5c28c17c635a6260013aaade2bf1012  crates/sk-invocation/tests/writer_filesystem.rs (F6 lock-unavailable; coordinator F1/F2)
e6cca12a437bf7a47ad567de52c283e2d202932292fcc697ff6fdd2f0d539d65  crates/sk-invocation/tests/support/mod.rs
9551a1a68c6741a44007431f96319e1ad73cfde1c659fa9673974e3f77e8c0e8  crates/sk-invocation/Cargo.toml             (coordinator-owned; NOT edited here)
c3b39d76ba154f62c72245b4ed336441441c3b1d9e2af660bfd2978e373dec88  crates/sk-kernel/src/reference_path.rs        (NEW, F5; coordinator doc clarification)
ed1bec27c6e2e60988f53975b79db97b38e485f30b264e14656b8c03a8221c7d  crates/sk-kernel/src/lib.rs                  (module + re-export only)
a82d3067c8eb3a37fce7dcd2ca4fbd7c08524967f685dc9b057f8ae3c7656c72  crates/sk-kernel/src/fs_resolve.rs            (UNCHANGED)
3dc1c42cfca9e60e79ae6bf18b75d4035a67a18f28bb143f4ce1b0efa05e0f1c  crates/sk-kernel/src/containment.rs           (UNCHANGED)
```

`crates/sk-invocation/src/resolve.rs` was **deleted** (F5); it is still in
`git status` as `D`. `Cargo.lock` is clean (`git status --porcelain Cargo.lock`
empty); `Cargo.toml` has only the coordinator's tempfile promotion.

## 3. Finding-by-finding

### F1 — >8 MiB start/close asymmetry (preserved + independent oracle closure)

Coordinator's fix retained. Independent evidence added
(`tests/oracle_vectors.rs::an_over_eight_mebibyte_record_closes_through_both_writers`):
a native-started 9 MiB record is **closed by the pinned Python writer** and a
Python-started 9 MiB record is **closed by the native writer**; the two trails
are byte-identical, two lines each, and `read_started` returns the 9 MiB
request. The test asserts the fixture exceeds 8 MiB, so it cannot pass
vacuously. Green: see §4.

### F2 — non-object mode rows (preserved)

No change to the coordinator's `read_started_mode` object requirement or its
test. This round only documents the resulting contract in the method doc and
D9 (F4).

### F3 — custody Git stdout deadlock (fixed)

`git_lines` now captures Git's stdout in a **temporary regular file**
(`tempfile::tempfile()` + `try_clone()` as `Stdio`), polls `try_wait()` against
the unchanged **30 s** `GIT_TIMEOUT`, and only after a **successful exit**
seeks to 0 and reads the **full** listing (lossy UTF-8, empty ⇒ `None`). There
is no 64 KiB/8 MiB listing cap and no pipe. Every spawned-child termination
path is handled for the **direct child only** (no process groups): success and
non-zero exit are reaped by `try_wait`; timeout and `try_wait` error both
`kill` + `wait` and return `None` (the reference's degradation). No global
environment or `set_var` is used.

Red-first evidence (temporarily restored HEAD `git_lines`, then restored this
fix; final `op_custody.rs` hash re-verified):
`/tmp/sk-op-f3-red.log` — `a_large_worktree_listing_still_finds_a_record_in_another_checkout`
failed after **30.17 s** with `left: None`, `right: Some("…/lane-0799")`
(800 hand-written linked-worktree admin entries; porcelain > 64 KiB, asserted).
Green after the fix: 14/14 custody tests in **0.25 s**
(`/tmp/sk-op-remediation-tests.log`).

The fixture writes the two back-pointer files `git worktree add` writes
instead of spawning 800 `git worktree add` processes, so the control is cheap
and deterministic; the smaller 20-worktree control
(`a_small_worktree_listing_finds_a_record_in_another_checkout`) separates a
parse bug from the pipe-capacity deadlock.

### F4 — `read_started` is not the claim-reader composition (declaration fixed)

Method docs now say both readers read **only this writer's record root**
(`self.record_root()`), that the reference's `_read_op_started_event` first
composes `locate_op_record_root(repo_root, id, preferred =
resolve_op_record_root(repo_root)) or repo_root`, and that the stage-2 claim
reader must compose that explicitly. No implicit redirect was invented; a
worktree-held record is still reported missing from the wrong root. D9 was
rewritten accordingly, including the F2 object/malformation rule.

### F5 — duplicate private resolver (deleted; shared kernel authority added)

New module `crates/sk-kernel/src/reference_path.rs`, exported as
`sk_kernel::reference_path::resolve_reference_path` with the typed, exported
`ReferencePathError` (`InteriorNul`, `SymlinkLoop`, `Io`), re-exported at the
crate root. It documents and implements the **reference-path profile**, which
is deliberately distinct from the containment/lock profile:

- missing, dangling and **unreadable (`EACCES`)** components are carried
  lexically (any `symlink_metadata` error is a lexical component);
- `..` is applied **after** the symlink in front of it is expanded;
- only a genuine cycle fails; cycle/reuse detection mirrors CPython's `seen`
  map (in-progress `None` ⇒ loop; resolved entry ⇒ reuse the cached prefix), so
  finite repeated links do not explode;
- **no total-link cap**: 41-, 42- and 200-link acyclic chains resolve; open
  self-links and two-link cycles fail;
- interior NUL is refused (CPython `ValueError`).

`normalise_ref` and custody's `resolve_or_keep` use it; `src/resolve.rs` and
its old 40-link budget/NUL-tolerant-by-accident copy are gone. Existing
containment/lock resolution (`resolve_within`, `fs_resolve::resolve_non_strict`,
append/lock placement) is **untouched** — hashes above — and the module docs
state the profiles are not interchangeable and that this does not widen
containment.

Red-first evidence: with the old wiring (temporarily restored `resolve.rs` and
its imports, then restored this fix; final hashes re-verified)
`/tmp/sk-op-f5-red.log` failed on `ref "chain41-00000"`:
`left: Some("chain41-00000")` (verbatim) vs `right: Some("target.txt")`.
Green: `/tmp/sk-op-remediation-tests.log` (2 new oracle tests, 9 new kernel
unit tests).

Four existing resolver unit tests moved into the new module without losing
Linux coverage (`missing_tail_is_appended_lexically` unconditionally, the three
symlink fixtures `#[cfg(unix)]` as before). New unit tests: self-link loop,
41/42/200-link chains, finite repeated reuse, unreadable intermediate
(non-root guard; explicit message when permission-bypass privileges make the
case non-discriminating), plus interior NUL. New live-oracle tests compare
**both** `normalise_ref` and the raw kernel API against the pinned
`writer.normalise_ref` / `kernel.resolution.resolve_rejecting_loops` for long
chains, real loops, `EACCES`, dangling/missing, `..` after expansion, finite
repeated links, absolute/relative/URL refs and NUL. Missing oracle fails
closed: 0 passed / 2 failed (`/tmp/sk-op-remediation-missing-oracle.log`).

### F6 — report accuracy and unasserted messages (corrected)

`IMPLEMENTATION-REPORT.md` corrected in place:

- §6 now lists each message with **where it is asserted**: end-to-end
  filesystem/lock integration (most), refusal-seam unit tests for
  `Failed to open invocation record safely:` / `Failed to append correlation
  event:` / `EVENT_LOG_SYMLINK`, end-to-end for
  `EVENT_LOG_LOCK_UNAVAILABLE`, and unit tests for the Op refusal messages.
  Nothing in that list is now presented as tested when it is only rendered.
- New assertions: two `writer::tests` seam tests (symlink wrapper, kernel
  append-failure correlation wrapper) and
  `an_unresolvable_git_marker_renders_the_lock_unavailable_vocabulary`
  (real `.git` file with an unparsable pointer ⇒
  `Failed to write invocation record: … cannot place the event-log lock …
  Error code: EVENT_LOG_LOCK_UNAVAILABLE.`, and no record created).
- §8 test name corrected to
  `record::tests::id_boundary_is_the_regex_not_ulids_decoder`.
- §3.1 added with the post-remediation hash table; the stale `resolve.rs` hash
  row is replaced by a deletion note instead of leaving two hashes.

### F7 — small details (fixed where source-backed, residual otherwise)

- `python_seconds` and JSON-number `repr` now use CPython's float repr
  (`python_float_repr`): exponent form below `1e-4` or at `1e16`
  (`1e-05`, `1e-07`, `1e+20`, `1e+300`), two-digit minimum exponent, `.0` on
  integral positional values, `-0.0`. Expected strings come from the pinned
  oracle (`/tmp/sk-op-float-repr-probe.log`).
- Exact residuals declared (not claimed as parity, not called impossible):
  Pydantic's multi-line validation detail text (D3); NBSP-class
  `str.isprintable()` edges in `python_repr` (D12); Python's raw
  `OSError`/`FileExistsError` types for an unreadable spine and
  `kitty-ops`-is-a-file (D6/D8/D15); JSON integers beyond `i64`/`u64` are
  `f64` in `serde_json`, so a giant non-string embedded id renders in
  scientific form where Python keeps an `int` (message-only).

### F8 — uncertainty retained

No cleanup deletes a possibly-appended record or spine line; the index stays
warning-only; append failures are not reported as proof that nothing was
written; no executor/admission/auto-commit/lifecycle/CLI was added. The new
Git capture is read-only and its failure paths return the reference's `None`.

## 4. Gates, counts, logs

All Cargo through `bash /tmp/sk-focused-cargo.sh` (shared whole-command lease,
existing target, `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, temporary
HOME/XDG/TMP, retained toolchain cache). Formatting was applied only to files
this task touched; no aggregate, release, clean or cache/evidence deletion.

| Gate | Result | Log |
|---|---|---|
| `cargo test -p sk-invocation -p sk-kernel --locked --offline` | **279 passed, 0 failed, 1 ignored** | `/tmp/sk-op-remediation-tests.log` |
| — `sk-invocation` | **80 passed / 0 failed / 0 ignored**: 30 lib, 4 lock, 14 custody, 4 oracle, 2 reference-path oracle, 26 filesystem | same |
| — `sk-kernel` | **199 passed / 0 failed / 1 ignored** (38 lib incl. 9 new; the ignored test is the pre-existing negative oracle-helper) | same |
| `cargo clippy -p sk-invocation -p sk-kernel --all-targets --locked --offline -- -D warnings` | pass | `/tmp/sk-op-remediation-clippy.log` |
| `rustfmt --edition 2024 --check` on every touched file | pass | `/tmp/sk-op-remediation-fmt.log` |
| `cargo fmt -p sk-invocation -p sk-kernel -- --check` | pass | `/tmp/sk-op-remediation-fmt-package.log` |
| Missing-oracle control (`SK_INVOCATION_PYTHON=/nonexistent/... --test reference_path_oracle`) | **fails closed**: 0 passed, 2 failed | `/tmp/sk-op-remediation-missing-oracle.log` |
| `scripts/rust/verify_oracle.py --root .` | `ok: true`, Python 3.11.15, 53/53 requirements, no drift/dupes | `/tmp/sk-op-remediation-oracle.log` |
| `git status --porcelain Cargo.lock` | empty | — |

Test-count arithmetic: stage-1 delivered 72; the coordinator added 2 (F1/F2) =
74 at the start of this round. This round deleted 4 resolver unit tests
(relocated to `sk-kernel`, +4 relocated) and added 6 net new `sk-invocation`
tests (2 custody, 1 oracle closure, 1 filesystem lock-unavailable, 2
reference-path oracle) plus 4 replacement unit tests (2 `python_json`, 2
`writer` seam) for the 4 removed unit slots → **80**, internally consistent
with 74. `sk-kernel` gains 9 unit tests (4 relocated + 5 new).

## 5. Exact failures observed

Red-first (re-captured by temporarily restoring the pre-fix implementation and
then restoring the fix; final hashes in §2 re-verified):

1. **F3** `/tmp/sk-op-f3-red.log`: `a_large_worktree_listing_still_finds_a_record_in_another_checkout`
   — `left: None`, `right: Some("…/lane-0799")`, `0 passed; 1 failed`, **30.17 s**.
   Cause: the piped stdout was only read after exit, so >64 KiB blocked Git
   until the 30 s probe bound killed it.
2. **F5** `/tmp/sk-op-f5-red.log`: `ref "chain41-00000" diverged from the
   reference`: `left: Some("chain41-00000")`, `right: Some("target.txt")`;
   `1 passed; 1 failed`. Cause: the deleted copy's `followed > 40` budget
   returned the ref verbatim at 41 links, while Python has no total cap.

Authoring failures (mine, corrected, not product defects):

3. First compile of `tests/reference_path_oracle.rs`:
   `E0308: expected Permissions, found &Permissions` on two
   `fs::set_permissions` calls (test-only).
4. Clippy `-D warnings`: `manual_range_contains` in `python_float_repr`
   (`exponent < -4 || exponent >= 16`); rewritten as `!(-4..16).contains(&exp)`.
5. rustfmt differences in the new module/`lib.rs` re-export ordering;
   formatted.

Not observed: any oracle byte/validation mismatch after the resolver switch
(all reference-path cases matched on the first green run), any regression in
the 74 retained tests, any cross-process lock change, or any flake.

## 6. Retained evidence

Logs: `/tmp/sk-op-f3-red.log`, `/tmp/sk-op-f5-red.log`,
`/tmp/sk-op-float-repr-probe.log`, `/tmp/sk-op-remediation-tests.log`,
`/tmp/sk-op-remediation-clippy.log`, `/tmp/sk-op-remediation-fmt.log`,
`/tmp/sk-op-remediation-fmt-package.log`,
`/tmp/sk-op-remediation-missing-oracle.log`,
`/tmp/sk-op-remediation-oracle.log`, plus the coordinator's retained
`/tmp/sk-op-foundation-f1-f2-red.log` and `/tmp/sk-op-f1-f2-git-coordinator.log`.

Backups kept only as evidence of the red-capture revert/restore:
`/tmp/sk-op-op_custody.fixed.rs`, `/tmp/sk-op-writer.fixed.rs`,
`/tmp/sk-op-lib.fixed.rs` (all equal to the final files by sha256). The
red-run fixture roots were inside the helper's temporary sandbox
(`/tmp/sk-focused-cargo.mNMgnH/tmp/.tmpU29kEQ`) and were removed by `TempDir`
drop; green-run fixtures are fresh temp dirs. Existing review roots
(`/tmp/native-large.2wKjuU`, `/tmp/chaincmp.6Ts4yS`, …) were not touched.

## 7. Remaining scope and limits (explicit)

- **No approval and no promotion.** This crate is still unwired: no executor,
  admission decision, catalog/registry, router, governance/glossary, model
  routing, auto-commit/`op_commit_policy`, evidence promotion, WP05 lifecycle,
  `MissionNextInvoked`, StepContract, owned checkouts, CLI/capability wiring.
  Stage-2 prerequisites 1–6 in `IMPLEMENTATION-REPORT.md` §10 still stand,
  including the claim reader composing `resolve_op_record_root` /
  `locate_op_record_root` (D9).
- **Platform:** Linux only. Windows/macOS were not compiled or executed; the
  EACCES case is only discriminating without permission-bypass privileges (the
  tests say so explicitly instead of pretending). `assert_owner_only` remains a
  no-op off-Unix.
- **Resolver residuals (declared, not claimed equal):** `normalise_ref`'s root
  fallback uses `resolve_reference_path(repo_root).unwrap_or_else(keep)` where
  the reference calls `repo_root.resolve()` (which can raise an uncaught
  `RuntimeError` for a looping root); not exercised because a looping repo root
  is outside the meaningful envelope. Unicode `isprintable` edges (D12),
  Pydantic detail text (D3), Python exception types (D6/D8/D15), huge JSON
  integers (F7) as listed above.
- **Not re-run / not claimed:** aggregate workspace gates, both release CLIs,
  performance, full parity, workflow/platform/campaign/≥3× claims, the 90
  migrations and 16 performance workloads, Ruff dev-tool findings, StepContract
  advance, Git long-history creation evidence beyond B1/B2. The coordinator
  runs the stable aggregate after both owners deliver.
- **Not touched:** `Cargo.toml`, `Cargo.lock`, manifests of other crates,
  `sk-git`/`sk-status`/`docs/rust/**` (other owners), dispatcher, CLI,
  coverage, capabilities, pins, production Python, existing kernel
  containment/lock/fs_resolve behaviour.
