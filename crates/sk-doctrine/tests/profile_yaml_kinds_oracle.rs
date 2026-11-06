//! Source-first conformance for constructed YAML kinds and implicit resolution.
//!
//! The reference loads a profile with `ruamel.yaml`'s **safe** constructor, so
//! the value Pydantic validates is a Python object graph, not YAML text:
//! `!!binary SGVsbG8=` is `bytes` (then Pydantic's lax `str` makes `Hello`),
//! `!!set {Reviewer: null}` is a `set` (then `list[str]`), `!!omap` is an
//! ordered dict, `!!pairs` is a list of tuples, `!!timestamp` is a `date`, and a
//! *plain* `017`/`2020-01-01` is an `int`/`date` before the model ever sees it.
//!
//! This matrix runs the pinned reference **first** in an asserted temporary
//! CWD/HOME/every XDG/TMPDIR, asserts the post-fix business facts (the seven
//! original `!!binary`/`!!set` counterexamples included), and only then loads
//! the same bytes with the native canonical loader in an isolated child process
//! (`PYTHONHASHSEED=0`, because a Python `set`'s iteration order is a function
//! of the hash secret). Loaded ids, every `model_dump(by_alias=True)` field and
//! the skipped declared ids must match field for field.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const BASE: &str = "profile-id: {id}\nname: {id}\npurpose: p\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n";

