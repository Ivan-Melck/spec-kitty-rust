//! CF73 — Source profile-cited sections and the prepared profile block.
//!
//! SOURCE FIRST: the parent writes one fixture project, runs the pinned oracle
//! Python child against the real
//! `charter.activation.context_renderers.profile_sections` /
//! `catalog_diagnosis` modules, then re-invokes this test as an isolated Native
//! child which renders the same blocks through
//! `sk_doctrine::profile_sections`. Both children assert the temporary
//! CWD/HOME/all-XDG/TMP roots, the independently supplied absolute Source and
//! packs roots, the hash seed and (Source) the exact interpreter BEFORE any
//! business read. The child comparison is the library owner pair, not a
//! whole-CLI/whole-text parity claim.

use serde_json::{Value, json};

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use sk_charter::ArtifactKind;
use sk_doctrine::context_render::{RenderInputs, render_compact_exact};
use sk_doctrine::text_budget::{BUDGET_DEFAULT, enforce_token_budget};
use sk_doctrine::{
    CanonicalOfferingService, CharterContext, ContextMode, DirectiveEntry, DrgEdge, DrgGraph,
    DrgNode, NodeKind, OfferingContext, OfferingRoots, OrgCharterBlock, ProfileRepository,
    ProfileSectionsInputs, ProjectCharterBlock, Relation, format_profile_directive_code,
    render_profile_sections,
};
use sk_kernel::PythonString;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT_ENV: &str = "CF73_PAIR_ROOT";
const REPO_ENV: &str = "CF73_PAIR_REPO";
const FIXTURE_ENV: &str = "CF73_PAIR_FIXTURE";
const MODE_ENV: &str = "CF73_CHILD_MODE";

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
        ("SK_CF73_ROOT", "root"),
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
        .env("CF73_PAIR_REPO", repo)
        .env("CF73_PAIR_ROOT", root)
        .env("CF73_PAIR_FIXTURE", fixture);
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

