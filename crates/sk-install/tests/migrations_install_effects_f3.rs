//! Behaviour of the three F3 install effects:
//!
//! * `3.1.1_direct_canonical_commands` — thin shims + full prompts + guarded
//!   stale cleanup (the `rewrite_agent_shims` contract);
//! * `3.0.3_globalize_skill_pack` — the user-global relink pass (symlink
//!   delivery retained, copies installed for missing files, manifest stamped);
//! * `3.2.0rc35_unified_bundle` — the canonical bundle report.
//!
//! The shim/global URL material is compared with the pinned Source shapes
//! recorded in `render::shim`'s unit tests and in the F3 report's Source probe;
//! these tests exercise the migration bodies themselves (detect/can_apply/apply
//! and their file effects).

use std::path::{Path, PathBuf};

use tempfile::TempDir;

fn project(agents: &[&str]) -> TempDir {
    let sandbox = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(sandbox.path().join(".kittify")).expect("kittify");
    let list: Vec<String> = agents.iter().map(|agent| format!("- {agent}")).collect();
    std::fs::write(
        sandbox.path().join(".kittify/config.yaml"),
        format!(
            "tools:\n  available:\n{}\n",
            list.join("\n").replace("- ", "    - ")
        ),
    )
    .expect("config");
    sandbox
}

fn native(id: &str) -> &'static sk_install::upgrade::NativeMigration {
    sk_install::migrations::completion_modern::IMPLEMENTATIONS
        .iter()
        .find(|migration| migration.id == id)
        .unwrap_or_else(|| panic!("no modern implementation for {id}"))
}

fn shim_path(root: &Path, directory: &str, command: &str, extension: &str) -> PathBuf {
    root.join(directory)
        .join(format!("spec-kitty.{command}.{extension}"))
}

#[test]
fn direct_canonical_commands_writes_shims_and_prompts_and_preserves_user_files() {
    let sandbox = project(&["claude"]);
    let root = sandbox.path();
    // A stale package-generated file (version marker proves ownership).
    // A CLI-driven file with the old shim dispatch triggers detection and is
    // regenerated in place.
    let stale = shim_path(root, ".claude/commands", "review", "md");
    std::fs::create_dir_all(stale.parent().expect("parent")).expect("commands");
    std::fs::write(
        &stale,
        "---\ndescription: old\n---\n<!-- spec-kitty-command-version: 1.0.0 -->\n\n`spec-kitty agent shim review`\n",
    )
    .expect("stale");
    // A stale package-generated file for a command this build no longer ships
    // is swept by the guarded cleanup (its version marker proves ownership).
    let orphan = shim_path(root, ".claude/commands", "legacything", "md");
    std::fs::write(
        &orphan,
        "<!-- spec-kitty-command-version: 1.0.0 -->\n\n`spec-kitty agent shim legacything`\n",
    )
    .expect("orphan");
    // An unproven user-authored file must survive.
    let user = shim_path(root, ".claude/commands", "custom", "md");
    std::fs::write(&user, "# my own command\n").expect("user file");

    let migration = native("3.1.1_direct_canonical_commands");
    assert!((migration.detect)(root).expect("detect"));
    let outcome = (migration.apply)(root).expect("apply");
    assert!(outcome.changed);

    let implement = std::fs::read_to_string(shim_path(root, ".claude/commands", "implement", "md"))
        .expect("implement shim");
    assert!(implement.contains("<!-- spec-kitty-command-version: "));
    assert!(implement.contains("`spec-kitty agent action implement $ARGUMENTS --agent claude`"));
    assert!(!implement.contains("spec-kitty agent shim"));
    for command in [
        "review",
        "accept",
        "consolidate",
        "status",
        "tasks-finalize",
    ] {
        assert!(
            shim_path(root, ".claude/commands", command, "md").is_file(),
            "{command} shim must exist"
        );
    }
    // Prompt-driven commands are full templates, not shims.
    let specify = std::fs::read_to_string(shim_path(root, ".claude/commands", "specify", "md"))
        .expect("specify prompt");
    assert!(!specify.contains("spec-kitty agent shim"));
    assert!(specify.lines().count() > 10);

    assert!(
        stale.is_file(),
        "a CLI-driven file is regenerated, not deleted"
    );
    assert!(
        !orphan.exists(),
        "a proven stale file for a removed command is swept"
    );
    assert_eq!(
        std::fs::read_to_string(&user).expect("user file"),
        "# my own command\n",
        "an unproven user file is preserved"
    );
    assert!(!(migration.detect)(root).expect("second detect"));
}

