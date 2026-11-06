//! CF75 — Source eight-kind global-selection block and its prepared budget input.
//!
//! SOURCE FIRST: the parent writes one fixture project (built-in pack, one org
//! pack, one project layer), runs the pinned oracle Python child against the real
//! `charter.activation.context_renderers.selection_block` + `token_budget`
//! modules, then re-invokes this test as an isolated Native child which renders
//! the same block through `sk_doctrine::selection_block`. Both children assert the
//! temporary CWD/HOME/all-XDG/TMP roots, the independently supplied absolute
//! Source/packs roots, the hash seed and (Source) the exact interpreter BEFORE any
//! business read. The comparison is the library-owner pair, not a whole-CLI or
//! whole-text parity claim.

use serde_json::{Value, json};

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use sk_doctrine::text_budget::{BUDGET_DEFAULT, enforce_token_budget};
use sk_doctrine::{
    CanonicalOfferingService, GovernanceCharterSelection, IdentityIndex, OfferingContext,
    OfferingRoots, SelectionBlockInputs, render_selection_block,
};
use sk_kernel::PythonString;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT_ENV: &str = "CF75_PAIR_ROOT";
const REPO_ENV: &str = "CF75_PAIR_REPO";
const FIXTURE_ENV: &str = "CF75_PAIR_FIXTURE";
const MODE_ENV: &str = "CF75_CHILD_MODE";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

fn text(value: &str) -> PythonString {
    PythonString::from_scalars(value)
}

fn scalar(value: &PythonString) -> String {
    value.to_scalar_string().expect("scalar fixture text")
}

