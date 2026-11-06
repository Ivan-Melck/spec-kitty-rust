//! Source-first differential: the project pack-skill catalog.
//!
//! The fixture (`tests/fixtures/pack_skill_oracle.json`) is a **recorded Source
//! run**, not a hand-written expectation: the pinned CPython 3.11.15 oracle
//! (`.venv-rust-oracle`, 53 distributions, asserted temporary
//! CWD/HOME/all-XDG/TMP) executed
//! `specify_cli/skills/catalog.py:resolve_project_skill_catalog` over the same
//! fixture this test rebuilds and recorded the resolved skills (name, origin,
//! `source_ref`, `source_hash`), the staged tree bytes and the staged file
//! modes, plus the refusal messages for four invalid shapes. The probe that
//! produced it is committed beside the fixture.
//!
//! The test fails closed when the fixture is missing: an unavailable oracle is
//! never a passing zero-comparison.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sk_install::skills::pack_skill::{
    PACK_SKILL_STAGING_REL, resolve_project_skill_catalog, resolve_project_skill_catalog_with,
    shipped_catalog,
};
use sk_install::{InstallError, SkillCatalog};

const FIXTURE_REL: &str = "tests/fixtures/pack_skill_oracle.json";

fn fixture() -> Value {
    let path = match std::env::var_os("SK_PACK_SKILL_ORACLE") {
        Some(value) => PathBuf::from(value),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_REL),
    };
    let raw = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the recorded Source pack-skill oracle {} is unavailable ({error}); \
             a missing oracle is a failure, never a pass",
            path.display()
        )
    });
    serde_json::from_slice(&raw).expect("recorded oracle is JSON")
}

fn case<'a>(oracle: &'a Value, name: &str) -> &'a Value {
    &oracle["cases"][name]
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).unwrap();
    std::fs::write(path, text).unwrap();
}

const PACK_YAML: &str = "pack_id: 01ARWG13C000000000000000AC\npack_version: 1.0.0\nparent_pack: null\nname: acme\npack_level: org\n";

const PROMPT_SKILL: &str = "body_path: deploy-helper.skill.md\ndescription: Ships   the  release.\nform: prompt\nid: deploy-helper\nschema_version: '1.0'\ntitle: Deploy helper\ntriggers:\n- ship it\n- say \"go\"\n";

const WRAPPER_SKILL: &str = "description: Runs the canonical consolidate command.\nexpands_to:\n  args: $ARGUMENTS\n  target: builtin:spec-kitty.consolidate\nform: wrapper\nid: release-wrapper\nschema_version: '1.0'\ntitle: Release wrapper\n";

fn make_pack(pack: &Path, namespace: Option<&str>, with_wrapper: bool, required: Option<&str>) {
    write(&pack.join("pack.yaml"), PACK_YAML);
    let mut charter = String::from("org_name: acme-org\n");
    if let Some(namespace) = namespace {
        charter.push_str(&format!("skill_namespace: {namespace}\n"));
    }
    if let Some(required) = required {
        charter.push_str(&format!("required_skills:\n- {required}\n"));
    }
    write(&pack.join("org-charter.yaml"), &charter);
    let mut fragment = String::from(
        "nodes:\n  - id: deploy-helper\n    kind: skills\n    title: \"Deploy helper\"\n    body_path: skills/deploy-helper.skill.yaml\n",
    );
    if with_wrapper {
        fragment.push_str(
            "  - id: release-wrapper\n    kind: skills\n    title: \"Release wrapper\"\n    body_path: skills/release-wrapper.skill.yaml\n",
        );
    }
    fragment.push_str(
        "  - id: release-proc\n    kind: procedures\n    title: \"Release procedure\"\n    body_path: procedures/release-proc.procedure.yaml\nedges:\n  - source: \"skill:deploy-helper\"\n    target: \"procedure:release-proc\"\n    relation: requires\n",
    );
    if with_wrapper {
        fragment.push_str(
            "  - source: \"skill:release-wrapper\"\n    target: \"procedure:release-proc\"\n    relation: requires\n",
        );
    }
    write(&pack.join("drg").join("fragment.yaml"), &fragment);
    write(
        &pack.join("procedures").join("release-proc.procedure.yaml"),
        "schema_version: \"1.0\"\nid: release-proc\nname: release-proc fixture procedure\npurpose: >\n  Substance carried by a procedure.\nentry_condition: >\n  A skill requires it.\nexit_condition: >\n  Done.\nsteps:\n  - title: Act\n    description: >\n      Do the thing.\n    actor: agent\n",
    );
    let skills = pack.join("skills");
    write(&skills.join("deploy-helper.skill.yaml"), PROMPT_SKILL);
    write(
        &skills.join("deploy-helper.skill.md"),
        "Run deploy-helper: $ARGUMENTS\n",
    );
    if with_wrapper {
        write(&skills.join("release-wrapper.skill.yaml"), WRAPPER_SKILL);
    }
}

