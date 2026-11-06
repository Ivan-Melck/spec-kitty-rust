//! CF44 — canonical pack-skill repository post-validation and admission
//! unification.
//!
//! Source: `src/charter/offering/pack_skills/{models,validation,repository}.py`.
//! These tests exercise the NEW canonical owner (`sk_doctrine::pack_skill_repository`)
//! and its two consumers:
//!
//! * a real offering-service load (the Skill arm + the namespaced typed
//!   accessor), including required-only activation availability;
//! * a paired Source/Native child run over the same fixture, comparing the
//!   effective folded record, provenance, body and the exact `Full artifact:`
//!   include payload. Both children assert their own CWD, HOME, every XDG root
//!   (DIRS included), TMPDIR/TMP/TEMP, absolute `PYTHONPATH` and pin pack root
//!   before computing. The Source side is the lock-exact pinned interpreter.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_charter::{ArtifactKind, PackTier};
use sk_doctrine::context_render::include_full_payload_lines;
use sk_doctrine::{
    CanonicalOfferingService, OfferingContext, OfferingRoots, PackSkill, PackSkillRepository,
    canonical_artifact_body_exact,
};

/// The scalar spelling of one exact value (these fixtures are all ASCII).
fn scalar(value: &sk_kernel::PythonString) -> String {
    value.to_scalar_string().expect("scalar")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, text).expect("write");
}

const BASE_BODY: &str = "Base body: $ARGUMENTS\n";

fn base_skill() -> &'static str {
    "schema_version: '1.0'\nid: base\ntitle: Base\ndescription: A base skill.\nform: prompt\nbody_path: base.md\n"
}

fn write_prompt_skill(dir: &Path, id: &str, body: &str, description: &str) {
    write(
        &dir.join(format!("{id}.skill.yaml")),
        &format!(
            "schema_version: '1.0'\nid: {id}\ntitle: {id}\ndescription: {description}\nform: prompt\nbody_path: {id}.md\n"
        ),
    );
    write(&dir.join(format!("{id}.md")), body);
}

fn tree(root: &Path) -> Vec<(String, String)> {
    fn walk(base: &Path, path: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
            .unwrap_or_default();
        entries.sort();
        for entry in entries {
            let relative = entry
                .strip_prefix(base)
                .expect("inside base")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&entry).unwrap();
            if metadata.is_dir() {
                out.push((format!("{relative}/"), String::new()));
                walk(base, &entry, out);
            } else if metadata.file_type().is_symlink() {
                out.push((
                    relative,
                    format!("link:{}", std::fs::read_link(&entry).unwrap().display()),
                ));
            } else {
                out.push((
                    relative,
                    format!("bytes:{:?}", std::fs::read(&entry).unwrap()),
                ));
            }
        }
    }
    let mut out = Vec::new();
    if root.is_dir() {
        walk(root, root, &mut out);
    }
    out
}

fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("sk-cf44-{name}-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clean fixture");
    }
    let built_in = root.join("built_in/skills");
    let org = root.join("org/skills");
    let project = root.join("project/skills");
    std::fs::create_dir_all(&built_in).expect("built-in dir");
    std::fs::create_dir_all(&org).expect("org dir");
    std::fs::create_dir_all(&project).expect("project dir");
    (root, built_in, org, project)
}

