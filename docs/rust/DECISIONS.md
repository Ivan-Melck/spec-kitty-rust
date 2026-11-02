---
type: explanation
updated: 2026-11-02
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

Upstream has moved event writes behind a separately released service. The Rust implementation must not append to existing upstream event ledgers or silently invent a substitute protocol. Initial upstream status support is read-only. Native Rust persistence, if added, requires an explicitly different versioned format, explicit initialization, and its own integrity/concurrency tests. Read-only compatibility is not write parity.

User-authored YAML and Markdown are never reserialized merely to inspect them. This avoids losing comments and formatting through a non-round-trip YAML serializer. Pack content is not bundled from the maintainer-only internal tier. Paths supplied by packs and users are untrusted; traversal and symlink escape must be rejected at the owning boundary.

## Delivery and verification

The full upstream surface is much larger than a single implementation tranche. A capability matrix must distinguish implemented, partially implemented, and pending behavior. Passing Rust tests establishes the covered contracts, not blanket compatibility with the Python implementation. No performance claim is made without a reproducible measurement.

New domain work starts from observable acceptance tests, then implementation. Subagents own non-overlapping crate directories; the coordinator owns workspace manifests, integration, documentation, publication, and independent review. Only the coordinator runs whole-workspace checks, with bounded job counts on the shared VM. Publication uses a topic branch and draft PR; the operator merges.