fn make_project(project: &Path, pack: Option<&Path>, ledger: &str, namespace: Option<&str>) {
    let mut config = String::new();
    if pack.is_some() || namespace.is_some() {
        config.push_str("charter_packs:\n");
    }
    if let Some(pack) = pack {
        config.push_str(&format!(
            "  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
            pack.display()
        ));
    }
    if let Some(namespace) = namespace {
        config.push_str(&format!("  project:\n    skill_namespace: {namespace}\n"));
    }
    config.push_str("agents:\n  available:\n    - claude\n");
    write(&project.join(".kittify").join("config.yaml"), &config);
    write(
        &project
            .join(".kittify")
            .join("charter")
            .join("activation.yaml"),
        ledger,
    );
}

/// Project-relative tree snapshot: `path -> digest-or-link` (directories end `/`).
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
                    format!(
                        "sha256:{:x}",
                        sha2::Sha256::digest(std::fs::read(&entry).unwrap())
                    ),
                ));
            }
        }
    }
    use sha2::Digest;
    let mut out = Vec::new();
    if root.is_dir() {
        walk(root, root, &mut out);
    }
    out
}

fn oracle_tree(value: &Value) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = value
        .as_object()
        .expect("tree object")
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect();
    entries.sort();
    entries
}

fn pack_entries(catalog: &SkillCatalog) -> Vec<Value> {
    catalog
        .skills()
        .iter()
        .filter(|skill| skill.is_pack())
        .map(|skill| {
            serde_json::json!({
                "name": skill.name,
                "origin": skill.origin,
                "source_hash": skill.source_hash,
            })
        })
        .collect()
}

fn sandbox(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("sk-c4-pack-{name}-{}", std::process::id()));
    if base.exists() {
        std::fs::remove_dir_all(&base).unwrap();
    }
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn the_shipped_catalog_matches_the_recorded_source_set() {
    let oracle = fixture();
    let shipped = shipped_catalog();
    let recorded: Vec<String> = case(&oracle, "case10")["shipped_names"]
        .as_array()
        .expect("shipped names")
        .iter()
        .map(|name| name.as_str().unwrap().to_string())
        .collect();
    assert_eq!(shipped.names(), recorded);
    // The shipped set carries no pack provenance.
    assert!(
        shipped
            .skills()
            .iter()
            .all(|skill| skill.origin == "builtin"
                && skill.source_ref.is_empty()
                && skill.source_hash.is_empty())
    );
}

