# Safe YAML event interface — prototype authorization only

2026-11-05. Parent dependency-owner decision; not fidelity/release approval.

The current compatible YAML path falsely refuses Source-valid nesting because
serde_yaml_ng0.10.0 hardcodes semantic depth128 and yaml-edit0.3.6 hardcodes syntax
depth256. Neither cached public API exposes a suitable configuration. Source
loader failure also depends on remaining Python recursion budget; simply raising
a constant is not parity. L6's actual graph builder takes constructed Value plus
markers, not events. L7 moved Mission onto graph views but does not remove these
barriers or migrate all Doctrine validation.

A bounded prototype may expose the EXISTING serde_yaml_ng safe libyaml event
reader as an additive public safe API. It must reuse that parser/grammar/error/
mark/anchor/tag authority, not clone or replace it with a private parser. One
visibility/API fork proposal outside the repository is authorized, copied from
exact cached0.10.0 package with source/checksum/license/provenance/diff recorded.
No new unsafe code or dependency version, no RootCargo/lock/workspace/vendor
integration, registry-cache edits or runtime Python. The coordinator must review
supply-chain implications and explicitly integrate any dependency change later.

The event boundary must be owned/borrowed safely, preserve scalar bytes/styles,
anchors/aliases/tags/document boundaries/marks/directives as actually available,
and parse iteratively without imposing a new house cap. Existing Deserialize
behavior must stay unchanged. Incomplete event metadata is named OPEN, never
silently invented. API tests compare Source parser events separately from safe-
loader constructor/model behavior: accepting a deep event stream does not prove
that Source safe_load accepts its constructed object at that caller depth.

This does not waive exact strings/surrogate transport, dialect policy, constructor
scheduling/identity/equality/hash/order/duplicate rules, raw-vs-container equality,
caller-dependent recursion or Source mark/error wire. Kernel graph integration
must subsequently keep ONE compatible syntax/construction authority; the existing
lossless editor may remain an explicitly separate authoring/inspection capability,
not an alternate Source fallback. No SourceFaithful label, callback/CLI registration,
aggregate/release/publication or performance claim follows from a prototype.

## Actual P1 handoff and parent inspection

The completion was serialized tool-call text, not an executed final-report
append. The actual `/tmp/sk-p1-safe-yaml-event-api-report.md` remains provisional;
`.Y3k1Os` preserved it and the actual prototype before inspection. No quoted
payload was executed, missing report reconstructed, or replacement agent started.
The original prototype and registry source both still verify against their
received tree manifests (`.egIaVN`). No dependency patch has been installed.

A fresh parent-only copy at `/tmp/sk-p1-parent-port.GBqAK2` fixes nine existing
raw-pointer implicit field borrows with explicit shared references inside the
existing unsafe operations, not a lint allowance. Pre-edit standard-eight map
validation is `.iwmBVB`. The typed error additionally implements `std::error::Error`.
Default-toolchain harness tests pass29 (`.R2jhO6`); API-only doctests `.RCHhze`
and explicit deny-warning docs `.1e1EyF` pass. Strict local-library Clippy
`.wSpm7I` **fails26 diagnostics in unchanged dependency code**; it is not a
clean gate. The package's complete original dev-dependency suite is not certified.

Corrected documentation distinguishes static borrowed C-string error text from
owned buffers and character columns from byte indices. Full error-pointer
lifetime/safety audit remains a requirement, not an ownership claim inferred
from successful tests. This proposal changes a component's source/provenance
and would need supply-chain/SBOM disposition even without a version-count change.

The parent replaced round-trip ruamel probing with the actual SAFE backend and
asserted all seven XDG variables, exact literal repository/resource paths and
temporary roots in both actual children. `.iGfKw9` retains raw exits/stdout/stderr,
input bytes/mode/inode/mtime and comparisons at
`/tmp/sk-p1-parent-pair.0j3y0nu7`. Parser events accept400/1000/4000 nesting; separate
Source constructor probes accept128/400 and raise RecursionError500/1000 in this
caller. Neither result defines a universal loader depth cap.

Common exposed fields match PyYAML on these16 fixtures, not whole-event/error
parity. Actual SAFE ruamel still differs at scalar/document-start marks and
accepts a Unicode anchor the Native reader refuses. Implicit flags, document
version/directives/end marks, typed error fields/wire, exact surrogate transport,
constructor scheduling and graph integration remain OPEN. The actual complete
parent package diff is preserved separately; no registry cache, RootCargo,
lockfile, Source, pack, callback registration, publication or performance approval
follows from this inspection.
