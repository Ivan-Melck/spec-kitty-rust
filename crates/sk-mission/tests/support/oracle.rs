#![allow(dead_code)]
//! Fail-closed, lock-exact development-oracle selection (never production code).
//!
//! Required comparison tests must never turn an unavailable reference into a
//! passing zero-comparison result: a missing or drifted oracle panics here,
//! exactly like the kernel's and workspace's own support modules.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub fn python() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let executable = if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            };
            let python = std::env::var_os("SK_KERNEL_PYTHON")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join(".venv-rust-oracle").join(executable));
            assert!(
                python.is_file(),
                "required Python oracle is missing at {}; provision .venv-rust-oracle with uv sync --frozen --no-dev --python 3.11 (UV_PROJECT_ENVIRONMENT=.venv-rust-oracle)",
                python.display()
            );
            let result = Command::new(&python)
                .arg(root.join("scripts/rust/verify_oracle.py"))
                .arg("--root")
                .arg(&root)
                .output()
                .expect("execute required Python oracle verifier");
            assert!(
                result.status.success(),
                "Python oracle is not lock-exact: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            python
        })
        .clone()
}

/// Run a Python snippet in an isolated temporary working directory and return
/// its trimmed stdout.
///
/// The child gets its own CWD, `HOME`, every XDG root and `TMPDIR`, so no probe
/// can read or write the operator's real state. `PYTHONPATH` and the pack root
/// point at the pinned source tree.
pub fn run(script: &str, args: &[&str]) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sandbox = tempfile::tempdir().expect("oracle sandbox");
    for directory in ["home", "cache", "config", "data", "state", "tmp", "cwd"] {
        std::fs::create_dir_all(sandbox.path().join(directory)).expect("sandbox root");
    }
    let output = Command::new(python())
        .arg("-c")
        .arg(script)
        .args(args)
        .current_dir(sandbox.path().join("cwd"))
        .env("HOME", sandbox.path().join("home"))
        .env("XDG_CACHE_HOME", sandbox.path().join("cache"))
        .env("XDG_CONFIG_HOME", sandbox.path().join("config"))
        .env("XDG_DATA_HOME", sandbox.path().join("data"))
        .env("XDG_STATE_HOME", sandbox.path().join("state"))
        .env("TMPDIR", sandbox.path().join("tmp"))
        .env("PYTHONPATH", root.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", root.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("run the pinned Python oracle");
    assert!(
        output.status.success(),
        "oracle script failed: {}\nstdout: {}\nstderr: {}",
        script,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Decode the JSON document an oracle snippet printed.
pub fn run_json(script: &str, args: &[&str]) -> serde_json::Value {
    let text = run(script, args);
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("oracle output was not JSON ({error}): {text}"))
}
