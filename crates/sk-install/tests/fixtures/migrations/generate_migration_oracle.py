#!/usr/bin/env python3
"""Generate the before/after/ledger oracle for the native `.gitignore` migrations.

Run it with the lock-exact development oracle, from the repository root:

    PYTHONPATH="$PWD/src" SPEC_KITTY_PACKS_ROOT="$PWD/packs" \
        .venv-rust-oracle/bin/python \
        crates/sk-install/tests/fixtures/migrations/generate_migration_oracle.py

It writes two fixtures next to itself:

* ``gitignore_oracle.json`` — for each case, the project files before, the
  pinned migration's ``detect``/``can_apply``/``apply`` verdicts, the project
  files after, the git index contents and the post-apply ``detect``.
* ``ledger_oracle.json`` — the exact ledger line the pinned ``build_record``
  produces for each declared id and result, at a fixed timestamp, so the native
  wire shape (identity, ``content_hash``, ``clone_local``, ``superseded_by``
  and JSON spelling) is compared rather than asserted from memory.

Every project, home, XDG directory and temp root is a fresh temporary
directory. Nothing here reads or writes the checkout's own runtime state, and
nothing here is a production path: this script is development tooling whose
output is a committed fixture.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

BASELINE = "9aed3a8012cb5eb22c0128b14d525039f95fea7a"
FIXED_AT = datetime(2026, 11, 2, 0, 0, 0, tzinfo=timezone.utc)
FIXED_CLI_VERSION = "4.0.0rc8"

#: ids this fixture covers, with the cases each one runs.
IGNORE_FAMILY = {
    "0.2.0_specify_to_kittify": [
        "no-specify",
        "specify-only",
        "specify-and-specs",
        "kittify-exists",
        "both-spec-dirs",
    ],
    "0.8.0_remove_active_mission": [
        "absent",
        "retired-file",
        "dangling-symlink",
    ],
    "0.4.8_gitignore_agents": [
        "no-file",
        "existing-operator-entry",
        "all-present",
        "crlf-partial",
        "substring-present",
    ],
    "2.0.9_state_gitignore": [
        "no-file",
        "existing-operator-entry",
        "all-present",
        "crlf-partial",
    ],
    "3.2.0rc35_kittify_runtime_git_hygiene": [
        "no-repo-missing",
        "no-repo-clean",
        "repo-tracked-runtime",
        "repo-clean",
    ],
    "3.2.3_encoding_provenance_gitignore_backfill": [
        "no-repo-missing",
        "repo-tracked-provenance",
    ],
    "3.2.4_derived_mission_views_gitignore_backfill": [
        "no-file",
        "slashless-present",
        "all-present",
    ],
    "3.2.4_runtime_dirs_gitignore_backfill": [
        "no-file",
        "slashless-present",
        "one-missing",
    ],
    "3.2.6rc3_lint_report_gitignore_backfill": [
        "no-file",
        "present",
        "operator-entry-only",
    ],
    "3.2.6rc3_worktrees_gitignore_backfill": [
        "no-repo-missing",
        "no-repo-present",
        "repo-missing",
        "repo-negation",
        "repo-tracked-under-root",
    ],
    "0.12.1_remove_kitty_specs_from_gitignore": [
        "no-file",
        "blocking",
        "subpath-only",
        "mixed-crlf",
        "symlink",
    ],
    "2.0.0a5_remove_wp_status_gitignore_rule": [
        "no-file",
        "rules-and-heading",
        "subpath-clean",
        "mixed-crlf",
        "symlink",
    ],
}

RETIRED_FAMILY = [
    "0.9.0_frontmatter_only_lanes",
    "0.10.9_repair_templates",
    "0.10.12_charter_cleanup",
    "2.0.0_historical_status_migration",
    "3.2.5_agents_skills_gitignore_backfill",
    "normalize_activation_absence",
    "unify_charter_activation_promote_answers",
]

#: Cases whose `can_apply` refuses for a project shape a real run never applies.
#: Their `apply()` mutates in a way the runner never reaches, so the fixture pins
#: `detect`/`can_apply` only (and leaves the tree untouched).
SKIP_APPLY = {
    ("0.2.0_specify_to_kittify", "kittify-exists"),
    ("0.2.0_specify_to_kittify", "both-spec-dirs"),
}

MARKER = "# Added by Spec Kitty CLI (auto-managed)"

RUNTIME_STATE = [
    ".kittify/runtime/",
    ".kittify/merge-state.json",
    ".kittify/events/",
    ".kittify/dossiers/",
    "kitty-specs/**/.kittify/dossiers/",
]
HYGIENE = [
    ".kittify/sync-state.json",
    ".kittify/encoding-provenance/",
    "kitty-ops/ops-index.jsonl",
]
HYGIENE_TRACKED = [
    ".kittify/charter/context-state.json",
    ".kittify/encoding-provenance/global.jsonl",
    "kitty-ops/ops-index.jsonl",
]
AGENTS = [
    ".claude/",
    ".codex/",
    ".opencode/",
    ".windsurf/",
    ".gemini/",
    ".cursor/",
    ".qwen/",
    ".kilocode/",
    ".augment/",
    ".roo/",
    ".amazonq/",
    ".github/copilot/",
]


def managed_block(entries: list[str], newline: str = "\n") -> str:
    return newline.join([MARKER, *entries, ""])


def project_files(migration_id: str, case: str) -> tuple[dict[str, str], list[str]]:
    """The project tree and the paths to stage in git for one case."""
    if migration_id == "0.2.0_specify_to_kittify":
        return {
            "no-specify": ({}, []),
            "specify-only": ({".specify/memory/MEMORY.md": "m\n"}, []),
            "specify-and-specs": (
                {
                    ".specify/memory/MEMORY.md": "m\n",
                    "specs/one/spec.md": "# spec\n",
                },
                [],
            ),
            "kittify-exists": (
                {".specify/one.md": "one\n", ".kittify/config.yaml": "agents: {}\n"},
                [],
            ),
            "both-spec-dirs": (
                {
                    ".specify/one.md": "one\n",
                    "specs/a/spec.md": "a\n",
                    "kitty-specs/b/spec.md": "b\n",
                },
                [],
            ),
        }[case]

    if migration_id == "0.8.0_remove_active_mission":
        return {
            "absent": ({".kittify/config.yaml": "agents: {}\n"}, []),
            "retired-file": (
                {
                    ".kittify/config.yaml": "agents: {}\n",
                    ".kittify/active-mission": "software-dev\n",
                },
                [],
            ),
            "dangling-symlink": (
                {
                    ".kittify/config.yaml": "agents: {}\n",
                    ".kittify/active-mission": "\u0000symlink:gone",
                },
                [],
            ),
        }[case]
    if migration_id == "0.4.8_gitignore_agents":
        present = "\n".join([*AGENTS, ""])
        return {
            "no-file": ({}, []),
            "existing-operator-entry": (
                {".gitignore": "node_modules/\n# operator note\n"},
                [],
            ),
            "all-present": (
                {
                    ".gitignore": (
                        "# operator note\n\n# AI Agent directories (added by Spec Kitty CLI)\n"
                        "# These contain auth tokens and should NEVER be committed\n" + present
                    )
                },
                [],
            ),
            "crlf-partial": (
                {".gitignore": "node_modules/\r\n.claude/\r\n.codex/\r\n"},
                [],
            ),
            "substring-present": (
                {".gitignore": ".claude/settings.json\n"},
                [],
            ),
        }[case]

    if migration_id == "0.12.1_remove_kitty_specs_from_gitignore":
        return {
            "no-file": ({}, []),
            "blocking": (
                {
                    ".gitignore": (
                        "node_modules/\n# specs are tracked\nkitty-specs/\n"
                        "kitty-specs/**/tasks/*.md\n/.kitty-specs/\n"
                    )
                },
                [],
            ),
            "subpath-only": (
                {".gitignore": "kitty-specs/**/tasks/*.md\nkitty-specs/*/tasks/*.md\n"},
                [],
            ),
            "mixed-crlf": (
                {".gitignore": "node_modules/\r\nkitty-specs\r\n/tasks/\r\n"},
                [],
            ),
            "symlink": (
                {
                    ".gitignore": "\u0000symlink:target.txt",
                    "target.txt": "node_modules/\nkitty-specs/\n",
                },
                [],
            ),
        }[case]

    if migration_id == "2.0.0a5_remove_wp_status_gitignore_rule":
        return {
            "no-file": ({}, []),
            "rules-and-heading": (
                {
                    ".gitignore": (
                        "# Block WP status files during merges\n"
                        "kitty-specs/**/tasks/*.md\n"
                        "kitty-specs/*/tasks/*.md\n"
                        "keep/\n\n\n\n"
                    )
                },
                [],
            ),
            "subpath-clean": ({".gitignore": "kitty-specs/**/tasks/notes.md\n"}, []),
            "mixed-crlf": (
                {".gitignore": "keep/\r\nkitty-specs/**/tasks/*.md\r\n"},
                [],
            ),
            "symlink": (
                {
                    ".gitignore": "\u0000symlink:target.txt",
                    "target.txt": "kitty-specs/**/tasks/*.md\n",
                },
                [],
            ),
        }[case]

    entries = {
        "2.0.9_state_gitignore": RUNTIME_STATE,
        "3.2.4_derived_mission_views_gitignore_backfill": [".kittify/derived/"],
        "3.2.4_runtime_dirs_gitignore_backfill": [".kittify/migrations/", ".kittify/logs/"],
        "3.2.6rc3_lint_report_gitignore_backfill": [".kittify/lint-report.json"],
        "3.2.6rc3_worktrees_gitignore_backfill": [".worktrees/"],
        "3.2.0rc35_kittify_runtime_git_hygiene": HYGIENE,
        "3.2.3_encoding_provenance_gitignore_backfill": HYGIENE,
    }[migration_id]

    if case in {"no-file", "no-repo-missing", "repo-missing"}:
        return {}, []
    if case in {"existing-operator-entry", "operator-entry-only"}:
        return {".gitignore": "node_modules/\n# operator note\n"}, []
    if case == "one-missing":
        return {".gitignore": f"node_modules/\n\n{MARKER}\n.kittify/migrations/\n"}, []
    if case in {"all-present", "present", "no-repo-present", "no-repo-clean", "repo-clean"}:
        return {".gitignore": "node_modules/\n\n" + managed_block(entries)}, []
    if case == "slashless-present":
        slashless = [entry.rstrip("/") for entry in entries]
        return {".gitignore": "node_modules/\n\n" + managed_block(slashless)}, []
    if case == "crlf-partial":
        return {".gitignore": f"node_modules/\r\n{entries[0]}\r\n"}, []
    if case == "repo-negation":
        return {".gitignore": ".worktrees/\n!.worktrees/keep/\n"}, []
    if case == "repo-tracked-runtime":
        return {
            ".gitignore": "node_modules/\n",
            **{path: "state\n" for path in HYGIENE_TRACKED},
        }, list(HYGIENE_TRACKED)
    if case == "repo-tracked-provenance":
        return {
            ".gitignore": "node_modules/\n\n" + managed_block(HYGIENE),
            ".kittify/encoding-provenance/global.jsonl": "{}\n",
        }, [".kittify/encoding-provenance/global.jsonl"]
    if case == "repo-tracked-under-root":
        return {
            ".gitignore": "node_modules/\n",
            ".worktrees/lane-01/README.md": "tracked\n",
        }, [".worktrees/lane-01/README.md"]

    raise AssertionError(f"no fixture for {migration_id}/{case}")


def snapshot(root: Path) -> dict:
    files: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if relative.parts and relative.parts[0] == ".git":
            continue
        if path.is_symlink():
            files[relative.as_posix()] = "\u0000symlink:" + os.readlink(path)
            continue
        if not path.is_file():
            continue
        files[relative.as_posix()] = path.read_bytes().decode("utf-8")
    tracked: list[str] = []
    if (root / ".git").exists():
        output = subprocess.run(
            ["git", "ls-files", "--full-name"],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
            env=tool_env(root),
        )
        tracked = [line for line in output.stdout.splitlines() if line]
    return {"files": files, "tracked": tracked, "git": (root / ".git").is_dir()}


def tool_env(root: Path) -> dict[str, str]:
    environment = dict(os.environ)
    environment.update(
        {
            "HOME": str(root / ".home"),
            "USERPROFILE": str(root / ".home"),
            "XDG_CONFIG_HOME": str(root / ".config"),
            "XDG_CACHE_HOME": str(root / ".cache"),
            "XDG_STATE_HOME": str(root / ".state"),
            "TMPDIR": str(root / ".tmp"),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Fixture",
            "GIT_AUTHOR_EMAIL": "fixture@example.com",
            "GIT_COMMITTER_NAME": "Fixture",
            "GIT_COMMITTER_EMAIL": "fixture@example.com",
            "PYTHONDONTWRITEBYTECODE": "1",
        }
    )
    for name in (".home", ".config", ".cache", ".state", ".tmp"):
        (root / name).mkdir(parents=True, exist_ok=True)
    return environment


def materialize(root: Path, files: dict[str, str], tracked: list[str], want_git: bool) -> None:
    root.mkdir(parents=True, exist_ok=True)
    for relative, content in files.items():
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        if content.startswith("\u0000symlink:"):
            os.symlink(content.split("\u0000symlink:", 1)[1], target)
            continue
        target.write_bytes(content.encode("utf-8"))
    if want_git:
        subprocess.run(["git", "init", "-q", "-b", "main"], cwd=root, check=True, env=tool_env(root))
        if tracked:
            subprocess.run(["git", "add", "-f", "--", *tracked], cwd=root, check=True, env=tool_env(root))


def normalize_paths(value: dict, root: Path) -> dict:
    """Replace this run's temporary project path so the fixture is reproducible."""
    prefix = str(root)
    for key in ("changes", "warnings", "errors"):
        value[key] = [item.replace(prefix, "<project>") for item in value[key]]
    return value