/// (case id, extra/replacement lines). A line whose key already exists in BASE
/// replaces it; otherwise it is appended.
const CASES: &[(&str, &str)] = &[
    // --- the seven original Source-valid counterexamples -------------------
    ("binary-name", "name: !!binary SGVsbG8=\n"),
    ("binary-role-entry", "roles: [!!binary UmV2aWV3ZXI=]\n"),
    ("binary-role-scalar", "role: !!binary UmV2aWV3ZXI=\n"),
    ("binary-priority", "routing-priority: !!binary NTA=\n"),
    ("binary-bool", "sentinel: !!binary dHJ1ZQ==\n"),
    ("set-roles", "roles: !!set {Reviewer: null}\n"),
    ("set-capabilities", "capabilities: !!set {alpha: null}\n"),
    ("set-excluding", "excluding: !!set {capabilities: null}\n"),
    // --- more constructed bytes -------------------------------------------
    ("binary-purpose", "purpose: !!binary SGVsbG8=\n"),
    ("binary-description", "description: !!binary SGVsbG8=\n"),
    ("binary-name-empty", "name: !!binary \"\"\n"),
    ("binary-name-nul", "name: !!binary AEFC\n"),
    ("binary-block", "name: !!binary |\n  SGVs\n  bG8=\n"),
    ("binary-bad-base64", "name: !!binary '!!!'\n"),
    ("binary-greek-role", "roles: [!!binary zqPOn86mzp/Oow==]\n"),
    (
        "binary-nested-specialization",
        "specialization: {primary-focus: !!binary eA==}\n",
    ),
    (
        "binary-nested-mode-default",
        "mode-defaults: [{mode: !!binary bQ==, description: d, use-case: u}]\n",
    ),
    ("binary-capabilities", "capabilities: [!!binary YWxwaGE=]\n"),
    ("binary-name-utf8-error", "name: !!binary /w==\n"),
    ("binary-nonascii", "name: !!binary 'SGVsbG8\u{e9}'\n"),
    ("binary-bool-no", "sentinel: !!binary bm8=\n"),
    ("b64-canonical", "name: !!binary SGVsbG8=\n"),
    ("b64-padded-extra", "name: !!binary SGVsbA===\n"),
    ("b64-trailing-junk", "name: !!binary SGVsbG8=extra\n"),
    ("b64-unpadded-error", "name: !!binary SGVsbG8\n"),
    // --- constructed sets (Python hash order, seeded in both processes) ----
    (
        "set-roles-two",
        "roles: !!set {Reviewer: null, Implementer: null}\n",
    ),
    (
        "set-roles-four",
        "roles: !!set {Reviewer: null, Implementer: null, Architect: null, Planner: null}\n",
    ),
    (
        "set-capabilities-two",
        "capabilities: !!set {alpha: null, beta: null}\n",
    ),
    (
        "set-excluding-two",
        "excluding: !!set {capabilities: null, name: null}\n",
    ),
    (
        "set-roles-binary",
        "roles: !!set {!!binary UmV2aWV3ZXI=: null}\n",
    ),
    ("set-yes-key", "capabilities: !!set {yes: null}\n"),
    ("set-roles-empty", "roles: !!set {}\n"),
    (
        "set-dup-key",
        "capabilities: !!set {alpha: null, alpha: null}\n",
    ),
    ("set-no-seq", "capabilities: !!set [alpha]\n"),
    // --- ordered map / pairs / timestamps ---------------------------------
    (
        "omap-specialization",
        "specialization: !!omap\n  - primary-focus: x\n",
    ),
    (
        "omap-excluding",
        "excluding: !!omap\n  - capabilities: [alpha]\n",
    ),
    ("pairs-name", "name: !!pairs\n  - a: b\n"),
    ("pairs-mode-defaults", "mode-defaults: !!pairs\n  - a: b\n"),
    ("timestamp-name", "name: !!timestamp 2020-01-01\n"),
    (
        "timestamp-name-datetime",
        "name: !!timestamp 2020-01-01T01:02:03Z\n",
    ),
    ("timestamp-bad", "name: !!timestamp not-a-date\n"),
    // `!!timestamp 2020-13-01` (and `!!bool maybe`, `!!int abc`) make the
    // reference raise an *uncaught* ValueError/KeyError, aborting the whole
    // repository load. They are not Source-valid inputs; the native loader
    // refuses just that file (unit-tested), it does not abort the catalogue.
    // --- explicit core tags ----------------------------------------------
    ("str-tag-priority", "routing-priority: !!str \"50\"\n"),
    ("int-tag-priority", "routing-priority: !!int \"0x10\"\n"),
    ("float-tag-priority", "routing-priority: !!float 5.0e1\n"),
    ("bool-tag-sentinel", "sentinel: !!bool \"on\"\n"),
    ("null-tag-name", "name: !!null x\n"),
    ("seq-tag-roles", "roles: !!seq [Reviewer]\n"),
    (
        "map-tag-specialization",
        "specialization: !!map {primary-focus: x}\n",
    ),
    ("str-tag-number", "name: !!str 7\n"),
    ("unknown-tag-name", "name: !custom hello\n"),
    ("unknown-tag-seq", "roles: [!custom Reviewer]\n"),
    ("verbatim-core-tag", "name: !<tag:yaml.org,2002:int> 7\n"),
    (
        "yaml-org-tag-name",
        "name: !<tag:yaml.org,2002:timestamp> 2020-01-01\n",
    ),
    // --- implicit resolver parity ----------------------------------------
    ("plain-date-name", "name: 2020-01-01\n"),
    ("plain-datetime-name", "name: 2020-01-01 10:00:00\n"),
    ("plain-short-date", "name: 2020-1-1\n"),
    ("plain-leadingzero", "name: 017\n"),
    ("plain-underscore", "name: 1_000\n"),
    ("plain-sexagesimal", "name: 1:30\n"),
    ("plain-yes", "name: yes\n"),
    ("plain-on", "name: on\n"),
    ("plain-quoted-date", "name: \"2020-01-01\"\n"),
    ("plain-v1", "name: v1\n"),
    ("plain-dot5", "name: .5\n"),
    ("plain-5dot", "name: 5.\n"),
    ("plain-exp", "name: 1e5\n"),
    ("plain-inf", "name: .inf\n"),
    ("plain-neg-inf", "name: -.Inf\n"),
    ("plain-nan", "name: .NaN\n"),
    ("plain-neg-nan", "name: -.nan\n"),
    ("plain-exponent-no-sign", "name: 1.5e5\n"),
    ("plain-priority-hex", "routing-priority: 0x10\n"),
    ("plain-priority-oct", "routing-priority: 0o17\n"),
    ("plain-priority-leadingzero", "routing-priority: 017\n"),
    ("plain-priority-exp", "routing-priority: 1e5\n"),
    ("plain-priority-dot5", "routing-priority: 5.5\n"),
    ("plain-sentinel-yes", "sentinel: yes\n"),
    ("plain-sentinel-on", "sentinel: on\n"),
    // --- anchors and merge keys around constructed values -----------------
    (
        "anchor-binary",
        "description: &b !!binary SGVsbG8=\nname: *b\n",
    ),
    (
        "merge-binary-default",
        "base: &spec {primary-focus: x}\nspecialization: {<<: *spec}\n",
    ),
];

