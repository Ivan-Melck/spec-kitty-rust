//! Source-first conformance for the profile **load phase** and the process secret.
//!
//! Three behaviours of the reference that change the externally observable
//! result of `profiles list`:
//!
//! 1. **Constructor failures abort the whole load.** The reference's repository
//!    catches `(YAMLError, OSError)` per file, so an unknown tag or a bad node
//!    kind skips that one file — but `!!bool maybe` raises a bare `KeyError`, a
//!    bad `!!int`/`!!float`/impossible `!!timestamp` a `ValueError`, an
//!    unhashable `!!set` key a `TypeError`, a duplicate `!!omap` key an
//!    `AssertionError`, and a non-mapping document an `AttributeError`. None of
//!    those is caught: the command exits nonzero and prints **no** table, with
//!    no partial success.
//! 2. **An invalid `PYTHONHASHSEED` is fatal before `main`.** Exit 1, no stdout,
//!    and the exact two-line fatal message on stderr.
//! 3. **Unseeded is genuinely nondeterministic.** With no `PYTHONHASHSEED` the
//!    reference draws the SipHash keys from the OS per process, so the `!!set`
//!    order is not seed 0. Native must draw its own process secret, not fall
//!    back to the all-zero seed.
//!
//! Every case runs the reference first under an asserted temporary CWD + HOME +
//! all XDG/TMP roots, then requires the native binary to match the observable
//! contract.

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
        Self { _temp: temp, root }
    }

    fn write_profile(&self, name: &str, text: &str) {
        let directory = self.root.join(".kittify/profiles");
        fs::create_dir_all(&directory).expect("profiles directory");
        fs::write(directory.join(name), text).expect("write profile");
    }

    fn env(&self, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        let root = &self.root;
        let repo = repository_root();
        let mut env = vec![
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
            (
                "PYTHONPATH".to_owned(),
                repo.join("src").display().to_string(),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".to_owned(),
                repo.join("packs").display().to_string(),
            ),
            ("NO_COLOR".to_owned(), "1".to_owned()),
            ("SK_G3_LOAD_ROOT".to_owned(), root.display().to_string()),
        ];
        env.extend(
            extra
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        env
    }

    fn source(&self, args: &[&str], extra: &[(&str, &str)]) -> std::process::Output {
        let script = r#"import sys, os
from pathlib import Path
assert sys.version.split()[0] == '3.11.15', sys.version
root = Path(os.environ['SK_G3_LOAD_ROOT']).resolve()
assert Path.cwd().resolve() == root, (Path.cwd(), root)
for key, child in [('HOME', 'home'), ('XDG_CACHE_HOME', 'cache'),
                   ('XDG_CONFIG_HOME', 'config'), ('XDG_DATA_HOME', 'data'),
                   ('XDG_STATE_HOME', 'state'), ('XDG_RUNTIME_DIR', 'runtime'),
                   ('TMPDIR', 'tmp'), ('TMP', 'tmp'), ('TEMP', 'tmp')]:
    assert Path(os.environ[key]).resolve() == root / child, (key, os.environ.get(key))
from specify_cli import main
main()
"#;
        Command::new(python())
            .arg("-c")
            .arg(script)
            .args(args)
            .env_clear()
            .envs(self.env(extra))
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run source command")
    }

    fn native(&self, args: &[&str], extra: &[(&str, &str)]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
            .args(args)
            .env_clear()
            .envs(self.env(extra))
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run native command")
    }
}

const VALID: &str = "profile-id: {id}\nname: {id}\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\n";

fn valid(id: &str) -> String {
    VALID.replace("{id}", id)
}

