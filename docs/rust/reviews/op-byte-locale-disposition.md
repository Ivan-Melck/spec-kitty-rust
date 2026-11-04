---
type: reference
updated: 2026-11-04
audience: software-engineer
---
# Op byte/locale review and source-grounded follow-up

Independent `/tmp/sk-rust-op-byte-locale-review.md`, fixed publisheda522ebbd2:
**NARROW PASS WITH FINDINGS**. Renata loaded; production read-only/clean;
9/9bindings, Op220/0/2parent helpers+strict scoped Clippy/fmt/exact53 reproduced.
~35 actual libc/CPython locale probes match supported native facts. Own16pairs:
11equal actual OIDs/fullobjects/reflogs/state,3decisionequal refusals with known
index difference,2Sourcecommits/nativefalse-refusals (ISO8859-15 and nontextLC_ALL).
No new falsecommit/payload divergence found supportednormalbranches. No fullOp/
Sourcevalid-input/CLI/platform/workflow/performance approval. Historical report
unchanged,72unique logs `/tmp/sk-review-a522/` retained.

## F1: actual strict ISO-8859-15 implemented, not waived

Pre-edit plan `/tmp/sk-op-byte-review-followup.md`. Real PRE-EXISTING
OpCommitService+liveSource `de_DE@euro` withé: coor0/1red
`/tmp/sk-op-byte-review-followup.Y4LalO/latin15-existing-service-red.lag8mR.log`.
Source commits/native falsely refuses. Added one strict Latin-9 codec, selected
ONLY from actualOSISO8859-15 fact. Eight replacements map toA4/A6/A8/B4/B8/BC/BD/BE;
the displaced Latin-1 scalars are UNENCODABLE, not silently reused. EscapedDC80..
DCFF still raw bytes. Git—not this codec—owns finalobject conversion/header/log.
Euro→A4 may be stored by Git asC2A4, which is SOURCEbehavior, not a reason to
rewrite the authored preconversion payload asUTF8Euro.

Full independent encoder control: SAME production modules compiled in an external
tempmanifest, actualLatin15context, and exactPython os.fsencode compared across
**1,114,112 code points**.384successful mappings byte-identical (256normal+128low
surrogates); every other codepoint refused both sides. This is OSencoder parity
for this codec, NOT wholeOp acceptance (NUL/payloadbounds/staging differ).
`exact-scalar-pairs.GiWwio.log`, native/source vector files retained under same root.

Permanent public-service test binds equal FULLobjects/OIDs/reflog/paths/state foré,
all8replacementscalars+escapedA4, plus8Source/native displaced-scalar refusals.
All business calls still execute on otherhosts; actualSourcecodec determines
assertions, no locale-availability skip. Native256byte round-trip/control and
surrogate-boundary unit test also passes. Other legacy codecs remain required.

## F2/F3: source beats reviewer/coordinator assumptions

Matched platform `ldd` UbuntuGLIBC2.39-0ubuntu8.9. Downloaded upstream2.39
`intl/localealias.c` and `locale/findlocale.c` to evidence root; curl/metadata logs
retained, no systemfile mutation or imported C in production:

- https://github.com/bminor/glibc/blob/glibc-2.39/intl/localealias.c
- https://github.com/bminor/glibc/blob/glibc-2.39/locale/findlocale.c

**F2 opening-failure part is NOT established as a defect:** glibc read_alias_file
returns0 when fopen fails. Unconditionally converting an unreadable alias file
into native Uncertain/global refusal would be speculative Source-incompatible
hardening. Keep skip-on-open-failure. But discarding the WHOLEfile on invalidUTF8
is wrong: glibc tokenizes bytes. Newnative parser readsbytes, ASCII C whitespace
(includingVT), NUL/string and fgets400 (399payload) boundary; unrelatedbadbytes
no longer discard validASCIIaliases. A MATCHING nontexttarget is retained as native
uncertainty (still unsupported), not falsely Sourceabsent. No libc/env/host mutation.

**F3 actualcode calls one expansion**, not arbitrary recursion: _nl_find_locale
calls _nl_expand_alias once and uses returnedname; _nl_expand_alias doesnotrecurse
onthatvalue. Remove8-level recursive native algorithm and false "like libc"
claim; single-case-insensitive lookup. Native unit controls originally0/2red
`alias-native-unit-red.Rp5Kql.log`, nowgreen. These syntheticprivateparser/chain
controls are SOURCE-CODE-GROUNDED unit reds, NOT liveSourcebusiness counterexamples
(which would require changing root-owned aliasfiles). Byte/NUL/longline/unknown/
missingfile/matchingnontext controls added; no invented environment refusal.

## Stable evidence and current changed bindings

Op**224/0/2helpers**,10suites/doctests =220+1real-service Latin15 matrix+1codecunit+
2aliasunits. lib76/byte20; all existingwriter/numeric/depth/surrogate/index/hook/
landed/unconfirmed/spine/archive/protection controls pass.

Owner-idle workspace**2313pass/0fail/14helpers**,189suites/doctests; strictworkspace
all-target Clippy/fullfmt/both releaseCLIs+optimizedOp library green. Unique logs
`/tmp/sk-op-byte-review-followup.Y4LalO/command-log-index.tsv`:
`stable-aggregate.2jCxKW.log`, `stable-clippy.0BekH2.log`, `stable-fmt.9VrSAE.log`,
`stable-release.bW0QaE.log`. Exact53`oracle-final.8pv1ac.log`; completion remains
exit1/599/PERF27/pending/validaccounting/0of16 (`completion-report.json`). Mainremote
recheck in`upstream-final.3990Ys.log`, no source/golden/pin/manifest rebind.

```
4f26c86979255f6633d260052f36f17c58eb91d42b4278ab2ccc49748dad952e  crates/sk-invocation/src/python_os_encoding.rs
5f0771c2d59282fa20d466815d057305a8125bac2abfca955654ade3d31a71ce  crates/sk-invocation/src/os_locale.rs
4bc1b2324e2d665613f133b18ca990d2e81e72fba78728d25b219e2e36ad0bb5  crates/sk-invocation/tests/op_byte_message.rs
```

Other6a522bindings unchanged; historical integration/owner/independent reports
retained. Small findings coordinator-tested, NOT fresh independent verdict.
Coordinator commits+PUSHES completed verified draftcheckpoint. Next actual
source-orderedJ/P/admission/publication work, not another plan-only waiver.

## Not closed

NontextLC_ALL/alias-target and otherlegacy nonASCII/sourcecodec settings, archive/
non-glibc/path locales/wideargv, native64KiBmessage/2000reflog bounds; per-service
vsstartupcapture; admissionEnvironment-beforeGuard heuristic; SourceStage7before8/
missingSPINE-vsencode/NUL/index-on-failure; J/PF4four falseadmits/physicalpublication/
unborn-root/absentCAS/exactrefs/sourcepaths/errors; protected-main StepContract false
advance/fullProfileInvocation/CLI/migrations/workflows/upstream705/performance.
Native refusal/uncertainty/disclosure does not close these source-valid-input gaps.