fn profile_text(case: &str, snippet: &str) -> String {
    // Every key the snippet authors replaces the base line for that key; the
    // deprecated scalar `role:` replaces the canonical `roles:` list.
    let mut replaced: Vec<String> = snippet
        .lines()
        .filter_map(|line| line.split_once(':').map(|(key, _)| key.trim().to_owned()))
        .filter(|key| !key.is_empty())
        .collect();
    if replaced.iter().any(|key| key == "role") {
        replaced.push("roles".to_owned());
    }
    let mut text = String::new();
    for line in BASE.lines() {
        let key = line
            .split_once(':')
            .map(|(key, _)| key.trim())
            .unwrap_or_default();
        if !replaced.iter().any(|replaced| replaced == key) {
            text.push_str(&line.replace("{id}", case));
            text.push('\n');
        }
    }
    text.push_str(&snippet.replace("{id}", case));
    text
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    built_in: PathBuf,
    project: PathBuf,
    seed: String,
}

impl Fixture {
    fn new(seed: &str) -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let root = temp.path().canonicalize().expect("canonical root");
        let built_in = root.join("empty-built-in");
        let project = root.join("profiles");
        fs::create_dir_all(&project).expect("profiles directory");
        for directory in ["home", "cache", "config", "data", "state", "tmp"] {
            fs::create_dir_all(root.join(directory)).expect("isolated directory");
        }
        let fixture = Self {
            _temp: temp,
            root,
            built_in,
            project,
            seed: seed.to_owned(),
        };
        for (case, snippet) in CASES {
            let text = profile_text(case, snippet);
            fs::write(fixture.project.join(format!("{case}.agent.yaml")), text)
                .expect("write fixture");
        }
        fixture
    }

    fn env(&self) -> Vec<(String, String)> {
        let root = &self.root;
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
            // A `set`'s order is the CPython hash table's; both sides pin the
            // same seed so the comparison has one observable answer.
            ("PYTHONHASHSEED".to_owned(), self.seed.clone()),
            (
                "PYTHONPATH".to_owned(),
                support::repository_root().join("src").display().to_string(),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".to_owned(),
                support::repository_root()
                    .join("packs")
                    .display()
                    .to_string(),
            ),
            ("SK_G2_ROOT".to_owned(), root.display().to_string()),
            (
                "SK_G2_BUILT_IN".to_owned(),
                self.built_in.display().to_string(),
            ),
            (
                "SK_G2_PROJECT".to_owned(),
                self.project.display().to_string(),
            ),
            ("SK_G2_SEED".to_owned(), self.seed.clone()),
        ]
    }
}

// ---------------------------------------------------------------------------
// The reference, first
// ---------------------------------------------------------------------------

const SOURCE_SCRIPT: &str = r#"
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
    built_in_dir=Path(os.environ["SK_G2_BUILT_IN"]),
    project_dir=Path(os.environ["SK_G2_PROJECT"]),
)
loaded = {profile.profile_id: profile for profile in repo.list_all()}
skips = [entry.profile_id for entry in repo.skipped_profiles()]

