//! Real binary reachability for CF22/23, not Source differential or isolation proof.
//! CF14 exercises the actual sweep closure/retry; these cases cover diagnostic
//! dispatch, ordinary findings, filters, exact error envelopes and repair refusals.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    environment: PathBuf,
}

impl Fixture {
    fn new(agents: Option<&[&str]>) -> Self {
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path().join("project");
        let environment = temp.path().join("environment");
        fs::create_dir_all(&root).unwrap();
        for name in [
            "home",
            "cache",
            "config",
            "data",
            "state",
            "runtime",
            "config-dirs",
            "data-dirs",
            "tmp",
        ] {
            fs::create_dir_all(environment.join(name)).unwrap();
        }
        if let Some(agents) = agents {
            fs::create_dir_all(root.join(".kittify")).unwrap();
            let available: Vec<_> = agents
                .iter()
                .map(|agent| format!("    - {agent}\n"))
                .collect();
            fs::write(
                root.join(".kittify/config.yaml"),
                format!("agents:\n  available:\n{}", available.join("")),
            )
            .unwrap();
        }
        Self {
            _temp: temp,
            root,
            environment,
        }
    }

    fn run(&self, argv: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
        command.current_dir(&self.root).args(argv);
        for (key, name) in [
            ("HOME", "home"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("XDG_CONFIG_DIRS", "config-dirs"),
            ("XDG_DATA_DIRS", "data-dirs"),
            ("TMPDIR", "tmp"),
            ("TMP", "tmp"),
            ("TEMP", "tmp"),
        ] {
            command.env(key, self.environment.join(name));
        }
        command
            .env_remove("SPECIFY_REPO_ROOT")
            .env_remove("GIT_CEILING_DIRECTORIES")
            .env("SPECIFY_KITTY_NO_NAG", "1")
            .env("SPECIFY_KITTY_NO_UPGRADE_CHECK", "1")
            .output()
            .expect("real binary")
    }
}

fn payload(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid single JSON document: {error}; output={output:?}"))
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

#[test]
fn command_files_reaches_the_user_global_audit_without_writing() {
    let fixture = Fixture::new(Some(&["claude"]));
    let before = files(&fixture.root);
    let output = fixture.run(&["doctor", "command-files", "--json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let rows = payload(&output);
    let rows = rows.as_array().expect("Source raw issue array");
    assert_eq!(rows.len(), sk_install::CANONICAL_COMMANDS.len());
    for row in rows {
        assert_eq!(row["agent"], json!("claude"));
        assert_eq!(row["severity"], json!("error"));
        assert!(
            row["file"]
                .as_str()
                .unwrap()
                .starts_with(fixture.environment.join("home").to_str().unwrap())
        );
    }
    assert_eq!(files(&fixture.root), before);
}

#[test]
fn skills_and_filtered_tool_surfaces_return_real_missing_findings() {
    let fixture = Fixture::new(Some(&["codex"]));
    let before = files(&fixture.root);
    let skills = fixture.run(&["doctor", "skills", "--json"]);
    assert_eq!(skills.status.code(), Some(1), "{skills:?}");
    let skills = payload(&skills);
    assert_eq!(skills["ok"], json!(false));
    // Source verify() reserves `gaps` for absent recorded manifest entries.
    // This fresh project has no manifest: its real failure is uninstalled codex.
    assert_eq!(skills["entries"], json!(0));
    assert_eq!(skills["gaps"], json!([]));
    assert_eq!(skills["uninstalled_agents"], json!(["codex"]));
    assert_eq!(skills["tool_folders"][0]["kind"], json!("no_tool_folder"));

    let tools = fixture.run(&[
        "doctor",
        "tool-surfaces",
        "--kind",
        "command-skill",
        "--tool",
        "codex",
        "--json",
    ]);
    assert_eq!(tools.status.code(), Some(1), "{tools:?}");
    let tools = payload(&tools);
    assert_eq!(tools["ok"], json!(false));
    assert!(tools["summary"]["missing"].as_u64().unwrap() > 0, "{tools}");
    for surface in tools["surfaces"].as_array().unwrap() {
        assert_eq!(surface["tool"], json!("codex"));
        assert_eq!(surface["kind"], json!("command_skill"));
    }
    assert_eq!(files(&fixture.root), before);
}

#[test]
fn tool_errors_keep_the_source_envelope_and_the_actual_exit() {
    let outside = Fixture::new(None);
    for (leaf, exit) in [("command-files", 1), ("skills", 2), ("tool-surfaces", 2)] {
        let output = outside.run(&["doctor", leaf, "--json"]);
        assert_eq!(output.status.code(), Some(exit), "{output:?}");
        assert_eq!(
            payload(&output),
            json!({"ok": false, "error": {
                "code": "not_in_project", "message": "Not in a spec-kitty project"
            }})
        );
    }
    let fixture = Fixture::new(Some(&["codex"]));
    let output = fixture.run(&["doctor", "tool-surfaces", "--kind", "not-a-kind", "--json"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let error = payload(&output);
    assert_eq!(error["ok"], json!(false));
    assert_eq!(error["error"]["code"], json!("unknown_kind"));
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Known kinds")
    );
    assert!(error.get("error_code").is_none());

    fs::write(
        fixture.root.join(".kittify/config.yaml"),
        "- not-a-config-map\n",
    )
    .unwrap();
    for argv in [
        vec!["doctor", "skills", "--json"],
        vec!["doctor", "tool-surfaces", "--json"],
    ] {
        let output = fixture.run(&argv);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let error = payload(&output);
        assert_eq!(error["ok"], json!(false));
        assert_eq!(error["error"]["code"], json!("config_error"));
        assert!(error["error"]["message"].is_string());
    }
}

#[test]
fn unsupported_repairs_and_threshold_usage_remain_explicit_and_nonmutating() {
    let fixture = Fixture::new(Some(&["codex"]));
    let before = files(&fixture.root);
    for (leaf, code) in [
        ("skills", "UNSUPPORTED_DOCTOR_SKILLS_FIX"),
        ("tool-surfaces", "UNSUPPORTED_DOCTOR_TOOL_SURFACES_FIX"),
    ] {
        let output = fixture.run(&["doctor", leaf, "--fix", "--json"]);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let error = payload(&output);
        assert_eq!(error["error_code"], json!(code));
        assert!(error["coverage"].as_array().unwrap().len() >= 2);
    }
    let output = fixture.run(&["doctor", "ops", "--threshold", "0", "--json"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        payload(&output)["error"]
            .as_str()
            .unwrap()
            .contains("--close-stale")
    );
    assert_eq!(files(&fixture.root), before);
}
