//! CF65 test-only plumbing: the isolated Source child protocol for this binary.
//!
//! This mirrors `crates/sk-doctrine/tests/support/isolated.rs` (the E4 Source/Native
//! execution protocol) for a crate whose children include a real compiled binary:
//! every child runs with a cleared environment, a temporary CWD, HOME, all seven
//! XDG roots, TMP/TMPDIR/TEMP, the pinned absolute `PYTHONPATH`/packs root and
//! `PYTHONHASHSEED=0`, and the Source interpreter is selected and lock-exact
//! verified (never a bare `python3`).
//!
//! Not production code; nothing here is compiled into either binary.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The isolated roots every child asserts. `(env var, directory name)`.
pub const ROOTS: [(&str, &str); 9] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

/// The two remaining XDG roots the standing orders also require.
pub const EXTRA_XDG: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The pinned interpreter this protocol requires.
pub const PYTHON_VERSION: &str = "3.11.15";

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
/// instead of letting a Source comparison run against an unverified interpreter.
pub fn python() -> PathBuf {
    let root = repository_root();
    let executable = if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    };
    let python = std::env::var_os("SK_CF65_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".venv-rust-oracle").join(executable));
    assert!(
        python.is_file(),
        "required Python oracle is missing at {}",
        python.display()
    );
    let result = Command::new(&python)
        .arg(root.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(&root)
        .output()
        .expect("execute the required Python oracle verifier");
    assert!(
        result.status.success(),
        "Python oracle is not lock-exact: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    python
}

/// A fresh, unique, exclusive fixture directory (kept on disk).
pub fn fixture(case: &str) -> PathBuf {
    let base = repository_root().join("target/cf65-context");
    std::fs::create_dir_all(&base).expect("fixture base");
    let path = tempfile::Builder::new()
        .prefix(&format!("{case}-"))
        .tempdir_in(&base)
        .expect("fresh exclusive fixture directory")
        .keep();
    for (_, name) in ROOTS {
        std::fs::create_dir_all(path.join(name)).expect("isolated root");
    }
    for (_, name) in EXTRA_XDG {
        std::fs::create_dir_all(path.join(name)).expect("isolated root");
    }
    path
}

/// Assert the isolated roots this child was handed, and that nothing leaked.
pub fn assert_isolated(root: &Path) {
    let cwd = std::env::current_dir().expect("cwd");
    assert_eq!(cwd, root, "Native child CWD is the fixture");
    for (key, name) in ROOTS.into_iter().chain(EXTRA_XDG) {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    let repository = repository_root();
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(repository.join("src")),
        "PYTHONPATH is the absolute repository src"
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repository.join("packs")),
        "pack root is the absolute repository packs"
    );
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok("0"),
        "the hash seed is pinned"
    );
}

/// A child command with the isolated environment and `root` as its CWD.
pub fn isolated_command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let repository = repository_root();
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env("SK_CF65_SOURCE_ROOT", &repository)
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in ROOTS {
        command.env(key, root.join(name));
    }
    for (key, name) in EXTRA_XDG {
        command.env(key, root.join(name));
    }
    command
}
