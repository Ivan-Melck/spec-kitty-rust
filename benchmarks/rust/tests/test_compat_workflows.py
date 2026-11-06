"""Equivalence and registration tests for the native compatibility surface.

Scope: adapters, fixture recipes, workload registration, the frozen registry
binding and the additive mutation executor. No timing campaign is run.

Oracle-dependent tests use the lock-exact interpreter; the autouse session
fixture in ``conftest.py`` fails the suite when it (or a release binary) is
absent instead of skipping.
"""

from __future__ import annotations

import hashlib
import sys
from pathlib import Path

import pytest

from spkbench.execute import Executor
from spkbench.fixtures import FixtureBuilder, verify_manifest
from spkbench.mutation import (
    clone_fixture,
    diff_manifests,
    ignored_artifacts,
    run_mutation_pair,
    tree_manifest,
)
from spkbench.registry import PerformanceRegistry
from spkbench.workloads import (
    CommandSpec,
    MutationSpec,
    Workload,
    build_workloads,
    compare_canonical,
    gate_workloads,
    oracle_payload,
    rust_payload,
)

REPO = Path(__file__).resolve().parents[3]
BENCH = REPO / "benchmarks" / "rust"
ADAPTERS = BENCH / "adapters"
ORACLE = Path(__import__("os").environ.get("SPKBENCH_ORACLE_PYTHON", REPO / ".venv-rust-oracle" / "bin" / "python"))
COMPAT_BINARY = REPO / "target" / "release" / "spec-kitty"
SEED = 20261102


def _workloads(builder: FixtureBuilder, binary: Path = COMPAT_BINARY) -> list[Workload]:
    return build_workloads(
        builder=builder,
        rust_binary=binary,
        oracle_python=ORACLE,
        adapters_dir=ADAPTERS,
        registry=PerformanceRegistry.load(),
        repo_root=REPO,
        include_micro=True,
    )


def _by_id(workloads: list[Workload]) -> dict[str, Workload]:
    return {item.id: item for item in workloads}


def _run_side(workload: Workload, side: str, fixture: Path, run_dir: Path) -> tuple[object, int]:
    spec = workload.rust if side == "rust" else workload.oracle
    assert spec is not None
    executor = Executor(run_dir=run_dir, cwd=fixture)
    observation = executor.run(protocol=spec.protocol, argv=spec.argv, cwd=fixture)
    payload = (
        rust_payload(observation.stdout)
        if spec.protocol == "rust"
        else oracle_payload(observation.stdout)
    )
    if workload.normalize is not None:
        payload = workload.normalize(payload)
    return payload, observation.exit_code


# ---------------------------------------------------------------------------
# Registration / fixture contracts (no oracle required)
# ---------------------------------------------------------------------------


def test_compat_fixture_is_deterministic_and_manifest_clean(tmp_path: Path) -> None:
    first = FixtureBuilder(tmp_path / "a", seed=SEED).get("compat-project")
    second = FixtureBuilder(tmp_path / "b", seed=SEED).get("compat-project")

    assert first.manifest == second.manifest
    assert first.as_dict(include_manifest=False)["content_digest"] == second.as_dict(
        include_manifest=False
    )["content_digest"]
    assert ".git/HEAD" in first.manifest
    assert verify_manifest(first)["unchanged"]
    assert first.recipe["git"] == "unborn-main"
    assert first.recipe["git_version"].startswith("git version ")
    # No index/objects: an unborn branch keeps every .git byte reproducible.
    assert not (first.path / ".git" / "index").exists()
    assert not any((first.path / ".git" / "objects").glob("??"))


def test_replay_fixture_declares_its_exact_event_count(tmp_path: Path) -> None:
    fixture = FixtureBuilder(tmp_path, seed=SEED).get("replay-project")
    events = (fixture.path / "kitty-specs" / "002-bench-mission-002" / "status.events.jsonl").read_text()
    counted = [line for line in events.splitlines() if line.strip()]
    assert len(counted) == fixture.recipe["coherent_events"] == 2800
    assert fixture.recipe["coherent_work_packages"] == 200
    assert fixture.recipe["blocked_cycles_per_work_package"] == 4


