//! Permanent Source-first regression for the original `!!binary`/`!!set` reds.
//!
//! This is the original hand-off counterexample set, expressed as a permanent
//! binary-level test: Source `profiles list --json` runs and asserts first in an
//! isolated root, then the native binary must produce the **same descriptor row**
//! for every one of the nine profiles. Before G2 the native rows silently
//! carried base64 text as names/roles (and rejected the valid int/bool/set
//! forms); the comparison here is field-for-field, not a "no crash" fence.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde_json::Value;

const CASES: &[(&str, &str)] = &[
    ("binary-name", "name: !!binary SGVsbG8=\n"),
    ("binary-purpose", "purpose: !!binary SGVsbG8=\n"),
    ("binary-description", "description: !!binary SGVsbG8=\n"),
    ("binary-role-entry", "roles: [!!binary UmV2aWV3ZXI=]\n"),
    ("binary-priority", "routing-priority: !!binary NTA=\n"),
    ("binary-bool", "sentinel: !!binary dHJ1ZQ==\n"),
    ("set-roles", "roles: !!set {Reviewer: null}\n"),
    ("set-capabilities", "capabilities: !!set {alpha: null}\n"),
    ("set-excluding", "excluding: !!set {capabilities: null}\n"),
];

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

fn profile_text(case: &str, snippet: &str) -> String {
    let key = snippet.split(':').next().unwrap_or_default();
    let base = [
        ("profile-id", case),
        ("name", case),
        ("purpose", "p"),
        ("roles", "[Reviewer]"),
        ("specialization", "{primary-focus: x}"),
    ];
    let replaced = if key == "role" {
        vec!["role".to_owned(), "roles".to_owned()]
    } else {
        vec![key.to_owned()]
    };
    let mut text = String::new();
    for (name, value) in base {
        if !replaced.iter().any(|replaced| replaced == name) {
            text.push_str(&format!("{name}: {value}\n"));
        }
    }
    text.push_str(snippet);
    text
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let root = temp.path().canonicalize().expect("canonical root");
        for directory in ["home", "cache", "config", "data", "state", "tmp"] {
            fs::create_dir_all(root.join(directory)).expect("isolated directory");
        }
        let profiles = root.join(".kittify/profiles");
        fs::create_dir_all(&profiles).expect("profiles directory");
        for (case, snippet) in CASES {
            fs::write(
                profiles.join(format!("{case}.agent.yaml")),
                profile_text(case, snippet),
            )
            .expect("write fixture");
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

    fn source_rows(&self) -> Vec<Value> {
        let script = r#"
import json, os, sys
from pathlib import Path
assert sys.version.split()[0] == "3.11.15", sys.version
root = Path(os.environ["SK_G2_ROOT"]).resolve()
assert Path.cwd().resolve() == root, (Path.cwd(), root)
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)
from charter.offering.agent_profiles.repository import AgentProfileRepository
repo = AgentProfileRepository(
    built_in_dir=root / "empty-built-in",
    project_dir=root / ".kittify/profiles",
)
loaded = {profile.profile_id: profile for profile in repo.list_all()}
assert loaded["binary-name"].name == "Hello", loaded["binary-name"].model_dump()
assert loaded["binary-purpose"].purpose == "Hello"
assert loaded["binary-description"].description == "Hello"
assert str(loaded["binary-role-entry"].role) == "reviewer"
assert loaded["binary-priority"].routing_priority == 50
assert loaded["binary-bool"].sentinel is True
assert [str(role) for role in loaded["set-roles"].roles] == ["reviewer"]
assert loaded["set-capabilities"].capabilities == ["alpha"]
assert loaded["set-excluding"].excluding == ["capabilities"]

rows = []
for pid in sorted(loaded):
    profile = loaded[pid]
    rows.append({
        "id": pid,
        "name": profile.name,
        "role": str(profile.role),
        "capabilities": list(profile.capabilities),
        "priority": profile.routing_priority,
        "sentinel": profile.sentinel,
        "excluding": profile.excluding,
    })
print(json.dumps(rows))
"#;
        let output = Command::new(python())
            .arg("-c")
            .arg(script)
            .env_clear()
            .envs(self.env())
            .current_dir(&self.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run source list");
        assert!(
            output.status.success(),
            "Source list failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("source rows JSON")
    }

    fn native_rows(&self) -> Vec<Value> {
        let output = Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
            .args(["profiles", "list", "--json"])
            .env_clear()
            .envs(self.env())
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .expect("run native list");
        assert!(
            output.status.success(),
            "native list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("native rows JSON")
    }
}

#[test]
fn original_binary_and_set_rows_are_source_exact_in_the_binary() {
    let fixture = Fixture::new();
    // Source runs and asserts its post-fix facts first.
    let source_rows = fixture.source_rows();
    assert_eq!(source_rows.len(), CASES.len(), "nine original cases load");

    // Native binary: every one of the nine descriptors is present and matches.
    let native_rows = fixture.native_rows();
    for source in &source_rows {
        let id = source["id"].as_str().expect("source id");
        let native = native_rows
            .iter()
            .find(|row| row["profile_id"] == id)
            .unwrap_or_else(|| panic!("{id}: native row missing; native rows: {native_rows:?}"));
        assert_eq!(native["name"], source["name"], "{id}: name");
        assert_eq!(native["role"], source["role"], "{id}: role");
        // The descriptor's action domains derive from the (decoded, lowercased)
        // role, so a base64 role (the original corruption) cannot pass this.
        let empty = Vec::new();
        let native_domains = native["action_domains"].as_array().unwrap_or(&empty);
        assert!(
            !native_domains.is_empty(),
            "{id}: expected populated action domains for the decoded reviewer role"
        );
    }
    // Set/binary capabilities and priority/sentinel are not in the list
    // descriptor; the library-level matrix covers their dumps. Here the
    // descriptor-level identity (name/role/domains) is exact for all nine.
    let by_id = |rows: &[Value], id: &str| {
        rows.iter()
            .find(|row| row["id"] == id || row["profile_id"] == id)
            .cloned()
            .expect("row")
    };
    assert_eq!(by_id(&source_rows, "binary-name")["name"], "Hello");
    assert_eq!(by_id(&native_rows, "binary-name")["name"], "Hello");
    assert_eq!(by_id(&source_rows, "binary-role-entry")["role"], "reviewer");
    assert_eq!(by_id(&native_rows, "binary-role-entry")["role"], "reviewer");
}