/// Write the shared fixture pack and project used by both children.
fn write_fixture(root: &Path) -> PathBuf {
    let fixture = root.join("fixture");
    let pack = fixture.join("packs/built-in");
    for directory in [
        "directives",
        "tactics",
        "styleguides",
        "toolguides",
        "procedures",
        "paradigms",
        "agent_profiles",
    ] {
        fs::create_dir_all(pack.join(directory)).expect("pack directory");
    }
    fs::create_dir_all(fixture.join(".kittify/charter")).expect("charter dir");
    fs::write(
        fixture.join(".kittify/charter/charter.md"),
        "# Fixture Charter\n",
    )
    .expect("charter");
    fs::write(
        pack.join("directives/010-alpha.directive.yaml"),
        r#"schema_version: "1.0"
id: DIRECTIVE_010
title: Alpha Directive
intent: Keep the alpha behavior faithful.
enforcement: required
scope: Applies to alpha work.
procedures:
  - Trace the change to its requirement.
integrity_rules:
  - No unrecorded drift.
validation_criteria:
  - Notes map behavior to the specification.
"#,
    )
    .expect("alpha directive");
    let beta_intent = "x".repeat(2_500);
    fs::write(
        pack.join("directives/024-beta.directive.yaml"),
        format!(
            "schema_version: \"1.0\"\nid: DIRECTIVE_024\ntitle: Beta Directive\nintent: {beta_intent}\nenforcement: required\n"
        ),
    )
    .expect("beta directive");
    fs::write(
        pack.join("tactics/gamma-tactic.tactic.yaml"),
        r#"schema_version: "1.0"
id: gamma-tactic
name: Gamma Tactic
purpose: Do the gamma thing.
steps:
  - title: First step
    description: Do the first thing.
  - title: ""
    description: Only a description.
"#,
    )
    .expect("gamma tactic");
    fs::write(
        pack.join("tactics/delta-tactic.tactic.yaml"),
        r#"schema_version: "1.0"
id: delta-tactic
name: Delta Tactic
purpose: Do the delta thing.
steps:
  - title: Delta step
"#,
    )
    .expect("delta tactic");
    fs::write(
        pack.join("styleguides/docs-style.styleguide.yaml"),
        r#"schema_version: "1.0"
id: docs-style
title: Docs Style
scope: docs
principles:
  - Write one concern per page.
"#,
    )
    .expect("styleguide");
    fs::write(
        pack.join("styleguides/scoped-style.styleguide.yaml"),
        r#"schema_version: "1.0"
id: scoped-style
title: Scoped Style
scope: code
principles:
  - Keep the scoped rule.
applies_to_languages:
  - python
"#,
    )
    .expect("scoped styleguide");
    fs::write(
        pack.join("toolguides/tool-guide.toolguide.yaml"),
        r#"schema_version: "1.0"
id: tool-guide
tool: probe
title: Tool Guide
guide_path: tool-guide.md
summary: Use the probe tool.
"#,
    )
    .expect("toolguide");
    fs::write(
        pack.join("procedures/alpha-proc.procedure.yaml"),
        r#"schema_version: "1.0"
id: alpha-proc
name: Alpha Procedure
purpose: Run the alpha procedure.
entry_condition: Alpha starts.
exit_condition: Alpha ends.
steps:
  - title: Alpha step
    description: Do alpha.
"#,
    )
    .expect("alpha procedure");
    fs::write(
        pack.join("procedures/beta-proc.procedure.yaml"),
        r#"schema_version: "1.0"
id: beta-proc
name: Beta Procedure
purpose: Run the beta procedure.
entry_condition: Beta starts.
exit_condition: Beta ends.
steps:
  - title: Beta step
    description: |
      Do beta.

      Second paragraph.
"#,
    )
    .expect("beta procedure");
    fs::write(
        pack.join("paradigms/pass-through.paradigm.yaml"),
        r#"schema_version: "1.0"
id: pass-through
name: Pass Through
summary: A linking paradigm.
"#,
    )
    .expect("paradigm");
    fs::write(
        pack.join("agent_profiles/probe-profile.agent.yaml"),
        r#"profile-id: probe-profile
name: Probe Profile
schema-version: "1.0"
roles:
  - implementer
purpose: Probe purpose.
specialization:
  primary-focus: Probing.
directive-references:
  - code: "010"
    name: Alpha Directive
    rationale: alpha rationale
  - code: "010"
    name: Alpha Directive
    rationale: duplicate row
  - code: "024"
    name: Beta Directive
    rationale: beta rationale
tactic-references:
  - id: gamma-tactic
    rationale: |
      gamma rationale
styleguide-references:
  - id: docs-style
    rationale: docs rationale
toolguide-references:
  - id: tool-guide
    rationale: tool rationale
"#,
    )
    .expect("probe profile");
    fs::write(
        pack.join("agent_profiles/scoped-profile.agent.yaml"),
        r#"profile-id: scoped-profile
name: Scoped Profile
schema-version: "1.0"
roles:
  - implementer
purpose: Scoped purpose.
specialization:
  primary-focus: Scoping.
styleguide-references:
  - id: scoped-style
    rationale: scoped rationale
"#,
    )
    .expect("scoped profile");
    fs::write(
        pack.join("agent_profiles/typo-profile.agent.yaml"),
        r#"profile-id: typo-profile
name: Typo Profile
schema-version: "1.0"
roles:
  - implementer
purpose: Typo purpose.
specialization:
  primary-focus: Typoing.
directive-references:
  - code: "DIRECTIVE_01O"
    name: Almost Alpha
    rationale: typo rationale
  - code: "DIRECTIVE_999"
    name: Far Away
    rationale: far rationale
"#,
    )
    .expect("typo profile");
    fixture
}

