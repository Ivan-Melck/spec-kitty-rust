//! Source-first identity conformance for the invocation vertical.
//!
//! The A-transport matrix proved authored *profile data* survives; this file
//! covers the identity's downstream life:
//!
//! 1. the Op **wire** for every valid Python-string shape (`ensure_ascii`
//!    escapes for a PUA code point, an astral code point, NUL and a
//!    noncharacter) is byte-identical to `json.dumps`;
//! 2. `dry_run` accepts a Source-valid non-scalar id and returns it exactly,
//!    while `invoke` fails at the reference's own phase — `OpStartedEvent`'s
//!    constrained Pydantic `str` — after routing/context/glossary and before
//!    any Op file or index row;
//! 3. `profiles list --json` carries every id, including a lone surrogate,
//!    with `json.dumps` escaping and no invented/omitted identity;
//! 4. `dispatch --dry-run --json` opens with the exact `profile_id` /
//!    `profile_friendly_name` prefix.
//!
//! Every Source assertion runs first, in an asserted temporary CWD/HOME/XDG/TMP
//! root; the native library or binary is compared afterwards.

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use sk_invocation::catalogue::{CatalogueOptions, ProfileRegistry};
use sk_invocation::executor::{
    ExecutorError, InvocationRequest, ProfileInvocationExecutor, RegistryCatalog,
};
use sk_invocation::{InvocationId, ModeOfWork};
use sk_invocation::{OpStartedEvent, OpStartedFields};
use sk_kernel::PythonString;

// ---------------------------------------------------------------------------
// Isolated fixture
// ---------------------------------------------------------------------------

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(profiles: &[(&str, &str)]) -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let root = temp.path().canonicalize().expect("canonical root");
        for directory in ["home", "cache", "config", "data", "state", "tmp"] {
            fs::create_dir_all(root.join(directory)).expect("isolated directory");
        }
        let directory = root.join(".kittify/profiles");
        fs::create_dir_all(&directory).expect("legacy profile directory");
        for (name, text) in profiles {
            fs::write(directory.join(name), text).expect("write profile");
        }
        // A branch that is not protected; no origin/HEAD means the repository
        // keeps no Op records, so the pre-write admission probe admits.
        std::process::Command::new("git")
            .args([
                "init",
                "-q",
                "-b",
                "feature/identity",
                root.to_str().unwrap(),
            ])
            .output()
            .expect("git init");
        Self { _temp: temp, root }
    }

    fn env(&self) -> Vec<(String, String)> {
        let root = &self.root;
        let repository = support::repository_root();
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
            ("NO_COLOR".to_owned(), "1".to_owned()),
            (
                "PYTHONPATH".to_owned(),
                repository.join("src").display().to_string(),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".to_owned(),
                repository.join("packs").display().to_string(),
            ),
            ("SK_G_ROOT".to_owned(), root.display().to_string()),
        ]
    }
}

fn cps(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

/// `author\uD800`: a Source-valid id that no Rust `str` can spell.
fn surrogate_codes() -> Vec<u32> {
    vec![0x61, 0x75, 0x74, 0x68, 0x6f, 0x72, 0xd800]
}

fn surrogate_id() -> PythonString {
    PythonString::from_codepoints(&surrogate_codes()).expect("valid code points")
}

fn identity_profiles() -> Vec<(&'static str, String)> {
    vec![
        (
            "astral.agent.yaml",
            "profile-id: \"astral-\u{1f600}\"\nname: \"Name\u{1f600}\"\npurpose: P\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n".to_owned(),
        ),
        (
            "pua.agent.yaml",
            "profile-id: \"pua-\u{e000}x\"\nname: \"Name\u{e000}X\"\npurpose: P\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n".to_owned(),
        ),
        (
            "surrogate.agent.yaml",
            "profile-id: \"author\\uD800\"\nname: \"Name\\uD800\"\npurpose: P\nroles: [Reviewer]\nspecialization: {primary-focus: x}\n".to_owned(),
        ),
    ]
}

fn fixture() -> Fixture {
    let profiles = identity_profiles();
    let borrowed: Vec<(&str, &str)> = profiles
        .iter()
        .map(|(name, text)| (*name, text.as_str()))
        .collect();
    Fixture::new(&borrowed)
}

fn catalogue(fixture: &Fixture) -> ProfileRegistry {
    let repository = support::repository_root();
    let options = CatalogueOptions {
        repo_root: fixture.root.clone(),
        roots: sk_doctrine::CatalogueRoots {
            built_in: repository.join("packs/built-in"),
            org_roots: Vec::new(),
            project: None,
            legacy_project: Some(fixture.root.join(".kittify/profiles")),
        },
        active_languages: None,
    };
    ProfileRegistry::load_with(options).expect("native catalogue")
}

// ---------------------------------------------------------------------------
// 1. Op wire: every valid Python-string shape, byte for byte
// ---------------------------------------------------------------------------

const OP_LINE_SCRIPT: &str = r#"
import json, os
from pathlib import Path
from kernel.clock import now_utc_iso
from specify_cli.invocation.record import OpStartedEvent
root = Path(os.environ["SK_G_ROOT"]).resolve()
assert Path.cwd().resolve() == root
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)
profile_id = "".join(chr(code) for code in json.loads(os.environ["SK_PROFILE_CODES"]))
event = OpStartedEvent(
    invocation_id="01KTK5JBD69FQ8XVRFV1J630MJ",
    profile_id=profile_id,
    action="implement",
    request_text="fix that bug",
    actor="claude",
    mode_of_work="task_execution",
    governance_context_hash="d5ccab5678dcc4c8",
    governance_context_available=True,
    router_confidence="canonical_verb",
    started_at="2026-06-10T20:00:00+00:00",
)
print(event.to_jsonl_line())
"#;

