# Byte message / message-free target / full symbolic HEAD — FINAL

Status: **historical owner-final snapshot**, before coordinator corrections.
Current hashes/counts/isolation qualifications: `BYTE-MESSAGE-TARGET-INTEGRATION-REPORT.md`.
This is implementation
evidence, not independent approval. The coordinator integrates, runs aggregates,
commits/pushes, and commissions a fresh review; no Op wiring, Source verdict,
F4 closure, footprint closure, platform claim, or performance claim follows from
this report.

## Fixed inputs and authority

- Repository `/home/exedev/.t3/scratch/2026-11-02-pull-current-spec-kitty-from-442fd12a/spec-kitty-rust`,
  fixed published `1bf9dc702480a48225becfcba544013cf1c70e08`, clean tree before
  the increment.
- Orders `/tmp/sk-native-1bf-git-bytes-orders.md`; design
  `/tmp/sk-git-byte-message-preflight-design.md`; amendments
  `/tmp/sk-byte-message-coordinator-assessment.md`; plan
  `docs/rust/reviews/git-byte-message-plan.md`.
- Source/oracle `9aed3a8012cb5eb22c0128b14d525039f95fea7a`, lock-exact
  `.venv-rust-oracle` CPython 3.11.15.
- Loaded profile `packs/built-in/agent_profiles/implementer-ivan.agent.yaml`
  (directives 010/043/044/045/051; dependency-hygiene tactic) together with
  `AGENTS.md`, the charter, and `docs/rust/{DECISIONS,LOOP,VERIFICATION}.md`.
- Every Cargo invocation went through `bash /tmp/sk-focused-cargo.sh`
  (2 jobs, no incremental, isolated HOME/XDG/TMP, whole-command flock,
  existing `target/`). No manifest, lock, pin, fixture, golden, corpus,
  registry, or non-`sk-git` file changed. `git status --porcelain
  crates/sk-git/tests/fixtures crates/sk-git/Cargo.toml Cargo.toml Cargo.lock`
  is empty.

### Files changed (all inside `crates/sk-git`)

| File | Owner role |
|---|---|
| `src/message.rs` (new) | `CommitMessage`, `MessageOrigin`, the one byte validation authority, escaped `Debug`, `cfg(unix)` `from_os_string` |
| `src/safe_commit.rs` | `SafeCommitTargetRequest`, `SafeCommitPreflightStage`, `SafeCommitPlanFailure`, `SafeCommitTargetPlan`, `SafeCommitMessagePlan`, legacy `SafeCommitRequest`/`SafeCommitPlan` wrappers, one planner body, one execution view |
| `src/safe_commit_transaction.rs` | byte message file, commit-tree success-stderr warnings, exact-byte reflog derivation, shared execution body for both plan types |
| `src/refs.rs` | private `RawRefTransactionPlan` + one shared validation/CAS/readback core; public `RefTransactionPlan`/`Outcome` getters unchanged |
| `src/objects.rs` | `symbolic_head_ref` promoted in place to `pub fn -> Result<Option<RefName>>`; `require_unborn_head` reuses it |
| `src/repository.rs` | `current_branch` documented as the `--short` label, not a full ref |
| `src/lib.rs` | `mod message`, exports, limits docs |
| `tests/safe_commit_bytes.rs` (new) | byte API against real Git + live pinned-source parity |
| `tests/safe_commit_target.rs` (new) | message-free target/stage taxonomy/legacy compatibility |
| `tests/safe_commit_reflog_raw.rs` (new) | the pre-existing-entry-point behavioural regression |
| `tests/symbolic_head.rs` (new) | full symbolic HEAD shapes |
| `BYTE-MESSAGE-TARGET-REPORT.md` | this report (provisional replaced in place) |

## What is actually implemented

- **Opaque exact bytes.** `CommitMessage` captures bytes without re-encoding;
  `from_text` is the legacy string form, `from_bytes`/`from_os_string` the OS
  form. `validate` is the single non-empty / 64 KiB / NUL authority with the
  exact legacy error values. `as_utf8` is inspection only. Manual `Debug` is
  escaped ASCII (`\xNN`), never lossy and never used as wire/reflog data.
