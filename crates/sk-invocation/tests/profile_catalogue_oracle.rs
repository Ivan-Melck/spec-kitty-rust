//! Source-first oracle for the invocation `ProfileRegistry` catalogue.
//!
//! Four real project shapes are built from the shipped built-in packs, then read
//! by the pinned Source `ProfileRegistry` first and by
//! `sk_invocation::catalogue::ProfileRegistry` second. The comparison is the
//! catalogue contract, not a count:
//!
//! * which ids are present (the routing catalogue);
//! * every routing field the router consumes (role, priority, keywords, verbs);
//! * `resolve` vs `resolve_local` for a doctrine id and a legacy-overlay id;
//! * `has_profiles()` for the explicit-empty activation case; and
//! * the language-scope drop of a *doctrine* profile, while a legacy overlay
//!   profile with the same scope stays (the legacy layer is ungated and
//!   language-unfiltered).
//!
//! Isolation: both processes run with an asserted temporary CWD, `HOME`, every
//! `XDG_*` directory and `TMPDIR` under the temporary root.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use sk_doctrine::AgentProfile;
use sk_invocation::catalogue::{CatalogueError, ProfileRegistry};
use sk_kernel::PythonString;

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create directory");
    for entry in fs::read_dir(from).expect("read directory") {
        let entry = entry.expect("directory entry");
        let source = entry.path();
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&source, &target);
        } else if entry.file_type().expect("file type").is_file() {
            fs::copy(&source, &target).expect("copy file");
        }
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
    fs::write(path, text).expect("write fixture");
}

const LEGACY_COLLISION: &str = concat!(
    "profile-id: reviewer-renata\n",
    "name: Legacy Renata\n",
    "description: legacy overlay wins\n",
    "purpose: legacy parity\n",
    "roles: [reviewer]\n",
    "routing-priority: 91\n",
    "specialization:\n  primary-focus: review\n",
    "specialization-context:\n  domain-keywords: [legacy-keyword]\n",
);

const LEGACY_ONLY: &str = concat!(
    "profile-id: legacy-only\n",
    "name: Legacy Only\n",
    "purpose: legacy parity\n",
    "roles: [curator]\n",
    "specialization:\n  primary-focus: curate\n",
    "collaboration:\n  canonical-verbs: [curate]\n",
);

const LEGACY_SCOPED: &str = concat!(
    "profile-id: legacy-jvm\n",
    "name: Legacy Jvm\n",
    "purpose: legacy parity\n",
    "roles: [reviewer]\n",
    "applies_to_languages: [jvm]\n",
    "specialization:\n  primary-focus: review\n",
);

/// One catalogue shape.
struct Scenario {
    name: &'static str,
    languages: Option<&'static str>,
    activation: Option<&'static str>,
    legacy: bool,
    /// Author a Source-valid profile whose id holds a lone surrogate.
    surrogate: bool,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "plain",
        languages: None,
        activation: None,
        legacy: false,
        surrogate: false,
    },
    Scenario {
        name: "legacy",
        languages: None,
        activation: None,
        legacy: true,
        surrogate: true,
    },
    Scenario {
        name: "gated",
        languages: Some("languages:\n  - python\n"),
        activation: Some(
            "activated_agent_profiles:\n- reviewer-renata\n- python-pedro\n- java-jenny\n",
        ),
        legacy: true,
        surrogate: true,
    },
    Scenario {
        name: "empty-gate",
        languages: None,
        activation: Some("activated_agent_profiles: []\n"),
        legacy: false,
        surrogate: false,
    },
];

/// A Source-valid profile whose id is `author` + `U+D800`.
///
/// The reference catalogs it; a `String`-keyed catalogue would drop it. It is
/// authored in the *legacy* overlay so it exercises the ungated layer.
const SURROGATE_LEGACY: &str = concat!(
    "profile-id: \"author\\ud800\"\n",
    "name: Surrogate Author\n",
    "purpose: preserve authored identities\n",
    "roles: [reviewer]\n",
    "specialization:\n  primary-focus: review\n",
);

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    packs: PathBuf,
    project: PathBuf,
}

fn fixture(scenario: &Scenario) -> Fixture {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = temp.path().canonicalize().expect("canonical root");
    let packs = root.join("packs");
    let project = root.join("project");
    copy_tree(
        &support::repository_root().join("packs/built-in"),
        &packs.join("built-in"),
    );
    fs::create_dir_all(&project).expect("project directory");
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        fs::create_dir_all(root.join(directory)).expect("isolated directory");
    }
    if let Some(languages) = scenario.languages {
        write(&project.join(".kittify/config.yaml"), languages);
    }
    if let Some(activation) = scenario.activation {
        write(
            &project.join(".kittify/charter/activation.yaml"),
            activation,
        );
    }
    if scenario.legacy {
        let legacy = project.join(".kittify/profiles");
        write(&legacy.join("reviewer-renata.agent.yaml"), LEGACY_COLLISION);
        write(&legacy.join("legacy-only.agent.yaml"), LEGACY_ONLY);
        write(&legacy.join("legacy-jvm.agent.yaml"), LEGACY_SCOPED);
        if scenario.surrogate {
            write(&legacy.join("author.agent.yaml"), SURROGATE_LEGACY);
        }
    }
    Fixture {
        _temp: temp,
        root,
        packs,
        project,
    }
}

