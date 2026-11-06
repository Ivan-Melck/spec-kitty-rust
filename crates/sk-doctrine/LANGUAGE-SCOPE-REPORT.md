# Native language-scope service — implementation report (FINAL)

Status: **historical 170-test delivery reviewed before coordinator P2–P4 fixes**.
Current bindings/gates are in `LANGUAGE-REMEDIATION-REPORT.md`; this report's
source table is the independently reviewed input, not current source hashes.
The provisional version of this file was written before any code existed; this
version records what actually landed, the real first failures, the exact counts and hashes, and
the remaining limits.

Task: add the additive, read-only, typed **language-scope service** to `sk-doctrine`
(`scoped_language_vocabulary` + `infer_repo_languages`) — the missing doctrine-side provider
behind review item **N5**. **No CLI wiring was done.** The CLI detector
(catalog/configured/compiled/interview logic in `crates/sk-cli/src/compat/runtime.rs:1448-1760`)
is untouched and still resolves a custom `zig`
declaration to `unknown`, so **N5 is not fixed end-to-end**; the coordinator binds it only after
review.

## 1. What landed

| Path | State | sha256 |
|---|---|---|
| `crates/sk-doctrine/src/language_vocabulary.rs` | NEW | `b6feb7da5917f59ae7d9cdb67fa57ef832daa1e761ace2e8ef14eaae37983d24` |
| `crates/sk-doctrine/src/language_scope.rs` | NEW | `da12ab4a39352cd9adad2ff7541d32102191fd771c96dfd14c98977e04851cec` |
| `crates/sk-doctrine/tests/language_scope_oracle.rs` | NEW | `53a939cf57495b21d3b6b2cc06a60c6e6bb4a66f4709478939465d2104ad7ffa` |
| `crates/sk-doctrine/src/lib.rs` | additive `mod` + `pub use` + one doc bullet | `523863592cf5f558f15e70c77a63fe038b71cebb3db8a8fb1bbe46ad58435b17` |
| `crates/sk-doctrine/README.md` | new section + updated counts + gap 15 corrected | `6ff5c116aa91135a6bc74c7102dba2a67d1a31f4272d5df1d020d5c3ce4c6cff` (coordinator count correction; owner's original was `e7b3a138…`)  |
| `crates/sk-doctrine/LANGUAGE-SCOPE-REPORT.md` | this report (hash is self-referential, so omitted; the provisional hash was `1a4e2ebb8eb6d937e916ecdbbb84305e8fa19a0a7a452e46898b878fc976e6ec`) | — |

Repository diff for this scope (`git diff --stat crates/sk-doctrine`): **README.md +78/−7, src/lib.rs
+23/−1**, plus the four new files. The `lib.rs` diff is additive only — two `mod` lines, one
documentation bullet, two re-export blocks; no existing item, signature or behaviour changed.

Not touched: `sk-charter`, `sk-cli`, `sk-runtime`, `sk-kernel`, `sk-invocation`, `sk-mission`,
`sk-run-store`, `sk-workspace`, `sk-status`, `sk-glossary`, `sk-install`, `Cargo.toml`,
`Cargo.lock`, dispatcher/capabilities, agents, commits, `docs/rust/**`.

Coordinator clarification: the owner's original empty-dependency-diff statement is
**not a current freeze proof**. Integration includes coordinator Kernel/Op N1–N4
follow-ups (`crates/sk-invocation/NARROW-REMEDIATION-REPORT.md`), plus ignore-probe
and documentation changes. The language owner did not edit those surfaces. The
review binds current files/dependencies explicitly; the new Kernel reference API
is not called by the additive language service. No global clean-tree claim.

## 2. Canonical sources read in full

Reference hashes (sha256) at rewrite HEAD `38834239dcccf6171f7f56c0d62036117d11cb4c`
(the Python source remains byte-identical to `9aed3a8012cb5eb22c0128b14d525039f95fea7a`):

```text
ccd757f89248c4a04d3dab39a6a759952b5decf4b13ecd924ac01642eab782be  src/charter/activation/language_vocabulary.py
26007588f398af6e1e235640efa83aa339faf03b35297607eb016853cfc4b57c  src/charter/activation/language_scope.py
74c9968609cc33f3095aac61cbdcedae121779485dcf4b17cc630f3538b451e4  src/charter/activation/language_authority.py
6487763526c6a6f7c8d4f87a2433848f585ecb53468cd9006bd5df2feda0ae27  src/charter/offering/shared/scoping.py
f406367210377f8ef862a59cc2698fcc1e9a702f68eb102aeea2037e4e53b440  src/charter/offering/shared/language_catalog.py
96bd281c67ba32836d1b72dc5e1fa621b7cc8b568b104b5948885c4cb55f65b4  src/charter/offering/artifact_kinds.py
0efae41bbe8862e6236bc49c2da744e45b5993ac14d102e9ac224787268e8a01  src/charter/activation/_project_root_candidates.py
147294e353f872aaf0caea7ec471cf4fd7fef8df04d984a0d984c5ba274e256e  src/charter/activation/layer_roots.py
a7a21530ab907a7b1efc6647b0e2309ec62b5dd44f4e902ff65a0e4c0faaead1  src/charter/offering/pack_paths.py
949200b380834e252eaded4bddb3b58f617de13d36b208de280c2e32df4a77dc  src/charter/activation/charter_yaml_io.py
7a0babf64312a443fc0b8ebbf69367fb4c7db1f716b873429f0603f77e6e7c27  src/charter/activation/interview.py
04f478ce866dff4f5fbc1e076a65366af7928c4fd2694715e6c6872692eaa269  src/charter/defaults.yaml
1a7a2d2572f74586d2ae7d5b86ad5d1e4e0b62a5d8fea44a3ba8813dbf2221e5  src/charter/offering/drg/org_pack_config.py
```

Cross-referenced native seams (reused unchanged): `sk_charter::{ArtifactKind, BuiltInSearch,
PackRoots, load_project_config, PathExpansion, Limits, CHARTER_YAML}`,
`sk_doctrine::{org_charter::{built_in_search, normalize_packs_root}, profile::normalize_languages,
DoctrineError}`. Source hashes of the files this task edited before/after: `lib.rs`
`6b0a70bf…` → `52386359…`; `README.md` `812f07e5…` → reviewed `6ff5c116…` (the owner's `79b456bf…` pre-image was incorrect); every other
`crates/sk-doctrine` file is byte-identical (no `git diff`).

## 3. Typed API

`language_vocabulary`:

```rust
pub const SCOPE_FIELD: &str = "applies_to_languages";
pub const LANGUAGE_SCOPED_KINDS: [ArtifactKind; 5];   // tactic, styleguide, toolguide, procedure, agent_profile
pub const DEFAULT_CACHE_CAPACITY: usize = 8;

pub fn scoped_language_vocabulary(repo_root: Option<&Path>, packs_root: Option<&Path>)
    -> Result<BTreeSet<String>, DoctrineError>;

pub struct LanguageVocabularyCache;                    // explicit, caller-owned LRU
impl LanguageVocabularyCache {
    pub fn new() -> Self;  pub fn with_capacity(usize) -> Self;
    pub fn capacity(&self) -> usize;  pub fn len(&self) -> usize;  pub fn is_empty(&self) -> bool;
    pub fn clear(&mut self);
    pub fn keys(&self) -> impl Iterator<Item = (Option<&Path>, Option<&Path>)>;
    pub fn contains(&self, Option<&Path>, Option<&Path>) -> bool;
    pub fn vocabulary(&mut self, Option<&Path>, Option<&Path>)
        -> Result<Arc<BTreeSet<String>>, DoctrineError>;
}
```

`language_scope`:

```rust
pub struct LanguageScopeService;   // owns a LanguageVocabularyCache
impl LanguageScopeService {
    pub fn new() -> Self;  pub fn with_vocabulary_cache(LanguageVocabularyCache) -> Self;
    pub fn vocabulary_cache(&self) -> &LanguageVocabularyCache;
    pub fn vocabulary_cache_mut(&mut self) -> &mut LanguageVocabularyCache;
    pub fn clear_vocabulary_cache(&mut self);
    pub fn scoped_language_vocabulary(&mut self, Option<&Path>, Option<&Path>)
        -> Result<Arc<BTreeSet<String>>, DoctrineError>;
    pub fn infer(&mut self, &InferRequest<'_>) -> Result<InferredLanguages, DoctrineError>;
    pub fn lacks_specialist_guidance(&mut self, Option<&[String]>, Option<&Path>, Option<&Path>)
        -> Result<bool, DoctrineError>;
}

pub struct InferRequest<'a> { repo_root, interview, prefer_interview, packs_root }
pub enum InferredLanguages { NoSignal, Empty, Declared(Vec<String>) }   // None / [] / ["unknown"]
pub struct InterviewAnswers;   // str()-converted answers, file order
pub fn read_interview_answers(&Path) -> Option<InterviewAnswers>;

// free (one-shot, uncached) equivalents + the pure authorities
pub fn infer_repo_languages(&InferRequest<'_>) -> Result<InferredLanguages, DoctrineError>;
pub fn lacks_specialist_guidance(Option<&[String]>, Option<&Path>, Option<&Path>) -> Result<bool, _>;
pub fn declared_languages(&str) -> Vec<String>;          // + extract_declared_languages alias
pub fn configured_languages(&Path) -> Option<Vec<String>>;
pub fn expand_language_families<I: IntoIterator<Item = S>, S: AsRef<str>>(I) -> Vec<String>;
pub fn is_catalog_language(&str) -> bool;  pub fn is_language_family(&str) -> bool;
pub fn is_placeholder_language_answer(&str) -> bool;
pub const LANGUAGES: [LanguageSpec; 10];  pub const BUILD_TOOLS: [BuildToolSpec; 3];
pub const SHIPPED_DEFAULT_ANSWERS: [(&str, &str); 11];
pub const UNKNOWN_LANGUAGE / RESERVED_LANGUAGE_TOKENS / SENTINEL_LANGUAGE_TOKENS / JVM_FAMILY /
           CONFIG_LANGUAGES_KEY / CONFIG_RELATIVE_PATH / INTERVIEW_ANSWERS_RELATIVE_PATH /
           LANGUAGES_ANSWER_KEY / SHIPPED_DEFAULT_LANGUAGES_ANSWER / COMPOUND_PREFIX_GUARD;
```

Normalisation is **reused** (`crate::profile::normalize_languages`). Family expansion is
re-implemented in the new module because `profile::expand_language_families` is private and
`jvm`-specialised; `profile.rs` was deliberately left byte-identical.

## 4. Semantics, and what the live oracle verified

**Vocabulary.** Union of `applies_to_languages` over exactly the five scoped kinds, scanned from
`packs/built-in/<plural>`, the project layer (`resolve_project_root`: `.kittify/charter-packs`,
then the code-local `src/charter/offering` candidate; the retired repo-root `doctrine/` is **not**
a candidate) and every *existing* configured org pack root in both spellings
(`<plural>` **and** the project-tier directory). Only single `\w`-word string items become
tokens; tokens are normalized and `unknown`/`any`/`all` are stripped. The org registry read is
the lenient one the scan uses in the reference (`resolve_pack_chain(strict=False)`): a malformed
registry contributes nothing, a declared-but-missing root is dropped, and a `local_path` with an
unset variable is a typed failure (as the reference raises). No graph, no activation gate, no
invented confinement or refusal.

**Memo.** `LanguageVocabularyCache` is an explicit LRU bounded to 8 roots, MRU-first, with
`clear()` — the same bound and eviction policy as `functools.lru_cache(maxsize=8)`. Two declared
differences: the cache is caller-owned instead of a process global, and the explicit
`packs_root` seam is part of the key (the reference keys on `repo_root` alone because its packs
root is ambient). A failed scan is never cached.

**Inference.** Operator `languages` (validated against catalog tokens plus the `jvm` family, so a
confirmed list is never widened) → compiled `catalog.languages` (present-but-empty is
authoritative `[]`; missing/null/non-list falls back; items `str()`-converted and never filtered)
→ interview (`prefer_interview` resolves from the interview alone, supplied or on disk, and may
return no signal; a supplied interview that is only shipped-default text does not mask the disk
transcript). No `charter.md` fallback. A real declaration = built-in detector ∪ doctrine-vocabulary
whole-word hits, else `["unknown"]`; placeholders/default/absent declarations scan the other
answers with the built-in detector only and return no signal when nothing is found.

Oracle-observed values asserted in `tests/language_scope_oracle.rs` (all reproduced natively, both
sides exact):

- Shipped built-in vocabulary (no overlay): `["css","html","java","javascript","jvm","php",
  "python","twig","typescript"]` — no `unknown`/`any`/`all`.
- D2 guidance with that vocabulary: `None → false`, `[] → false`, `["unknown"] → true`,
  `["python"] → false`, `["rust"] → true`, `["python","unknown"] → false`, `["jvm"] → false`,
  `["kotlin"] → false`, `["java"] → false` (the last three exercise family expansion).
- Project overlay `tactic/zig-tactic.tactic.yaml` scoped `[zig]`: vocabulary gains `zig`;
  `infer(interview "languages_frameworks: zig") = ["zig"]`, `"Python and zig" = ["python","zig"]`,
  same under `prefer_interview`. Removing that file returns `["unknown"]` and the overlay token
  disappears from both sides (non-vacuous control).
- Org pack with `tactics/…[zig]`, `tactic/…[zigzag]` and `agent_profiles/…[zigpro]`: all three
  spellings contribute; deleting the plural tactic removes only `zig`; deleting the org root
  removes all three.
- Kind globs and unusual literals: only the five kinds and their own extension patterns count
  (`wrong.styleguide.yaml` in `tactic/`, a `directive/` artifact, and a `styleguides/` project
  directory — the project overlay spelling is singular for styleguide — contribute nothing);
  nested directories and dot-prefixed filenames do; malformed YAML, non-mapping YAML, a scalar
  scope, a `comment`-only mention, a non-UTF-8 file and a directory named `*.tactic.yaml` are all
  skipped; `[zig, "Python ", 12, null, true, "a b", "", "c#", "c++", "objective-c", "MIXED",
  "any", "ALL", "unknown"]` yields exactly `{zig, python, mixed}`.
- Declared detector: `"Python services with pytest and ruff. TypeScript … tsc." =
  ["python","typescript"]`, `"Java-based backend" = ["java"]`, `"Java 21 with Gradle" =
  ["java"]`, `"Kotlin with Gradle" = ["kotlin"]`, `"Gradle" = ["jvm"]`, `"maven" = ["jvm"]`,
  `"librespot-java" = []`, `"my-python-thing" = []`, `"React-TypeScript" = []`,
  `"node.js" = ["javascript"]`, `"node.jsx" = ["javascript"]`, `"C#" = []`, `"C++" = []`.
- Placeholders: `""`, `"[]"`, `"-"`, `"N/A"`, `"na"`, `"none"`, `"null"`, `"TBD"`, `"any"`,
  `"ALL"`, `"unknown"`, `"language-agnostic"`, `"language agnostic"`,
  `"[NEEDS CLARIFICATION: which?]"` and the shipped default text are `true`; `"zig"` is `false`.
- All 30 inference cases match exactly: configured-wins-over-everything (including
  `prefer_interview`), configured entry validation/dedup/first-seen order, empty and
  all-invalid configured lists, malformed and non-list configured files, compiled-first,
  compiled `[]` → `[]`, compiled null/non-list/malformed → interview fallback, compiled items
  `str()`-converted (`[12, null, " Zig "]` → `["12","none","zig"]`), `prefer_interview` in
  memory/on disk/absent, shipped-default not masking disk, empty supplied interview deferring to
  disk, no inputs → `None` (with a `charter.md` decoy), shipped-default-only → `None`,
  placeholder `prefer` → `None`, list-repr answers (`[Zig, C]` → `["unknown"]`, `[Python, Zig]`
  → `["python"]`, `['unknown']`/`[N/A]` → `None`), numeric answer `3` → `["unknown"]`, YAML
  `null` answer → placeholder → other-answer scan, `"n/a, none; tbd"` → `None`,
  `"n/a, Zig"` → `["unknown"]`.

## 5. Gates reproduced (all through `bash /tmp/sk-focused-cargo.sh`)

| Gate | Result | Log |
|---|---|---|
| `cargo test -p sk-doctrine --locked` | **170 passed, 0 failed, 0 ignored** — 120 unit + 18 `baseline_fixtures` + 21 `governance_oracle` + 9 `language_scope_oracle` + 2 doctests | `/tmp/sk-language-scope-doctrine-test.log` |
| `cargo clippy -p sk-doctrine --all-targets --locked -- -D warnings` | **pass** (exit 0) | `/tmp/sk-language-scope-clippy.log` |
| `cargo fmt -p sk-doctrine -- --check` | **pass** (exit 0) | `/tmp/sk-language-scope-fmt.log` |
| `cargo check -p sk-cli -p sk-install --locked` | **pass** — the additive API breaks no dependent (`sk-cli`, `sk-install`, and their transitive `sk-mission`/`sk-git`/`sk-workspace`/`sk-runtime`/`sk-run-store`/`sk-glossary`) | `/tmp/sk-language-scope-dependents-check.log` |

The pre-existing doctrine suite is intact: 108 unit + 18 + 21 + 2 = **149** all still pass; the
new evidence is 12 unit + 9 oracle tests = **21** additive tests (170 total; owner's 169 was an arithmetic error, corrected by summing the five recorded suite results). No aggregate,
release, workspace-wide or `cargo clean` command was run. `sk-git` changes in the working tree are
the parallel ignore-probe scope; the dependent check compiled them as-is and did not edit them.

New unit tests (12): `language_vocabulary::{cache_is_bounded_to_the_reference_size,
word_tokens_exclude_non_words, reserved_and_sentinel_tokens_are_never_vocabulary}`,
`language_scope::{declared_languages_follow_catalog_rank_and_the_build_token_rule,
families_expand_one_way_only, placeholders_cover_the_reference_set,
answer_items_parse_a_python_list_repr, real_parts_drop_placeholders_and_split_on_separators,
word_tokens_apply_the_compound_guard_and_hash_plus_exclusion,
python_value_rendering_matches_str_for_the_shapes_that_matter,
packaged_default_interview_is_detected, shipped_defaults_match_the_reference_file}`.

New oracle tests (9): `shipped_vocabulary_and_guidance_match_the_oracle`,
`project_overlay_scope_union_and_removal_match_the_oracle`,
`org_chain_scopes_both_spellings_and_removal_match_the_oracle`,
`kind_globs_and_unusual_scope_literals_match_the_oracle`,
`project_layer_candidates_match_the_oracle`,
`declared_languages_and_placeholders_match_the_oracle`, `inference_tiers_match_the_oracle`
(30 sub-cases), `vocabulary_cache_is_bounded_memoised_and_invalidatable`,
`inferred_languages_preserve_the_three_states`.

## 6. First failures actually hit (in order)

1. `cargo check -p sk-doctrine`: unused `BUILT_IN_DIR` const in `language_vocabulary.rs` (the
   leaf normalisation already lives in `org_charter::normalize_packs_root`) → removed.
2. `clippy -D warnings`: `redundant_closure` on `.is_some_and(|c| is_word_char(c))` → passed the
   function directly; later `len_zero` on `guidance_cases.len() > 0` → `!is_empty()`.
3. `cargo test --lib` reddened **on my own wrong expectations**, not on product behaviour:
   `answer_items("('Zig',)")` is *not* a bracketed list repr, so the reference (and native) keep
   the raw text; `"maven and rust"` sorts by `declared_rank` (`["rust","jvm"]`, not
   `["jvm","rust"]`); `word_tokens` receives already-lowercased text; and `word_tokens("java-
   based")` is only `["java"]` because the `(?<!\w-)` guard also drops the tail word.
4. Oracle integration test, first run: three of my test expectations were wrong, and each failure
   was informative rather than a product bug — (a) the project overlay's styleguide directory is
   the **singular** `styleguide/` (`PROJECT_KIND_DIRS`), so a `styleguides/` file is correctly
   invisible on both sides (now asserted as a negative control); (b) my org fixture's agent
   profile also scoped `zig`, so deleting the tactic could not remove `zig` (fixture token
   renamed to `zigpro`); (c) a *missing* packs root does **not** fail, because
   `sk_charter::BuiltInSearch` deliberately warns and falls back to the ancestor walk — the
   "failed scan is not cached" test now uses a real typed failure (an org `local_path` with an
   unset environment variable, which the reference also raises).
5. `cargo fmt --check`: only the two new files needed rustfmt; they were formatted with
   `rustfmt --edition 2024` on those files only (no package-wide/global reformat).

No test was weakened to get green, and no crate source was edited to satisfy an assertion. Every
comparison above is against the live lock-exact oracle on the same fixture.

## 7. Remaining limits (declared, not hidden)

- **N5 is open end-to-end.** The CLI's separate catalog/configured/compiled/interview implementation still does not call this service;
  a scoped custom `zig` declaration still resolves to `unknown` in `compat/runtime.rs`. Binding
  the provider (and the prompt/registry consumers) is a separate, unreviewed step.
- `str.casefold()` is approximated with `to_lowercase()` (ASCII-identical; `ß`/final-sigma edges
  differ), and Python `\w` with `char::is_alphanumeric() || '_'` (category edges such as some
  marks/numbers are not proven identical).
- The `ast.literal_eval` subset covers list/tuple/dict/scalar shapes. Float and exotic reprs are
  best-effort (integral floats keep `.0`; shortest-repr exponent spelling like `1e+20` is not
  reproduced), and set syntax / unparsable text fall back to the raw answer exactly where the
  reference falls back. `str()` conversion of YAML values mirrors Python for strings, ints,
  `None`/`True`/`False`, lists and mappings.
- Reference logging (`configured_languages` drops, unreadable org registry) is value-identical
  but not emitted natively.
- Org `subdir` resolution for a **not-yet-materialised** pack root uses the lexical join
  (`base.join(subdir)`) because the reference resolves with `strict=False`; for existing roots it
  goes through `sk-charter`'s containment. Symlink-escape behaviour for that missing-base case is
  therefore not proven.
- Path comparison uses `Path::exists()` as the reference does; Windows path/spelling semantics
  and non-UTF-8 *file names* are untested (POSIX only). Non-UTF-8 artifact *content* is skipped like the reference; interview
  transcripts are different: Python charter ingestion detects UTF-16 and other
  encodings and can propagate decode/ambiguity errors, while this delivery's
  reader returns no signal. P7 remains a real fidelity gap.
- The native cache key includes the packs-root seam (deliberate, declared) and belongs to the
  caller; no performance/3× or memory claim is made, and no benchmark was run.
- Unicode/`\w`/casefold divergences are the only known sources of possible token-level
  disagreement; all ASCII and documented cases are oracle-exact.

## 8. Explicit non-claims

This task does **not** claim: N5 end-to-end correctness or a fixed CLI detector; any
governance/invocation/StepContract/prompt-path parity; aggregate or release green; workspace
parity; a performance, coverage or evidence promotion; a new gate; or any approval of the frozen
Kernel/Git/Op production code. The 170 tests and 30 oracle sub-cases are scoped evidence for this
additive service only, and remain subject to the coordinator's independent review.
