# QUARANTINED-BY-C-006: dev-only golden-fixture generator.
#
# This file is NOT a production code path. It regenerates the committed golden
# fixtures under `crates/sk-runtime/tests/golden/` from the pinned upstream
# Python runtime (`runtime.next._internal_runtime`) so the Rust pure policy can
# be differentially tested against the reference implementation.
#
# It must never be invoked by runtime code and is not run by `cargo test`.
#
# Usage (from the repository root):
#
#   SPEC_KITTY_PACKS_ROOT=/tmp/skpacks \
#   PYTHONPATH=src \
#   /home/exedev/.local/share/uv/tools/spec-kitty-cli/bin/python \
#   crates/sk-runtime/tests/oracle/generate_goldens.py
#
# `SPEC_KITTY_PACKS_ROOT` must name a directory containing a synced
# `built-in` pack tree (the internal runtime imports the charter package at
# import time). Fixture state lives in isolated temporary directories; the
# generator never touches a real project or a real run store.

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path
from typing import Any

import yaml

from runtime.next._internal_runtime import engine
from runtime.next._internal_runtime.schema import (
    ActorIdentity,
    MissionPolicySnapshot,
    MissionRunSnapshot,
    MissionRuntimeError,
)
from runtime.next._internal_runtime.significance import evaluate_significance
from runtime.next._internal_runtime.planner import compose_template_with_workflow
from runtime.next._internal_runtime.workflow_registry import (
    get_workflow,
    list_available_workflows,
    resolve_workflow_path,
)
from runtime.next._internal_runtime.workflow_schema import WorkflowSequence

LOW_DIMS = {
    "user_customer_impact": 0,
    "architectural_system_impact": 0,
    "data_security_compliance_impact": 0,
    "operational_reliability_impact": 0,
    "financial_commercial_impact": 0,
    "cross_team_blast_radius": 0,
}
MEDIUM_DIMS = {
    **LOW_DIMS,
    "user_customer_impact": 3,
    "operational_reliability_impact": 3,
    "architectural_system_impact": 2,
}
HIGH_DIMS = dict.fromkeys(LOW_DIMS, 3)

_TIMESTAMP = re.compile(
    r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?"
)


