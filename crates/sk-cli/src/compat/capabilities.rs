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
                "state": "implemented",
                "mutates": true,
                "surface": "upstream",
                "evidence": "sk-install initialize; project config, metadata, agent assets and ownership manifest",
                "gaps": ["6-tier command-template sourcing uses the built-in pack plus project overrides; org-pack acquisition is not integrated"]
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
                "evidence": "sk-mission tasks.md dependency parse, frontmatter patch with preserved user content",
                "gaps": ["requirement-mapping gates", "planning commit and lanes.json refresh", "status lifecycle events"]
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
                "evidence": "sk-run-store durable start/query/advance/answer with governed authorization; upstream-shaped decision payload",
                "gaps": ["mission-board progress and guard composition", "WP workspace resolution in the decision", "decision-log git commits", "analysis currency", "lifecycle records", "retrospective hooks"]
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
                "name": "upgrade",
                "reason": "sk-install owns the upgrade/migration pipeline; its public upgrade API is not complete yet",
                "blocker": "sk-install upgrade tranche"
            },
            {
                "name": "agent status emit",
                "reason": "sk-status owns replay and validation but no canonical event-log writer yet",
                "blocker": "sk-status mutation tranche"
            },
            {
                "name": "agent status lifecycle",
                "reason": "the canonical mission-lifecycle reducer is not implemented in any Rust crate yet",
                "blocker": "sk-status lifecycle tranche"
            },
            {
                "name": "agent tasks move-task",
                "reason": "requires the status event writer plus lane transition gates",
                "blocker": "sk-status mutation tranche"
            },
            {
                "name": "agent tasks mark-status",
                "reason": "requires the status event writer plus lane transition gates",
                "blocker": "sk-status mutation tranche"
            },
            {
                "name": "agent decision open/resolve/defer/cancel",
                "reason": "decision ledger service is not implemented in Rust yet",
                "blocker": "decision-service tranche"
            },
            {
                "name": "accept",
                "reason": "acceptance gates and matrix evaluation are not implemented in Rust yet",
                "blocker": "acceptance tranche"
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
                "name": "charter activation/synthesis",
                "reason": "sk-charter is read-only today",
                "blocker": "charter mutation tranche"
            },
            {
                "name": "doctor groups",
                "reason": "diagnostic families are not ported yet",
                "blocker": "doctor tranche"
            },
            {
                "name": "tracker/service/orchestrator-api",
                "reason": "external service boundaries are not ported yet",
                "blocker": "external-contract tranche"
            }
        ],
        "known_gaps": {
            "git_commits": "No compatibility route commits to Git in this slice; payloads disclose committed=false and omit commit hashes rather than inventing them.",
            "org_packs": "The org-pack tier of template resolution is not wired; project overrides, global tiers, and the built-in pack are.",
            "human_interviews": "Interactive discovery interviews are agent-driven upstream; this CLI never prompts on stdin.",
            "byte_parity": "JSON is emitted as compact UTF-8. Python's json.dumps defaults escape non-ASCII; parsed values and key sets match."
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