fn graph() -> DrgGraph {
    let node = |urn: &str, kind: NodeKind| DrgNode::new(urn, kind, None, Vec::new()).expect("node");
    let nodes = vec![
        node("agent_profile:probe-profile", NodeKind::AgentProfile),
        node("agent_profile:scoped-profile", NodeKind::AgentProfile),
        node("agent_profile:typo-profile", NodeKind::AgentProfile),
        node("procedure:alpha-proc", NodeKind::Procedure),
        node("procedure:beta-proc", NodeKind::Procedure),
        node("tactic:gamma-tactic", NodeKind::Tactic),
        node("tactic:delta-tactic", NodeKind::Tactic),
        node("paradigm:pass-through", NodeKind::Paradigm),
    ];
    let edge = |source: &str, target: &str, relation, when: Option<&str>, reason: Option<&str>| {
        let edge = DrgEdge::new(source, target, relation).expect("edge");
        let edge = match when {
            Some(value) => edge.with_when(value),
            None => edge,
        };
        match reason {
            Some(value) => edge.with_reason(value),
            None => edge,
        }
    };
    let edges = vec![
        edge(
            "agent_profile:probe-profile",
            "procedure:alpha-proc",
            Relation::Requires,
            None,
            None,
        ),
        edge(
            "procedure:alpha-proc",
            "procedure:beta-proc",
            Relation::Requires,
            None,
            None,
        ),
        edge(
            "agent_profile:probe-profile",
            "tactic:gamma-tactic",
            Relation::Suggests,
            Some("reviewing a change"),
            None,
        ),
        edge(
            "agent_profile:probe-profile",
            "tactic:delta-tactic",
            Relation::Suggests,
            None,
            Some("authored reason"),
        ),
        edge(
            "paradigm:pass-through",
            "tactic:delta-tactic",
            Relation::Suggests,
            Some(""),
            None,
        ),
        edge(
            "agent_profile:probe-profile",
            "paradigm:pass-through",
            Relation::Suggests,
            None,
            None,
        ),
    ];
    DrgGraph::new("1.0", "2026-11-06T00:00:00Z", "cf73-fixture", nodes, edges).expect("graph")
}

fn offering(pack: &Path, project: &Path) -> CanonicalOfferingService {
    let context = OfferingContext::new(
        project.to_path_buf(),
        OfferingRoots {
            built_in: Some(pack.to_path_buf()),
            org_roots: Vec::new(),
            project: None,
        },
        Some(vec!["ruby".to_owned()]),
    );
    CanonicalOfferingService::new(context)
}

fn render_child_blocks(fixture: &Path) -> Value {
    let pack = fixture.join("packs/built-in");
    let graph = graph();
    let profiles = ProfileRepository::load(&pack, &[], None).expect("profiles");
    let catalog = offering(&pack, fixture);
    let mut blocks = Vec::new();
    let mut warnings = Vec::new();
    let mut all_warnings: Vec<String> = Vec::new();
    let mut budgeted: Option<String> = None;
    for (index, id) in ["probe-profile", "scoped-profile", "typo-profile"]
        .iter()
        .enumerate()
    {
        sk_doctrine::reset_emitted_warnings();
        let profile = profiles.resolve_str(id, &graph).expect("profile");
        let captured = RefCell::new(Vec::<String>::new());
        let sink = |message: &str| captured.borrow_mut().push(message.to_owned());
        let inputs = ProfileSectionsInputs {
            profile: &profile,
            graph: &graph,
            catalog: &catalog,
            scope: None,
            warning_sink: Some(&sink),
        };
        let block = render_profile_sections(&inputs).expect("profile block");
        if budgeted.is_none() {
            // The prepared block is the real budget candidate: a large
            // non-candidate tail forces the same substitution the Source engine
            // performs on this exact text + block.
            let mut full = text("Header:\n\n");
            full.append(&block);
            full.push_scalars("\n");
            full.push_scalars(&"z".repeat(41_000));
            let after = enforce_token_budget(
                &full,
                &text("implement"),
                &block,
                &text(""),
                &text(""),
                BUDGET_DEFAULT,
                None,
            );
            budgeted = Some(scalar(&after));
        }
        blocks.push(scalar(&block));
        warnings.push(captured.borrow().clone());
        all_warnings.extend(captured.borrow().clone());
        let _ = index;
    }
    let _ = all_warnings;
    json!({
        "blocks": blocks,
        "warnings": warnings,
        "budgeted": budgeted,
    })
}