#[test]
fn tier_rules_and_body_confinement_match_the_source_phase_order() {
    let (_root, built_in, org, project) = fixture("tiers");
    // Built-in: exempt from the non-built-in rules, but not from the limits.
    write(&built_in.join("base.skill.yaml"), base_skill());
    write(&built_in.join("base.md"), BASE_BODY);
    write_prompt_skill(&built_in, "spk-reserved", "built-in body\n", "Built-in.");
    // Org: reserved prefix / script directory / escaping and missing bodies.
    write_prompt_skill(&org, "spk-org", "org body\n", "Org.");
    write_prompt_skill(&org, "org-helper", "org helper body\n", "Org helper.");
    write(
        &org.join("escape.skill.yaml"),
        "schema_version: '1.0'\nid: escape\ntitle: E\ndescription: D\nform: prompt\nbody_path: ../secret.md\n",
    );
    write(&org.join("escape.md"), "secret\n");
    write(
        &org.join("absolute.skill.yaml"),
        "schema_version: '1.0'\nid: absolute\ntitle: A\ndescription: D\nform: prompt\nbody_path: /etc/passwd\n",
    );
    write(
        &org.join("no-body.skill.yaml"),
        "schema_version: '1.0'\nid: no-body\ntitle: N\ndescription: D\nform: prompt\nbody_path: missing.md\n",
    );
    write_prompt_skill(&org, "scripty", "scripty body\n", "Scripty.");
    std::fs::create_dir_all(org.join("scripty/scripts")).expect("scripts dir");
    // Project: reserved/frontmatter/description limits.
    write_prompt_skill(&project, "proj-ok", "project body\n", "Project.");
    write(
        &project.join("front.skill.yaml"),
        &base_skill()
            .replace("base", "front")
            .replace("base.md", "front.md"),
    );
    write(
        &project.join("front.md"),
        "---\nAllowed-Tools: [bash]\n---\nbody\n",
    );
    let long = "x".repeat(1100);
    write(
        &project.join("toolong.skill.yaml"),
        &format!(
            "schema_version: '1.0'\nid: toolong\ntitle: T\ndescription: {long}\nform: prompt\nbody_path: toolong.md\n"
        ),
    );
    write(&project.join("toolong.md"), "body\n");
    // An unrelated kind in the same directory must never be read.
    write(&project.join("broken.directive.yaml"), "id: [unclosed\n");

    let repository =
        PackSkillRepository::load(&built_in, std::slice::from_ref(&org), Some(&project), None)
            .expect("loads with skips");
    let mut ids = repository.ids();
    ids.sort();
    assert_eq!(ids, vec!["base", "org-helper", "proj-ok", "spk-reserved"]);
    assert_eq!(repository.tier_of("base"), Some(PackTier::BuiltIn));
    assert_eq!(repository.tier_of("org-helper"), Some(PackTier::Org));
    assert_eq!(repository.tier_of("proj-ok"), Some(PackTier::Project));
    assert_eq!(repository.body_text("base").as_deref(), Some(BASE_BODY));
    // The built-in reserved name is admitted; the org one is skipped.
    assert!(repository.skill("spk-reserved").is_some());
    assert!(repository.skill("spk-org").is_none());
    assert!(repository.skill("escape").is_none());
    assert!(repository.skill("absolute").is_none());
    assert!(repository.skill("scripty").is_none());
    assert!(repository.skill("no-body").is_none());
    assert!(repository.skill("front").is_none());
    assert!(repository.skill("toolong").is_none());
    let warnings = repository.warnings().join("\n");
    assert!(
        warnings.contains("prefix reserved for built-in skills"),
        "{warnings}"
    );
    assert!(
        warnings.contains("'scripts/' directory is not allowed"),
        "{warnings}"
    );
    assert!(
        warnings.contains("escapes the skill directory"),
        "{warnings}"
    );
    assert!(warnings.contains("not found"), "{warnings}");
    assert!(
        warnings.contains("permission-widening frontmatter"),
        "{warnings}"
    );
    assert!(warnings.contains("description limit"), "{warnings}");
}

#[cfg(unix)]
#[test]
fn body_symlink_semantics_are_resolved_inside_the_skill_directory() {
    use std::os::unix::fs::symlink;
    let (root, built_in, _org, project) = fixture("symlink");
    let outside = root.join("outside.md");
    write(&outside, "outside secret\n");
    write_prompt_skill(&project, "inside", "inside body\n", "Inside.");
    symlink(project.join("inside.md"), project.join("link-inside.md")).expect("symlink inside");
    write(
        &project.join("link-inside.skill.yaml"),
        "schema_version: '1.0'\nid: link-inside\ntitle: L\ndescription: D\nform: prompt\nbody_path: link-inside.md\n",
    );
    symlink(&outside, project.join("link-outside.md")).expect("symlink outside");
    write(
        &project.join("link-outside.skill.yaml"),
        "schema_version: '1.0'\nid: link-outside\ntitle: L\ndescription: D\nform: prompt\nbody_path: link-outside.md\n",
    );
    let repository =
        PackSkillRepository::load(&built_in, &[], Some(&project), None).expect("loads");
    assert!(repository.skill("inside").is_some());
    assert!(repository.skill("link-inside").is_some());
    assert_eq!(
        repository.body_text("link-inside").as_deref(),
        Some("inside body\n")
    );
    assert!(repository.skill("link-outside").is_none());
    assert!(
        repository
            .warnings()
            .join("\n")
            .contains("escapes the skill directory")
    );
}

