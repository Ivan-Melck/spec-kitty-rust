//! Source-first lossless-transport matrix for authored agent profiles.
//!
//! Every case is authored twice into one temporary project: once as the YAML
//! text, and once as the expected Python code points. The **pinned Source
//! oracle runs first** and asserts each expectation (`AgentProfileRepository`
//! over the same directory), so a drift in the reference — or in my reading of
//! it — reddens before any native assertion. Only then does the native loader
//! read the same bytes and the two are compared field-for-field.
//!
//! The interesting class is a double-quoted scalar whose decoded value holds a
//! surrogate code unit (`U+D800..=U+DFFF`): the reference keeps it, and native
//! must not drop the profile, refuse it, pair it, or replace it with the literal
//! escape text.
//!
//! Isolation: both processes run with an asserted temporary CWD, `HOME`, every
//! `XDG_*` directory and `TMPDIR` under the same temporary root. The oracle also
//! asserts its own environment, so a leaked host directory cannot make this pass.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sk_doctrine::ProfileRepository;
use sk_kernel::PythonString;

/// One authored profile fixture plus the exact values the reference produces.
#[derive(Clone, Debug)]
struct Expected {
    id: Vec<u32>,
    name: Vec<u32>,
    description: Vec<u32>,
    purpose: Vec<u32>,
    roles: Vec<Vec<u32>>,
    routing_priority: i64,
    max_concurrent_tasks: i64,
    schema_version: Vec<u32>,
    sentinel: bool,
    primary_focus: Vec<u32>,
    tactic_ids: Vec<Vec<u32>>,
    domain_keywords: Vec<Vec<u32>>,
    capabilities: Vec<Vec<u32>>,
    canonical_verbs: Vec<Vec<u32>>,
    mode_use_cases: Vec<Vec<u32>>,
}

impl Default for Expected {
    fn default() -> Self {
        Self {
            id: Vec::new(),
            name: Vec::new(),
            description: Vec::new(),
            purpose: purpose(),
            // `Role(value)` lower-cases in the reference model.
            roles: vec![scalars("reviewer")],
            routing_priority: 50,
            max_concurrent_tasks: 5,
            schema_version: scalars("1.0"),
            sentinel: false,
            primary_focus: scalars("review"),
            tactic_ids: Vec::new(),
            domain_keywords: Vec::new(),
            capabilities: Vec::new(),
            canonical_verbs: Vec::new(),
            mode_use_cases: Vec::new(),
        }
    }
}

