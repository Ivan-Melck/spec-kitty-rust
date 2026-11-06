"""C6 Source-first: the global runtime owner manifest / `.update.lock` protocol.

Drives the pinned `ensure_runtime()` over an isolated HOME and records the real
observable contract: the owner inventory bytes, the persistent lock node, the
version stamp, managed-tree modes/hashes, and the mixed managed/custom/config/
cache/symlink/drift/stale/corrupt cases. Every value is observed, never invented.
"""
import hashlib, json, os, stat, sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
assert str(root).startswith("/tmp/sk-c6-oracle."), root
for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[key]).parent == root, (key, os.environ[key])
assert Path.cwd() == root / "work", Path.cwd()

from specify_cli.runtime.bootstrap import ensure_runtime, get_kittify_home  # noqa: E402

home = get_kittify_home()
assert home == Path(os.environ["HOME"]) / ".kittify", home
inventory = home / "cache/runtime_bootstrap-assets.json"
lock = home / "cache/.update.lock"
version = home / "cache/version.lock"

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None

def node(path):
    try:
        st = path.lstat()
    except FileNotFoundError:
        return {"kind": "absent"}
    mode = stat.S_IMODE(st.st_mode)
    if stat.S_ISLNK(st.st_mode):
        return {"kind": "symlink", "target": os.readlink(path), "mode": mode}
    if stat.S_ISDIR(st.st_mode):
        return {"kind": "directory", "mode": mode, "mtime_ns": st.st_mtime_ns}
    return {"kind": "file", "mode": mode, "size": st.st_size, "sha256": sha(path),
            "mtime_ns": st.st_mtime_ns, "inode": st.st_ino}

def tree(base):
    out = {}
    for path in sorted(base.rglob("*")):
        rel = path.relative_to(base).as_posix()
        out[rel] = node(path)
    return out

def record(label, keys=("missions", "scripts", "AGENTS.md", "config.yaml", "cache", "missions/custom")):
    return {
        "label": label,
        "inventory_exists": inventory.is_file(),
        "inventory_text": inventory.read_text(encoding="utf-8") if inventory.is_file() else None,
        "inventory_entries": (
            json.loads(inventory.read_text(encoding="utf-8"))["entries"] if inventory.is_file() else None),
        "inventory_node": node(inventory),
        "lock": node(lock),
        "version": (version.read_text(encoding="utf-8").strip() if version.is_file() else None),
        "version_node": node(version),
        "tree": tree(home),
    }

def fresh(home):
    if home.exists():
        import shutil
        shutil.rmtree(home)
    home.mkdir(parents=True)

snapshots = {}

# 1. fresh provisioning
fresh(home)
ensure_runtime()
snapshots["fresh"] = record("fresh")
assert snapshots["fresh"]["inventory_exists"]
assert snapshots["fresh"]["lock"]["kind"] == "file", snapshots["fresh"]["lock"]
assert snapshots["fresh"]["lock"]["mode"] == 0o644, snapshots["fresh"]["lock"]
assert snapshots["fresh"]["lock"]["size"] == 0, snapshots["fresh"]["lock"]
assert snapshots["fresh"]["version"] == "4.0.0rc8", snapshots["fresh"]["version"]
entries = snapshots["fresh"]["inventory_entries"]
# Inventory keys are relative to the owner's operation root: `global_asset_root`
# anchors at `commonpath((HOME, ~/.kittify)).parent`, i.e. the directory ABOVE the
# home folder (observed here, not assumed).
prefix = home.relative_to(home.parent.parent).as_posix()
assert prefix == "home/.kittify", prefix
key = lambda rel: f"{prefix}/{rel}"
assert key("missions/software-dev") in entries, sorted(entries)[:5]
assert entries[key("missions/software-dev")]["kind"] == "directory"
assert entries[key("missions/software-dev")]["mode"] == 0o755
managed = key("missions/software-dev/mission.yaml")
assert entries[managed]["kind"] == "file" and len(entries[managed]["sha256"]) == 64, entries.get(managed)
assert entries[managed]["mtime_ns"] is None, entries[managed]
# `AGENTS.md` is not recorded in this layout: the pinned `packs/built-in` ships no
# AGENTS.md, so the managed-file loop skips it (observed, never assumed).
assert key("AGENTS.md") not in entries
assert key("cache/.update.lock") not in entries and key("cache/version.lock") not in entries

# 2. idempotent re-run: no node is rewritten.
before = {k: v for k, v in snapshots["fresh"]["tree"].items()}
ensure_runtime()
second = record("second")
assert second["inventory_text"] == snapshots["fresh"]["inventory_text"], "inventory rewritten"
for rel, state in before.items():
    after = second["tree"][rel]
    if state["kind"] == "file" and not rel.startswith("cache/"):
        assert after["sha256"] == state["sha256"], rel
        assert after["mtime_ns"] == state["mtime_ns"], (rel, state, after)
