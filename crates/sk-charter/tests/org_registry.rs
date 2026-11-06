//! Org-only reads reuse canonical pack validation without judging unrelated fields.

use sk_charter::{Limits, load_org_pack_registry, load_project_config};

fn write_config(root: &std::path::Path, source: &str) {
    std::fs::create_dir_all(root.join(".kittify")).unwrap();
    std::fs::write(root.join(".kittify/config.yaml"), source).unwrap();
}

#[test]
fn registry_read_does_not_weaken_whole_project_validation() {
    let temp = tempfile::tempdir().unwrap();
    for unrelated in [
        "charter: 123\ncharter_packs:\n",
        "charter_packs:\n  project:\n    skill_namespace: '  '\n",
    ] {
        write_config(
            temp.path(),
            &format!(
                "{unrelated}  org:\n    packs:\n      - name: sample\n        local_path: packs/sample\n"
            ),
        );
        assert!(load_project_config(temp.path(), &Limits::default()).is_err());
        let packs = load_org_pack_registry(temp.path(), &Limits::default()).unwrap();
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, "sample");
        assert_eq!(packs[0].local_path, std::path::Path::new("packs/sample"));
    }
}

#[test]
fn pack_declaration_errors_are_shared_by_both_readers() {
    let temp = tempfile::tempdir().unwrap();
    for entry in [
        "- name: sample",
        "- name: sample\n        local_path: p\n        unexpected: true",
        "- name: sample\n        local_path: p\n        subdir: ../outside",
    ] {
        write_config(
            temp.path(),
            &format!("charter_packs:\n  org:\n    packs:\n      {entry}\n"),
        );
        let whole = load_project_config(temp.path(), &Limits::default()).unwrap_err();
        let registry = load_org_pack_registry(temp.path(), &Limits::default()).unwrap_err();
        assert_eq!(whole.to_string(), registry.to_string());
    }
}

#[test]
fn absent_registry_and_bom_documents_preserve_presence_semantics() {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        load_org_pack_registry(temp.path(), &Limits::default())
            .unwrap()
            .is_empty()
    );
    assert!(
        !load_project_config(temp.path(), &Limits::default())
            .unwrap()
            .present
    );
    write_config(
        temp.path(),
        "\u{feff}charter: local.yaml\ncharter_packs: {}\n",
    );
    assert!(
        load_org_pack_registry(temp.path(), &Limits::default())
            .unwrap()
            .is_empty()
    );
    let whole = load_project_config(temp.path(), &Limits::default()).unwrap();
    assert!(whole.present);
    assert_eq!(whole.charter_pointer.as_deref(), Some("local.yaml"));
}
