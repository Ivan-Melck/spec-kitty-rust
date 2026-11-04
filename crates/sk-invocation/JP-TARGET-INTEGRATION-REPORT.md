# Source-ordered J/H/P target — coordinator integration

Status: coordinator tested; **fresh independent review pending**. Provisional
`/tmp/sk-jp-target-integration.md` preceded integration fixes. Published base
33a9102d8f23b35a91cdb9d3cdd6bc4149d9d9b8/source-oracle9aed3a8012cb5eb22c0128b14d525039f95fea7a.
Owner `JP-TARGET-REPORT.md` is historical pre-integration snapshot, not current
hashes/counts/full source-path proof. No source/fixture/pin/manifest rebind.

## Real target behavior now exercised

One shared per-call Op target adjudicator: J=executor --short+Pythonstrip,
H=full symbolicHEAD+stripWHOLEstdout then removeheads prefix, P=exact physical
publication name. Source shape→roots→H==J→refs/heads/Jexists→source paths→J-policy
union→ONEcanonicalSTANDARDguard, only then nativephysical target plan/lateOSbytes/
canonicalapply. Admission and actualcommit are DISTINCT: source preflight failure
may admit audit write but refuse commit. Source preflight allows/native cannot
prepare has explicit no-plan native failure, never fakeReady/SourceRefuses.

Actual six shadow rows: born/unborn topicNBSP+existingtopic ×unprotected/protectJ/
invalidconfig. Four nativefalse-admits now fixed/read-only6of6bothsides. Supported
born-unprotected binds equal actual OID/FULLobject/reflog/paths/Ptip; J/main seeds
unchanged. UnbornSource creates ROOTcommit, native truthfully no publish/plan/
evidence and P remains absent. Owner phrase "one parent token" is wrong: there
is ONEOIDTOKEN in rev-list line, **ZEROparents**. Never seedPfromJ/fakeparent.
D1refs/x missing sentinel protected/invalid refuses; DIRECTORYsentinel admits
because source6bprecedes6c. Actual-only-P protection quirk and Source tag-label
Jheads/topic vs Htopic commit refusal preserved, no speculative extra P policy.

Source/native path projections share ONE FS observation/shape/gitlink substrate;
oldnativeplanner ordering/errorvalues and32casecorpus retained. Source path proof
now carries exact normalized staging spelling via sealed SourceCommitPath, so
publication consumes the source-resolved path, not an unnormalized symlinkalias.

## Coordinator findings, corrections and evidence classification

Evidence `/tmp/sk-jp-integration.dpvKNJ/`, every attempt unique wrapper log.

1. Owner passed relative staging path to source projection even though Source
   uses writer record_root/path spelling (usually absolute). It also reused a
   native-only absolute parent resolver without Sourceleaf-loop/stat or tolerant
   missing/.. behavior. True public-API + LIVE pinned preflight reds0of2:
   `absolute-source-path-red.jXRMZ9.log`: absolute looping leaf incorrectlypassed;
   absolute missing/../record incorrectlyrefused. Source asserts ran first.
   Fix consumes actualwriter path; source parent resolution delegates existing
   canonical Kernel resolve_reference_path (NOTduplicatedresolver), keepsleaf/
   probesleafloop, returns normalized SourceCommitPath for canonicalpublisher.
   Native oldnormalization untouched. ThreeactualSourcepreflightcontrols nowpass:
   leafloop, missing/.., symlink-parentnormalizedstaging+worktreespolicy. Owner's
   six path controls are REALGIT/NATIVEassertions, not six liveSourcecomparisons.
2. Existence oldread_ref was for-each-ref enumeration/checked/OIDparse, NOTthe
   rev-parse predicate promised inreport. Addedcanonical ref_exists exactargv
   rev-parse--verify--quiet/allcompletednonzero=false; never-started/capture/bounds
   remain typederrors. Positive existing/missing/realblob-ref controls pass (not
   claimed business reds), no commit-type assumption/OIDfabrication.
3. Owner folded EVERYUnreadableHead into Sourcecompletednonzero, including
   SUCCESSstdout that fails UTF8/RefName validation. One canonical fullHEAD
   observation now retains completedstatus separately from exact decodedtarget;
   Source onlycompletednonzero=>mismatch, validation/spawn/capturefailures are
   errors. Existing strict symbolic_head_ref wraps SAMEprobe/parser and keeps
   original RefNamevalidation/errorvalues (no widening). Observation itself is
   NOTpublicationauthority and cancarry Sourcelegal refs/x text the stricter
   RefName cannot represent. No Display-errorparsing/secondHEADresolver.
4. Source-allowed non-heads physicalref case: realrefs/x+shadowrefs/heads/refs/x+
   root x (all point to REALseedOID) disambiguate J=H=refs/x; actualSource commits.
   Firstcoorcase before rootx expected Jrefs/x but actualJx: FIXTUREASSUMPTIONfailure
   `nonheads-native-capability-red.Fh4Ypd.log`, notbusiness red. After actualrootref
   fixture, `.JZAiB5` exposed strictnativeRefName limitation; after rawobservation,
   TRUE `.Jv76X7` Sourcecommit/nativeinvented HEADmismatch(H==Jsame!). Fix native
   non-heads P classification to SourceAllowedButNativeCannotPublish, no plan/
   evidence/mutation. Publication itself remains required; truthfulclassification
   is NOTsourcevalidpublicationparity. Reason now says SOURCEPREFLIGHTallows, not
   a promise that staging/hooks/commit would succeed.