#[test]
fn one_prompt_skill_in_force_is_staged_tagged_and_rendered_like_the_source() {
    let oracle = fixture();
    let root = sandbox("case1");
    let pack = root.join("pack");
    make_pack(&pack, Some("acme"), true, None);
    let project = root.join("project");
    make_project(
        &project,
        Some(&pack),
        "activated_skills:\n- deploy-helper\n",
        None,
    );

    let catalog = resolve_project_skill_catalog(&project, true).expect("resolves");
    let recorded = &case(&oracle, "case1");
    let pack_skills: Vec<&Value> = recorded["skills"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|skill| skill["origin"] == "pack")
        .collect();
    assert_eq!(pack_skills.len(), 1);
    let expected = pack_skills[0];

    let staged = catalog
        .skills()
        .iter()
        .find(|skill| skill.is_pack())
        .expect("one pack skill");
    assert_eq!(staged.name, expected["name"]);
    assert_eq!(staged.origin, expected["origin"]);
    assert_eq!(staged.source_hash, expected["source_hash"]);
    // The recorded `source_ref` is absolute inside the sandbox; the pack-relative
    // suffix is the part the sandbox can agree on.
    let suffix = expected["source_ref"]
        .as_str()
        .unwrap()
        .split("/case1/")
        .nth(1)
        .expect("recorded suffix");
    assert!(
        staged.source_ref.ends_with(suffix),
        "{} does not end with {suffix}",
        staged.source_ref
    );
    assert!(staged.source_ref.starts_with('/'));
    let prepared = staged;
    let canonical = sk_install::skills::pack_skill::source_hash_payload(
        &sk_install::skills::pack_skill::PackSkill::from_yaml(
            &serde_yaml_ng::from_str(PROMPT_SKILL).unwrap(),
        )
        .unwrap(),
        &["procedure:release-proc".to_string()],
        &prepared.name,
    );
    assert_eq!(
        canonical,
        case(&oracle, "case1_hash_inputs")["canonical_json"]
            .as_str()
            .unwrap(),
        "the hash payload differs from the recorded Source json.dumps(sort_keys=True)"
    );

    // The staged tree, bytes and mode match the recorded Source tree exactly.
    let staging = project.join(PACK_SKILL_STAGING_REL);
    assert_eq!(tree(&staging), oracle_tree(&recorded["staging"]));
    let recorded_text = &recorded["staging_text"];
    for (relative, text) in recorded_text.as_object().unwrap() {
        let path = project.join(relative);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text.as_str().unwrap(),
            "staged bytes differ from the recorded Source render for {relative}"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = staging.join(&staged.name).join("SKILL.md");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    // A warm resolution rewrites nothing (recorded: the mtime is unchanged).
    let target = staging.join(&staged.name).join("SKILL.md");
    let before = std::fs::metadata(&target).unwrap().modified().unwrap();
    assert_eq!(*case(&oracle, "case2_warm_stable"), Value::Bool(true));
    let again = resolve_project_skill_catalog(&project, true).expect("resolves");
    assert_eq!(
        std::fs::metadata(&target).unwrap().modified().unwrap(),
        before
    );
    assert_eq!(pack_entries(&again), pack_entries(&catalog));
}

#[test]
fn nothing_in_force_retires_the_staged_set() {
    let oracle = fixture();
    let root = sandbox("case3");
    let pack = root.join("pack");
    make_pack(&pack, Some("acme"), true, None);
    let project = root.join("project");
    make_project(
        &project,
        Some(&pack),
        "activated_skills:\n- deploy-helper\n",
        None,
    );
    resolve_project_skill_catalog(&project, true).expect("resolves");
    // A stale leftover (as a crashed run leaves) is swept too.
    write(
        &project
            .join(PACK_SKILL_STAGING_REL)
            .join("stale-leftover")
            .join("SKILL.md"),
        "stale\n",
    );

    make_project(&project, Some(&pack), "activated_skills: []\n", None);
    let catalog = resolve_project_skill_catalog(&project, true).expect("resolves");
    let recorded = case(&oracle, "case3");
    assert_eq!(
        catalog.names(),
        recorded["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    );
    assert!(pack_entries(&catalog).is_empty());
    assert_eq!(
        tree(&project.join(PACK_SKILL_STAGING_REL)),
        oracle_tree(&recorded["staging"])
    );
}

#[test]
fn read_only_resolution_never_writes_the_project_tree() {
    let oracle = fixture();
    let root = sandbox("case4");
    let pack = root.join("pack");
    make_pack(&pack, Some("acme"), true, None);
    let project = root.join("project");
    make_project(
        &project,
        Some(&pack),
        "activated_skills:\n- deploy-helper\n",
        None,
    );
    let before = tree(&project);

    let catalog = resolve_project_skill_catalog(&project, false).expect("resolves");
    let recorded = case(&oracle, "case4");
    assert_eq!(
        Value::Bool(tree(&project) == before),
        recorded["project_tree_unchanged"]
    );
    assert_eq!(
        Value::Bool(
            catalog
                .skills()
                .iter()
                .filter(|skill| skill.is_pack())
                .flat_map(|skill| skill.files.iter())
                .all(|file| !file.source.starts_with(&project))
        ),
        recorded["staged_outside_project"]
    );
    assert!(catalog.skills().iter().any(|skill| skill.is_pack()));
}

#[test]
fn an_absent_activated_skills_key_takes_the_org_required_set() {
    let oracle = fixture();
    let root = sandbox("case5");
    let pack = root.join("pack");
    make_pack(&pack, Some("acme"), true, Some("release-wrapper"));
    let project = root.join("project");
    make_project(&project, Some(&pack), "{}\n", None);

    let catalog = resolve_project_skill_catalog(&project, true).expect("resolves");
    let recorded = case(&oracle, "case5");
    let expected: Vec<Value> = recorded["pack_skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            serde_json::json!({
                "name": item["name"],
                "origin": item["origin"],
                "source_hash": item["source_hash"],
            })
        })
        .collect();
    assert_eq!(pack_entries(&catalog), expected);
    for item in recorded["pack_skills"].as_array().unwrap() {
        let name = item["name"].as_str().unwrap();
        let path = project
            .join(PACK_SKILL_STAGING_REL)
            .join(name)
            .join("SKILL.md");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            item["text"].as_str().unwrap(),
            "wrapper render differs from the recorded Source render"
        );
    }
    assert_eq!(
        tree(&project.join(PACK_SKILL_STAGING_REL)),
        oracle_tree(&recorded["staging"])
    );
}

