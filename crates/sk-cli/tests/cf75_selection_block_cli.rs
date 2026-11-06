//! CF75 — live `charter context` consumer of the prepared global-selection block.
//!
//! SOURCE FIRST: the parent runs the pinned Source CLI (guarded child, the actual
//! `specify_cli` entry point) and the Native CLI facade
//! (`sk_cli::compat::charter::execute_context`, guarded child) on two independent
//! fixtures. Both children assert CWD/HOME/all-XDG/TMP, the independently supplied
//! absolute Source and packs roots, the hash seed and the actual filesystem codec
//! before any business read.
//!
//! This is a *presence/order* consumer check, not a whole-text claim: the two
//! facades still differ in their surrounding Action Doctrine / reference text, so
//! only the selection block's own lines and its position are compared. The
//! byte-identical block and its shared budget substitution are proven by the
//! `sk-doctrine` `cf75_selection_block` library pair.

use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MODE_ENV: &str = "CF75_CLI_CHILD_MODE";
const ROOT_ENV: &str = "CF75_CLI_ROOT";
const REPO_ENV: &str = "CF75_CLI_REPO";
const FIXTURE_ENV: &str = "CF75_CLI_FIXTURE";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

fn env_roots() -> [(&'static str, &'static str); 12] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CONFIG_DIRS", "config_dirs"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_DATA_DIRS", "data_dirs"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
        ("SK_CF75_ROOT", "root"),
    ]
}

fn assert_child_environment(root: &Path, repo: &Path, fixture: &Path) {
    assert!(repo.is_absolute(), "absolute Source repo");
    assert!(fixture.is_absolute(), "absolute fixture");
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    assert_eq!(std::env::current_dir().expect("child cwd"), root);
    for (key, name) in env_roots() {
        let expected = if name == "root" {
            root.to_path_buf()
        } else {
            root.join(name)
        };
        assert_eq!(
            PathBuf::from(std::env::var_os(key).unwrap_or_default()),
            expected,
            "{key}"
        );
    }
    assert_eq!(
        PathBuf::from(std::env::var_os("PYTHONPATH").expect("absolute PYTHONPATH")),
        repo.join("src")
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("SPEC_KITTY_PACKS_ROOT").expect("packs root")),
        repo.join("packs")
    );
    assert_eq!(
        std::env::var_os("PYTHONHASHSEED").as_deref(),
        Some(std::ffi::OsStr::new("0"))
    );
}

fn child_command(root: &Path, repo: &Path, fixture: &Path, mut command: Command) -> Command {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/home/exedev/.cargo/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .env(ROOT_ENV, root)
        .env(REPO_ENV, repo)
        .env(FIXTURE_ENV, fixture);
    for (key, name) in env_roots() {
        let value = if name == "root" {
            root.to_path_buf()
        } else {
            root.join(name)
        };
        command.env(key, value);
    }
    command
}

fn write_fixture(root: &Path) -> PathBuf {
    let fixture = root.join("fixture");
    fs::create_dir_all(fixture.join(".kittify/charter")).expect("charter dir");
    fs::create_dir_all(fixture.join(".kittify/charter-packs/directive")).expect("project layer");
    fs::create_dir_all(fixture.join("kitty-specs")).expect("missions dir");
    fs::write(
        fixture.join(".kittify/charter/charter.md"),
        "# Probe Charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n",
    )
    .expect("charter");
    fs::write(
        fixture.join(".kittify/charter-packs/directive/901-selected.directive.yaml"),
        "schema_version: \"1.0\"\nid: DIRECTIVE_901\ntitle: Selected Directive\nintent: The selection block must carry this intent.\nenforcement: required\n",
    )
    .expect("selected directive");
    fs::write(
        fixture.join(".kittify/charter/charter.yaml"),
        "governance:\n  charter:\n    selected_directives:\n      - DIRECTIVE_901\n",
    )
    .expect("charter yaml");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .current_dir(&fixture)
            .args([
                "-c",
                "user.name=cf75",
                "-c",
                "user.email=cf75@example.invalid",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", root.join("home"))
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", ".kittify"]);
    git(&["commit", "-q", "-m", "fixture"]);
    fixture
}

/// Native facade child (inert unless the parent sets the mode).
#[test]
fn cf75_native_cli_child() {
    let Ok(mode) = std::env::var(MODE_ENV) else {
        return;
    };
    assert_eq!(mode, "native");
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("root"));
    let repo = PathBuf::from(std::env::var_os(REPO_ENV).expect("repo"));
    let fixture = PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("fixture"));
    assert_child_environment(&root, &repo, &fixture);
    let outcome = sk_cli::compat::charter::execute_context(
        &fixture,
        "implement",
        None,
        None,
        false,
        false,
        true,
    )
    .expect("native context");
    let text = outcome
        .text
        .to_scalar_string()
        .expect("scalar fixture text");
    println!("CF75_JSON={}", json!({"text": text}));
}

