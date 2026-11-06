//! Acceptance tests for bounded, read-only `.kittify/config.yaml` inspection.
//!
//! The canonical shape is `charter_packs.org.packs[]` with `name` and
//! `local_path`; `local_path`/`subdir` remain untrusted strings until
//! `OrgPackConfig::effective_root` resolves them through the containment
//! primitive. Nothing is ever reserialized.

mod support;

use std::path::Path;

use sk_charter::{CharterError, Limits, PathExpansion, load_project_config};
use support::write;

fn config_path(project: &Path) -> std::path::PathBuf {
    project.join(".kittify/config.yaml")
}

#[test]
fn absent_config_is_absence_not_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let config = load_project_config(&project, &Limits::default()).unwrap();
    assert!(!config.present);
    assert!(config.org_packs.is_empty());
    assert_eq!(config.charter_pointer, None);
    assert_eq!(config.project_skill_namespace, None);
    assert_eq!(config.path, config_path(&project));
}

#[test]
fn reads_the_canonical_org_packs_registry_in_declaration_order() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &config_path(&project),
        r#"
charter: .kittify/charter/charter.yaml
charter_packs:
  org:
    packs:
      - name: team
        pack_id: 01ARWG13C000000000000000F1
        local_path: ${PACKS}/team
        subdir: pack-core
        source_type: git
        url: https://example.invalid/team.git
        ref: v1
      - name: ops
        local_path: packs/ops
  project:
    skill_namespace: acme
unrelated_key:
  tolerated: true
"#,
    );

    let config = load_project_config(&project, &Limits::default()).unwrap();
    assert!(config.present);
    assert_eq!(
        config.charter_pointer.as_deref(),
        Some(".kittify/charter/charter.yaml")
    );
    assert_eq!(config.project_skill_namespace.as_deref(), Some("acme"));
    assert_eq!(config.org_packs.len(), 2);

    let team = &config.org_packs[0];
    assert_eq!(team.name, "team");
    assert_eq!(team.pack_id.as_deref(), Some("01ARWG13C000000000000000F1"));
    assert_eq!(team.local_path, Path::new("${PACKS}/team"));
    assert_eq!(team.subdir.as_deref(), Some("pack-core"));
    assert_eq!(team.source_type.as_deref(), Some("git"));
    assert_eq!(
        team.url.as_deref(),
        Some("https://example.invalid/team.git")
    );
    assert_eq!(team.r#ref.as_deref(), Some("v1"));
    assert_eq!(config.org_packs[1].name, "ops");
}

#[test]
fn local_paths_expand_env_vars_and_home_but_stay_raw_on_the_model() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &config_path(&project),
        "charter_packs:\n  org:\n    packs:\n      - name: team\n        local_path: ${PACKS}/team\n",
    );
    let config = load_project_config(&project, &Limits::default()).unwrap();
    let expansion = PathExpansion::empty()
        .with_var("PACKS", temp.path().join("packs").display().to_string())
        .with_home(temp.path().join("home"));

    let root = config.org_packs[0]
        .effective_root(&project, &expansion)
        .unwrap();
    assert_eq!(root, temp.path().join("packs/team"));
    // The authored value is never rewritten in place.
    assert_eq!(config.org_packs[0].local_path, Path::new("${PACKS}/team"));
}

#[test]
fn unresolved_env_tokens_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &config_path(&project),
        "charter_packs:\n  org:\n    packs:\n      - name: team\n        local_path: ${MISSING}/team\n",
    );
    let config = load_project_config(&project, &Limits::default()).unwrap();
    let error = config.org_packs[0]
        .effective_root(&project, &PathExpansion::empty())
        .unwrap_err();
    match error {
        CharterError::UnresolvedPathToken { token, .. } => assert_eq!(token, "${MISSING}"),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn an_empty_env_value_is_not_silently_a_literal_path() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &config_path(&project),
        "charter_packs:\n  org:\n    packs:\n      - name: team\n        local_path: $EMPTY/team\n",
    );
    let config = load_project_config(&project, &Limits::default()).unwrap();
    let expansion = PathExpansion::empty().with_var("EMPTY", "");
    let error = config.org_packs[0]
        .effective_root(&project, &expansion)
        .unwrap_err();
    assert!(matches!(error, CharterError::UnresolvedPathToken { .. }));
}