5. Owner born-metadata child inheritedsourcecrateCWD; control0/1harnessred
   `born-child-cwd-harness-red.EsVn8w.log`, notbusiness. ExplicittempCWD/HOME/allXDG/
   TMP+assertion nowrequired; complete Source/native boundobjectpair reruns green.
   Twoowner exploratorygitcommands beforewrapper are uncited hygiene defects;
   their originalconsole output isn't claimed retainedverificationevidence.

No newdeps/unsafe/runtimePython/mutationbackend/Kernel/CLI/parser/locale/writer/
fixtures/goldens. NewFS/error/classification seams remain subject to freshreview.

## Stable verification / arithmetic

OwnerGit338=332+6controls; Op232=224+8. CoordinatorGit**343/0/7helpers**,27targets/
doctests (+3liveSourcepath +2observation/refpredicate positive controls). Op**233/
0/2helpers**,10targets/doctests (+1realSource non-heads capability classification).
Born childCWDassertionextends existingtest, no new count. Combined576/0/9helpers.

Stable owner-idle **2333pass/0fail/14parent-exercised helpers**,191suites/doctests;
Git343/Op233/Kernel199/doctrine175/charter80 INCLUDED notextracounts. Strictworkspace
all-target Clippy/fullfmt/bothreleaseCLIs+optimizedOplibgreen. Logs/index under
`/tmp/sk-jp-integration.dpvKNJ/command-log-index.tsv`: `stable-aggregate.yg1MRR.log`,
`stable-clippy.3rjeem.log`,`stable-fmt.pNirO7.log`,`stable-release.2axy1i.log`;
scoped`scoped-integrated-final.pL3J4T.log`,lockedoffline`locked-offline.c3r4qc.log`.
Exact53`oracle-final.4H5Ved.log`; completion599/PERF27/pending/validaccounting/0of16
unchanged (`completion-report.json`). Mainremote recheck `upstream-final.078w0R.log`;
source705gap unchanged. No inventory/workflow/measured-performance promotion.

## Current sha256 bindings (historical owner table not current)

```
22f7ae55ef0727b44a35c7f69cfb9da25015f33fa1e6b79e256d8cf93b597810  crates/sk-git/src/lib.rs
53d4e063dcef04a37ca08a2899746c792b07c9bd28ad9888859e288bbebc25dd  crates/sk-git/src/safe_commit.rs
18149f4ea0171d98c5aef16de3447af1432c473119849fef1d1464fdc073398b  crates/sk-git/src/safe_commit_paths.rs
edf6dfa6fabd13e0b86ddbb894e1645b175f78ddfa1f3f7b58d9384fe533218e  crates/sk-git/src/objects.rs
fe39c8e5c128f70628124a0c38ef0ca2a25498ea9ca0331d709aa18b0d54a156  crates/sk-git/src/refs.rs
f239a4992f903c37b989c2e36c345826d9343cf1f3aa4a37ddff77df17b3b7d3  crates/sk-git/tests/source_path_preflight.rs
ff5f4a8ee3450e4ac44ee1dbdd6b9ab86b150e1fd7ce3ade0009a2cceaac8d14  crates/sk-git/tests/source_path_oracle.rs
3e66fd35305a063ed190da908f5ba84db8c654d06a683cae64906d2cb67868b1  crates/sk-git/tests/symbolic_head.rs
3b1ef9c65b5a03afc6cf668ef8ae94f641999971cc5fa167591acc5549951ffc  crates/sk-invocation/src/lib.rs
8ebdbf17eed3cb3083a0ca3d9fce55935be694e3b0c1b7b425e7c84f155b3943  crates/sk-invocation/src/op_admission.rs
392714b0a1caea29ad40d2b5864c5fafbbbea688934deb7e82995403db5fc97f  crates/sk-invocation/src/op_commit.rs
a22433da45acacb6ec808742c2f6186fe4a7490587f2e6b22a2ed4073ead4aa7  crates/sk-invocation/src/op_target.rs
fb5dc992b1ce034f8fa26d82c074bff6dd82da406d1b2b09dad0f57450f35a7d  crates/sk-invocation/tests/op_admission_commit.rs
```

## Not closed / next required increments

Unborn root/absent-ref CAS and exact-existing-ref/non-heads publication; broader
sourcepath/IO/probe/error wording/abbrev fallback and nonUTF8 ref cases; SourceStage7
before8/index-on-encoding-failure/missingSPINE/NUL; native64KiBmessage/2000reflog/
processbounds; OSlocalearchives/othercodecs/nontextsettings/wideargv; fullProfile/
StepContract protected-mainfalseadvance/CLI/migrations/workflows/platform/upstream
semantic adoption/perf>=3xactualcampaign. Sourcepreflightallows is NOTsourcecommit
success; nativeunavailable/typeduncertainty is NOTparity. Coordinator commits+PUSHES
verified draftcheckpoint then freezesproduction for freshindependentJP/path review
before backendpublication/staging increments. No approvalbydisclosure.
