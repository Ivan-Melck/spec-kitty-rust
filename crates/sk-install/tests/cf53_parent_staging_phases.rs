//! Actual catalog readonly staging must encode the entire key before building files.
use sk_install::skills::pack_skill::resolve_project_skill_catalog;
use std::{fs, path::Path};

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn readonly_hash_encoding_failure_precedes_all_build_file_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    write(
        root,
        ".kittify/config.yaml",
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(
        root,
        ".kittify/charter/activation.yaml",
        "activated_skills: [a-good, z-bad]\n",
    );
    write(
        root,
        "packs/acme/org-charter.yaml",
        "schema_version: 1\norg_name: acme\nskill_namespace: acme\n",
    );
    for id in ["a-good", "z-bad"] {
        let extra = if id == "z-bad" {
            "parameters:\n  - name: scope\n    default: \"14d\\ud800\"\n"
        } else {
            ""
        };
        write(
            root,
            &format!("packs/acme/skills/{id}.skill.yaml"),
            &format!(
                "schema_version: '1.0'\nid: {id}\ntitle: Title\ndescription: Description\nform: prompt\nbody_path: {id}.md\n{extra}"
            ),
        );
        write(
            root,
            &format!("packs/acme/skills/{id}.md"),
            "Do the thing.\n",
        );
    }
    let error = resolve_project_skill_catalog(root, false)
        .expect_err("the readonly hash encodes every render first");
    assert!(
        error.to_string().contains("surrogates not allowed"),
        "{error}"
    );
    let process_root =
        std::env::temp_dir().join(format!("spec-kitty-pack-skills-{}", std::process::id()));
    assert!(
        process_root.is_dir(),
        "Source creates the per-process parent before hashing"
    );
    assert_eq!(
        fs::read_dir(process_root).unwrap().count(),
        0,
        "hash failure must precede all staging child creation"
    );
}
