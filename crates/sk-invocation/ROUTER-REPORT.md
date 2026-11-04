> Historical owner delivery snapshot on dirty1514 (including two coordinator
> manifest edits), not current hashes or test locations. Original bytes/mtime:
> `/tmp/sk-router-integration.d6ZqNS/owner-delivery-report-original.md`.
> Coordinator made Unicode logic private and moved all five oracle tests into
> the crate; no test-only public module remains. Effective scan classes are
> decisive-Cased (`Cased && !Case_Ignorable`) and all Case_Ignorable, including
> 200 also-Cased points—not disjoint Unicode properties. Numeric facts/digests
> unchanged; generator docs/private symbols clarified and an independent suffix
> observation now checks all code points. Removed the incomplete profile-view
> Default (priority0, not Source50); no old parser semantics changed.
> Current gates/12 bindings: `docs/rust/reviews/action-router-disposition.md`.

# ROUTER-REPORT — FINAL (implemented, oracle-verified, frozen)

Status: **IMPLEMENTED and VERIFIED** for one bounded slice: the pure, I/O-free
`ActionRouter`, the canonical role-capability table it consumes, and the pinned
CPython-fact tokenizer it needs. Catalogue (`D2`/`D3`), executor, payload,
`StepContract`, CLI, model routing, glossary and governance text are **not**
implemented and are not claimed.

Repo: `/home/exedev/.t3/scratch/2026-11-02-pull-current-spec-kitty-from-442fd12a/spec-kitty-rust`
HEAD: `1514cf029eb9e8ffe0da8709b9ef381119b49cbd` (unchanged; nothing committed,
pushed, fetched, checked out, staged, reset or stashed by this task).

Provisional first: this report's predecessor was written at this path **before**
any probe or source analysis (only orders/grounding/profile documents had been
read). Its bytes and mtime are preserved outside the repo at
`/tmp/sk-router-1514/provisional/ROUTER-REPORT.PROVISIONAL.md`
(size 2418, mtime `2026-11-04 14:02:29.923546584 +0000`,
sha256 `db20b3098282f131a4fa1bd33f2c9d5759f952cadb908cdd85f9875b202e5bef`).

---

## 1. What was implemented

| File | Kind | Lines | sha256 |
|---|---|---|---|
| `crates/sk-invocation/src/router.rs` | new — router, tables, types, 11 unit tests | 956 | `1fbca24d0f141439a723dc07d178f033ed29dc22180eab8034d97137d0b6d225` |
| `crates/sk-invocation/src/router_unicode.rs` | new — lower/split logic, 3 unit tests | 227 | `cd4d7df48a4c90db44d68f6fd70518b88541b122ad96b201b913cfef70135499` |
| `crates/sk-invocation/src/router_unicode_data.rs` | new — generated CPython facts, 3 unit tests | 2972 | `41e8236a7627631778d7c46949e70e93891107e2be3eeb8b1d154b0e5a612f7a` |
| `crates/sk-invocation/tests/router_oracle.rs` | new — live Source oracle, 5 tests | 966 | `450a0b72980ef2db4068a962202795e5278655d2d3e66c6d9ee2bac9b5a9cc62` |
| `crates/sk-doctrine/src/role_capabilities.rs` | new — 8-row canonical table, 3 unit tests | 150 | `01069f82ab90f766ae399d1547cdc5d530757f167fd7c3839609b98230327cd3` |
| `scripts/rust/generate_router_unicode.py` | new — fail-closed generator | 564 | `ce5cc9179d0a3e8003d974b3386e1e11f07858a0582cf97ed38a473b65f13538` |
| `scripts/rust/tests/test_generate_router_unicode.py` | new — 11 generator controls | 194 | `bad80573b3739a315ed63a44d2bb763f3a70fdb21899b85e0e34cb300ef4886d` |
| `crates/sk-doctrine/src/lib.rs` | modified (+4/−0) | 159 | `3730a9b8159e8e71146c52ee3cb77001d97c7d75ac9999a243930d36e6aef1f5` |
| `crates/sk-invocation/src/lib.rs` | modified (+15/−3) | 108 | `bcc27cecbba7f1ed7a7a70e6ee6c0454967b46a4db3d82b8bd8027895033176e` |
| `crates/sk-invocation/src/legacy_json.rs` | modified (+106/−10, additive only) | 882 | `0f63e4ded7f66965f34a5beef790f42bda673dc452eb5178867798c64688c3e4` |
| `crates/sk-invocation/ROUTER-REPORT.md` | this report | — | — |

Coordinator-owned pre-existing edits, **not touched by this task** (verified by
`git diff`, log `orient-coor-edits.LyNk4E.log`): `crates/sk-invocation/Cargo.toml`
adds `sk-doctrine = { path = "../sk-doctrine" }`; `Cargo.lock` adds
`"sk-doctrine"` to the existing `sk-invocation` package. No manifest, lock or
dependency was edited here. No other producer, no commit, no push.