/// The Native child re-invocation. It is inert unless the parent sets the mode.
#[test]
fn cf73_native_profile_child() {
    let Ok(mode) = std::env::var(MODE_ENV) else {
        return;
    };
    assert_eq!(mode, "native");
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("root"));
    let repo = PathBuf::from(std::env::var_os(REPO_ENV).expect("repo"));
    let fixture = PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("fixture"));
    assert_child_environment(&root, &repo, &fixture);
    let value = render_child_blocks(&fixture);
    println!("CF73_JSON={value}");
}

/// The Source child script (written into the fixture's parent).
const SOURCE_CHILD: &str = r#"
import json, os, sys, types, warnings
from pathlib import Path

root = Path(os.environ["CF73_PAIR_ROOT"])
repo = Path(os.environ["CF73_PAIR_REPO"])
fixture = Path(os.environ["CF73_PAIR_FIXTURE"])
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

from charter.activation.context_renderers.profile_sections import _render_profile_sections
from charter.activation.context_renderers.token_budget import _enforce_token_budget
from charter.offering.agent_profiles.repository import AgentProfileRepository
from charter.offering.directives.repository import DirectiveRepository
from charter.offering.paradigms.repository import ParadigmRepository
from charter.offering.procedures.repository import ProcedureRepository
from charter.offering.styleguides.repository import StyleguideRepository
from charter.offering.tactics.repository import TacticRepository
from charter.offering.toolguides.repository import ToolguideRepository
from charter.offering.drg.models import DRGEdge, DRGGraph, DRGNode, NodeKind, Relation
from charter.offering.drg.reachability import agent_profile_seed_urns, profile_channel_reachable

node = lambda urn, kind: DRGNode(urn=urn, kind=kind)
nodes = [
    node("agent_profile:probe-profile", NodeKind.AGENT_PROFILE),
    node("agent_profile:scoped-profile", NodeKind.AGENT_PROFILE),
    node("agent_profile:typo-profile", NodeKind.AGENT_PROFILE),
    node("procedure:alpha-proc", NodeKind.PROCEDURE),
    node("procedure:beta-proc", NodeKind.PROCEDURE),
    node("tactic:gamma-tactic", NodeKind.TACTIC),
    node("tactic:delta-tactic", NodeKind.TACTIC),
    node("paradigm:pass-through", NodeKind.PARADIGM),
]
edge = lambda s, t, r, when=None, reason=None: DRGEdge(source=s, target=t, relation=r, when=when, reason=reason)
edges = [
    edge("agent_profile:probe-profile", "procedure:alpha-proc", Relation.REQUIRES),
    edge("procedure:alpha-proc", "procedure:beta-proc", Relation.REQUIRES),
    edge("agent_profile:probe-profile", "tactic:gamma-tactic", Relation.SUGGESTS, when="reviewing a change"),
    edge("agent_profile:probe-profile", "tactic:delta-tactic", Relation.SUGGESTS, reason="authored reason"),
    edge("paradigm:pass-through", "tactic:delta-tactic", Relation.SUGGESTS, when=""),
    edge("agent_profile:probe-profile", "paradigm:pass-through", Relation.SUGGESTS),
]
graph = DRGGraph(schema_version="1.0", generated_at="2026-11-06T00:00:00Z", generated_by="cf73", nodes=nodes, edges=edges)

class ChannelRepo:
    def profile_channel_reached(self, profile_id):
        seed = f"agent_profile:{profile_id}"
        if seed not in agent_profile_seed_urns(graph):
            return frozenset()
        return profile_channel_reachable(graph, {seed})
    def profile_channel_procedure_ids(self, profile_id):
        prefix = "procedure:"
        reached = self.profile_channel_reached(profile_id)
        return sorted(u[len(prefix):] for u in reached if u.startswith(prefix))
    @property
    def drg(self):
        return graph
    def get(self, item_id):
        return None

