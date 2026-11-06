# CF68 delivery — historical pending intake

Subsequent bounded corrections/checks are recorded in
`cf68-rails-catalogue-bounded-intake.md`. The observations and storage-pause
chronology below are preserved as the actual earlier state, not current claims.

No approval, publication, whole-command parity, aggregate, independent release
review or performance result. Published e38b075 / Source9aed3a8, doctor13/6 and
migrations132/136 remain unchanged.

Parent first receipt `.YtADHR` preserved actual report/orders/current owners/
tests/map/Source externally cp-p/cmp/SHA/stat before probes. Actual report
`/tmp/sk-cf68-source-context-rails-report.md` was read through EOF. The new four
text rails/catalogue-count/live consumer and producer scoped pairs are delivery
claims, not yet current-parent acceptance. Producer's two discovered logic reds
were post-implementation; no Native pre-edit rail red is reconstructed. Its
203-test Doctrine and93-test CLI library sweeps exceed the tiny assigned scope;
those are retained as scope slips, not precedent. Foreign Rich-wrap red and
format drift were not fixed or relabelled as whole-tree success.

## Concrete review concerns requiring correction / verification

- `context.rs::charter_present` currently calls the correct canonical
  source_path_exists then `.unwrap_or(false)`, claiming Source absorbs every
  stat error. That claim is false: Source Path.exists does not ignore EACCES
  (or EINVAL), as prior actual errno controls establish. The new API erases
  those failures. Its live caller also computes presence eagerly before the
  undeclared rail, whereas executable Source returns that rail before presence.
  A correction must retain both unignored existence failures AND branch laziness;
  simply changing false to an eager error would create another mismatch.
- `reference_catalog.rs` currently uses the older bounded serde-YAML charter
  reader and language_scope repr. This does not carry exact Cs/NUL/non-scalar
  values like the existing Kernel object graph. `reference_admitted` also uses
  scalar conversion with empty-string fallback. Those are data-loss/fake-empty
  defects, not an authorized semantic policy. The actual Source reader is
  load_charter_yaml: UTF8 text/universal-newline/ruamel round-trip, not Path-byte
  BOM-autodetection; exists is outside its caught read/load block. Constructor
  runtime failures outside (YAMLError, UnicodeDecodeError, OSError) must not be
  silently empty. Reuse the canonical graph/str/deny owners, no new parser.
- Compact mark-before-render is represented syntactically, but prose and other
  render inputs are read before the mark. Full Source explicit-depth API and
  read/render/mark failure ordering are not established by the current CLI
  mode/header controls. Source CLI defaults do not make compact+first_load=true
  reachable; do not fabricate a CLI red for that explicit-depth library case.
- Full compact/property/profile/budget/pointer selection/order/logging/error
  wire, declared-but-starved control and Native depth/platform limits remain
  open; selected mode/count/header assertions do not certify them.

No parent Source/code/test edits, Cargo builds, tests, cleanup or new delegate
have been performed in this intake. New findings are pending fixes, not acceptance.

## Storage decision pending

Read-only inventory `.G5JKkl`: target/debug165G, of which deps161G,
incremental1.5G, build117M, fingerprints177M; target/cf65-context110M.
Filesystem22G available at observation time. These are du/df rounded values,
not promised reclaim amounts. Target contains retained fixture/evidence data.
User asked an opinion about cargo clean, not an instruction to delete. Nothing
has been removed. Prefer an inventoried build-profile/package-scoped cleanup
under the whole Cargo lock after preserving any retained build artifacts; do
not blindly delete all target. Defer additional builds/dispatch while that
storage decision is pending.