def test_registration_uses_the_frozen_ids(tmp_path: Path) -> None:
    registry = PerformanceRegistry.load()
    workloads = _by_id(_workloads(FixtureBuilder(tmp_path, seed=SEED, dry_run=True)))
    frozen = {wid: workloads[wid] for wid in registry.required_ids}
    assert set(frozen) == set(registry.required_ids)
    for wid, workload in frozen.items():
        assert workload.required is True
        assert workload.source == "registry"

    # The first harness's independent ids must not survive as required rows.
    for stale in ("mission.inventory.full", "lifecycle.mutations", "startup.version"):
        if stale in workloads:
            assert workloads[stale].required is False or stale in registry.required_ids

    tasks = workloads["tasks.list"]
    assert tasks.required is False, "non-frozen strict pairs are diagnostics"
    assert tasks.counts_for_gate() is False
    replay = workloads["status.replay"]
    assert replay.mutation is not None
    assert replay.mutation.timing_ready is True
    assert replay.runnable is True
    assert replay.counts_for_gate() is True
    assert replay.fixture == "replay-project"


def test_unimplemented_frozen_workloads_are_absent_not_faked(tmp_path: Path) -> None:
    workloads = _by_id(_workloads(FixtureBuilder(tmp_path, seed=SEED, dry_run=True)))
    for wid in ("state.event-append", "state.move-task", "workflow.consolidate", "workflow.merge"):
        workload = workloads[wid]
        assert workload.measurement == "absent"
        assert workload.runnable is False
        assert workload.pending_reason and len(workload.pending_reason) > 40
        assert workload.rust is None and workload.oracle is None
        assert workload.expectations is None


def test_compat_routes_resolve_sibling_compat_binary(tmp_path: Path) -> None:
    inspection = REPO / "target" / "release" / "spec-kitty-rust"
    workloads = _by_id(_workloads(FixtureBuilder(tmp_path, seed=SEED, dry_run=True), binary=inspection))
    assert Path(workloads["status.replay"].rust.argv[0]).name == "spec-kitty"
    assert Path(workloads["tasks.list"].rust.argv[0]).name == "spec-kitty"


def test_compat_routes_block_when_compat_binary_is_absent(tmp_path: Path) -> None:
    missing = tmp_path / "missing" / "spec-kitty-rust"
    workloads = _by_id(_workloads(FixtureBuilder(tmp_path, seed=SEED, dry_run=True), binary=missing))
    replay = workloads["status.replay"]
    assert replay.runnable is False
    assert replay.runtime_blocker and "compatibility CLI" in replay.runtime_blocker
    for workload_id in ("tasks.list", "tasks.list_dependents.projection"):
        workload = workloads[workload_id]
        assert workload.runnable is False
        reason = workload.runtime_blocker or workload.pending_reason or ""
        assert "compatibility binary" in reason or "compatibility CLI" in reason


def test_gate_population_is_only_measured_frozen_work(tmp_path: Path) -> None:
    workloads = _workloads(FixtureBuilder(tmp_path, seed=SEED, dry_run=True))
    gate_ids = {item.id for item in gate_workloads(workloads)}
    assert gate_ids == {"status.replay"}
    for excluded in (
        "tasks.list",
        "mission.branch_context.projection",
        "transition.check.approved-done",
        "startup.version",
        "cli.mission.list",
    ):
        assert excluded not in gate_ids


def test_rust_payload_accepts_all_documented_envelopes() -> None:
    bare = rust_payload(b'{"count": 0, "tasks": []}')
    assert bare == {"ok": True, "data": {"count": 0, "tasks": []}}
    enveloped = rust_payload(b'{"ok": true, "data": {"count": 0}}')
    assert enveloped == {"ok": True, "data": {"count": 0}}
    versioned = rust_payload(b'{"command": "mission.list", "data": {"count": 1}}')
    assert versioned["ok"] is True and versioned["data"] == {"count": 1}
    refusal = rust_payload(b'{"command": "x", "error": {"message": "boom"}}')
    assert refusal["ok"] is False and refusal["error"] == {"message": "boom"}
    assert "_unparseable" in rust_payload(b"not json")


def test_declared_projection_drops_do_not_hide_unrelated_changes(tmp_path: Path) -> None:
    workload = _by_id(_workloads(FixtureBuilder(tmp_path, seed=SEED, dry_run=True)))[
        "mission.branch_context.projection"
    ]
    assert workload.normalize is not None
    left = {
        "repo_root": str(tmp_path),
        "NOW_UTC_ISO": "2026-11-02T00:00:00Z",
        "spec_kitty_version": "4.0.0rc8",
        "runtime_vars": {"now_utc_iso": "2026-11-02T00:00:00Z", "target_branch": "main"},
        "target_branch": "main",
        "branch_matches_target": True,
    }
    right = dict(left)
    right["target_branch"] = "feat/other"
    assert workload.normalize(left) != workload.normalize(right)


# ---------------------------------------------------------------------------
# Mutation executor unit contracts (no oracle required)
# ---------------------------------------------------------------------------


