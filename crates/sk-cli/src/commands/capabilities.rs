//! The machine-readable capability matrix.
//!
//! This is the authority a script or agent should consult before trying a
//! command. It is intentionally blunt about what is absent: a refusal is safer
//! than a simulated success.

use serde_json::json;

use crate::output::CommandData;

/// Report the implemented, partial and unsupported surfaces.
pub fn report() -> CommandData {
    CommandData::success(json!({
        "binary": "spec-kitty-rust",
        "implementation": "partial-rewrite",
        "status": "read-only inspection adapter",
        "python_runtime": false,
        "mutates_project": false,
        "full_next": false,
        "event_ledger_writes": false,
        "network_access": false,
        "schema_version": 1,
        "commands": [
            {
                "name": "capabilities",
                "state": "implemented",
                "mutates": false,
                "description": "Report this capability matrix."
            },
            {
                "name": "project root",
                "state": "implemented",
                "mutates": false,
                "description": "Discover the project root from an explicit path or the current directory."
            },
            {
                "name": "git inspect",
                "state": "implemented",
                "mutates": false,
                "description": "Read branch, HEAD, dirtiness and porcelain status; no worktree operations."
            },
            {
                "name": "mission list",
                "state": "implemented",
                "mutates": false,
                "description": "List Missions from kitty-specs/*/meta.json with all-or-nothing validation."
            },
            {
                "name": "mission show",
                "state": "implemented",
                "mutates": false,
                "description": "Show one Mission via a strict full-id, mid8 or exact-slug selector; no fallback."
            },
            {
                "name": "mission graph",
                "state": "implemented",
                "mutates": false,
                "description": "Validate and report the work-package dependency DAG; no state reconstruction."
            },
            {
                "name": "charter inspect",
                "state": "implemented",
                "mutates": false,
                "description": "Inspect config, charter bundle, pack descriptors/roots and assets without reserializing them."
            },
            {
                "name": "glossary lookup",
                "state": "implemented",
                "mutates": false,
                "description": "Resolve a term with explicit scopes and optional explicit seed files."
            },
            {
                "name": "runtime query",
                "state": "partial",
                "mutates": false,
                "description": "Pure planner query over an explicit frozen template and run snapshot; not full governed next and never advances a run."
            },
            {
                "name": "events inspect",
                "state": "implemented",
                "mutates": false,
                "description": "Bounded event-stream statistics; not a reducer and never a current-lane claim."
            },
            {
                "name": "transition check",
                "state": "implemented",
                "mutates": false,
                "description": "Pure lane-FSM validation only; never persists a transition or writes an event."
            }
        ],
        "unsupported_list_exhaustive": false,
        "unlisted_commands": "unsupported; refused with exit 2",
        "path_output": "path strings are display-only; Unix Git entries additionally expose lossless path_bytes_hex",
        "unsupported": [
            {
                "name": "init",
                "reason": "Project initialization and file generation are not implemented."
            },
            {
                "name": "upgrade",
                "reason": "Generated-command and migration upgrades are not implemented."
            },
            {
                "name": "worktrees",
                "reason": "Worktree creation, placement, repair and cleanup are not implemented; git inspect is read-only."
            },
            {
                "name": "merge",
                "reason": "Lane/mission merge and consolidation are not implemented."
            },
            {
                "name": "activation",
                "reason": "Charter activation and ledger writes are not implemented."
            },
            {
                "name": "event-writes",
                "reason": "Event-ledger appends belong to the external events service; this adapter never writes them."
            },
            {
                "name": "next",
                "reason": "The full governed next workflow (run persistence, prompt emission, workspace resolution, gates) is not implemented. runtime query is a pure planner query only."
            },
            {
                "name": "status-writes",
                "reason": "Lane mutation and status persistence are not implemented."
            },
            {
                "name": "mission-create",
                "reason": "Mission creation and meta.json writes are not implemented."
            }
        ],
        "exit_codes": {
            "0": "success (including step, decision_required and terminal runtime queries)",
            "1": "domain/data refusal or I/O failure (including unresolved filesystem paths)",
            "2": "usage error (unknown command, malformed syntax or invalid scalar option value)",
            "3": "runtime query returned a blocked decision"
        },
        "json_output": "one JSON document for operational commands; help/version are JSON with --json, human-readable otherwise; diagnostics on stderr",
        "notes": [
            "There is no Python runtime fallback.",
            "Unsupported commands are refused with a usage error, never simulated.",
            "User-authored YAML and Markdown are inspected, never reserialized.",
            "charter inspect fails on missing declared pack roots; source checkouts may require make sync-packs (network needed) before inspection.",
            "Explicit --project-root is an authoritative marked root, not a discovery start; invalid filesystem inputs exit 1."
        ]
    }))
}
