# Byte messages / target progress / full HEAD — coordinator integration

Status: coordinator tested; **fresh independent review pending**. Owner snapshot
`BYTE-MESSAGE-TARGET-REPORT.md` retains original counts/hashes/missing evidence.
Provisional integration plan `/tmp/sk-git-byte-integration.md` preceded changes.
Starting published1bf9dc702480a48225becfcba544013cf1c70e08; source/oracle remains
9aed3a8012cb5eb22c0128b14d525039f95fea7a. No source/fixture/pin/manifest rebind.

## Actual integrated capability

Opaque CommitMessage bytes/provenance/single validation; message-free TargetRequest/
TargetPlan; typed native failure stage + original GitError; actual-message prepare
into applicable MessagePlan. Public legacy request/plan constructors/builders and
message()->&str preserved. One planner, one transaction/execution view, one captured
index/CAS/ref authority. Private raw ref-message carrier preserves all public
String getters and shares validation/prestate/write/readback. Existing symbolic
HEAD reader promoted in place, validated full RefName; current_branch remains a
possibly disambiguated/shortened display label.

Actual native API commits raw80/ff, with Git-owned encoding/newline/cleanup/hooks/
signing. Successful commit-tree stderr now becomes warnings. Existing UTF8request
+ raw-writing hook had true warning/reflog reds; fixes affect existing callers,
not just a new unwired API. Corpus unchanged32=31exact+1documented literal correction.
No copied commit writer/Source path validator/Op encoding policy.

## Source beats our earlier reflog assumption

Coordinator independently repeats five real pinned safe_commit cases in isolated
CWD/HOME/XDG/TMP, exact53. Source hook80 produces object bodyC280 but reflog80;
hookff objectC3BF/reflogFF; ISO8859-1 objects+logs keep80/FF. Hook80 real source OID
672394dc554ac28aaff9432a0d36024ad6a2f2d8 reproduces owner's object. Retained root
`/tmp/sk-byte-coordinator.XcG2Dn/source-live`, data `source-reflog.TuTER5.json`.

Our earlier order's suggested post-Git-created subject was WRONG for this source
route: reflog is PRE-CONVERSION. Correct exact-byte implementation is retained,
not replaced by object-body decoding, Latin1 guessing, Debug escaping or U+FFFD.
Valid UTF8data keeps legacy derivation. New raw data still has native2000-byte
reflog budget, not blanket source parity. Fixed metadata/parents/tree/header/body
required for actual-object claims; a missing-LF design probe was not equivalent.

## Coordinator findings and corrections

1. Non-Unix constructor called nonexistent from_str (renamed to from_text during
   owner work). Real compile defect in cfg-disabled host branch, not platform
   acceptance. Immutable extracted message module with ONLYcfg selection switched
   to exercise that branch on Linux: E0599 red `nonunix-branch-red.vwBe98.log`.
   Fixed to exact from_bytes(text.into_bytes()), preserving OS provenance. Green
   separate immutable projection `nonunix-branch-green.t8TKT2.log`; unconditional
   Unicode OS-string test added. NOT Windows build/certification (Linux target only).
2. Oracle helper and alternate-index setup env_clear did NOT change CWD; assertion
   control true8pass/2fail in `cwd-isolation-red.xppZJh.log`: both ran from source
   crate directory. Harness-isolation red, NOT business red. Spawn explicit isolated
   CWD for helper/worker; assertions+allXDG roots now enforce it. Actual source/native
   seven-row full-object/reflog/status/path equality reruns green in corrected roots.
   Original owner's isolation claim needs this qualification, not silent approval.
3. Public typed failure lacked standard Error/Display (E0277 compile/API usability
   red, not business). Implemented source() returning exact underlying GitError;
   original legacy error values/into_error unchanged. Existing target test checks it.
4. Corrected public comments: native target progress is NOT reference path or
   stage-before-encoding equivalence; removed duplicate message-check documentation
   on message-free method. Other namespaces MAY shorten under --short, not guaranteed
   full spelling. No generic branch normalization or RefName widening.
5. Three additional actual-byte positive controls: pinned parent+exact blobs+
   verbatim newline/ISO8859-1, index deletion retains authored worktree file, and
   configured unusable signer refuses pinned route without HEAD/index mutation
   while ordinary raw-byte route still commits unsigned. All already pass current
   shared backend—NOT fabricated bug/fix reds. Pinned raw bytes are Git capability,
   not source Python pinned write_text surrogate parity.