#[test]
fn a_sibling_org_id_conflict_aborts_the_whole_load() {
    let (root, built_in, org, _project) = fixture("conflict");
    let second = root.join("org2/skills");
    std::fs::create_dir_all(&second).expect("second org");
    write_prompt_skill(&org, "shared", "one\n", "One.");
    write_prompt_skill(&second, "shared", "two\n", "Two.");
    let error = PackSkillRepository::load(&built_in, &[org, second], None, None)
        .err()
        .expect("sibling conflict");
    assert!(error.to_string().contains("two org packs"), "{error}");
}

#[test]
fn augmentations_fold_into_the_target_id_like_the_source() {
    let (_root, built_in, org, project) = fixture("augment");
    write(&built_in.join("base.skill.yaml"), base_skill());
    write(&built_in.join("base.md"), BASE_BODY);
    write_prompt_skill(&org, "org-skill", "org body\n", "Org.");
    // A valid enhancement of the built-in base.
    write(
        &org.join("tune.skill.yaml"),
        "schema_version: '1.0'\nid: tune\ntitle: Tuned\ndescription: Tuned description.\nform: prompt\nbody_path: base.md\nenhances: base\n",
    );
    // An enhancement that changes the body is invalid and skipped.
    write(
        &org.join("bad-tune.skill.yaml"),
        "schema_version: '1.0'\nid: bad-tune\ntitle: Bad\ndescription: Bad.\nform: prompt\nbody_path: other.md\nenhances: base\n",
    );
    // A project override of the org skill is allowed; of a built-in is not.
    write(
        &project.join("org-skill.skill.yaml"),
        "schema_version: '1.0'\nid: org-skill\ntitle: Project override\ndescription: Project.\nform: prompt\nbody_path: org-skill.md\n",
    );
    write(&project.join("org-skill.md"), "project body\n");
    // A project augmenter cannot override a built-in target.
    write_prompt_skill(&built_in, "pure", "pure body\n", "Pure.");
    write(
        &project.join("pure-override.skill.yaml"),
        "schema_version: '1.0'\nid: pure-override\ntitle: P\ndescription: P.\nform: prompt\nbody_path: pure.md\noverrides: pure\n",
    );
    write(&project.join("pure.md"), "pure body\n");

    let base_path = built_in.join("base.skill.yaml");
    let org_skill_path = org.join("org-skill.skill.yaml");
    let project_override_path = project.join("org-skill.skill.yaml");
    let repository =
        PackSkillRepository::load(&built_in, std::slice::from_ref(&org), Some(&project), None)
            .expect("loads with augmentation warnings");
    let base = repository.skill("base").expect("folded base");
    assert_eq!(scalar(&base.title), "Tuned");
    assert_eq!(scalar(&base.id), "base");
    assert!(base.overrides.is_none() && base.enhances.is_none());
    assert_eq!(repository.tier_of("base"), Some(PackTier::Org));
    // An enhancement keeps the base file as the body source.
    assert_eq!(repository.source_path("base"), Some(base_path.as_path()));
    assert_eq!(repository.body_text("base").as_deref(), Some(BASE_BODY));
    // The augmenter id is not served on its own.
    assert!(repository.skill("tune").is_none());

    let project_skill = repository.skill("org-skill").expect("project override");
    assert_eq!(scalar(&project_skill.title), "Project override");
    assert_eq!(repository.tier_of("org-skill"), Some(PackTier::Project));
    assert_eq!(
        repository.source_path("org-skill"),
        Some(project_override_path.as_path())
    );
    assert_eq!(
        repository.body_text("org-skill").as_deref(),
        Some("project body\n")
    );
    assert_ne!(
        repository.source_path("org-skill"),
        Some(org_skill_path.as_path())
    );

    let warnings = repository.warnings().join("\n");
    assert!(
        warnings.contains("may not change the body or expansion"),
        "{warnings}"
    );
    assert!(warnings.contains("replaceable-builtins"), "{warnings}");
}

