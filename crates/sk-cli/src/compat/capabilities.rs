//! The compatibility capability matrix.
//!
//! This is deliberately blunt. A route appears as `implemented` only when the
//! binary executes real domain behavior; `partial` names the exact upstream
//! fields or behaviors that are not yet supplied; `unsupported` names the
//! blocker. Scripts should consult this before invoking a compatibility route.

use serde_json::{Value, json};

/// Build the machine matrix.
pub fn matrix() -> Value {
    json!({
        "binary": "spec-kitty",
        "implementation": "native-rust",
        "python_runtime": false,
        "inventory": {
            "authority": "docs/rust/inventory/python-surface.json",
            "upstream_leaves": 244,
            "upstream_groups": 41,
            "upstream_options": 888,
            "note": "The matrix below covers only routes this binary serves; every other upstream leaf remains pending and is refused rather than simulated.",
        },
        "commands": [
            {
                "name": "init",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "full template/skill/profile/session delivery, activation, identity, canonical ProjectInitialized and ledger; CLI delivery and full post-state oracle tests",
                "gaps": ["full-delivery concurrency", "remaining upstream options and packaged/global-runtime delivery", "org-pack acquisition and independent continuation approval"]
            },
            {
                "name": "agent status validate",
                "state": "implemented",
                "mutates": false,
                "surface": "upstream",
                "evidence": "sk-status raw event schema audit, FSM edge validation, canonical materialization drift",
                "gaps": ["derived-view drift beyond status.json is not audited"]
            },
            {
                "name": "agent status materialize",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "sk-status canonical replay; status.json written atomically under the canonical Mission lock",
                "gaps": ["coordination-routed write surfaces are refused before mutation", "Mission alias/rekey lock integration is under review"]
            },
            {
                "name": "agent status emit",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "canonical Mission-locked writer, for-review commit gate, planning-lane evidence, legacy nullable identity and numbered-slug lane-head regressions",
                "gaps": ["coordination-routed missions refuse", "full YAML mirror edge-case parity is under review", "execution projection refresh", "status/lifecycle fan-out"]
            },
            {
                "name": "agent status lifecycle",
                "state": "partial",
                "mutates": false,
                "surface": "upstream",
                "evidence": "lifecycle derivation over canonical status and mission events, including oracle payload comparison",
                "gaps": ["coordination authority integration", "datetime offset spelling parity"]
            },
            {
                "name": "agent tasks move-task",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "review/feedback/independence guards plus primary/non-owned auto-commit preflight, canonical review-cycle artifacts, committed-byte durability and verified not-landed rollback regressions",
                "gaps": ["coordination writes", "commit/rollback and broader pre-review fidelity need independent review", "exotic review-cycle scalar formatting", "done merge-ancestry gate requires explicit override", "owned-checkout lifecycle"]
            },
            {
                "name": "agent tasks mark-status",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "case-insensitive ownership/idempotence and stored-topology commit/verified-rollback pipeline; flat annotations remain uncommitted like the reference",
                "gaps": ["coordination writes", "history/dossier side writes", "commit/rollback fidelity needs independent review"]
            },
            {
                "name": "agent tasks list-tasks",
                "state": "implemented",
                "mutates": false,
                "surface": "upstream",
                "evidence": "sk-mission frontmatter + sk-status canonical lanes",
                "gaps": []
            },
            {
                "name": "agent tasks status",
                "state": "partial",
                "mutates": false,
                "surface": "upstream",
                "evidence": "kanban rows, lane counts, weighted readiness, done percentage",
                "gaps": ["git-backed stale verdicts", "stalled-review flags", "auto_commit read from config"]
            },
            {
                "name": "agent tasks list-dependents",
                "state": "implemented",
                "mutates": false,
                "surface": "upstream",
                "evidence": "sk-mission dependency graph",
                "gaps": []
            },
            {
                "name": "agent mission branch-context",
                "state": "implemented",
                "mutates": false,
                "surface": "upstream",
                "evidence": "deterministic branch contract from sk-git HEAD and the primary-branch cascade",
                "gaps": []
            },
            {
                "name": "agent mission create",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "sk-mission transactional scaffold plan with canonical spec template; allocation state reported as planning_artifacts_only",
                "gaps": ["coordination-topology branch minting/materialization", "git safe-commit of scaffold artifacts", "mission_number allocation", "origin binding"]
            },
            {
                "name": "specify",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "slug validation + mission create + scaffold_only guidance",
                "gaps": ["same create gaps", "specify widen interview (interactive only upstream)"]
            },
            {
                "name": "agent mission setup-plan",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "canonical plan template resolution and plan.md scaffold",
                "gaps": ["substantive-content gate", "planning commit", "lifecycle phase events"]
            },
            {
                "name": "plan",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "delegates to the setup-plan slice",
                "gaps": ["same plan gaps", "interactive plan interview"]
            },
            {
                "name": "agent mission finalize-tasks",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "dependency/occurrence/glob gates, PRIMARY prompt scan, established COORD status placement, captured planning tip, per-seed status publication and branch-ref validate-only reads; completed planning writes remain on failure",
                "gaps": ["published/E2 status publication and ambiguous read-probe refusals", "warning-and-skip and validation order", "target branch checkout, ErrorLogged and source wire fields", "indeterminate commit/recovery and independent Source/workflow verification"]
            },
            {
                "name": "tasks",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "delegates to the finalize-tasks slice",
                "gaps": ["same finalize gaps"]
            },
            {
                "name": "implement",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "sk-workspace lane allocation, protected-branch refusal, planning-lane repo-root placement",
                "gaps": ["dependency-branch merge preflight", "bulk-edit gate", "workspace context persistence", "auto-commit"]
            },
            {
                "name": "next",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "read-only no-run preview and durable next query/advance/answer; finalized WP-board override, guard re-issue, weighted progress, origin tier, WP workspace resolution, contained prompt artifacts, analysis-report currency, human-actor answers",
                "gaps": ["StepContract composition dispatch and invocation records", "requirement-mapping and occurrence-map gates (the tasks step fails closed)", "decision-log git commits", "lifecycle records", "retrospective completion gate", "coordination-topology workspaces", "cross-implementation prompt-file paths"]
            },
            {
                "name": "runtime start",
                "state": "implemented",
                "mutates": true,
                "surface": "native-extension",
                "evidence": "sk-run-store start contract",
                "gaps": []
            },
            {
                "name": "runtime query",
                "state": "implemented",
                "mutates": false,
                "surface": "native-extension",
                "evidence": "sk-run-store non-mutating query contract",
                "gaps": []
            },
            {
                "name": "runtime advance",
                "state": "implemented",
                "mutates": true,
                "surface": "native-extension",
                "evidence": "sk-run-store transactional advance contract",
                "gaps": []
            },
            {
                "name": "runtime answer",
                "state": "implemented",
                "mutates": true,
                "surface": "native-extension",
                "evidence": "sk-run-store governed answer contract",
                "gaps": []
            },
            {
                "name": "runtime recover",
                "state": "implemented",
                "mutates": true,
                "surface": "native-extension",
                "evidence": "sk-run-store journal/cursor reconciliation",
                "gaps": []
            },
            {
                "name": "workspace recover",
                "state": "implemented",
                "mutates": true,
                "surface": "native-extension",
                "evidence": "sk-workspace allocation journal reconciliation",
                "gaps": []
            },
            {
                "name": "upgrade",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "canonical sequential main and inherited-scope worktree execution; six charter callbacks and shared previews reachable through registry",
                "gaps": ["four rescue callbacks withheld", "exact parser subclass/ledger-note/error-wire fidelity", "remaining preview and machine planner contract differences", "cutover/model/provider/owned-checkout boundaries and independent Source workflows"]
            },
            {
                "name": "migrate backfill-runtime-state",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "real selector-bound or best-effort corpus cutover, dry-run, group flags, seed/verify/flip and owned seed-root forwarding; scoped native smoke",
                "gaps": ["owned birth-cutover production caller", "selector/error output channel and exact cause fidelity", "cross-process owned lock proof and independent Source/COORD workflow verification"]
            },
            {
                "name": "accept",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "Source collector/policy and real matrix/record pipeline, non-WP frozen-runtime/declared-artifact facts and activated sparse mission providers; read-only diagnose",
                "gaps": ["owned-file evidence and attest flags", "non-WP coordination presence and lazy WP reads", "sparse roster inheritance/action projection and provider coverage", "normalize-encoding/checklists/allow-fail", "independent isolated Source differential and recovery/error coverage"]
            },
            {
                "name": "charter",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "exact merged/profile include payload reaches Kernel-JSON CLI; canonical skill repository shared with installer, layer-aware list availability/floors, templates, activation and preset publication/restamp; scoped functional receipts",
                "gaps": ["resynthesize refused; other activation/migration restamp consumers pending", "status org/governance/provenance/evidence and bundle compatibility", "directive prevalidation abort phase, raw org downstream coercing readers/typed-fold and namespace timing, strict deny-floor and explicit-empty activation fidelity", "action/bootstrap declaring-file scalar delivery, exact skill/list interfaces, non-string model keys and stdout codecs", "context budgeting/field-provenance consumption, Source error/wire/ordering, constructor/depth/platform/concurrency proof"]
            },
            {
                "name": "doctor",
                "state": "partial",
                "mutates": true,
                "surface": "upstream",
                "evidence": "thirteen reachable leaves including registry diagnostics, decision reconciliation/fork, stale-Op sweep and read-only command/skill/session probes; scoped functional smoke",
                "implemented_leaves": super::doctor::IMPLEMENTED_LEAVES,
                "refused_leaves": super::doctor::REFUSED_LEAVES,
                "gaps": ["decision repair, lifecycle-owner promotion and event-prefix classifier sharing; coordination collector composed but root route held for committed-event admission and input/error fidelity", "tool-surface repairs and remaining six diagnostic leaves refused", "plugin-bundle probes, profile/pack catalogs, canonical-render collisions and legacy agent configuration", "session JSON parsing, repeated-read/race semantics and escaping-error/traceback wire; registry advisory sweep/error wire", "Op SaaS propagation, error text and spine race semantics", "meta size/integer/pattern/roster bounds", "human/wire output and independent Source/platform/race verification"]
            },
            {
                "name": "native",
                "state": "implemented",
                "mutates": false,
                "surface": "native-inspection",
                "evidence": "the versioned inspection envelope: project, git, mission inventory, charter, glossary, planner query, event stats, FSM check",
                "gaps": []
            }
        ],
        "unsupported": [
            {
                "name": "agent decision open/resolve/defer/cancel",
                "reason": "decision ledger service is not implemented in Rust yet",
                "blocker": "decision-service tranche"
            },
            {
                "name": "consolidate",
                "reason": "merge/consolidation protocols are not implemented in Rust yet",
                "blocker": "consolidation tranche"
            },
            {
                "name": "review",
                "reason": "post-merge review scanning is not implemented in Rust yet",
                "blocker": "review tranche"
            },
            {
                "name": "tracker/service/orchestrator-api",
                "reason": "external service boundaries are not ported yet",
                "blocker": "external-contract tranche"
            }
        ],
        "known_gaps": {
            "git_commits": "Commit-capable routes use canonical sk-git; route-specific commit, recovery and physical-publication fidelity remains under review.",
            "org_packs": "Canonical explicit-root templates resolve org tiers; Source template includes preserve their project-layer base. Other template consumers and DRG catalog projection remain incomplete.",
            "human_interviews": "Interactive discovery interviews are agent-driven upstream; this CLI never prompts on stdin.",
            "byte_parity": "Canonical exact codecs exist in Kernel; route-specific wire/error/ordering fidelity is not globally approved by functional smoke tests."
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_implemented_route_names_evidence() {
        let report = matrix();
        for command in report["commands"].as_array().expect("commands") {
            assert!(
                command["evidence"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "missing evidence: {command}"
            );
        }
    }
}