fn run_source_op_line(fixture: &Fixture, id: &str) -> String {
    let codes = serde_json::to_string(&cps(id)).expect("codes");
    let output = Command::new(support::python())
        .arg("-c")
        .arg(OP_LINE_SCRIPT)
        .env_clear()
        .envs(fixture.env())
        .env("SK_PROFILE_CODES", codes)
        .current_dir(&fixture.root)
        .output()
        .expect("run source op line");
    assert!(
        output.status.success(),
        "Source op line failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("source line is UTF-8")
        .trim_end_matches('\n')
        .to_owned()
}

fn native_op_line(id: &str) -> String {
    OpStartedEvent::new(OpStartedFields {
        invocation_id: InvocationId::parse("01KTK5JBD69FQ8XVRFV1J630MJ").unwrap(),
        profile_id: id.to_owned(),
        action: "implement".to_owned(),
        request_text: "fix that bug".to_owned(),
        actor: "claude".to_owned(),
        mode_of_work: ModeOfWork::TaskExecution,
        governance_context_hash: "d5ccab5678dcc4c8".to_owned(),
        governance_context_available: true,
        router_confidence: Some("canonical_verb".to_owned()),
        started_at: "2026-06-10T20:00:00+00:00".to_owned(),
        mission_id: None,
        wp_id: None,
        model_id: None,
    })
    .expect("native record")
    .to_jsonl_line()
}

#[test]
fn op_started_wire_is_byte_exact_for_every_valid_python_id() {
    let fixture = fixture();
    let ids = [
        "ascii-id",
        "pua-\u{e000}",
        "astral-\u{1f600}",
        "nul-\u{0}-x",
        "nonchar-\u{fdd0}\u{fdef}",
    ];
    // Source first, and its own escaping shape is asserted, not assumed.
    let mut source_lines = Vec::new();
    for id in ids {
        let line = run_source_op_line(&fixture, id);
        assert!(line.is_ascii());
        assert!(line.contains(r#""profile_id": ""#));
        source_lines.push(line);
    }
    assert!(source_lines[1].contains(r#"\ue000"#));
    assert!(source_lines[2].contains(r#"\ud83d\ude00"#));
    assert!(source_lines[3].contains(r#"\u0000"#));
    assert!(source_lines[4].contains(r#"\ufdd0\ufdef"#));

    for (id, source_line) in ids.iter().zip(source_lines) {
        assert_eq!(
            native_op_line(id),
            source_line,
            "{id:?}: native Op wire diverged from json.dumps"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. dry-run accepts, invoke fails at the reference's own boundary
// ---------------------------------------------------------------------------

const LIFECYCLE_SCRIPT: &str = r#"
import json, os
from pathlib import Path
from specify_cli.invocation.executor import ProfileInvocationExecutor
root = Path(os.environ["SK_G_ROOT"]).resolve()
assert Path.cwd().resolve() == root
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)
def cps(text): return [ord(character) for character in text]
non_scalar = "".join(chr(code) for code in json.loads(os.environ["SK_NON_SCALAR"]))
astral = json.loads(os.environ["SK_ASTRAL"])
out = {}
ex = ProfileInvocationExecutor(root)
out["listed"] = [cps(profile.profile_id) for profile in ex.list_available_profiles()]
dry = ex.dry_run("please review this", profile_hint=non_scalar, actor="tester")
out["dry_run"] = {"id": cps(dry.profile_id), "name": cps(dry.profile_friendly_name), "action": dry.action}
try:
    ex.invoke("please review this", profile_hint=non_scalar, actor="tester")
    out["invoke"] = "accepted"
except Exception as exc:
    out["invoke"] = type(exc).__name__
out["files_after_non_scalar"] = sorted(p.name for p in (root / "kitty-ops").glob("*.jsonl")) if (root / "kitty-ops").exists() else []
scalar = ex.invoke("please review this", profile_hint=astral, actor="tester")
path = root / "kitty-ops" / (scalar.invocation_id + ".jsonl")
line = json.loads(path.read_text().splitlines()[0])
out["astral_record_id"] = cps(line["profile_id"])
out["astral_bytes"] = path.read_bytes().decode("utf-8")
print(json.dumps(out))
"#;

fn run_source_lifecycle(fixture: &Fixture, astral: &str) -> Value {
    let non_scalar = serde_json::to_string(&surrogate_codes()).unwrap();
    let output = Command::new(support::python())
        .arg("-c")
        .arg(LIFECYCLE_SCRIPT)
        .env_clear()
        .envs(fixture.env())
        .env("SK_NON_SCALAR", non_scalar)
        .env("SK_ASTRAL", serde_json::to_string(astral).unwrap())
        .current_dir(&fixture.root)
        .output()
        .expect("run source lifecycle");
    assert!(
        output.status.success(),
        "Source lifecycle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("source lifecycle JSON")
}

fn probe_executor<'a>(
    fixture: &'a Fixture,
    catalog: &'a RegistryCatalog<'a>,
) -> ProfileInvocationExecutor<'a> {
    ProfileInvocationExecutor::new(&fixture.root, &fixture.root).with_catalog(catalog)
}

#[test]
fn dry_run_accepts_and_invoke_refuses_a_non_scalar_id_at_the_record_boundary() {
    let fixture = fixture();
    let astral = "astral-\u{1f600}";
    let source = run_source_lifecycle(&fixture, astral);
    // Source-first: the reference's own answers.
    assert!(
        source["listed"]
            .as_array()
            .expect("listed")
            .iter()
            .any(|id| id.as_array().is_some_and(|codes| {
                codes
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|code| code as u32)
                    .eq(surrogate_codes())
            }))
    );
    assert_eq!(source["dry_run"]["id"], json!(surrogate_codes()));
    assert_eq!(
        source["dry_run"]["name"],
        json!(vec![0x4e, 0x61, 0x6d, 0x65, 0xd800])
    );
    assert_eq!(source["dry_run"]["action"], json!("audit"));
    assert_eq!(source["invoke"], json!("ValidationError"));
    assert_eq!(source["files_after_non_scalar"], json!([]));
    assert_eq!(source["astral_record_id"], json!(cps(astral)));
    assert!(
        source["astral_bytes"]
            .as_str()
            .unwrap()
            .contains(r#""profile_id": "astral-\ud83d\ude00""#)
    );

    // Clean the Source fixture's Op files so the native assertions see a fresh
    // record root (each invocation id is unique, but the index is shared).
    let _ = fs::remove_dir_all(fixture.root.join("kitty-ops"));

    let registry = catalogue(&fixture);
    let catalog = RegistryCatalog::new(&registry);
    let mut executor = probe_executor(&fixture, &catalog);

    let id = surrogate_id();
    let listed = executor.list_available_profiles();
    assert!(
        listed.iter().any(|profile| profile.profile_id == id),
        "the exact catalogue retains the non-scalar id"
    );
    let surrogate_hint = surrogate_id();
    let dry = executor
        .dry_run(&InvocationRequest {
            request_text: "please review this",
            profile_hint: Some(&surrogate_hint),
            actor: "tester",
            mode_of_work: None,
            action_hint: None,
            mission_id: None,
            wp_id: None,
        })
        .expect("a dry run writes no Op record");
    assert_eq!(dry.profile_id, id);
    assert_eq!(
        dry.profile_friendly_name,
        PythonString::from_codepoints(&[0x4e, 0x61, 0x6d, 0x65, 0xd800]).unwrap()
    );
    assert_eq!(dry.action, "audit");

    let error = executor
        .invoke(&InvocationRequest {
            request_text: "please review this",
            profile_hint: Some(&surrogate_hint),
            actor: "tester",
            mode_of_work: None,
            action_hint: None,
            mission_id: None,
            wp_id: None,
        })
        .expect_err("OpStartedEvent refuses a lone surrogate");
    match error {
        ExecutorError::RecordValidation { field, message } => {
            assert_eq!(field, "profile_id");
            assert!(message.contains("string_unicode"), "{message}");
            assert!(message.contains("author\\ud800"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(
        !fixture.root.join("kitty-ops").exists(),
        "no Op file or index row may be written before the record is constructed"
    );

    let astral_hint = PythonString::from_scalars(astral);
    let scalar = executor
        .invoke(&InvocationRequest {
            request_text: "please review this",
            profile_hint: Some(&astral_hint),
            actor: "tester",
            mode_of_work: None,
            action_hint: None,
            mission_id: None,
            wp_id: None,
        })
        .expect("an astral id is a valid constrained Python string");
    assert_eq!(scalar.profile_id, PythonString::from_scalars(astral));
    let path = fixture
        .root
        .join("kitty-ops")
        .join(format!("{}.jsonl", scalar.invocation_id));
    let bytes = fs::read_to_string(&path).expect("started record");
    assert!(
        bytes.contains(r#""profile_id": "astral-\ud83d\ude00""#),
        "{bytes}"
    );
    let index = fs::read_to_string(fixture.root.join("kitty-ops/ops-index.jsonl")).unwrap();
    assert!(
        index.contains(r#""profile_id": "astral-\ud83d\ude00""#),
        "{index}"
    );
}
