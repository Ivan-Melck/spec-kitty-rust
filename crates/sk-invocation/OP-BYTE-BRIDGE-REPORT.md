# Op byte-message / late native-target bridge — implementation report

Status: **historical first-owner snapshot**, 195 scoped tests in the unpublished
dirty a6a tree, NOT coordinator verification. Four integration regressions were
subsequently found/fixed; current hashes/counts/limits are in
`OP-BYTE-BRIDGE-INTEGRATION-REPORT.md`. Not an acceptance verdict, not whole-Op/CLI/J-P/publication approval,
and not a waiver for the source behaviours listed as remaining work.

Evidence root: `/tmp/sk-op-byte-bridge.aQ0tLd` — one fresh `mktemp` log per
attempt, including failures, indexed in `command-log-index.tsv`. Baseline commit
`a6a2c0cf100c178a936b2d3c2974b1f22658a53b` (clean; `origin/main`
`de5f4dc5f374170226a5331129c80927946e5d64`). Source/oracle pinned
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`, CPython 3.11.15, exact 53 runtime
requirements re-verified at the final tree
(`final-oracle.2jFIzg.log`).

## What actually changed

Production (private seam; public `OpCommitService` API unchanged):

* `src/op_commit.rs` — the close path now keeps the message as Python **code
  points** (`op(<profile>): <action> [<8>]( (doctor sweep))`) and runs: started
  row → path+code points → worktree/branch → skip → detached → roots →
  protection/config → **message-free `SafeCommitTargetRequest` plan** (paths,
  HEAD/tip, canonical protection guard, operation marker) → **late** filesystem
  encoding of the whole message → `SafeCommitTargetPlan::prepare(CommitMessage)`
  (existing native payload guard: empty/oversized/NUL) → one
  `apply_safe_message_commit`. The one request builder is
  `build_safe_commit_target` (destination, exact path, protection union,
  `STANDARD`), shared with admission; no placeholder message reaches the
  backend. Error→outcome mapping (protected refusal, landed/unconfirmed
  evidence, best-effort text) is the previous mapping, factored once.
* `src/python_os_encoding.rs` (**new**, private) — the one Op/application-layer
  POSIX `fsencode`/`surrogateescape` authority. Captures the startup context
  once per service from `PYTHONUTF8`, `PYTHONCOERCECLOCALE` and the
  `LC_ALL > LC_CTYPE > LANG` chain (PEP 540 auto UTF-8 mode, PEP 538 coercion
  skipped when `LC_ALL` is set). `\udc80..\udcff` → raw `0x80..0xff`; UTF-8
  scalars encoded; ASCII context refuses non-ASCII scalars; every other
  surrogate unencodable with code point + index; unsupported source-valid
  contexts (other charsets, non-`0`/`1` `PYTHONUTF8`, non-text environment
  values) are truthful native capability failures.
* `src/op_admission.rs` — same message-free builder plus
  `plan_safe_commit_target` (the source preflight never inspects the payload).
  Every admission decision path, refusal code and read-only property is
  unchanged; all admission tests still pass.
* `src/legacy_json.rs` — minimal private exact-code-point append/accessors
  (`PythonString::from_scalars`/`push_scalars`/`codepoints`,
  `PythonText::append_to`); the UTF-8-only `to_utf8` is now a `#[cfg(test)]`
  inspection helper. No parser/grammar/depth/integer/writer change.
* `src/lib.rs` — private `mod python_os_encoding;` plus the crate-doc paragraph
  that previously claimed a UTF-8-only commit-message seam (API unchanged).
* Tests: new `tests/op_byte_message.rs` (9 tests), rewritten/updated
  `tests/op_admission_commit.rs` rows, and `tests/support/mod.rs` oracle
  isolation (own CWD, `HOME`, every `XDG_*`, `TMPDIR`; the oracle *verifier*
  runs in a UTF-8 startup context because `uv.lock` is not ASCII).

