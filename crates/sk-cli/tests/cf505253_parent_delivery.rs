//! Focused real consumers of exact action text and Source stdout policy.
use sk_kernel::python_json::{self, PythonJsonValue};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

fn fixture(intent: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(".kittify/config.yaml", "# fixture\n");
    write(
        ".kittify/charter/charter.yaml",
        "schema_version: '1.1.0'\ncatalog:\n  references: []\n",
    );
    write(
        ".kittify/charter/charter.md",
        "# Charter\n\n## Policy Summary\n\n- Rule\n",
    );
    write(
        ".kittify/charter-packs/directive/001-architectural-integrity-standard.directive.yaml",
        &format!("id: DIRECTIVE_001\ntitle: Parent title\nintent: \"{intent}\"\n"),
    );
    assert!(
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    root
}

fn command(root: &Path, json: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
    command
        .current_dir(root)
        .env("SPECIFY_REPO_ROOT", root)
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .env_remove("PYTHONIOENCODING")
        .env("LC_ALL", "C.UTF-8")
        .env("PYTHONUTF8", "1")
        .args([
            "charter",
            "context",
            "--action",
            "implement",
            "--mission-type",
            "software-dev",
        ]);
    if json {
        command.arg("--json");
    }
    command
}

#[test]
fn exact_action_summary_is_preserved_in_both_json_text_fields() {
    let root = fixture("Parent summary \\ud800 tail");
    let output = command(root.path(), true).output().unwrap();
    assert!(output.status.success());
    let value = python_json::parse(&String::from_utf8(output.stdout).unwrap()).unwrap();
    for key in ["text", "context"] {
        let Some(PythonJsonValue::String(text)) = value.field(key) else {
            panic!("text field");
        };
        assert!(
            text.codepoints().contains(&0xd800),
            "exact summary was dropped from {key}"
        );
    }
}

#[test]
fn action_text_uses_the_shared_surrogateescape_stdout_policy() {
    let root = fixture("Parent summary \\udc80 tail");
    let output = command(root.path(), false).output().unwrap();
    assert!(output.status.success(), "{:?}", output.status);
    assert!(
        output
            .stdout
            .windows(b"Parent summary \x80 tail".len())
            .any(|part| part == b"Parent summary \x80 tail"),
        "the exact summary byte sequence was omitted"
    );
}

#[test]
fn action_json_uses_the_physical_stdout_codec() {
    let root = fixture("ASCII summary");
    let output = command(root.path(), true)
        .env("PYTHONIOENCODING", "utf-16le")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.get(..2), Some(&b"{\0"[..]));
}

#[test]
fn action_output_encoding_failure_keeps_the_prior_first_load_write() {
    let root = fixture("Parent summary \\ud800 tail");
    let output = command(root.path(), false).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "an unencodable Source summary must not disappear"
    );
    assert!(
        output
            .stdout
            .starts_with(b"Action: implement (bootstrap)\n")
    );
    assert!(
        root.path()
            .join(".kittify/charter/context-state.json")
            .is_file(),
        "Source marks loaded before printing"
    );
}

#[test]
fn nonempty_whitespace_summary_survives_the_real_action_dto() {
    let root = fixture("   ");
    let output = command(root.path(), true).output().unwrap();
    assert!(output.status.success());
    let value = python_json::parse(&String::from_utf8(output.stdout).unwrap()).unwrap();
    let Some(PythonJsonValue::Array(entries)) = value.field("directives") else {
        panic!("directive array");
    };
    let entry = entries
        .iter()
        .find(|entry| entry.field("id") == Some(&PythonJsonValue::scalar("DIRECTIVE_001")))
        .unwrap();
    assert_eq!(
        entry.field("summary"),
        Some(&PythonJsonValue::scalar("   "))
    );
}

#[test]
fn action_json_broken_pipe_is_not_silently_successful() {
    let root = fixture("ASCII summary");
    let mut child = command(root.path(), true)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    assert_eq!(child.wait().unwrap().code(), Some(120));
    assert!(
        root.path()
            .join(".kittify/charter/context-state.json")
            .is_file()
    );
}