pack = fixture / "packs" / "built-in"
langs = ["ruby"]
service = types.SimpleNamespace(
    directives=DirectiveRepository(built_in_dir=pack / "directives"),
    tactics=TacticRepository(built_in_dir=pack / "tactics", active_languages=langs),
    styleguides=StyleguideRepository(built_in_dir=pack / "styleguides", active_languages=langs),
    toolguides=ToolguideRepository(built_in_dir=pack / "toolguides", active_languages=langs),
    procedures=ProcedureRepository(built_in_dir=pack / "procedures", active_languages=langs),
    paradigms=ParadigmRepository(built_in_dir=pack / "paradigms"),
    agent_profiles=ChannelRepo(),
)
profiles = AgentProfileRepository(built_in_dir=pack / "agent_profiles", active_languages=langs)

blocks = []
warnings_by_profile = []
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    for profile_id in ("probe-profile", "scoped-profile", "typo-profile"):
        profile = profiles.get(profile_id)
        blocks.append(str(_render_profile_sections(profile, service)))
        warnings_by_profile.append([])
    messages = [str(item.message) for item in caught]
offset = 0
# Split warnings per profile by rendering order: collect then partition by context.
for message in messages:
    context = "profile:probe-profile"
    if "profile:scoped-profile" in message:
        context = "profile:scoped-profile"
    elif "profile:typo-profile" in message:
        context = "profile:typo-profile"
    index = ("probe-profile", "scoped-profile", "typo-profile").index(context.split(":", 1)[1])
    warnings_by_profile[index].append(message)
full_text = "Header:\n\n" + blocks[0] + "\n" + ("z" * 41000)
budgeted = _enforce_token_budget(
    full_text, action="implement", profile_block=blocks[0], section_block=""
)
print("CF73_JSON=" + json.dumps({"blocks": blocks, "warnings": warnings_by_profile, "budgeted": budgeted}))
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
        .find(|line| line.starts_with("CF73_JSON="))
        .unwrap_or_else(|| panic!("{label} child printed no CF73_JSON line:\n{stdout}"));
    serde_json::from_str(&line["CF73_JSON=".len()..]).expect("child JSON")
}

#[test]
fn cf73_profile_sections_match_source() {
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
    let script = workspace.path().join("cf73_source_child.py");
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
        .args(["--exact", "cf73_native_profile_child", "--nocapture"]);
    let native = run_child(native_command, "native");

    assert_eq!(
        source["blocks"], native["blocks"],
        "the prepared profile blocks must be byte-identical"
    );
    assert_eq!(
        source["warnings"], native["warnings"],
        "the catalogue-miss warning policy must match"
    );
    assert_eq!(
        source["budgeted"], native["budgeted"],
        "the shared budget engine must substitute this exact prepared block identically"
    );

    let main = source["blocks"][0].as_str().expect("main block");
    let scoped = source["blocks"][1].as_str().expect("scoped block");
    let typo = source["blocks"][2].as_str().expect("typo block");
    // Section order + exact headers.
    let headers: Vec<&str> = main
        .lines()
        .filter(|line| line.starts_with("Profile-"))
        .collect();
    assert_eq!(
        headers,
        vec![
            "Profile-Cited Directives (probe-profile):",
            "Profile-Cited Tactics (probe-profile):",
            "Profile-Cited Styleguides (probe-profile):",
            "Profile-Cited Toolguides (probe-profile):",
            "Profile-Resolved Procedures (probe-profile):",
            "Profile-Suggested Paradigms (probe-profile):",
            "Profile-Suggested Tactics (probe-profile):",
        ]
    );
    // Authored duplicate rows and inline/fetch body split.
    assert_eq!(
        main.matches("  - DIRECTIVE_010: Alpha Directive").count(),
        2
    );
    assert!(main.contains("    Intent: Keep the alpha behavior faithful."));
    assert!(main.contains("Run: spec-kitty charter context --include directive:DIRECTIVE_024"));
    // Styleguide/toolguide are pointer-only even though their bodies are tiny.
    assert!(main.contains("Run: spec-kitty charter context --include styleguide:docs-style"));
    assert!(main.contains("Run: spec-kitty charter context --include toolguide:tool-guide"));
    assert!(!main.contains("Title: Docs Style"));
    // Procedure profile mode omits entry/exit conditions.
    assert!(main.contains("  - alpha-proc\n    Name: Alpha Procedure"));
    assert!(!main.contains("Entry condition: Alpha starts."));
    // Suggested doctrine carries the reaching `when` (normalised) and defaults.
    assert!(main.contains("Profile-Suggested Tactics (probe-profile):"));
    assert!(main.contains("  - delta-tactic: authored reason"));
    assert!(main.contains("When you need to fetch when this artefact's guidance applies"));
    // Scope-filtered and typo diagnosis stanzas.
    assert!(scoped.contains("Cause: scope_filtered"));
    assert!(scoped.contains("is scoped to specific languages and does not apply"));
    assert!(typo.contains("did you mean 'DIRECTIVE_010'?"));
    // 2400/2401 per-entry boundary: a 2500-char intent is over, so it fetches.
    assert!(main.contains("DIRECTIVE_024"));
}

