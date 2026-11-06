//! Real binary composition; current Native error wire is NOT Source traceback parity.
use serde_json::{Value, json};
use sk_install::session::{SESSION_START_CMD, SESSION_STOP_CMD};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::SystemTime,
};

fn fixture(tool: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".kittify")).unwrap();
    fs::write(
        temp.path().join(".kittify/config.yaml"),
        format!("agents:\n  available: [{tool}]\n"),
    )
    .unwrap();
    for name in [
        "home",
        "config",
        "cache",
        "data",
        "state",
        "runtime",
        "config-dirs",
        "data-dirs",
        "tmp",
    ] {
        fs::create_dir(temp.path().join(name)).unwrap();
    }
    temp
}
fn run(root: &Path, tool: &str, kind: &str) -> Output {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("SPECIFY_REPO_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("SPECIFY_KITTY_NO_NAG", "1")
        .env("SPECIFY_KITTY_NO_UPGRADE_CHECK", "1")
        .args([
            "doctor",
            "tool-surfaces",
            "--kind",
            kind,
            "--tool",
            tool,
            "--json",
        ]);
    for (key, name) in [
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
    ] {
        command.env(key, root.join(name));
    }
    command.output().unwrap()
}
type Snapshot = BTreeMap<PathBuf, (Vec<u8>, u32, u64, SystemTime)>;
fn snapshot(root: &Path) -> Snapshot {
    fn walk(root: &Path, here: &Path, out: &mut Snapshot) {
        for entry in fs::read_dir(here).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                walk(root, &path, out);
            } else {
                #[cfg(unix)]
                let (mode, ino) = {
                    use std::os::unix::fs::MetadataExt;
                    (meta.mode(), meta.ino())
                };
                #[cfg(not(unix))]
                let (mode, ino) = (0, 0);
                out.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    (
                        fs::read(&path).unwrap(),
                        mode,
                        ino,
                        meta.modified().unwrap(),
                    ),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}
fn document(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {output:?}"))
}
fn hooks() -> String {
    json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": SESSION_START_CMD}]}], "Stop": [{"hooks": [{"type": "command", "command": SESSION_STOP_CMD}]}]}}).to_string()
}
#[test]
fn actual_binary_honors_all_four_utf16_utf32_boms_without_writes() {
    for (width, big) in [(2, false), (2, true), (4, false), (4, true)] {
        let temp = fixture("claude");
        let root = temp.path();
        fs::create_dir(root.join(".claude")).unwrap();
        let mut bytes = match (width, big) {
            (2, false) => vec![0xff, 0xfe],
            (2, true) => vec![0xfe, 0xff],
            (4, false) => vec![0xff, 0xfe, 0, 0],
            _ => vec![0, 0, 0xfe, 0xff],
        };
        if width == 2 {
            for unit in hooks().encode_utf16() {
                bytes.extend_from_slice(&if big {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
        } else {
            for ch in hooks().chars() {
                bytes.extend_from_slice(&if big {
                    (ch as u32).to_be_bytes()
                } else {
                    (ch as u32).to_le_bytes()
                });
            }
        }
        fs::write(root.join(".claude/settings.json"), bytes).unwrap();
        let before = snapshot(root);
        let output = run(root, "claude", "hook");
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let value = document(&output);
        assert_eq!(value["ok"], true);
        assert_eq!(value["summary"]["present"], 2);
        assert_eq!(value["findings"], json!([]));
        assert_eq!(snapshot(root), before);
    }
}
#[test]
fn actual_binary_preserves_unsafe_hook_vs_escaping_orientation_distinction() {
    let temp = fixture("claude");
    let root = temp.path();
    fs::create_dir(root.join(".claude")).unwrap();
    fs::write(root.join(".claude/settings.json"), b"caf\xe9").unwrap();
    let before = snapshot(root);
    let output = run(root, "claude", "hook");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = document(&output);
    assert_eq!(value["summary"]["errors"], 2);
    assert_eq!(value["summary"]["missing"], 0);
    assert_eq!(value["surfaces"].as_array().unwrap().len(), 2);
    assert!(
        value["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state"] == "unsafe")
    );
    assert!(
        value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["code"] == "session-presence-undecodable")
    );
    assert_eq!(snapshot(root), before);

    let temp = fixture("codex");
    let root = temp.path();
    fs::write(root.join("AGENTS.md"), b"caf\xe9\n").unwrap();
    let before = snapshot(root);
    let output = run(root, "codex", "context_file");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = document(&output);
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("'utf-8' codec can't decode byte 0xe9 in position 3")
    );
    assert!(value.get("surfaces").is_none());
    assert!(value.get("summary").is_none());
    assert!(value.get("ok").is_none());
    assert_eq!(snapshot(root), before);
}
#[test]
fn actual_binary_non_directory_settings_parent_is_missing_not_an_escaping_read() {
    let temp = fixture("claude");
    let root = temp.path();
    fs::write(root.join(".claude"), b"not a directory").unwrap();
    let before = snapshot(root);
    let output = run(root, "claude", "hook");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let value = document(&output);
    assert_eq!(value["summary"]["missing"], 2);
    assert!(value.get("error").is_none());
    assert_eq!(snapshot(root), before);
}