/// The reference's uncaught constructor exception per authored body.
fn uncaught_cases() -> Vec<(&'static str, String)> {
    vec![
        (
            "bad-bool",
            format!("{}sentinel: !!bool maybe\n", valid("bad-bool")),
        ),
        (
            "bad-int",
            format!("{}capabilities: [!!int abc]\n", valid("bad-int")),
        ),
        (
            "bad-float",
            format!("{}capabilities: [!!float abc]\n", valid("bad-float")),
        ),
        (
            "bad-timestamp",
            format!(
                "{}capabilities: [!!timestamp 2020-13-01]\n",
                valid("bad-timestamp")
            ),
        ),
        (
            "unhashable-set",
            format!(
                "profile-id: unhashable-set\nname: n\npurpose: p\nspecialization: {{primary-focus: x}}\nroles: !!set {{? [a, b] : null}}\n"
            ),
        ),
        (
            "duplicate-omap",
            format!(
                "{}capabilities: !!omap [{{a: b}}, {{a: c}}]\n",
                valid("duplicate-omap")
            ),
        ),
        ("non-mapping-document", "- a list\n".to_owned()),
    ]
}

#[test]
fn uncaught_constructor_failures_abort_the_whole_load() {
    for (case, body) in uncaught_cases() {
        let fixture = Fixture::new();
        // A valid sibling first, so a "graceful skip" would still print a table.
        fixture.write_profile("zzz-sibling.agent.yaml", &valid("sibling"));
        fixture.write_profile("mmm-bad.agent.yaml", &body);
        // The reference runs first; its contract is the expectation. Plain
        // (non-`--json`) output is used because the reference's abort writes a
        // traceback to stderr and nothing to stdout in both modes.
        let source = fixture.source(&["profiles", "list"], &[]);
        assert_ne!(
            source.status.code(),
            Some(0),
            "{case}: the reference must abort the load: {}",
            String::from_utf8_lossy(&source.stderr)
        );
        assert!(
            source.stdout.is_empty(),
            "{case}: the reference prints no list when the load aborts"
        );
        let expected_class = match case {
            "bad-bool" => "KeyError:",
            "bad-int" | "bad-float" | "bad-timestamp" => "ValueError:",
            "unhashable-set" => "TypeError:",
            "duplicate-omap" => "AssertionError",
            "non-mapping-document" => "AttributeError:",
            _ => unreachable!("declared constructor case"),
        };
        assert!(
            String::from_utf8_lossy(&source.stderr).contains(expected_class),
            "{case}: Source must fail for the intended cause, not a broken fixture: {}",
            String::from_utf8_lossy(&source.stderr)
        );
        let native = fixture.native(&["profiles", "list"], &[]);
        assert_eq!(
            native.status.code(),
            source.status.code(),
            "{case}: exit status must match the reference (stderr: {})",
            String::from_utf8_lossy(&native.stderr)
        );
        assert!(
            native.stdout.is_empty(),
            "{case}: a partial success is not the reference's behaviour: {}",
            String::from_utf8_lossy(&native.stdout)
        );
        assert!(
            !native.stderr.is_empty(),
            "{case}: the refusal is reported on stderr"
        );
    }
}

#[test]
fn an_unknown_tag_still_skips_only_that_file() {
    // The contrast case: a `ConstructorError` is a `YAMLError`, so the reference
    // skips the file and still lists its siblings. Native must not turn this
    // into a whole-load abort.
    let fixture = Fixture::new();
    fixture.write_profile("zzz-sibling.agent.yaml", &valid("sibling"));
    fixture.write_profile(
        "mmm-bad.agent.yaml",
        &format!("{}capabilities: [!custom x]\n", valid("bad-tag")),
    );
    // The contrast case: the source skips that file and still lists its siblings in JSON.
    let source = fixture.source(&["profiles", "list", "--json"], &[]);
    assert_eq!(source.status.code(), Some(0));
    let source_rows: serde_json::Value =
        serde_json::from_slice(&source.stdout).expect("Source JSON");
    let source_rows = source_rows.as_array().expect("Source descriptor rows");
    assert!(
        source_rows.iter().any(|row| row["profile_id"] == "sibling"),
        "Source must retain the valid sibling"
    );
    assert!(!source_rows.iter().any(|row| row["profile_id"] == "bad-tag"));
    let native = fixture.native(&["profiles", "list", "--json"], &[]);
    assert_eq!(native.status.code(), Some(0));
    let text = String::from_utf8_lossy(&native.stdout);
    assert!(text.contains("sibling"), "{text}");
    assert!(!text.contains("bad-tag"), "{text}");
}

