# Semantic benchmark evidence — scoped review disposition

Reference: `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
Independent report: `/tmp/sk-rust-semantic-evidence-review.md`.
Reviewed harness sources are uncommitted on `d64468a76`; the report records hashes.
No performance campaign, current build receipt, or speed/readiness claim exists.

## Independently reproduced

- **78 benchmark pytest tests**, **59 checker/oracle tests**, **49 self-tests**;
  these counts precede the identity-contract correction below.
- Real two-root fixture regeneration rejects consistent forged fixture tables.
  Parsed `fixtures.json` must match the bundle table.
- Stored mutation bytes, manifests and deltas are checked against actual size,
  JSON payload, allowed mutation and unchanged-path contracts. Empty/wrong-path
  deltas, extra files, byte tampering, 2799 events and 199 done WPs all refuse.
- Global observation counters must be unique, adjacent within each pair, ordered
  by pair index and consistent with the raw sample records.
- Command templates match the registry and coverage projection; an error cannot
  coexist with a PASS verdict. Synthetic flags do not bypass the final checks.
- Production still refuses: **599 errors, 27 PERF errors**, 20 required rows,
  17 gate rows and 16 absent workloads. Floors remain 3×, 20 samples per side,
  at least 14 strict workloads, and a 10% regression allowance.

## Remaining scientific binding and subsequent correction

**R1 independently substantiated:** the observer now
returns actual two-root per-file manifests; recorded mutation inputs and
unchanged outputs are compared with those facts. One normalization covers both
pristine and clone roots, including embedded original-root paths. Synthetic
controls now use real generated input hashes. Owner evidence is **86 benchmark
pytest / 61 checker-oracle / 49 self-tests**, with re-sealed R1 and R2 negatives
through the final checker (`benchmarks/rust/INPUT-MANIFEST-REPORT.md`). Fresh
independent report `/tmp/sk-rust-input-manifest-review.md` reproduces those exact
counts and rejects the old false hashes, missing/extra/differing input entries
and all four forged stored-identity fields. It proves the registry change is
exactly the four stronger R2 equalities plus self-digest repair. This closes
these scoped findings, not the production performance gate.

**R2 coordinator correction:** the written `status.json` contract now requires
its Mission slug, number, type and schema version to equal stdout's existing
contract. A red-first contract regression and four stored-byte negative controls
pass (**5 focused tests**; `/tmp/sk-post-state-identity-{red,green}.log`). This
strengthens expectations only: no workload, measurement, sample floor or policy
was changed. The registry digest necessarily changes; no old receipt is valid
for the new bytes. The owner repaired the resulting stale registry self-digest;
coordinator independently loads and validates it: file SHA-256 `74a0f408…`,
self-digest `sha256:82435983…`, still 20 required / 17 gate / 16 absent and
unchanged floors. Independent full-suite results reproduce the owner above;
no measured campaign or current build receipt follows from that acceptance.

## Scope qualifications, not performance evidence

Caller-supplied audit registries are explicit trust anchors; production uses the
live canonical registry. Chronology proves stored structure, not historical
execution, and command binding currently proves the recipe rather than a separate
per-invocation argv receipt. Production callers explicitly request fixture
regeneration even though the lower-level evaluator defaults otherwise. A fixture
seed is a declared run parameter. Historical red-first claims cannot be inferred
from uncommitted tests alone; retain actual red/green logs and publish the code.

Keep these limits explicit. They do not turn the passing synthetic control into
production measurements or certify any of the absent workloads.