fn refusal_detail(error: InstallError) -> String {
    match error {
        InstallError::PackSkillCatalog { detail } => detail,
        other => panic!("expected a pack-skill refusal, got {other}"),
    }
}

#[test]
fn every_recorded_refusal_is_refused_with_zero_writes() {
    let oracle = fixture();
    for (name, needle) in [
        ("case6_no_namespace", "no skill namespace"),
        ("case7_hostile_namespace", "is not valid"),
        ("case8_builtin_collision", "name of a built-in skill"),
        ("case9_project_namespace", "no skill namespace"),
    ] {
        let recorded = case(&oracle, name);
        assert_eq!(recorded["refused"], Value::Bool(true), "{name}");
        let root = sandbox(name);
        let pack = root.join("pack");
        let mut ledger = "activated_skills:\n- deploy-helper\n".to_string();
        let namespace = match name {
            "case7_hostile_namespace" => Some("../x"),
            "case6_no_namespace" | "case9_project_namespace" => None,
            _ => Some("acme"),
        };
        make_pack(&pack, namespace, false, None);
        let project = root.join("project");
        let mut project_namespace = None;
        if name == "case9_project_namespace" {
            project_namespace = Some("proj");
            // A project-tier namespace cannot rescue an org-tier skill.
            ledger = "activated_skills:\n- deploy-helper\n".to_string();
        }
        make_project(&project, Some(&pack), &ledger, project_namespace);
        let before = tree(&project);

        let error = if name == "case8_builtin_collision" {
            let shipped_root = root.join("builtin");
            write(
                &shipped_root.join("acme-deploy-helper").join("SKILL.md"),
                "---\nname: x\ndescription: y\n---\n# x\n",
            );
            let shipped = SkillCatalog::discover(&shipped_root).expect("builtin fixture");
            resolve_project_skill_catalog_with(&project, Some(&shipped), true)
                .expect_err("collision refused")
        } else {
            resolve_project_skill_catalog(&project, true).expect_err("refused")
        };
        let detail = refusal_detail(error);
        assert!(
            detail.contains(needle),
            "{name}: {detail:?} does not mention {needle:?}"
        );
        assert_eq!(
            Value::Bool(tree(&project) == before),
            recorded["project_unchanged"],
            "{name} wrote into the project tree"
        );
        assert!(!project.join(PACK_SKILL_STAGING_REL).exists(), "{name}");
    }
}

