//! CF74 tiny **paired** control: the two real step-model consumers.
//!
//! The parent writes two independent business fixtures, runs the pinned Source
//! child (real `Tactic`/`Procedure` models + real
//! `artifact_bodies._format_inline_*` and `profile_sections.format_inline_named_body`)
//! and re-executes this binary as an isolated Native child over its own fixture
//! (canonical offering repository + `render_profile_sections` +
//! `include_body_lines_exact`). Both children assert their temporary
//! CWD/HOME/all-XDG/TMP roots, the independently supplied absolute Source and
//! packs roots, the hash seed and the filesystem codec before any business read;
//! the Source child also asserts the exact interpreter and that its real models
//! came from the real module.
//!
//! Scope: the step lines of both consumers on one authored record shape. Not
//! whole-text parity, not whole-CLI, not an independent review.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};
use sk_doctrine::context_render::include_body_lines_exact;
use sk_doctrine::{
    CanonicalOfferingService, DrgEdge, DrgGraph, DrgNode, NodeKind, OfferingContext, OfferingRoots,
    ProfileRepository, ProfileSectionsInputs, Relation, exact_payload_value,
    render_profile_sections,
};
use sk_kernel::PythonString;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

const PAIR_ROOT: &str = "CF74_PAIR_ROOT";
const PAIR_REPO: &str = "CF74_PAIR_REPO";
const PAIR_FIXTURE: &str = "CF74_PAIR_FIXTURE";
const CHILD_MODE: &str = "CF74_CHILD_MODE";

fn scalar(value: &PythonString) -> String {
    value.to_scalar_string().expect("scalar fixture text")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

/// The seven XDG variables plus HOME and the three temporary-directory spellings.
fn env_roots() -> [(&'static str, &'static str); 11] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CONFIG_DIRS", "config_dirs"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_DATA_DIRS", "data_dirs"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}

fn assert_child_environment(root: &Path, repo: &Path, fixture: &Path) {
    assert!(repo.is_absolute(), "absolute Source repo");
    assert!(fixture.is_absolute(), "absolute fixture");
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    assert_eq!(
        std::env::current_dir().expect("child cwd"),
        root,
        "child must run in its own temporary root"
    );
    for (key, name) in env_roots() {
        assert_eq!(
            PathBuf::from(std::env::var_os(key).unwrap_or_default()),
            root.join(name),
            "{key}"
        );
    }
    assert_eq!(
        PathBuf::from(std::env::var_os("PYTHONPATH").expect("absolute PYTHONPATH")),
        repo.join("src")
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("SPEC_KITTY_PACKS_ROOT").expect("packs root")),
        repo.join("packs")
    );
    assert_eq!(
        std::env::var_os("PYTHONHASHSEED").as_deref(),
        Some(std::ffi::OsStr::new("0"))
    );
}

fn child_command(root: &Path, repo: &Path, fixture: &Path, mut command: Command) -> Command {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .env(PAIR_REPO, repo)
        .env(PAIR_ROOT, root)
        .env(PAIR_FIXTURE, fixture);
    for (key, name) in env_roots() {
        command.env(key, root.join(name));
    }
    command
}

/// The independent business fixture one child reads (never shared with the other).
fn write_fixture(root: &Path) -> PathBuf {
    let fixture = root.join("fixture");
    let pack = fixture.join("packs/built-in");
    for directory in ["tactics", "procedures", "agent_profiles"] {
        fs::create_dir_all(pack.join(directory)).expect("pack directory");
    }
    fs::create_dir_all(fixture.join(".kittify/charter")).expect("charter dir");
    fs::write(
        fixture.join(".kittify/charter/charter.md"),
        "# Fixture Charter\n",
    )
    .expect("charter");
    fs::write(
        pack.join("tactics/cf74-echo.tactic.yaml"),
        r#"schema_version: "1.0"
id: cf74-echo
name: Cf74 Echo
purpose: Echo the steps.
steps:
  - title: ""
    description:
  - title: ""
    description: described
  - title: Only title
  - title: ""
    description: ""
    examples:
      - one
    references:
      - name: Architectural Integrity
        type: directive
        id: DIRECTIVE_001
        when: always
"#,
    )
    .expect("echo tactic");
    fs::write(
        pack.join("procedures/cf74-run.procedure.yaml"),
        r#"schema_version: "1.0"
id: cf74-run
name: Cf74 Run
purpose: Run the steps.
entry_condition: Entry.
exit_condition: Exit.
steps:
  - title: ""
    description: ""
  - title: Procedure step
    description: Do the step.
"#,
    )
    .expect("run procedure");
    fs::write(
        pack.join("agent_profiles/cf74-profile.agent.yaml"),
        r#"profile-id: cf74-profile
name: Cf74 Profile
schema-version: "1.0"
roles:
  - implementer
purpose: Cf74 purpose.
specialization:
  primary-focus: Echoing.
tactic-references:
  - id: cf74-echo
    rationale: echo rationale
"#,
    )
    .expect("cf74 profile");
    fixture
}