def _normalize(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: _normalize(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_normalize(v) for v in value]
    if isinstance(value, str):
        return _TIMESTAMP.sub("<TIMESTAMP>", value)
    return value


def _gate_template(significance: dict[str, Any] | None, *, extra_step: bool = True) -> dict[str, Any]:
    steps: list[dict[str, Any]] = [
        {"id": "lead_in", "title": "Lead-in", "description": "lead", "prompt": "Run it."}
    ]
    if extra_step:
        steps.append(
            {
                "id": "follow_up",
                "title": "Follow-up",
                "description": "follow",
                "prompt": "Follow.",
                "depends_on": ["review_gate"],
            }
        )
    audit: dict[str, Any] = {
        "id": "review_gate",
        "title": "Review Gate",
        "description": "gate",
        "audit": {"trigger_mode": "manual", "enforcement": "blocking"},
        "depends_on": ["lead_in"],
    }
    if significance is not None:
        audit["significance"] = significance
    return {
        "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
        "steps": steps,
        "audit_steps": [audit],
    }


def _plain_template() -> dict[str, Any]:
    return {
        "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
        "steps": [
            {"id": "s1", "title": "S1", "description": "one", "prompt": "do", "requires_inputs": ["topic"]},
            {"id": "s2", "title": "S2", "description": "two", "prompt": "then", "depends_on": ["s1"]},
        ],
    }


def _snapshot(**overrides: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "run_id": "run-1",
        "mission_key": "m",
        "template_path": "/nonexistent/runtime-golden/mission.yaml",
        "template_hash": "frozen-hash",
        "policy_snapshot": MissionPolicySnapshot().model_dump(mode="json"),
        "completed_steps": [],
        "issued_step_id": None,
        "inputs": {},
        "decisions": {},
        "pending_decisions": {},
        "blocked_reason": None,
        "mission_id": None,
        "mission_slug": None,
    }
    base.update(overrides)
    return MissionRunSnapshot.model_validate(base).model_dump(mode="json")


def _significance_record(dimensions: dict[str, int], hard_triggers: list[str], cutoffs: Any) -> dict[str, Any]:
    return evaluate_significance(dimensions, hard_triggers, cutoffs).model_dump(mode="json")


def _pending_request(decision_id: str, step_id: str, question: str, options: list[str]) -> dict[str, Any]:
    return {
        "decision_id": decision_id,
        "step_id": step_id,
        "question": question,
        "options": options,
        "requested_by": {
            "actor_id": "agent-1",
            "actor_type": "llm",
            "display_name": "",
            "provider": None,
            "model": None,
            "tool": None,
        },
        "requested_at": "2026-01-01T00:00:00Z",
    }


AUDIT_REQUEST = _pending_request(
    "audit:review_gate", "review_gate", "Audit checkpoint: Review Gate. Approve to continue?", ["approve", "reject"]
)
INPUT_REQUEST = _pending_request("input:topic", "s1", "Input required before step 's1': provide value for 'topic'.", [])


def _plan_scenarios() -> list[dict[str, Any]]:
    return [
        {
            "name": "plan_low_band_auto_proceeds",
            "kind": "plan",
            "template": _gate_template({"dimensions": LOW_DIMS, "hard_triggers": []}),
            "snapshot": _snapshot(
                completed_steps=["lead_in"],
                inputs={"mission_owner_id": "owner-1"},
            ),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_low_band_without_owner_records_sentinels",
            "kind": "plan",
            "template": _gate_template({"dimensions": LOW_DIMS, "hard_triggers": []}),
            "snapshot": _snapshot(completed_steps=["lead_in"]),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_medium_band_soft_gate",
            "kind": "plan",
            "template": _gate_template({"dimensions": MEDIUM_DIMS, "hard_triggers": []}),
            "snapshot": _snapshot(completed_steps=["lead_in"], inputs={"mission_owner_id": "owner-1"}),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_high_band_hard_trigger",
            "kind": "plan",
            "template": _gate_template(
                {"dimensions": LOW_DIMS, "hard_triggers": ["security_privacy_access_control"]}
            ),
            "snapshot": _snapshot(completed_steps=["lead_in"], inputs={"mission_owner_id": "owner-1"}),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_high_band_composite",
            "kind": "plan",
            "template": _gate_template({"dimensions": HIGH_DIMS, "hard_triggers": []}),
            "snapshot": _snapshot(completed_steps=["lead_in"], inputs={"mission_owner_id": "owner-1"}),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_custom_band_cutoffs_upgrade_medium_to_high",
            "kind": "plan",
            "template": _gate_template({"dimensions": MEDIUM_DIMS, "hard_triggers": []}),
            "snapshot": _snapshot(
                completed_steps=["lead_in"],
                inputs={"mission_owner_id": "owner-1"},
                policy_snapshot={
                    "strictness": "max",
                    "default_route": "same_llm_context",
                    "extras": {
                        "significance_band_cutoffs": {
                            "low": [0, 3],
                            "medium": [4, 7],
                            "high": [8, 18],
                        }
                    },
                },
            ),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_plain_step_records_inferred_raci",
            "kind": "plan",
            "template": _plain_template(),
            "snapshot": _snapshot(inputs={"topic": "x"}),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_pending_audit_reuses_recorded_significance",
            "kind": "plan",
            "template": _gate_template(
                {"dimensions": LOW_DIMS, "hard_triggers": ["architecture_foundation"]}
            ),
            "snapshot": _snapshot(
                completed_steps=["lead_in"],
                inputs={"mission_owner_id": "owner-1"},
                pending_decisions={"audit:review_gate": AUDIT_REQUEST},
                decisions={
                    "significance:audit:review_gate": _significance_record(
                        MEDIUM_DIMS, [], None
                    ),
                },
            ),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_explicit_raci_is_recorded",
            "kind": "plan",
            "template": {
                "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
                "steps": [
                    {
                        "id": "s1",
                        "title": "S1",
                        "description": "one",
                        "prompt": "do",
                        "raci": {
                            "responsible": {"actor_type": "llm", "actor_id": "claude"},
                            "accountable": {"actor_type": "human", "actor_id": "owner-1"},
                            "consulted": [{"actor_type": "human"}],
                            "informed": [{"actor_type": "llm", "actor_id": "  "}],
                        },
                        "raci_override_reason": "explicit oversight",
                    }
                ],
            },
            "snapshot": _snapshot(),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_advisory_audit_is_a_step",
            "kind": "plan",
            "template": {
                "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
                "steps": [{"id": "lead_in", "title": "Lead-in", "description": "lead", "prompt": "Run it."}],
                "audit_steps": [
                    {
                        "id": "advisory",
                        "title": "Advisory Audit",
                        "description": "adv",
                        "audit": {"trigger_mode": "manual", "enforcement": "advisory"},
                        "depends_on": ["lead_in"],
                    }
                ],
            },
            "snapshot": _snapshot(completed_steps=["lead_in"]),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_pending_input_decision",
            "kind": "plan",
            "template": _plain_template(),
            "snapshot": _snapshot(pending_decisions={"input:topic": INPUT_REQUEST}),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_blocked_run",
            "kind": "plan",
            "template": _plain_template(),
            "snapshot": _snapshot(blocked_reason="manual block"),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_terminal_run",
            "kind": "plan",
            "template": _plain_template(),
            "snapshot": _snapshot(completed_steps=["s1", "s2"], inputs={"topic": "x"}),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_unschedulable_run",
            "kind": "plan",
            "template": _plain_template(),
            "snapshot": _snapshot(issued_step_id="s1"),
            "agent_id": "agent-1",
        },
        {
            "name": "plan_template_drift",
            "kind": "plan",
            "template": _plain_template(),
            "snapshot": _snapshot(inputs={"topic": "x"}),
            "agent_id": "agent-1",
            "drift": True,
        },
    ]


def _answer_scenarios() -> list[dict[str, Any]]:
    owner = {"actor_id": "owner-1", "actor_type": "human", "display_name": "", "provider": None, "model": None, "tool": None}
    llm = {"actor_id": "bot", "actor_type": "llm", "display_name": "", "provider": None, "model": None, "tool": None}

    def input_snapshot(inputs: dict[str, Any] | None = None) -> dict[str, Any]:
        merged = {"llm_delegations": {}} if inputs is None else inputs
        return _snapshot(pending_decisions={"input:topic": INPUT_REQUEST}, inputs=merged)

    def audit_snapshot(band: dict[str, Any] | None, inputs: dict[str, Any] | None = None) -> dict[str, Any]:
        decisions: dict[str, Any] = {}
        if band is not None:
            decisions["significance:audit:review_gate"] = band
        return _snapshot(
            completed_steps=["lead_in"],
            inputs={"mission_owner_id": "owner-1"} if inputs is None else inputs,
            decisions=decisions,
            pending_decisions={"audit:review_gate": AUDIT_REQUEST},
        )

    high = _significance_record(LOW_DIMS, ["security_privacy_access_control"], None)
    medium = _significance_record(MEDIUM_DIMS, [], None)

    return [
        # Input decisions
        {
            "name": "answer_input_human",
            "kind": "answer",
            "snapshot": input_snapshot(),
            "decision_id": "input:topic",
            "answer": "alpha",
            "actor": owner,
        },
        {
            "name": "answer_input_llm_delegated",
            "kind": "answer",
            "snapshot": input_snapshot(
                {
                    "llm_delegations": {
                        "input:topic": {
                            "authority_role": "trusted_bot",
                            "rationale_linkage": "  owner approved  ",
                        }
                    }
                }
            ),
            "decision_id": "input:topic",
            "answer": "gamma",
            "actor": llm,
        },
        {
            "name": "answer_input_llm_not_delegated",
            "kind": "answer",
            "snapshot": input_snapshot(),
            "decision_id": "input:topic",
            "answer": "x",
            "actor": llm,
        },
        {
            "name": "answer_input_llm_delegation_without_rationale",
            "kind": "answer",
            "snapshot": input_snapshot({"llm_delegations": {"*": {"authority_role": "delegated_llm"}}}),
            "decision_id": "input:topic",
            "answer": "x",
            "actor": llm,
        },
        {
            "name": "answer_unknown_decision",
            "kind": "answer",
            "snapshot": input_snapshot(),
            "decision_id": "input:nope",
            "answer": "x",
            "actor": owner,
        },
        # Audit decisions without significance
        {
            "name": "answer_audit_approve_without_significance",
            "kind": "answer",
            "snapshot": audit_snapshot(None),
            "decision_id": "audit:review_gate",
            "answer": "approve",
            "actor": owner,
        },
        {
            "name": "answer_audit_reject_without_significance",
            "kind": "answer",
            "snapshot": audit_snapshot(None),
            "decision_id": "audit:review_gate",
            "answer": "reject",
            "actor": owner,
        },
        {
            "name": "answer_audit_invalid_answer",
            "kind": "answer",
            "snapshot": audit_snapshot(None),
            "decision_id": "audit:review_gate",
            "answer": "yolo",
            "actor": owner,
        },
        # Authority denials
        {
            "name": "answer_audit_denied_llm_actor",
            "kind": "answer",
            "snapshot": audit_snapshot(None),
            "decision_id": "audit:review_gate",
            "answer": "approve",
            "actor": llm,
        },
        {
            "name": "answer_audit_denied_wrong_owner",
            "kind": "answer",
            "snapshot": audit_snapshot(None),
            "decision_id": "audit:review_gate",
            "answer": "approve",
            "actor": {**owner, "actor_id": "intruder"},
        },
        {
            "name": "answer_audit_denied_missing_owner",
            "kind": "answer",
            "snapshot": audit_snapshot(None, inputs={}),
            "decision_id": "audit:review_gate",
            "answer": "approve",
            "actor": owner,
        },
        # High band
        {
            "name": "answer_audit_high_approve",
            "kind": "answer",
            "snapshot": audit_snapshot(high),
            "decision_id": "audit:review_gate",
            "answer": "approve",
            "actor": owner,
        },
        {
            "name": "answer_audit_high_rejects_soft_action",
            "kind": "answer",
            "snapshot": audit_snapshot(high),
            "decision_id": "audit:review_gate",
            "answer": "decide_solo",
            "actor": owner,
        },
        # Medium band
        {
            "name": "answer_audit_medium_decide_solo",
            "kind": "answer",
            "snapshot": audit_snapshot(medium),
            "decision_id": "audit:review_gate",
            "answer": "decide_solo",
            "actor": owner,
        },
        {
            "name": "answer_audit_medium_defer_keeps_gate",
            "kind": "answer",
            "snapshot": audit_snapshot(medium),
            "decision_id": "audit:review_gate",
            "answer": "defer",
            "actor": owner,
        },
        {
            "name": "answer_audit_medium_rejects_approve",
            "kind": "answer",
            "snapshot": audit_snapshot(medium),
            "decision_id": "audit:review_gate",
            "answer": "approve",
            "actor": owner,
        },
    ]


def _compose_scenarios() -> list[dict[str, Any]]:
    return [
        {
            "name": "compose_virtual_discovery_prefix",
            "kind": "compose",
            "template": {
                "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
                "steps": [
                    {
                        "id": "discovery",
                        "title": "Discovery",
                        "description": "d",
                        "prompt": "discover",
                        "requires_inputs": ["topic"],
                        "depends_on": ["legacy"],
                    },
                    {
                        "id": "specify",
                        "title": "Specify",
                        "description": "old",
                        "prompt": "spec",
                        "raci": {
                            "responsible": {"actor_type": "llm", "actor_id": "claude"},
                            "accountable": {"actor_type": "human", "actor_id": "owner"},
                        },
                        "raci_override_reason": "explicit",
                        "agent_profile": "source-profile",
                        "contract_ref": "contract-1",
                    },
                ],
                "audit_steps": [
                    {
                        "id": "gate",
                        "title": "Gate",
                        "audit": {"trigger_mode": "manual", "enforcement": "blocking"},
                        "depends_on": ["specify"],
                    }
                ],
            },
            "workflow": {
                "workflow_id": "software-dev-default",
                "description": "A workflow.",
                "initial": "specify",
                "version": 1,
                "actions": [
                    {
                        "action_name": "specify",
                        "next": ["mission-review"],
                        "description": "Do specify.",
                        "agent_profile": "action-profile",
                    },
                    {
                        "action_name": "mission-review",
                        "next": [],
                        "description": "",
                        "terminal": True,
                        "human_in_the_loop": "operator_approval",
                    },
                ],
            },
        },
        {
            "name": "compose_named_discovery_action",
            "kind": "compose",
            "template": {
                "mission": {"key": "m", "name": "M", "version": "1.0.0", "description": ""},
                "steps": [{"id": "discovery", "title": "Discovery", "description": "d"}],
                "audit_steps": [],
            },
            "workflow": {
                "workflow_id": "custom",
                "description": "A workflow.",
                "initial": "discovery",
                "version": 2,
                "actions": [
                    {"action_name": "discovery", "next": ["specify"], "description": "Discover."},
                    {"action_name": "specify", "next": [], "description": "", "terminal": True},
                ],
            },
        },
    ]


def _project_step(step: dict[str, Any]) -> dict[str, Any]:
    keys = (
        "id",
        "title",
        "description",
        "prompt",
        "prompt_template",
        "expected_output",
        "requires_inputs",
        "depends_on",
        "raci",
        "raci_override_reason",
        "agent_profile",
        "contract_ref",
    )
    return {key: step.get(key) for key in keys}


def _run_compose(scenario: dict[str, Any]) -> dict[str, Any]:
    from runtime.next._internal_runtime.schema import MissionTemplate

    template = MissionTemplate.model_validate(scenario["template"])
    workflow = WorkflowSequence.model_validate(scenario["workflow"])
    composed = compose_template_with_workflow(template, workflow)
    wire = composed.model_dump(mode="json")
    return {
        "mission": wire["mission"],
        "steps": [_project_step(step) for step in wire["steps"]],
        "audit_steps": wire["audit_steps"],
    }


def _run_workflow_discovery() -> dict[str, Any]:
    workflow = get_workflow("software-dev-default")
    return {
        "available": list_available_workflows(),
        "workflow": _normalize(workflow.model_dump(mode="json")),
        "path_suffix": "/".join(
            str(resolve_workflow_path("software-dev-default")).split("/")[-2:]
        ),
    }


def _run_plan(scenario: dict[str, Any]) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="runtime-golden-") as tmp:
        run_dir = Path(tmp)
        template = scenario["template"]
        frozen = run_dir / "mission_template_frozen.yaml"
        frozen.write_text(yaml.safe_dump(template, sort_keys=True), encoding="utf-8")
        raw_snapshot = dict(scenario["snapshot"])
        if scenario.get("drift"):
            # Point the snapshot at the real frozen file so the upstream engine
            # hashes it and sees a hash different from the captured one.
            raw_snapshot["template_path"] = str(frozen)
        snapshot = MissionRunSnapshot.model_validate(raw_snapshot)
        (run_dir / "state.json").write_text(
            json.dumps(snapshot.model_dump(mode="json")), encoding="utf-8"
        )
        run_ref = engine.MissionRunRef(
            run_id=snapshot.run_id, run_dir=str(run_dir), mission_key=snapshot.mission_key
        )
        plan = engine.plan_advance(
            run_ref,
            agent_id=scenario["agent_id"],
            result=scenario.get("result", "success"),
            expected_issued_step=scenario.get("expected_issued_step"),
        )
        snapshot_wire = plan.snapshot.model_dump(mode="json")
        if scenario.get("drift"):
            # The upstream engine only sees drift through the real frozen file;
            # the captured state keeps the scenario's stable path so both sides
            # compare like for like.
            snapshot_wire["template_path"] = scenario["snapshot"]["template_path"]
        return {
            "decision": _normalize(plan.decision.model_dump(mode="json")),
            "snapshot": _normalize(snapshot_wire),
            "significance": _normalize(plan.significance.model_dump(mode="json"))
            if plan.significance is not None
            else None,
        }


