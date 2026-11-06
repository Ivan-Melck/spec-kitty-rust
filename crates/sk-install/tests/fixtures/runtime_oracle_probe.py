"""Source-first oracle: record `ensure_runtime()` over a mixed pre-existing home.

Run under the pinned interpreter with an asserted temporary CWD/HOME/all XDG/TMP
and `SPEC_KITTY_PACKS_ROOT=<repo>/packs`.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
assert str(root).startswith("/tmp/sk-c4-oracle."), root
for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[key]).parent == root, (key, os.environ[key])
assert Path.cwd() == root / "work", Path.cwd()

from specify_cli.runtime.bootstrap import ensure_runtime  # noqa: E402
from specify_cli.runtime.home import get_kittify_home, get_package_asset_root  # noqa: E402


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def tree(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    if not path.exists():
        return out
    for child in sorted(path.rglob("*")):
        relative = child.relative_to(path).as_posix()
        if child.is_symlink():
            out[relative] = "link:" + os.readlink(child)
        elif child.is_dir():
            out[relative + "/"] = ""
        else:
            out[relative] = "sha256:" + hashlib.sha256(child.read_bytes()).hexdigest()
    return out


home = get_kittify_home()
outside = root / "outside-scripts"
write(outside / "keep.txt", "user content\n")
write(home / "missions" / "software-dev" / "stale.txt", "stale\n")
write(home / "missions" / "custom" / "mine" / "x.md", "mine\n")
write(home / "config.yaml", "user: config\n")
write(home / "cache" / "version.lock", "0.0.1")
write(home / "AGENTS.md", "old agents\n")
(home / "scripts").symlink_to(outside)

before = tree(home)
agents_before = (home / "AGENTS.md").read_text(encoding="utf-8")
ensure_runtime()
after = tree(home)

pack = get_package_asset_root()
results = {
    "home_rel": home.relative_to(root).as_posix(),
    "package_assets_rel": pack.relative_to(root).as_posix() if str(pack).startswith(str(root)) else str(pack),
    "managed_source_dirs": sorted(
        entry.name for entry in (pack.parent / "missions").iterdir() if entry.is_dir()
    ),
    "package_has_scripts": (pack.parent / "scripts").exists(),
    "package_has_agents_md": (pack.parent / "AGENTS.md").exists(),
    "before": before,
    "after": after,
    "version_lock": (home / "cache" / "version.lock").read_text(encoding="utf-8"),
    "outside_unchanged": tree(outside),
    "agents_before": agents_before,
    "agents_after": (home / "AGENTS.md").read_text(encoding="utf-8"),
    "assets_manifest": (home / "cache" / "runtime_bootstrap-assets.json").read_text(encoding="utf-8")
    if (home / "cache" / "runtime_bootstrap-assets.json").is_file()
    else None,
    "update_lock_bytes": (home / "cache" / ".update.lock").read_bytes().hex()
    if (home / "cache" / ".update.lock").is_file()
    else None,
    "warm_tree_stable": (ensure_runtime(), tree(home))[1] == after,
    "top_level": sorted(entry.name for entry in home.iterdir()),
}

print(json.dumps(results, indent=2, sort_keys=True))