#[test]
fn invalid_yaml_and_non_mapping_documents_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(&config_path(&project), "charter_packs: [unterminated\n");
    let error = load_project_config(&project, &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::InvalidYaml { .. }));

    let project = temp.path().join("project2");
    write(&config_path(&project), "- a\n- b\n");
    let error = load_project_config(&project, &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::ExpectedMapping { .. }));
}

#[test]
fn duplicate_pack_names_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &config_path(&project),
        "charter_packs:\n  org:\n    packs:\n      - name: same\n        local_path: a\n      - name: same\n        local_path: b\n",
    );
    let error = load_project_config(&project, &Limits::default()).unwrap_err();
    match error {
        CharterError::InvalidConfig { reason, .. } => assert!(reason.contains("duplicate")),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn missing_or_malformed_pack_fields_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for (label, body) in [
        (
            "missing local_path",
            "charter_packs:\n  org:\n    packs:\n      - name: team\n",
        ),
        (
            "blank name",
            "charter_packs:\n  org:\n    packs:\n      - name: ' '\n        local_path: x\n",
        ),
        (
            "unknown field",
            "charter_packs:\n  org:\n    packs:\n      - name: team\n        local_path: x\n        surprise: true\n",
        ),
        (
            "absolute subdir",
            "charter_packs:\n  org:\n    packs:\n      - name: team\n        local_path: x\n        subdir: /etc\n",
        ),
        (
            "traversal subdir",
            "charter_packs:\n  org:\n    packs:\n      - name: team\n        local_path: x\n        subdir: ../escape\n",
        ),
        (
            "unnamed single pack block",
            "charter_packs:\n  org:\n    local_path: x\n",
        ),
    ] {
        let project = temp.path().join(label.replace(' ', "-"));
        write(&config_path(&project), body);
        let error = match load_project_config(&project, &Limits::default()) {
            Ok(_) => panic!("{label}: expected an error"),
            Err(error) => error,
        };
        assert!(
            matches!(error, CharterError::InvalidConfig { .. }),
            "{label}: {error}"
        );
    }
}

#[test]
fn oversized_config_is_refused_without_being_parsed() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let padding = "x".repeat(4096);
    write(&config_path(&project), &format!("charter: {padding}\n"));
    let limits = Limits::default().with_max_config_bytes(1024);
    let error = load_project_config(&project, &limits).unwrap_err();
    match error {
        CharterError::TooLarge { path, limit } => {
            assert_eq!(path, config_path(&project));
            assert_eq!(limit, 1024);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn empty_config_file_is_treated_as_an_empty_document() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(&config_path(&project), "\n   \n");
    let config = load_project_config(&project, &Limits::default()).unwrap();
    assert!(config.present);
    assert!(config.org_packs.is_empty());
}

#[test]
fn an_invalid_pack_id_or_source_type_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for (label, body) in [
        (
            "bad id",
            "charter_packs:\n  org:\n    packs:\n      - name: team\n        pack_id: not-a-ulid\n        local_path: x\n",
        ),
        (
            "bad source type",
            "charter_packs:\n  org:\n    packs:\n      - name: team\n        local_path: x\n        source_type: ftp\n",
        ),
        (
            "blank namespace",
            "charter_packs:\n  project:\n    skill_namespace: ' '\n",
        ),
    ] {
        let project = temp.path().join(label.replace(' ', "-"));
        write(&config_path(&project), body);
        let error = match load_project_config(&project, &Limits::default()) {
            Ok(_) => panic!("{label}: expected an error"),
            Err(error) => error,
        };
        assert!(
            matches!(error, CharterError::InvalidConfig { .. }),
            "{label}: {error}"
        );
    }
}