def verdict(callable_) -> dict:
    try:
        return {"value": callable_()}
    except Exception as error:  # noqa: BLE001 - the exception IS the oracle answer
        return {"exception": f"{type(error).__name__}: {error}"}


def apply_case(migration, root: Path) -> dict:
    result = verdict(lambda: migration.apply(root, dry_run=False))
    if "exception" in result:
        return {
            "success": False,
            "changes": [],
            "warnings": [],
            "errors": [result["exception"]],
            "exception": result["exception"],
        }
    value = result["value"]
    return {
        "success": bool(value.success),
        "changes": list(value.changes_made),
        "warnings": list(value.warnings),
        "errors": list(value.errors),
        "exception": None,
    }


def generate_ignore_cases(workspace: Path) -> list[dict]:
    from specify_cli.upgrade.migrations import auto_discover_migrations
    from specify_cli.upgrade.registry import MigrationRegistry

    auto_discover_migrations()
    cases: list[dict] = []
    for migration_id, case_names in IGNORE_FAMILY.items():
        for case in case_names:
            files, tracked = project_files(migration_id, case)
            root = workspace / f"{migration_id}-{case}".replace("/", "_")
            materialize(root, files, tracked, case.startswith("repo-"))
            migration = MigrationRegistry.get_by_id(migration_id)
            assert migration is not None, migration_id
            before = snapshot(root)
            detect = verdict(lambda: bool(migration.detect(root)))
            can_apply = verdict(lambda: list(migration.can_apply(root)))
            if (migration_id, case) in SKIP_APPLY:
                cases.append(
                    {
                        "id": migration_id,
                        "case": case,
                        "before": before,
                        "detect": detect,
                        "can_apply": can_apply,
                        "apply": None,
                        "after": before,
                        "detect_after": detect,
                        "reason_comparable": True,
                    }
                )
                continue
            applied = normalize_paths(apply_case(migration, root), root)
            after = snapshot(root)
            detect_after = verdict(lambda: bool(migration.detect(root)))
            cases.append(
                {
                    "id": migration_id,
                    "case": case,
                    "before": before,
                    "detect": detect,
                    "can_apply": can_apply,
                    "apply": applied,
                    "after": after,
                    "detect_after": detect_after,
                    "reason_comparable": case != "symlink",
                }
            )
    return cases