- **Message-free target plan.** `plan_safe_commit_target` runs the one planner
  body with `message: None`; `plan_safe_commit` runs it with `Some(&message)`.
  The legacy order `Environment → Message → RequestShape → PathResolution →
  PathPolicy → PathShape → HeadAssertion → DestinationExistence →
  HeadTipConsistency → ExpectedParent → Guard → OperationInProgress` and every
  original `GitError` value are preserved; failures are wrapped in
  `SafeCommitPlanFailure { stage, error }` with `into_error()` returning the
  original value.
- **Applicable byte plan only.** `SafeCommitTargetPlan::prepare(CommitMessage)`
  revalidates through the same authority and only then yields
  `SafeCommitMessagePlan`; it is the only input to
  `apply_safe_message_commit`. Both plan types produce one
  `SafeCommitExecution` view consumed by one transaction body, so HEAD/tip CAS,
  captured operator index, hooks, cleanup, signing, landed/unconfirmed evidence,
  and index reconciliation are shared, not duplicated.
- **Bytes through the canonical route.** The message file is written as bytes;
  the newline rule is unchanged (pinned always appends, ordinary appends only
  when missing); hooks/`stripspace`/`commit-tree -F`/`i18n.commitEncoding` stay
  Git-owned. `commit-tree`'s **successful** stderr is now captured into
  `SafeCommitOutcome::warnings` (previously dropped). Signing behaviour is
  unchanged (ordinary forced unsigned, pinned configured signing).
- **Exact reflog boundary.** The reflog payload is the exact pre-conversion
  message bytes (the same bytes the reference records), not `U+FFFD` and not
  escaped `Debug`. Valid UTF-8 keeps byte-identical legacy rendering (unit test
  asserts `reflog_message_bytes == reflog_message(text).as_bytes()`); raw
  payloads use a private `RawRefTransactionPlan` on the existing single CAS
  writer, with the old public `RefTransactionPlan::message() -> &str` and legacy
  plans untouched.
- **Full HEAD authority.** `symbolic_head_ref` is the same function promoted in
  place: same `git symbolic-ref --quiet HEAD` argv/process seam, full validated
  `RefName`, `UnreadableHead` for corrupt/non-UTF-8/invalid targets, `None` only
  for a detached HEAD. `require_unborn_head` consumes it directly. No second
  resolver, no `RefName` widening to `HEAD`, no `BranchName` acceptance change;
  `current_branch()` remains the possibly-ambiguous `--short` label.

## Red first (retained logs)

All under `/tmp/sk-byte-impl.1793751192/`.

| Log | What it records |
|---|---|
| `red-existing-reflog.log` | **Behavioural red** through the pre-existing `GitRepository::safe_commit`: a `commit-msg` hook writes raw `0x80`; the commit's UTF-8 warning was dropped (`warnings: []`) |
| `red-existing-reflog-order2.log` | Same red with the reflog assertion first: native recorded `commit: op(\xef\xbf\xbd): rewritten` (`U+FFFD`) where the reference records `0x80` |
| `red-api-absence-compile.log`, `red-api-absence-compile-target.log` | **Compile/API absence only** (not behavioural reds): missing `CommitMessage`, `SafeCommitTargetRequest`, `SafeCommitPreflightStage`, `SafeCommitPlanFailure`, `plan_safe_commit_target`, `apply_safe_message_commit`, private `symbolic_head_ref` |
| `green-existing-reflog.log` | The same behavioural test green after the fix |

The red was produced before any product edit; the first run stopped at the
warning assertion, so the test was re-ordered briefly to record the reflog red
and then restored to its final warning-first order (both logs retained).

## Measured reference facts (retained probe root)

`/tmp/sk-byte-srcprobe.Dgtfsl` (isolated HOME/XDG/TMP, fixed identity/date,
exact oracle): live pinned `safe_commit` with a `commit-msg` hook rewriting the
message to `op(\x80): rewritten` produced

- commit object body `op(\xc2\x80): rewritten` (Git converts, warns),
- successful-commit stderr `Warning: commit message did not conform to UTF-8.`,
- branch reflog `commit: op(\x80): rewritten` (raw pre-conversion bytes),
- and with `i18n.commitEncoding=ISO-8859-1`: raw `0x80` in the object plus an
  `encoding ISO-8859-1` header, no warning, raw `0x80` in the reflog.