/// The seven XDG variables plus the three temporary-directory spellings.
fn env_roots() -> [(&'static str, &'static str); 12] {
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
        ("SK_CF75_ROOT", "root"),
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
        "child must run in its temporary root"
    );
    for (key, name) in env_roots() {
        let expected = if name == "root" {
            root.to_path_buf()
        } else {
            root.join(name)
        };
        assert_eq!(
            PathBuf::from(std::env::var_os(key).unwrap_or_default()),
            expected,
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
        .env("CF75_PAIR_REPO", repo)
        .env("CF75_PAIR_ROOT", root)
        .env("CF75_PAIR_FIXTURE", fixture);
    for (key, name) in env_roots() {
        let value = if name == "root" {
            root.to_path_buf()
        } else {
            root.join(name)
        };
        command.env(key, value);
    }
    command
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("fixture directory");
    fs::write(path, contents).expect("fixture file");
}

const DIRECTIVE: &str = concat!(
    "schema_version: \"1.0\"\n",
    "id: {id}\n",
    "title: {title}\n",
    "intent: {intent}\n",
    "enforcement: required\n",
);

/// The shared fixture: an org layer and a project layer whose ids the selection
/// names, so the chosen artifacts and their provenance are the same in both trees.
fn write_fixture(root: &Path) -> PathBuf {
    let fixture = root.join("fixture");
    let packs = fixture.join("packs/built-in");
    let org = fixture.join("org-pack");
    for directory in [
        "directives",
        "tactics",
        "styleguides",
        "toolguides",
        "procedures",
        "paradigms",
        "agent_profiles",
        "missions/built_in_step_contracts",
    ] {
        fs::create_dir_all(packs.join(directory)).expect("pack directory");
    }
    fs::create_dir_all(fixture.join(".kittify/charter")).expect("charter dir");
    write(
        &fixture,
        ".kittify/charter/charter.md",
        "# Fixture Charter\n",
    );
    // An org directive (small body) and a tactic (steps, so the repository serves it).
    write(
        &fixture,
        "org-pack/directives/777-org-overlay.directive.yaml",
        &DIRECTIVE
            .replace("{id}", "DIRECTIVE_777")
            .replace("{title}", "Org Overlay")
            .replace("{intent}", "Org intent stays."),
    );
    write(
        &fixture,
        "org-pack/org-charter-placeholder.yaml",
        "placeholder: true\n",
    );
    write(
        &fixture,
        "org-pack/org-charter.yaml",
        "required_directives:\n  - DIRECTIVE_777\nrequired_tactics:\n  - org-tactic\n",
    );
    write(
        &fixture,
        "org-pack/tactics/org-tactic.tactic.yaml",
        "schema_version: \"1.0\"\nid: org-tactic\nname: Org tactic\npurpose: Org purpose.\nsteps:\n  - title: Org step\n    description: Do the org thing.\n",
    );
    // A project directive whose file stem differs from its declared id, and a
    // project styleguide whose body overflows the per-entry limit.
    write(
        &fixture,
        ".kittify/charter-packs/directive/888-project.directive.yaml",
        &DIRECTIVE
            .replace("{id}", "DIRECTIVE_888")
            .replace("{title}", "Project Overlay")
            .replace("{intent}", "Project intent stays."),
    );
    write(
        &fixture,
        ".kittify/charter-packs/styleguide/docs-style.styleguide.yaml",
        "schema_version: \"1.0\"\nid: docs-style\ntitle: Docs Style\nscope: docs\nprinciples:\n  - One rule per page.\n",
    );
    // A project directive whose body overflows the per-entry limit: it must
    // render as the canonical fetch stanza instead of the verbatim body.
    write(
        &fixture,
        ".kittify/charter-packs/directive/889-long.directive.yaml",
        &DIRECTIVE
            .replace("{id}", "DIRECTIVE_889")
            .replace("{title}", "Long Directive")
            .replace("{intent}", &"y".repeat(2_600)),
    );
    write(
        &fixture,
        "org-pack/paradigms/org-paradigm.paradigm.yaml",
        "schema_version: \"1.0\"\nid: org-paradigm\nname: Org paradigm\nsummary: Org summary.\n",
    );
    write(
        &fixture,
        "org-pack/procedures/org-procedure.procedure.yaml",
        "schema_version: \"1.0\"\nid: org-procedure\nname: Org procedure\npurpose: Org purpose.\nentry_condition: starts\nexit_condition: ends\nsteps:\n  - title: Org step\n    description: Do it.\n",
    );
    write(
        &fixture,
        "org-pack/toolguides/org-toolguide.toolguide.yaml",
        "schema_version: \"1.0\"\nid: org-toolguide\ntool: probe\ntitle: Org Tool\nguide_path: org-tool.md\nsummary: Use the probe.\n",
    );
    write(
        &fixture,
        "org-pack/agent_profiles/org-profile.agent.yaml",
        "profile-id: org-profile\nname: Org Profile\nschema-version: \"1.0\"\nroles:\n  - implementer\npurpose: Org profile purpose.\nspecialization:\n  primary-focus: Org work.\n",
    );
    write(
        &fixture,
        "org-pack/mission_step_contracts/org-contract.step-contract.yaml",
        "schema_version: \"1.0\"\nid: org-contract\naction: audit\nmission: documentation\nsteps:\n  - id: bootstrap\n    description: Load the context\n",
    );
    write(
        &fixture,
        ".kittify/config.yaml",
        &format!(
            "charter_packs:\n  org:\n    packs:\n      - name: fixture-org\n        local_path: {}\n",
            org.display()
        ),
    );
    fixture
}

/// The selected ids both children render, in the reference's authored order.
const SELECTED_DIRECTIVES: [&str; 6] = [
    "DIRECTIVE_777",
    "888-project",
    "DIRECTIVE_777",
    "DIRECTIVE_999",
    "DIRECTIVE_024",
    "889-long",
];

fn native_selection() -> GovernanceCharterSelection {
    let exact = |values: &[&str]| values.iter().map(|value| text(value)).collect();
    GovernanceCharterSelection {
        selected_paradigms: exact(&["org-paradigm"]),
        selected_directives: exact(&SELECTED_DIRECTIVES),
        selected_tactics: exact(&["org-tactic"]),
        selected_styleguides: exact(&["docs-style"]),
        selected_toolguides: exact(&["org-toolguide"]),
        selected_procedures: exact(&["org-procedure"]),
        selected_agent_profiles: exact(&["org-profile"]),
        selected_mission_step_contracts: exact(&["org-contract"]),
        ..GovernanceCharterSelection::default()
    }
}

fn native_block(fixture: &Path) -> Value {
    let packs = fixture.join("packs/built-in");
    let org = fixture.join("org-pack");
    let project = fixture.join(".kittify/charter-packs");
    let context = OfferingContext::new(
        fixture.to_path_buf(),
        OfferingRoots {
            built_in: Some(packs),
            org_roots: vec![org.clone()],
            project: Some(project.clone()),
        },
        None,
    );
    let catalog = CanonicalOfferingService::new(context);
    // Source's identity built-in layer is the resolved pack tier, not the
    // repository's explicit directory; mirror that so both indexes agree.
    let identity =
        IdentityIndex::from_roots(&repo().join("packs/built-in"), &[org], Some(&project));
    let selection = native_selection();
    let captured = RefCell::new(Vec::<String>::new());
    let sink = |message: &str| captured.borrow_mut().push(message.to_owned());
    let inputs = SelectionBlockInputs {
        charter: Some(&selection),
        catalog: &catalog,
        identity: Some(&identity),
        repo_root: None,
        scope: None,
        warning_sink: Some(&sink),
    };
    let block = render_selection_block(&inputs).expect("selection block");
    let mut full = text("Charter Context (Bootstrap):\n\n");
    full.append(&block);
    full.push_scalars("\n\n");
    full.push_scalars(&"z".repeat(41_000));
    let budgeted = enforce_token_budget(
        &full,
        &text("implement"),
        &text(""),
        &text(""),
        &block,
        BUDGET_DEFAULT,
        None,
    );
    // The reference's empty-config short circuit, checked on the same catalogue.
    let empty = GovernanceCharterSelection::default();
    let empty_block = render_selection_block(&SelectionBlockInputs {
        charter: Some(&empty),
        catalog: &catalog,
        identity: Some(&identity),
        repo_root: None,
        scope: None,
        warning_sink: None,
    })
    .expect("empty selection block");
    json!({
        "block": scalar(&block),
        "budgeted": scalar(&budgeted),
        "warnings": captured.borrow().clone(),
        "empty": scalar(&empty_block),
    })
}

/// The Native child re-invocation. It is inert unless the parent sets the mode.
#[test]
fn cf75_native_selection_child() {
    let Ok(mode) = std::env::var(MODE_ENV) else {
        return;
    };
    assert_eq!(mode, "native");
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("root"));
    let repo = PathBuf::from(std::env::var_os(REPO_ENV).expect("repo"));
    let fixture = PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("fixture"));
    assert_child_environment(&root, &repo, &fixture);
    println!("CF75_JSON={}", native_block(&fixture));
}

