//! Fail-closed, lock-exact development oracle selection (never production code).
//!
//! Included by several integration-test binaries; not every binary uses every
//! helper, so the module allows dead code rather than forcing a per-test shim.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The repository root (`spec-kitty-rust/`).
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root resolves")
}

/// The verified Python development oracle.
///
/// Fails closed: a missing oracle or a lock-exact verification failure panics
/// instead of letting a comparison test pass with zero cases.
pub fn python() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let root = repository_root();
            let executable = if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            };
            let python = std::env::var_os("SK_DOCTRINE_PYTHON")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join(".venv-rust-oracle").join(executable));
            assert!(
                python.is_file(),
                "required Python oracle is missing at {}; provision .venv-rust-oracle with \
                 uv sync --frozen --no-dev --python 3.11 (UV_PROJECT_ENVIRONMENT=.venv-rust-oracle)",
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

/// The environment every oracle child needs (`PYTHONPATH=src`, pack root).
pub fn oracle_env(packs_root: &Path) -> Vec<(String, String)> {
    let root = repository_root();
    vec![
        (
            "PYTHONPATH".to_owned(),
            root.join("src").display().to_string(),
        ),
        (
            "SPEC_KITTY_PACKS_ROOT".to_owned(),
            packs_root.display().to_string(),
        ),
    ]
}