def _fake_workload(script: str, *, ignored: tuple[str, ...] = ()) -> Workload:
    spec = CommandSpec("py-adapter", [sys.executable, "-c", script, "{FIXTURE}"])
    return Workload(
        id="fake.mutation",
        description="executor self-test",
        fixture="fake",
        klass="pending",
        equivalence="pending",
        required=True,
        rust=spec,
        oracle=CommandSpec("py-adapter", list(spec.argv)),
        compare=compare_canonical,
        expected_exit=0,
        mutation=MutationSpec(ignored_post_state=ignored),
    )


def test_tree_manifest_diff_and_ignored_artifacts(tmp_path: Path) -> None:
    root = tmp_path / "tree"
    (root / "locks").mkdir(parents=True)
    (root / "a.txt").write_text("a")
    (root / "locks" / "x.lock").write_text("lock")
    before = tree_manifest(root, ignored_prefixes=("locks/",))
    assert before == {"a.txt": hashlib.sha256(b"a").hexdigest()}
    assert list(ignored_artifacts(root, ("locks/",))) == ["locks/x.lock"]

    (root / "a.txt").write_text("b")
    (root / "b.txt").write_text("new")
    after = tree_manifest(root, ignored_prefixes=("locks/",))
    delta = diff_manifests(before, after)
    assert delta["added"] == ["b.txt"]
    assert delta["changed"] == ["a.txt"]
    assert delta["removed"] == []


def test_clone_fixture_isolates_writes(tmp_path: Path) -> None:
    master = tmp_path / "master"
    master.mkdir()
    (master / "data.txt").write_text("pristine")
    clone = clone_fixture(master, tmp_path / "clone")
    (clone / "data.txt").write_text("mutated")
    (clone / "new.txt").write_text("new")
    assert (master / "data.txt").read_text() == "pristine"
    assert not (master / "new.txt").exists()
    with pytest.raises(FileExistsError):
        clone_fixture(master, clone)


def test_mutation_executor_uses_fresh_clone_per_invocation(tmp_path: Path) -> None:
    master = tmp_path / "fixture"
    master.mkdir()
    (master / "base.txt").write_text("base")
    script = (
        "import json,pathlib,sys\n"
        "root = pathlib.Path(sys.argv[1])\n"
        "assert not (root / 'marker.txt').exists(), 'clone was reused'\n"
        "(root / 'marker.txt').write_text('written')\n"
        "print(json.dumps({'ok': True}))\n"
    )
    workload = _fake_workload(script)
    result = run_mutation_pair(
        workload=workload,
        fixture=master,
        executor=Executor(run_dir=tmp_path / "run", cwd=tmp_path),
        invocations=2,
        keep_clones=True,
    )
    assert result.equivalent
    assert result.post_state_equal
    assert result.master_unchanged
    assert [item.invocation for item in result.observations] == [0, 1, 0, 1]
    clones = {item.clone for item in result.observations}
    assert len(clones) == 4, "every invocation must receive its own clone"
    for clone in clones:
        assert Path(clone).name.startswith("clone-")


def test_mutation_executor_detects_post_state_divergence(tmp_path: Path) -> None:
    master = tmp_path / "fixture"
    master.mkdir()
    (master / "base.txt").write_text("base")
    rust_script = (
        "import json,pathlib,sys\n"
        "pathlib.Path(sys.argv[1], 'rust.txt').write_text('r')\n"
        "print(json.dumps({'same': True}))\n"
    )
    oracle_script = (
        "import json,pathlib,sys\n"
        "pathlib.Path(sys.argv[1], 'oracle.txt').write_text('o')\n"
        "print(json.dumps({'same': True}))\n"
    )
    workload = _fake_workload(rust_script)
    workload.oracle = CommandSpec("py-adapter", [sys.executable, "-c", oracle_script, "{FIXTURE}"])
    result = run_mutation_pair(
        workload=workload,
        fixture=master,
        executor=Executor(run_dir=tmp_path / "run", cwd=tmp_path),
        invocations=1,
    )
    assert not result.post_state_equal
    assert not result.equivalent
    assert result.master_unchanged
    assert any("post-state" in problem for problem in result.problems)


# ---------------------------------------------------------------------------
# Oracle-backed equivalence checks
# ---------------------------------------------------------------------------


