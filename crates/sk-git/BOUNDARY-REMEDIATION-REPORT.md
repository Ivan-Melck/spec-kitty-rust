# Conclusion boundary remediation — coordinator delivery, review pending

Baseline: `f42d5380b0d3be11773e0b2354eb73e9c2a2c47d`. Independent input:
`/tmp/sk-rust-conclusion-evidence-final-review.md` (G1–G4, NOT ACCEPTED).
Provisional work/evidence record was written before implementation at
`/tmp/sk-git-boundary-remediation-coordinator.md`. No independent acceptance,
aggregate, release, platform, real-fork, workflow or performance claim.

## Corrections

- **G1:** `reflog_conclusion_candidate` validates that the pre-invocation boundary
  exists in the bounded read before examining any potential commit. Only the
  strictly newer slice is eligible. A missing/out-of-window boundary yields no
  candidate, even if an older same-parent commit remains visible.
- **G2:** `commit (amend)` entries are not the original conclusion. Fallback
  skips them; a retained original commit is reported, otherwise identity stays
  unknown. Candidate wording is not weakened into arbitrary publication proof.
- **G3:** the apply API and workspace consumer comments now distinguish
  invocation-bound creation from parent matching and subsequent reachability.
  No functional workspace/CLI consumer change was needed.
- **G4:** the original report now says its hashes describe uncommitted work on
  `d64468a76`, subsequently published as `f42d5380b`, not the older commit's
  tree. Its historical hashes and the independent rejection remain explicit.

The optional suggestion to discard candidate evidence when HEAD/marker look
unchanged was not adopted. A proven created commit must still prevent a
not-landed claim even if later effects restore HEAD or a marker. Existing
process-phase/marker classification, unknown-state retention, argv, hooks,
signing, operator-index authority, bounds and corpus are unchanged. No reset,
abort, new generic refusal, deletion or new success fallback was introduced.

## Red first

Independent reviewer probe re-run: `/tmp/sk-git-boundary-coordinator-red.log`
(observational, not a passing correctness gate). Coordinator assertion probes:
`/tmp/sk-git-boundary-regressions-red.log`, **0 passed / 3 failed**, exit 101.
The same cases were promoted into `tests/conclusion_boundary.rs`; real Cargo
red `/tmp/sk-git-boundary-cargo-red.log`: **0 passed / 3 failed**, exit 101.

1. Rejecting hook deletes exactly the watermark; historical merge must not
   become evidence. Expected ordinary `CommandFailed`, previously foreign
   `MergeConclusionUnconfirmed.candidate`.
2. Selective rewrite leaves a historical same-parent merge but removes the
   child-created object and watermark. Fixture proves object pruning and the
   surviving old entry. Expected unconfirmed/None, previously false `Ok`.
3. Hook amends then overruns the started command's timeout. Expected original
   created commit as candidate and amended HEAD as observation, previously the
   amendment was misattributed.

Probe roots are retained; the new tests use isolated HOME/config/TMP and retain
fixture roots. Hook executable bits are Unix-conditional; no Windows run is
claimed.

## Coordinator verification

Through `bash /tmp/sk-focused-cargo.sh` (whole-invocation lease, existing target,
two jobs, no incremental, temporary HOME/XDG/TMP):

- `test -p sk-git --locked`: **259 passed / 0 failed / 3 parent-exercised
  helpers**, 19 suite results including four doctests.
- Scoped all-target strict Clippy: pass.
- Git/workspace format check and `git diff --check`: pass.

Logs: `/tmp/sk-git-boundary-{cargo-green,clippy,fmt-check}.log`.
The 32-case corpus remains 31 exact + one disclosed literal-path correction;
no claim of 32 equal cases. Other status implementation files changed
concurrently outside these packages; no aggregate/release was run.

Reviewed-input hashes for the next independent check (SHA-256):

```
44576da4d249bfce8715c6f1e0d2fb2b61561a619b3a9d37a820def1f1cd13ee  crates/sk-git/src/merge.rs
9acee9ee2d9338349f3d2b3e078e97d98719c295591d432ddd09997e1c74147d  crates/sk-git/tests/conclusion_boundary.rs
dd0c7f5eb17d5727f5674743cd05d3a9c2bfa505351a8798ffdd913ca0970e09  crates/sk-git/README.md
8da9342482b6ce96d6782dad0d7d57e08062cbae6ab933eb02bd3fe97cc94407  crates/sk-workspace/src/planning.rs
```
