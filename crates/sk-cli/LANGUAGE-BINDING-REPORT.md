# Language CLI binding report (FINAL)

Status: **historical 1eb binding checkpoint**, not current composition approval.
The later independent review confirms the signal/P1 consolidation but finds F1–F4
composition/evidence defects. In particular the withdrawn cited-tactic assertion
was withdrawn for the wrong reason: skipped org graph loading, not renderer
breadth. Preserve the original sequence below; see
`COMPOSITION-REMEDIATION-REPORT.md` for corrected composition, non-vacuous
positive/missing-profile controls, BOM ownership and current bound hashes.

Task: bind the live CLI language signal to the canonical `sk_doctrine` language-scope
service (independent review P1), delete the CLI's second catalog/tier/interview
implementation, and replace the private JVM-family/sentinel duplicate in
`sk_doctrine::profile` with the canonical `language_scope` authorities. Functional port,
not a cybersecurity change; no new generic refusal and no success facade.

Published checkpoint: `5909598e12fcce111012030e5856addbee771207`
(clean `git status --porcelain` before work).

## Scope and result

Owned files actually changed:

| Path | Change |
|---|---|
| `crates/sk-cli/src/compat/runtime.rs` | Deleted the duplicate catalog / declared-language / placeholder / configured / compiled / interview implementation (file diff: +30/−312; the owner's 342 counted changed lines, not removals). `active_project_languages` is now an explicit `LanguageScopeService` adapter; `action_context` creates and owns one service/cache and passes the same resolved packs root used for context/profile composition. Service errors propagate. |
| `crates/sk-doctrine/src/profile.rs` | Deleted the private `jvm`-only family expansion and `"unknown"/"any"/"all"` literals. `applies_to_languages_match` now uses `language_scope::{RESERVED_LANGUAGE_TOKENS, SENTINEL_LANGUAGE_TOKENS, expand_language_families}`. `normalize_languages` remains the one normalization authority and matching semantics are unchanged. |
| `crates/sk-cli/tests/runtime_next.rs` | Added the red-first custom-Zig acceptance test, the Rust-vs-Zig change control, the declaration-removal control, and a live required-oracle CLI decision test. |
| `crates/sk-cli/tests/language_binding.rs` (new) | Live required-oracle coverage for the inference tiers and the canonical profile matcher, including `compiled=[]` vs `None`, configured override, compiled stringification, bracketed Python-literal answers, shipped-default/disk masking and generic/JVM matching. |
| `crates/sk-cli/LANGUAGE-BINDING-REPORT.md` (this file) | Provisional before work, finalized in place. |

Not edited: `language_scope` / `language_vocabulary` / `language_yaml` / `lib.rs`,
`sk-charter`, Kernel/Git/Op/runtime-store/mission/lifecycle, manifests, lockfiles,
dispatcher, capabilities, coverage. P2–P4 from the independent review were already
fixed at `5909598e1`; P5–P8 remain open below.

## Fixed hashes

| Path | sha256 before | sha256 after |
|---|---|---|
| `crates/sk-cli/src/compat/runtime.rs` | `17f5e1c1b294934f1171117b7345d4148115a67135afaf367c576ca3e0aefeab` | `ede445fc5c17eb239ad184fcaade48c2557223b861a45ed2684637a912bc20d2` |
| `crates/sk-doctrine/src/profile.rs` | `4f5db7a988eeb8974c7154dfece86e601d874e1d4ba2621736721764b94702aa` | `1322df5c70115046b6ce8979a59021369203118794df0b945394565f6f4ca365` |
| `crates/sk-cli/tests/runtime_next.rs` | `30972ba37c2d6d3b5c89f9814c3c01efab81debabb283b764e8e6770e6ed22b6` | `53a9cb037b27a3770a7a40fbbd063f9a63dc4a34effe952b6567750faf756f74` (coordinator comment-only correction; owner original `2b97b252…`) |
| `crates/sk-cli/tests/language_binding.rs` (new) | — | `aeee20b54be279ccd03969f18367b974d2c24356c2005e001548852920e6ac81` |

Frozen surfaces were byte-identical before and after the implementation owner's
work. Coordinator later updates only the public `lib.rs` documentation to describe
the binding; current doc-only `lib.rs` hash is
`668507ebccb910b474be5ae8d024a0d47e6dcd9b9476ed4052ef63b04d3ff168`.
The table below records the owner's frozen snapshot, not that later hash:

| Frozen path | sha256 |
|---|---|
| `crates/sk-doctrine/src/language_scope.rs` | `70ceaf687f3be01a20ad29c1033f8e6af5a3d1b02566f18b1578c1db5cb5dbb3` |
| `crates/sk-doctrine/src/language_vocabulary.rs` | `af2b62962b8956e0c2ef5738b9959e628de0a0667758fffb4b7c2dd9d4ebdf83` |
| `crates/sk-doctrine/src/language_yaml.rs` | `7e23e6764c634a761df0109368aeafb1e7c6adcb201b51059d6de19d9803953c` |
| `crates/sk-doctrine/src/lib.rs` | `3b0bb8f96fe2d8fbab19c238765c17228f7edf5653931d29fbc8c726024ca7d0` |

Pinned reference sources read (unchanged):

| Python source | sha256 |
|---|---|
| `src/charter/activation/language_scope.py` | `26007588f398af6e1e235640efa83aa339faf03b35297607eb016853cfc4b57c` |
| `src/charter/activation/language_authority.py` | `74c9968609cc33f3095aac61cbdcedae121779485dcf4b17cc630f3538b451e4` |
| `src/charter/activation/language_vocabulary.py` | `ccd757f89248c4a04d3dab39a6a759952b5decf4b13ecd924ac01642eab782be` |
| `src/charter/offering/shared/scoping.py` | `6487763526c6a6f7c8d4f87a2433848f585ecb53468cd9006bd5df2feda0ae27` |
| `src/charter/offering/shared/language_catalog.py` | `f406367210377f8ef862a59cc2698fcc1e9a702f68eb102aeea2037e4e53b440` |
| `src/charter/activation/charter_yaml_io.py` | `949200b380834e252eaded4bddb3b58f617de13d36b208de280c2e32df4a77dc` |
| `src/charter/activation/interview.py` | `7a0babf64312a443fc0b8ebbf69367fb4c7db1f716b873429f0603f77e6e7c27` |
| `src/charter/defaults.yaml` | `04f478ce866dff4f5fbc1e076a65366af7928c4fd2694715e6c6872692eaa269` |

## Red-first evidence (live CLI)

The acceptance test (`a_custom_zig_interview_admits_a_zig_scoped_profile`) drives the real
`spec-kitty next` entrypoint against a temporary project with an org pack containing a
Zig-scoped profile and a Rust-scoped sibling, with `WP01` pinned to
`agent_profile: zig-specialist`. It asserts the full authored WP frontmatter, project
metadata and the observable profile-scope diagnostics.

- **Genuine RED** — `/tmp/sk-lang-bind-red3.log`:
  `29 passed; 1 failed` — only `a_custom_zig_interview_admits_a_zig_scoped_profile`
  failed because the old CLI had no doctrine-derived vocabulary and returned `unknown`,
  excluding the Zig profile.
- **Test-authoring corrections, not product defects** — the first run
  `/tmp/sk-lang-bind-red.log` was `27 passed; 3 failed`. Two of those failures were my own
  test-expectation/fixture defects: (a) I asserted that a profile-cited org tactic would
  appear in the rendered `Tactic IDs`; after authoring the missing org fragment edge it
  still did not, so this prompt renderer does not expose that profile-channel citation for
  this shape and the assertion was withdrawn as outside the binding surface; (b) I
  asserted a profile-unresolved diagnostic that this renderer also does not expose. Both
  were test-expectation corrections, not production failures. The genuine behavioral red
  above remained.
- **GREEN** — `/tmp/sk-lang-bind-green.log` and the final combined run: the Zig profile is
  admitted under `Zig`, the Rust sibling is filtered; changing to `Rust` reverses the
  result; removing the declaration returns no signal and admits every scoped profile.

## Behaviour change (explicit)

The old CLI copy is deleted; the CLI now uses the canonical service, so these are real
changes, not a rename:

- `catalog.languages` items are `str()`-converted and never filtered, instead of dropping
  non-string/`null` items. The new oracle test pins `[12, null, ' ZIG ']` →
  `["12", "none", "zig"]`.
- Interview answers use the reference's Python-literal subset (`ast.literal_eval` shape)
  instead of parsing bracketed text as YAML, and a real declaration unions the
  doctrine-derived vocabulary. The red-first Zig case and the bracketed
  `['Zig', 'C++', None, 12]` oracle case pin this.
- The shipped-default interview no longer masks a real disk transcript; the
  configured → compiled → interview order and the explicit `[]` state are the canonical
  service's.
- A real service failure is propagated (governance block reports unavailable) instead of
  being silently converted to “no signal”. Valid/default projects never hit this path in
  the covered fixtures.

Profile matching semantics are preserved: unscoped profiles load, `None` admits every
scoped profile, explicit `[]`/`unknown` admits none, `any`/`all` remain unscoped
defenses, `unknown` is stripped from both sides, and `jvm` expansion remains one-way.

## Coordinator integration boundaries

The tier/matcher tests in `language_binding.rs` call the canonical library,
not the CLI adapter. The live CLI cases establish scope decisions, authored WP
fields and metadata; they do **not** prove exact full-prompt equivalence or cited
org-tactic delivery. Initial withdrawn assertions remain disclosed above, not
relabelled as passing evidence. Both contexts receive the same selected packs
override; resolver fallback/normalization and actual selected-profile delivery
still require independent integration checking.

Coordinator reproduces **2,112/0/9 helpers**,181 workspace suites including
doctests, strict all-target Clippy/fmt and both release builds after owners stop.
Logs `/tmp/sk-native-language-binding-stable-{aggregate,clippy,fmt,release}.log`.
Scoped doctests and full formatting pass again after the public-doc correction
(`/tmp/sk-language-binding-doc-{followup,fmt}.log`). Completion stays599/PERF27/
pending, accounting valid,0/16 workflows. Those gates
are not scoped independent approval or compatibility/performance promotion.

## Gates (all through `bash /tmp/sk-focused-cargo.sh`)

| Gate | Result | Log |
|---|---|---|
| `cargo test -p sk-cli --lib --test runtime_next --test language_binding --test compat --locked --no-fail-fast` | **69 passed / 0 failed** (runtime_next 31, lib 9, compat 26, language_binding 3) | `/tmp/sk-lang-bind-cli-final.log` |
| `cargo test -p sk-doctrine --locked --no-fail-fast` | **174 passed / 0 failed** (unit 120 + baseline 18 + governance 21 + language oracle 13 + doctests 2) | `/tmp/sk-lang-bind-doctrine-final.log` |
| `cargo clippy -p sk-cli -p sk-doctrine --all-targets --locked -- -D warnings` | pass | `/tmp/sk-lang-bind-clippy-final.log` |
| `cargo fmt -p sk-cli -p sk-doctrine -- --check` | pass | `/tmp/sk-lang-bind-fmt-final.log` |
| Required oracle identity (`verify_oracle.py`) | **53/53 requirements, ok, Python 3.11.15, no missing/drift** | `/tmp/sk-lang-bind-oracle-verify.log` |

The new oracle tests select `.venv-rust-oracle/bin/python` through the shared
fail-closed `oracle_support` (missing or lock drift fails, never skips) and run with the
fixture CWD plus absolute `PYTHONPATH=<repo>/src` and
`SPEC_KITTY_PACKS_ROOT=<repo>/packs`. Every probe used an isolated temp project/HOME.
No aggregate, release, workspace-wide, `cargo clean`, cache deletion, manifest/lockfile,
dispatcher or capability change was made.

## Remaining gaps (not claimed closed)

- **P5** — Python `casefold` vs native `to_lowercase` for non-ASCII doctrine tokens.
- **P6** — Python shortest-repr float exponent (`1e+20`) in the compiled tier.
- **P7** — UTF-16/ambiguous-encoding interview transcripts (native reads UTF-8 only).
- **P8** — coordinator updates README/public module docs/status to describe the
  binding; historical pre-binding reports keep their checkpoint scope. Stale
  test comments about cited-tactic delivery are corrected, not used as evidence.
- No claim of whole CLI/`next`/language parity, StepContract execution, or
  protected-main false-advance repair. This tranche binds one real signal; it does not
  repair the separate runtime gaps recorded in `docs/rust/VERIFICATION.md`.
