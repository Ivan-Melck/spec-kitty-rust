# Native Op F1/F2 legacy started-object JSON remediation

Status: **FINAL historical owner snapshot**, before coordinator integration.
Owner-delivered code, tests, logs and bindings; not independent approval.
Later corrections, controls, current bindings and publication are recorded in
`LEGACY-JSON-INTEGRATION-REPORT.md`.
Audience: software-engineer. Updated: 2026-11-03.
Baseline published: `2fc7d5a5053da5e9eb47eb4dfbf4c74c5e88e5a9` (clean tree
after the coordinator's F3 protection-detail correction).
Pinned Python source / development oracle:
`9aed3a8012cb5eb22c0128b14d525039f95fea7a`, `.venv-rust-oracle`
(Python 3.11.15, 53/53 verified requirements).

## Scope

Only the executor's raw legacy started-object read and field
stringification for the best-effort Op commit:

- `executor.py::ProfileInvocationExecutor._read_started_event` —
  `path.read_text(encoding="utf-8").splitlines()[0]` then `json.loads`.
  Non-dict, IndexError/OSError/JSONDecodeError become Invalid invocation record;
  UnicodeDecodeError is outside that caught set and reaches the close's generic
  exception handler. Native error wording remains the separate F5 boundary.
- `executor.py::ProfileInvocationExecutor._commit_op_record` —
  `str(started.get("profile_id") or "unknown")` and the same for `action`, used
  in the commit message.

This closes review findings **F1** (bare `NaN`/`Infinity` rows: source commits,
native refused) and **F2** (nested-map insertion order and `1e400` overflow
changed the commit message/object).

Untouched: admission, branch/protection/policy, primary cache, `sk-git`
transaction/evidence, writer/models/modes/custody/Kernel/Git/workspace/CLI/
doctrine, the F3 detail mapping, F4 NBSP shadow branch, F5 error wording, and
every other crate. No manifest, lock, registry, service pin, release or Python
source change. No aggregate run, commit or push by the owner.

## Authority and method

Source beats reports. All probes used the exact
`.venv-rust-oracle/bin/python` with isolated project/CWD/HOME/XDG/TMP,
`PYTHONPATH=repo/src`, `SPEC_KITTY_PACKS_ROOT=repo/packs`. Live oracle facts
were recorded before coding (`/tmp/sk-legacy-json-facts.Tta3vV`,
`/tmp/sk-legacy-json-facts2.La4Lir`):

```
NaN -> nan (truthy)      Infinity -> inf      -Infinity -> -inf
1e400 -> inf   -1e400 -> -inf   1e-400 -> 0.0   -1e-400 -> -0.0 (falsy)
-0 -> int 0 (falsy)      -0.0 -> float -0.0 (falsy)
{"b":1,"a":2} -> {'b': 1, 'a': 2}
{"a":NaN,"b":1,"a":2} -> {'a': 2, 'b': 1}
[1,NaN,{"z":1,"y":2}] -> [1, nan, {'z': 1, 'y': 2}]
"NaN"/"Infinity" strings stay verbatim
+Infinity, nan, -NaN, trailing data, BOM, raw control chars rejected
lone surrogates parse in Python; >4300-digit ints raise ValueError
```

## Delivered changes (authorized surface only)

| File | Change |
| --- | --- |
| `crates/sk-invocation/src/legacy_json.rs` (new, private) | Ordered `LegacyObject` (`Vec<(String, LegacyValue)>`; duplicate key replaces the value in the first position) and a recursive-descent CPython `json.loads` lexical reader: case-sensitive `NaN`/`Infinity`/`-Infinity`, exact integer tokens (`-0`→`0`), float tokens via `f64` (`±inf`/signed zero), full first-value validation with trailing-data rejection, string/`\u` decoding and surrogate-pair combination, Python truthiness, `str()`/`repr()` composition in insertion order. No depth/size limit was invented. |
| `crates/sk-invocation/src/op_commit.rs` | Bottom only: `read_started_object` returns the private ordered object after the unchanged `python_first_line` splitlines rule; `started_field` is `str(value or "unknown")` over that tree; import swap; F1/F2 module and function docs updated, including the remaining gaps. All higher admission/branch/policy/cache/commit/evidence code and F3 mapping are untouched. |
| `crates/sk-invocation/src/python_json.rs` | `python_float_repr` is now `pub(crate)` and renders `nan`/`inf`/`-inf` like CPython; the existing shortest-round-trip/threshold formatter and `python_repr` are reused, not copied. |
| `crates/sk-invocation/src/lib.rs` | One private `mod legacy_json;` declaration. No public re-export or capability. |
| `crates/sk-invocation/tests/op_admission_commit.rs` | Six new F1/F2 regression tests (plus shared helpers) through the pre-existing `OpCommitService::commit_op_record` and the live `oracle_commit` harness; one new `python_json` unit test and nine `legacy_json` unit tests. |

No global `serde_json` `preserve_order`, no new dependency, no runtime Python,
no `eval`, no sentinel substitution, no copied wire writer, no public API
promotion.

## Red-first evidence (`/tmp/sk-legacy-json-red.log`)

Tests were added before any product edit; the suite compiled and ran, so these
are behavioural reds, not harness failures:

- `legacy_bare_nonfinite_tokens...`: native `Invalid invocation record:
  01ARZ3NDEKTSV4RRFFQ69G5FB3` (source commits `op(nan): a …`).
- `legacy_nested_collections...`: native `Invalid invocation record: …FBC`
  (source commits the `[1, nan, {'z': 1, 'y': 2}, True, None, [], {}]` subject).
- `legacy_ordered_and_duplicate_nested_maps...`: native
  `op({'a': 2, 'b': 1}): {'y': 4, 'z': 3}` vs source
  `op({'b': 1, 'a': 2}): {'z': 3, 'y': 4}`.
- `legacy_numeric_overflow...`: native `op(1e+400): -1e+400` vs source
  `op(inf): -inf`.
- Green controls in the same red run: prior legacy/default behaviour, authored
  `"NaN"`/`"Infinity"` strings, and invalid-row refusals (3 passed).

Result line: `3 passed; 4 failed` (`/tmp/sk-legacy-json-red.log`).

Classification note: after the first product edit the new module failed to
compile once (`E0599` `u8::to_digit`); that is an implementation compile error,
not behavioural evidence, and the log path was reused by the rerun so only the
green compile is retained. No behavioural test was retried to green.

## Green evidence

- `/tmp/sk-legacy-json-unit-green.log`: `legacy_json` unit tests **9 passed, 0
  failed**.
- `/tmp/sk-legacy-json-green.log`: **7 passed, 0 failed** = six NEW F1/F2
  integration tests plus ONE pre-existing legacy control (not seven new tests).
  Successful-case helpers assert native committed=true, returned OID == native
  post-commit HEAD, cat-file type=commit, exact subject/path and byte-identical
  audit rows; the live source method matches subject/decision. Invalid-row
  controls assert refusal/no advance/retained bytes on both sides. No fixed
  cross-side author/committer timestamps or complete object equality established.
- `/tmp/sk-legacy-json-tests.log`: scoped **154 passed / 0 failed / 2
  parent-exercised helpers** (138 baseline + 10 unit [9 parser + 1 float] +
  6 NEW integration), 9 result
  sections including the doctest. The 2 ignored helpers are the pre-existing
  child-process helpers, not waived acceptance.
- Scoped strict Clippy `--all-targets -- -D warnings` exit 0:
  `/tmp/sk-legacy-json-clippy.log`.
- Scoped `fmt --check` exit 0: `/tmp/sk-legacy-json-fmt.log`.
- `cargo check -p sk-invocation --locked --offline` exit 0:
  `/tmp/sk-legacy-json-check.log`.
- Required exact oracle **53/53**, `python 3.11.15`, no drift/missing/
  duplicates/unsatisfied: `/tmp/sk-legacy-json-oracle.log`, sandbox
  `/tmp/sk-legacy-json-oracle.MNvIBC`.

All Cargo commands ran as whole commands through `bash /tmp/sk-focused-cargo.sh`
(shared flock lease, 2 jobs, no incremental, existing target, temporary
HOME/XDG/TMP).

## Historical owner bindings (sha256, working tree at `HEAD=2fc7d5a50…`)

```
b4a1dbebf8dae7cdd19121b576e811daf9d992be405c36aa8be5eaae08d8b245  crates/sk-invocation/src/legacy_json.rs
33bc1140b2ca8e31da6be6f0609771ca6dcebc24e4acbae393759796ba926b61  crates/sk-invocation/src/op_commit.rs
ccd29eb9a51902c875434e4bf4959b6c375f7619b3851a439ef5a19235e11c9a  crates/sk-invocation/src/python_json.rs
c8df30ad8b1854d4f83029c2a3467a43f1175866e9fa7426d71d592176cca4f9  crates/sk-invocation/src/lib.rs
40dfb627e7264bde4c8d930c1a1e4cb64e40432d08c39e477e51022257951b34  crates/sk-invocation/tests/op_admission_commit.rs
```

`git status --porcelain` shows only these five paths plus this report; no
manifest/lock/registry/pin/source-Python change. (An earlier
`legacy_json.rs`/`op_commit.rs` content hash changed only through the scoped
`cargo fmt -p sk-invocation` pass and the final documentation wording.)

## Remaining limitations (genuine functional gaps, not waived)

- Lone-surrogate strings (`"\ud800"`): Python parses them; Rust `String` cannot
  represent them, so native refuses the record. When such a surrogate is
  outside `profile_id`/`action`, source can still commit, so this is a real
  input difference (direction: native refuses).
- CPython's default 4300-digit integer conversion ceiling (plain `ValueError`,
  hence source refusal) is not emulated; a longer integer token stays exact and
  may commit natively (direction: native commits).
- CPython's recursion ceiling for pathological nesting is not emulated; the
  reader has no invented depth/size limit.
- Nested-string `repr` reuses the canonical `python_repr`, whose Python
  `isprintable()` table is incomplete (for example NBSP), so some nested string
  renders differ. Non-UTF-8 records, BOMs and `repr(Exception)` refusal wording
  remain the previously disclosed F5 boundary.
- F4 (NBSP shadow branch) and F5 (error `repr()`/classification) are untouched
  coordinator-owned findings.

## Non-claims / coordinator handoff

This is F1/F2 started-object compatibility only, demonstrated for the exercised
legacy rows. It is not arbitrary-JSON equivalence, not executor/CLI/StepContract
parity, and not whole-command or performance promotion. Historical/coordinator
documents (`docs/rust/VERIFICATION.md`,
`docs/rust/reviews/op-admission-commit-disposition.md`,
`crates/sk-invocation/ADMISSION-COMMIT-INTEGRATION-REPORT.md`) still describe
F1/F2 as open until the coordinator integrates this slice and updates them;
those files were deliberately not edited here. The coordinator owns the
aggregate/workspace gates, commit and push.