/// `s` as exact Python code points.
fn scalars(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

/// `text` followed by the surrogate code units `codes`.
fn scalars_with(text: &str, codes: &[u32]) -> Vec<u32> {
    let mut points = scalars(text);
    points.extend_from_slice(codes);
    points
}

fn purpose() -> Vec<u32> {
    scalars("Preserve authored routing identities.")
}

/// A minimal valid profile body suffix.
const TAIL: &str =
    "purpose: Preserve authored routing identities.\nspecialization:\n  primary-focus: review\n";

fn case(file: &str, id: Vec<u32>, name: Vec<u32>, yaml_extra: &str, expect: Expected) -> Case {
    let expect = Expected { id, name, ..expect };
    let mut yaml = yaml_extra.to_owned();
    if !yaml.contains("roles:") {
        yaml.push_str("roles: [reviewer]\n");
    }
    yaml.push_str(TAIL);
    Case {
        file: file.to_owned(),
        yaml,
        expect,
    }
}

struct Case {
    file: String,
    yaml: String,
    expect: Expected,
}

fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::with_capacity(28);

    // c01 — a lone surrogate in the id and in a nested keyword list.
    cases.push(case(
        "c01-surrogate-id.agent.yaml",
        scalars_with("author", &[0xd800]),
        scalars("Source-valid profile"),
        concat!(
            "profile-id: \"author\\ud800\"\n",
            "name: Source-valid profile\n",
            "description: \"desc-\\ud800\"\n",
            "specialization-context:\n  domain-keywords: [widget, \"kw-\\ud800\"]\n",
        ),
        Expected {
            description: scalars_with("desc-", &[0xd800]),
            domain_keywords: vec![scalars("widget"), scalars_with("kw-", &[0xd800])],
            ..Expected::default()
        },
    ));
    // c02 — a literal backslash escape stays literal text.
    cases.push(case(
        "c02-literal-backslash.agent.yaml",
        scalars("lit-backslash"),
        scalars("Literal"),
        concat!(
            "profile-id: lit-backslash\n",
            "name: Literal\n",
            "description: \"author\\\\ud800\"\n",
        ),
        Expected {
            // `\\u` is an escaped backslash followed by the letters `u800`.
            description: scalars("author\\ud800"),
            ..Expected::default()
        },
    ));
    // c03 — the reference does NOT combine a surrogate pair.
    cases.push(case(
        "c03-surrogate-pair.agent.yaml",
        scalars_with("pair-id-", &[0xd83d, 0xde00]),
        scalars("Pair"),
        concat!(
            "profile-id: \"pair-id-\\uD83D\\uDE00\"\n",
            "name: Pair\n",
            "description: \"pair-\\uD83D\\uDE00\"\n",
        ),
        Expected {
            description: scalars_with("pair-", &[0xd83d, 0xde00]),
            ..Expected::default()
        },
    ));
    // c04 — an astral escape is an ordinary scalar.
    cases.push(case(
        "c04-astral.agent.yaml",
        scalars_with("astral-", &[0x1f600]),
        scalars("Astral"),
        concat!(
            "profile-id: \"astral-\\U0001F600\"\n",
            "name: Astral\n",
            "description: \"big-\\U0001F600\"\n",
        ),
        Expected {
            description: scalars_with("big-", &[0x1f600]),
            ..Expected::default()
        },
    ));
    // c05 — private-use code points are ordinary data.
    cases.push(case(
        "c05-pua.agent.yaml",
        scalars_with("pua-", &[0xe000]),
        scalars("Pua"),
        concat!(
            "profile-id: \"pua-\\uE000\"\n",
            "name: Pua\n",
            "description: \"pua-\\uE000\"\n",
        ),
        Expected {
            description: scalars_with("pua-", &[0xe000]),
            ..Expected::default()
        },
    ));
    // c06 — NUL is preserved and is distinct from every surrogate.
    cases.push(case(
        "c06-nul.agent.yaml",
        scalars("nul-\u{0}-x"),
        scalars("Nul"),
        concat!(
            "profile-id: \"nul-\\u0000-x\"\n",
            "name: Nul\n",
            "description: \"nul-\\u0000-x\"\n",
        ),
        Expected {
            description: scalars("nul-\u{0}-x"),
            ..Expected::default()
        },
    ));
    // c07 — single quotes never unescape.
    cases.push(case(
        "c07-single-quoted.agent.yaml",
        scalars("single"),
        scalars("Single"),
        concat!(
            "profile-id: single\n",
            "name: Single\n",
            "description: 'sq-\\ud800'\n",
        ),
        Expected {
            description: scalars("sq-\\ud800"),
            ..Expected::default()
        },
    ));
    // c08 — a literal block scalar never unescapes.
    cases.push(case(
        "c08-block-literal.agent.yaml",
        scalars("block"),
        scalars("Block"),
        concat!(
            "profile-id: block\n",
            "name: Block\n",
            "description: |-\n  block-\\ud800\n",
        ),
        Expected {
            description: scalars("block-\\ud800"),
            ..Expected::default()
        },
    ));
    // c09 — folded block scalar.
    cases.push(case(
        "c09-block-folded.agent.yaml",
        scalars("folded"),
        scalars("Folded"),
        concat!(
            "profile-id: folded\n",
            "name: Folded\n",
            "description: >-\n  folded\n  text\n",
        ),
        Expected {
            description: scalars("folded text"),
            ..Expected::default()
        },
    ));
    // c10 — repeated surrogates are kept, not merged.
    cases.push(case(
        "c10-repeat.agent.yaml",
        scalars_with("double-", &[0xd800, 0xd800]),
        scalars("Double"),
        concat!("profile-id: \"double-\\uD800\\uD800\"\n", "name: Double\n",),
        Expected::default(),
    ));
    // c11 — no pairing in either direction.
    cases.push(case(
        "c11-reverse.agent.yaml",
        scalars_with("reverse-", &[0xdc00, 0xd800]),
        scalars("Reverse"),
        concat!(
            "profile-id: \"reverse-\\uDC00\\uD800\"\n",
            "name: Reverse\n",
        ),
        Expected::default(),
    ));
    // c12 — a surrogate followed by a normal code point.
    cases.push(case(
        "c12-mixed.agent.yaml",
        scalars_with("mixed-", &[0xd800, 0x41]),
        scalars("Mixed"),
        concat!("profile-id: \"mixed-\\uD800\\u0041\"\n", "name: Mixed\n",),
        Expected::default(),
    ));
    // c13 — an anchor and an alias carry the same exact value.
    cases.push(case(
        "c13-alias.agent.yaml",
        scalars("alias"),
        scalars_with("shared", &[0xd800]),
        concat!(
            "profile-id: alias\n",
            "name: &shared \"shared\\ud800\"\n",
            "description: *shared\n",
        ),
        Expected {
            description: scalars_with("shared", &[0xd800]),
            ..Expected::default()
        },
    ));
    // c14 — a `<<` merge key contributes exact reference entries.
    cases.push(case(
        "c14-merge-key.agent.yaml",
        scalars("merged"),
        scalars("Merged"),
        concat!(
            "profile-id: merged\n",
            "name: Merged\n",
            "mode-defaults:\n",
            "  - &ref {mode: \"mode-\\ud800\", description: authored, use-case: first}\n",
            "  - <<: *ref\n",
            "    use-case: second\n",
            "tactic-references:\n",
            "  - id: \"tac-\\ud800\"\n",
            "    rationale: authored\n",
            "  - id: tac-2\n",
            "    rationale: second\n",
        ),
        Expected {
            tactic_ids: vec![scalars_with("tac-", &[0xd800]), scalars("tac-2")],
            mode_use_cases: vec![scalars("first"), scalars("second")],
            ..Expected::default()
        },
    ));
    // c15 — a plain scalar keeps the backslash literally.
    cases.push(case(
        "c15-plain.agent.yaml",
        scalars("plain"),
        scalars("Plain"),
        concat!(
            "profile-id: plain\n",
            "name: Plain\n",
            "description: plain-\\ud800-text\n",
        ),
        Expected {
            description: scalars("plain-\\ud800-text"),
            ..Expected::default()
        },
    ));
    // c16 — roles are lower-cased.
    cases.push(case(
        "c16-role-case.agent.yaml",
        scalars("roles-upper"),
        scalars("Roles"),
        concat!(
            "profile-id: roles-upper\n",
            "name: Roles\n",
            "roles: [\"Reviewer\"]\n",
        ),
        Expected::default(),
    ));
    // c17 — the control: model defaults, nothing invented.
    cases.push(case(
        "c17-control.agent.yaml",
        scalars("control"),
        scalars("Control"),
        concat!("profile-id: control\n", "name: Control\n"),
        Expected::default(),
    ));
    // c18 — double-quote folding around a surrogate.
    cases.push(case(
        "c18-fold.agent.yaml",
        scalars("fold-newline"),
        scalars("Fold"),
        concat!(
            "profile-id: fold-newline\n",
            "name: Fold\n",
            "description: \"multi\n  line\\ud800\"\n",
        ),
        Expected {
            description: scalars_with("multi line", &[0xd800]),
            ..Expected::default()
        },
    ));
    // c19 — an escaped line break around a surrogate.
    cases.push(case(
        "c19-escaped-break.agent.yaml",
        scalars("esc-break"),
        scalars("Break"),
        concat!(
            "profile-id: esc-break\n",
            "name: Break\n",
            "description: \"esc\\\n  break\\ud800\"\n",
        ),
        Expected {
            description: scalars_with("escbreak", &[0xd800]),
            ..Expected::default()
        },
    ));
    // c20 — all 2048 surrogate code units in one authored field.
    let all_surrogates: Vec<u32> = (0xd800..=0xdfff).collect();
    let mut escapes = String::new();
    for code in &all_surrogates {
        escapes.push_str(&format!("\\u{code:04X}"));
    }
    cases.push(case(
        "c20-all-surrogates.agent.yaml",
        scalars("all-surrogates"),
        scalars("All"),
        &format!("profile-id: all-surrogates\nname: All\ndescription: \"{escapes}\"\n"),
        Expected {
            description: all_surrogates,
            ..Expected::default()
        },
    ));
    // Coordinator Source-first counterexamples: the marker namespace is data,
    // trailing quoted whitespace is authored, and custom roles are half-open.
    for (file, id, name_token) in [
        (
            "c21-marker-quoted.agent.yaml",
            "marker-quoted",
            "\"\\uFDD0d800\\uFDEF\"",
        ),
        (
            "c25-marker-single.agent.yaml",
            "marker-single",
            "'\u{fdd0}d800\u{fdef}'",
        ),
        (
            "c26-marker-plain.agent.yaml",
            "marker-plain",
            "\u{fdd0}d800\u{fdef}",
        ),
        (
            "c27-marker-block.agent.yaml",
            "marker-block",
            "|-\n  \u{fdd0}d800\u{fdef}",
        ),
    ] {
        cases.push(case(
            file,
            scalars(id),
            scalars("\u{fdd0}d800\u{fdef}"),
            &format!("profile-id: {id}\nname: {name_token}\n"),
            Expected::default(),
        ));
    }
    cases.push(case(
        "c22-marker-collision.agent.yaml",
        scalars("marker-collision"),
        scalars("\u{fdd0}d800\u{fdef}"),
        "profile-id: marker-collision\nname: \"\\uFDD0d800\\uFDEF\"\ndescription: \"\\ud800\"\n",
        Expected {
            description: vec![0xd800],
            ..Expected::default()
        },
    ));
    cases.push(case(
        "c23-trailing.agent.yaml",
        scalars("trailing"),
        scalars_with("trail", &[0xd800, 0x20, 0x20]),
        "profile-id: trailing\nname: \"trail\\ud800  \"\n",
        Expected::default(),
    ));
    cases.push(case(
        "c24-role-sigma.agent.yaml",
        scalars("role-sigma"),
        scalars("Role sigma"),
        "profile-id: role-sigma\nname: Role sigma\nroles: [\"\\u039F\\u03A3\"]\n",
        Expected {
            roles: vec![vec![0x3bf, 0x3c2]],
            ..Expected::default()
        },
    ));
    cases.push(case(
        "c28-role-new-unicode.agent.yaml",
        scalars("role-new-unicode"),
        scalars("Role new Unicode"),
        "profile-id: role-new-unicode\nname: Role new Unicode\nroles: [\"\\u1C89\"]\n",
        Expected {
            roles: vec![vec![0x1c89]],
            ..Expected::default()
        },
    ));
    cases
}