### 1.1 Public surface (`sk-invocation`)

`ActionRouter::route(&PythonString, Option<&PythonString>, &[RoutableProfile])`
plus `ActionRouter::route_scalars(&str, Option<&str>, …)`; `RoutableProfile`;
`RouterDecision`; `RouterAlternative`; `RouterConfidence`
(`Exact | CanonicalVerb | DomainKeyword | GenericFallback`, with
`as_str`/`from_source_str` round-trip — the router never *produces*
`GenericFallback`); `RouterError` (private fields, `code()`, `request_text()`,
`candidates()`, `suggestion()`, `message()` = `str(error)`); `RouterErrorCode`;
`STOP_WORDS` (45); `CANONICAL_VERB_MAP` accessors `canonical_verb_entries()` /
`canonical_verb()`; `normalize_tokens()`.
`PythonString` (the crate's single code-point string authority, now a public
seam: `from_codepoints` validated `0..=U+10FFFF` including surrogates,
`from_scalars`, `codepoints`, `to_scalar_string`, `repr`, `is_empty`,
`Ord`/`Display`-escaped-diagnostic) is re-exported with `InvalidCodepoint`.
`router_unicode` is `#[doc(hidden)] pub` **only** so the exhaustive oracle can
compare all 1,114,112 code points; it is documented as an implementation detail.

`sk-doctrine` exports `ROLE_CAPABILITIES` (8 rows: role, default capabilities,
canonical verbs, description), `RoleCapabilities`, `role_capabilities(&str)`
(exact match on already-normalised role), `role_default_verb(&str)`. The router
reads this table directly, so there is exactly one canonical capability
authority and no duplicate case-folding/profile-loader in `sk-invocation`.

### 1.2 Reference semantics reproduced (each oracle-verified)

1. Empty profile list is reported **before** hint resolution, with the
   empty-catalogue message.
2. `Some("")` is an explicit miss, not "absent"; `PROFILE_NOT_FOUND` suggestion
   is `Profile '<hint>' not found. Available: <python list repr>` with ids sorted
   and each element rendered by the one canonical `repr` formatter.
3. Explicit hint → `exact`; action = first canonical-verb token in the request,
   else the role capability's first verb, else `"review"` (this differs from the
   executor's role-default branch, which stays out of scope and is not merged).
4. Two tiers: canonical-verb candidates outrank domain-keyword candidates
   regardless of `routing_priority`; `routing_priority` breaks ties only within
   the selection pool (stable, higher wins) and appends
   `" (selected by routing_priority)"` to the reason.
5. Candidate order: verb matches by first matching token, then profile order;
   keyword candidates after, in profile order, de-duplicated by id; keyword pool
   = `specialization-context.domain-keywords` ++ `collaboration.canonical-verbs`
   in authored order, first match wins the reason; `kw.lower()` must equal a
   token (multi-word keywords never match).
6. `alternatives` = every non-winner from the **full** candidate list; an
   ambiguity error reports only the tied top candidates.
7. Primary role only (`roles[0]`), custom roles allowed (no native validity
   gate), no capability row → keyword/hint action `"review"`.
8. Exact error codes/messages: `ROUTER_NO_MATCH` (two distinct texts),
   `PROFILE_NOT_FOUND`, `ROUTER_AMBIGUOUS`; `str(error)` = `"<code>: <suggestion>"`.
