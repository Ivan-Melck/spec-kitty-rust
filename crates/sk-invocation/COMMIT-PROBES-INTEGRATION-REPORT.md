# Native Op commit probes — coordinator integration checkpoint

Historical `d94f6f6e9` bindings/gates. Later review finds and coordinator fixes
the legal-long-ref cap; current correction bindings/limits are in
`LONG-REF-REMEDIATION-REPORT.md`. Scoped acceptance remains explicitly bounded.

Prepared after `12b48e0c5f4dc8596559d5732cf2eac1c0426fce`. Plans written before
work: `/tmp/sk-op-commit-probes-coordinator.md`,
`/tmp/sk-op-commit-probes-integration.md`. Original owner evidence remains in
`COMMIT-PROBES-REPORT.md`; this file binds current integration inputs.

## Real functionality and boundaries

Generic symbolic-ref reads use GitRepository's captured process/root/options/
index/environment authority. Op-domain code implements primary-ref resolution
without current-branch bias, actual tree/record-shape tracking, exact-path ignore
and source skip strings, plus a caller-owned cache bound by lifetime to ONE
borrowed repository handle. No cache can be reused with another handle through
this API. Primary tracking runs/caches before ignore, matching the executor;
failed tracking retries, successful tracking survives a later ignore failure.
Completed nonzero ref/symbolic answers mean absent; spawn/timeout/capture failures
stay typed. Ignore/tree failures remain probe failures, not commit eligibility.

This is not an executor, admission, protection preflight, auto-commit or CLI.
The reference's 30s budget must be selected by a future composing caller through
GitOptions; this handle-based API preserves its configured timeout (default10s).
Opening a repository is a distinct boundary; the9-call budget applies only to
resolution, not discovery. Literal GitPath/PurePath normalization and native
error-message/UTF-8 failure differences remain disclosed. A declared difference
is not independent approval or whole-command parity.

## Coordinator corrections

1. Repository binding is real: OpCommitSkipProbe borrows GitRepository, rather
   than taking an arbitrary new root on each cached call. No fix needed there.
2. Shared invocation-id validation replaces a second private Crockford validator.
   `InvocationId::parse` remains regex-shaped (26Z/26zero accepted), not a ULID
   decoder. Existing pattern/directory/newline controls remain green.
3. Source stdout stripping was missing. Legal trailing-NBSP branch name:
   live Python strips symbolic stdout, fails to find the stripped name, then
   chooses main without Ops; native previously chose the NBSP branch with Ops.
   `/tmp/sk-op-symbolic-whitespace-red.log` proves **0/1 differential red**.
   Policy now applies Python whitespace strip BEFORE origin-prefix removal.
   Generic Git read deliberately retains the real ref spelling. A leading-NBSP
   name control confirms trimming the short name instead would be wrong.
4. Oracle scripts/verifier now use temporary CWD with unchanged absolute source
   imports. Existing helper had run scripts from the source checkout; read-only
   intent was not an exception to the required isolation. New probe establishes
   actual CWD/import source, red **0/1** against original helper:
   `/tmp/sk-op-oracle-cwd-isolation-red.log`. All older Op oracle tests rerun.
5. Corrected original policy suite accounting: owner log actually27 pass+1 helper
   (28 tests), not26+1. Two coordinator tests make29 pass+1 helper (30 tests).
   Missing-symbol compiler reds and initial test-harness mistakes remain
   labelled, not promoted as behavioral-red evidence. Runtime-policy docs now
   refer to the reference executor's force-add, not a nonexistent native commit.

## Stable coordinator verification

All owners idle; every ENTIRE Cargo invocation through
`bash /tmp/sk-focused-cargo.sh`, existing target/shared whole-command lease/
2 jobs/no incremental/temp HOME-XDG-TMP:

- Earlier integration before the Unicode-ref correction: **2,163/0/10**,
  183 suites, strict Clippy/fmt/releases; logs retained as
  `/tmp/sk-native-op-probes-stable-{aggregate,clippy,fmt,release}.log`.
- FINAL after the actual source-fidelity correction: **2,164 passed / 0 failed /
  10 parent-exercised helpers**,183 suites including doctests. Includes
  Op115/Git292/Kernel199/doctrine175, not additional totals.
- Strict workspace all-target Clippy/full fmt and both release binaries pass;
  `/tmp/sk-native-op-probes-final-{aggregate,clippy,fmt,release}.log`.
- Completion still exits1: **599 findings/PERF27/pending**, accounting valid,
  **0/16 workflows**. `/tmp/sk-op-probes-final-completion{.log,-report.json}`,
  isolated sandbox `/tmp/sk-op-probes-completion.qGQmAc` retained.

No interruption/retry-to-green, missing oracle skip, source/golden/pack repin,
measured campaign/current performance receipt/platform/full-workflow approval.
Independent review of this new Op/Git surface follows; no self-approval.

## Fixed current bindings

```text
1eeb6d0c795a03551f5e207d633ad5d14068e90a8ade279a0446c8cd4450f75a  crates/sk-git/src/symbolic.rs
464a3abe6cc9016cef46e5dca1c792c586c92804c2af9788c33d099427766069  crates/sk-git/src/lib.rs
11c4f8c98bd77dd35641df8bb0941c30e2672ff885cee256c63d7b2a7d1cfb51  crates/sk-git/tests/symbolic.rs
7cd7e5e7a72310cf68fcc313bf50be4e1920b01e92f065351d477767e5b2a052  crates/sk-invocation/src/op_commit_policy.rs
e63244c57e2bb75565c1e4021619b58209535a42b5f11d3faf4a2cfdeca906c9  crates/sk-invocation/src/lib.rs
3d7ae395e950d30df248e91d482e487b139bcb528ee6fc59eac8ad1fc3cd9185  crates/sk-invocation/tests/op_commit_policy.rs
924708db9adf6a22ff804633388d02a74878c05cc6e7d8739470f4a4ae9366fd  crates/sk-invocation/tests/support/mod.rs
343fee39d45bb94520fe5e3b7c48eb1ffd5d3373750364a8d4d36a0e16e69194  crates/sk-invocation/Cargo.toml
f7463c00bf1474db8e3f83689ba621d3b00c8465f8fbe573e0c1993be9e8da97  Cargo.lock
```

The coordinator's only lock change adds the existing local sk-git edge to
sk-invocation; registry packages/pins are unchanged. Historical earlier Cargo
hashes remain checkpoint bindings, not current ones. Always commit and PUSH
completed verified changes; PR stays draft, no automatic merge/release.

Remaining: real protection-union/admission, best-effort commit outcomes and
custody/claim composition; catalogue/router/governance/glossary/full profile
execution and StepContract (protected-main false advance); language Unicode/
number/charset differences, migrations/services/workflows/platforms and genuine
≥3× equivalent-work performance acceptance. No inventory promotion from tests.