#[test]
fn the_checked_adapter_reads_the_same_record_as_the_canonical_dump() {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        "schema_version: '1.0'\nid: deploy-helper\ntitle: Deploy helper\ndescription: Ships   the  release.\ntriggers: [ship it, say \"go\"]\nform: prompt\nbody_path: deploy-helper.skill.md\nparameters:\n  - name: scope\n    required: 'yes'\ninvocation:\n  side_effects: [gh-write]\n",
    )
    .expect("yaml");
    let skill = PackSkill::from_yaml(&value).expect("admitted");
    assert_eq!(scalar(&skill.id), "deploy-helper");
    assert!(skill.parameters[0].required);
    assert!(!skill.invocation.model_invocable);
    assert_eq!(skill.tools.len(), 1);
    assert_eq!(scalar(&skill.tools[0]), "*");
    let (dump, markers) = skill.exact_mapping().expect("dump");
    let graph = sk_doctrine::exact_mapping_graph(&dump, &markers).expect("graph");
    let payload = canonical_artifact_body_exact(ArtifactKind::Skill, graph.root_view())
        .expect("payload")
        .to_serde()
        .expect("scalar payload");
    assert_eq!(payload["id"], serde_json::json!("deploy-helper"));
    assert_eq!(
        payload["description"],
        serde_json::json!("Ships   the  release.")
    );
    assert_eq!(payload["form"], serde_json::json!("prompt"));
    assert_eq!(
        payload["invocation"]["model_invocable"],
        serde_json::json!(false)
    );
    assert_eq!(
        payload["parameters"][0]["required"],
        serde_json::json!(true)
    );
    // Refusals are the canonical model's, not a second parser's.
    let invalid = value.clone();
    let mut invalid_mapping = invalid.as_mapping().expect("mapping").clone();
    invalid_mapping.insert(
        serde_yaml_ng::Value::String("unknown".to_owned()),
        serde_yaml_ng::Value::Bool(true),
    );
    assert!(PackSkill::from_yaml(&serde_yaml_ng::Value::Mapping(invalid_mapping)).is_err());
}

#[test]
fn the_offering_service_serves_the_folded_skill_and_keeps_skills_required_only() {
    let (root, built_in, org, project_dir) = fixture("offering");
    write(&built_in.join("base.skill.yaml"), base_skill());
    write(&built_in.join("base.md"), BASE_BODY);
    write(
        &org.join("tune.skill.yaml"),
        "schema_version: '1.0'\nid: tune\ntitle: Tuned\ndescription: Tuned description.\nform: prompt\nbody_path: base.md\nenhances: base\n",
    );
    write_prompt_skill(&org, "org-only", "org body\n", "Org only.");

    let roots = OfferingRoots {
        built_in: Some(root.join("built_in")),
        org_roots: vec![root.join("org")],
        project: Some(root.join("project")),
    };
    let before = tree(&root);
    let service = CanonicalOfferingService::new(OfferingContext::new(root.clone(), roots, None));
    let skill = service.skill("base").expect("loads").expect("base");
    assert_eq!(scalar(&skill.title), "Tuned");
    assert_eq!(scalar(&skill.id), "base");
    // The reference's `skills` activation key is required-only: it resolves no
    // declared ids even though the repository serves records.
    assert!(service.ids_for("activated_skills").expect("ids").is_empty());
    assert!(service.ids_for("activated_skills").expect("ids").is_empty());
    assert!(CanonicalOfferingService::serves_documents(
        ArtifactKind::Skill
    ));
    assert_eq!(
        service.skill_repository().expect("repo").tier_of("base"),
        Some(PackTier::Org)
    );
    assert_eq!(
        service
            .skill("org-only")
            .expect("loads")
            .map(|s| scalar(&s.id)),
        Some("org-only".to_owned())
    );
    // A pure read/write-free consumer.
    assert_eq!(tree(&root), before, "the skill consumer wrote the fixture");
    // The project layer is optional here; the arm must still serve built-in/org.
    let _ = project_dir;
}

// ---------------------------------------------------------------------------
// Paired Source/Native proof (both actual isolated children)
// ---------------------------------------------------------------------------

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

fn paired_fixture(root: &Path) -> BTreeMap<String, String> {
    let built_in = root.join("built_in/skills");
    let org = root.join("org/skills");
    let project = root.join("project/skills");
    std::fs::create_dir_all(&built_in).expect("built-in");
    std::fs::create_dir_all(&org).expect("org");
    std::fs::create_dir_all(&project).expect("project");
    write(&built_in.join("base.skill.yaml"), base_skill());
    write(&built_in.join("base.md"), BASE_BODY);
    write(
        &org.join("tune.skill.yaml"),
        "schema_version: '1.0'\nid: tune\ntitle: Tuned\ndescription: Tuned description.\nform: prompt\nbody_path: base.md\nenhances: base\n",
    );
    BTreeMap::from([
        ("built_in".to_owned(), built_in.display().to_string()),
        ("org".to_owned(), org.display().to_string()),
        ("project".to_owned(), project.display().to_string()),
    ])
}