9. Tokenisation = `re.split(r"[\s\W]+", text.lower())` minus the 45 stop words:
   `_` kept, Unicode alphanumerics kept, combining marks/emoji/punctuation/lone
   surrogates split, `lower()` (not casefold, not Rust's Unicode version).

All ids, roles, request text, keywords, match reasons, suggestions and error text
are **exact Python code points**: NUL and lone surrogates survive as data; no
Rust `Debug`, lossy `String` or JSON `serde` value is used as a wire form.
Actions are `&'static str` because every action comes from the closed tables.

---

## 2. Source chronology (what was measured, in order)

Every probe/gate ran through `/tmp/sk-evidence-run.sh` in a fresh root with
`env -i` and an isolated `HOME`/`XDG_*`/`TMPDIR`; every Cargo invocation ran
through `/tmp/sk-focused-cargo.sh` (2 jobs, no incremental, repo `target/`).
Roots: `/tmp/sk-router-1514/attempt-20261104T1404*` … `attempt-20261104T1416*`,
final chain `/tmp/sk-router-1514/final-20261104T141811Z`.

1. **Provisional report written** before any probe (`140229`, bytes+mtime
   preserved above).
2. **Oracle orientation** (`attempt…140529Z`): `verify_oracle.py` reports
   `{"ok": true, "python": "3.11.15", "checked_runtime_requirements": 53,
   "installed_count": 53, "drift": [], "missing": []}`; `sys.version_info[:3] ==
   (3, 11, 15)`, `unicodedata.unidata_version == 14.0.0`. `support::run_python`
   was read and re-checked, not assumed: it runs the same verifier, asserts the
   CWD and all six `HOME`/`XDG_*`/`TMPDIR` roots **inside** the interpreter, and
   fails closed.
3. **Source table/tokenisation facts** (`probe-tables`): 45 stop words, 59
   canonical verbs, the eight role rows, and the tokenisation vector table.
4. **Unicode facts, first reading** (`probe-tables` v1): a symmetric
   case-ignorable model produced `neither_count = 0`, which is impossible; real
   `str.lower()` spot checks (`"ΑΣ5".lower() == "ας5"`, `"5Σ".lower() == "5σ"`)
   refuted it. **Second reading** (v2): "after-scan stops only at a cased code
   point, before-scan skips case-ignorable" — which matched every
   single-character probe.
5. **Router fixture matrix** (`probe-router`, `probe-router2`): 54 real cases
   projected from the real `ProfileRegistry` (26 built-ins, activation subsets,
   a `languages: [python]` scope, and legacy `.kittify/profiles` documents whose
   ids/requests carry NUL and lone surrogates). This produced the oracle
   expectations used by the native test.
6. **Native implementation** (`router.rs`, `router_unicode.rs`,
   `role_capabilities.rs`, the `PythonString` seam, exports) and the generator.
7. **Native red #1 (business):** the exhaustive composed oracle rejected the v2
   sigma model at `"ΑΣ_Α"`: native produced `ασ_α`, the interpreter `ας_α`. The
   correct rule is the standard Unicode `Final_Sigma` with **both** clauses and
   **both** scans skipping case-ignorable code points (a non-ignorable suffix
   ends the after-scan *in favour of* final, it does not veto). Fixed in
   `final_sigma_context`, docs and generator narrative; `"ΑΣ.Α"` → `ασ.α` and
   `"ΑΣ5Α"` → `ας5α` now pin both directions.
8. **Native red #2/#3 (test-expectation errors, mine, not business):** an emoji
   was asserted to be a `\w` code point (it is not), and the surrogate-id fixture
   string was built with the wrong code points. Corrected; labelled here rather
   than presented as source regressions.
9. **Gate reds (style/build, not behaviour):** a `pub use query:{` typo and a
   `//!`-stripped doc line from my own edits (compile), `let [only]` dead-code
   metadata constants (would fail scoped Clippy `-D warnings`), rustdoc
   private-intra-doc links (`-D warnings`), ruff `SIM117` in the new Python
   test, and rustfmt instability of the generated table (the template now emits
   rustfmt-stable output; `cargo fmt --check` and `./generate_router_unicode.py
   --check` are both green on the same bytes).
10. **Final chain** (`final-20261104T141811Z`): all gates below, one unique log
    per command. Nothing was masked, tailed away, watched or polled; no process
    group was signalled; no fixture or log was deleted or reused.

No deliberately-wrong tokenizer was introduced to manufacture a pre-existing
business red (the grounding's §3.8 ASCII strategy was rejected as ordered). No
Native unsupported/refusal/ready/ID was invented to fake a baseline, and the
pre-change tree was **not** run, so no claim is made about it beyond the 76
`--lib` tests recorded in the grounding document.

---

## 3. Verification (final, one fresh root)

| Gate | Command (through `sk-focused-cargo.sh`/evidence wrapper) | Result |
|---|---|---|
| Oracle pin | `verify_oracle.py --root …` | exit 0, 3.11.15, 53/53, no drift |
| Generator table | `generate_router_unicode.py --check` | exit 0, `734 word ranges, 1432 lower singles, 1 expansions` |
| Generator controls | `python -m unittest …test_generate_router_unicode.py` | **11 tests OK** |
| Targeted Ruff | `ruff check <2 new python files>` | `All checks passed!` |
| Invocation tests | `cargo test -p sk-invocation --locked --offline` | **268 passed / 0 failed / 3 ignored** (93 lib + 4 + 67 + 21 + 30 + 15 + 4 + 2 + 5 + 26 + 1) |
| Doctrine tests | `cargo test -p sk-doctrine --locked --offline` | **178 passed / 0 failed** (124 lib + 18 + 21 + 13 + 2) |
| Strict scoped Clippy | `clippy -p sk-invocation -p sk-doctrine --all-targets -- -D warnings` | pass |
| fmt check | `fmt -p sk-invocation -p sk-doctrine -- --check` | pass |
| Strict scoped docs | `doc -p … --no-deps` with `RUSTDOCFLAGS=-D warnings` | pass |
| Dependent compile (courtesy, not an aggregate gate) | `check -p sk-cli --locked --offline` | pass |

The 3 ignored cases are the pre-existing parent-exercised subprocess helpers
(2 + 1); no required case is waived. `sk-invocation --lib` grew 76 → 93 (+17: 11
router + 3 tokenizer + 3 generated-table). `sk-doctrine --lib` grew 121 → 124
(+3); the 121 figure is arithmetic (124 − 3 new), not a baseline run.

### 3.1 Oracle test coverage (`tests/router_oracle.rs`, 5 tests)

* `oracle_is_the_pinned_interpreter_with_the_locked_closure` — exact 3.11.15,
  Unicode 14.0.0, `checked_runtime_requirements == 53`, empty drift/missing.
* `reference_tables_match_the_source_modules` — 45 stop words, 59 canonical
  verbs (token/action/role triples, both directions), 8 role rows with all four
  fields, and `role_capabilities()` resolving the identical row.
* `every_unicode_code_point_matches_the_pinned_interpreter` — **all 1,114,112
  code points** (surrogates included): `\w` membership, the exact `lower`
  mapping (including the `U+0130` two-code-point expansion), and both
  `Final_Sigma` scans; plus 2,000+ composed/deterministic sequences
  (`"ΑΣ5"`, `"ΑΣ_Α"`, `"ΑΣ.Α"`, `"ΑΣ\u0301Α"`, `"5ΣΑ"`, `"ΑΣ😀Α"`, `İ`, `ǅ`,
  math alphanumerics, U+10FFFF, lone surrogates, an LCG sequence).
* `every_router_case_matches_the_reference_decision_or_error` — all **54** cases,
  full fields and order: catalogue order, decision profile/action/confidence/
  reason, every alternative in order, error code/request/candidates/suggestion/
  `str(error)`.
* `token_normalization_matches_the_reference_vectors` — the reference
  `_normalize_tokens` vectors, including the surrogate/NUL vectors through the
  typed code-point path.

Generated table facts frozen in `router_unicode_data.rs`: 133,548 word code
points in 734 ranges; 1,432 single lower mappings, 1 expansion, 1,112,679
identity; 4,253 cased in 149 ranges; 2,602 case-ignorable-not-cased in 427
ranges; 1,107,257 neither. Digests: `word a878d650…f484`,
`lower 70f83b9d…c389`, `cased 1150b952…f628`, `ignorable dd8055f4…57ac`
(asserted from the tables in the generated unit test via
`sk_kernel::content_digest::sha256_hex`).

Orders file: `41641a3d7d57a945b4c5bc7efeca33b5723f82264c4328863bdcd3a3c0a2bdd7`
(26 lines).

---

## 4. Remaining APIs / open gaps (explicit non-claims)

* **Catalogue** (`D2`/`D3`): activation-gated doctrine + legacy `.kittify/profiles`
  overlay + language scope + `resolve_local` — not implemented here. The router
  takes a caller-supplied snapshot only; it reads no YAML, no ledger, no
  directory. `D2` (public lenient org-root resolver) and `D3` (legacy flat
  overlay) remain coordinator decisions.
* **Executor / payload / `dry_run` result / governance text+hash / glossary
  chokepoint / model routing** — untouched. The reference's executor
  explicit-hint branch (role-default action, `router_confidence=None`) is
  deliberately **not** merged into the router's token-derived hint branch.
* **`StepContract`, CLI wiring, `plan_advance → commit_advance` once-only
  advance, protected-main false advance** — untouched; a library router does not
  close them.
* **`ActionRouterPlugin`** — intentionally not ported (unused no-op stub).
* Not claimed: catalogue correctness, arbitrary-JSON parity, casefold (P5),
  number formatting (P6), encoding/ingestion (P7), nested governance, Doctrine
  inventory promotion, releases, publication, any workspace aggregate, any
  performance figure (no timing was measured; the oracle test's ~24 s is a debug
  build cost, not a benchmark), any platform/Widevine/OS-argv claim, the parent
  599/PERF-27/0-of-16/2447/19/194/Git-444/Op-246/690–14–40 counters.
* Unicode parity is exact for **this** slice (`\w` + `lower` + `Final_Sigma`
  over all code points, pinned to CPython 3.11.15 / UCD 14). It is not a
  casefold, normalisation, collation or regex-engine claim; a future Python/UCD
  bump is a deliberate generator re-pin (`EXPECTED_VERSION`/`EXPECTED_UNIDATA`)
  that fails closed today.
* No commit, push, fetch, checkout, rebase, reset, stash, subagent, owner
  contact, release, inventory promotion or protected-main advance was performed.
  Integration, aggregates, releases and publication remain the coordinator's.
