---
type: explanation
updated: 2026-11-02
audience: software-engineer
---
# Rust rewrite tooling notes

- Subagent model selection required the exact provider-qualified catalog id, `tokenfactory/deepseek-v4.1-flash-1017`; no alternate model was substituted. Async completion notifications, rather than task polling, drove integration.
- Rust was absent from the VM. Rustup installed the minimal toolchain; the repository now pins 1.99.1 with formatter and Clippy components. The declared compiler floor matches the tested release line.
- The shared VM has other workloads. Agent crate checks and coordinator aggregate checks used `CARGO_BUILD_JOBS=2`; final aggregate evidence is coordinator-owned. The first kernel build used an external target directory to avoid incomplete sibling manifests.
- Changing the declared Rust minimum from an untested 1.85 to 1.99 enabled additional Clippy suggestions (notably let-chains). These were fixed in code, not suppressed; aggregate tests, Clippy and formatting were rerun.
- Green initial unit tests missed FIFO opens that block before metadata checks, an escaping asset-directory anchor, and a complete non-LF event tail. Independent review and real-process regressions found these classes; the common I/O boundary was unified rather than patching only the CLI.
- The account already had an older public fork. Its `main` was 6,097 commits behind the fetched upstream baseline. The Rust topic branch contains the upstream refresh, while the existing `main` was preserved; the draft PR explicitly describes the inherited comparison and provides the Rust-only baseline.
- Pack trees are pinned upstream artifacts, not committed content in this checkout. Charter inspection refuses missing declared roots. Fetching packs is a separate upstream build-tooling/network action; no Rust command silently fetches or invokes Python.
- Planning approach and remaining work are maintained in `STATUS.md`; design decisions in `DECISIONS.md`; verification and review dispositions in `VERIFICATION.md`. These artifacts describe the delivered foundation, not a claim that the full rewrite is finished.