assert second["lock"]["inode"] == snapshots["fresh"]["lock"]["inode"], "lock replaced"
snapshots["second"] = second

# 3. mixed state: drift in a managed file, operator file inside a managed tree,
#    custom mission, config, cache and a symlinked managed file.
managed_file = home / "missions/software-dev/mission.yaml"
managed_file.write_text("operator edit\n", encoding="utf-8")
(home / "missions/software-dev/operator.md").write_text("keep me\n", encoding="utf-8")
(home / "missions/custom").mkdir(parents=True, exist_ok=True)
(home / "missions/custom/mine.md").write_text("custom\n", encoding="utf-8")
(home / "config.yaml").write_text("project: keep\n", encoding="utf-8")
(home / "cache/operator.txt").write_text("cache\n", encoding="utf-8")
(home / "missions/software-dev/expected-artifacts.yaml").unlink()
outside = root / "outside-artifacts.yaml"
outside.write_text("linked\n", encoding="utf-8")
(home / "missions/software-dev/expected-artifacts.yaml").symlink_to(outside)
snapshots["mixed_before"] = record("mixed_before")
ensure_runtime()
mixed = record("mixed_after")
assert (home / "missions/software-dev/mission.yaml").read_text(encoding="utf-8") != "operator edit\n"
assert (home / "missions/software-dev/operator.md").read_text(encoding="utf-8") == "keep me\n"
assert (home / "missions/custom/mine.md").read_text(encoding="utf-8") == "custom\n"
assert (home / "config.yaml").read_text(encoding="utf-8") == "project: keep\n"
assert (home / "cache/operator.txt").read_text(encoding="utf-8") == "cache\n"
assert (home / "missions/software-dev/expected-artifacts.yaml").is_symlink()
assert os.readlink(home / "missions/software-dev/expected-artifacts.yaml") == str(outside)
snapshots["mixed_after"] = mixed

# 4. a recorded-but-absent managed descendant is pruned; drift is preserved.
from specify_cli.runtime.asset_preparation import AssetPreparation, global_asset_root  # noqa: E402
from specify_cli.tool_surface.operations import ApplyConsent  # noqa: E402

prep = AssetPreparation(
    "runtime_bootstrap", global_asset_root("runtime_bootstrap", (home,)),
    home / "cache", ".update.lock", ApplyConsent(automatic=True))
prep.asset(home / "missions/software-dev/obsolete.md", b"old release\n", 0o644, managed_tree=True)
prep.asset(home / "missions/software-dev/edited.md", b"old release edited\n", 0o644, managed_tree=True)
assessment = prep.finish(None, "4.0.0rc8")
from specify_cli.runtime.asset_preparation import apply_assets  # noqa: E402
result = apply_assets(assessment, ApplyConsent(automatic=True))
assert (home / "missions/software-dev/obsolete.md").is_file(), result
assert (home / "missions/software-dev/edited.md").is_file()
# Drift a recorded file AFTER it was recorded: `retire` must preserve it while the
# unchanged recorded `obsolete.md` is pruned.
(home / "missions/software-dev/edited.md").write_text("operator drift\n", encoding="utf-8")
prepaced = record("stale_before")
ensure_runtime()
stale = record("stale_after")
assert not (home / "missions/software-dev/obsolete.md").exists(), "recorded stale file not pruned"
assert (home / "missions/software-dev/edited.md").read_text(encoding="utf-8") == "operator drift\n", "edited recorded file was removed"
snapshots["stale_before"] = prepaced
snapshots["stale_after"] = stale

# 5. corrupt inventory is refused, not silently reset.
inventory.write_text("{not json\n", encoding="utf-8")
try:
    ensure_runtime()
except Exception as exc:  # noqa: BLE001 - the refusal is contract data
    corrupt = {"outcome": "error", "error_type": type(exc).__name__, "error": str(exc),
               "inventory_after": inventory.read_text(encoding="utf-8")}
else:
    corrupt = {"outcome": "ok", "inventory_after": inventory.read_text(encoding="utf-8")}
snapshots["corrupt"] = corrupt
assert corrupt["outcome"] == "error", corrupt
assert corrupt["inventory_after"] == "{not json\n", "corrupt inventory was overwritten"

# 6. a managed directory replaced by a regular file is preserved, not clobbered.
fresh(home)
(home / "scripts").write_text("not a directory\n", encoding="utf-8")
ensure_runtime()
collision = {"tree": tree(home), "scripts": node(home / "scripts")}
assert (home / "scripts").read_text(encoding="utf-8") == "not a directory\n", collision
snapshots["collision"] = collision

out = {"assertions": "all passed", "probe": "c6-runtime-ownership",
       "python": sys.version.split()[0], "home": str(home), "snapshots": snapshots}
path = root / "runtime-ownership-record-c6.json"
path.write_text(json.dumps(out, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print("record=", path)
print(json.dumps(out, indent=2, sort_keys=True))