No `sk-git`, kernel, CLI, manifest, lock, fixture, doctrine, inventory or
dispatcher change; no new dependency; no process-global environment mutation.

## Red-first record (through the existing `commit_op_record`)

`red-byte-message.zu4Yx4.log` (before production code): **0 passed / 9 failed**.
True product reds, in the order a reviewer should read them:

| Red | Observed before the fix |
| --- | --- |
| used `\udc80`/`\udcff` profile/action (UTF-8 mode 0/1, forced ASCII, doctor spine) | source committed; native returned `Op commit message is not representable as UTF-8 (code point U+DC80/DCFF)` |
| protected topic + used low surrogate (and `\ud800`) | source refused `protected branch 'topic'`; native returned the **encoding** refusal first (guard masked) |
| used `\ud800`/`\udc00`/`\udc7f` | both refused, but the wording was the false UTF-8-only seam message rather than the measured filesystem encoder |

Two of the nine first-run failures were **test-harness** defects, not product
reds, and are recorded as such: (a) the oracle verifier itself crashed under the
forced-ASCII child because `uv.lock` is read as locale text — fixed by running
the verifier in a UTF-8 startup context while the product probe still inherits
the context under test (`support/mod.rs`); (b) my new `head()` helper compared a
newline-terminated `git rev-parse` output with an `Oid` string.

## Measured Git/source facts this binds

Repeated through the real pinned `_commit_op_record` and real Git 2.43 in
isolated CWD/`HOME`/XDG/TMP (`fsenc-*`, `fsenc2-*`, `srcservice4-*`,
`probe_commit_tree`, `locale-matrix`):

* `os.fsencode` (UTF-8 context): `\udc80` → `80`, `\udcff` → `ff`, `é` →
  `c3 a9`, astral → four bytes, `\ud800`/`\udc00`/`\udc7f` →
  `UnicodeEncodeError`.
* Forced ASCII (`LC_ALL=C PYTHONUTF8=0 [PYTHONCOERCECLOCALE=0]`): same escaped
  bytes, but `é`/astral are `UnicodeEncodeError('ascii', …, 3, 4,
  'ordinal not in range(128)')`; UTF-8 locale with mode 0, `LC_ALL=C` (auto
  mode 1) and `PYTHONUTF8=1` all commit.
* Both `git commit -m` **and** `git commit-tree -F` convert an invalid-UTF-8
  payload byte in the commit object body (`80`→`U+0080`=`c2 80`,
  `ff`→`U+00FF`=`c3 bf`) and warn on stderr, while the reflog keeps the
  pre-conversion bytes. With `i18n.commitEncoding=ISO-8859-1` both keep the raw
  payload and add `encoding ISO-8859-1`. Git owns that decision on both sides.

## Verification at the final tree

Scoped `sk-invocation` only (2 jobs, whole-workspace flock, existing target,
`CARGO_INCREMENTAL=0`, isolated `HOME`/XDG/TMP; every command log unique):

* `verify-3-tests.8TZUKG.log` — **194 passed / 0 failed**, 2
  subprocess-helper tests ignored by design. Per target: lib 58, lock_contention
  4, op_admission_commit 46 (+1 ignored helper), **op_byte_message 9**,
  op_commit_policy 30 (+1 ignored helper), op_custody_roots 15,
  oracle_vectors 4, reference_path_oracle 2, writer_filesystem 26.
* `verify-2-doctests.gjA2j3.log` — 1 passed. Total **195 pass / 0 fail**
  (baseline was 180 = 179 + 1; +9 focused tests, +6 new private unit tests; no
  pre-existing test was deleted or weakened to pass).
* `verify-3-clippy.Dq7u3g.log` — strict `clippy -p sk-invocation
  --all-targets -- -D warnings` clean.
* `verify-3-fmt.bf5g3Y.log` — `cargo fmt -p sk-invocation -- --check`
  clean.
