# Canonical YAML object graph: architecture decision, not fidelity approval

2026-11-05. Pinned Source and packs unchanged; no host Mission/Op/gate approval.

## Decision

The Source-compatible loader must construct ONE canonical Python object graph,
shared by Mission and Doctrine. An arena with typed internal links and stable
object ownership is approved. Links identify structural references, never business
IDs, escaped spellings or pseudo-values. Cycles and sharing survive construction;
no recursive owned-tree expansion, scalar dropping or formatted-string substitute.
Native code remains safe Rust, with bounded *inspection* distinct from compatible
execution. No new house nesting cap is approved as Source parity.

Replace/delegate the current semantic-tree constructor; do not add an independent
fallback graph/parser/model/hash/renderer. Keep exactly one construction, object
identity, equality, hash/probing and text authority. Legacy views are explicit
projections, not another source of truth, and must not make a valid ignored cycle
fail before Source validation. Existing SourceValue/OrderedMap aliases must remain
real within-build aliases, not lookalike DTOs.

Actual loader provenance matters: `.nan` returns a loader singleton, but explicit
`!!float nan` constructs a fresh object. Python scalar NaN equality remains false;
container key/element comparison uses the appropriate identity shortcut. Hashes of
None/NaN require actual stable modeled objects, not one unrelated byte sentinel,
zero, node-index-as-pointer or another process's sampled address. Alias edges,
constructor scheduling and failure phases are behavior, not serialization details.

## Established corrections and remaining evidence

- L5 delivered real tuple keys and finite equality controls, not graph completion.
  Original report/7-file subject preserved in `.ljcARj.log` before parent edits.
- Parent schema-valid pre-edit classification `.Xs5Xps.log`; actual isolated
  Source-first date-set ordering red `.QpZVTK.log`. Pinned CPython v3.11.15 C/header
  `.tsUrbe.log` establishes packed4/10-byte hashes for date/naive datetime and
  normalized timedelta tuple hashing for aware datetime. The former arbitrary
  digit-split tuple mixer and consumer-needs rationale were removed.
- Real tuple members inside `!!pairs` now permit ruamel's shallow conversion of
  that list into a tuple key. PyYAML's refusal and nested unhashable-member phase
  stay dialect-specific. Existing public Pairs value shape is unchanged.
- Current paired corpus `.n7XPDY.log`:128 receipts,126 ordinary comparisons,
  24 refusals,11 exact runtime messages and2 declared cycle divergences. Those
  two are NOT parity greens. Actual Source-child assertions were missing in the
  old L4/L5 oracle script despite environment setup; parent added version/root/
  PYTHONPATH/packs/all-seven-XDG assertions to both actual legs and input
  bytes/mode/inode/mtime footprint checks. Historical weaker receipts stay weaker.
- New actual counterexample `.yOWtd3.log` / `.vQQqqs.log`: Source accepts2 fresh
  NaN keys in both dialects; Native collapses1 or refuses. Fixing date hash does
  not establish universal key equality or authorize that association loss.
- Loader-only depth proof `.CUXC2H.log` avoids recursive repr/JSON measurement:
  both loaders accept400/492 at a shallow call, fail500, and extra20 Python
  frames make490 fail. Source recursionlimit1000 is NOT a universal492 bound.
  Current serde_yaml_ng hardcodes128 and yaml-edit0.3.6 hardcodes256; Source-valid
  inputs are falsely refused. Cached APIs have no public configurable parse
  limit (`.BUdI2e.log`). An arena does not magically remove the syntax barrier.

A safe event/CST parser interface needed to remove that barrier belongs to the
coordinator/dependency owner. No silent registry patch, dependency fork, magic512
bump, ad-hoc deep-only parser or SourceFaithful label is authorized. A syntax-port
proposal may preserve the existing lossless editor as an explicitly separate
capability, but compatible semantic execution must keep one real authority.

Scoped Kernel105-unit/UTF8/Mission/Doctrine consumer checks and strict/docs pass
`.Lx8vaV/.HgBU11/.CUZdVZ/.sIk2pX`; these finite gates are not broad graph, offering,
acceptance, callback, workflow or performance approval. Central126/136 unchanged.
