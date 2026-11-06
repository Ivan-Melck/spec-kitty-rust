# CF62 — exception-aware execution, bounded parent intake

Published HEAD remains `e38b075eebc6ceffb2adb96b1b1735acfd815da7`; Source pin
`9aed3a8012cb5eb22c0128b14d525039f95fea7a` is unchanged. Shared delivery is WIP.
Doctor remains 13 reachable / 6 refused; migrations remain 132 / 136.
CF63 and CF64 remain separate active owners. No aggregate, release, registration,
publication, independent review, whole-store parity or performance approval.

## Actual delivery and Source policy

Actual report `/tmp/sk-cf62-source-execution-exceptions-report.md` was preserved
with orders/current owners/Source before parent probes (`.EClxfZ`) and read through
EOF. Evidence stays `/tmp/sk-evidence-cf62-251106`; its failures and slips are not
reconstructed by later parent checks.

`sk_status::source_execution` now owns typed `Store`, `Escaped` and native-only
`Operational` outcomes; `execute_source_diary` composes the existing exact stream
and fold, with explicit annotation Fold/Ignore selection. Existing annotation,
metadata and line decoders report their phase rather than every failure becoming
an invalid row or healthy None. The committed DONE consumer forwards escaped
exceptions/native limits instead of describing those as healthy absence.

Parent inspected actual Source `_partition_event_stream_from_text` and resolver:
per-line JSONDecodeError is caught separately; the admission/resolver try catches
KeyError/ValueError/TypeError/AttributeError. Integer Infinity's OverflowError
escapes. A JSON-line integer ceiling ValueError is outside the JSONDecodeError
catch. A field-level int-string ValueError is inside the admission catch. The
fold is outside the store. `exists()` errors differ from read errors: metadata
read errors caught by Source stay None, but an escaped exists failure cannot be
made a metadata miss. Native operational parser limits are labelled as native,
not fabricated Python exceptions. These outcomes are necessary capabilities;
labels do not authorize the remaining Source-valid refusals.

## Parent finding and correction: partial symlink prefixes

The delivered resolver handled an IO error from the canonical walker by reverting
the entire candidate/root to its original lexical path. That loses any symlink
prefix already expanded. For an aliased specs root plus an inaccessible metadata
tail, Source reaches `exists()` and raises PermissionError, while Native compared
a lexical alias against a resolved root and returned healthy None.

Actual PRE standard8 map and all three validators:
`docs/rust/occurrence-maps/cf62-parent-partial-path-resolution.yaml` (`.rUhZRs`);
preimages copied/compared/hashed/stat-bound externally. Existing dual-real-child
pair gained a fresh temporary specs-root alias case. `.egf4DZ` is the genuine
before-fix business red: Source raised, Native answered ok.

The ONE Kernel walker now accepts `ResolveIoPolicy`; default MissingTail behavior
is unchanged. Explicit IgnoreMetadataErrors continues component-by-component after
lstat errors, retaining expanded prefixes and subsequent traversal. Paths exposes
a same-engine facade; no second walker/path table, parser or registry exists.
Status selects that policy rather than substituting the original path. Candidate
resolution failures follow Source's caught wrapper; remaining root-resolution IO
errors are outside that wrapper, not fake lexical roots. Readlink/CWD failures are
not indiscriminately treated as lstat failures.

The walker's existing 40-symlink native ceiling is **unchanged and unapproved as
Source acceptance policy**. It can still confuse a long acyclic chain with a loop;
this correction is not unbounded pathlib parity. Public default facade docs were
corrected not to imply an exact Python IO policy.

## Focused parent evidence

Receipt root `/tmp/sk-vertical-integration.TwDlpw`:

- `.nZVYbb`: original new execution pair, existing full-stream pair and parent
  integer admission target pass before parent changes.
- `.ar2ztX`: new committed pair and existing CF54/59 committed controls pass.
- `.egf4DZ`: real alias/inaccessible-tail before-fix failure described above.
- `.gu5fic`: extended pair and existing stream/integer controls pass after the
  canonical policy correction.
- `.mcYhcq`: existing Kernel containment and path-contract targets pass; default
  inspection behavior remains separately tested.
- `.lXmi9E`: final extended pair (Source PermissionError premise explicitly checked),
  stream/integer and three named committed targets pass on corrected bytes.
- `.09GjGT`: four new execution-policy module controls pass (helper controls,
  not four paired features).
- `.aFTzUu`, final `.UJF4kv`: selected Kernel/Status/Workspace libraries + two
  named CF62 targets, unsuppressed `-D warnings`, pass.
- `.5JzNOU`: owned formatting / shared path-file check-only clean.

These are named bounded checks, not summed into an aggregate. The new pair has
16 planned cases after the parent alias addition (the producer prose's “13 cases”
was not an exact count; its original vector had 15). It is not 16 independent
paired features. Pair environment assertions were inspected: both actual children
check temporary CWD/HOME/all seven XDG/TMP variants, absolute independently supplied
Source/packs and hash seed before business reads; Source verifies its interpreter
version/import origin. Committed fixtures use fresh Git repositories, not copied
linked repositories. Existing reference/native divergence assertions are not
strict equality proof for those rows.

## Retained limitations / rejected shortcuts

- PermissionError versus the Native OSError family and exact OS error prose/path
  remain mismatches, not approved simply because the report calls subclass mapping
  “guessing.” Both ordinary and aliased unreadable metadata controls now prove
  Source PermissionError and Native's escaped exists phase, not class/message parity.
- Source deep metadata raises RecursionError; Native reports its operational frame
  budget. This is honest classification of a Native limitation, not equivalence.
- The old absorbing `SourceSlugResolver::resolve` view is explicitly an inspection
  adapter; Source execution uses typed resolution. No healthy None approval for
  escaped business errors follows from keeping that adapter.
- Full fold exception coverage, Source warning effects, native filename/path error
  codecs, readlink/permission races, platforms, whole-collector/root CLI catch
  composition and historical bounds remain open. Root doctor is not promoted.
- Producer whole-crate Status 314 / Workspace library and multiple-target 204
  reports were **broader than the assigned tiny-check scope**. They remain scoped
  producer observations, not parent aggregate closure or precedent. Unwrapped
  shell writes/edits/reads and other disclosed protocol slips remain slips.
- Parent `.8bolLh` was a shared import-format difference, repaired precisely;
  `.4ldTfq` queried a nonexistent `tests/paths.rs` after listing actual targets,
  exit2 harness error, not a Source business observation. All attempts retained.

No additional semantic exception is authorized. Exact Source behavior, including
failure phase and footprint, remains required beyond these bounded deliveries.
