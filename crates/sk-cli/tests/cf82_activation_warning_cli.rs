//! CF82 parent correction: a present activation list needs no absent-key
//! preservation service. Source `_preservation_ids` returns before that demand,
//! so this actual command must NOT emit the gate's unknown-token logger lines.
//! This is a Native binary regression, not an independent paired CLI claim.
//! Actual service-API warning delivery remains tested in sk-doctrine.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, text).expect("fixture write");
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=cf82",
            "-c",
            "user.email=cf82@example.invalid",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

fn fixture(root: &Path) -> PathBuf {
    let business = root.join("business");
    let project = business.join("project");
    for name in [
        "home",
        "xdg-cache",
        "xdg-config",
        "xdg-data",
        "xdg-state",
        "xdg-runtime",
        "xdg-config-dirs",
        "xdg-data-dirs",
        "tmp",
    ] {
        fs::create_dir_all(root.join(name)).expect("env root");
    }
    fs::create_dir_all(project.join("kitty-specs")).expect("missions dir");
    write(
        &project.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(&project.join("packs/acme/org-charter.yaml"), "{}\n");
    write(
        &project.join(".kittify/charter/charter.md"),
        "# Probe Charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n",
    );
    let tactic = fs::read_to_string(
        repo_root().join("packs/built-in/tactics/moscow-scoping-lens.tactic.yaml"),
    )
    .expect("built-in tactic");
    write(
        &project.join("packs/acme/tactics/gamma.tactic.yaml"),
        &tactic.replace("id: moscow-scoping-lens", "id: gamma-tactic"),
    );
    write(
        &project.join(".kittify/charter/activation.yaml"),
        "activated_tactics: [t01, t02, t03, t04, t05, t06, t07, t08, t09, t10]\n",
    );
    git(&project, &["init", "-q"]);
    git(&project, &["add", ".kittify", "packs", "kitty-specs"]);
    git(&project, &["commit", "-q", "-m", "fixture"]);
    project
}

fn run_present_list(only: bool) {
    let repo = repo_root();
    // The actual filesystem codec this process would use for repository paths.
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path();
    let project = fixture(root);
    assert!(project.is_absolute() && project.is_dir());

    let output = Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
        .current_dir(&project)
        .args(["charter", "activate", "tactic", "gamma", "--json"])
        .args(only.then_some("--only"))
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("XDG_CACHE_HOME", root.join("xdg-cache"))
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_DATA_HOME", root.join("xdg-data"))
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .env("XDG_RUNTIME_DIR", root.join("xdg-runtime"))
        .env("XDG_CONFIG_DIRS", root.join("xdg-config-dirs"))
        .env("XDG_DATA_DIRS", root.join("xdg-data-dirs"))
        .env("TMPDIR", root.join("tmp"))
        .env("TMP", root.join("tmp"))
        .env("TEMP", root.join("tmp"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .output()
        .expect("run the native CLI");

    assert!(
        output.status.success(),
        "CLI failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Actual stdout bytes: still one parseable JSON payload, with the activation
    // the consumer reports.
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let payload: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(payload["id"], serde_json::json!("gamma"));
    assert_eq!(payload["changed"], serde_json::json!(["gamma"]));
    assert_eq!(payload["command"], serde_json::json!("activate"));

    assert!(
        output.stderr.is_empty(),
        "Source does not demand the gated service for a present list: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The activation still landed in the authored ledger.
    let ledger = fs::read_to_string(project.join(".kittify/charter/activation.yaml"))
        .expect("ledger readable");
    assert!(ledger.contains("gamma"), "the CLI write landed: {ledger}");
    if only {
        assert!(
            !ledger.contains("t01"),
            "--only narrowed the list: {ledger}"
        );
        let dropped = payload["no_longer_in_force"]
            .as_array()
            .expect("dropped ids");
        assert_eq!(dropped.len(), 10, "the previous present list is retained");
    } else {
        assert!(ledger.contains("t01"), "the existing list is retained");
    }
}

#[test]
fn a_present_list_skips_absent_key_service_logging_and_keeps_its_json() {
    run_present_list(false);
}

#[test]
fn narrowing_a_present_list_also_skips_service_and_keeps_previous_ids() {
    run_present_list(true);
}
