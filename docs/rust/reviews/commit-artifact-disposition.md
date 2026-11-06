# Ordinary commit artifact: independent review and follow-up

## Independent review at `5afb1b0184542e3182750dee905bef11252180dd`

**NARROW PASS WITH FINDINGS**, exercised ordinary artifact/owned-lock slice only.
Actual final review `/tmp/sk-rust-commit-artifact-review.md`; retained provisional
`/tmp/sk-rust-commit-artifact-review.provisional.md`; evidence index
`/tmp/sk-rust-artifact-review.MNfEd0/command-log-index.tsv` (55 invocations).
Reviewer reproduced Git373/Op245 =618/0/12 helpers/39 suites, strict scoped
Clippy/fmt/docs/offline/exact53, all12 bindings and164-row lock SBOM/dependency
checks. Workspace/release/completion were coordinator-cited, not reviewer gates.

Independent Source-first probes confirm unchanged artifact-before-noop timing,
OLD pre-commit artifact and valid DIRC lock, Source-landed extra paths after
`git add -u; touch index.lock` (the revised expectation was Source-grounded),
missing index and cleanup modes. Deterministic post-object replacement retained
a real commit candidate, unchanged HEAD, observed old tip, uncertainty detail
and foreign lock bytes. No approval of full phases/platforms/performance follows.
Reviewer disclosed raw read-only orientation before its provisional/wrapper;
its later wrapped logs do not retroactively establish all-command compliance.

## Findings and coordinator disposition

- **F1 — partial acquisition / duplicate descriptor:** transfer the created File
  into the held `same-file::Handle`, and write through `as_file_mut()`, rather
  than duplicating the descriptor after creating the public lock name. This
  removes the EMFILE/ENFILE duplicate-descriptor failure path. A bounded Linux
  child (prlimit64, exactly one free descriptor) now verifies acquisition and
  normal ownership-safe cleanup; no global resource/environment mutation.
  This regression was added **after** the inspection-driven fix; it is positive
  fault-injection evidence, **not** an original pre-fix runtime/business red.
  **Residual:** a genuine file-identity/fstat I/O failure after creation can still
  leave a lock whose identity is unconfirmed. The typed error now explicitly
  records creation/possible leftover/path and warns against unlink-by-name.
  No blind cleanup, private-name/hardlink filesystem substitution or fake ID was
  introduced; this rare I/O recovery case remains required, not fully closed F1.
- **F2 — wrong red citation:** the actual05:54:08 Source-first spelling red is
  `/tmp/sk-commit-artifact-ea717/red-git-primary.1TJv6T.log`. Earlier
  `red-git-primary.lFET3v.log` at05:54:01 is the harness NotFound, **not** that red.
  Current integration citation corrected; historical owner text/logs retained
  with this correction, never reconstructed/backdated. Op consumer was added
  after the fix; its Source-first positive is not a separate pre-fix red.
- **F3 — corrupt index error phase:** Source staged-patch capture/recovery vs
  Native prospective-index failure remains REQUIRED in the staging bridge.
  Equal refusal/ref immobility does not establish phase parity.
- **F4 — missing ownership regressions:** permanent positives now cover
  prepare-commit-msg replacement, symlink replacement (link/foreign target both
  preserved), and post-object replacement with a real typed candidate/observed
  old tip and no publication. These add3 test functions including the FD case;
  the existing ownership test also gains a prepare-phase row. No fabricated reds.

Only transaction ownership storage and artifact tests changed at runtime; no
Op runtime/Kernel/Workspace/CLI/Source/parser/encoder/corpus/dependency/pin change.
The Linux FD and Unix symlink tests are explicit platform-scoped controls, not
Windows/ReFS proof. PID/held identity/fences/CAS/readback and residual check/unlink
and post-CAS resync windows remain as before.

## Coordinator stable follow-up gates and current bindings

Evidence `/tmp/sk-artifact-review-followup.Y2A3QW/command-log-index.tsv`:
workspace **2378/0/17 helpers/193 suites** (`stable-aggregate.6WV9vT.log`);
Git **376**, Op **245**, combined621/0/12/39 (`scoped-git-op.cOI8e1.log`).
Strict workspace all-target Clippy `stable-clippy.MuZ6B8.log`, full fmt
`stable-fmt.mdOFEu.log`, strict Git/Op rustdoc `strict-git-op-doc.wQCQ7Q.log`,
locked/offline all-target check `offline-check.khUNB6.log`, both release CLIs and
optimized Op library `stable-release.eWdppp.log`, exact53
`oracle-final.njgy2U.log` all pass. Completion expected exit1
`completion-final.mOXwsa.log`: **599/PERF27/pending/accounting valid/0-of-16**.
FD child `descriptor-resource-control.vyBbiD.log`, artifact14+1helper
`artifact-ownership-controls.seyoxR.log` pass. Final test-module documentation
clarifies Native-only ownership controls are not Source pairs; post-object shell
wrapper control explicitly Unix-scoped, no Linux behavior change. Final fmt and
artifact14+1helper rerun pass (`final-test-doc-fmt.a29iNR.log`,
`final-artifact-controls.osFGMe.log`); binding `final-artifact-hash.ZmckpW.log`.
No new dependency or lock/SBOM delta.

```text
12ba26beadbb5a712ffe54b8eb94318a3f141263670764b320e90da5c55fc840  crates/sk-git/src/safe_commit_transaction.rs
969860566dcdfe4b9ae30e5c2e6f93b4c63203d67a6815fe7500070b388bc455  crates/sk-git/tests/safe_commit_artifact.rs
```

Other10 bindings in the integration report remain byte-identical; that report's
12 hashes/counts are the historical5afb snapshot, not silently current hashes.
No new delegate for these small findings. Coordinator performed verification;
independent review only certifies its fixed5afb tree. Follow-up provisional
`/tmp/sk-artifact-review-followup.md` preceded changes/probes. One initial raw
read-only `rg` orientation (same-file methods/GitOptions/reviewer probe paths)
escaped the wrapper; disclosed, not cited as wrapped evidence or reconstructed.

## Still required

R2/R3/R4, actual operator staging before encoding, precise Source staged-patch
capture/restore and exception/error precedence with ownership-safe recovery;
rare identity-I/O partial acquisition; broader filters/hook environment/index
permissions, concurrency/post-CAS window, platform/ReFS, message bounds,
full executor/CLI/StepContract, upstream semantic adoption and measured≥3×.
Disclosure or conservative refusal does not close Source-valid gaps.
