"""Fixtures must be reproducible at any audit location.

The first evidence bundle could not be re-derived: runtime snapshots embedded an
absolute ``template_path``, so the same recipe + seed at a different root changed
9 of 13 files. The portable (root-canonicalised) content digest fixes that, and
these tests prove it for every recipe.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from spkbench.fixtures import FixtureBuilder, canonical_bytes, content_manifest, verify_manifest
from spkbench.mutation import clone_fixture, tree_manifest

SEED = 20261102
RECIPES = ("project-small", "compat-project", "replay-project", "runtime-large")


@pytest.mark.parametrize("recipe", RECIPES)
def test_recipe_is_portable_across_roots(recipe: str, tmp_path: Path) -> None:
    short = FixtureBuilder(tmp_path / "a" / recipe, seed=SEED).get(recipe)
    long = FixtureBuilder(
        tmp_path / ("a-much-longer-audit-root-name-" + "x" * 40) / recipe, seed=SEED
    ).get(recipe)
    short_record = short.as_dict(include_manifest=False)
    long_record = long.as_dict(include_manifest=False)
    assert short_record["file_count"] == long_record["file_count"]
    assert short_record["content_digest"] == long_record["content_digest"]
    assert short_record["portable"] is True
    # The raw per-file digests may differ (absolute paths); the portable ones may not.
    assert short.manifest == long.manifest or short_record["content_digest"] == long_record[
        "content_digest"
    ]
    assert verify_manifest(short)["unchanged"]
    assert verify_manifest(long)["unchanged"]


def test_runtime_snapshots_embed_absolute_paths_but_hash_portably(tmp_path: Path) -> None:
    first = FixtureBuilder(tmp_path / "one" / "runtime-large", seed=SEED).get("runtime-large")
    second = FixtureBuilder(tmp_path / "two" / "runtime-large", seed=SEED).get("runtime-large")

    def snapshots(fixture) -> dict[str, str]:
        out = {}
        for path in sorted((fixture.path / "snapshots").glob("*.json")):
            payload = json.loads(path.read_text())
            out[path.name] = payload["template_path"]
        return out

    one, two = snapshots(first), snapshots(second)
    assert one != two, "fixture must still exercise absolute template paths"
    assert all(str(first.path) in value for value in one.values())
    # ... yet the recorded digest is identical at both roots.
    assert (
        first.as_dict(include_manifest=False)["content_digest"]
        == second.as_dict(include_manifest=False)["content_digest"]
    )
    canonical = canonical_bytes(
        (first.path / "snapshots" / "step.json").read_bytes(), first.path
    )
    assert str(first.path).encode() not in canonical
    assert b"<FIXTURE>" in canonical


def test_clone_manifest_binds_to_the_pristine_fixture_manifest(tmp_path: Path) -> None:
    """A clone embeds the original fixture root; the shared normalisation binds it."""
    fixture = FixtureBuilder(tmp_path / "source" / "runtime-large", seed=SEED).get(
        "runtime-large"
    )
    clone = clone_fixture(fixture.path, tmp_path / "clone")
    raw = tree_manifest(clone)
    portable = tree_manifest(clone, roots=(fixture.path,))
    assert raw != portable, "the runtime fixture must still embed location-dependent bytes"
    assert raw != fixture.expected_content()
    assert portable == fixture.expected_content()


def test_content_manifest_detects_drift(tmp_path: Path) -> None:
    fixture = FixtureBuilder(tmp_path, seed=SEED).get("compat-project")
    expected = fixture.as_dict(include_manifest=False)["content_digest"]
    target = fixture.path / "kitty-specs" / "002-bench-mission-002" / "status.events.jsonl"
    target.write_text(target.read_text() + "\n")
    report = verify_manifest(fixture)
    assert report["unchanged"] is False
    assert report["changed"] == ["kitty-specs/002-bench-mission-002/status.events.jsonl"]
    assert content_manifest(fixture.path)
    assert report["expected_manifest_sha256"] == expected


def test_fixture_build_does_not_touch_the_checkout() -> None:
    repo = Path(__file__).resolve().parents[3]
    before = sorted(path.as_posix() for path in repo.iterdir())
    import tempfile

    work = Path(tempfile.mkdtemp(prefix="spkbench-fixture-check-"))
    try:
        FixtureBuilder(work, seed=SEED).get("compat-project")
    finally:
        import shutil

        shutil.rmtree(work, ignore_errors=True)
    assert sorted(path.as_posix() for path in repo.iterdir()) == before
