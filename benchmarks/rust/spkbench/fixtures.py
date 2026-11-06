"""Deterministic benchmark fixtures.

Fixtures are generated from recipes, never copied from a developer working tree
and never mutated by a workload. Every generated tree has a stable manifest
(relative path -> sha256) recorded in the report so a reader can rebuild the
exact input from the recipe + seed.

Recipes are intentionally small enough to generate on a shared VM in a couple
of seconds, but large enough that the measured work dominates interpreter
startup for the *workload* class (startup is measured separately).
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
# 2026-11-02T00:00:00Z
BASE_MS = 1793491200000

SCOPES = ("mission_local", "team_domain", "audience_domain", "spec_kitty_core")
LANES = ("genesis", "planned", "claimed", "in_progress", "for_review", "in_review", "approved", "done", "blocked", "canceled")
TRANSITION_LANES = (
    ("planned", "claimed"),
    ("claimed", "in_progress"),
    ("in_progress", "for_review"),
    ("for_review", "in_review"),
    ("in_review", "approved"),
    ("approved", "done"),
    ("in_progress", "blocked"),
    ("blocked", "in_progress"),
)


def _encode_crockford(value: int, length: int) -> str:
    chars = []
    for _ in range(length):
        chars.append(CROCKFORD[value & 0x1F])
        value >>= 5
    return "".join(reversed(chars))


def ulid(seed: str, index: int, *, ms_offset: int | None = None) -> str:
    """Deterministic, valid 26-character ULID."""
    ms = BASE_MS + (index * 1000 if ms_offset is None else ms_offset)
    entropy = int.from_bytes(hashlib.sha256(f"{seed}:{index}".encode()).digest()[:10], "big")
    return _encode_crockford(ms, 10) + _encode_crockford(entropy, 16)


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def canonical_bytes(
    data: bytes, root: Path | None = None, *, roots: tuple[Path, ...] = ()
) -> bytes:
    """Replace every known location root with one stable token.

    Some fixture content legitimately embeds the path it was generated at (a
    runtime snapshot records ``template_path``). Hashing those bytes directly
    makes the manifest location-dependent: the same recipe + seed at a
    different root produces a different digest, so an auditor cannot rebuild
    the input the evidence claims to measure.

    A measured mutation copies the fixture to a **clone** and runs there, so the
    bytes can still legitimately embed the *original* fixture root. Callers that
    compare a measured tree with a regenerated fixture must therefore normalise
    every known root (fixture and clone) with this one function. Replacing only
    the location-dependent parts keeps the digest content-addressed without
    altering what the workloads read from disk.
    """
    candidates: list[Path] = []
    if root is not None:
        candidates.append(Path(root))
    candidates.extend(Path(item) for item in roots)
    substitutions: list[bytes] = []
    for candidate in candidates:
        for text in (str(candidate.resolve()), str(candidate)):
            needle = text.encode("utf-8")
            if needle and needle not in substitutions:
                substitutions.append(needle)
    # Longest first so a nested root is not partially replaced by its parent.
    for needle in sorted(substitutions, key=len, reverse=True):
        if needle in data:
            data = data.replace(needle, b"<FIXTURE>")
    return data


def is_ignored(rel: str, prefixes: tuple[str, ...] = ()) -> bool:
    """Whether ``rel`` is one of the declared implementation-private paths."""
    return any(rel == prefix.rstrip("/") or rel.startswith(prefix) for prefix in prefixes)


def content_manifest(
    root: Path, *, roots: tuple[Path, ...] = (), ignored_prefixes: tuple[str, ...] = ()
) -> dict[str, str]:
    """Portable per-file digest: sha256 of the root-canonicalised bytes.

    This is the single manifest definition used for the pristine fixture and
    for a measured mutation clone. ``roots`` adds location roots that may be
    embedded in the bytes (the fixture root of a clone); ``ignored_prefixes``
    excludes only declared implementation-private artifacts, never a
    user-observable directory.
    """
    manifest: dict[str, str] = {}
    for path in sorted(Path(root).rglob("*")):
        rel = path.relative_to(root).as_posix()
        if is_ignored(rel, ignored_prefixes):
            continue
        if path.is_symlink():
            target = canonical_bytes(
                os.readlink(path).encode("utf-8"), root, roots=roots
            ).decode("utf-8", "replace")
            manifest[rel] = "symlink:" + target
        elif path.is_file():
            manifest[rel] = hashlib.sha256(
                canonical_bytes(path.read_bytes(), root, roots=roots)
            ).hexdigest()
    return manifest


@dataclass
class Fixture:
    name: str
    path: Path
    recipe: dict
    manifest: dict[str, str] = field(default_factory=dict)
    content: dict[str, str] = field(default_factory=dict)
    notes: list[str] = field(default_factory=list)

    def expected_content(self) -> dict[str, str]:
        """Portable manifest sealed at build time.

        Falls back to recomputing when a caller constructed a ``Fixture`` by
        hand (unit tests), never when evidence is verified: a sealed fixture
        keeps the build-time expectation so later drift is detectable.
        """
        return dict(self.content) if self.content else content_manifest(self.path)

    def as_dict(self, *, include_manifest: bool = True) -> dict:
        portable = self.expected_content()
        record = {
            "name": self.name,
            "path": str(self.path),
            "recipe": self.recipe,
            "file_count": len(portable),
            "content_digest": _manifest_digest(portable),
            "portable": True,
            "path_independent": True,
            "notes": self.notes,
        }
        if include_manifest:
            record["manifest"] = portable
            record["manifest_sha256"] = record["content_digest"]
        return record


def verify_manifest(fixture: Fixture) -> dict:
    """Recompute the portable fixture manifest and report any drift.

    Read-only workloads must not modify their inputs. A drift here means a
    command wrote into a fixture (or another process did), so the measurement is
    not a read-only equivalence run. The comparison uses the same
    root-canonicalised digests as :func:`content_manifest`, so it is valid at an
    arbitrary audit location.
    """
    current = content_manifest(fixture.path)
    expected = fixture.expected_content()
    added = sorted(set(current) - set(expected))
    removed = sorted(set(expected) - set(current))
    changed = sorted(rel for rel in set(current) & set(expected) if current[rel] != expected[rel])
    return {
        "unchanged": not (added or removed or changed),
        "added": added[:20],
        "removed": removed[:20],
        "changed": changed[:20],
        "current_manifest_sha256": _manifest_digest(current),
        "expected_manifest_sha256": _manifest_digest(expected),
    }


def _manifest_digest(manifest: dict[str, str]) -> str:
    h = hashlib.sha256()
    for rel in sorted(manifest):
        h.update(rel.encode())
        h.update(b"\0")
        h.update(manifest[rel].encode())
        h.update(b"\n")
    return h.hexdigest()


class FixtureBuilder:
    def __init__(self, root: Path, seed: int, *, dry_run: bool = False):
        self.root = root
        self.seed = seed
        self.dry_run = dry_run
        self._cache: dict[str, Fixture] = {}
        if not dry_run:
            root.mkdir(parents=True, exist_ok=True)

    def get(self, recipe_name: str) -> Fixture:
        if recipe_name not in self._cache:
            builder = RECIPES.get(recipe_name)
            if builder is None:
                raise KeyError(f"unknown fixture recipe {recipe_name!r}")
            if self.dry_run:
                # Metadata-only listing: never allocate or write a fixture tree.
                self._cache[recipe_name] = Fixture(
                    name=recipe_name,
                    path=self.root / recipe_name,
                    recipe={"kind": recipe_name, "seed": self.seed},
                )
            else:
                fixture = builder(self, self.root / recipe_name)
                # Seal the portable (root-canonicalised) manifest at build time.
                # Evidence records this digest and verification compares against
                # it, so a fixture that drifts later is detectable.
                fixture.content = content_manifest(fixture.path)
                self._cache[recipe_name] = fixture
        return self._cache[recipe_name]


# ---------------------------------------------------------------------------
# Writers
# ---------------------------------------------------------------------------


def _write(fixture: Fixture, path: Path, text: str, *, binary: bool = False) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if binary:
        path.write_bytes(text.encode("utf-8") if isinstance(text, str) else text)
    else:
        path.write_text(text, encoding="utf-8")
    rel = path.relative_to(fixture.path).as_posix()
    fixture.manifest[rel] = _sha256_text(text)


def _yaml_scalar(value: str) -> str:
    if any(c in value for c in ":#{}[]&*!|>'\"%@`") or value.strip() != value:
        return json.dumps(value)
    return value


def _meta(slug: str, mission_id: str, number: int, mission_type: str = "software-dev") -> str:
    return json.dumps(
        {
            "created_at": "2026-11-02T00:00:00Z",
            "flattened": False,
            "friendly_name": f"Bench Mission {number:04d}",
            "mission_id": mission_id,
            "mission_number": number,
            "mission_slug": slug,
            "mission_type": mission_type,
            "slug": slug,
            "status_phase": "1",
            "target_branch": "main",
            "topology": "single_branch",
        },
        indent=2,
        sort_keys=True,
    ) + "\n"


def _wp_text(wp_id: str, title: str, deps: list[str], mission_id: str) -> str:
    dep_list = "[" + ", ".join(deps) + "]"
    return (
        "---\n"
        f"work_package_id: {wp_id}\n"
        f"title: {_yaml_scalar(title)}\n"
        f"dependencies: {dep_list}\n"
        "subtasks:\n- T001\n- T002\n"
        "phase: Bench\n"
        "execution_mode: code_change\n"
        f"mission_id: {mission_id}\n"
        "---\n\n"
        f"# Work Package {wp_id}: {title}\n\n"
        "Deterministic benchmark fixture body. No source mutation is performed.\n"
    )


def _record_tree(fixture: Fixture, root: Path) -> None:
    """Record every file below ``root`` in the fixture manifest.

    Used for generated metadata that is not written through :func:`_write`
    (currently only the deterministic ``.git`` skeleton of ``compat-project``).
    """
    for path in sorted(root.rglob("*")):
        if path.is_file():
            rel = path.relative_to(fixture.path).as_posix()
            fixture.manifest[rel] = hashlib.sha256(path.read_bytes()).hexdigest()


def _git_init(fixture: Fixture) -> str:
    """Create an unborn ``main`` branch in the fixture with an isolated config.

    Upstream's ``agent tasks`` reads insist on a branch (a detached HEAD is a
    hard refusal), while the native CLI does not. An unborn branch is enough:
    no commit, index or object directory is written, so every ``.git`` byte is
    reproducible (unlike ``git add``/``git commit``, whose index records inode
    and timestamp stat data). The git version is recorded because the sample
    hooks shipped by ``git init`` are installation-defined.
    """
    env = {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "HOME": str(fixture.path / ".git-home"),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0",
        "LC_ALL": "C.UTF-8",
        "LANG": "C.UTF-8",
        "TZ": "UTC",
    }
    (fixture.path / ".git-home").mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["git", "init", "-q", "-b", "main", "."],
        cwd=fixture.path,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=True,
    )
    version = subprocess.run(
        ["git", "--version"],
        cwd=fixture.path,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=True,
    ).stdout.decode("utf-8", "replace").strip()
    # The temporary HOME is only a config sandbox; it is not fixture input.
    shutil.rmtree(fixture.path / ".git-home", ignore_errors=True)
    _record_tree(fixture, fixture.path / ".git")
    return version


def _seed_yaml(terms: list[dict]) -> str:
    lines = ["# Spec Kitty glossary seed (benchmark fixture)", "terms:"]
    for term in terms:
        lines.append(f"  - surface: {_yaml_scalar(term['surface'])}")
        lines.append(f"    definition: {_yaml_scalar(term['definition'])}")
        lines.append(f"    confidence: {term.get('confidence', 1.0)}")
        lines.append(f"    status: {term.get('status', 'active')}")
    lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Recipes
# ---------------------------------------------------------------------------


def _project(builder: FixtureBuilder, path: Path, *, missions: int, wps: int, terms_per_scope: int, events: int) -> Fixture:
    fixture = Fixture(
        name=path.name,
        path=path,
        recipe={
            "kind": "project",
            "seed": builder.seed,
            "missions": missions,
            "work_packages_per_mission": wps,
            "terms_per_scope": terms_per_scope,
            "events": events,
        },
    )
    path.mkdir(parents=True, exist_ok=True)
    (path / ".kittify").mkdir(parents=True, exist_ok=True)
    _write(fixture, path / ".kittify" / "config.yaml", "version: '1.0'\ncharter:\n  enabled: false\n")
    specs = path / "kitty-specs"
    specs.mkdir(parents=True, exist_ok=True)

    for m in range(missions):
        number = m + 1
        slug = f"{number:03d}-bench-mission-{number:03d}"
        mission_id = ulid("bench-mission", m)
        mission_dir = specs / slug
        mission_dir.mkdir(parents=True, exist_ok=True)
        _write(fixture, mission_dir / "meta.json", _meta(slug, mission_id, number))
        _write(fixture, mission_dir / "spec.md", f"# Bench Mission {number:03d}\n\nDeterministic fixture.\n")
        tasks = mission_dir / "tasks"
        tasks.mkdir(parents=True, exist_ok=True)
        for w in range(wps):
            wp_id = f"WP{w + 1:02d}"
            if w == 0:
                deps: list[str] = []
            elif w == 1:
                deps = ["WP01"]
            elif w == 2:
                deps = ["WP01"]
            else:
                deps = [f"WP{w - 1:02d}", f"WP{w - 2:02d}"]
            _write(
                fixture,
                tasks / f"{wp_id}-bench-{number:03d}-{w + 1:02d}.md",
                _wp_text(wp_id, f"Bench WP {w + 1:02d} of mission {number:03d}", deps, mission_id),
            )

    # Glossary: unique active terms per scope + deliberate cross-scope collisions.
    gloss_dir = path / ".kittify" / "glossaries"
    per_scope = max(1, terms_per_scope)
    for s_index, scope in enumerate(SCOPES):
        terms = [
            {
                "surface": f"bench-{scope}-term-{i:05d}",
                "definition": f"Benchmark {scope} term {i:05d} definition text for deterministic loading.",
                "confidence": 0.9,
                "status": "active",
            }
            for i in range(per_scope)
        ]
        if scope == "mission_local":
            terms.append(
                {
                    "surface": "bench-resolved",
                    "definition": "A term with exactly one active sense, in mission_local.",
                    "confidence": 1.0,
                    "status": "active",
                }
            )
            terms.append(
                {
                    "surface": "bench-noactive",
                    "definition": "Draft-only term used to exercise the no-active-sense path.",
                    "confidence": 0.4,
                    "status": "draft",
                }
            )
            for i in range(3):
                terms.append(
                    {
                        "surface": "bench-ambiguous",
                        "definition": f"Ambiguity candidate {i} in mission_local.",
                        "confidence": 1.0,
                        "status": "active",
                    }
                )
        elif scope == "team_domain":
            terms.append(
                {
                    "surface": "bench-ambiguous",
                    "definition": "Ambiguity candidate in team_domain.",
                    "confidence": 1.0,
                    "status": "active",
                }
            )
        _write(fixture, gloss_dir / f"{scope}.yaml", _seed_yaml(terms))
    _ = s_index

    # Canonical status event log for one mission, mixed typed families.
    events_dir = specs / "001-bench-mission-001"
    lines: list[str] = []
    for i in range(events):
        if i % 10 == 9:
            actor = {"role": "implementer", "profile": None, "tool": "codex", "model": None}
            lines.append(
                json.dumps(
                    {
                        "event_id": ulid("bench-event", i),
                        "kind": "annotation",
                        "wp_id": f"WP{(i % max(wps, 1)) + 1:02d}",
                        "at": "2026-11-02T00:00:00Z",
                        "actor": actor,
                        "delta": {"note": f"bench annotation {i}"},
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                )
            )
        elif i % 10 == 8:
            lines.append(
                json.dumps(
                    {
                        "event_type": "DecisionPointOpened" if i % 20 == 8 else "BenchFutureExtension",
                        "payload": {"id": f"D{i}"},
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                )
            )
        else:
            from_lane, to_lane = TRANSITION_LANES[i % len(TRANSITION_LANES)]
            lines.append(
                json.dumps(
                    {
                        "event_id": ulid("bench-event", i),
                        "mission_slug": "001-bench-mission-001",
                        "mission_id": ulid("bench-mission", 0),
                        "wp_id": f"WP{(i % max(wps, 1)) + 1:02d}",
                        "from_lane": from_lane,
                        "to_lane": to_lane,
                        "at": "2026-11-02T00:00:00Z",
                        "actor": f"agent-{i % 5}",
                        "force": False,
                        "execution_mode": "worktree",
                        "reason": None,
                        "review_ref": None,
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                )
            )
    _write(fixture, events_dir / "status.events.jsonl", "\n".join(lines) + "\n")
    return fixture


def _runtime_template(path: Path, *, steps: int, audits: str) -> str:
    lines = [
        "mission:",
        "  key: bench-mission",
        "  name: Bench Runtime Mission",
        "  version: '1.0.0'",
        "  description: Deterministic planner fixture",
        "steps:",
    ]
    for i in range(steps):
        deps = []
        if i >= 1:
            deps.append(f"step-{i - 1:04d}")
        if i >= 10 and i % 7 == 0:
            deps.append(f"step-{i - 10:04d}")
        if i == 42:
            # Deliberate long edge used by the unschedulable scenario: the
            # dependency is skipped as the issued step, so step-0042 can never
            # become eligible while other work remains.
            deps.append("step-0030")
        dep_text = "[" + ", ".join(deps) + "]"
        requires = '["topic"]' if i == 100 else "[]"
        body = (
            f"Deterministic benchmark step {i:04d}. "
            "This description is intentionally long enough that template parsing and "
            "schema validation are measurable work rather than constant overhead. "
            "It contains no timestamps, no paths, and no random values. "
        ) * 3
        lines.extend(
            [
                f"  - id: step-{i:04d}",
                f"    title: Bench Step {i:04d}",
                f"    description: {json.dumps(body)}",
                f"    prompt: {json.dumps('Execute bench step ' + format(i, '04d') + '.')}",
                f"    expected_output: {json.dumps('bench-output-' + format(i, '04d'))}",
                f"    requires_inputs: {requires}",
                f"    depends_on: {dep_text}",
            ]
        )
    if audits != "none":
        lines.extend(
            [
                "audit_steps:",
                "  - id: audit-0001",
                "    title: Bench Audit Checkpoint",
                "    description: Deterministic audit checkpoint.",
                "    audit:",
                "      trigger_mode: post_merge",
                f"      enforcement: {audits}",
                f"    depends_on: [step-{steps - 1:04d}]",
            ]
        )
    return "\n".join(lines) + "\n"


def _runtime_snapshot(*, template_path: Path, template_hash: str, completed: list[str], blocked: str | None = None, pending: dict | None = None, issued: str | None = None, inputs: dict | None = None) -> str:
    return json.dumps(
        {
            "run_id": "bench-run-0001",
            "mission_key": "bench-mission",
            "template_path": str(template_path),
            "template_hash": template_hash,
            "policy_snapshot": {
                "strictness": "medium",
                "default_route": "same_llm_context",
                "extras": {},
            },
            "completed_steps": completed,
            "issued_step_id": issued,
            "inputs": inputs or {},
            "decisions": {},
            "pending_decisions": pending or {},
            "blocked_reason": blocked,
            "mission_id": ulid("bench-mission", 0),
            "mission_slug": "001-bench-mission-001",
        },
        indent=2,
        sort_keys=True,
    ) + "\n"


def _runtime(builder: FixtureBuilder, path: Path) -> Fixture:
    fixture = Fixture(
        name=path.name,
        path=path,
        recipe={"kind": "runtime", "seed": builder.seed, "steps": 900, "scenarios": 8},
    )
    path.mkdir(parents=True, exist_ok=True)
    steps = 900
    base_yaml = _runtime_template(path, steps=steps, audits="none")
    blocking_yaml = _runtime_template(path, steps=steps, audits="blocking")
    advisory_yaml = _runtime_template(path, steps=steps, audits="advisory")
    _write(fixture, path / "mission.yaml", base_yaml)
    _write(fixture, path / "mission_blocking_audit.yaml", blocking_yaml)
    _write(fixture, path / "mission_advisory_audit.yaml", advisory_yaml)
    # A live template with one changed byte for drift detection.
    drifted = base_yaml.replace("Bench Runtime Mission", "Bench Runtime Mission (edited)", 1)
    _write(fixture, path / "mission_live_drifted.yaml", drifted)

    base_hash = _sha256_text(base_yaml)
    blocking_hash = _sha256_text(blocking_yaml)
    advisory_hash = _sha256_text(advisory_yaml)
    drift_hash = _sha256_text(drifted)

    def write(name: str, text: str) -> None:
        _write(fixture, path / "snapshots" / name, text)

    write(
        "step.json",
        _runtime_snapshot(
            template_path=path / "mission.yaml",
            template_hash=base_hash,
            completed=[f"step-{i:04d}" for i in range(20)],
        ),
    )
    write(
        "blocked.json",
        _runtime_snapshot(
            template_path=path / "mission.yaml",
            template_hash=base_hash,
            completed=[],
            blocked="Awaiting operator decision",
        ),
    )
    write(
        "terminal.json",
        _runtime_snapshot(
            template_path=path / "mission.yaml",
            template_hash=base_hash,
            completed=[f"step-{i:04d}" for i in range(steps)],
        ),
    )
    write(
        "pending-decision.json",
        _runtime_snapshot(
            template_path=path / "mission.yaml",
            template_hash=base_hash,
            completed=[f"step-{i:04d}" for i in range(5)],
            pending={
                "input:topic": {
                    "decision_id": "input:topic",
                    "step_id": "step-0100",
                    "question": "Provide the benchmark topic.",
                    "options": [],
                    "requested_by": {
                        "actor_id": "bench-agent",
                        "actor_type": "llm",
                        "provider": "bench",
                        "model": "bench-model",
                        "tool": "bench-tool",
                    },
                    "requested_at": "2026-11-02T00:00:00Z",
                }
            },
        ),
    )
    write(
        "missing-input.json",
        _runtime_snapshot(
            template_path=path / "mission.yaml",
            template_hash=base_hash,
            completed=[f"step-{i:04d}" for i in range(100)],
        ),
    )
    write(
        "audit-blocking.json",
        _runtime_snapshot(
            template_path=path / "mission_blocking_audit.yaml",
            template_hash=blocking_hash,
            completed=[f"step-{i:04d}" for i in range(steps)],
        ),
    )
    write(
        "audit-advisory.json",
        _runtime_snapshot(
            template_path=path / "mission_advisory_audit.yaml",
            template_hash=advisory_hash,
            completed=[f"step-{i:04d}" for i in range(steps)],
        ),
    )
    write(
        "unschedulable.json",
        _runtime_snapshot(
            template_path=path / "mission.yaml",
            template_hash=base_hash,
            completed=[f"step-{i:04d}" for i in range(steps) if i not in (30, 42)],
            issued="step-0030",
        ),
    )
    write(
        "drift.json",
        _runtime_snapshot(
            template_path=path / "mission.yaml",
            template_hash=base_hash,
            completed=[f"step-{i:04d}" for i in range(20)],
        ),
    )
    fixture.notes.append(f"drift live hash: {drift_hash}")
    return fixture


def _compat_project(
    builder: FixtureBuilder,
    path: Path,
    *,
    missions: int = 12,
    wps: int = 6,
    terms_per_scope: int = 40,
    events: int = 240,
    coherent_wps: int = 6,
    blocked_cycles: int = 0,
) -> Fixture:
    """Fixture for the native compatibility routes.

    Adds two properties the older projection fixtures do not have:

    * an unborn ``main`` git branch, because upstream's ``agent tasks`` reads
      refuse a detached HEAD (the native CLI does not); and
    * a *coherent* per-WP lane history for mission 002, so canonical status
      materialization is compared over a plausible event stream rather than
      only the deliberately chaotic stream used by the projection fixtures.

    Mission 001 keeps the mixed stream (annotations, future extensions and
    unordered transitions) so reducer parity is still exercised on odd input.

    ``coherent_wps`` / ``blocked_cycles`` scale the coherent mission's replay
    work without changing the recipe's shape: each WP walks the normal lane
    chain and may cycle ``in_progress -> blocked -> in_progress`` a number of
    times. This is what makes the canonical-replay workload work-dominated
    instead of launch-dominated; the event count is recorded in the recipe so
    expectations can assert it exactly.
    """
    fixture = _project(
        builder,
        path,
        missions=missions,
        wps=wps,
        terms_per_scope=terms_per_scope,
        events=events,
    )
    fixture.recipe.update(
        {
            "kind": "compat-project",
            "missions": missions,
            "work_packages_per_mission": wps,
            "terms_per_scope": terms_per_scope,
            "events": events,
            "coherent_work_packages": coherent_wps,
            "blocked_cycles_per_work_package": blocked_cycles,
            "git": "unborn-main",
        }
    )
    specs = path / "kitty-specs"
    coherent_mission = "002-bench-mission-002"
    mission_id = ulid("bench-mission", 1)
    chain = ["planned", "claimed", "in_progress", "for_review", "in_review", "approved", "done"]
    lines: list[str] = []
    index = 0
    for wp in range(coherent_wps):
        wp_id = f"WP{wp + 1:03d}"
        pairs: list[tuple[str, str]] = []
        for step in range(len(chain) - 1):
            pairs.append((chain[step], chain[step + 1]))
            if chain[step + 1] == "in_progress":
                for _ in range(blocked_cycles):
                    pairs.append(("in_progress", "blocked"))
                    pairs.append(("blocked", "in_progress"))
        for source, target in pairs:
            lines.append(
                json.dumps(
                    {
                        "event_id": ulid("bench-compat-event", index),
                        "mission_slug": coherent_mission,
                        "mission_id": mission_id,
                        "wp_id": wp_id,
                        "from_lane": source,
                        "to_lane": target,
                        "at": f"2026-11-02T00:{index // 60:02d}:{index % 60:02d}Z",
                        "actor": f"agent-{wp}",
                        "force": False,
                        "execution_mode": "worktree",
                        "reason": None,
                        "review_ref": None,
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                )
            )
            index += 1
    fixture.recipe["coherent_events"] = index
    _write(fixture, specs / coherent_mission / "status.events.jsonl", "\n".join(lines) + "\n")

    fixture.recipe["git_version"] = _git_init(fixture)
    fixture.notes.append(
        "compat-project adds an unborn main branch (git reads) and a coherent "
        f"mission-002 event log ({index} events, {coherent_wps} WPs) for canonical "
        "status materialization."
    )
    return fixture


RECIPES = {
    "project-small": lambda b, p: _project(b, p, missions=4, wps=4, terms_per_scope=40, events=200),
    "project-large": lambda b, p: _project(b, p, missions=800, wps=4, terms_per_scope=1500, events=20000),
    "compat-project": _compat_project,
    # Canonical replay workload: same compatibility surface, but the coherent
    # mission holds 200 WPs x (6 lane transitions + 4 blocked cycles) = 2,800
    # events so reducer work dominates process launch on both sides.
    "replay-project": lambda b, p: _compat_project(
        b, p, missions=2, wps=6, terms_per_scope=40, events=240, coherent_wps=200, blocked_cycles=4
    ),
    "runtime-large": _runtime,
}