/// The Source child script (written into the workspace).
const SOURCE_CHILD: &str = r#"
import json, os, sys, types, warnings
from pathlib import Path

root = Path(os.environ["CF75_PAIR_ROOT"])
repo = Path(os.environ["CF75_PAIR_REPO"])
fixture = Path(os.environ["CF75_PAIR_FIXTURE"])
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
assert (repo / "src" / "charter" / "activation" / "context_renderers" / "selection_block.py").is_file()
assert (repo / "packs" / "built-in" / "directives").is_dir()

from charter.activation.context_renderers import selection_block as sb
from charter.activation.context_renderers.token_budget import _enforce_token_budget
from charter.activation.identity_index import IdentityIndex
from charter.activation.schemas import GovernanceCharterConfig
from charter.offering.agent_profiles.repository import AgentProfileRepository
from charter.offering.directives.repository import DirectiveRepository
from charter.offering.missions.step_contracts import MissionStepContractRepository
from charter.offering.paradigms.repository import ParadigmRepository
from charter.offering.procedures.repository import ProcedureRepository
from charter.offering.styleguides.repository import StyleguideRepository
from charter.offering.tactics.repository import TacticRepository
from charter.offering.toolguides.repository import ToolguideRepository
from charter.offering.artifact_kinds import ArtifactKind

packs = fixture / "packs" / "built-in"
org = fixture / "org-pack"
project = fixture / ".kittify" / "charter-packs"
service = types.SimpleNamespace(
    paradigms=ParadigmRepository(built_in_dir=packs / "paradigms", org_dirs=[org / "paradigms"]),
    directives=DirectiveRepository(built_in_dir=packs / "directives", org_dirs=[org / "directives"], project_dir=project / "directive"),
    tactics=TacticRepository(built_in_dir=packs / "tactics", org_dirs=[org / "tactics"]),
    styleguides=StyleguideRepository(built_in_dir=packs / "styleguides", org_dirs=[org / "styleguides"], project_dir=project / "styleguide"),
    toolguides=ToolguideRepository(built_in_dir=packs / "toolguides", org_dirs=[org / "toolguides"]),
    procedures=ProcedureRepository(built_in_dir=packs / "procedures", org_dirs=[org / "procedures"]),
    agent_profiles=AgentProfileRepository(built_in_dir=packs / "agent_profiles", org_dirs=[org / "agent_profiles"]),
    mission_step_contracts=MissionStepContractRepository(built_in_dir=packs / "missions" / "built_in_step_contracts", org_dirs=[org / "mission_step_contracts"]),
)
identity = IdentityIndex(org_roots=[org], project_root=project, offering_root=repo / "packs" / "built-in")
config = GovernanceCharterConfig(
    selected_paradigms=["org-paradigm"],
    selected_directives=["DIRECTIVE_777", "888-project", "DIRECTIVE_777", "DIRECTIVE_999", "DIRECTIVE_024", "889-long"],
    selected_tactics=["org-tactic"],
    selected_styleguides=["docs-style"],
    selected_toolguides=["org-toolguide"],
    selected_procedures=["org-procedure"],
    selected_agent_profiles=["org-profile"],
    selected_mission_step_contracts=["org-contract"],
)
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    block = str(sb._render_selection_block(config, service, identity=identity))
    empty = str(sb._render_selection_block(GovernanceCharterConfig(), service, identity=identity))
    messages = [str(item.message) for item in caught]
