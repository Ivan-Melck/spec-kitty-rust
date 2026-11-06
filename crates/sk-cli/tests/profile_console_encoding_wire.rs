//! Source-first conformance for the **human** `profiles list` console.
//!
//! The JSON view is the exact escaped wire; the human view is Rich's table. A
//! row identity holding a lone surrogate cannot be encoded to the UTF-8
//! terminal, and the reference does not substitute anything: the Rich table
//! write raises `UnicodeEncodeError`, so **no byte reaches stdout** and the
//! command exits nonzero. An identity with a PUA code point, an astral code
//! point or a NUL *is* encodable and appears raw.
//!
//! This test asserts the reference's behaviour first in an isolated root, then
//! requires the native console to match the observable contract: same exit
//! status, and either no stdout at all or the raw encoded identity — never a
//! `repr` substitution and never a replacement character.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

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

impl Fixture {
    fn new(case: &str, text: &str) -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let root = temp.path().canonicalize().expect("canonical root");
        for directory in ["home", "cache", "config", "data", "state", "tmp"] {
            fs::create_dir_all(root.join(directory)).expect("isolated directory");
        }
        fs::create_dir_all(root.join(".kittify/profiles")).expect("profiles directory");
        fs::write(
            root.join(".kittify/profiles")
                .join(format!("{case}.agent.yaml")),
            text,
        )
        .expect("write profile");
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
            ("TMPDIR".to_owned(), root.join("tmp").display().to_string()),
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
            ("SK_G2_ROOT".to_owned(), root.display().to_string()),
        ]
    }

    fn source(&self) -> std::process::Output {
        self.source_at(None)
    }

    fn source_at(&self, columns: Option<&str>) -> std::process::Output {
        let script = "import sys, os\nfrom pathlib import Path\nassert sys.version.split()[0] == '3.11.15', sys.version\nroot = Path(os.environ['SK_G2_ROOT']).resolve()\nassert Path.cwd().resolve() == root, (Path.cwd(), root)\nfrom specify_cli import main\nmain()\n";
        let mut command = Command::new(python());
        command
            .arg("-c")
            .arg(script)
            .arg("profiles")
            .arg("list")
            .env_clear()
            .envs(self.env());
        if let Some(columns) = columns {
            command.env("COLUMNS", columns);
        } else {
            command.env_remove("COLUMNS");
        }
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run source console")
    }

    fn native(&self) -> std::process::Output {
        self.native_at(None)
    }

    fn native_at(&self, columns: Option<&str>) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
        command
            .args(["profiles", "list"])
            .env_clear()
            .envs(self.env());
        if let Some(columns) = columns {
            command.env("COLUMNS", columns);
        } else {
            command.env_remove("COLUMNS");
        }
        command
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run native console")
    }
}

fn assert_source_contract(source: &std::process::Output, case: &str) {
    if case.starts_with("surrogate") {
        assert_ne!(source.status.code(), Some(0), "{case}: source must fail");
        assert!(
            source.stdout.is_empty(),
            "{case}: the Rich table writes no byte when the encode raises"
        );
        assert!(
            String::from_utf8_lossy(&source.stderr).contains("UnicodeEncodeError"),
            "{case}: the reference raises UnicodeEncodeError"
        );
    } else {
        assert_eq!(source.status.code(), Some(0), "{case}: source must succeed");
        assert!(!source.stdout.is_empty(), "{case}: source table is printed");
        assert!(
            !String::from_utf8_lossy(&source.stdout).contains('\u{fffd}'),
            "{case}: no replacement character"
        );
    }
}

#[test]
fn human_profile_console_matches_the_reference_encoding_contract() {
    let cases: [(&str, &str, Option<&str>); 5] = [
        (
            "surrogate-id",
            "profile-id: \"author\\ud800\"\nname: \"Name\"\npurpose: p\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n",
            None,
        ),
        (
            "surrogate-name",
            "profile-id: \"surrogate-name\"\nname: \"Name\\ud800\"\npurpose: p\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n",
            None,
        ),
        (
            "pua-id",
            "profile-id: \"author\\ue000\"\nname: \"PUA Name\"\npurpose: p\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n",
            Some("author\u{e000}"),
        ),
        (
            "astral-id",
            "profile-id: \"author\\U0001F600\"\nname: \"Astral Name\"\npurpose: p\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n",
            Some("author\u{1f600}"),
        ),
        (
            "nul-id",
            "profile-id: \"author\\u0000\"\nname: \"Nul Name\"\npurpose: p\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n",
            Some("author\0"),
        ),
    ];
    for (case, text, encodable) in cases {
        let fixture = Fixture::new(case, text);
        // The reference runs (and its contract is asserted) before native.
        let source = fixture.source();
        assert_source_contract(&source, case);
        let native = fixture.native();
        assert_eq!(
            native.status.code(),
            source.status.code(),
            "{case}: exit status must match the reference"
        );
        if encodable.is_none() {
            assert!(
                native.stdout.is_empty(),
                "{case}: a non-encodable identity must not print a partial/substituted table"
            );
            assert!(
                !native.stderr.is_empty(),
                "{case}: the refusal is reported on stderr"
            );
        } else {
            let identity = encodable.expect("checked");
            assert!(
                native
                    .stdout
                    .windows(identity.len())
                    .any(|window| window == identity.as_bytes()),
                "{case}: the raw encodable identity must appear in the table"
            );
        }
    }
}