#[test]
fn cf73_diagnosis_and_code_normalisation_are_pure_source_policies() {
    let code = |raw: &str| scalar(&format_profile_directive_code(&text(raw)));
    assert_eq!(code("010"), "DIRECTIVE_010");
    assert_eq!(code("DIRECTIVE_003"), "DIRECTIVE_003");
    assert_eq!(code("025-boy-scout-rule"), "DIRECTIVE_025");
    assert_eq!(code("  044 "), "DIRECTIVE_044");
    assert_eq!(code("12_x"), "DIRECTIVE_012");
    assert_eq!(code("12a-3"), "12a-3");

    let workspace = tempfile::tempdir().expect("workspace");
    let fixture = write_fixture(workspace.path());
    let pack = fixture.join("packs/built-in");
    let catalog = offering(&pack, &fixture);
    let available = catalog.artifact_ids(ArtifactKind::Directive).expect("ids");
    let typo = sk_doctrine::classify_catalog_miss(&text("DIRECTIVE_01O"), &available);
    assert_eq!(typo.cause, sk_doctrine::CatalogMissCause::TypoSuspected);
    assert_eq!(typo.suggestion, Some(text("DIRECTIVE_010")));
    let far = sk_doctrine::classify_catalog_miss(&text("zzzzzzzzzzzzzzzz"), &available);
    assert_eq!(far.cause, sk_doctrine::CatalogMissCause::MissingArtifact);
    let filtered = catalog
        .artifact_scope_filtered_ids(ArtifactKind::Styleguide)
        .expect("scope filtered");
    assert!(filtered.iter().any(|id| id == &text("scoped-style")));
    let scoped = sk_doctrine::diagnose_catalog_miss(
        &text("scoped-style"),
        &catalog,
        ArtifactKind::Styleguide,
        Some(&fixture),
    );
    assert_eq!(scoped.cause, sk_doctrine::CatalogMissCause::ScopeFiltered);

    sk_doctrine::reset_emitted_warnings();
    let delivered = RefCell::new(Vec::<String>::new());
    let sink = |message: &str| delivered.borrow_mut().push(message.to_owned());
    assert!(sk_doctrine::emit_catalog_miss_warning(
        "directive",
        &text("DIRECTIVE_01O"),
        &typo,
        Some("profile:typo-profile"),
        &sink,
    ));
    // The once-per-process latch suppresses the exact repeat.
    assert!(!sk_doctrine::emit_catalog_miss_warning(
        "directive",
        &text("DIRECTIVE_01O"),
        &typo,
        Some("profile:typo-profile"),
        &sink,
    ));
    assert_eq!(delivered.borrow().len(), 1);
    assert!(delivered.borrow()[0].contains("cause=typo_suspected"));
    assert!(delivered.borrow()[0].contains("context='profile:typo-profile'"));
    sk_doctrine::reset_emitted_warnings();
}

