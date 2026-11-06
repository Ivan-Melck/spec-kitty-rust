# Captured index endpoint: independent review disposition

## Independent subject `34c80624e92a7b021a172fcabb81b55da48bebff`

**NARROW PASS WITH FINDINGS (LOW)**, Linux Git2.43.0/exercised endpoint only.
Final `/tmp/sk-rust-index-lock-endpoint-review.md`, original provisional
`/tmp/sk-rust-index-lock-endpoint-review.provisional-original.md` (sha256
`73186fe1fe0f9b629977770d7797c2eed74372301603bb6b33a03f9cbbc2f8d5`,08:31:26).
Evidence `/tmp/sk-index-lock-review.nGFRSB/command-log-index.tsv`.

Reviewer independently reproduced Git393/0/9helpers29suites and Op245/0/3/10,
combined638/0/12/39 including doctests; strict scoped all-target Clippy/fmt/docs,
exact CPython3.11.15/53 and missing/drift/foreign fail-closed controls; all4
bindings and exact10-file scope. Own Git/strace and live Source helper probes
confirm target vs alias locks, relative/absolute/default symlinks, retarget-before,
foreign alias ignored/foreign target refused, five readlink attempts and
stop-on-error. Permanent Source-first full-object/artifact/captured-stage pair
also reproduced. Workspace2395/release/completion599/PERF27 remain coordinator
claims, not reviewer gates. Clean/same HEAD at end, no production change.

## Findings / actions

1. **Stale `OperatorIndexLock` comment corrected**: actual lock is the resolved
   final endpoint plus `.lock`, from the canonical repository getter, not
   necessarily the captured spelling plus suffix. Runtime acquisition unchanged.
2. **Unused public `explicit_lock_path` retained deliberately** for captured
   spelling records and API/data compatibility. Its docs now explicitly forbid
   using the unresolved candidate to check/acquire the operator lock, and direct
   callers to the canonical repository getter. No deprecation warning or breaking
   removal, no second lock authority, no runtime change.
3. Six-link complete hook/resync/Stage7 intermediate-link footprint remains
   required; resolver depth alone is not whole-phase equivalence.
4. Retarget during hooks/between resolve+acquire/post-CAS/check-unlink windows
   remain required, not magically owned by the getter.
5. Other Git versions, Windows/ReFS/junction/nonUTF8/permission breadth remain
   unapproved. Git error text may name the alias while the kernel opens target;
   target identity/bytes, not diagnostic wording, bind the endpoint proof.
6. Pair's alias-lock absence has a separate Native assertion; live pair checks
   target presence and released locks. No new product defect was found.

## Comment-only follow-up verification

Only comments in index_authority.rs/safe_commit_transaction.rs changed. The
non-comment bytes are identical to34c, checked against actual Git source blobs
(`comment-only-delta.BCJdkx.log`); no new aggregate/release is claimed. Existing
34c2395/0/17helpers193suites/strict/release receipts remain the product baseline.
Strict Git+Op rustdoc, full fmt and exact53 rerun green
(`strict-git-op-doc.E5V1nY.log`, `fmt-check.BanU6w.log`,
`oracle-exact53.lE4MLQ.log`) under
`/tmp/sk-index-review-disposition.fYbFvX/command-log-index.tsv`.
Provisional coordinator plan `/tmp/sk-index-endpoint-review-disposition.md`
preceded probes/edits. Small findings fixed by coordinator, no new review agent.

```text
f466556626e5335f0ff2466fc6c80786f5813c4e3a07393f0e9c92940d7d731f  crates/sk-git/src/index_authority.rs
b7f7fef6dc7a1a1a053fa120ad1af8b4d123298b0f2ff843cf8c38168bf64a44  crates/sk-git/src/safe_commit_transaction.rs
```

Other3 endpoint integration bindings unchanged; its4 hashes remain historical34c
values, not silently current. No Source/Op runtime/Kernel/Workspace/CLI/corpus/
manifest/dependency/pin/inventory mutation. Main remotes still15ab39216,
localmain stale/baseline9aed/718 historical commits unadopted.

## Next

Actual Source staging before encoding and requested-only patch recovery remain
required: R2/R3/R4/corrupt capture and completed-failure restoration. No whole
pre-image rollback, Unix/symlink Unsupported substitute, guessed NoMutation or
64KiB-as-E2BIG classification is authorized. Use one existing captured handle,
canonical resolved endpoint and mutation core; preserve unrelated/current stage,
Source error precedence and real candidate/landed/uncertain data. Rare fstat
leftover, broader phases/platforms/executor/CLI/workflows and measured≥3× remain
required. This narrow verdict does not waive them.