* `verify-2-check.d0MRtB.log` — `cargo check -p sk-invocation
  --all-targets --locked --offline` clean.
* `verify-2-oracle.fv2Sio.log` — exact53 `ok: true`, Python 3.11.15, no missing/
  drift/duplicates; no probe skipped for a missing oracle.

Cross-side byte evidence (each in its own child with fixed
author/committer/dates, isolated roots):

* `utf8_mode_zero_commits_used_low_surrogate_bytes` — five rows
  (`\udc80` profile, `\udcff` action, `\udc80\udcff`, `\udc80 + é + astral`,
  `é + astral` with `\udc80` action): source and native both commit; **equal
  OID, equal complete `cat-file commit` object, equal reflog payload bytes,
  equal paths, equal clean status**, exact object body pinned per row.
* `utf8_mode_zero_binds_complete_objects_for_utf8_payloads` — `é`, paired
  `\ud83d\ude00`, mixed `é😀\u00a0`: equal OIDs and complete objects.
* `utf8_mode_one_commits_used_low_surrogate_bytes` — `LC_ALL=C` auto UTF-8
  mode: equal object, raw reflog `op(\x80): é`.
* `forced_ascii_context_matches_source_bytes_and_refusals` — `\udc80`/`\udcff`
  commit with equal objects and raw reflog; `é`, astral and `a\u00a0b` refuse on
  both sides with the truthful native code point, valid row preserved; the
  **source keeps the record staged** (`A kitty-ops/<id>.jsonl`) while the native
  plan adds nothing — measured, asserted, **not waived**.
* `protection_refuses_before_message_encoding` — protected `topic` with
  `\udc80` and `\ud800`: both refuse on the canonical guard, nothing lands, the
  native refusal is not the encoding text.
* `unencodable_surrogates_are_truthful_capability_failures` —
  `\ud800`/`\udc00`/`\udc7f`/action `\ud800`/`\udc80\ud800`: both refuse, native
  names the first code point and never claims `Invalid invocation record`.
* `nested_and_unused_surrogates_keep_canonical_repr_bytes` — list and dict
  profiles (lone surrogates + printable `é`) plus an unused nested surrogate:
  one canonical `repr`, equal objects/OIDs.
* `doctor_sweep_commits_only_the_closure_spine_with_low_surrogate_bytes` — used
  `\udc80` archive row: both commit **only** `kitty-ops/op-closures.jsonl`,
  equal OID/object/reflog, archive row and spine bytes untouched.
* `iso8859_1_commit_encoding_keeps_the_preconversion_payload` — both sides keep
  `op(\x80)…` and carry Git's `encoding ISO-8859-1`, equal OIDs.
* Existing controls kept green: normal ASCII/Unicode commits, legacy
  `NaN`/ordering/depth/integer rows, unused/nested surrogates, NUL refusal,
  writer/model/admission/protection/hook/landed/unconfirmed/captured-index/
  skip/detached/archive/spine tests.

## Remaining limits and non-claims (required next work, not waived)

* **Source stage-before-encoding / index-on-error footprint**: the source stages
  the record before the argv encoding can fail; the native plan precedes
  staging. Asserted as the measured difference in the forced-ASCII test.
* **Used NUL**: source `ValueError('embedded null byte')` after staging vs the
  native payload guard before staging (existing control unchanged).
* **F4 / J-P** — shadow-name vs physical-ref resolution, the four protected/
  invalid admission false-admits, born/unborn protected vs invalid, and
  unprotected-unborn source commits are untouched. Native target-plan progress
  is not source-path proof; no `SourceRefuses`/`guard-reached` claim is derived
  from an opaque backend error.
* **Exact-existing-ref / unborn publication**, full source path semantics and
  error precedence, and the reference's `rev-parse --abbrev-ref` fallback remain
  open.