full = "Charter Context (Bootstrap):\n\n" + block + "\n\n" + ("z" * 41000)
budgeted = _enforce_token_budget(
    full, action="implement", profile_block="", section_block="", selection_block=block
)
print("CF75_JSON=" + json.dumps({
    "block": block,
    "budgeted": budgeted,
    "warnings": messages,
    "empty": empty,
}))
"#;

fn run_child(mut command: Command, label: &str) -> Value {
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
        .find(|line| line.starts_with("CF75_JSON="))
        .unwrap_or_else(|| panic!("{label} child printed no CF75_JSON line:\n{stdout}"));
    serde_json::from_str(&line["CF75_JSON=".len()..]).expect("child JSON")
}

#[test]
fn cf75_selection_block_matches_source() {
    let workspace = tempfile::tempdir().expect("workspace");
    let repo = repo();

    let source_root = workspace.path().join("source-root");
    let native_root = workspace.path().join("native-root");
    for root in [&source_root, &native_root] {
        for (_, name) in env_roots() {
            if name == "root" {
                fs::create_dir_all(root).expect("root");
            } else {
                fs::create_dir_all(root.join(name)).expect("root subdir");
            }
        }
    }

    // Independent business trees as well as independent CWD/HOME/XDG/TMP:
    // neither child may observe the other's repository/cache/read side effects.
    let source_fixture = write_fixture(&source_root);
    let native_fixture = write_fixture(&native_root);
    let script = workspace.path().join("cf75_source_child.py");
    fs::write(&script, SOURCE_CHILD).expect("source child script");
    let mut source_command = Command::new(oracle::python());
    source_command = child_command(&source_root, &repo, &source_fixture, source_command);
    source_command.arg(&script);
    let source = run_child(source_command, "source");

    let mut native_command = child_command(
        &native_root,
        &repo,
        &native_fixture,
        Command::new(std::env::current_exe().expect("test binary")),
    );
    native_command
        .env(MODE_ENV, "native")
        .arg(std::env::current_exe().expect("test binary"))
        .args(["--exact", "cf75_native_selection_child", "--nocapture"]);
    let native = run_child(native_command, "native");

    assert_eq!(
        source["block"], native["block"],
        "the prepared eight-kind selection block must be byte-identical"
    );
    assert_eq!(
        source["warnings"], native["warnings"],
        "the catalogue-miss warning policy must match"
    );
    assert_eq!(
        source["budgeted"], native["budgeted"],
        "the shared budget engine must substitute this exact prepared block identically"
    );
    assert_eq!(source["empty"], "", "an empty config renders no block");
    assert_eq!(native["empty"], "");

    let block = source["block"].as_str().expect("block");
    // The eight kind headers, in the reference's order.
    let headers: Vec<&str> = block
        .lines()
        .filter(|line| line.starts_with("Selected "))
        .collect();
    assert_eq!(
        headers,
        vec![
            "Selected paradigms:",
            "Selected directives:",
            "Selected tactics:",
            "Selected styleguides:",
            "Selected toolguides:",
            "Selected procedures:",
            "Selected agent profiles:",
            "Selected mission step contracts:",
        ]
    );
    // The org layer is the only layer with a provenance suffix.
    assert_eq!(block.matches("(source: org)").count(), 7, "{block}");
    assert!(block.contains("  - DIRECTIVE_777 (source: org)"));
    assert!(block.contains("    Intent: Org intent stays."));
    // The authored stem is rendered verbatim; the declared id is the lookup key.
    assert!(block.contains("  - 888-project\n"), "{block}");
    assert!(block.contains("    Intent: Project intent stays."));
    // Authored duplicate collapsed to one entry; the unknown id keeps its stanza.
    assert_eq!(block.matches("  - DIRECTIVE_777 (source: org)").count(), 1);
    assert!(block.contains("(catalog entry not found for directive:DIRECTIVE_999)"));
    // A short styleguide body stays inline; the long directive body overflows the
    // per-entry limit and becomes the canonical fetch stanza.
    assert!(block.contains("    Title: Docs Style"));
    assert!(block.contains("Run: spec-kitty charter context --include directive:889-long"));
    assert!(!block.contains("yyyyyyyyyy"));
    // A tactic's steps keep their typed model titles/descriptions.
    assert!(block.contains("      - Org step\n        Do the org thing."));
    // The budget substitution keeps every id and one parameterized fetch per kind.
    let budgeted = source["budgeted"].as_str().expect("budgeted");
    assert!(budgeted.contains("Run: spec-kitty charter context --include directive:SELECTED_ID"));
    assert!(budgeted.contains("  - DIRECTIVE_777 (source: org)"));
    assert!(budgeted.contains("  - 888-project"));
    assert!(!budgeted.contains("Intent: Org intent stays."));
}
