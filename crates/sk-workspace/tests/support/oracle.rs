//! Fail-closed, lock-exact development-oracle selection (never production code).
//!
//! Required comparison tests must never turn an unavailable reference into a
//! passing zero-comparison result: a missing or drifted oracle panics here,
//! exactly like the kernel's own support module.

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

/// Run a Python snippet against the pinned source tree and return trimmed stdout.
pub fn run(script: &str, args: &[&str]) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(python())
        .arg("-c")
        .arg(script)
        .args(args)
        .current_dir(&root)
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