fn graph() -> DrgGraph {
    let node = |urn: &str, kind: NodeKind| DrgNode::new(urn, kind, None, Vec::new()).expect("node");
    DrgGraph::new(
        "1.0",
        "2026-11-06T00:00:00Z",
        "cf74-pair",
        vec![
            node("agent_profile:cf74-profile", NodeKind::AgentProfile),
            node("tactic:cf74-echo", NodeKind::Tactic),
            node("procedure:cf74-run", NodeKind::Procedure),
        ],
        vec![
            DrgEdge::new(
                "agent_profile:cf74-profile",
                "tactic:cf74-echo",
                Relation::Requires,
            )
            .expect("edge"),
            DrgEdge::new(
                "agent_profile:cf74-profile",
                "procedure:cf74-run",
                Relation::Requires,
            )
            .expect("edge"),
        ],
    )
    .expect("graph")
}

/// The `      - ` step headers and their `        ` continuation lines.
fn step_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.starts_with("      - ") || line.starts_with("        "))
        .map(str::to_owned)
        .collect()
}

fn offering_for(fixture: &Path) -> (PathBuf, CanonicalOfferingService) {
    let pack = fixture.join("packs/built-in");
    let catalog = CanonicalOfferingService::new(OfferingContext::new(
        fixture.to_path_buf(),
        OfferingRoots {
            built_in: Some(pack.clone()),
            org_roots: Vec::new(),
            project: None,
        },
        Some(vec!["ruby".to_owned()]),
    ));
    (pack, catalog)
}

/// The Native child's view: prepared profile step lines and inline include bodies.
fn native_pair_views(fixture: &Path) -> JsonValue {
    let (pack, catalog) = offering_for(fixture);
    let graph = graph();
    let profiles = ProfileRepository::load(&pack, &[], None).expect("profiles");
    let profile = profiles
        .resolve_str("cf74-profile", &graph)
        .expect("profile");
    let inputs = ProfileSectionsInputs {
        profile: &profile,
        graph: &graph,
        catalog: &catalog,
        scope: None,
        warning_sink: None,
    };
    let block = render_profile_sections(&inputs)
        .expect("profile block")
        .to_scalar_string()
        .expect("scalar profile block");
    let mut include = Vec::new();
    for (kind, file, artifact_kind) in [
        (
            "tactic",
            "tactics/cf74-echo.tactic.yaml",
            sk_charter::ArtifactKind::Tactic,
        ),
        (
            "procedure",
            "procedures/cf74-run.procedure.yaml",
            sk_charter::ArtifactKind::Procedure,
        ),
    ] {
        let source = fs::read_to_string(pack.join(file)).expect("fixture read");
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&source).expect("fixture yaml");
        let payload = exact_payload_value(artifact_kind, &value).expect("payload");
        include.push(json!({
            "kind": kind,
            "lines": include_body_lines_exact(kind, &payload)
                .iter()
                .map(scalar)
                .collect::<Vec<_>>(),
        }));
    }
    json!({
        "profile_step_lines": step_lines(&block),
        "include": include,
    })
}

/// The Native child re-invocation. Inert unless the parent sets the mode.
#[test]
fn cf74_native_pair_child() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    assert_eq!(mode, "native");
    let root = PathBuf::from(std::env::var_os(PAIR_ROOT).expect("root"));
    let repo = PathBuf::from(std::env::var_os(PAIR_REPO).expect("repo"));
    let fixture = PathBuf::from(std::env::var_os(PAIR_FIXTURE).expect("fixture"));
    assert_child_environment(&root, &repo, &fixture);
    println!("CF74_JSON={}", native_pair_views(&fixture));
}

/// The Source child: the real offering models and the real consumer formatters.
const SOURCE_CHILD: &str = r#"
import json, os, sys
from pathlib import Path

root = Path(os.environ["CF74_PAIR_ROOT"])
repo = Path(os.environ["CF74_PAIR_REPO"])
fixture = Path(os.environ["CF74_PAIR_FIXTURE"])
for name in ("HOME","XDG_CONFIG_HOME","XDG_CONFIG_DIRS","XDG_CACHE_HOME",
             "XDG_DATA_HOME","XDG_DATA_DIRS","XDG_STATE_HOME","XDG_RUNTIME_DIR",
             "TMPDIR","TMP","TEMP"):
    value = os.environ.get(name)
    assert value and Path(value).is_absolute(), name