impl Fixture {
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
            (
                "PYTHONPATH".to_owned(),
                support::repository_root().join("src").display().to_string(),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".to_owned(),
                self.packs.display().to_string(),
            ),
            (
                "SK_CATALOGUE_ROOT".to_owned(),
                self.root.display().to_string(),
            ),
        ]
    }
}

const SOURCE_SCRIPT: &str = r#"
import json, os
from pathlib import Path
from specify_cli.invocation.registry import ProfileRegistry

root = Path(os.environ["SK_CATALOGUE_ROOT"]).resolve()
project = root / "project"

assert Path.cwd().resolve() == project, (Path.cwd(), project)
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)

def cps(text):
    return [ord(character) for character in text]

def values(items):
    return [cps(value) for value in (items or [])]

registry = ProfileRegistry(project)
profiles = registry.list_all()

def describe(profile):
    sc = profile.specialization_context
    return {
        "id": cps(profile.profile_id),
        "name": cps(profile.name),
        "description": cps(profile.description),
        "role": cps(str(profile.role)),
        "routing_priority": profile.routing_priority,
        "domain_keywords": values(sc.domain_keywords if sc else []),
        "canonical_verbs": values(profile.collaboration.canonical_verbs),
        "applies_to_languages": values(profile.applies_to_languages),
    }

def resolution(resolver, profile_id):
    try:
        return {"ok": describe(resolver(profile_id))}
    except Exception as exc:  # ProfileNotFoundError carries the available list
        return {"error": type(exc).__name__}

print(json.dumps({
    "has_profiles": registry.has_profiles(),
    "ids": [cps(profile.profile_id) for profile in profiles],
    "profiles": [describe(profile) for profile in profiles],
    "resolve_doctrine": resolution(registry.resolve, "reviewer-renata"),
    "resolve_local_doctrine": resolution(registry.resolve_local, "reviewer-renata"),
    "resolve_legacy": resolution(registry.resolve, "legacy-only"),
    "resolve_local_legacy": resolution(registry.resolve_local, "legacy-only"),
    "resolve_missing": resolution(registry.resolve, "no-such-profile"),
}))
"#;

fn run_source(fixture: &Fixture) -> Value {
    let output = Command::new(support::python())
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .env_clear()
        .envs(fixture.env())
        .current_dir(&fixture.project)
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
        .map(|code| u32::try_from(code.as_u64().expect("integer")).expect("u32"))
        .collect()
}

fn exact(value: &Value) -> PythonString {
    PythonString::from_codepoints(&points(value)).expect("valid id")
}

fn describe(profile: &AgentProfile) -> Value {
    let names = |values: Vec<PythonString>| -> Vec<Vec<u32>> {
        values
            .iter()
            .map(|value| value.codepoints().to_vec())
            .collect()
    };
    serde_json::json!({
        "id": profile.id.codepoints(),
        "name": profile.name().map(|value| value.codepoints().to_vec()).unwrap_or_default(),
        "description": profile.description().map(|value| value.codepoints().to_vec()).unwrap_or_default(),
        "role": profile.role().map(|value| value.codepoints().to_vec()).unwrap_or_default(),
        "routing_priority": profile.routing_priority(),
        "domain_keywords": names(profile.domain_keywords()),
        "canonical_verbs": names(profile.canonical_verbs()),
        "applies_to_languages": names(profile.applies_to_languages()),
    })
}

