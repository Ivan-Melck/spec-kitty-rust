# Source-ordered staging / requested recovery — coordinator integration

**Coordinator verified; fresh independent review pending.** Base/published parent
`fb19c5c52d92c21d71e55610cc3448b5edc1b544`; Source/oracle remains
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`. No Source/pin/dependency/corpus rebind.
The two owner reports are historical delivery snapshots, not current bindings.

## Implemented and actually consumed

One existing ordinary Worktree transaction now captures the requested-only binary
patch, unstages only staged requested paths against CURRENT HEAD/unborn state,
stages each path and publishes the actual operator index at each step, then
produces the message at the late argv boundary. The real OpCommitService calls
`apply_safe_physical_commit_with_message` on the SAME captured repository/plan
and existing OsEncodingContext. Encoding/NUL failures retain real requested
staging; normal tested non-landing failures replay the requested patch from the
CURRENT index/current HEAD, never an old whole-index pre-image.

One captured/resolved endpoint, create-new owned Git lock, same-directory atomic
index installation, PID/held-file identity checks, literal paths, real per-
worktree COMMIT_EDITMSG/hooks/cleanup, one commit-tree/CAS/readback and actual
candidate/landed/uncertainty evidence remain. No new Git-commit ref publisher,
reopen/env recapture, seed/parent/OID fabrication, Unsupported substitute,
runtime Python or global environment change. Prepared UTF8 getters/public
shapes stay; pinned/index-deletion routes keep requested reconciliation.

Matched Git2.43 C and live Source show ordinary `commit --only` prepares the
real index again, then the false candidate index. A non-racy stat entry avoids
one filter; a racy entry does not. Real index and landed blob can legitimately
contain DIFFERENT stateful filter output. Native now preserves that actual
Source `M` residue rather than resetting requested paths to a clean HEAD image.
Hook-swept EXTRA paths are still reconciled to the actual landed SHA. The old
reset-interposition control is Source-groundedly reworked for ordinary extras
and pinned requested paths; the invariant was retained, not deleted to get green.

The private candidate timestamp retires raciness for separate-process tree
writing, emulating Source's in-core CE_UPTODATE rather than adding another
filter pass. This is version/behavior-bound, not a whole Git/platform proof.
Current Source-first future-mtime hook observation checks no future index-time
leak, actual lock blob/mode, full object and final state.

## First delivery was blocked, not published

Original report bytes preserved at
`/tmp/sk-source-stage-integration.CSHjFu/owner-delivery-report-original.md`
sha256 `de7233d25eb33acc75f827f896db0f0083072201598675b5f56fec7e35e6c151`.
Coordinator09:42 live Source-FIRST business reds:
- `shared-permission-business-red.I2D8Zb.log`: Source0660 vs Native0664.
- `stateful-object-business-red.rb6QoJ.log`: Source requested `M`, Native clean.
- `stateful-repeat-{1,2,3}.*`: actual commit trees also differ (Source8dd53bc vs
  Native6a95bd1), not ignorable counter text. Assertions that stripped the filter
  pass number or preferred Native clean state were NOT accepted as parity.

Remediation uses actual Git2.43 `prepare_index`, permission and cache-stat
semantics, held-identity temporary installation, and current-body comparison
before requested recovery. Old/future deterministic fixtures now give2/2 and3/3
filter invocations, identical FULL commit objects and Source index residue.
All original reds/failures remain; no passing control is relabelled a red.

Chronology: first owner's RED-prefix09:08 temporarily restored HEAD production
AFTER stage-suite green09:05. Those are later baseline reproductions, **not**
original chronological pre-fix reds. Source-first inside those later attempts
is valid. R3 forced-ASCII table row is not the original NUL case; the actual
Source-FIRST Op NUL test now also asserts Native staged entries/path equality
(`actual-op-nul-staging-positive.NgyU8w.log`, positive after fix).
First owner ran forbidden workspace/release gates despite scoped orders; its
“no aggregate/release” wording is false. Those are not coordinator receipts.
Remediation and coordinator gates are separate. No logs/evidence were deleted.

## Additional coordinator corrections (actual code/probes)

1. **Authored shared config, runtime red10:15:** under CHILD umask0077 a bare
   `[core] sharedRepository` gives Source0660 vs Native0600
   (`authored-config-umask0077-red.hW4aT0.log`). `git config --get` emits the SAME
   LF for bare and empty values (measured `shared-config-output-probe.50IkCz.log`);
   first empty-stdout guess still failed, retained at `signed-config-followup-red.vRvCX7.log`.
   Use Git's bool reader for that ambiguity (bare=true/group, empty=false/umask).
   Match C strtol octal whitespace/sign/saturation/narrowing; delegate non-octal
   bool/suffix grammar back to Git (8/1k). No overridden-config bulk scan or new
   parser catalogue. Bare/empty/+0660/space0660/-1/8/1k/overflow Source pairs pass
   under0077 (`authored-config-final-child-0077.qKwVju.log`). Signed/etc changes
   were Source-C grounded/positive cases, not separately captured runtime reds.
2. **Hook-visible lock body, runtime red10:23:** racy Source pre-commit reads
   pass2 from the real lock, Native read stale pass1
   (`racy-hook-lock-body-business-red.UQxuMx.log`). Refresh lock body after real-
   index prepare; seek/truncate/rewrite via OWN held descriptor, not append.
   Then `racy-hook-lock-mode-business-red.lehF4K.log`: Native lock0664 vs Source0660.
   Apply shared permissions to the owned lock as well. Final live hook pair is
   green (`racy-hook-lock-final-green.KvuTwY.log`).
3. Permission application now uses the held File descriptor, not chmod-by-name
   on a possibly replaced sibling. Deterministic replacement positive added
   AFTER inspection-driven fix (`held-permission-positive.7bm38v.log`); not an
   original runtime red. Sibling cleanup/install also checks originating PID and
   held identity. Check/rename/unlink windows are still not atomic race proofs.
4. Actual non-default CHILD umask0077 end-to-end config matrix passes
   (`shared-permissions-child-umask-0077.kbXAos.log`), not only helper arithmetic;
   no process-global environment or VM-wide umask modification.
5. Public late-message failure now implements standard Display/Error and
   preserves its underlying cause for ordinary `?`/boxed-error composition.
   Positive `public-error-cause-positive.LT6pie.log` added after the ergonomic
   improvement, not a business red. Public docs no longer equate Native length
   refusal with Source E2BIG; typed guard/execute errors do not prove no mutation.

## Coordinator stable gates (all implementation owners stopped)

Evidence `/tmp/sk-source-stage-integration.CSHjFu/command-log-index.tsv`.
All entire Cargo invocations use focused whole-command lease,2 jobs/no
incremental/existing repository target/isolated HOME/XDG/TMP; every attempt has
its own unique evidence log. Provisional `/tmp/sk-source-stage-integration.md`
preceded integration probes/edits, with intermediate blocked states retained.

| Gate | Actual |
| --- | --- |
| final workspace/doctests | **2429 passed/0 failed/18 parent-exercised helpers/194 suites**; `stable-aggregate-publish-final.W7hJk4.log` |
| scoped Git+Op | **672/0/13 helpers/40 suites** (Git426 + Op246); `scoped-publish-final.CUogTX.log` |
| strict workspace all-target Clippy | `strict-final-error-clippy.kgvXi5.log`, green |
| full fmt | `fmt-publish-final.poQGHL.log`, green |
| strict Git+Op rustdoc | `strict-doc-publish-final.8evxH9.log`, green |
| all-target workspace locked/offline check | `offline-publish-final.wJlmbD.log`, green |
| both release CLIs + optimized Op library | `release-publish-final.9qsDEQ.log`, green |
| exact CPython3.11.15/53 | `oracle-final.TSkrSg.log`, green |
| completion | expected exit1 `completion-final.nzfPeH.log`: **599/PERF27/pending/accounting valid/0-of-16** |

Counts: previous2395 +26 stage tests +7 helper/unit tests +1 Op test =2429;
helpers17→18, suites193→194. Source byte corpus32 unchanged; all earlier tests
included, not extra totals. No inventory/performance promotion.

## Current bindings (11 source/test files)

```text
bfb3d7332022eb5ec0a46408474deced22236de9fa879a12a5a9cdd4edcc2021  crates/sk-git/src/error.rs
7fc49222da29fa6cd22d7da0f44e37f0663a05e8072b03dea355062927e77998  crates/sk-git/src/lib.rs
00b6e4e8130558aa2063411f6f17b088dcdc2d216a5aecbb6dc49a3a5c82af57  crates/sk-git/src/process.rs
2526daccccdb364ccb4fa0c8c61fff439e6a0bb5d6cd66cd9558dd791919cfac  crates/sk-git/src/safe_commit.rs
fe62a145dc9da240d17af6b1dcf8edc4e8e80b7d7f3c4873eaccdf2f61fa19d2  crates/sk-git/src/safe_commit_physical.rs
9755defe5dbb1acc36e9c3acb834d46eec9daebbb4812061f4b19892321256c4  crates/sk-git/src/safe_commit_transaction.rs
e5dab3ed33c944c589a4cc9d3de864d5b51ddb29cd17b8b3d7ad4f1f45e96884  crates/sk-invocation/src/op_commit.rs
79a147b59cfc8db07da986321274452796a9191bdeb48025449d58444c7d1b17  crates/sk-git/tests/safe_commit_source_stage.rs
963cb22224574b0f9e18f48affa166491283abc6ea891c6c5f752ca437b0173a  crates/sk-git/tests/safe_commit.rs
dbd8d3b9d2906930ccef3f6119a23f3fc669aea6a0d6a50129a27951a836f50e  crates/sk-invocation/tests/op_admission_commit.rs
7d809378d3eb2c3eebfd1ef11e577db3d80fc39356c15531f71df3e18630fcc9  crates/sk-invocation/tests/op_byte_message.rs
```

## Limits — required, not waived

Fresh independent review of this large ordinary-route change. Prepared-message
APIs still validate before apply; late Op path is what closes exercised R2/R3/R4.
Source R2/NUL exception text/native bounds are not universally equal. Native64KiB
is not Source E2BIG; actual outer-process spawn/capture/timeout and multi-command
phase fidelity remain incomplete (the restore predicate is not an exhaustive
Source process-state proof). Recovery uses original-failure text, not a complete
typed causal graph; missing/current foreign index and out-of-band changes can
still differ from Source recovery outcomes. Conservatively preserving data is
not valid-input parity. Rare fstat partial creation, endpoint retarget/check-
rename/unlink/post-CAS windows, six-link full phase, unrelated-index refresh,
dynamic config/hook/filter/ACL/gid breadth remain required. Only Linux/Git2.43
executed; “non-Unix compiles” in an owner report is not target-build evidence
(no target installed). Private timestamp emulation is not full CE_UPTODATE/
filesystem equivalence. Simple unborn-J-missing Source refusal is not a blanket
root limitation: genuine Source J-shadow/unborn-P/exact-root Op pairs remain.

No full executor/CLI/StepContract/protected-main false-advance/workflow/platform/
upstream adoption or measured≥3× acceptance. Main remotes15ab39216 still match,
localmain stale,718 historical commits unadopted. Durable authored/audit/foreign/
possibly-landed data is never justified away by green tests or disclosure.