assert Path.cwd() == root, (Path.cwd(), root)
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
assert os.environ["PYTHONHASHSEED"] == "0"
assert sys.version.split()[0] == "3.11.15", sys.version
assert sys.getfilesystemencoding() == "utf-8", sys.getfilesystemencoding()
assert sys.getfilesystemencodeerrors() == "surrogateescape"
assert os.path.realpath(sys.prefix).endswith(".venv-rust-oracle")

import yaml
from charter.activation.context_renderers.artifact_bodies import (
    _format_inline_procedure_body,
    _format_inline_tactic_body,
)
from charter.activation.context_renderers.profile_sections import format_inline_named_body
from charter.offering.procedures.models import Procedure
from charter.offering.tactics.models import Tactic

pack = fixture / "packs" / "built-in"
tactic = Tactic.model_validate(yaml.safe_load((pack / "tactics" / "cf74-echo.tactic.yaml").read_text()))
procedure = Procedure.model_validate(
    yaml.safe_load((pack / "procedures" / "cf74-run.procedure.yaml").read_text())
)
assert type(tactic).__module__ == "charter.offering.tactics.models"
assert type(procedure).__module__ == "charter.offering.procedures.models"

def step_lines(lines):
    return [l for l in lines if l.startswith("      - ") or l.startswith("        ")]

profile_step_lines = step_lines(format_inline_named_body(tactic)) + step_lines(
    format_inline_named_body(procedure)
)
print("CF74_JSON=" + json.dumps({
    "profile_step_lines": profile_step_lines,
    "include": [
        {"kind": "tactic", "lines": _format_inline_tactic_body(tactic)},
        {"kind": "procedure", "lines": _format_inline_procedure_body(procedure)},
    ],
}))
"#;

fn run_child(mut command: Command, label: &str) -> JsonValue {
    let output = command.output().expect("child runs");
    assert!(
        output.status.success(),
        "{label} child failed: status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| line.starts_with("CF74_JSON="))
        .unwrap_or_else(|| panic!("{label} child printed no CF74_JSON line:\n{stdout}"));
    serde_json::from_str(&line["CF74_JSON=".len()..]).expect("child JSON")
}

#[test]
fn cf74_step_model_consumers_match_the_pinned_source_children() {
    let workspace = tempfile::tempdir().expect("workspace");
    let repo = repo_root();
    let source_root = workspace.path().join("source-root");
    let native_root = workspace.path().join("native-root");
    for root in [&source_root, &native_root] {
        fs::create_dir_all(root).expect("root");
        for (_, name) in env_roots() {
            fs::create_dir_all(root.join(name)).expect("root subdir");
        }
    }
    // Independent business trees: neither child observes the other's fixture.
    let source_fixture = write_fixture(&source_root);
    let native_fixture = write_fixture(&native_root);
    let script = workspace.path().join("cf74_source_child.py");
    fs::write(&script, SOURCE_CHILD).expect("source child script");

    let mut source_command = Command::new(oracle::python());
    source_command = child_command(&source_root, &repo, &source_fixture, source_command);
    source_command.arg(&script);
    let source = run_child(source_command, "source");

    let executable = std::env::current_exe().expect("test binary");
    let mut native_command = child_command(
        &native_root,
        &repo,
        &native_fixture,
        Command::new(&executable),
    );
    native_command
        .env(CHILD_MODE, "native")
        .arg(&executable)
        .args(["--exact", "cf74_native_pair_child", "--nocapture"]);
    let native = run_child(native_command, "native");

    assert_eq!(
        source["include"], native["include"],
        "the inline include step lines must match the Source formatter"
    );
    assert_eq!(
        source["profile_step_lines"], native["profile_step_lines"],
        "the prepared profile block's step lines must match the Source formatter"
    );
    // The step under discussion: a falsy title/description step is the model
    // `str(step)`, never a JSON rendering.
    let lines = native["profile_step_lines"]
        .as_array()
        .expect("step lines")
        .iter()
        .filter_map(JsonValue::as_str)
        .collect::<Vec<_>>();
    assert!(
        lines
            .iter()
            .any(|line| line == &"      - title='' description=None examples=[] references=[]"),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line
            == &"      - title='' description='' examples=['one'] references=[TacticReference(name='Architectural Integrity', type=<ArtifactKind.DIRECTIVE: 'directive'>, id='DIRECTIVE_001', when='always')]"),
        "{lines:?}"
    );
}
