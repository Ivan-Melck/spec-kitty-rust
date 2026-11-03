"""Mutation executor support for paired mutating workloads.

The read-only executor (``spkbench.execute``) runs one argv in one fixture. A
mutating workflow needs three extra guarantees before its output can be called
equivalent:

1. every invocation starts from a **fresh clone** of the pristine fixture, so a
   write from invocation *n* can never feed invocation *n+1*;
2. the comparison includes the **post-state**, not only stdout; and
3. the pristine fixture is unchanged afterwards.

This module implements those guarantees as an additive layer. It does not touch
the frozen measurement core: ``spkbench.runner`` has no hook for it yet, so
registered mutation workloads stay ``equivalence="pending"`` and are proven with
``benchmarks/rust/tools/mutation_check.py``. The exact runner patch needed to
time them is in ``benchmarks/rust/ADAPTER-REPORT.md``.

Declared exclusions are deliberately narrow. ``MutationSpec.ignored_post_state``
holds only implementation-private lock artifacts; those are still captured and
reported separately, so a difference is visible instead of normalised away.
"""

from __future__ import annotations

import hashlib
import json
import shutil
from dataclasses import dataclass, field
from pathlib import Path

from .execute import Executor, Observation
from .fixtures import content_manifest, is_ignored
from .workloads import (
    MutationSpec,
    Workload,
    canonical,
    oracle_payload,
    rust_payload,
    sha_payload,
)


def _is_ignored(rel: str, prefixes: tuple[str, ...]) -> bool:
    return is_ignored(rel, prefixes)


def tree_manifest(
    root: Path,
    *,
    ignored_prefixes: tuple[str, ...] = (),
    roots: tuple[Path, ...] = (),
) -> dict[str, str]:
    """Content manifest for every file under ``root``.

    Digest values use the *same* root canonicalisation as the pristine fixture
    manifest (:func:`spkbench.fixtures.content_manifest`). A mutation runs in a
    clone, whose bytes may legitimately embed the original fixture root; pass
    that root in ``roots`` so the recorded manifest is comparable with the
    independently regenerated fixture instead of being silently
    location-dependent.

    Symlinks are recorded by target so a link swap is not mistaken for an
    unchanged file. Ignored prefixes are only used for the equality manifest;
    ``ignored_artifacts`` captures the same paths for disclosure.
    """
    return content_manifest(
        root, roots=roots, ignored_prefixes=ignored_prefixes
    )


def ignored_artifacts(root: Path, prefixes: tuple[str, ...]) -> dict[str, str]:
    """Capture the declared internal artifacts that are excluded from equality."""
    if not prefixes:
        return {}
    full = tree_manifest(root)
    return {rel: digest for rel, digest in full.items() if _is_ignored(rel, prefixes)}


def diff_manifests(before: dict[str, str], after: dict[str, str]) -> dict:
    added = sorted(set(after) - set(before))
    removed = sorted(set(before) - set(after))
    changed = sorted(rel for rel in set(before) & set(after) if before[rel] != after[rel])
    return {
        "added": added,
        "removed": removed,
        "changed": changed,
        "added_sha256": {rel: after[rel] for rel in added},
        "removed_sha256": {rel: before[rel] for rel in removed},
        "changed_sha256": {rel: {"before": before[rel], "after": after[rel]} for rel in changed},
    }


