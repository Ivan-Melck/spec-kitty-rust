//! Source-first exact-identity matrix: layer merge, lineage union, `excluding`.
//!
//! The reference merges a project profile into its built-in base field by field,
//! unions the list fields up a `specializes_from` lineage, and applies the leaf's
//! `excluding` declaration to the final result — all keyed by *Python string*
//! equality. An implementation that shadows a surrogate with a constant (e.g.
//! NUL) would collide three distinct authored values here:
//!
//! * `cap-\ud800`   (a surrogate code unit),
//! * `cap-\u0000`   (a real NUL), and
//! * `cap-\\ud800`  (the literal backslash text).
//!
//! Every case is asserted on the pinned Source oracle first, then on the native
//! loader, field for field.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sk_doctrine::{
    DrgEdge, DrgGraph, DrgNode, NodeKind, ProfileLayer, ProfileRepository, Relation,
};
use sk_kernel::PythonString;

fn scalars(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

fn scalars_with(text: &str, codes: &[u32]) -> Vec<u32> {
    let mut points = scalars(text);
    points.extend_from_slice(codes);
    points
}

const SOURCE_SCRIPT: &str = r#"
import json, os
from pathlib import Path
from charter.offering.agent_profiles.repository import AgentProfileRepository
from charter.offering.drg.models import DRGGraph, DRGNode, DRGEdge, NodeKind, Relation

root = Path(os.environ["SK_MERGE_ROOT"]).resolve()
built_in = Path(os.environ["SK_MERGE_BUILT_IN"]).resolve()
project = Path(os.environ["SK_MERGE_PROJECT"]).resolve()

assert Path.cwd().resolve() == root, (Path.cwd(), root)
for key in ("HOME", "TMPDIR", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"):
    path = Path(os.environ[key]).resolve()
    assert str(path).startswith(str(root) + os.sep), (key, path, root)

def cps(text):
    return [ord(character) for character in text]

def values(items):
    return [cps(value) for value in items]

graph = DRGGraph(
    schema_version="1.0",
    generated_at="1970-01-01T00:00:00Z",
    generated_by="profile-merge-oracle",
    nodes=[
        DRGNode(urn="agent_profile:lineage-base", kind=NodeKind.AGENT_PROFILE),
        DRGNode(urn="agent_profile:lineage-leaf", kind=NodeKind.AGENT_PROFILE),
    ],
    edges=[
        DRGEdge(
            source="agent_profile:lineage-leaf",
            target="agent_profile:lineage-base",
            relation=Relation.SPECIALIZES_FROM,
        )
    ],
)

repo = AgentProfileRepository(built_in_dir=built_in, project_dir=project, drg=graph)
skipped = repo.skipped_profiles()
assert skipped == [], [(s.layer, s.path, s.error_summary) for s in skipped]

# 1. A same-id project overlay is field-merged into the built-in base.
merged = repo.get("merge-target")
assert merged is not None
assert merged.description == "Org overlay", merged.description
assert [cps(value) for value in merged.capabilities] == [cps("a"), cps("b")], merged.capabilities
sc = merged.specialization_context
assert sc.frameworks == ["flask"], sc.frameworks
assert sc.languages == ["python"], sc.languages
assert [cps(value) for value in sc.domain_keywords] == [cps("kw-" + chr(0xD800))]

# 2. A lossy value in the overlay replaces the base value exactly.
replaced = repo.get("replace-target")
assert replaced is not None
assert cps(replaced.description) == cps("shared-") + [0xD801], cps(replaced.description)

# 3. Lineage union merge, then the leaf's exact `excluding`.
resolved = repo.resolve_profile("lineage-leaf")
assert [cps(value) for value in resolved.capabilities] == [
    cps("a"),
    cps("cap-" + chr(0x0)),
    cps("cap-\\ud800"),
    cps("b"),
], [cps(value) for value in resolved.capabilities]
assert [cps(ref.code) for ref in resolved.directive_references] == [cps("001"), cps("002")]

# 4. A non-scalar id loads, resolves and is provenance-tagged; no empty or
#    invented ancestry is produced for it.
non_scalar = [0x61, 0x75, 0x74, 0x68, 0x6F, 0x72, 0xD800]
exact_id = "".join(chr(code) for code in non_scalar)
profile = repo.get(exact_id)
assert profile is not None, sorted(cps(key) for key in repo._profiles)
assert cps(profile.profile_id) == non_scalar
assert repo.get_provenance(exact_id) == "project"
assert repo.get_ancestors(exact_id) == []
resolved_non_scalar = repo.resolve_profile(exact_id)
assert resolved_non_scalar.profile_id == exact_id

# 5. A scoped non-scalar profile is recorded in the scope-filtered set exactly.
scoped = [0x73, 0x63, 0x6F, 0x70, 0x65, 0x64, 0xD800]
filtered = AgentProfileRepository(
    built_in_dir=built_in,
    project_dir=project,
    active_languages=["python"],
    drg=graph,
)
scoped_id = "".join(chr(code) for code in scoped)
assert scoped_id in filtered.scope_filtered_ids, sorted(cps(v) for v in filtered.scope_filtered_ids)
assert filtered.get(scoped_id) is None

print(json.dumps({
    "merged": {
        "description": cps(merged.description),
        "capabilities": values(merged.capabilities),
        "frameworks": values(sc.frameworks),
        "languages": values(sc.languages),
        "domain_keywords": values(sc.domain_keywords),
    },
    "replaced": {"description": cps(replaced.description)},
    "resolved": {
        "capabilities": values(resolved.capabilities),
        "directive_codes": [cps(ref.code) for ref in resolved.directive_references],
    },
    "non_scalar": {
        "id": non_scalar,
        "provenance": repo.get_provenance(exact_id),
        "ancestors": [cps(value) for value in repo.get_ancestors(exact_id)],
    },
    "scoped": {"id": scoped, "filtered": [cps(value) for value in filtered.scope_filtered_ids]},
}))
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    built_in: PathBuf,
    project: PathBuf,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
    fs::write(path, text).expect("write fixture");
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = temp.path().canonicalize().expect("canonical root");
    let built_in = root.join("packs/built-in");
    // `built_in_dir` for the reference is the `agent_profiles` directory; the
    // native loader takes the pack root. Both read the same files.
    let built_in_profiles = built_in.join("agent_profiles");
    let project = root.join(".kittify/charter-packs");
    let project_profiles = project.join("agent_profiles");
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        fs::create_dir_all(root.join(directory)).expect("isolated directory");
    }

    write(
        &built_in_profiles.join("merge-target.agent.yaml"),
        concat!(
            "profile-id: merge-target\n",
            "name: Merge Target\n",
            "description: Built-in\n",
            "purpose: merge parity\n",
            "roles: [reviewer]\n",
            "capabilities: [a, b]\n",
            "specialization:\n  primary-focus: review\n",
            "specialization-context:\n  languages: [python]\n  frameworks: [django]\n",
            "  domain-keywords: [\"kw-\\uD800\"]\n",
        ),
    );
    write(
        &project_profiles.join("merge-target.agent.yaml"),
        "profile-id: merge-target\ndescription: Org overlay\nspecialization-context:\n  frameworks: [flask]\n",
    );
    write(
        &built_in_profiles.join("replace-target.agent.yaml"),
        concat!(
            "profile-id: replace-target\n",
            "name: Replace\n",
            "description: \"shared-\\uD800\"\n",
            "purpose: merge parity\n",
            "roles: [reviewer]\n",
            "specialization:\n  primary-focus: review\n",
        ),
    );
    write(
        &project_profiles.join("replace-target.agent.yaml"),
        "profile-id: replace-target\ndescription: \"shared-\\uD801\"\n",
    );
    write(
        &built_in_profiles.join("lineage-base.agent.yaml"),
        concat!(
            "profile-id: lineage-base\n",
            "name: Base\n",
            "purpose: lineage parity\n",
            "roles: [reviewer]\n",
            "specialization:\n  primary-focus: review\n",
            "capabilities: [a, \"cap-\\uD800\", \"cap-\\u0000\"]\n",
            "directive-references:\n  - code: '001'\n    name: one\n    rationale: base\n",
        ),
    );
    write(
        &project_profiles.join("lineage-leaf.agent.yaml"),
        concat!(
            "profile-id: lineage-leaf\n",
            "name: Leaf\n",
            "purpose: lineage parity\n",
            "roles: [reviewer]\n",
            "specialization:\n  primary-focus: review\n",
            "capabilities: [\"cap-\\\\ud800\", b]\n",
            "directive-references:\n  - code: '002'\n    name: two\n    rationale: leaf\n",
            "excluding:\n  capabilities: [\"cap-\\uD800\"]\n",
        ),
    );
    // A non-scalar identity in the project layer.
    write(
        &project_profiles.join("author-surrogate.agent.yaml"),
        concat!(
            "profile-id: \"author\\uD800\"\n",
            "name: Source-valid profile\n",
            "purpose: preserve authored identities\n",
            "roles: [reviewer]\n",
            "specialization:\n  primary-focus: review\n",
        ),
    );
    // A scoped non-scalar identity, filtered out for a python-only project.
    write(
        &project_profiles.join("scoped-surrogate.agent.yaml"),
        concat!(
            "profile-id: \"scoped\\uD800\"\n",
            "name: Scoped\n",
            "purpose: scope parity\n",
            "roles: [reviewer]\n",
            "applies_to_languages: [jvm]\n",
            "specialization:\n  primary-focus: review\n",
        ),
    );
    Fixture {
        _temp: temp,
        root,
        built_in,
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
            ("SK_MERGE_ROOT".to_owned(), root.display().to_string()),
            (
                "SK_MERGE_BUILT_IN".to_owned(),
                self.built_in.join("agent_profiles").display().to_string(),
            ),
            (
                "SK_MERGE_PROJECT".to_owned(),
                self.project.join("agent_profiles").display().to_string(),
            ),
        ]
    }
}

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