`/tmp/sk-byte-srcprobe.Dgtfsl/ct2.log` records `git commit-tree -F` on the same
message file reproducing the reference OID
`672394dc554ac28aaff9432a0d36024ad6a2f2d8`, the byte-identical commit object, and
the identical warning — the byte-file route is the same object route.

## Verification evidence

| Gate | Command (all through `sk-focused-cargo.sh`) | Result | Log |
|---|---|---|---|
| Baseline before edits | `test -p sk-git` | 293 passed / 0 failed / 4 ignored helpers, 21 result lines | `baseline-sk-git-test.log` |
| Baseline strict | `clippy -p sk-git --all-targets -- -D warnings`, `fmt -p sk-git -- --check`, `check --locked --offline -p sk-git --all-targets` | pass | `baseline-clippy.log`, `baseline-fmt.log`, `baseline-check.log` |
| Final scoped tests | `test -p sk-git` | **327 passed / 0 failed / 7 ignored helpers**, 25 result lines (24 test targets + doctests) | `full-sk-git-test-5-final.log` |
| Final strict Clippy | `clippy -p sk-git --all-targets -- -D warnings` | pass | `final2-clippy.log` |
| Final scoped fmt | `fmt -p sk-git -- --check` | pass | `final2-fmt.log` |
| Final locked/offline | `check --locked --offline -p sk-git --all-targets` | pass | `final2-check-locked-offline.log` |
| Blast radius compile | `check --locked --offline -p sk-invocation -p sk-workspace -p sk-cli --all-targets` | pass, no edits needed | `dependent-crates-check.log` |
| Corpus | inside `test -p sk-git` | 32 cases unchanged: 31 exact matches + 1 documented literal-path divergence, 0 open mismatches; fixture digests unchanged | `full-sk-git-test-5-final.log` |
| Exact-53 oracle | `verify_oracle.py --root <repo>` with isolated env | `ok=True, checked_runtime_requirements=53, installed_count=53, drift=0, missing=0, unsatisfied=0, python=3.11.15` | `/tmp/sk-byte-exact53.gwi7Ud/verify.log` (+ `verify.json`) |
| Live byte parity | `byte_commits_match_the_live_pinned_source_object_for_object` (child, `env_clear`, fixed metadata, verifies exact-53 first) | pass for `raw80`, `rawff`, `NBSP`, astral, `raw80+ISO-8859-1`, `rawff+ISO-8859-1`, hook-rewrite-`0x80`; equal OID, full `cat-file commit` bytes, reflog message bytes, `status --porcelain`, and committed paths | `full-sk-git-test-5-final.log`, `run-bytes-2.log` |
| Byte API shapes | `test -p sk-git --test safe_commit_bytes` | 10 passed / 3 ignored helpers: raw `0x80`/`0xff`, encoding config, NBSP/astral, empty/NUL/64 KiB bound, hook rewrite, unrelated partial staging, alternate captured `GIT_INDEX_FILE`, stale-HEAD refusal, cross-checkout plan refusal | `run-bytes-2.log` |
| Target/stage | `test -p sk-git --test safe_commit_target` | 7 passed: target plan has no message, `prepare` applies exact bytes, legacy getters/order unchanged, bad message still precedes a bad path, shared failures carry the same error value and truthful stage, unborn → `DestinationExistence`, held lock → `OperationInProgress` | `run-symbolic-target-2.log` |
| Symbolic HEAD | `test -p sk-git --test symbolic_head` | 9 passed: born, unborn, detached, NBSP, tag ambiguity (`--short` = `heads/topic`, full = `refs/heads/topic`), non-head namespace, 1099-byte branch name, three corrupt shapes, non-UTF-8 target | `run-symbolic-target-2.log` |

Count arithmetic: 293 baseline + 4 `message.rs` unit tests + 7 target + 9
symbolic + 1 reflog regression + 10 byte + 3 transaction unit tests = 327. The 7
ignored entries are the 4 pre-existing subprocess helpers plus the oracle helper,
the alternate-index child, and its worker.

### Source hashes (sha256, after final formatting)