def generate_retired_cases(workspace: Path) -> list[dict]:
    from specify_cli.upgrade.migrations import auto_discover_migrations
    from specify_cli.upgrade.registry import MigrationRegistry

    auto_discover_migrations()
    cases: list[dict] = []
    for migration_id in RETIRED_FAMILY:
        migration = MigrationRegistry.get_by_id(migration_id)
        assert migration is not None, migration_id
        for case in ("empty-project", "populated-project"):
            root = workspace / f"{migration_id}-{case}".replace("/", "_")
            files = (
                {}
                if case == "empty-project"
                else {
                    ".kittify/config.yaml": "agents:\n  available:\n  - claude\n",
                    ".kittify/metadata.yaml": "spec_kitty:\n  version: 4.0.0rc8\n",
                    ".gitignore": "node_modules/\n",
                    "kitty-specs/one/spec.md": "# spec\n",
                }
            )
            materialize(root, files, [], False)
            before = snapshot(root)
            detect = verdict(lambda: bool(migration.detect(root)))
            can_apply = verdict(lambda: list(migration.can_apply(root)))
            if (migration_id, case) in SKIP_APPLY:
                cases.append(
                    {
                        "id": migration_id,
                        "case": case,
                        "before": before,
                        "detect": detect,
                        "can_apply": can_apply,
                        "apply": None,
                        "after": before,
                        "detect_after": detect,
                        "reason_comparable": True,
                    }
                )
                continue
            applied = normalize_paths(apply_case(migration, root), root)
            after = snapshot(root)
            detect_after = verdict(lambda: bool(migration.detect(root)))
            cases.append(
                {
                    "id": migration_id,
                    "case": case,
                    "before": before,
                    "detect": detect,
                    "can_apply": can_apply,
                    "apply": applied,
                    "after": after,
                    "detect_after": detect_after,
                    "superseded_by": getattr(migration, "superseded_by", ""),
                    "reason_comparable": True,
                }
            )
    return cases