All evidence paths above under `/tmp/sk-byte-coordinator.XcG2Dn/`. No overwritten
logs or fixture expectation changes by coordinator. Owner AGAIN overwrote two
wrong-import source probe logs; originals unrecoverable/missing, not retained
business reds. Owner disclosure is honest but does not satisfy unique-log policy.
All future command logs need fresh mktemp allocation, not fixed-name redirection.

## Verification and arithmetic

Owner327=baseline293+34; coordinator **331pass/0fail/7parent-exercised helpers**,
25 result lines/doctests. +1 Unicode OS-string unit +3 actual byte route controls.
Error trait/CWD controls extend existing tests, not new totals.

Stable owner-idle workspace **2268pass/0fail/14parent-exercised helpers**,188 suites
including doctests; includes Git331/Op180/Kernel199/doctrine175/charter80 (not extra
counts). Strict workspace all-target Clippy, full fmt, both release CLI binaries
AND optimized Op library pass. Unique logs indexed by
`/tmp/sk-byte-coordinator.XcG2Dn/final-stable-log-index.3QPfb9.txt`; prior stable
snapshot/index also retained, same2268/0/14 counts. Scoped331 log
`scoped-green.BVgBvL.log`. Exact53 `oracle.uneHRe.log`; completion remains exit1/
599errors/PERF27/pending/valid accounting/0of16 (`completion-report.json`).
No inventory/workflow/performance promotion. Remotes main de5f4dc... unchanged
(`upstream-refresh.gqOHRn.log`);
705-commit semantic gap, local main unchanged/stale. New public API is real but not
CLI/Op composition or measured equivalent-work performance acceptance.

## Current hashes (supersede historical owner bindings for changed files)

```
620641e4bd8c7ce1b952d3b704f24c7610824a3cbb65f2596834d5aacae74bea  src/message.rs
0e0969ebd770c1003d5256628461b97507984b20c1e843e5222058bc970c73d0  src/safe_commit.rs
e67e5e0eee9295b47cc32701da9316bbf593e9a935f4f67571408c67289143d5  src/safe_commit_transaction.rs
86e88b2c345f6ab1a12079332f931c9ccec8a27552e6e5a4b24cdb6a7a33fe02  src/refs.rs
54491ab4511a6f682e179081807884450b7cd18ac9d88f60622c1bff9753ca1a  src/objects.rs
6c54058a32ec779c34c24802a49780681c3c0c1498f8973aabc042459ea52550  src/repository.rs
4817c6a61bd5147db5c3433dd8ab03d53899c21a50fe7b9bc6b3884ac760aae5  src/lib.rs
cbe905982b133a2f77299241b671a23f5475c13276820439522ffb0a46acf1b2  tests/safe_commit_bytes.rs
391a13f271bc44cab5759e62320504fa09de266059f2df4c55ce08f5f615ec1b  tests/safe_commit_target.rs
6ea6348860a1ee0e570e1a12668dba17fc3bc0d4db0297a2a5bd700e9f0a33ef  tests/safe_commit_reflog_raw.rs
440550037ef3da36af8bcd2b459e3654f0058d1c31f3aa93c604635fd30fd55b  tests/symbolic_head.rs
```

## Still REQUIRED, not waived

Op fsencode/J-P verdict/composition remains absent; fullHEAD API does not itself
fix born/unborn shadow family, guard/path-source reachability or false admission.
SourceAllowsButNativeCannotPublish must not be mislabeled SourceRefuses/Ready.
prepare-before-apply does not reproduce source stage7→encode8/left-staged failure
footprint; source path semantics and native progress differ. Unborn root commit/
absent-ref CAS and exact existing refs still absent, no J fake parent. Source NUL/
high surrogate/encoding/protection precedence, errors/call-stack limits/platform
fsencoding remain required. Non-Unix raw payload unsupported; post-hook nonUTF8
wide-argv behavior unclaimed.64KiB/noNUL and2000byte native bounds preserved, not
new blanket compatibility. S1/history/platform/CLI/full executor/StepContract/
599completion findings and>=3x measured performance gates stay open. Coordinator
commits+PUSHES verified checkpoint, keeps PR draft, then freezes production for
fresh independent review before Op composition.
