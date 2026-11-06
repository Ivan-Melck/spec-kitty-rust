"""C6/A5 Source-first: the three cutover ownership provers (all shapes, guard, retry)."""
import json, os, sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
assert str(root).startswith("/tmp/sk-c6-oracle."), root
for key in ("HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    assert Path(os.environ[key]).parent == root, (key, os.environ[key])
assert Path.cwd() == root / "work", Path.cwd()

from specify_cli.asset_preservation.guard import guard_destructive_removal  # noqa: E402
from specify_cli.skills.manifest import compute_content_hash  # noqa: E402
from specify_cli.upgrade.migrations._charter_pack_cutover_skills import _FrozenTreeHashProver  # noqa: E402
from specify_cli.upgrade.migrations.m_4_0_0rc6_charter_pack_cutover import (  # noqa: E402
    _EmptyTreeProver,
    _MovedCopyProver,
)

work = root / "work"
project = work / "project"
project.mkdir()

def write(path: Path, text: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path

def prove(prover, path):
    try:
        proof = prover.prove(path, project)
    except Exception as exc:  # noqa: BLE001 - the refusal is contract data
        return {"outcome": "error", "error_type": type(exc).__name__, "error": str(exc)}
    if proof is None:
        return {"outcome": "none"}
    return {"outcome": "proof", "kind": proof.kind, "reference": proof.reference}

def guard(prover, path, *, is_tree=False, dry_run=False):
    verdict = guard_destructive_removal(path, project, prover=prover, is_tree=is_tree, dry_run=dry_run)
    out = {
        "owned": verdict.owned,
        "reason": verdict.reason,
        "diagnostic": verdict.diagnostic,
        "exists_after": path.exists() or path.is_symlink(),
        "preserved_path": None if verdict.preserved_path is None else str(verdict.preserved_path),
        "backup_path": None if verdict.backup_path is None else str(verdict.backup_path),
    }
    if verdict.proof is not None:
        out["proof"] = {"kind": verdict.proof.kind, "reference": verdict.proof.reference}
    return out

cases = {}

# ---------------- MovedCopy ----------------
moved = project / "legacy/old.md"
write(moved, "same bytes\n")
copy = write(project / "pack/new.md", "same bytes\n")
cases["moved_copy_identical"] = prove(_MovedCopyProver(copy), moved)
cases["moved_copy_symlink_identical"] = {
    "source": str(project / "legacy/link-old"),
    "copy": str(project / "pack/link-new"),
}
link_source = project / "legacy/link-old"
link_copy = project / "pack/link-new"
link_source.parent.mkdir(parents=True, exist_ok=True)
link_copy.parent.mkdir(parents=True, exist_ok=True)
link_source.symlink_to("target-a.md")
link_copy.symlink_to("target-a.md")
cases["moved_copy_symlink_identical"] = prove(_MovedCopyProver(link_copy), link_source)
link_other = project / "pack/link-other"
link_other.symlink_to("target-b.md")
cases["moved_copy_symlink_different_target"] = prove(_MovedCopyProver(link_other), link_source)
different = write(project / "pack/different.md", "other bytes\n")
cases["moved_copy_different_bytes"] = prove(_MovedCopyProver(different), moved)
cases["moved_copy_source_symlink_copy_file"] = prove(_MovedCopyProver(moved), link_source)
missing = project / "pack/missing.md"
cases["moved_copy_missing_copy"] = prove(_MovedCopyProver(missing), moved)
directory_copy = project / "pack/dir-copy"
directory_copy.mkdir(parents=True)
cases["moved_copy_copy_is_directory"] = prove(_MovedCopyProver(directory_copy), moved)

# guard: dry run then real removal then retry
guard_source = write(project / "legacy/guard-me.md", "guard bytes\n")
guard_copy = write(project / "pack/guard-copy.md", "guard bytes\n")
prover = _MovedCopyProver(guard_copy)
cases["guard_moved_copy_dry_run"] = guard(prover, guard_source, dry_run=True)
cases["guard_moved_copy_remove"] = guard(prover, guard_source, dry_run=False)
cases["guard_moved_copy_retry"] = guard(prover, guard_source, dry_run=False)
guard_preserved = write(project / "legacy/keep-me.md", "mine\n")
cases["guard_moved_copy_unproven_preserved"] = guard(_MovedCopyProver(guard_copy), guard_preserved, dry_run=True)

# ---------------- EmptyTree ----------------
empty = project / "legacy/empty"
empty.mkdir(parents=True)
cases["empty_tree_empty"] = prove(_EmptyTreeProver(), empty)
nested = project / "legacy/nested"
(nested / "a/b/c").mkdir(parents=True)
cases["empty_tree_nested_empty"] = prove(_EmptyTreeProver(), nested)
with_file = project / "legacy/with-file"
(with_file / "sub").mkdir(parents=True)
write(with_file / "sub/keep.txt", "x\n")
cases["empty_tree_with_file"] = prove(_EmptyTreeProver(), with_file)
with_link = project / "legacy/with-link"
with_link.mkdir(parents=True)
(with_link / "sub").symlink_to(nested)
cases["empty_tree_with_symlinked_dir"] = prove(_EmptyTreeProver(), with_link)
file_path = write(project / "legacy/plain.md", "x\n")
cases["empty_tree_file_path"] = prove(_EmptyTreeProver(), file_path)
link_path = project / "legacy/link-root"
link_path.symlink_to(empty)
cases["empty_tree_symlink_root"] = prove(_EmptyTreeProver(), link_path)
guard_empty = project / "legacy/guard-empty"
(guard_empty / "inner").mkdir(parents=True)
cases["guard_empty_tree_dry_run"] = guard(_EmptyTreeProver(), guard_empty, is_tree=True, dry_run=True)
cases["guard_empty_tree_remove"] = guard(_EmptyTreeProver(), guard_empty, is_tree=True)
cases["guard_empty_tree_retry"] = guard(_EmptyTreeProver(), guard_empty, is_tree=True)

# ---------------- TreeHashes ----------------
tree = project / "legacy/skill"
write(tree / "SKILL.md", "skill body\n")
write(tree / "references/notes.md", "notes\n")
hashes = {
    "SKILL.md": compute_content_hash(tree / "SKILL.md").removeprefix("sha256:"),
    "references/notes.md": compute_content_hash(tree / "references/notes.md").removeprefix("sha256:"),
}
cases["tree_hashes_exact"] = prove(_FrozenTreeHashProver(hashes), tree)
cases["tree_hashes_exact_map_floor"] = {"map": hashes}
extra = project / "legacy/skill-extra"
write(extra / "SKILL.md", "skill body\n")
write(extra / "references/notes.md", "notes\n")
write(extra / "extra.md", "extra\n")
cases["tree_hashes_extra_file"] = prove(_FrozenTreeHashProver(hashes), extra)
missing_tree = project / "legacy/skill-missing"
write(missing_tree / "SKILL.md", "skill body\n")
cases["tree_hashes_missing_file"] = prove(_FrozenTreeHashProver(hashes), missing_tree)
changed_tree = project / "legacy/skill-changed"
write(changed_tree / "SKILL.md", "changed body\n")
write(changed_tree / "references/notes.md", "notes\n")
cases["tree_hashes_changed_bytes"] = prove(_FrozenTreeHashProver(hashes), changed_tree)
linked_tree = project / "legacy/skill-link"
write(linked_tree / "SKILL.md", "skill body\n")
write(linked_tree / "references/notes.md", "notes\n")
(linked_tree / "alias.md").symlink_to("SKILL.md")
cases["tree_hashes_symlink_member"] = prove(_FrozenTreeHashProver(hashes), linked_tree)
cases["tree_hashes_empty_map"] = prove(_FrozenTreeHashProver({}), tree)
root_link = project / "legacy/skill-root-link"
root_link.symlink_to(tree)
cases["tree_hashes_symlink_root"] = prove(_FrozenTreeHashProver(hashes), root_link)
cases["tree_hashes_file_path"] = prove(_FrozenTreeHashProver(hashes), file_path)
guard_skill = project / "legacy/guard-skill"
write(guard_skill / "SKILL.md", "skill body\n")
write(guard_skill / "references/notes.md", "notes\n")
skill_prover = _FrozenTreeHashProver(hashes)
cases["guard_tree_hashes_dry_run"] = guard(skill_prover, guard_skill, is_tree=True, dry_run=True)
cases["guard_tree_hashes_remove"] = guard(skill_prover, guard_skill, is_tree=True)
cases["guard_tree_hashes_retry"] = guard(skill_prover, guard_skill, is_tree=True)

# ---------------- assertions (before any native equivalent) ----------------
assert cases["moved_copy_identical"]["outcome"] == "proof"
assert cases["moved_copy_identical"]["kind"] == "canonical_content"
assert cases["moved_copy_identical"]["reference"] == "moved-copy:legacy/old.md"
assert cases["moved_copy_symlink_identical"]["outcome"] == "proof"
assert cases["moved_copy_symlink_different_target"]["outcome"] == "none"
assert cases["moved_copy_different_bytes"]["outcome"] == "none"
assert cases["moved_copy_source_symlink_copy_file"]["outcome"] == "none"
assert cases["moved_copy_missing_copy"]["outcome"] == "none"
assert cases["moved_copy_copy_is_directory"]["outcome"] == "none"
assert cases["guard_moved_copy_dry_run"]["owned"] is True and cases["guard_moved_copy_dry_run"]["exists_after"] is True
assert cases["guard_moved_copy_remove"]["owned"] is True and cases["guard_moved_copy_remove"]["exists_after"] is False
assert cases["guard_moved_copy_retry"]["owned"] is False and cases["guard_moved_copy_retry"]["reason"] == "absent"
assert cases["guard_moved_copy_unproven_preserved"]["owned"] is False and cases["guard_moved_copy_dry_run"]["exists_after"] is True
assert cases["empty_tree_empty"]["outcome"] == "proof"
assert cases["empty_tree_empty"]["kind"] == "managed_path"
assert cases["empty_tree_empty"]["reference"] == "emptied-legacy-root:legacy/empty"
assert cases["empty_tree_nested_empty"]["outcome"] == "proof"
assert cases["empty_tree_with_file"]["outcome"] == "none"
assert cases["empty_tree_with_symlinked_dir"]["outcome"] == "none"
assert cases["empty_tree_file_path"]["outcome"] == "none"
assert cases["empty_tree_symlink_root"]["outcome"] == "none"
assert cases["guard_empty_tree_dry_run"]["owned"] is True and cases["guard_empty_tree_dry_run"]["exists_after"] is True
assert cases["guard_empty_tree_remove"]["owned"] is True and cases["guard_empty_tree_remove"]["exists_after"] is False
assert cases["guard_empty_tree_retry"]["owned"] is False and cases["guard_empty_tree_retry"]["reason"] == "absent"
assert cases["tree_hashes_exact"]["outcome"] == "proof"
assert cases["tree_hashes_exact"]["kind"] == "canonical_content"
assert cases["tree_hashes_exact"]["reference"] == "shipped-skill-hash:legacy/skill"
assert cases["tree_hashes_extra_file"]["outcome"] == "none"
assert cases["tree_hashes_missing_file"]["outcome"] == "none"
assert cases["tree_hashes_changed_bytes"]["outcome"] == "none"
assert cases["tree_hashes_symlink_member"]["outcome"] == "none"
assert cases["tree_hashes_empty_map"]["outcome"] == "none"
assert cases["tree_hashes_symlink_root"]["outcome"] == "none"
assert cases["tree_hashes_file_path"]["outcome"] == "none"
assert cases["guard_tree_hashes_dry_run"]["owned"] is True and cases["guard_tree_hashes_dry_run"]["exists_after"] is True
assert cases["guard_tree_hashes_remove"]["owned"] is True and cases["guard_tree_hashes_remove"]["exists_after"] is False
assert cases["guard_tree_hashes_retry"]["owned"] is False and cases["guard_tree_hashes_retry"]["reason"] == "absent"

record = {"assertions": "all passed", "probe": "c6-preserve-provers", "python": sys.version.split()[0],
          "cases": cases}
path = root / "preserve-provers-record-c6.json"
path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print("record=", path)
print(json.dumps(record, indent=2, sort_keys=True))