def test_tasks_list_is_strictly_equal(tmp_path: Path) -> None:
    fixture = FixtureBuilder(tmp_path / "fixtures", seed=SEED).get("compat-project")
    workload = _by_id(_workloads(FixtureBuilder(tmp_path / "fixtures", seed=SEED, dry_run=True)))[
        "tasks.list"
    ]
    rust, rust_exit = _run_side(workload, "rust", fixture.path, tmp_path / "run")
    oracle, oracle_exit = _run_side(workload, "oracle", fixture.path, tmp_path / "run")
    assert rust_exit == oracle_exit == 0
    assert compare_canonical(rust, oracle) == (True, "")
    assert rust["count"] == 6
    assert verify_manifest(fixture)["unchanged"]


def test_tasks_list_dependents_projection_and_disclosed_gap(tmp_path: Path) -> None:
    fixture = FixtureBuilder(tmp_path / "fixtures", seed=SEED).get("compat-project")
    workload = _by_id(_workloads(FixtureBuilder(tmp_path / "fixtures", seed=SEED, dry_run=True)))[
        "tasks.list_dependents.projection"
    ]
    rust_raw, rust_exit = _run_side(workload, "rust", fixture.path, tmp_path / "run")
    rust_spec = workload.rust
    oracle_spec = workload.oracle
    assert rust_spec is not None and oracle_spec is not None
    executor = Executor(run_dir=tmp_path / "run", cwd=fixture.path)
    raw_rust = rust_payload(executor.run(protocol="rust", argv=rust_spec.argv, cwd=fixture.path).stdout)
    raw_oracle = oracle_payload(
        executor.run(protocol="py-adapter", argv=oracle_spec.argv, cwd=fixture.path).stdout
    )
    assert raw_rust["data"]["dependents"] == raw_oracle["dependents"] == ["WP04", "WP05"]
    assert rust_exit == 0
    assert raw_oracle["depends_on"] == ["[WP01]"]
    assert raw_rust["data"]["depends_on"] == ["WP01"]
    assert compare_canonical(rust_raw, workload.normalize(raw_oracle)) == (True, "")


def test_branch_context_projection_and_disclosed_gap(tmp_path: Path) -> None:
    fixture = FixtureBuilder(tmp_path / "fixtures", seed=SEED).get("compat-project")
    workload = _by_id(_workloads(FixtureBuilder(tmp_path / "fixtures", seed=SEED, dry_run=True)))[
        "mission.branch_context.projection"
    ]
    rust, rust_exit = _run_side(workload, "rust", fixture.path, tmp_path / "run")
    oracle, oracle_exit = _run_side(workload, "oracle", fixture.path, tmp_path / "run")
    assert rust_exit == oracle_exit == 0
    assert compare_canonical(rust, oracle) == (True, "")
    assert rust["target_branch"] == "main"
    assert rust["branch_matches_target"] is True
    assert "spec_kitty_version" not in rust

    executor = Executor(run_dir=tmp_path / "run", cwd=fixture.path)
    oracle_spec = workload.oracle
    assert oracle_spec is not None
    raw_oracle = oracle_payload(
        executor.run(protocol="py-adapter", argv=oracle_spec.argv, cwd=fixture.path).stdout
    )
    assert raw_oracle.get("spec_kitty_version")
    assert "spec_kitty_version" not in (workload.normalize(raw_oracle) or {})


def test_status_replay_mutation_equivalence(tmp_path: Path) -> None:
    fixture = FixtureBuilder(tmp_path / "fixtures", seed=SEED).get("replay-project")
    workload = _by_id(_workloads(FixtureBuilder(tmp_path / "fixtures", seed=SEED, dry_run=True)))[
        "status.replay"
    ]
    result = run_mutation_pair(
        workload=workload,
        fixture=fixture.path,
        executor=Executor(run_dir=tmp_path / "run", cwd=tmp_path),
        invocations=2,
    )
    assert result.equivalent, result.problems
    assert result.post_state_equal
    assert result.master_unchanged
    assert verify_manifest(fixture)["unchanged"]

    preflight = {item.side: item for item in result.observations if item.invocation == 0}
    status = "kitty-specs/002-bench-mission-002/status.json"
    assert preflight["rust"].delta["added"] == [status]
    assert preflight["oracle"].delta["added"] == [status]
    assert (
        preflight["rust"].delta["added_sha256"][status]
        == preflight["oracle"].delta["added_sha256"][status]
    )
    # Only implementation-private lock artifacts are excluded; the canonical
    # Mission lock namespace itself must agree.
    assert preflight["rust"].ignored
    assert preflight["oracle"].ignored
    assert set(preflight["rust"].ignored) == set(preflight["oracle"].ignored)
    assert all("spec-kitty-locks" in path for path in preflight["rust"].ignored)
    assert all("spec-kitty-locks" in path for path in preflight["oracle"].ignored)
