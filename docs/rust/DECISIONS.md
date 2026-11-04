---
type: explanation
updated: 2026-11-04
audience: software-engineer
---
# Rust rewrite decisions

## Authority and baseline

The operator explicitly requested a Rust rewrite in the public fork `Ivan-Melck/spec-kitty-rust`. This overrides the upstream charter's Python-only implementation requirement for the new Rust workspace, not for upstream Python maintenance. Upstream provenance, copyright notices, canonical Mission terminology, user customization preservation, and fail-closed integrity requirements remain binding. No upstream merge or mainline publication is authorized.

The source baseline is `spec-kitty/spec-kitty` commit `9aed3a8012cb5eb22c0128b14d525039f95fea7a` (2026-11-02). Existing Python sources remain the reference implementation during migration. They are not invoked by Rust at runtime.

## Design direction

This is a domain-oriented rewrite, not a module-by-module translation. Cargo dependency edges enforce bounded contexts; private validated newtypes distinguish identifiers and paths; exhaustive enums model closed states; fallible operations return typed errors; filesystem resources are scoped by RAII; side effects are separated from deterministic decisions. Unsafe Rust is forbidden in workspace crates.

Initial bounded contexts are kernel, Git adapter, charter/pack reads, glossary resolution, Mission discovery and dependency planning, status replay, runtime decisions, and a CLI adapter. The adapter must call implemented domain behavior; absent operations must be refused, never simulated as successful.

## Migration safety

Upstream is moving event writes behind separately released service contracts. The initial foundation is read-only; this is a migration fence, not permission to omit writes from the full rewrite. The operator's continuation request requires native mutations too. Lift that fence only after implementing the actual pinned-baseline wire format, canonical writer/lock authority, validation, durable batch/readback, recovery and interoperability contracts with conformance and concurrency tests. Never silently invent a replacement protocol or add an uncoordinated second writer. External services remain external contract dependencies, not vendored reimplementations. Explicitly distinct experimental formats cannot count as upstream write parity. Read-only compatibility is not write parity.

Writer repository selection must be coherent with the Git adapter. Raw topology
queries retain inherited Git-environment semantics, but current writer authority
boundaries explicitly refuse inherited repository selectors rather than silently
splitting lock and mutation namespaces. This is a temporary fail-closed supported-
input restriction, not complete environment-override mutation parity. A shared,
captured repository context is required before lifting it. Required development
oracle comparisons likewise fail on missing or drifted dependencies; they must
never turn an unavailable reference into a passing zero-comparison test.

User-authored YAML and Markdown are never reserialized merely to inspect them. This avoids losing comments and formatting through a non-round-trip YAML serializer. Pack content is not bundled from the maintainer-only internal tier. Paths supplied by packs and users are untrusted; traversal and symlink escape must be rejected at the owning boundary.

## Distinct JSON read contracts

The executor's local first-object legacy read is deliberately looser than typed
Op CLAIM/model validation. A private ordered value profile preserves Python dict
order, duplicate-key behavior and nonfinite/overflow floats there without changing
the standard wire writer or enabling serde's workspace-wide preserve_order feature.
String/float rendering reuses the existing primitive authority. Python strings
retain code points, including surrogate units; one repr formatter uses verified,
reproducibly generated CPython printability data. Encoding an actual Git message
is a separate late capability boundary, not lossy replacement or parse invalidity. This is not a second
writer or arbitrary-JSON parity claim; unsupported source inputs stay findings.
The pinned interpreter's default1000 recursion budget bounds native reader stack
use as an operational guard, not a fixed JSON-format or caller-ceiling claim.
Default4300 integer conversion respects existing PYTHONINTMAXSTRDIGITS startup
configuration (not a new Spec Kitty knob); floats are not integer-digit-limited.

## Shared Python-string ownership (H1a)

The single PythonString/InvalidCodepoint type, inherent repr method, canonical
formatter and existing CPython3.11.15/UCD14 printability table belong in Kernel.
Invocation retains its public names/methods through exact re-exports; no wrapper,
second nominal type, formatter or dependency cycle. Checked owned/append seams
preserve the type's valid range including surrogates. The primitive never pairs
surrogates; legacy JSON pairing remains a reader rule, not a universal YAML rule.
No manifest/lock/dependency/pin change. See `reviews/shared-python-string-disposition.md`.
This foundation does not approve the colliding NUL-shadow transport, retype the
profile API, introduce graph refusals, or fix the Source-valid loader data loss.

## Portable ordinary-commit lock identity

The ordinary COMMIT_EDITMSG transaction uses Git's existing captured-index lock
path, not a new advisory-lock namespace. Keep an open file identity and validate
it across hooks and before publication; path/size/mtime or fabricated zero IDs
are not ownership. If an object already exists when ownership is lost, retain
candidate/ref/readback evidence rather than returning an ordinary no-candidate
error. No shared-message write follows detected pre-commit ownership loss.

Coordinator-approved dependency addition: `same-file = "=1.0.6"` (MIT choice),
with locked Windows-only `winapi-util 0.1.11`; existing windows-sys0.61.2 remains.
The owner's Windows std identity calls are still unstable (`windows_by_handle`)
in the PINNED Rust commit58cf8f9c3, so retaining them would introduce a Windows
compile regression. Existing fs2 supplies advisory locking, not file identity;
a small safe cross-platform API avoids workspace unsafe/FFI or guessed identity.
This explicitly extends the owner's no-dependency scope under coordinator
manifest/lock ownership. It does NOT repin Python/Source/packs or certify Windows
runtime/filesystems. Registry/checksum/freshness/no-build-script/incident-query
and lock-inventory evidence: `dependencies/README.md`.

## Delivery and verification

The operator requires autonomous continuation through the full rewrite and a substantial verified speed increase; `LOOP.md` defines the control loop and completion gates. The full upstream surface is much larger than a single implementation tranche. A capability matrix must distinguish implemented, partially implemented, and pending behavior. Passing Rust tests establishes the covered contracts, not blanket compatibility with the Python implementation. No performance claim is made without a reproducible measurement.

New domain work starts from observable acceptance tests, then implementation. Subagents own non-overlapping crate directories; the coordinator owns workspace manifests, integration, documentation, publication, and independent review. Only the coordinator runs whole-workspace checks, with bounded job counts on the shared VM. Publication uses a topic branch and draft PR; the operator merges.
