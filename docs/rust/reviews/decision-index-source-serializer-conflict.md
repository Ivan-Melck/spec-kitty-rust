# DecisionIndex: authorized narrow serializer-preservation exception

**Operator-authorized preservation exception; NOT Source byte parity.** After
being asked whether Native should remain lossless under this narrow exception,
the user authorized: "do that and work on all fidelity gaps". Native must retain
exact authored keys/values here. Source/oracle pins and historical evidence stay
unchanged; this does not authorize any other compatibility deviation.

## Independent causal proof

Pinned Source `9aed3a8012cb5eb22c0128b14d525039f95fea7a`, CPython3.11.15,
exact53 locked requirements; drivers SHA
`5fc6fbbd1fdc020d0494c4c5140c74b60dd743ba7231ebab8306540f84b50853`,
Decision models SHA
`78b42e1725d13b1a4eb7e99676d9c77344ef5af9c229fab60b57244680bdb97d`.

For a Source-valid entry with summary_json keys U+D800 and U+D801 and values
`first` and `second`:

1. Actual public raw union preserves both exact keys and values.
2. Actual `DecisionIndex.model_validate` ALSO preserves both keys and values.
3. **`model_dump(mode="json")`** converts both keys to three U+FFFD code points,
   collapses them to one key, and retains only `second`.
4. Actual `run_decision_index_driver` writes that collapsed projection. Original
   Python input objects and the base/theirs files remain unchanged; the written
   authoritative `%A` has lost one authored entry.

The single-key case likewise changes U+D800 to U+FFFD U+FFFD U+FFFD while keeping
its U+DFFF value. This is a JSON-mode projection effect, NOT key coercion during
schema validation (the H6 producer's phase description was too broad).

## Evidence and Native result

- Source phases: coordinator `.CTnAsg.log`, fixture
  `/tmp/sk-h6-summary-source.bnkBbv/phase-records.json`; every phase records exact
  code-point arrays. Source first, all temporary CWD/HOME/XDG including RUNTIME/
  TMP assertions, pack/PYTHONPATH roots and exact53 fail-closed verification.
- Equivalent isolated Native: `.4l7dHY.log`, **exit101 business red** on BOTH
  fixtures. It preserves the exact original keys and both authored values;
  bytes therefore differ from Source. Actual written and returned bytes agree.
- Native production probe: `/tmp/sk-h6-parent-model-probe.9jgRBS/`; Cargo whole
  command lease/offline/absolute repository target-dir `.I4eWDn.log`.
- Initial `.liumdw.log` failed BEFORE Source model execution because parent
  passed unsupported verifier argument `--json`; harness error, not a Source
  refusal or semantic red. It and its fresh fixture are retained.
- H6 original code/tests remain preserved. Parent verified the entire H6
  DecisionIndex section stayed byte-identical during unrelated parent fixes
  (`.CRyJNy.log`). Its 99 finite cases did not test this known mismatch.

## Authorized scope and boundary

Only `DecisionIndex` JSON-mode projection of **unpaired-surrogate keys inside
`IndexEntry.summary_json`** is excepted: Native preserves those exact keys and
all associated authored values, including collisions introduced by Source's
lossy projection. Raw union, validated model, summary values, other fields,
validation rules, ordering, dates and physical publication are NOT excepted.
There is no new refusal and no lossy bridge or replacement identity.

The original Source/Native red remains a real historical byte divergence. It
is now an explicitly accepted difference, not retroactively a Source pass or a
license to mask other fixture bytes. New executable tests must run actual
Source first, assert its precise corruption phase, and separately prove Native
preservation and all unaffected fields/footprints. Source/goldens/pins are not
rewritten; benchmark conformance must disclose the exception, never silently
call unequal work equivalent. Approval does not extend to workflow, release,
other fidelity gaps or performance certification.

## Executable authorized-exception contract

`crates/sk-workspace/tests/decision_serializer_preservation_exception.rs` passes
`.owFsco.log` (actual Source first, isolated Native): single key, collision, and
all2048 surrogate keys alongside NUL/PUA/noncharacter/authored replacement keys.
Source retains2052 keys through validation but writes4; Native retains2052 and
all values. Entire unaffected projected bytes, returned/written bytes, mode0640
and non-output inputs are asserted. A second attempt with stale open entries
must REFUSE inertly on BOTH sides; no invented idempotent success. Initial
`.9x1PJj.log` was that invalid retry premise, preserved, not a Native business
red. The historical original mismatch and Source bytes were not rewritten.