* **Startup-encoding coverage**: only UTF-8 and ASCII charsets are implemented.
  A source-valid locale with another charset (e.g. `en_US.ISO-8859-1`), an
  unknown codeset, or an invalid `PYTHONUTF8` value is a truthful native
  capability failure, never a guessed codec and never a source refusal. Unknown
  but existent/non-existent locale fallback behaviour is not emulated.
* **Non-Unix wide argv** is unverified; it keeps the previous UTF-8 behaviour
  for valid UTF-8 input rather than inventing a refusal.
* Git-layer bounds remain: 2000-byte native message/reflog truncation, message
  cleanup rules, hooks/signing, and object-body conversion (Git owns it).
* Runtime Python limit mutations and caller-dependent recursion remain outside.
* No aggregate/workspace gate, release, inventory, capability-matrix,
  CLI/executor, StepContract, protected-main or performance claim is made here.
  The coordinator owns aggregate gates, commit+PUSH, and a fresh independent
  review before the J-P/publication/footprint increments.

Historical notes: `UNICODE-STRING-REMEDIATION-REPORT.md` and
`ADMISSION-COMMIT-REPORT.md` still describe the earlier UTF-8-only commit
message seam and the message-carrying admission request. They are retained as
historical owner reports (out of this increment's edit scope) and are
**superseded by this report** for the Op byte message and the shared
message-free target builder.

## Final bindings

```
1d6ac596beb289363ceb7b43b71995224e5fe8ebcc1ef602fe82f1092c16a539  crates/sk-invocation/src/op_commit.rs
814be000c28b5579003c3e6429da274bc72ca816c51156fbd337c1eef2a19f9b  crates/sk-invocation/src/op_admission.rs
58de9c96648570dfef51b29d3c6310f2d58f7ce83b79f2dd34281013355aca91  crates/sk-invocation/src/legacy_json.rs
28a7a4a7b0d8fa199a881fbafc136aca32cbc91cbc832d734a5e05d2b666189c  crates/sk-invocation/src/lib.rs
f658b6c9005bcae4c7d441fd3730ec625d037599b56b3ac290b3a488e0aa0e13  crates/sk-invocation/src/python_os_encoding.rs
e2abebcda4668d3f143b1d1bd39dba08dd02653e6342d20995a617641ebda848  crates/sk-invocation/tests/op_admission_commit.rs
a52ad2ba1f483f7588131651697a2a0d78b0b1bcd3f7c8b3e5f7e59b69616796  crates/sk-invocation/tests/op_byte_message.rs
b29d6b35360225aa583f676721152217f094d70621f1afdc6994624da7e7bbcd  crates/sk-invocation/tests/support/mod.rs
```

This report is not self-bound (its own hash changes with every edit). The
PROVISIONAL report that preceded the code was
`8bf3ceaef81047e85b87f12f528e75055bcee97c568146a25d30b456da64f75b`.

Pre-change bindings (for the review diff):

```
be36ba6729770bcf751fa8e9980802b3ff31542a7cc8363e6a747031d17d01ed  crates/sk-invocation/src/op_commit.rs
4bc52a693434d2e3724c4f5a8afb08b497c6bd2982189c001e2aade3fa6bb3ab  crates/sk-invocation/src/op_admission.rs
898dc7ab62a58142d498fd9ca53f688f61f4d18c4c27fa18f6bf54f11a258765  crates/sk-invocation/src/legacy_json.rs
5a647a54a4f9acbb37a73875778bb851714e9bcf0ae6cdc55ec1bf2272228d32  crates/sk-invocation/src/lib.rs
9f63b4c86d9b3cb4f7a600536e9106fa4b1d84cbe0a7ebd9354a75bb5c1224f6  crates/sk-invocation/tests/op_admission_commit.rs
924708db9adf6a22ff804633388d02a74878c05cc6e7d8739470f4a4ae9366fd  crates/sk-invocation/tests/support/mod.rs
```