# The seven original Source-valid counterexamples, asserted before native runs.
assert loaded["binary-name"].name == "Hello", loaded["binary-name"].model_dump()
assert loaded["binary-role-entry"].role == "reviewer"
assert loaded["binary-role-scalar"].role == "reviewer"
assert loaded["binary-priority"].routing_priority == 50
assert loaded["binary-bool"].sentinel is True
assert [str(role) for role in loaded["set-roles"].roles] == ["reviewer"]
assert loaded["set-capabilities"].capabilities == ["alpha"]
assert loaded["set-excluding"].excluding == ["capabilities"]
assert loaded["binary-purpose"].purpose == "Hello"
assert loaded["binary-description"].description == "Hello"
assert {str(role) for role in loaded["set-roles-two"].roles} == {"implementer", "reviewer"}
assert loaded["omap-specialization"].specialization.primary_focus == "x"
assert loaded["str-tag-priority"].routing_priority == 50
assert loaded["int-tag-priority"].routing_priority == 16
assert loaded["bool-tag-sentinel"].sentinel is True
assert loaded["plain-quoted-date"].name == "2020-01-01"
assert loaded["plain-sexagesimal"].name == "1:30"
assert loaded["plain-priority-hex"].routing_priority == 16
assert "plain-date-name" not in loaded
assert "plain-leadingzero" not in loaded
assert "unknown-tag-name" not in loaded
assert "set-roles-empty" not in loaded
assert "binary-name-utf8-error" not in loaded

def dump(profile):
    return profile.model_dump(by_alias=True)

out = {
    "ids": sorted(loaded),
    "skipped": skips,
    "dumps": {pid: dump(profile) for pid, profile in loaded.items()},
}
print(json.dumps(out, sort_keys=True))
"#;

