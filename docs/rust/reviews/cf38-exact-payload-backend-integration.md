# CF38 exact payload backend — bounded parent outcome

Published baseline remains `e38b075`; Source/packs/pins unchanged. This is backend integration, not production include parity, whole-tree approval, or performance certification.

## Canonical causes and corrections

The exact schema walker emits Kernel Python JSON values; its finite serde API remains a checked adapter. Actual isolated Source observation `cf38-parent-actual-source-float-and-timestamp-cause.fCaNvE.log` showed:

- ruamel constructs implicit datetimes; the earlier claim that they remain strings was wrong;
- Pydantic model JSON emits UTC `Z`, unlike Python datetime string/isoformat `+00:00`;
- YAML12 negative infinity must receive its sign once.

Parent corrected the YAML12 unsigned special-value magnitude, added one `clock::model_json_datetime_text` presentation facade (not a parser), and reused it in exact artifact payloads and existing DecisionMoment serialization. The implicit timestamp resolver is unchanged. The old explicit positive-infinity test expectation was corrected only after the actual Source cause/fix; the failed run remains evidence.

## Receipts and limits

Evidence root: `/tmp/sk-vertical-integration.TwDlpw`.

- `.Xeo7Lb` / `.DLFWLp`: actual backend/time-owner preimages.
- `.N3mNya`: invalid parent map syntax; preserved `.MOowE5`, corrected before production edits; canonical gates `.7tBzZa`. Additional paired fixtures preclassified `.IUQAa1`; later explicit stale-infinity assertion clarification also validated before its edit.
- `.cz3R7N`: parent's nonexistent public API import, a harness/compiler failure, not a behavior red.
- `.p5maJH`: two genuine before-fix failures (negative infinity and model datetime).
- `.Tib8nn`: 13 focused passes (Kernel float, Doctrine datetime, CF38 paired fixtures, CF31 fixtures). The strengthened CF38 pair compares payload bytes, not production include delivery.
- `.SJDuYK`: seven exact-walker units passed, one stale positive-infinity expectation failed after the production fix.
- `.eWogdF`: all eight exact-walker units passed after correcting that premise, without reducing assertions.
- `.wViL4j`: existing DecisionMoment model-projection Source regression passed. Its historical isolation helper is not newly certified as a seven-XDG pair.
- `.LOfNmD`: selected Kernel/Doctrine/Workspace libraries and three exact-feature targets strict Clippy passed, without suppressions.
- `.p304cY`: owned-file formatting check, current-byte SHA bindings, same HEAD and no Source/pack diff. This is a scoped binding, not an aggregate gate.

Producer failures/unwrapped or overscoped attempts remain disclosed in the actual report; subsequent greens do not erase them. CF42 also formatted the parent datetime test's whitespace; no destructive reconstruction/restoration was performed.

## Still missing

Production non-template includes still use the finite path; merged records lack the required exact field provenance, and exact profile/inline/full output must reach the real CLI. A new CF43 consumer implementation is planned separately, reusing existing graph/model/merge/formatter authorities. No source-valid scalar refusal, silent drop, marker output, or alternate schema is approved. Non-string keys, construction/depth/identity, failure phases, wire/encoding/platform/concurrency and whole-workflow proof remain qualified. No genuine equivalent-work speed measurement, commit, push, or publication occurred.