#[test]
fn a_pack_skill_never_enters_the_user_global_skill_root() {
    let root = sandbox("global");
    let pack = root.join("pack");
    make_pack(&pack, Some("acme"), true, None);
    let project = root.join("project");
    make_project(
        &project,
        Some(&pack),
        "activated_skills:\n- deploy-helper\n",
        None,
    );
    let catalog = resolve_project_skill_catalog(&project, true).expect("resolves");

    let report = sk_install::install_skills_for_agents(
        &project,
        &["claude".to_string()],
        &catalog,
        &sk_install::SkillInstallOptions::with_version("test"),
    )
    .expect("installs");

    // The pack skill is a project copy with pack provenance.
    let pack_entry = report
        .entries
        .iter()
        .find(|entry| entry.skill_name == "acme-deploy-helper")
        .expect("pack skill installed");
    assert_eq!(pack_entry.origin, "pack");
    assert_eq!(pack_entry.delivery_mode, "copy");
    assert!(pack_entry.source_hash.starts_with("sha256:"));
    assert!(
        project
            .join(".claude/skills/acme-deploy-helper/SKILL.md")
            .is_file()
    );
    // A shipped skill is also written to the user-global canonical root; the
    // pack skill is never written there.
    let home = sk_kernel::paths::home_directory().expect("home");
    assert!(!home.join(".claude/skills/acme-deploy-helper").exists());
    let shipped_name = report
        .entries
        .iter()
        .find(|entry| entry.origin == "builtin" && entry.skill_name != "acme-deploy-helper")
        .map(|entry| entry.skill_name.clone())
        .expect("a shipped skill");
    assert!(
        home.join(".claude/skills")
            .join(&shipped_name)
            .join("SKILL.md")
            .is_file(),
        "the shipped skill {shipped_name} is missing from the user-global root"
    );
}

#[test]
fn a_retired_pack_skill_is_removed_and_a_modified_one_is_preserved() {
    let root = sandbox("retire");
    let pack = root.join("pack");
    make_pack(&pack, Some("acme"), false, None);
    let project = root.join("project");
    make_project(
        &project,
        Some(&pack),
        "activated_skills:\n- deploy-helper\n",
        None,
    );
    let agents = vec!["claude".to_string()];
    let catalog = resolve_project_skill_catalog(&project, true).expect("resolves");
    sk_install::install_skills_for_agents(
        &project,
        &agents,
        &catalog,
        &sk_install::SkillInstallOptions::with_version("test"),
    )
    .expect("installs");
    let installed = project.join(".claude/skills/acme-deploy-helper/SKILL.md");
    assert!(installed.is_file());

    // Nothing in force any more: the catalog retires the copy.
    make_project(&project, Some(&pack), "activated_skills: []\n", None);
    let retired = resolve_project_skill_catalog(&project, true).expect("resolves");
    let report = sk_install::install_skills_for_agents(
        &project,
        &agents,
        &retired,
        &sk_install::SkillInstallOptions::with_version("test"),
    )
    .expect("retires");
    assert!(!installed.exists());
    let manifest = sk_install::ManagedSkillManifest::load(&project).expect("manifest");
    assert!(manifest.entries.iter().all(|entry| entry.origin != "pack"));
    assert!(report.warnings.is_empty() || !report.preserved.is_empty());

    // A locally edited copy is preserved, not silently destroyed.
    make_project(
        &project,
        Some(&pack),
        "activated_skills:\n- deploy-helper\n",
        None,
    );
    let catalog = resolve_project_skill_catalog(&project, true).expect("resolves");
    sk_install::install_skills_for_agents(
        &project,
        &agents,
        &catalog,
        &sk_install::SkillInstallOptions::with_version("test"),
    )
    .expect("installs");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&installed).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&installed, permissions).unwrap();
    }
    std::fs::write(&installed, "my edits\n").unwrap();
    make_project(&project, Some(&pack), "activated_skills: []\n", None);
    let retired = resolve_project_skill_catalog(&project, true).expect("resolves");
    let report = sk_install::install_skills_for_agents(
        &project,
        &agents,
        &retired,
        &sk_install::SkillInstallOptions::with_version("test"),
    )
    .expect("retires");
    assert_eq!(std::fs::read_to_string(&installed).unwrap(), "my edits\n");
    assert!(
        report
            .preserved
            .iter()
            .any(|path| path.ends_with("acme-deploy-helper/SKILL.md"))
    );
}

// ---------------------------------------------------------------------------
// User-global runtime provisioning (`ensure_runtime`)
// ---------------------------------------------------------------------------

const RUNTIME_FIXTURE_REL: &str = "tests/fixtures/runtime_oracle.json";

fn runtime_fixture() -> Value {
    let path = match std::env::var_os("SK_RUNTIME_ORACLE") {
        Some(value) => PathBuf::from(value),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join(RUNTIME_FIXTURE_REL),
    };
    let raw = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the recorded Source runtime oracle {} is unavailable ({error}); \
             a missing oracle is a failure, never a pass",
            path.display()
        )
    });
    serde_json::from_slice(&raw).expect("recorded runtime oracle is JSON")
}

