use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty-rust")
}
fn invoke(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(root)
        .env_remove("SPECIFY_REPO_ROOT")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .args(args)
        .output()
        .unwrap()
}
fn document(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn explicit_roots_must_be_marked_roots_not_subdirectories_or_parents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let nested = root.join("src");
    fs::create_dir_all(root.join(".kittify")).unwrap();
    fs::create_dir(&nested).unwrap();
    for invalid in [nested.as_path(), temp.path()] {
        let output = invoke(
            &root,
            &[
                "mission",
                "list",
                "--project-root",
                invalid.to_str().unwrap(),
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(document(&output)["error"]["kind"], "domain");
    }
    assert!(invoke(&nested, &["mission", "list"]).status.success());
}

#[test]
fn ignored_environment_override_does_not_claim_to_be_the_resolution_source() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".kittify")).unwrap();
    let output = Command::new(binary())
        .current_dir(temp.path())
        .env("SPECIFY_REPO_ROOT", temp.path().join("missing"))
        .env_remove("GIT_CEILING_DIRECTORIES")
        .args(["project", "root"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(document(&output)["data"]["source"], "discovery");
}

#[test]
fn json_help_and_version_are_single_json_documents() {
    let temp = tempfile::tempdir().unwrap();
    for (flag, name) in [("--help", "cli.help"), ("--version", "cli.version")] {
        let output = invoke(temp.path(), &["--json", flag]);
        assert!(output.status.success());
        assert_eq!(document(&output)["command"], name);
    }
    let error = invoke(temp.path(), &["bogus"]);
    assert!(
        !document(&error)["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("error:")
    );
}

#[cfg(unix)]
#[test]
fn git_json_preserves_raw_unix_filename_bytes_alongside_display_text() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".kittify")).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(temp.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        temp.path()
            .join(std::ffi::OsString::from_vec(b"bad\xff.txt".to_vec())),
        b"data",
    )
    .unwrap();
    let output = invoke(temp.path(), &["git", "inspect"]);
    assert!(output.status.success());
    assert_eq!(
        document(&output)["data"]["entries"][0]["path_bytes_hex"],
        "626164ff2e747874"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_charter_roots_and_asset_names_are_domain_errors_not_internal_errors() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    let invalid_root = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"project\xff".to_vec()));
    fs::create_dir_all(invalid_root.join(".kittify")).unwrap();
    let output = Command::new(binary())
        .arg("--project-root")
        .arg(&invalid_root)
        .args(["charter", "inspect"])
        .output()
        .unwrap();
    assert_eq!(document(&output)["error"]["kind"], "domain");
    let pack = temp.path().join(".kittify/charter-packs");
    fs::create_dir_all(pack.join("assets")).unwrap();
    fs::write(
        pack.join("pack.yaml"),
        "pack_id: 01ARZ3NDEKTSV4RRFFQ69G5FAV\npack_level: project\nname: Test\n",
    )
    .unwrap();
    fs::write(
        pack.join("assets")
            .join(std::ffi::OsString::from_vec(b"bad\xff.asset.yaml".to_vec())),
        "id: test\nmime: text/plain\npath: test.txt\n",
    )
    .unwrap();
    let output = invoke(temp.path(), &["charter", "inspect"]);
    assert_eq!(document(&output)["error"]["kind"], "domain");
    assert!(
        document(&output)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("UTF-8")
    );
}

#[test]
fn cli_and_library_share_the_eight_mib_seed_budget() {
    let temp = tempfile::tempdir().unwrap();
    let seed = temp.path().join("core.yaml");
    let mut yaml = "terms:\n  - surface: mission\n    definition: A delivery unit\n    confidence: 1.0\n    status: active\n".to_owned();
    yaml.push_str(&" ".repeat(2 * 1024 * 1024));
    fs::write(&seed, yaml).unwrap();
    let option = format!("spec_kitty_core={}", seed.display());
    let output = invoke(
        temp.path(),
        &[
            "glossary",
            "lookup",
            "mission",
            "--scope",
            "spec_kitty_core",
            "--seed-file",
            &option,
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(document(&output)["ok"], true);
}
