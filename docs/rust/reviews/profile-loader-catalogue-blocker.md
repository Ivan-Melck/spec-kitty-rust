# Catalogue prerequisite: Source-valid profile dropped by native loader

Fixed source at `bc810439541219b178187db59a6ee5e21fb0648a` (router6b1 source
bytes unchanged). This is a NEW Source-first loader counterexample, not a router
finding or a catalogue implementation. Provisional catalogue assessment
`/tmp/sk-invocation-catalogue-coordinator-plan.md` preceded probes/source analysis.

## Actual Source-first evidence

Evidence `/tmp/sk-catalogue-assessment.YUZxiQ/command-log-index.tsv`.
Exact CPython3.11.15/53 verified (`exact-oracle.Zh0i5p.log`). A public-native-API
probe was compiled from the current sk-doctrine through the focused Cargo lease
with an absolute repository target directory (`build-loader-probe.QzS99K.log`).

Same authored file under an actual project profile directory:

```yaml
profile-id: "author\ud800"
name: Source-valid profile
roles: [reviewer]
purpose: Preserve authored routing identities.
specialization:
  primary-focus: Review
collaboration:
  canonical-verbs: [check]
specialization-context:
  domain-keywords: [widget]
```

Pinned Source `AgentProfileRepository(built_in_dir=.../agent_profiles,
project_dir=.../agent_profiles)` loads and asserts the exact id code points
`[97,117,116,104,111,114,55296]` BEFORE Native starts. Source keeps27 profiles;
Native `ProfileRepository::load` keeps26 and records
`YAML error: found invalid Unicode character escape code ...` for the valid
profile. Source's rich Pydantic model and loader actually admit it; it is not a
monkeypatched registry or invented malformed input. Source and Native processes
assert actual temporary CWD/HOME/allXDG/TMP, and no repo profile was edited.

`source-first-loader-gap.XzVTeS.log` is a successful DIAGNOSTIC of this mismatch,
not a parity pass or original failing gate. Fresh strict equality run
`source-first-valid-profile-business-red.cJBO3U.log` fails after Source's exact
profile assertion: `Source-valid authored profile silently dropped`.
No production fix has been applied; every fixture/log retained.

## Why catalogue composition must not hide this

- Existing native `AgentProfile.id: String` / `body: serde_yaml_ng::Mapping`
  cannot represent every Source Python string. Native JSON/router now has one
  exact `PythonString`, but it currently lives in sk-invocation; sk-doctrine
  cannot depend back on sk-invocation (the approved edge points the other way).
- A second YAML parser or codepoint string inside the catalogue would duplicate
  existing authorities. Escaped/PUA fake IDs, lossy Unicode, silent skipping,
  new hardening refusals, a runtime-Python fallback or claiming pre-existing P7
  as approval are NOT accepted fixes.
- Lenient org-root reuse (D2) and flat legacy-layer reuse (D3) remain needed.
  Legacy overlay is ungated AND language-unfiltered and wins collisions; normal
  doctrine profiles still use activation/language/required-floor authority.
- Source model normalization/defaults and all authored fields must be retained;
  a routing-only projection must not be misrepresented as full ProfileRegistry.

Next bounded work is to establish a viable shared-loader/codepoint transport
using existing native parser/representation authorities, with unchanged scalar
API semantics, no dependency cycle and a real Source-first POC. No full generic
YAML rewrite, public-shape break, new dependency or production implementation is
authorized by this note. The existing private lenient root helper and profile
loader need concrete reuse, not parallel policy. The router's independent
NARROW PASS remains intact; catalogue/executor/StepContract/CLI/workflow and
599/PERF27/0-of-16/performance gates remain open.
