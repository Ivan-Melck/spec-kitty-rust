"""The runner's mutation path: clone-per-invocation, post-state, timing window.

The old executor ran every invocation in one shared fixture directory and had no
runner hook at all, so a mutating workload could not be timed honestly. These
tests exercise the hook directly with a tiny script workload: each invocation gets
a fresh clone, clone and post-state work stay outside the timed window, and a
post-state that violates its declared schema is reported.
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path

from spkbench import runner
from spkbench.execute import Executor
from spkbench.workloads import CommandSpec, MutationSpec, Workload, compare_canonical


def _workload(script: str, *, expectations: dict | None = None) -> Workload:
    argv = [sys.executable, "-c", script, "{FIXTURE}"]
    return Workload(
        id="fake.mutation",
        description="runner mutation self-test",
        fixture="fake",
        klass="work",
        equivalence="strict",
        required=False,
        rust=CommandSpec("py-adapter", argv),
        oracle=CommandSpec("py-adapter", list(argv)),
        compare=compare_canonical,
        normalize=lambda value: value,
        expected_exit=0,
        mutation=MutationSpec(ignored_post_state=("locks/",)),
        expectations=expectations
        or {
            "exit": 0,
            "payload": {"required_keys": ["ok"]},
            "post_state": {
                "required_paths": ["out.txt"],
                "expected_additions": ["out.txt"],
                "expected_changes": [],
                "expected_removals": [],
                "ignored_prefixes": ["locks/"],
            },
        },
        source="diagnostic",
    )


def _fixture(tmp_path: Path) -> Path:
    root = tmp_path / "pristine"
    (root / "locks").mkdir(parents=True)
    (root / "base.txt").write_text("base")
    return root


def test_each_invocation_gets_a_fresh_clone(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    script = (
        "import json,pathlib,sys\n"
        "root = pathlib.Path(sys.argv[1])\n"
        "assert not (root / 'out.txt').exists(), 'clone was reused'\n"
        "(root / 'out.txt').write_text('result')\n"
        "print(json.dumps({'ok': True}))\n"
    )
    workload = _workload(script)
    executor = Executor(run_dir=tmp_path / "run", cwd=tmp_path)
    first = runner._invoke(
        executor=executor, workload=workload, side="rust",
        fixture_path=root, run_dir=tmp_path / "run", counter=1, warmup=False,
    )
    second = runner._invoke(
        executor=executor, workload=workload, side="rust",
        fixture_path=root, run_dir=tmp_path / "run", counter=2, warmup=False,
    )
    assert first.observation.exit_code == second.observation.exit_code == 0
    assert first.clone != second.clone
    assert first.post_state_sha == second.post_state_sha
    assert first.problems == [] and second.problems == []
    assert not Path(first.clone).exists(), "clones are removed after the invocation"
    assert (root / "base.txt").read_text() == "base"
    assert not (root / "out.txt").exists()


def test_clone_and_post_state_scan_are_outside_the_timed_window(tmp_path: Path, monkeypatch) -> None:
    root = _fixture(tmp_path)
    script = "import json,sys; print(json.dumps({'ok': True}))\n"
    workload = _workload(script)
    real_clone = runner.clone_fixture

    def slow_clone(source: Path, destination: Path) -> Path:
        time.sleep(0.35)
        return real_clone(source, destination)

    monkeypatch.setattr(runner, "clone_fixture", slow_clone)
    executor = Executor(run_dir=tmp_path / "run", cwd=tmp_path)
    invocation = runner._invoke(
        executor=executor, workload=workload, side="rust",
        fixture_path=root, run_dir=tmp_path / "run", counter=1, warmup=False,
    )
    assert invocation.observation.duration_s < 0.30, (
        "clone cost must not be credited to the measured command "
        f"(duration={invocation.observation.duration_s:.3f}s)"
    )


def test_undeclared_post_state_writes_are_reported(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    script = (
        "import json,pathlib,sys\n"
        "root = pathlib.Path(sys.argv[1])\n"
        "(root / 'out.txt').write_text('result')\n"
        "(root / 'bonus.txt').write_text('unexpected')\n"
        "print(json.dumps({'ok': True}))\n"
    )
    workload = _workload(script)
    executor = Executor(run_dir=tmp_path / "run", cwd=tmp_path)
    invocation = runner._invoke(
        executor=executor, workload=workload, side="rust",
        fixture_path=root, run_dir=tmp_path / "run", counter=1, warmup=False,
    )
    assert any("undeclared post-state" in problem for problem in invocation.post_state_problems)


def test_degenerate_payload_fails_the_declared_schema(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    script = "import json,sys; print(json.dumps({}))\n"
    workload = _workload(script)
    workload.expectations = {"exit": 0, "payload": {"required_keys": ["ok"]}}
    executor = Executor(run_dir=tmp_path / "run", cwd=tmp_path)
    invocation = runner._invoke(
        executor=executor, workload=workload, side="rust",
        fixture_path=root, run_dir=tmp_path / "run", counter=1, warmup=False,
    )
    assert invocation.problems
    assert "missing required key" in invocation.problems[0]