#[test]
fn cf73_compact_profile_augmentation_only_when_nonempty() {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture = write_fixture(workspace.path());
    let pack = fixture.join("packs/built-in");
    let graph = graph();
    let profiles = ProfileRepository::load(&pack, &[], None).expect("profiles");
    let catalog = offering(&pack, &fixture);
    let probe = profiles
        .resolve_str("probe-profile", &graph)
        .expect("probe profile");
    let context = CharterContext {
        context_schema_version: "1.4.0".to_owned(),
        action: "implement".to_owned(),
        mode: ContextMode::Compact,
        directives: Vec::new(),
        tactics: Vec::new(),
        styleguides: Vec::new(),
        toolguides: Vec::new(),
        procedures: Vec::new(),
        all_directives: Vec::new(),
        directives_source: None,
        diagnostics: Vec::new(),
        references: Vec::new(),
        project_charter: ProjectCharterBlock::default(),
        org_charter: OrgCharterBlock::default(),
        governance_references: Vec::new(),
        profile_links: Vec::new(),
        tension_arbiters: Vec::new(),
        unarbitrated_tensions: Vec::new(),
    };
    let inputs = RenderInputs {
        profile_sections: Some(ProfileSectionsInputs {
            profile: &probe,
            graph: &graph,
            catalog: &catalog,
            scope: None,
            warning_sink: None,
        }),
        ..RenderInputs::default()
    };
    let text = scalar(&render_compact_exact(&context, &inputs).expect("compact"));
    assert!(text.starts_with("Governance:"));
    assert!(text.contains("Profile-Cited Directives (probe-profile):"));
    assert!(!text.contains("# Governance payload:"));

    // `profile=None` never appends a fabricated section.
    let bare = scalar(&render_compact_exact(&context, &RenderInputs::default()).expect("bare"));
    assert!(!bare.contains("Profile-"));

    // A compact body already over the shared budget still gets the same owner:
    // the profile block is substituted (its headers survive) and the warning is
    // appended, never a universal cap or a re-render.
    let mut large = context;
    large.all_directives = (0..3_000)
        .map(|index| DirectiveEntry {
            id: format!("DIRECTIVE_{index:04}"),
            source: "built-in".to_owned(),
            title: None,
            summary: None,
        })
        .collect();
    let large_text = scalar(&render_compact_exact(&large, &inputs).expect("large compact"));
    assert!(large_text.contains("Profile-Cited Directives (probe-profile):"));
    assert!(large_text.contains("# Governance payload:"));
    assert!(large_text.contains("section:profile-citations"));
}

#[test]
fn cf73_profile_block_is_a_real_budget_candidate() {
    let workspace = tempfile::tempdir().expect("workspace");
    let fixture = write_fixture(workspace.path());
    let pack = fixture.join("packs/built-in");
    let graph = graph();
    let profiles = ProfileRepository::load(&pack, &[], None).expect("profiles");
    let catalog = offering(&pack, &fixture);
    let profile = profiles
        .resolve_str("probe-profile", &graph)
        .expect("profile");
    let inputs = ProfileSectionsInputs {
        profile: &profile,
        graph: &graph,
        catalog: &catalog,
        scope: None,
        warning_sink: None,
    };
    let block = render_profile_sections(&inputs).expect("block");
    let mut full_text = text("Charter Context (Bootstrap):\n");
    full_text.append(&block);
    // A large non-candidate tail forces the budget owner to substitute the
    // profile block itself (the only candidate in this text).
    full_text.push_scalars("\n");
    full_text.push_scalars(&"z".repeat(41_000));
    let budgeted = enforce_token_budget(
        &full_text,
        &text("implement"),
        &block,
        &text(""),
        &text(""),
        BUDGET_DEFAULT,
        None,
    );
    // The prepared block is the budget candidate: kind headers survive while the
    // substitutable bodies become the canonical profile-citations stanza.
    assert!(
        block
            .to_scalar_string()
            .unwrap()
            .contains("Alpha Directive")
    );
    let after = budgeted.to_scalar_string().unwrap();
    assert!(after.contains("Profile-Cited Directives (probe-profile):"));
    assert!(after.contains("Run: spec-kitty charter context --include section:profile-citations"));
    assert!(after.contains("# Governance payload:"));
}