def generate_ledger(workspace: Path) -> dict:
    from specify_cli.upgrade.ledger import build_record
    from specify_cli.upgrade.migrations import auto_discover_migrations
    from specify_cli.upgrade.registry import MigrationRegistry

    del workspace
    auto_discover_migrations()
    lines: dict[str, dict[str, str]] = {}
    for spec in json.loads(
        (
            Path(__file__).resolve().parents[3] / "src/migration_catalog.json"
        ).read_text()
    ):
        migration = MigrationRegistry.get_by_id(spec["id"])
        if migration is None:
            continue
        entry: dict[str, str] = {}
        for result in ("success", "skipped", "failed"):
            record = build_record(
                migration,
                result,
                cli_version=FIXED_CLI_VERSION,
                applied_at=FIXED_AT,
                superseded_by=getattr(migration, "superseded_by", ""),
            )
            entry[result] = json.dumps(record.to_dict())
        lines[spec["id"]] = entry
    return lines


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parent)
    args = parser.parse_args()

    root = Path(tempfile.mkdtemp(prefix="sk-migration-oracle-"))
    try:
        workspace = root / "projects"
        workspace.mkdir()
        environment = tool_env(root)
        for name in ("PYTHONPATH", "SPEC_KITTY_PACKS_ROOT"):
            assert os.environ.get(name), f"{name} must be set for the pinned oracle"
        os.environ.update(environment)
        cases = generate_ignore_cases(workspace) + generate_retired_cases(workspace)
        ledger = generate_ledger(workspace)
    finally:
        shutil.rmtree(root, ignore_errors=True)

    fixture = {
        "baseline": BASELINE,
        "generator": "crates/sk-install/tests/fixtures/migrations/generate_migration_oracle.py",
        "python": sys.version.split()[0],
        "cli_version": FIXED_CLI_VERSION,
        "applied_at": FIXED_AT.isoformat(),
        "cases": cases,
    }
    (args.out / "gitignore_oracle.json").write_text(
        json.dumps(fixture, indent=1, sort_keys=True) + "\n"
    )
    (args.out / "ledger_oracle.json").write_text(
        json.dumps(
            {
                "baseline": BASELINE,
                "cli_version": FIXED_CLI_VERSION,
                "applied_at": FIXED_AT.isoformat(),
                "records": ledger,
            },
            indent=1,
            sort_keys=True,
        )
        + "\n"
    )
    print(f"wrote {len(cases)} cases and {len(ledger)} ledger entries to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
