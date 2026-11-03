//! Fail-closed, lock-exact development oracle selection (never production code).

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
            verify_python(&python)
        })
        .clone()
}

/// Verify an explicit interpreter override through the same required seam.
pub fn verify_python(python: &Path) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(
        python.is_file(),
        "required Python oracle is missing at {}; provision .venv-rust-oracle with uv sync --frozen --no-dev --python 3.11 (UV_PROJECT_ENVIRONMENT=.venv-rust-oracle)",
        python.display()
    );
    let result = Command::new(python)
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
    python.to_path_buf()
}