def _run_answer(scenario: dict[str, Any]) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="runtime-golden-") as tmp:
        run_dir = Path(tmp)
        snapshot = MissionRunSnapshot.model_validate(scenario["snapshot"])
        (run_dir / "state.json").write_text(
            json.dumps(snapshot.model_dump(mode="json")), encoding="utf-8"
        )
        run_ref = engine.MissionRunRef(
            run_id=snapshot.run_id, run_dir=str(run_dir), mission_key=snapshot.mission_key
        )
        actor = ActorIdentity.model_validate(scenario["actor"])
        error: str | None = None
        try:
            engine.provide_decision_answer(
                run_ref, scenario["decision_id"], scenario["answer"], actor
            )
        except MissionRuntimeError as exc:  # noqa: PERF203 - fixture generator
            error = str(exc)
        state = json.loads((run_dir / "state.json").read_text(encoding="utf-8"))
        events_path = run_dir / "run.events.jsonl"
        events = []
        if events_path.exists():
            events = [
                json.loads(line)
                for line in events_path.read_text(encoding="utf-8").splitlines()
                if line.strip()
            ]
        return {
            "error": _normalize(error),
            "state": _normalize(state),
            "events": [
                {"event_type": event["event_type"], "payload": _normalize(event["payload"])}
                for event in events
            ],
        }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "golden",
        help="Output directory for golden JSON files.",
    )
    args = parser.parse_args(argv)

    args.out.mkdir(parents=True, exist_ok=True)
    for scenario in _plan_scenarios():
        scenario["snapshot"] = _snapshot_from(scenario)
        scenario["current_template_hash"] = (
            "live-hash" if scenario["name"] == "plan_template_drift" else "frozen-hash"
        )
        golden = {"scenario": scenario, "expected": _run_plan(scenario)}
        _write(args.out, scenario["name"], golden)
    for scenario in _answer_scenarios():
        golden = {"scenario": scenario, "expected": _run_answer(scenario)}
        _write(args.out, scenario["name"], golden)
    for scenario in _compose_scenarios():
        golden = {"scenario": scenario, "expected": _run_compose(scenario)}
        _write(args.out, scenario["name"], golden)
    _write(
        args.out,
        "workflow_shipped_default",
        {
            "scenario": {"name": "workflow_shipped_default", "kind": "workflow_discovery"},
            "expected": _run_workflow_discovery(),
        },
    )
    return 0


def _snapshot_from(scenario: dict[str, Any]) -> dict[str, Any]:
    return MissionRunSnapshot.model_validate(scenario["snapshot"]).model_dump(mode="json")


def _write(out: Path, name: str, golden: dict[str, Any]) -> None:
    path = out / f"{name}.json"
    path.write_text(json.dumps(golden, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    sys.exit(main())