```
859d021971127260180aeca219a9981dcd96ce03d3acc9f89e1b35a7ac93ece1  src/message.rs
4be827fce4c71b992e2bd913b4a8835b21ab5a79b30af79178353f3ba65af376  src/safe_commit.rs
e67e5e0eee9295b47cc32701da9316bbf593e9a935f4f67571408c67289143d5  src/safe_commit_transaction.rs
86e88b2c345f6ab1a12079332f931c9ccec8a27552e6e5a4b24cdb6a7a33fe02  src/refs.rs
54491ab4511a6f682e179081807884450b7cd18ac9d88f60622c1bff9753ca1a  src/objects.rs
e79a1f0b154a2e0bde7e875135d68a48313278ed78732ee190b089ca29b54389  src/repository.rs
4817c6a61bd5147db5c3433dd8ab03d53899c21a50fe7b9bc6b3884ac760aae5  src/lib.rs
fe8f48789c49a11de6475de20ea9eb6b6c6f2850d7041b392eebf59f7e3673a5  tests/safe_commit_bytes.rs
ed6cfc79cab99f739571892aa10254545653da4d726d0459d1ff6b610652b967  tests/safe_commit_target.rs
6ea6348860a1ee0e570e1a12668dba17fc3bc0d4db0297a2a5bd700e9f0a33ef  tests/safe_commit_reflog_raw.rs
440550037ef3da36af8bcd2b459e3654f0058d1c31f3aa93c604635fd30fd55b  tests/symbolic_head.rs
```

## Limits, residuals, and non-claims

- **No Op/Source wiring.** `sk-invocation` (J/P resolver, `fsencode`, admission
  and close rewiring), `sk-cli`, doctrine, kernel, workspace, status, manifests,
  locks, pins, dispatcher, capability/inventory, and broad docs are untouched.
  No Source verdict, no `Ready` claim, no F4 closure.
- **No failure-footprint parity.** The reference leaves the requested path
  staged when argv encoding fails at its step 8; the byte path refuses before any
  index mutation, so that residue is not reproduced. `SourceRefuses`-vs-native
  precedence (NUL/high-surrogate, skip/path/protection/encoding) is not fixed by
  `prepare`-before-apply.
- **Unborn publication / exact existing refs** (`refs/x`, `-name`, `HEAD`-named
  branch), Windows wide-argv, non-UTF-8 `sys.getfilesystemencoding()`, and
  reference wording/reflog caps remain open. `HeadTipConsistency` is
  race-only under a symbolic `HEAD` (the two reads normally agree), and is kept
  as a truthful stage rather than removed.
- **Message bound:** native 64 KiB + no-NUL stays; the reference has no length
  bound. Reflog text remains bounded at 2000 bytes (existing rule).
- **Non-Unix raw reflog:** a plan whose payload is not UTF-8 is refused with
  `GitError::Unsupported` before mutation; a hook that rewrites a payload to
  non-UTF-8 on such a platform is not claimed (untested here).
- **No parity-by-disclosure, no independent approval.** Passing scoped tests
  establish these contracts only; full rewrite, aggregate gates, ≥3× performance,
  and every workflow/platform claim remain open. The draft PR stays draft; the
  coordinator commits/pushes after integration.

## Evidence defects (retained, not cleaned)

- The first two live-source probe attempts failed on import names
  (`specify_cli.git.destinations`, `specify_cli.git.commit_guard`) and their log
  path `/tmp/sk-byte-srcprobe.Dgtfsl/probe.log` was overwritten by the next
  attempt. The successful probe log and `results.json` are retained, and the
  failed commands' output was only console-visible; this violates the
  "never overwrite a failed log" rule for those two attempts and is disclosed
  rather than reconstructed.
- All later probe and Cargo logs use distinct names under
  `/tmp/sk-byte-impl.1793751192/`; probe roots
  `/tmp/sk-byte-srcprobe.Dgtfsl`, `/tmp/sk-byte-exact53.gwi7Ud`,
  `/tmp/sk-headprobe*`, `/tmp/sk-headprobe2*`, `/tmp/sk-headprobe3.yZzi8W` are
  retained. No evidence was deleted.

## Suggested coordinator handoff

1. Re-run the scoped gates and an aggregate lane outside this sub-agent turn.
2. Fresh independent review of `CommitMessage`/target-plan/execution/refs/HEAD
   seams, including the legacy byte-identity claim and the private raw reflog
   carrier.
3. Only then compose the later Op increment (`J`/`P`, late `fsencode`,
   `SourceAllowsButNativeCannotPublish`), followed by failure-footprint, unborn,
   and exact-ref increments — each with its own source-grounded proof.