fn points(value: &Value) -> Vec<u32> {
    value
        .as_array()
        .expect("code point array")
        .iter()
        .map(|code| u32::try_from(code.as_u64().expect("integer")).expect("u32"))
        .collect()
}

fn points_list(value: &Value) -> Vec<Vec<u32>> {
    value.as_array().expect("list").iter().map(points).collect()
}

fn graph() -> DrgGraph {
    DrgGraph::new(
        "1.0",
        "1970-01-01T00:00:00Z",
        "profile-merge-oracle",
        vec![
            DrgNode::new(
                "agent_profile:lineage-base",
                NodeKind::AgentProfile,
                None,
                Vec::new(),
            )
            .unwrap(),
            DrgNode::new(
                "agent_profile:lineage-leaf",
                NodeKind::AgentProfile,
                None,
                Vec::new(),
            )
            .unwrap(),
        ],
        vec![
            DrgEdge::new(
                "agent_profile:lineage-leaf",
                "agent_profile:lineage-base",
                Relation::SpecializesFrom,
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

fn exact(codes: &[u32]) -> PythonString {
    PythonString::from_codepoints(codes).expect("valid code points")
}

#[test]
fn merge_lineage_and_excluding_are_exact_like_the_source() {
    let fixture = fixture();
    let source = run_source(&fixture);
    let repository = ProfileRepository::load(&fixture.built_in, &[], Some(&fixture.project))
        .expect("native load");
    assert!(
        repository.skipped().is_empty(),
        "{:?}",
        repository.skipped()
    );

    // 1. Same-id overlay merge.
    let merged = repository
        .get(&exact(&scalars("merge-target")))
        .expect("merged profile");
    assert_eq!(
        merged
            .description()
            .map(|value| value.codepoints().to_vec()),
        Some(points(&source["merged"]["description"]))
    );
    assert_eq!(
        merged
            .strings("capabilities")
            .iter()
            .map(|value| value.codepoints().to_vec())
            .collect::<Vec<_>>(),
        points_list(&source["merged"]["capabilities"])
    );
    assert_eq!(
        merged
            .body
            .nested_strings("specialization-context", "frameworks")
            .iter()
            .map(|value| value.codepoints().to_vec())
            .collect::<Vec<_>>(),
        points_list(&source["merged"]["frameworks"])
    );
    assert_eq!(
        merged
            .body
            .nested_strings("specialization-context", "languages")
            .iter()
            .map(|value| value.codepoints().to_vec())
            .collect::<Vec<_>>(),
        points_list(&source["merged"]["languages"])
    );
    assert_eq!(
        merged
            .domain_keywords()
            .iter()
            .map(|value| value.codepoints().to_vec())
            .collect::<Vec<_>>(),
        points_list(&source["merged"]["domain_keywords"])
    );
    assert_eq!(
        repository.provenance(&exact(&scalars("merge-target"))),
        Some(ProfileLayer::Project.as_str())
    );

    // 2. A lossy overlay value replaces the base value instead of colliding.
    let replaced = repository
        .get(&exact(&scalars("replace-target")))
        .expect("replaced profile");
    assert_eq!(
        replaced
            .description()
            .map(|value| value.codepoints().to_vec()),
        Some(points(&source["replaced"]["description"]))
    );
    assert_eq!(
        points(&source["replaced"]["description"]),
        scalars_with("shared-", &[0xd801])
    );

    // 3. Lineage union + exact `excluding`.
    let resolved = repository
        .resolve(&exact(&scalars("lineage-leaf")), &graph())
        .expect("lineage resolution");
    assert_eq!(
        resolved
            .strings("capabilities")
            .iter()
            .map(|value| value.codepoints().to_vec())
            .collect::<Vec<_>>(),
        points_list(&source["resolved"]["capabilities"])
    );
    assert_eq!(
        points_list(&source["resolved"]["capabilities"]),
        vec![
            scalars("a"),
            scalars_with("cap-", &[0x0]),
            scalars("cap-\\ud800"),
            scalars("b"),
        ]
    );
    assert_eq!(
        resolved
            .directive_references()
            .iter()
            .map(|value| value.codepoints().to_vec())
            .collect::<Vec<_>>(),
        points_list(&source["resolved"]["directive_codes"])
    );
    assert_eq!(
        resolved
            .lineage
            .iter()
            .filter_map(|id| id.to_scalar_string().ok())
            .collect::<Vec<_>>(),
        vec!["lineage-base".to_owned(), "lineage-leaf".to_owned()]
    );
}

#[test]
fn a_non_scalar_identity_resolves_without_invented_ancestry() {
    let fixture = fixture();
    let source = run_source(&fixture);
    let repository = ProfileRepository::load(&fixture.built_in, &[], Some(&fixture.project))
        .expect("native load");
    let id = exact(&point_list(&source["non_scalar"]["id"]));
    assert_eq!(source["non_scalar"]["provenance"], json!("project"));
    assert_eq!(source["non_scalar"]["ancestors"], json!([]));

    let profile = repository.get(&id).expect("non-scalar profile loads");
    assert_eq!(profile.scalar_id(), None);
    assert_eq!(repository.provenance(&id), Some("project"));
    // The reference reports no ancestors; native must agree, without a refusal
    // and without inventing a parent.
    let resolved = repository
        .resolve(&id, &graph())
        .expect("a non-scalar id still resolves");
    assert_eq!(resolved.lineage, vec![id.clone()]);
    assert_eq!(
        resolved.id.codepoints(),
        points(&source["non_scalar"]["id"]).as_slice()
    );
}

#[test]
fn a_scoped_non_scalar_profile_is_recorded_exactly() {
    let fixture = fixture();
    let source = run_source(&fixture);
    let active = vec!["python".to_owned()];
    let repository = ProfileRepository::load_with_languages(
        &fixture.built_in,
        &[],
        Some(&fixture.project),
        Some(&active),
    )
    .expect("native load");
    let id = exact(&point_list(&source["scoped"]["id"]));
    assert!(repository.get(&id).is_none());
    assert!(repository.scope_filtered_exact_ids().contains(&id));
    assert_eq!(
        points_list(&source["scoped"]["filtered"]),
        vec![points(&source["scoped"]["id"])]
    );
    // The `String`-shaped diagnostic set cannot hold a lone surrogate, so it
    // carries the exact `repr` label (an injective escaped rendering) instead of
    // a lossy or substituted id.
    let label = id.repr();
    assert!(repository.scope_filtered_ids().contains(&label));
    assert!(label.contains("\\ud800"), "{label}");
}

fn point_list(value: &Value) -> Vec<u32> {
    points(value)
}