/// The managed subtrees of a runtime home (`missions/`, minus `custom/`).
fn managed_tree(home: &Path) -> Vec<(String, String)> {
    tree(home)
        .into_iter()
        .filter(|(path, _)| path.starts_with("missions/") && !path.starts_with("missions/custom/"))
        .collect()
}

#[test]
fn global_runtime_provisioning_matches_the_recorded_source_over_a_mixed_home() {
    let oracle = runtime_fixture();
    let root = sandbox("runtime");
    let home = root.join("home").join(".kittify");
    let outside = root.join("outside-scripts");
    write(&outside.join("keep.txt"), "user content\n");
    write(&home.join("missions/software-dev/stale.txt"), "stale\n");
    write(&home.join("missions/custom/mine/x.md"), "mine\n");
    write(&home.join("config.yaml"), "user: config\n");
    write(&home.join("cache/version.lock"), "0.0.1");
    write(&home.join("AGENTS.md"), "old agents\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, home.join("scripts")).unwrap();

    let version = oracle["version_lock"].as_str().expect("recorded version");
    // The recorded oracle resolved `~/.kittify` against `<root>/home`, so the
    // owner anchor (and therefore every inventory key) is `<root>`; supply the
    // same user home instead of inheriting the test process's environment.
    let user_home = root.join("home");
    let report = sk_install::migrations::completion_modern::global_runtime::ensure_runtime_with(
        &home, &user_home, version,
    )
    .expect("provisions")
    .expect("package assets present");

    // The managed mission trees are byte-identical to the recorded Source tree.
    let recorded_after = oracle["after"].as_object().expect("recorded tree");
    let mut recorded_managed: Vec<(String, String)> = recorded_after
        .iter()
        .filter(|(path, _)| path.starts_with("missions/") && !path.starts_with("missions/custom/"))
        .map(|(path, value)| (path.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect();
    recorded_managed.sort();
    let mut provisioned = managed_tree(&home);
    provisioned.sort();
    assert_eq!(
        provisioned, recorded_managed,
        "the provisioned mission trees differ from the recorded Source tree"
    );

    // User-owned state is preserved, and the foreign `scripts` symlink is not
    // followed or rewritten (the package ships no `scripts` tree).
    assert_eq!(
        std::fs::read_to_string(home.join("AGENTS.md")).unwrap(),
        oracle["agents_after"].as_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(home.join("config.yaml")).unwrap(),
        "user: config\n"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("missions/custom/mine/x.md")).unwrap(),
        "mine\n"
    );
    // Source OVERLAYS a managed tree: an operator file inside it is preserved
    // (the recorded tree still carries `missions/software-dev/stale.txt`).
    assert_eq!(
        std::fs::read_to_string(home.join("missions/software-dev/stale.txt")).unwrap(),
        "stale\n"
    );
    assert_eq!(
        std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
        "user content\n"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("cache/version.lock")).unwrap(),
        version
    );
    assert!(report.changed());

    // A second pass is a no-op over the managed trees (recorded: warm stable).
    let before = managed_tree(&home);
    sk_install::migrations::completion_modern::global_runtime::ensure_runtime_with(
        &home, &user_home, version,
    )
    .expect("provisions again");
    assert_eq!(managed_tree(&home), before);
    assert_eq!(oracle["warm_tree_stable"], Value::Bool(true));

    // C6: the owner record and the persistent flock anchor are now written, and
    // their bytes are the recorded Source bytes (the C4 gap is closed).
    let inventory = home.join("cache/runtime_bootstrap-assets.json");
    let written = std::fs::read_to_string(&inventory).expect("owner inventory written");
    assert_eq!(
        written,
        oracle["assets_manifest"]
            .as_str()
            .expect("recorded inventory"),
        "the owner inventory differs from the recorded Source bytes"
    );
    let lock = home.join("cache/.update.lock");
    assert_eq!(
        std::fs::read(&lock).expect("persistent lock written"),
        oracle["update_lock_bytes"]
            .as_str()
            .expect("recorded lock bytes")
            .as_bytes(),
        "the persistent lock node differs from the recorded Source bytes"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&lock).expect("lock").permissions().mode() & 0o7777,
            0o644,
            "the persistent lock is not 0644 like the recorded Source node"
        );
    }
}