fn expected_json(expect: &Expected) -> Value {
    json!({
        "id": expect.id,
        "name": expect.name,
        "description": expect.description,
        "purpose": expect.purpose,
        "roles": expect.roles,
        "routing_priority": expect.routing_priority,
        "max_concurrent_tasks": expect.max_concurrent_tasks,
        "schema_version": expect.schema_version,
        "sentinel": expect.sentinel,
        "primary_focus": expect.primary_focus,
        "tactic_ids": expect.tactic_ids,
        "domain_keywords": expect.domain_keywords,
        "capabilities": expect.capabilities,
        "canonical_verbs": expect.canonical_verbs,
        "mode_use_cases": expect.mode_use_cases,
    })
}

/// The source-side profile assertions, generated from the same expectations.
///
/// The script asserts the reference's own values *and* the environment before
/// emitting JSON, so "Source ran first" is enforceable, not just claimed.
const SOURCE_SCRIPT: &str = r#"
import json, os, sys
from pathlib import Path
from charter.offering.agent_profiles.repository import AgentProfileRepository

root = Path(os.environ["SK_TRANSPORT_ROOT"]).resolve()
profiles_dir = Path(os.environ["SK_TRANSPORT_PROFILES"]).resolve()
empty_built_in = Path(os.environ["SK_TRANSPORT_EMPTY"]).resolve()
expectations = json.loads(Path(os.environ["SK_TRANSPORT_EXPECT"]).read_text(encoding="utf-8"))