#[test]
fn invalid_hash_seed_is_the_reference_fatal_startup_error() {
    for seed in ["abc", "-1", "4294967296"] {
        let fixture = Fixture::new();
        fixture.write_profile("aaa.agent.yaml", &valid("aaa"));
        // Source first: the interpreter refuses to start at all.
        let source = fixture.source(&["profiles", "list"], &[("PYTHONHASHSEED", seed)]);
        assert_eq!(
            source.status.code(),
            Some(1),
            "{seed}: the reference exits 1"
        );
        assert!(source.stdout.is_empty(), "{seed}: no stdout");
        let expected = String::from_utf8_lossy(&source.stderr).to_string();
        assert!(
            expected.contains("config_init_hash_seed"),
            "{seed}: unexpected reference stderr: {expected}"
        );
        let native = fixture.native(&["profiles", "list"], &[("PYTHONHASHSEED", seed)]);
        assert_eq!(native.status.code(), source.status.code(), "{seed}: exit");
        assert!(native.stdout.is_empty(), "{seed}: no stdout");
        assert_eq!(
            String::from_utf8_lossy(&native.stderr),
            expected,
            "{seed}: the fatal message must be byte-identical"
        );
    }
}

#[test]
fn unseeded_set_order_is_not_the_zero_seed() {
    // A `!!set` of four roles has 24 possible Python orders. Seed 0 is one of
    // them; an unseeded process draws its own secret, so requiring at least one
    // of six runs to differ from seed 0 is a real test of "process-owned secret"
    // (the chance of a false failure is about 24^-5).
    let fixture = Fixture::new();
    fixture.write_profile(
        "set-roles.agent.yaml",
        "profile-id: set-roles\nname: Set Roles\npurpose: p\nspecialization: {primary-focus: x}\nroles: !!set {? alpha : null, ? beta : null, ? gamma : null, ? delta : null}\n",
    );
    // The reference at seed 0, first.
    let source_seed0 = fixture.source(&["profiles", "list", "--json"], &[("PYTHONHASHSEED", "0")]);
    assert_eq!(source_seed0.status.code(), Some(0));
    let source_report: serde_json::Value =
        serde_json::from_slice(&source_seed0.stdout).expect("source JSON");
    let source_role = first_role_of(&source_report, "set-roles");
    let seeded = fixture.native(&["profiles", "list", "--json"], &[("PYTHONHASHSEED", "0")]);
    assert_eq!(seeded.status.code(), Some(0));
    let seeded_report: serde_json::Value =
        serde_json::from_slice(&seeded.stdout).expect("native JSON");
    assert_eq!(
        first_role_of(&seeded_report, "set-roles"),
        source_role,
        "seed 0 must stay exactly the reference's order"
    );

    let mut differed = false;
    for _ in 0..8 {
        let unseeded = fixture.native(&["profiles", "list", "--json"], &[]);
        assert_eq!(
            unseeded.status.code(),
            Some(0),
            "an unseeded native run must succeed: {}",
            String::from_utf8_lossy(&unseeded.stderr)
        );
        let report: serde_json::Value =
            serde_json::from_slice(&unseeded.stdout).expect("native JSON");
        let role = first_role_of(&report, "set-roles");
        assert!(
            ["alpha", "beta", "gamma", "delta"].contains(&role.as_str()),
            "the primary role is one of the authored members: {role}"
        );
        if role != source_role {
            differed = true;
        }
    }
    assert!(
        differed,
        "an unseeded native run must draw a process secret, not the all-zero seed"
    );
}

/// The primary `role` of one profile descriptor in `profiles list --json`.
fn first_role_of(report: &serde_json::Value, profile_id: &str) -> String {
    let rows = report.as_array().expect("descriptor array");
    let row = rows
        .iter()
        .find(|row| row["profile_id"] == serde_json::json!(profile_id))
        .unwrap_or_else(|| panic!("{profile_id} is not listed: {report}"));
    row["role"]
        .as_str()
        .expect("the descriptor's primary role")
        .to_owned()
}