#[test]
fn globalize_skill_pack_stamps_the_manifest_and_keeps_a_verified_link() {
    use sk_install::skills::{
        ManagedFileEntry, ManagedSkillManifest, SKILLS_MANIFEST_REL, global_skill_root,
    };

    let sandbox = project(&["claude"]);
    let root = sandbox.path();
    let migration = native("3.0.3_globalize_skill_pack");
    assert!(
        (migration.detect)(root).expect("detect"),
        "a project with no installed canonical skill needs the relink"
    );

    let outcome = (migration.apply)(root).expect("apply");
    assert!(outcome.changed);
    let manifest = ManagedSkillManifest::load(root).expect("manifest written");
    assert_eq!(manifest.spec_kitty_version, "3.0.3");
    assert!(
        manifest
            .entries
            .iter()
            .any(|entry| entry.installed_path.starts_with(".claude/skills/")),
        "canonical skills are installed for the configured agent"
    );

    // Now model the 3.0.3 layout: one installed file becomes a link into the
    // user-global canonical root, and the manifest records `symlink` delivery.
    let Some(home) = global_skill_root("claude") else {
        // An unresolvable home is the honest reason to skip the link half.
        return;
    };
    let entry = manifest
        .entries
        .iter()
        .find(|entry| entry.installed_path.starts_with(".claude/skills/"))
        .expect("a claude skill entry")
        .clone();
    let relative = entry
        .installed_path
        .strip_prefix(".claude/skills/")
        .expect("skill path");
    let global_file = home.join(relative);
    std::fs::create_dir_all(global_file.parent().expect("parent")).expect("global dir");
    let content = std::fs::read(root.join(&entry.installed_path)).expect("installed bytes");
    std::fs::write(&global_file, &content).expect("global file");
    let project_file = root.join(&entry.installed_path);
    if global_file.parent() == Some(project_file.parent().expect("parent")) {
        // A test home that resolves onto the project would make this a
        // meaningless link; skip the link assertion instead of faking it.
        return;
    }
    std::fs::remove_file(&project_file).expect("remove copy");
    std::os::unix::fs::symlink(&global_file, &project_file).expect("link");

    let mut link_entry: ManagedFileEntry = entry.clone();
    link_entry.delivery_mode = "symlink".to_string();
    let mut updated = manifest.clone();
    updated.entries.retain(|existing| {
        existing.installed_path != link_entry.installed_path
            || existing.agent_key != link_entry.agent_key
    });
    updated.add_entry(link_entry.clone());
    updated.save(root).expect("save manifest");
    assert!(root.join(SKILLS_MANIFEST_REL).is_file());

    (migration.apply)(root).expect("second apply");
    let after = std::fs::read_link(&project_file).expect("link retained");
    assert_eq!(after, global_file, "a verified global link is not replaced");
    let manifest = ManagedSkillManifest::load(root).expect("manifest");
    let retained = manifest
        .find(&link_entry.installed_path)
        .expect("link entry retained");
    assert!(retained.is_symlink_delivery());
}

#[test]
fn unified_bundle_reports_the_v2_manifest_state() {
    let sandbox = tempfile::tempdir().expect("tempdir");
    let root = sandbox.path();
    std::fs::create_dir_all(root.join(".git")).expect(".git dir");
    let charter = root.join(".kittify/charter");
    std::fs::create_dir_all(&charter).expect("charter");
    std::fs::write(charter.join("charter.md"), "# charter\n").expect("charter.md");
    std::fs::write(charter.join("charter.yaml"), "metadata: {}\n").expect("charter.yaml");
    std::fs::write(charter.join("references.yaml"), "{}\n").expect("unexpected file");

    let migration = native("3.2.0rc35_unified_bundle");
    assert!((migration.detect)(root).expect("detect"));
    let outcome = (migration.apply)(root).expect("apply");
    let report: serde_json::Value =
        serde_json::from_str(&outcome.changes[0]).expect("report is JSON");
    assert_eq!(report["migration_id"], "3.2.0rc35_unified_bundle");
    assert_eq!(report["charter_present"], true);
    assert_eq!(report["bundle_validation"]["passed"], true);
    assert_eq!(report["applied"], false);
    assert_eq!(report["chokepoint_refreshed"], false);
    let unexpected = report["bundle_validation"]["unexpected"]
        .as_array()
        .expect("unexpected list");
    assert!(
        unexpected
            .iter()
            .any(|value| value == ".kittify/charter/references.yaml"),
        "undeclared bundle members are reported, not deleted"
    );
}