// ---------------------------------------------------------------------------
// The shared destructive-removal guard (B4 finding, registered by the parent)
// ---------------------------------------------------------------------------

const GUARD_FIXTURE_REL: &str = "tests/fixtures/preserve_guard_oracle.json";

fn guard_fixture() -> Value {
    let path = match std::env::var_os("SK_PRESERVE_GUARD_ORACLE") {
        Some(value) => PathBuf::from(value),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join(GUARD_FIXTURE_REL),
    };
    let raw = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the recorded Source guard oracle {} is unavailable ({error}); \
             a missing oracle is a failure, never a pass",
            path.display()
        )
    });
    serde_json::from_slice(&raw).expect("recorded guard oracle is JSON")
}

#[test]
fn an_unowned_kittify_candidate_is_archived_under_kittify_like_the_source() {
    use sk_install::preserve::{Prover, RemovalVerdict, guard_destructive_removal};

    let oracle = guard_fixture();
    let recorded = &oracle["case_kittify_archive"];
    let root = sandbox("guard");
    let project = root.join("project");
    let candidate = project.join(".kittify").join("retired-release-skill.md");
    write(&candidate, "operator content\n");

    let prover = Prover::ManagedPath(vec![".claude/commands".to_string()]);
    let verdict = guard_destructive_removal(
        &candidate,
        &project,
        &prover,
        false,
        Some(&project.join(".kittify")),
        false,
    )
    .expect("guard runs");

    // The recorded Source run archives the candidate into `.kittify/.backup-*/`
    // and removes it; it never preserves it in place.
    assert_eq!(recorded["owned"], Value::Bool(false));
    assert_eq!(recorded["candidate_exists"], Value::Bool(false));
    let RemovalVerdict::Archived { path, .. } = &verdict else {
        panic!("expected an archive, got {verdict:?}");
    };
    assert!(
        !candidate.exists(),
        "the archived candidate must be removed"
    );
    let relative = path
        .strip_prefix(project.join(".kittify"))
        .expect("archived under .kittify")
        .to_string_lossy()
        .replace('\\', "/");
    // The recorded Source shape is `.kittify/.backup-<UTC stamp>/.kittify/<candidate>`:
    // a fresh stamp per run, so the test asserts the SHAPE, not the recorded stamp.
    let recorded_rel = recorded["backup_path_rel"].as_str().expect("recorded");
    assert!(recorded_rel.starts_with(".kittify/.backup-"));
    let parts: Vec<&str> = relative.split('/').collect();
    assert!(
        parts
            .first()
            .is_some_and(|part| part.starts_with(".backup-")),
        "the archive dir is not a .backup-* directory: {relative}"
    );
    assert_eq!(
        parts.get(1).copied(),
        Some(".kittify"),
        "the archived path must keep the candidate's project-relative path"
    );
    // The archived content is verbatim, under the candidate's project-relative path.
    assert_eq!(std::fs::read_to_string(path).unwrap(), "operator content\n");
    assert!(path.ends_with(".kittify/retired-release-skill.md"));
}

#[test]
fn an_archive_parent_inside_the_removed_path_still_fails_closed() {
    use sk_install::preserve::{Prover, RemovalVerdict, guard_destructive_removal};

    let root = sandbox("guard-recursive");
    let project = root.join("project");
    let tree = project.join(".kittify").join("legacy-tree");
    write(&tree.join("notes.md"), "mine\n");
    let prover = Prover::ManagedPath(vec![".claude/commands".to_string()]);
    // An archive parent inside the tree being removed would recurse into itself:
    // the only relation the guard still refuses (a real data-loss guard, not a
    // refusal of the Source-valid `.kittify/` case above).
    let verdict = guard_destructive_removal(
        &tree,
        &project,
        &prover,
        true,
        Some(&tree.join(".backup")),
        false,
    )
    .expect("guard runs");
    assert!(matches!(verdict, RemovalVerdict::Preserved { .. }));
    assert_eq!(
        std::fs::read_to_string(tree.join("notes.md")).unwrap(),
        "mine\n"
    );
}
