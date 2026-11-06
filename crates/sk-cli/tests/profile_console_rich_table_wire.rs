//! Source-first byte equality for the **whole** human `profiles list` table.
//!
//! G2's console test asserts the encoding contract (a lone surrogate aborts with
//! no stdout); this test asserts the *layout*: column widths (including the
//! `no_wrap` profile-id column and the `ellipsis` overflow of the others), the
//! centered title, the `HEAVY_HEAD` box characters, word wrap, left
//! justification, and the terminal-cell measure for CJK (two cells), combining
//! marks (zero cells), astral and private-use code points.
//!
//! The reference renders first at each width and its stdout **is** the
//! expectation; one differing byte fails the case.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn python() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let root = repository_root();
            let python = std::env::var_os("SK_DOCTRINE_PYTHON")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join(".venv-rust-oracle/bin/python"));
            assert!(python.is_file(), "required Python oracle is missing");
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

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let root = temp.path().canonicalize().expect("canonical root");
        for directory in ["home", "cache", "config", "data", "state", "runtime", "tmp"] {
            fs::create_dir_all(root.join(directory)).expect("isolated directory");
        }
        let profiles = root.join(".kittify/profiles");
        fs::create_dir_all(&profiles).expect("profiles directory");
        let cases: &[(&str, &str)] = &[
            (
                "long-identity.agent.yaml",
                "profile-id: \"an-extremely-long-profile-identifier-for-folding\"\nname: \"Extremely Long Friendly Name Here\"\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\n",
            ),
            (
                "cjk.agent.yaml",
                "profile-id: \"cjk-agent\"\nname: \"中文 字\"\npurpose: p\nroles: [architect]\nspecialization: {primary-focus: x}\n",
            ),
            (
                "combining.agent.yaml",
                "profile-id: \"combining-agent\"\nname: \"Nai\u{308}ve Cafe\u{301} Name\"\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\n",
            ),
            (
                "astral.agent.yaml",
                "profile-id: \"astral-agent\"\nname: \"\u{1f600} Emoji Name\"\npurpose: p\nroles: [implementer]\nspecialization: {primary-focus: x}\n",
            ),
            (
                "pua.agent.yaml",
                "profile-id: \"pua-agent\"\nname: \"\u{e000} Private Use\"\npurpose: p\nroles: [curator]\nspecialization: {primary-focus: x}\n",
            ),
            (
                "wide-role.agent.yaml",
                "profile-id: \"role-agent\"\nname: \"Role Name\"\npurpose: p\nroles: [architecture-scout]\nspecialization: {primary-focus: x}\n",
            ),
        ];
        for (name, text) in cases {
            fs::write(profiles.join(name), text).expect("write profile");
        }
        Self { _temp: temp, root }
    }

    fn env(&self) -> Vec<(String, String)> {
        let root = &self.root;
        let repo = repository_root();
        vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("HOME".to_owned(), root.join("home").display().to_string()),
            (
                "XDG_CACHE_HOME".to_owned(),
                root.join("cache").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                root.join("config").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".to_owned(),
                root.join("data").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_owned(),
                root.join("state").display().to_string(),
            ),
            (
                "XDG_RUNTIME_DIR".to_owned(),
                root.join("runtime").display().to_string(),
            ),
            ("TMPDIR".to_owned(), root.join("tmp").display().to_string()),
            ("TMP".to_owned(), root.join("tmp").display().to_string()),
            ("TEMP".to_owned(), root.join("tmp").display().to_string()),
            ("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()),
            ("PYTHONHASHSEED".to_owned(), "0".to_owned()),
            (
                "PYTHONPATH".to_owned(),
                repo.join("src").display().to_string(),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".to_owned(),
                repo.join("packs").display().to_string(),
            ),
            ("NO_COLOR".to_owned(), "1".to_owned()),
            ("SK_G3_ROOT".to_owned(), root.display().to_string()),
        ]
    }

    fn source(&self, columns: Option<&str>) -> std::process::Output {
        let script = r#"import sys, os
from pathlib import Path
assert sys.version.split()[0] == '3.11.15', sys.version
root = Path(os.environ['SK_G3_ROOT']).resolve()
assert Path.cwd().resolve() == root, (Path.cwd(), root)
for key, child in [('HOME', 'home'), ('XDG_CACHE_HOME', 'cache'),
                   ('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                   ('XDG_STATE_HOME', 'state'), ('XDG_RUNTIME_DIR', 'runtime'),
                   ('TMPDIR', 'tmp'), ('TMP', 'tmp'), ('TEMP', 'tmp')]:
    assert Path(os.environ[key]).resolve() == root / child, (key, os.environ.get(key))
from specify_cli import main
main()
"#;
        let mut command = Command::new(python());
        command
            .arg("-c")
            .arg(script)
            .arg("profiles")
            .arg("list")
            .env_clear()
            .envs(self.env());
        match columns {
            Some(columns) => command.env("COLUMNS", columns),
            None => command.env_remove("COLUMNS"),
        };
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run source console")
    }

    fn native(&self, columns: Option<&str>) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
        command
            .args(["profiles", "list"])
            .env_clear()
            .envs(self.env());
        match columns {
            Some(columns) => command.env("COLUMNS", columns),
            None => command.env_remove("COLUMNS"),
        };
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run native console")
    }
}

#[test]
fn human_profile_table_bytes_match_the_reference_at_every_width() {
    let fixture = Fixture::new();
    for columns in [Some("200"), Some("120"), Some("80"), Some("60"), None] {
        // Source first: its rendering is the expectation.
        let source = fixture.source(columns);
        assert_eq!(
            source.status.code(),
            Some(0),
            "source failed at {columns:?}: {}",
            String::from_utf8_lossy(&source.stderr)
        );
        assert!(!source.stdout.is_empty(), "source printed no table");
        let native = fixture.native(columns);
        assert_eq!(
            native.status.code(),
            Some(0),
            "native failed at {columns:?}: {}",
            String::from_utf8_lossy(&native.stderr)
        );
        if native.stdout != source.stdout {
            let source_text = String::from_utf8_lossy(&source.stdout);
            let native_text = String::from_utf8_lossy(&native.stdout);
            let first = source_text
                .lines()
                .zip(native_text.lines())
                .position(|(left, right)| left != right);
            panic!(
                "the human table must be byte-identical at {columns:?}; first differing line {first:?}\n\
                 reference:\n{source_text}\nnative:\n{native_text}"
            );
        }
    }
}
