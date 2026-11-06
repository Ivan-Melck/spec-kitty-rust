//! Source-first identity wire for `profiles list` and `dispatch`.
//!
//! Drives the real `spec-kitty` binary over a temporary project and compares it
//! with the pinned Source CLI, which runs first and asserts its own escaping
//! (`ensure_ascii=True`: a PUA code point is `\ue000`, an astral code point is
//! `\ud83d\ude00`, a lone surrogate is `\ud800`). No identity is omitted or
//! replaced by a marker.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde_json::{Value, json};
use tempfile::TempDir;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root resolves")
}

fn python() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let root = repository_root();
            let python = root.join(".venv-rust-oracle/bin/python");
            assert!(python.is_file(), "missing {}", python.display());
            let result = Command::new(&python)
                .arg(root.join("scripts/rust/verify_oracle.py"))
                .arg("--root")
                .arg(&root)
                .output()
                .expect("execute required Python oracle verifier");
            assert!(
                result.status.success(),
                "Python oracle is not lock-exact: {}",
                String::from_utf8_lossy(&result.stdout)
            );
            python
        })
        .clone()
}

struct Project {
    _temp: TempDir,
    root: PathBuf,
}

impl Project {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary root");
        let root = temp.path().canonicalize().expect("canonical root");
        for directory in ["home", "cache", "config", "data", "state", "tmp"] {
            fs::create_dir_all(root.join(directory)).expect("isolated directory");
        }
        let profiles = root.join(".kittify/profiles");
        fs::create_dir_all(&profiles).expect("profile directory");
        fs::write(
            profiles.join("astral.agent.yaml"),
            "profile-id: \"astral-\u{1f600}\"\nname: \"Name\u{1f600}\"\npurpose: P\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n",
        )
        .expect("astral profile");
        fs::write(
            profiles.join("pua.agent.yaml"),
            "profile-id: \"pua-\u{e000}x\"\nname: \"Name\u{e000}X\"\npurpose: P\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n",
        )
        .expect("pua profile");
        fs::write(
            profiles.join("surrogate.agent.yaml"),
            "profile-id: \"author\\uD800\"\nname: \"Name\\uD800\"\npurpose: P\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n",
        )
        .expect("surrogate profile");
        let output = Command::new("git")
            .args([
                "init",
                "-q",
                "-b",
                "feature/identity",
                root.to_str().expect("utf8"),
            ])
            .output()
            .expect("git init");
        assert!(output.status.success());
        Self { _temp: temp, root }
    }

    fn env(&self) -> Vec<(String, String)> {
        let root = &self.root;
        let repository = repository_root();
        vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("HOME".to_owned(), root.join("home").display().to_string()),
            (
                "XDG_CACHE_HOME".to_owned(),
                root.join("cache").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                root.join("config").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".to_owned(),
                root.join("data").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_owned(),
                root.join("state").display().to_string(),
            ),
            ("TMPDIR".to_owned(), root.join("tmp").display().to_string()),
            ("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
            (
                "PYTHONPATH".to_owned(),
                repository.join("src").display().to_string(),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".to_owned(),
                repository.join("packs").display().to_string(),
            ),
        ]
    }

    fn source(&self, args: &[&str]) -> String {
        let script = "from specify_cli import main\nmain()\n";
        let output = Command::new(python())
            .arg("-c")
            .arg(script)
            .args(args)
            .env_clear()
            .envs(self.env())
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run source CLI");
        assert!(
            output.status.success(),
            "Source CLI failed: {args:?}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("source stdout")
    }

    fn native(&self, args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
            .args(args)
            .env_clear()
            .envs(self.env())
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run native CLI");
        assert!(
            output.status.success(),
            "native CLI failed: {args:?}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("native stdout")
    }
}

const LIST_COMPARE: &str = r#"
import json, os
from pathlib import Path
source = json.loads(Path(os.environ["SK_SOURCE_JSON"]).read_text(encoding="utf-8"))
native = json.loads(Path(os.environ["SK_NATIVE_JSON"]).read_text(encoding="utf-8"))
wanted = ["".join(chr(int(code)) for code in codes) for codes in json.loads(os.environ["SK_WANTED"])]
source_by_id = {row["profile_id"]: row for row in source}
native_by_id = {row["profile_id"]: row for row in native}
out = {}
for profile_id in wanted:
    assert profile_id in source_by_id, (profile_id, sorted(source_by_id))
    assert profile_id in native_by_id, (profile_id, sorted(native_by_id))
    assert native_by_id[profile_id] == source_by_id[profile_id], (
        profile_id, native_by_id[profile_id], source_by_id[profile_id])
    out[repr(profile_id)] = True
print(json.dumps({"count": len(out), "astral": source_by_id["astral-\U0001F600"]}))
"#;

#[test]
fn profiles_list_json_keeps_every_exact_identity() {
    let project = Project::new();
    let source = project.source(&["profiles", "list", "--json"]);
    // Source-first: the exact escapes are the reference's own output.
    assert!(source.contains(r#""astral-\ud83d\ude00""#), "{source}");
    assert!(source.contains(r#""pua-\ue000x""#), "{source}");
    assert!(source.contains(r#""author\ud800""#), "{source}");
    assert!(
        !source.contains("astral-\u{1f600}"),
        "ensure_ascii is required"
    );
    assert!(
        !source.contains("pua-\u{e000}x"),
        "ensure_ascii is required"
    );

    let native = project.native(&["profiles", "list", "--json"]);
    assert!(native.contains(r#""astral-\ud83d\ude00""#), "{native}");
    assert!(native.contains(r#""pua-\ue000x""#), "{native}");
    assert!(native.contains(r#""author\ud800""#), "{native}");
    assert!(
        !native.contains("astral-\u{1f600}"),
        "ensure_ascii is required"
    );
    assert!(
        !native.contains("pua-\u{e000}x"),
        "ensure_ascii is required"
    );

    // Field-for-field row equality, including the non-scalar identity.
    let source_path = project.root.join("source-list.json");
    let native_path = project.root.join("native-list.json");
    fs::write(&source_path, &source).expect("source list");
    fs::write(&native_path, &native).expect("native list");
    let wanted = vec![
        vec![0x61, 0x73, 0x74, 0x72, 0x61, 0x6c, 0x2d, 0x1f600],
        vec![0x70, 0x75, 0x61, 0x2d, 0xe000, 0x78],
        vec![0x61, 0x75, 0x74, 0x68, 0x6f, 0x72, 0xd800],
    ];
    let output = Command::new(python())
        .arg("-c")
        .arg(LIST_COMPARE)
        .env_clear()
        .envs(project.env())
        .env("SK_SOURCE_JSON", &source_path)
        .env("SK_NATIVE_JSON", &native_path)
        .env("SK_WANTED", serde_json::to_string(&wanted).unwrap())
        .current_dir(&project.root)
        .output()
        .expect("compare profile rows");
    assert!(
        output.status.success(),
        "profile row comparison failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value = serde_json::from_slice(&output.stdout).expect("rows");
    assert_eq!(summary["count"], json!(3));
    let astral = &summary["astral"];
    assert_eq!(astral["role"], json!("reviewer"));
    assert_eq!(astral["source"], json!("project_local"));
    assert_eq!(
        astral["action_domains"],
        json!(["assess", "audit", "review"])
    );
}

#[test]
fn dispatch_dry_run_json_prefix_is_exact() {
    let project = Project::new();
    let astral = "astral-\u{1f600}";
    let source = project.source(&[
        "dispatch",
        "please review this",
        "--profile",
        astral,
        "--dry-run",
        "--json",
    ]);
    let expected = r#"{"profile_id": "astral-\ud83d\ude00", "profile_friendly_name": "Name\ud83d\ude00", "action": "audit""#;
    assert!(
        source.starts_with(expected),
        "Source prefix: {}",
        &source[..source.len().min(200)]
    );

    let native = project.native(&[
        "dispatch",
        "please review this",
        "--profile",
        astral,
        "--dry-run",
        "--json",
    ]);
    assert!(
        native.starts_with(expected),
        "native prefix: {}",
        &native[..native.len().min(200)]
    );
    let value: Value = serde_json::from_str(&native).expect("native dry-run JSON");
    assert_eq!(value["profile_id"], json!(astral));
    assert_eq!(value["action"], json!("audit"));
    assert_eq!(value["status"], json!("dry_run"));
}