const SOURCE_CHILD: &str = r#"
import os, runpy, sys
from pathlib import Path

root = Path(os.environ["CF75_CLI_ROOT"])
repo = Path(os.environ["CF75_CLI_REPO"])
fixture = Path(os.environ["CF75_CLI_FIXTURE"])
for name in ("HOME","XDG_CONFIG_HOME","XDG_CONFIG_DIRS","XDG_CACHE_HOME",
             "XDG_DATA_HOME","XDG_DATA_DIRS","XDG_STATE_HOME","XDG_RUNTIME_DIR",
             "TMPDIR","TMP","TEMP"):
    value = os.environ.get(name)
    assert value and Path(value).is_absolute(), name
assert Path.cwd() == root, (Path.cwd(), root)
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
assert os.environ["PYTHONHASHSEED"] == "0"
assert sys.version.split()[0] == "3.11.15", sys.version
assert sys.getfilesystemencoding() == "utf-8", sys.getfilesystemencoding()
assert sys.getfilesystemencodeerrors() == "surrogateescape"
assert os.path.realpath(sys.prefix).endswith(".venv-rust-oracle")

os.chdir(fixture)
sys.argv = ["spec-kitty", "charter", "context", "--action", "implement"]
runpy.run_module("specify_cli", run_name="__main__")
"#;

fn run_child(mut command: Command, label: &str) -> String {
    let output = command.output().expect("child runs");
    assert!(
        output.status.success(),
        "{label} child failed: status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(line) = stdout.lines().find(|line| line.starts_with("CF75_JSON=")) {
        let value: serde_json::Value =
            serde_json::from_str(&line["CF75_JSON=".len()..]).expect("child JSON");
        return value["text"].as_str().expect("text").to_owned();
    }
    stdout.into_owned()
}

#[test]
fn cf75_live_selection_block_reaches_both_cli_facades() {
    let workspace = tempfile::tempdir().expect("workspace");
    let source_fixture = write_fixture(&workspace.path().join("source"));
    let native_fixture = write_fixture(&workspace.path().join("native"));
    let repo = repo();

    let mut roots = Vec::new();
    for name in ["source-root", "native-root"] {
        let root = workspace.path().join(name);
        for (_, dir) in env_roots() {
            if dir == "root" {
                fs::create_dir_all(&root).expect("root");
            } else {
                fs::create_dir_all(root.join(dir)).expect("root subdir");
            }
        }
        roots.push(root);
    }

    let script = workspace.path().join("cf75_source_cli_child.py");
    fs::write(&script, SOURCE_CHILD).expect("source script");
    let python = repo.join(".venv-rust-oracle/bin/python");
    assert!(python.is_file(), "required oracle missing");
    let mut source_command =
        child_command(&roots[0], &repo, &source_fixture, Command::new(&python));
    source_command.arg(&script);
    let source = run_child(source_command, "source");

    let mut native_command = child_command(
        &roots[1],
        &repo,
        &native_fixture,
        Command::new(std::env::current_exe().expect("test binary")),
    );
    native_command.env(MODE_ENV, "native").args([
        "--exact",
        "cf75_native_cli_child",
        "--nocapture",
    ]);
    let native = run_child(native_command, "native");

    assert!(
        source.contains("    Intent: The selection block must carry this intent."),
        "the Source bootstrap inlines the selected body:\n{source}"
    );
    for (label, text) in [("SOURCE", &source), ("NATIVE", &native)] {
        assert!(
            text.contains("Selected directives:"),
            "{label} rendered no selection header:\n{text}"
        );
        assert!(
            text.contains("  - DIRECTIVE_901"),
            "{label} did not select the project directive:\n{text}"
        );
        // The Native facade's larger Action Doctrine text makes the shared budget
        // substitute this candidate group; either the verbatim body or the
        // canonical fetch stanza must follow the id. A catalogue miss is not
        // acceptable for an artifact that exists.
        assert!(
            !text.contains("catalog entry not found for directive:DIRECTIVE_901"),
            "{label} fabricated a catalogue miss:\n{text}"
        );
        assert!(
            text.contains("    Intent: The selection block must carry this intent.")
                || text.contains("Run: spec-kitty charter context --include directive:SELECTED_ID"),
            "{label} did not render the selected entry:\n{text}"
        );
        // The block sits where the reference renders it: after the profile block
        // (absent here) and before the Action Doctrine heading.
        let selection = text.find("Selected directives:").expect("selection");
        let doctrine = text.find("Action Doctrine (implement):").expect("doctrine");
        assert!(selection < doctrine, "{label} order:\n{text}");
        // A project layer carries no org provenance suffix.
        assert!(
            !text.contains("DIRECTIVE_901 (source: org)"),
            "{label}:\n{text}"
        );
    }
}