# Environment isolation is asserted, not assumed.
assert Path.cwd().resolve() == root, (Path.cwd(), root)
home = Path(os.environ["HOME"]).resolve()
tmp = Path(os.environ["TMPDIR"]).resolve()
xdg = [Path(os.environ[key]).resolve() for key in (
    "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME")]
for path in [home, tmp, *xdg]:
    assert str(path).startswith(str(root) + os.sep), (path, root)
assert not empty_built_in.exists()
for leaked in ("SK_KERNEL_PYTHON", "SK_DOCTRINE_PYTHON"):
    assert leaked not in os.environ, leaked

def cps(text):
    return [ord(character) for character in text]

repo = AgentProfileRepository(built_in_dir=empty_built_in, project_dir=profiles_dir)
skipped = repo.skipped_profiles()
assert skipped == [], [(s.layer, s.path, s.error_summary) for s in skipped]
profiles = repo.list_all()
assert len(profiles) == len(expectations), (len(profiles), len(expectations))

by_id = {tuple(cps(profile.profile_id)): profile for profile in profiles}
out = []
for expect in expectations:
    key = tuple(expect["id"])
    assert key in by_id, (expect["id"], sorted(by_id))
    profile = by_id[key]
    sc = profile.specialization_context
    actual = {
        "id": cps(profile.profile_id),
        "name": cps(profile.name),
        "description": cps(profile.description),
        "purpose": cps(profile.purpose),
        "roles": [cps(str(role)) for role in profile.roles],
        "routing_priority": profile.routing_priority,
        "max_concurrent_tasks": profile.max_concurrent_tasks,
        "schema_version": cps(profile.schema_version),
        "sentinel": profile.sentinel,
        "primary_focus": cps(profile.specialization.primary_focus),
        "tactic_ids": [cps(ref.id) for ref in profile.tactic_references],
        "domain_keywords": [cps(value) for value in (sc.domain_keywords if sc else [])],
        "capabilities": [cps(value) for value in profile.capabilities],
        "canonical_verbs": [cps(value) for value in profile.collaboration.canonical_verbs],
        "mode_use_cases": [cps(mode.use_case) for mode in profile.mode_defaults],
    }
    for field, want in expect.items():
        got = actual[field]
        assert got == want, (expect["id"], field, got, want)
    out.append(actual)