def clone_fixture(source: Path, destination: Path) -> Path:
    """Copy a pristine fixture to a new directory (never reusing an old clone)."""
    if destination.exists():
        raise FileExistsError(f"clone destination already exists: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source, destination, symlinks=True)
    return destination


@dataclass
class MutationObservation:
    side: str
    invocation: int
    exit_code: int | None
    duration_s: float
    payload_sha: str
    payload: object
    delta: dict
    ignored: dict
    clone: str
    timed_out: bool = False

    def as_dict(self) -> dict:
        return {
            "side": self.side,
            "invocation": self.invocation,
            "exit_code": self.exit_code,
            "duration_s": self.duration_s,
            "timed_out": self.timed_out,
            "payload_sha256": self.payload_sha,
            "delta": self.delta,
            "ignored_artifacts": self.ignored,
            "clone": self.clone,
        }


@dataclass
class MutationResult:
    workload_id: str
    equivalent: bool
    post_state_equal: bool
    master_unchanged: bool
    invocations: int
    expected_exit: int | None
    problems: list[str] = field(default_factory=list)
    equivalence_diff: str = ""
    observations: list[MutationObservation] = field(default_factory=list)
    mutation: dict = field(default_factory=dict)

    def as_dict(self) -> dict:
        return {
            "schema": "spkbench.mutation.v1",
            "workload_id": self.workload_id,
            "equivalent": self.equivalent,
            "post_state_equal": self.post_state_equal,
            "master_unchanged": self.master_unchanged,
            "invocations": self.invocations,
            "expected_exit": self.expected_exit,
            "problems": self.problems,
            "equivalence_diff": self.equivalence_diff,
            "mutation": self.mutation,
            "observations": [item.as_dict() for item in self.observations],
        }


def _payload_for(observation: Observation) -> object:
    if observation.protocol == "rust":
        return rust_payload(observation.stdout)
    return oracle_payload(observation.stdout)


def _run_side(
    *,
    executor: Executor,
    workload: Workload,
    side: str,
    invocation: int,
    fixture: Path,
    work_root: Path,
    before: dict[str, str],
    ignored_prefixes: tuple[str, ...],
    roots: tuple[Path, ...],
) -> MutationObservation:
    spec = workload.rust if side == "rust" else workload.oracle
    if spec is None:
        raise ValueError(f"{workload.id}: missing {side} command spec")
    mutation = workload.mutation or MutationSpec()
    clone = clone_fixture(fixture, work_root / f"clone-{side}-{invocation}")
    argv = mutation.rewrite(spec.argv, clone)
    observation = executor.run(
        protocol=spec.protocol,
        argv=argv,
        timeout_s=spec.timeout_s,
        cwd=clone,
    )
    payload = _payload_for(observation)
    if workload.normalize is not None:
        payload = workload.normalize(payload)
    after = tree_manifest(clone, ignored_prefixes=ignored_prefixes, roots=roots)
    return MutationObservation(
        side=side,
        invocation=invocation,
        exit_code=observation.exit_code,
        duration_s=observation.duration_s,
        payload_sha=sha_payload(payload),
        payload=payload,
        delta=diff_manifests(before, after),
        ignored=ignored_artifacts(clone, ignored_prefixes),
        clone=str(clone),
        timed_out=observation.timed_out,
    )


def run_mutation_pair(
    *,
    workload: Workload,
    fixture: Path,
    executor: Executor,
    invocations: int = 2,
    keep_clones: bool = False,
) -> MutationResult:
    """Run both sides ``invocations`` times, each in a fresh fixture clone.

    Invocation 0 is the preflight. Later invocations must reproduce the same
    normalized payload and the same post-state delta; a mismatch is reported as
    nondeterminism rather than averaged away.
    """
    if workload.mutation is None:
        raise ValueError(f"{workload.id}: not a mutation workload")
    if workload.rust is None or workload.oracle is None:
        raise ValueError(f"{workload.id}: mutation workload needs both command specs")
    if workload.compare is None:
        raise ValueError(f"{workload.id}: mutation workload needs a comparator")
    if invocations < 1:
        raise ValueError("invocations must be >= 1")

    prefixes = workload.mutation.ignored_post_state
    pristine = tree_manifest(fixture)
    before = tree_manifest(fixture, ignored_prefixes=prefixes)
    work_root = executor.run_dir / "mutations" / workload.id.replace("/", "-")
    # A rerun into the same run directory must not reuse an old clone.
    shutil.rmtree(work_root, ignore_errors=True)
    work_root.mkdir(parents=True, exist_ok=True)
    result = MutationResult(
        workload_id=workload.id,
        equivalent=False,
        post_state_equal=False,
        master_unchanged=False,
        invocations=invocations,
        expected_exit=workload.expected_exit,
        mutation={
            "ignored_post_state": list(prefixes),
            "fixture": str(fixture),
            "pristine_manifest_sha256": hashlib.sha256(canonical(pristine).encode()).hexdigest(),
            "pristine_files": len(pristine),
        },
    )

    preflight: dict[str, MutationObservation] = {}
    for side in ("rust", "oracle"):
        for invocation in range(invocations):
            observation = _run_side(
                executor=executor,
                workload=workload,
                side=side,
                invocation=invocation,
                fixture=fixture,
                work_root=work_root,
                before=before,
                ignored_prefixes=prefixes,
                roots=(fixture,),
            )
            result.observations.append(observation)
            if observation.timed_out:
                result.problems.append(f"{side} invocation {invocation} timed out")
                continue
            if invocation == 0:
                preflight[side] = observation
                continue
            expected = preflight[side]
            if observation.exit_code != expected.exit_code:
                result.problems.append(
                    f"{side} invocation {invocation} exit {observation.exit_code} "
                    f"!= preflight {expected.exit_code}"
                )
            if observation.payload_sha != expected.payload_sha:
                result.problems.append(
                    f"{side} invocation {invocation} normalized payload changed "
                    f"({observation.payload_sha[:12]} != {expected.payload_sha[:12]})"
                )
            if observation.delta != expected.delta:
                result.problems.append(
                    f"{side} invocation {invocation} post-state delta changed"
                )

    if "rust" not in preflight or "oracle" not in preflight:
        result.problems.append("preflight did not complete for both sides")
        result.master_unchanged = tree_manifest(fixture) == pristine
        if not keep_clones:
            shutil.rmtree(work_root, ignore_errors=True)
        return result

    rust = preflight["rust"]
    oracle = preflight["oracle"]
    if workload.expected_exit is not None:
        for item in (rust, oracle):
            if item.exit_code != workload.expected_exit:
                result.problems.append(
                    f"{item.side} exit {item.exit_code}, expected {workload.expected_exit}"
                )
    elif rust.exit_code != oracle.exit_code:
        result.problems.append(
            f"exit codes differ: rust={rust.exit_code} oracle={oracle.exit_code}"
        )

    equivalent, diff = workload.compare(rust.payload, oracle.payload)
    result.equivalence_diff = diff
    if not equivalent:
        result.problems.append("normalized result payloads differ")

    result.post_state_equal = rust.delta == oracle.delta
    if not result.post_state_equal:
        result.problems.append(
            "post-state deltas differ (see observations[].delta; ignored lock "
            "artifacts are reported separately)"
        )

    result.master_unchanged = tree_manifest(fixture) == pristine
    if not result.master_unchanged:
        result.problems.append("the pristine fixture changed during the mutation run")

    result.equivalent = (
        equivalent
        and result.post_state_equal
        and result.master_unchanged
        and not result.problems
    )
    if not keep_clones:
        shutil.rmtree(work_root, ignore_errors=True)
    return result


def mutation_summary(workload: Workload) -> dict:
    """Static description used by ``tools/mutation_check.py --list``."""
    mutation = workload.mutation
    return {
        "id": workload.id,
        "fixture": workload.fixture,
        "equivalence": workload.equivalence,
        "required": workload.required,
        "timing_ready": bool(mutation and mutation.timing_ready),
        "ignored_post_state": list(mutation.ignored_post_state) if mutation else [],
        "pending_reason": workload.pending_reason,
    }


def dump_json(value: object) -> str:
    return json.dumps(value, indent=2, sort_keys=True)
