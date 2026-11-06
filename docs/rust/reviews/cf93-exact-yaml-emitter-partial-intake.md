# CF93 — exact activation YAML emission foundation (partial)

## Delivered, not whole activation closure

`sk_charter::ordered_yaml_string_lists` now accepts authored field names and
`PythonString` list members, including lone surrogates. It uses the **existing**
canonical YAML emitter: shared block mapping/sequence state machines, scalar
style selection and one widened codepoint double-quoted writer. There is no
second serializer, JSON/repr/marker value bridge or new dependency.

The old UTF-8 `Value` APIs still use that same emitter. Its private duplicate
bool/int/float/null/timestamp resolver was removed in favor of the canonical
Kernel YAML 1.2 resolver. The measured `_` quoting mismatch exposed a missing
initial-character dispatch guard there: an underscore-initial token never
reaches Source's integer regex; signed underscore tokens still can. Corrected
in the canonical owner, not special-cased in emission.

**Still missing:** exact semantic/CST `YamlDocument` ownership and mutation,
planner/transaction data propagation, actual activation/deactivation CLI
responses and physical output. Those scalar paths are not fixed by this input
API. No registration/count, full workflow, release or performance claim.

## Source grounding and bounded checks

Pinned Source: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Parent personally ran the actual public activation manager before this change:
an ASCII declared ID of a valid raw-named project tactic resolves to a stem
ending in U+DCFF. Source stores `- "cf93-\uDCFF"` (uppercase hex), repeats and
`--only` leave ledger bytes unchanged, deactivation records the exact deny stem,
and reactivation clears it. Receipt `.UJ2bHp`; raw observations retained under
`/tmp/sk-cf93-source-business.tDYR1u`. This is manager evidence, not CLI proof.
Earlier arbitrary `custom:` modern-ledger fixtures were Source-invalid and are
not Native business reds (`.zyDGxE`, `.Gb4bWm`).

New named target `sk-charter/cf93_exact_yaml_string_lists` runs one independent
Source/Native business-process plan with nine cases. Source calls the actual
`render_yaml_document` helper; Native calls the new formatter and the canonical
YAML graph reader. Both business children assert fresh independent
CWD/HOME/seven XDG/TMPDIR/TMP/TEMP roots, independently supplied absolute expected
Source/PYTHONPATH/packs, hash seed, actual filesystem/stdout/stderr codec policies
and pinned CPython 3.11.15/exact53 checks before business reads. Source verifies
module origin. Fixtures are copied independently; actual complete output bytes
and parsed codepoint associations are compared without path/output rewriting.

Coverage includes all 2,048 surrogate units individually and in one long
string, separate surrogate-pair units versus supplementary scalar codepoints,
NUL/C0/C1 units, PUA/noncharacters, quoting, wrapping, retained field order and
empty collections. Surrogate cases additionally assert full input roundtrip.
Scalar-only inputs exercise the unchanged `ordered_yaml` input API too. Nine
cases are **one pair plan**, not nine independent process pairs; the oracle
checks and in-process roundtrip assertions are not extra pairs.

Source's own equality guard is authoritative. A combined long control-string
probe was rejected by Source after its emitter/parser inserted a space
(`.w5Zs91`, preserved rejected-case diagnostic); it was not a valid Native red.
Individual control-unit cases are admitted. Some admitted Source scalar output
is equal under `yaml_documents_equal` without literal input equality, so tests
compare the actual parsed Source values as well as bytes rather than impose a
stronger policy. No new semantic exception or exception-to-healthy fallback was
introduced. Full Source guard/failure behavior remains a required consumer
integration obligation.

Focused accepted checks:

- new target, one test / nine-case plan: `.EUJIYJ`;
- directly affected `canonical_yaml::tests`, four tests: `.yPhYLv`;
- selected Kernel/Charter library strict checks: `.bzlS7n`;
- new named target strict check: `.smlUVR`.

Final byte-bound named target: `.iXZzD6`; formatting, all three map gates,
narrow inventory and protected-path checks: `.7iwtKB`; compatibility binary
compile: `.RYyZ2a`. Publication receipts are recorded in the parent report. No unfiltered library/all-target/aggregate/performance sweep
was used. File bytes and these controls do not prove metadata, read counts,
syscalls, races, platform behavior, writer admission or the root CLI.

## Attempt history / qualifications

The first delegate returned only a provisional report and a premature claim
that Rust strings cannot carry the needed YAML syntax. Parent measured Source
and showed ASCII escaped syntax is possible. A subsequent implementation
provider stream failed; it was not retried, switched or replaced. Parent did
this implementation directly. Producer pre-report unwrapped listings/pipes and
incomplete initialization remain in their actual provisional reports; not
retrospectively qualified.

Parent FIRST post-failure custody `.AEh3Ge`, original API census `.hqEy8A` and
`.W7ynEa`, later demanded emitter/helper custody `.Qgzcup` and resolver custody
`.mW0rMC` are not a claim of complete FIRST custody for newly discovered owners.
Prospective map gates `.rythEY`, `.YW3rIM`, `.ddRFfo` preceded their respective
edits; status stays `accepted:false`.

Retained attempts: `.96oog0` repeated TMP-directory setup error (harness, not a
red); `.l1ApQN`/`.Atdpbb`/`.w5Zs91` Source control-string premise failures;
`.3UT9AH` over-strong harness input-equality assertion; `.zh9VFC` actual full
byte mismatch and `.RRExR3` localization; `.MWOoAi` showed emitter resolver
unification alone did not fix the canonical dispatch. Failed exact-edit match
and `.GcwzR1` formatting-only attempt are retained. Initial test TempDirs were
automatically dropped on panic; later fixtures use `keep()` and retain raw
stdout/stderr/results. Lost earlier fixtures are not reconstructed or claimed
preserved. No old-byte A/B replay, restore, Source/pin edit or storage cleanup.