const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys, warnings
root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root, (str(pathlib.Path.cwd()), str(root))
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["PYTHONPATH"]).parent
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
from charter.activation.context_renderers.artifact_bodies import _format_full_artifact_payload_body
from charter.offering.pack_skills.repository import PackSkillRepository
roots = json.loads(os.environ["SK_CF44_ROOTS"])
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    repository = PackSkillRepository(
        built_in_dir=pathlib.Path(roots["built_in"]),
        org_dirs=[pathlib.Path(roots["org"])],
        project_dir=pathlib.Path(roots["project"]),
    )
    skill = repository.get("base")
    lines = _format_full_artifact_payload_body(skill)
    body = repository.body_text("base")
payload = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "ids": sorted(item.id for item in repository.list_all()),
    "provenance": repository.provenance_of("base"),
    "title": skill.title,
    "body": body,
    "lines": lines,
    "warnings": [str(item.message) for item in caught if issubclass(item.category, UserWarning)],
}
print("CF44_RECEIPT " + json.dumps(payload, sort_keys=True))
"#;

#[test]
fn cf44_native_child_and_receipt() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    isolated::assert_isolated(&root);
    for (key, name) in EXTRA_XDG_ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is asserted inside the actual Native child"
        );
    }
    let roots: serde_json::Value =
        serde_json::from_str(&std::env::var("SK_CF44_ROOTS").expect("roots env")).expect("roots");
    let built_in = PathBuf::from(roots["built_in"].as_str().expect("built_in"));
    let org = PathBuf::from(roots["org"].as_str().expect("org"));
    let project = PathBuf::from(roots["project"].as_str().expect("project"));
    let repository = PackSkillRepository::load(&built_in, &[org], Some(&project), None)
        .expect("native repository loads");
    let skill = repository.skill("base").expect("folded base");
    let body = repository.body_text("base");
    let (dump, markers) = skill.exact_mapping().expect("model dump");
    let graph = sk_doctrine::exact_mapping_graph(&dump, &markers).expect("graph");
    let payload = canonical_artifact_body_exact(ArtifactKind::Skill, graph.root_view())
        .expect("payload")
        .to_serde()
        .expect("scalar payload");
    println!(
        "CF44_RECEIPT {}",
        serde_json::json!({
            "runner": "native",
            "cwd": std::env::current_dir().expect("cwd").display().to_string(),
            "ids": repository.ids(),
            "provenance": repository.provenance_of("base"),
            "title": scalar(&skill.title),
            "body": body,
            "lines": include_full_payload_lines(&payload),
            "warnings": repository.warnings(),
        })
    );
}

fn parse_receipt(stdout: &str) -> serde_json::Value {
    let index = stdout
        .find("CF44_RECEIPT ")
        .unwrap_or_else(|| panic!("no CF44 receipt in {stdout:?}"));
    let line = stdout[index + "CF44_RECEIPT ".len()..]
        .lines()
        .next()
        .expect("receipt line");
    serde_json::from_str(line.trim()).expect("receipt JSON")
}

