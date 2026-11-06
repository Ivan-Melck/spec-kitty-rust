//! CF13: a registered sparse org/pack mission type resolves through its
//! activation roster instead of falling back to "unregistered".
//!
//! Source `_build_neutral_org_mission` (T021/#3831): when no resolver tier has a
//! `mission.yaml` for a Mission's recorded `mission_type`, but the project has
//! ACTIVATED that type and a `mission_types/<id>.yaml` registration exists, the
//! Mission still resolves — as a neutral identity built from the registration
//! (`display_name`, `action_sequence`, and the registration's own
//! `path_conventions`). An unactivated type stays unresolved.
//!
//! These drive `sk_cli::compat::lifecycle::accept` (the dispatcher entry) so the
//! whole composition is exercised: activation ledger → layered roster →
//! Mission-definition resolver → collector → Source policy. `--diagnose` keeps
//! the run read-only, which is enough to observe the resolved facts.
//!
//! Native functional contracts; the pinned-Source differential run is listed as
//! outstanding in the task report.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_cli::compat::lifecycle::{AcceptArgs, accept};

const SLUG: &str = "acme-mission";
const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B68AA";
const TYPE: &str = "acme-flow";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The sparse registration: identity, action sequence and its OWN conventions.
fn registration() -> &'static str {
    "schema_version: 1\nid: acme-flow\ndisplay_name: Acme Flow\naction_sequence: [specify, plan, review]\npath_conventions:\n  workspace: docs/\n  deliverables: docs/\n"
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(activated: bool) -> Self {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("repo");
        git(&root, &["init", "-q", "-b", "main"]);
        write(&root.join("README.md"), "# Demo\n");
        let activations = if activated {
            "activated_kinds: []\nmission_type_activations: [acme-flow]\n"
        } else {
            "activated_kinds: []\nmission_type_activations: []\n"
        };
        write(&root.join(".kittify/charter/activation.yaml"), activations);
        // Project-layer registration: `<repo>/.kittify/missions/mission_types/<id>.yaml`.
        write(
            &root
                .join(".kittify/missions/mission_types")
                .join(format!("{TYPE}.yaml")),
            registration(),
        );
        let mission = root.join("kitty-specs").join(SLUG);
        std::fs::create_dir_all(mission.join("tasks")).expect("tasks dir");
        write(
            &mission.join("meta.json"),
            &serde_json::to_string_pretty(&json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "mission_number": 14,
                "mission_type": TYPE,
                "friendly_name": "Acme mission",
                "slug": SLUG,
                "target_branch": "main",
                "topology": "lanes",
            }))
            .unwrap(),
        );
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "acme mission"]);
        Self { _temp: temp, root }
    }
}

fn args() -> AcceptArgs {
    AcceptArgs {
        mission: Some(SLUG.to_owned()),
        mode: None,
        merge_commit: None,
        target_branch: None,
        attest_first_landing_commit: false,
        origin_check: None,
        lenient: false,
        diagnose: true,
        no_commit: false,
        actor: Some("operator".to_owned()),
        // CF20: Source defaults for the new attestation flags.
        attest_no_implementation: Vec::new(),
        attest_reason: None,
        json: false,
    }
}

fn strings(payload: &Value, key: &str) -> Vec<String> {
    payload[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn an_activated_sparse_type_resolves_with_its_own_path_conventions() {
    let fixture = Fixture::new(true);
    let (payload, code) = accept(&fixture.root, &args()).expect("diagnose runs");
    assert_eq!(code, 0, "diagnose reports: {payload}");
    // The neutral identity carries the registration's OWN conventions, so the
    // declared `docs/` paths are really evaluated (and a fresh mission with no
    // `docs/` is a strict violation) — not Source's "no Mission resolved" no-op.
    let violations = strings(&payload, "path_violations");
    assert!(
        !violations.is_empty(),
        "the registered conventions must be evaluated: {payload}"
    );
    assert!(
        violations.iter().any(|text| text.contains("docs")),
        "{violations:?}"
    );
    // A neutral identity declares NO artifacts, so no optional artifacts are
    // missing — the fallback list is only used when nothing resolved.
    assert!(
        strings(&payload, "optional_missing").is_empty(),
        "{payload}"
    );
}

#[test]
fn an_unactivated_type_stays_unresolved() {
    let fixture = Fixture::new(false);
    let (payload, code) = accept(&fixture.root, &args()).expect("diagnose runs");
    assert_eq!(code, 0, "diagnose reports: {payload}");
    // No Mission resolved: no declared paths to evaluate, and the optional-artifact
    // fact falls back to Source's `_FALLBACK_OPTIONAL_ARTIFACTS` list.
    assert!(strings(&payload, "path_violations").is_empty(), "{payload}");
    assert_eq!(
        strings(&payload, "optional_missing"),
        vec![
            "quickstart.md".to_owned(),
            "data-model.md".to_owned(),
            "research.md".to_owned(),
            "contracts".to_owned(),
        ],
        "{payload}"
    );
}

#[test]
fn a_registration_for_a_type_with_a_mission_yaml_does_not_shadow_it() {
    // The packaged `plan` mission exists at a higher-precedence tier than an
    // inline registration would be consulted for; the registration below is for a
    // DIFFERENT id, so the plan Mission must resolve its real `mission.yaml`.
    let fixture = Fixture::new(true);
    let mission_dir = fixture.root.join("kitty-specs").join(SLUG);
    write(
        &mission_dir.join("meta.json"),
        &serde_json::to_string_pretty(&json!({
            "mission_id": MISSION_ID,
            "mission_slug": SLUG,
            "mission_number": 14,
            "mission_type": "plan",
            "friendly_name": "Plan mission",
            "slug": SLUG,
            "target_branch": "main",
            "topology": "lanes",
        }))
        .unwrap(),
    );
    git(&fixture.root, &["add", "-A"]);
    git(&fixture.root, &["commit", "-q", "-m", "switch to plan"]);
    let (payload, code) = accept(&fixture.root, &args()).expect("diagnose runs");
    assert_eq!(code, 0, "{payload}");
    // The plan mission declares `docs/` too, but it also declares artifacts, so
    // the optional-artifact fact is the plan definition's own list — not the
    // registration's empty one and not the fallback.
    let optional = strings(&payload, "optional_missing");
    assert!(
        optional.iter().any(|entry| entry == "research.md"),
        "{payload}"
    );
    assert!(
        !optional.iter().any(|entry| entry == "quickstart.md"),
        "{payload}"
    );
}