print(json.dumps({"count": len(out), "profiles": out}))
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    profiles: PathBuf,
    empty: PathBuf,
}

fn fixture(cases: &[Case]) -> Fixture {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = temp.path().canonicalize().expect("canonical root");
    // `profiles` is the pack root; the loader reads `profiles/agent_profiles`,
    // which is also the directory handed to the reference as `project_dir`.
    let profiles = root.join("profiles");
    let empty = root.join("empty-built-in");
    fs::create_dir_all(profiles.join("agent_profiles")).expect("profiles directory");
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        fs::create_dir_all(root.join(directory)).expect("isolated directory");
    }
    for case in cases {
        fs::write(profiles.join("agent_profiles").join(&case.file), &case.yaml)
            .expect("write fixture");
    }
    Fixture {
        _temp: temp,
        root,
        profiles,
        empty,
    }
}

impl Fixture {
    /// The environment both children run with: every shared root is temporary.
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
        ]
    }
}

/// Run the pinned Source oracle over the fixture and assert its own values.
fn run_source(fixture: &Fixture, cases: &[Case]) -> Value {
    let expectations: Vec<Value> = cases
        .iter()
        .map(|case| expected_json(&case.expect))
        .collect();
    let expect_path = fixture.root.join("expectations.json");
    fs::write(
        &expect_path,
        serde_json::to_vec(&expectations).expect("serialise expectations"),
    )
    .expect("write expectations");
    let mut environment = fixture.env();
    environment.push((
        "PYTHONPATH".to_owned(),
        support::repository_root().join("src").display().to_string(),
    ));
    environment.push((
        "SPEC_KITTY_PACKS_ROOT".to_owned(),
        support::repository_root()
            .join("packs")
            .display()
            .to_string(),
    ));
    environment.push((
        "SK_TRANSPORT_ROOT".to_owned(),
        fixture.root.display().to_string(),
    ));
    environment.push((
        "SK_TRANSPORT_PROFILES".to_owned(),
        fixture
            .profiles
            .join("agent_profiles")
            .display()
            .to_string(),
    ));
    environment.push((
        "SK_TRANSPORT_EMPTY".to_owned(),
        fixture.empty.display().to_string(),
    ));
    environment.push((
        "SK_TRANSPORT_EXPECT".to_owned(),
        expect_path.display().to_string(),
    ));
    let output = Command::new(support::python())
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .env_clear()
        .envs(environment)
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

fn points(value: &Value) -> Vec<u32> {
    value
        .as_array()
        .expect("code point array")
        .iter()
        .map(|code| u32::try_from(code.as_u64().expect("integer code point")).expect("u32"))
        .collect()
}

fn points_list(value: &Value) -> Vec<Vec<u32>> {
    value
        .as_array()
        .expect("list of code point arrays")
        .iter()
        .map(points)
        .collect()
}

fn string_points(value: Option<PythonString>) -> Vec<u32> {
    value
        .map(|value| value.codepoints().to_vec())
        .unwrap_or_default()
}

fn points_of(strings: Vec<PythonString>) -> Vec<Vec<u32>> {
    strings
        .iter()
        .map(|value| value.codepoints().to_vec())
        .collect()
}

/// Read the same authored bytes through the native loader.
fn assert_native_loaded(empty: &Path, profiles: &Path, cases: &[Case]) {
    let repository =
        ProfileRepository::load(empty, &[], Some(profiles)).expect("native profile load");
    assert!(
        repository.skipped().is_empty(),
        "native skipped {} profile(s): {:?}",
        repository.skipped().len(),
        repository.skipped()
    );
    assert_eq!(repository.ids().len(), cases.len());
    for case in cases {
        let id = PythonString::from_codepoints(&case.expect.id).expect("valid id");
        let profile = repository
            .get(&id)
            .unwrap_or_else(|| panic!("native lost the profile for {:?}", case.file));
        assert_eq!(
            string_points(profile.name()),
            case.expect.name,
            "{} name",
            case.file
        );
        assert_eq!(
            string_points(profile.description()),
            case.expect.description,
            "{} description",
            case.file
        );
        assert_eq!(
            points_of(profile.roles()),
            case.expect.roles,
            "{} roles",
            case.file
        );
        assert_eq!(profile.routing_priority(), case.expect.routing_priority);
        assert_eq!(
            profile.max_concurrent_tasks(),
            Some(case.expect.max_concurrent_tasks)
        );
        assert_eq!(
            profile.max_concurrent_tasks_exact().decimal(),
            case.expect.max_concurrent_tasks.to_string(),
            "{} exact max-concurrent-tasks",
            case.file
        );
        assert_eq!(
            profile.schema_version().codepoints(),
            &case.expect.schema_version[..]
        );
        assert_eq!(profile.sentinel(), case.expect.sentinel);
        assert_eq!(
            string_points(profile.primary_focus()),
            case.expect.primary_focus,
            "{} primary focus",
            case.file
        );
        assert_eq!(
            points_of(profile.tactic_references()),
            case.expect.tactic_ids,
            "{} tactic references",
            case.file
        );
        assert_eq!(
            points_of(profile.domain_keywords()),
            case.expect.domain_keywords,
            "{} domain keywords",
            case.file
        );
        assert_eq!(
            points_of(profile.strings("capabilities")),
            case.expect.capabilities,
            "{} capabilities",
            case.file
        );
        assert_eq!(
            points_of(profile.canonical_verbs()),
            case.expect.canonical_verbs,
            "{} canonical verbs",
            case.file
        );
        let mode_use_cases: Vec<Vec<u32>> = profile
            .list("mode-defaults")
            .into_iter()
            .map(|item| {
                item.get("use-case")
                    .and_then(serde_yaml_ng::Value::as_str)
                    .map(|value| profile.body.exact_string(value).codepoints().to_vec())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            mode_use_cases, case.expect.mode_use_cases,
            "{} mode use cases",
            case.file
        );
        assert_eq!(profile.scalar_id().is_some(), id.to_scalar_string().is_ok());
    }
}

fn assert_native(fixture: &Fixture, _cases: &[Case]) {
    let output = Command::new(std::env::current_exe().expect("native test executable"))
        .args([
            "--exact",
            "native_profile_transport_helper",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .envs(fixture.env())
        .env("SK_NATIVE_PROFILE_TRANSPORT_ROOT", &fixture.root)
        .current_dir(&fixture.root)
        .output()
        .expect("run isolated native loader");
    assert!(
        output.status.success(),
        "native loader failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "exercised by the Source-first parent tests in isolated roots"]
fn native_profile_transport_helper() {
    let root = PathBuf::from(
        std::env::var_os("SK_NATIVE_PROFILE_TRANSPORT_ROOT")
            .expect("parent supplies native fixture root"),
    );
    assert_eq!(std::env::current_dir().unwrap(), root);
    for key in [
        "HOME",
        "TMPDIR",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
    ] {
        assert_eq!(
            PathBuf::from(std::env::var_os(key).expect("isolated root")).parent(),
            Some(root.as_path())
        );
    }
    let cases = cases();
    let expected: Value =
        serde_json::from_slice(&fs::read(root.join("expectations.json")).unwrap()).unwrap();
    assert_eq!(
        expected,
        json!(
            cases
                .iter()
                .map(|case| expected_json(&case.expect))
                .collect::<Vec<_>>()
        )
    );
    assert_native_loaded(&root.join("empty-built-in"), &root.join("profiles"), &cases);
}

#[test]
fn source_and_native_agree_on_every_authored_case() {
    let cases = cases();
    let fixture = fixture(&cases);
    let source = run_source(&fixture, &cases);
    assert_eq!(source["count"], json!(cases.len()));

    // Compare the Source payload with the authored expectations (the script
    // already asserts this; re-checking here keeps the comparison explicit).
    let profiles = source["profiles"].as_array().expect("profiles array");
    for (case, source) in cases.iter().zip(profiles) {
        assert_eq!(points(&source["id"]), case.expect.id, "{} id", case.file);
        assert_eq!(
            points(&source["description"]),
            case.expect.description,
            "{} source description",
            case.file
        );
        assert_eq!(
            points_list(&source["tactic_ids"]),
            case.expect.tactic_ids,
            "{} source tactic ids",
            case.file
        );
    }

    assert_native(&fixture, &cases);
}

#[test]
fn literal_backslash_and_nul_are_distinct_from_surrogates() {
    let cases = cases();
    let fixture = fixture(&cases);
    let source = run_source(&fixture, &cases);
    let profiles = source["profiles"].as_array().expect("profiles array");
    let by_id = |wanted: &[u32]| -> &Value {
        profiles
            .iter()
            .find(|profile| points(&profile["id"]) == wanted)
            .unwrap_or_else(|| panic!("missing {wanted:?}"))
    };
    // "author\\ud800" is 12 code points: a literal backslash, not a surrogate.
    let literal = by_id(&scalars("lit-backslash"));
    assert_eq!(points(&literal["description"]), scalars("author\\ud800"));
    assert!(!points(&literal["description"]).contains(&0xd800));
    // NUL is preserved and is not the surrogate.
    let nul = by_id(&scalars("nul-\u{0}-x"));
    assert_eq!(points(&nul["description"]), scalars("nul-\u{0}-x"));
    assert!(points(&nul["description"]).contains(&0));
    assert!(!points(&nul["description"]).contains(&0xd800));
    // The astral scalar and the surrogate pair are different values.
    let astral = by_id(&scalars_with("astral-", &[0x1f600]));
    assert_eq!(
        points(&astral["description"]),
        scalars_with("big-", &[0x1f600])
    );
    let pair = by_id(&scalars_with("pair-id-", &[0xd83d, 0xde00]));
    assert_eq!(
        points(&pair["description"]),
        scalars_with("pair-", &[0xd83d, 0xde00])
    );
    assert_ne!(points(&pair["description"]), points(&astral["description"]));
    assert_native(&fixture, &cases);
}
