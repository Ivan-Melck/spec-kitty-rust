# Native Op admission / actual commit integration

Status: historical coordinator evidence for published b80e5ab57. Independent
review gives scoped PASS WITH FINDINGS, not valid-input parity. Later F3
correction/current bindings: `docs/rust/reviews/op-admission-commit-disposition.md`.
Audience:
software-engineer. Updated: 2026-11-03. Baseline published
`42994b2c226bfcfb87e3e682c608a2777179153a`; pinned Python source/oracle
`9aed3a8012cb5eb22c0128b14d525039f95fea7a` unchanged.

## Scope and authority

New `op_admission.rs` / `op_commit.rs` compose existing local writer, Git handle,
primary tracking/ignore probes, project+record ProtectionPolicy union and real
`plan_safe_commit`/`safe_commit`. Both paths share one STANDARD-capability request
builder. Admission is read-only; close rejudges branch/config, keeps source-lifetime
primary cache and returns last outcome. Agent commits per-record; doctor only
closure spine, preserving archive bytes. No record rollback, fabricated OIDs,
privileged capability, runtime Python or global configuration/environment mutation.

Coordinator adds only the existing local sk-workspace dependency edge to the
invocation manifest/lock; no registry/pin change. Owner report retains historical
136-test/hashes snapshot and real compiler/harness first failures. Its signing
misattribution is explicitly corrected, not implemented as a product change.

## Coordinator findings and corrections

Provisional plan: `/tmp/sk-op-admission-commit-integration.md` before edits.

1. Real legal `topic<NBSP>` branch, positive primary and matching protection:
   pinned executor admits after stripping symbolic stdout; original native
   falsely refused. True live-oracle/native red **0 pass / 1 fail**:
   `/tmp/sk-op-admission-branch-behavior-red.log`. Earlier missing `Value` import
   compiler error `/tmp/sk-op-admission-branch-red.log` is not behavioral evidence.
   A single private Python-whitespace strip helper now serves primary-ref policy
   and service current-branch reads; generic Git retains exact ref spelling.
   Added close/no-commit/durable-byte controls; no claim of error-text parity.
2. Real nested pre-commit creates candidate and moves HEAD: backend returns typed
   `SafeCommitAdvanceUnconfirmed`, but adapter discarded candidate/tip into text.
   True red **0/1**, `/tmp/sk-op-admission-uncertainty-red.log`. New typed
   `AdvanceUnconfirmed` evidence retains candidate/destination/observed tip/detail.
   Test verifies actual commit object, distinct current tip, unchanged audit bytes,
   last outcome and no confirmed-success/rewind. Existing landed evidence retained.
3. Pinned `git/commit_helpers.py:1017` explicitly uses
   `git -c commit.gpgsign=false commit --only`. Owner's claim of source signing
   divergence was false. Equivalent real repos now test both source and native
   committing unsigned with `commit.gpgsign=true` and unusable gpg program.
   This is positive source validation, not a behavioral red/fix.
4. Corrected overbroad arbitrary-raw-JSON `str()` assertion: native map ordering
   is sorted, Python insertion ordered. Nonfinite/nonUTF8 and nested raw-map repr
   boundaries remain genuine differences, not waived by documentation.

## Verification

All Cargo commands use `/tmp/sk-focused-cargo.sh`: whole-command flock lease,
2 jobs, no incremental, existing target, temporary HOME/XDG/TMP. Owners idle.

- Scoped Op: **138 passed / 0 failed / 2 parent-exercised helpers** including
  doctest; new suite **22 pass / 1 helper**. Scoped all-target strict Clippy passes.
  `/tmp/sk-op-admission-integration-{tests,clippy,fmt}.log`.
- Stable workspace: **2,188 passed / 0 failed / 11 parent-exercised helpers**,
  **184 suites including doctests**; strict workspace all-target Clippy, full fmt
  check and both release CLI binaries pass, without interruption.
  `/tmp/sk-native-admission-stable-{aggregate,clippy,fmt,release}.log`.
- Required live oracle is not optional in tests; standalone verifier confirms
  Python3.11.15/53 exact requirements, zero drift/missing/duplicates/unsatisfied.
  `/tmp/sk-admission-stable-oracle.log`. Completion exits1:599 findings/PERF27,
  pending performance, valid accounting,0/16 workflows;
  `/tmp/sk-admission-stable-completion{.log,-report.json}`. Isolated root
  `/tmp/sk-admission-completion.VN5whJ` retained.

These are Linux test/build gates, not whole-command parity or a speed result.

## Historical b80e5ab57 source bindings

```
4bc52a693434d2e3724c4f5a8afb08b497c6bd2982189c001e2aade3fa6bb3ab  crates/sk-invocation/src/op_admission.rs
dc0923572473f75b90fb2beacaee178a3a41de7595247de3e4902836b343e44a  crates/sk-invocation/src/op_commit.rs
7782eedba521b3e15439bb850dce3a412e2e5382d7ec1ebc6dbfb6ec82725362  crates/sk-invocation/src/op_commit_policy.rs
fcf04e83cf2f1a2d368b7bea212edbb8fe2c75f9201807b0ffe36c79261f46ec  crates/sk-invocation/src/lib.rs
61667c53f5e7339fe547977c88fab34a14fa066e514454ae20f6928cfeda47bd  crates/sk-invocation/tests/op_admission_commit.rs
7a497e116ea24479e8878bea9d0caeff57ea1dd8492faf2812dbeeb6f1df3784  crates/sk-invocation/Cargo.toml
fbf228d738dfbf278e8c2822e81446b7726aaa8ec659973ebe84dc8169648363  Cargo.lock
```

## Open boundaries

Symbolic current branch is source-stripped but source abbrev fallback is absent.
Repository binding is construction-time (not source per-call rediscovery); root
becoming a repository later is not supported by an existing service. Generic Git
errors retain native text. Source preflight precedence/native adapter-only checks,
short branch restrictions, raw legacy JSON stringification and full topology/error
breadth remain open. Independent review concretely finds NaN/Infinity cause native
refusal where source commits; nested map ordering and1e400 (source inf) change
commit messages/objects. A checked-out NBSP-suffixed branch with an existing
stripped-name shadow also commits in source but refuses native HEAD assertion.
These are functional gaps, not merely diagnostics or approval by disclosure. Protection's own remote-default probes retain
workspace options. 16MiB Git capture and existing Git saturated-history evidence
limits remain. Disclosure does not establish parity for valid-source inputs.

No full profile catalogue/router/governance/glossary/executor, cross-checkout CLAIM
composition, StepContract, CLI wiring, protected-main false-advance repair, inventory
promotion, workflow/platform acceptance, release publication or measured performance
campaign. Source remains681 commits semantically behind fork/upstream main, now
both `1bff48901c62ac971ba11e286c94a4de214f0752`. New19 tail:45 files,+1,446/-1,
contract-test roots/docs/Mission+Op records only; source/pins unchanged. Current
completion stays blocked; coordinator commits+pushes the verified checkpoint and
keeps PR#1 draft. Independent review establishes the exercised slice only;
findings and later corrections are tracked in the disposition linked above.