fn run_native_child(root: &Path, roots_env: &str) -> String {
    let repository = isolated::repository_root();
    let executable = std::env::current_exe().expect("test binary");
    let mut command = std::process::Command::new(executable);
    command
        .args([
            "--exact",
            "cf44_native_child_and_receipt",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("SK_E4_NATIVE_ROOT", root)
        .env("SK_CF44_ROOTS", roots_env)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in isolated::ROOTS.into_iter().chain(EXTRA_XDG_ROOTS) {
        command.env(key, root.join(name));
    }
    let output = command.output().expect("run the Native child");
    std::fs::write(root.join("cf44-native.stdout"), &output.stdout).expect("stdout receipt");
    std::fs::write(root.join("cf44-native.stderr"), &output.stderr).expect("stderr receipt");
    assert!(
        output.status.success(),
        "Native child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("native stdout is UTF-8")
}

#[test]
fn source_and_native_agree_on_the_folded_skill_and_its_include_payload() {
    let root = isolated::fixture("cf44-pack-skill");
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra XDG root");
    }
    let roots = paired_fixture(&root);
    let roots_env = serde_json::to_string(&roots).expect("roots JSON");
    let source_stdout = isolated::source_script(
        &root,
        SOURCE_SCRIPT,
        &[
            ("SK_CF44_ROOTS", roots_env.as_str()),
            (
                "XDG_CONFIG_DIRS",
                root.join("config-dirs").to_str().expect("path"),
            ),
            (
                "XDG_DATA_DIRS",
                root.join("data-dirs").to_str().expect("path"),
            ),
        ],
    );
    let source = parse_receipt(&source_stdout);
    assert_eq!(source["runner"], "source-python-3.11.15");
    assert_eq!(source["cwd"], root.display().to_string());

    let native_stdout = run_native_child(&root, &roots_env);
    let native = parse_receipt(&native_stdout);
    assert_eq!(native["runner"], "native");
    assert_eq!(native["cwd"], root.display().to_string());

    for key in ["ids", "provenance", "title", "body", "lines", "warnings"] {
        assert_eq!(source[key], native[key], "{key} differs from the Source");
    }
    assert_eq!(source["provenance"], serde_json::json!("org"));
    assert_eq!(source["title"], serde_json::json!("Tuned"));
    assert_eq!(source["body"], serde_json::json!(BASE_BODY));
    assert!(
        source["lines"]
            .as_array()
            .is_some_and(|lines| lines.len() > 2),
        "the Source rendered a real payload"
    );
    assert_eq!(source["warnings"], serde_json::json!([]));
}

#[test]
fn an_unhashable_overlay_id_and_an_invalid_utf8_body_abort_like_the_source() {
    let (_root, built_in, org, _project) = fixture("fatal");
    // `if item_id in built_in` hashes the raw id; a list raises TypeError.
    write(&org.join("bad.skill.yaml"), "id: [a]\n");
    let error = PackSkillRepository::load(&built_in, std::slice::from_ref(&org), None, None)
        .err()
        .expect("unhashable id aborts");
    assert!(
        error.to_string().contains("unhashable type: 'list'"),
        "{error}"
    );

    let (_root2, built_in2, org2, _project2) = fixture("utf8");
    write(
        &org2.join("bad-body.skill.yaml"),
        "schema_version: '1.0'\nid: bad-body\ntitle: T\ndescription: D\nform: prompt\nbody_path: bad-body.md\n",
    );
    std::fs::write(org2.join("bad-body.md"), [0xff, 0xfe]).expect("invalid utf8 body");
    let error = PackSkillRepository::load(&built_in2, &[org2], None, None)
        .err()
        .expect("invalid utf-8 body aborts");
    assert!(error.to_string().contains("utf-8"), "{error}");
}

#[test]
fn a_different_id_override_folds_onto_its_org_target_and_wrapper_argv_is_admitted() {
    let (_root, built_in, org, project) = fixture("override");
    write_prompt_skill(&org, "org-base", "org base body\n", "Org base.");
    write(
        &project.join("org-base-alt.skill.yaml"),
        "schema_version: '1.0'\nid: org-base-alt\ntitle: Alt\ndescription: Alt.\nform: prompt\nbody_path: org-base.md\noverrides: org-base\n",
    );
    write(&project.join("org-base.md"), "alt body\n");
    // A wrapper's `cli:` target follows the model's argv guard; metacharacters
    // are refused by admission, not by a second parser here.
    write(
        &org.join("wrap.skill.yaml"),
        "schema_version: '1.0'\nid: wrap\ntitle: W\ndescription: W.\nform: wrapper\nexpands_to:\n  target: cli:spec-kitty consolidate --mission m\n  args: --dry-run\n",
    );
    write(
        &org.join("bad-wrap.skill.yaml"),
        "schema_version: '1.0'\nid: bad-wrap\ntitle: B\ndescription: B.\nform: wrapper\nexpands_to:\n  target: cli:spec-kitty; rm -rf /\n",
    );
    let repository = PackSkillRepository::load(&built_in, &[org], Some(&project), None)
        .expect("loads with skips");
    let folded = repository.skill("org-base").expect("folded override");
    assert_eq!(scalar(&folded.title), "Alt");
    assert_eq!(repository.tier_of("org-base"), Some(PackTier::Project));
    assert_eq!(
        repository.source_path("org-base"),
        Some(project.join("org-base-alt.skill.yaml").as_path())
    );
    assert!(repository.skill("org-base-alt").is_none());
    let wrapper = repository.skill("wrap").expect("wrapper admitted");
    assert_eq!(wrapper.form, sk_doctrine::SkillForm::Wrapper);
    assert_eq!(
        wrapper.expands_to.as_ref().map(|item| scalar(&item.target)),
        Some("cli:spec-kitty consolidate --mission m".to_owned())
    );
    assert!(repository.skill("bad-wrap").is_none());
}
