//! CF79 — the actual CLI consumer of the Source activation-reader warnings.
//!
//! `spec-kitty charter list` is the ported read that Loads the reader on its
//! ordinary command path (Source `list_cmd` → `load_activation_ledger`), so it is
//! where the reference's two plain-stderr warning lines must reach the real
//! process stderr while `--json` stdout stays parseable. `spec-kitty charter
//! status` is the Source `report_stray=False` collector, so the same fixture must
//! print nothing there.
//!
//! Every case runs the real binary in a fresh project with isolated HOME/all-XDG/
//! TMPDIR/TMP/TEMP and its own working directory.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("sandbox");
        for directory in ["home", "cache", "config", "data", "state", "runtime", "tmp"] {
            fs::create_dir_all(temp.path().join(directory)).expect("sandbox dir");
        }
        Self { root: temp.keep() }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn env_pairs(&self) -> [(&'static str, &'static str); 9] {
        [
            ("HOME", "home"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("TMPDIR", "tmp"),
            ("TMP", "tmp"),
            ("TEMP", "tmp"),
        ]
    }
}

fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, body).expect("fixture write");
}

/// A modern project with stray legacy allow keys beside the ledger.
fn stray_project(sandbox: &Sandbox) -> PathBuf {
    let root = sandbox.path("stray");
    write(
        &root,
        ".kittify/config.yaml",
        "activated_directives: [010-x]\nactivated_tactics: [t-1]\n",
    );
    write(&root, ".kittify/charter/activation.yaml", "{}\n");
    root
}

/// A legacy project with a hand-written deny key.
fn legacy_project(sandbox: &Sandbox) -> PathBuf {
    let root = sandbox.path("legacy");
    write(
        &root,
        ".kittify/config.yaml",
        "activated_directives: []\ndeactivated_directives: [900-unknown]\n",
    );
    root
}

struct Run {
    status: u8,
    stdout: String,
    stderr: Vec<u8>,
}

fn run(sandbox: &Sandbox, root: &Path, args: &[&str]) -> Run {
    let mut command = Command::new(binary());
    command
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        // The pinned pack tree is a read-only input, exactly as in production.
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"));
    for (key, name) in sandbox.env_pairs() {
        command.env(key, sandbox.path(name));
    }
    let output = command.output().expect("run the native CLI");
    Run {
        status: output
            .status
            .code()
            .expect("the CLI exits with a code")
            .try_into()
            .expect("exit code fits u8"),
        stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        stderr: output.stderr,
    }
}

fn stray_line(path: &Path) -> String {
    format!(
        "Warning: Legacy activation keys beside .kittify/charter/activation.yaml are ignored; the \
         activation ledger (activation.yaml) wins: activated_directives, activated_tactics in {}. \
         Remove them, or run `spec-kitty doctor charter-packs`.\n",
        path.display()
    )
}

fn deny_line(path: &Path) -> String {
    format!(
        "Warning: deactivated_directives in {} is not applied: a deactivation is recorded only in \
         .kittify/charter/activation.yaml. Remove the key by hand, then run `spec-kitty upgrade` \
         (or `spec-kitty migrate activation-ledger`).\n",
        path.display()
    )
}

#[test]
fn charter_list_delivers_the_activation_warnings_on_stderr_only() {
    let sandbox = Sandbox::new();

    let stray = stray_project(&sandbox);
    let listed = run(&sandbox, &stray, &["charter", "list", "--json"]);
    assert_eq!(listed.status, 0, "charter list exits 0");
    assert_eq!(
        listed.stderr,
        stray_line(&stray.join(".kittify/config.yaml")).into_bytes(),
        "one warning line on the real stderr"
    );
    // JSON stays parseable, and the warning is not inside it.
    let payload: Value = serde_json::from_str(&listed.stdout).expect("stdout is one JSON document");
    assert!(
        !listed.stdout.contains("Warning:"),
        "warnings never on stdout"
    );
    assert!(payload.is_object() || payload.is_array());

    let legacy = legacy_project(&sandbox);
    let legacy_listed = run(&sandbox, &legacy, &["charter", "list", "--json"]);
    assert_eq!(legacy_listed.status, 0);
    assert_eq!(
        legacy_listed.stderr,
        deny_line(&legacy.join(".kittify/config.yaml")).into_bytes()
    );
    let payload: Value =
        serde_json::from_str(&legacy_listed.stdout).expect("legacy stdout is one JSON document");
    assert!(!legacy_listed.stdout.contains("Warning:"));
    assert!(payload.is_object() || payload.is_array());

    // The Source `report_stray=False` collector: the same project must not print.
    let status = run(&sandbox, &stray, &["charter", "status", "--json"]);
    assert!(
        !String::from_utf8_lossy(&status.stderr).contains("Warning:"),
        "charter status reads with report_stray=False: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let _: Value =
        serde_json::from_str(&status.stdout).expect("status stdout is one JSON document");
}
