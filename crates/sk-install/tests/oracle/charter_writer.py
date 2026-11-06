"""Actual Source charter callback: failure class/order, Unicode and readonly preview."""

import json
import os
from pathlib import Path
import stat
import sys

root = Path(os.environ["SK_CHARTER_WRITER_ROOT"])
repo = Path(os.environ["PYTHONPATH"]).parent
assert Path.cwd() == root and sys.version_info[:3] == (3, 11, 15)
for key, name in [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
]:
    assert os.environ[key] == str(root / name)
assert repo.is_absolute() and Path(os.environ["PYTHONPATH"]) == repo / "src"
assert os.environ["SPEC_KITTY_PACKS_ROOT"] == str(repo / "packs")
from specify_cli.upgrade.migrations.m_3_1_1_charter_rename import CharterRenameMigration  # noqa: E402 - assert actual Source roots before product import

BODY = "abcdefghij🙂 constitution\nCONSTITUTION conſtitution constİtutıon\n"


def fixture(project, case):
    directory = project / ".kittify/charter"
    directory.mkdir(parents=True)
    (directory / "z.md").write_bytes(b"constitution later\n")
    if case == "symlink":
        (project / "external").write_bytes(b"constitution target\n")
        (directory / "a.md").symlink_to("../../external")
    elif case == "symlink-dir":
        (project / "external").mkdir()
        (project / "external/a.md").write_bytes(b"constitution target\n")
        (directory / "a").symlink_to("../../external", target_is_directory=True)
    elif case == "oserror":
        (directory / "locked").mkdir()
        (directory / "locked/a.md").write_bytes(b"constitution locked\n")
    else:
        (directory / "a.md").write_text(BODY if case == "unicode" else "constitution first\n", encoding="utf-8")
    artifact = directory / f".a.md.tmp{os.getpid()}" if case == "collision" else None
    if artifact:
        artifact.write_bytes(b"operator artifact\x00\xff")
    for path in project.rglob("*"):
        if not path.is_symlink():
            path.chmod(0o775 if path.is_dir() else 0o664)
    if case == "oserror":
        (directory / "locked").chmod(0o555)
    return artifact


def snapshot(project):
    result = {}
    for path in project.rglob("*"):
        metadata = path.lstat()
        if stat.S_ISDIR(metadata.st_mode):
            continue
        result[str(path.relative_to(project))] = (
            list(os.readlink(path).encode()) if path.is_symlink() else list(path.read_bytes()),
            stat.S_IMODE(metadata.st_mode),
            path.is_symlink(),
            metadata.st_ino,
            metadata.st_mtime_ns,
        )
    return result


rows = []
for number, (case, preview) in enumerate(
    [
        ("collision", False),
        ("symlink", False),
        ("unicode", False),
        ("symlink-dir", False),
        ("oserror", False),
        ("collision", True),
        ("unicode", True),
    ]
):
    project = root / f"source-{number}"
    artifact = fixture(project, case)
    before = snapshot(project)
    caught = None
    try:
        result = CharterRenameMigration().apply(project, dry_run=preview)
    except Exception as error:
        caught = error
    after = snapshot(project)
    row = {"case": case, "preview": preview, "project": str(project), "raw_before": before.copy(), "raw_after": after.copy()}
    rewritten = set()
    if caught is not None:
        assert not preview and case in ("collision", "symlink")
        assert type(caught) is ValueError, (case, type(caught), caught)
        if artifact:
            assert str(caught) == f"Refusing to replace an unproven atomic writer artifact: {artifact}"
        assert before == after, "ValueError must abort before the later file"
        row.update(exception="ValueError", message=str(caught), relative_message=str(caught).replace(str(project), "<project>"))
    else:
        assert preview or case not in ("collision", "symlink"), (case, result)
        assert result.success == (case != "oserror"), (case, result)
        assert len(result.errors) == (1 if case == "oserror" else 0), (case, result)
        row.update(success=result.success, errors=result.errors, changes=result.changes_made, warnings=result.warnings)
        rewritten = {change.removeprefix("Rewrote content in ") for change in result.changes_made if change.startswith("Rewrote content in ")}
        if preview:
            assert before == after, "preview changed physical artifacts"
        elif case == "unicode":
            assert (project / ".kittify/charter/a.md").read_text() == "abcdefghij🙂 charter\nCharter charter charter\n"
        elif case == "symlink-dir":
            assert (project / "external/a.md").read_bytes() == b"constitution target\n"
        elif case == "oserror":
            assert (project / ".kittify/charter/locked/a.md").read_bytes() == b"constitution locked\n"
            assert (project / ".kittify/charter/z.md").read_bytes() == b"charter later\n"
    if artifact:
        key = str(artifact.relative_to(project))
        assert before[key] == after[key], "pre-existing PID artifact was touched"
        row["artifact_path"] = str(artifact)
        row["artifact_pid"] = os.getpid()
        del after[key]
        del before[key]
    # Source-generated writes ALWAYS replace the inode, but creation timestamps
    # can legitimately coincide at the filesystem's clock resolution. Retain
    # the real metadata, enforce exact untouched mtimes and publication instead
    # of asserting that every independent write must tick the clock.
    for key, record in after.items():
        if key in rewritten:
            assert key in before and record[3] != before[key][3], (key, "not published")
        else:
            assert record == before[key], (key, "untouched artifact changed")
    row["rewritten"] = sorted(rewritten)
    row["files"] = {
        key: {
            "bytes": record[0],
            "mode": record[1],
            "symlink": record[2],
            "same_inode": key in before and record[3] == before[key][3],
            "same_mtime": None if key in rewritten else key in before and record[4] == before[key][4],
        }
        for key, record in after.items()
    }
    rows.append(row)
json.dump(rows, sys.stdout, ensure_ascii=True)