fn run_source(fixture: &Fixture) -> Value {
    let output = Command::new(support::python())
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .env_clear()
        .envs(fixture.env())
        .current_dir(&fixture.root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run source oracle");
    assert!(
        output.status.success(),
        "Source oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("source oracle JSON")
}

// ---------------------------------------------------------------------------
// Native, in an isolated child with the same hash seed
// ---------------------------------------------------------------------------

fn run_native(fixture: &Fixture) -> Value {
    let output = Command::new(std::env::current_exe().expect("native test executable"))
        .args([
            "--exact",
            "native_yaml_kinds_helper",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .envs(fixture.env())
        .current_dir(&fixture.root)
        .output()
        .expect("run isolated native loader");
    assert!(
        output.status.success(),
        "native loader failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| line.starts_with('{'))
        .expect("native loader JSON line");
    serde_json::from_str(line).expect("native loader JSON")
}

#[test]
#[ignore = "exercised by the Source-first parent test in an isolated root"]
fn native_yaml_kinds_helper() {
    let root = PathBuf::from(std::env::var("SK_G2_ROOT").expect("isolated root"));
    assert_eq!(
        std::env::current_dir()
            .expect("cwd")
            .canonicalize()
            .unwrap(),
        root
    );
    let repository = sk_doctrine::ProfileRepository::load_profile_directories(
        Path::new(&std::env::var("SK_G2_BUILT_IN").expect("built-in")),
        &[],
        Some(Path::new(&std::env::var("SK_G2_PROJECT").expect("project"))),
        None,
    )
    .expect("native load");
    let mut ids: Vec<String> = repository
        .ids()
        .into_iter()
        .filter_map(|id| id.to_scalar_string().ok())
        .collect();
    ids.sort();
    let mut dumps = serde_json::Map::new();
    for id in &ids {
        let profile = repository.get_str(id).expect("loaded profile");
        dumps.insert(
            id.clone(),
            json_dump(&serde_yaml_ng::Value::Mapping(
                profile.model_dump().shadow().clone(),
            )),
        );
    }
    let skipped: Vec<Option<String>> = repository
        .skipped()
        .iter()
        .map(|entry| entry.profile_id_label())
        .collect();
    println!(
        "{}",
        json!({ "ids": ids, "skipped": skipped, "dumps": dumps })
    );
}

fn json_dump(value: &serde_yaml_ng::Value) -> Value {
    match value {
        serde_yaml_ng::Value::Null => Value::Null,
        serde_yaml_ng::Value::Bool(flag) => Value::Bool(*flag),
        serde_yaml_ng::Value::Number(number) => {
            json!(number.as_i64().expect("integer profile dump value"))
        }
        serde_yaml_ng::Value::String(text) => Value::String(text.clone()),
        serde_yaml_ng::Value::Sequence(items) => {
            Value::Array(items.iter().map(json_dump).collect())
        }
        serde_yaml_ng::Value::Mapping(mapping) => {
            let mut object = serde_json::Map::new();
            for (key, value) in mapping {
                object.insert(
                    key.as_str()
                        .expect("profile dump keys are strings")
                        .to_owned(),
                    json_dump(value),
                );
            }
            Value::Object(object)
        }
        serde_yaml_ng::Value::Tagged(tagged) => json_dump(&tagged.value),
    }
}

// ---------------------------------------------------------------------------
// The comparison
// ---------------------------------------------------------------------------

fn compare_for_seed(seed: &str) -> Value {
    let fixture = Fixture::new(seed);
    // Source runs and asserts its own post-fix facts first.
    let source = run_source(&fixture);
    // Only then the native loader, in an isolated child with the same seed.
    let native = run_native(&fixture);

    let source_ids: Vec<String> = source["ids"]
        .as_array()
        .expect("source ids")
        .iter()
        .map(|id| id.as_str().expect("scalar id").to_owned())
        .collect();
    let native_ids: Vec<String> = native["ids"]
        .as_array()
        .expect("native ids")
        .iter()
        .map(|id| id.as_str().expect("scalar id").to_owned())
        .collect();
    assert!(
        source_ids.len() >= 45,
        "the matrix must actually load the constructed-kind cases: {source_ids:?}"
    );
    for original in [
        "binary-name",
        "binary-role-entry",
        "binary-role-scalar",
        "binary-priority",
        "binary-bool",
        "set-roles",
        "set-capabilities",
        "set-excluding",
    ] {
        assert!(
            native_ids.iter().any(|id| id == original),
            "the original counterexample {original} must load natively"
        );
    }
    assert_eq!(native_ids, source_ids, "loaded profile set at seed {seed}");

    let source_skips: Vec<Option<String>> = source["skipped"]
        .as_array()
        .expect("source skips")
        .iter()
        .map(|id| id.as_str().map(str::to_owned))
        .collect();
    let native_skips: Vec<Option<String>> = native["skipped"]
        .as_array()
        .expect("native skips")
        .iter()
        .map(|id| id.as_str().map(str::to_owned))
        .collect();
    assert!(
        source_skips.iter().any(Option::is_some) && source_skips.iter().any(Option::is_none),
        "both a model-invalid and a YAML-invalid profile must be skipped: {source_skips:?}"
    );
    let mut source_skips_sorted = source_skips;
    source_skips_sorted.sort();
    let mut native_skips_sorted = native_skips;
    native_skips_sorted.sort();
    assert_eq!(
        native_skips_sorted, source_skips_sorted,
        "skipped declared ids (source {:?})",
        source["skipped"]
    );

    for id in &source_ids {
        assert_eq!(
            &native["dumps"][id], &source["dumps"][id],
            "{id}: native model_dump diverged from the reference at seed {seed}"
        );
    }
    source
}

#[test]
fn constructed_yaml_kinds_and_implicit_resolution_are_source_exact_seed0() {
    let source = compare_for_seed("0");
    // The reference's set order is the hash table's, not the authored order:
    // seed 0 lists Implementer before Reviewer for `!!set {Reviewer, Implementer}`.
    assert_eq!(
        source["dumps"]["set-roles-two"]["roles"],
        json!(["implementer", "reviewer"])
    );
}

#[test]
fn constructed_yaml_kinds_and_implicit_resolution_are_source_exact_seed1() {
    let source = compare_for_seed("1");
    assert_eq!(
        source["dumps"]["set-roles-two"]["roles"],
        json!(["reviewer", "implementer"]),
        "a different hash seed must change the reference's set order"
    );
}