fn check(scenario: &Scenario) {
    let fixture = fixture(scenario);
    let source = run_source(&fixture);
    let registry = ProfileRegistry::load(&fixture.project)
        .unwrap_or_else(|error| panic!("{}: native catalogue failed: {error}", scenario.name));

    assert_eq!(
        registry.has_profiles(),
        source["has_profiles"].as_bool().expect("boolean"),
        "{}: has_profiles",
        scenario.name
    );
    let source_ids: Vec<PythonString> = source["ids"]
        .as_array()
        .expect("ids array")
        .iter()
        .map(exact)
        .collect();
    assert_eq!(
        registry.ids(),
        source_ids,
        "{}: catalogue ids",
        scenario.name
    );
    let native_profiles: Vec<Value> = source_ids
        .iter()
        .map(|id| describe(registry.get(id).expect("catalogued id")))
        .collect();
    assert_eq!(
        Value::Array(native_profiles),
        source["profiles"],
        "{}: every routing field",
        scenario.name
    );

    // `resolve` and `resolve_local` agree with the reference for a doctrine id.
    let doctrine = PythonString::from_scalars("reviewer-renata");
    if source["resolve_doctrine"].get("ok").is_some() {
        assert_eq!(
            describe(registry.resolve(&doctrine).expect("resolve")),
            source["resolve_doctrine"]["ok"]
        );
        assert_eq!(
            describe(registry.resolve_local(&doctrine).expect("resolve_local")),
            source["resolve_local_doctrine"]["ok"]
        );
    } else {
        assert!(matches!(
            registry.resolve(&doctrine),
            Err(CatalogueError::ProfileNotFound { .. })
        ));
    }
    // The legacy overlay is resolvable through both catalogues too.
    let legacy = PythonString::from_scalars("legacy-only");
    if source["resolve_legacy"].get("ok").is_some() {
        assert_eq!(
            describe(registry.resolve(&legacy).expect("legacy resolve")),
            source["resolve_legacy"]["ok"]
        );
        assert_eq!(
            describe(
                registry
                    .resolve_local(&legacy)
                    .expect("legacy resolve_local")
            ),
            source["resolve_local_legacy"]["ok"]
        );
        assert_eq!(
            registry.provenance(&legacy),
            Some("project"),
            "{}: legacy provenance",
            scenario.name
        );
    } else {
        assert!(matches!(
            registry.resolve(&legacy),
            Err(CatalogueError::ProfileNotFound { .. })
        ));
    }
    assert!(matches!(
        registry.resolve_str("no-such-profile"),
        Err(CatalogueError::ProfileNotFound { .. })
    ));

    // The legacy overlay wins the reviewer-renata collision where it exists.
    if scenario.legacy && source["resolve_doctrine"].get("ok").is_some() {
        let overlay = registry.resolve(&doctrine).expect("collision id");
        assert_eq!(
            overlay.name().map(|name| name.codepoints().to_vec()),
            Some(points(&source["resolve_doctrine"]["ok"]["name"])),
            "{}: legacy overlay wins the collision",
            scenario.name
        );
        assert_eq!(registry.provenance(&doctrine), Some("project"));
    }
    // A doctrine profile dropped by the language scope is reported exactly.
    if scenario.languages.is_some() {
        assert!(
            !registry.scope_filtered_exact_ids().is_empty(),
            "{}: expected a recorded language-scope drop",
            scenario.name
        );
    }
}

#[test]
fn plain_repository_matches_the_source_catalogue() {
    check(&SCENARIOS[0]);
}

#[test]
fn legacy_overlay_matches_the_source_catalogue() {
    check(&SCENARIOS[1]);
}

#[test]
fn activation_language_and_legacy_gates_match_the_source_catalogue() {
    check(&SCENARIOS[2]);
}

#[test]
fn an_explicitly_empty_activation_set_is_an_empty_catalogue() {
    check(&SCENARIOS[3]);
}

#[test]
fn a_source_valid_non_scalar_profile_is_catalogued_and_routable() {
    let scenario = &SCENARIOS[1];
    check(scenario);
    let fixture = fixture(scenario);
    let registry = ProfileRegistry::load(&fixture.project).expect("catalogue");
    let exact = PythonString::from_codepoints(&[0x61, 0x75, 0x74, 0x68, 0x6f, 0x72, 0xd800])
        .expect("valid id");
    assert_eq!(registry.non_scalar_ids(), vec![exact.clone()]);
    let profile = registry.profile(&exact).expect("catalogued exactly");
    assert_eq!(
        profile.name().unwrap().codepoints(),
        &[
            0x53u32, 0x75, 0x72, 0x72, 0x6f, 0x67, 0x61, 0x74, 0x65, 0x20, 0x41, 0x75, 0x74, 0x68,
            0x6f, 0x72
        ]
    );
    assert_eq!(registry.resolve(&exact).expect("resolve").id, exact);
    assert_eq!(
        registry.resolve_local(&exact).expect("resolve_local").id,
        exact
    );
    // It is a real routing candidate: the router view keeps its exact id.
    let routable = registry.routing_profile(&exact).expect("routable");
    assert_eq!(routable.profile_id, exact);
    assert_eq!(
        routable.role.codepoints(),
        &[0x72u32, 0x65, 0x76, 0x69, 0x65, 0x77, 0x65, 0x72]
    );
    let snapshot = registry.routing_profiles();
    assert_eq!(snapshot.len(), registry.ids().len());
    assert!(snapshot.iter().any(|profile| profile.profile_id == exact));
    assert!(
        registry
            .profiles()
            .iter()
            .any(|profile| profile.id == exact)
    );
    // The scalar `str` path is a capability boundary, not a substituted id.
    assert!(matches!(
        registry.resolve_str("author"),
        Err(CatalogueError::ProfileNotFound { .. })
    ));
}
