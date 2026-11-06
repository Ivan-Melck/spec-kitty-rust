use std::path::PathBuf;

use sk_runtime::{MAX_TEMPLATE_BYTES, RuntimeError, load_template};

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn write(path: &std::path::Path, text: &str) {
    std::fs::write(path, text).expect("write fixture");
}

#[test]
fn loads_and_validates_the_upstream_reference_parity_template() {
    let path = repo_path("tests/fixtures/runtime_parity/reference_mission/mission.yaml");
    let template = load_template(&path).expect("upstream reference mission should load");

    assert_eq!(template.mission.key, "ref-parity");
    assert_eq!(template.mission.name, "Reference Parity Mission");
    assert_eq!(template.mission.version, "1.0.0");
    assert_eq!(
        template
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        vec!["discovery", "specify", "done"]
    );
    assert_eq!(template.steps[0].requires_inputs, vec!["topic"]);
    assert_eq!(template.steps[1].depends_on, vec!["discovery"]);
    assert!(template.audit_steps.is_empty());
}

#[test]
fn accepts_legacy_top_level_shorthand_and_stringifies_numeric_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("legacy-mission").join("mission.yaml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write(
        &path,
        r#"
key: shorthand
name: Shorthand Mission
version: 2
description: Legacy shape.
steps:
  - id: only
    title: Only
    prompt: Do it.
"#,
    );

    let template = load_template(&path).expect("shorthand should load");
    assert_eq!(template.mission.key, "shorthand");
    assert_eq!(template.mission.version, "2");
    assert_eq!(template.steps.len(), 1);
}

#[test]
fn falls_back_to_directory_name_when_shorthand_has_no_key_or_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fallback-mission").join("mission.yaml");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    write(
        &path,
        r#"
version: "1.0.0"
steps:
  - id: only
    title: Only
"#,
    );

    let template = load_template(&path).expect("shorthand should load");
    assert_eq!(template.mission.key, "fallback-mission");
    assert_eq!(template.mission.name, "fallback-mission");
}

#[test]
fn empty_template_is_rejected_as_empty_not_as_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(&path, "");

    let error = load_template(&path).expect_err("empty template must fail");
    assert!(matches!(error, RuntimeError::EmptyTemplate), "{error:?}");
}

#[test]
fn top_level_non_mapping_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(&path, "- one\n- two\n");

    let error = load_template(&path).expect_err("sequence is not a template");
    assert!(matches!(error, RuntimeError::Malformed { .. }), "{error:?}");
}

#[test]
fn oversized_template_is_rejected_before_parsing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let oversized_path = dir.path().join("mission.yaml");
    let bytes = vec![b'#'; MAX_TEMPLATE_BYTES as usize + 1];
    std::fs::write(&oversized_path, bytes).unwrap();

    let error = load_template(&oversized_path).expect_err("oversized template must fail");
    assert!(
        matches!(error, RuntimeError::TooLarge { ref path, limit } if path == &oversized_path && limit == MAX_TEMPLATE_BYTES),
        "{error:?}"
    );
}

#[test]
fn non_utf8_template_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();

    let error = load_template(&path).expect_err("non-utf8 must fail");
    assert!(matches!(error, RuntimeError::NotUtf8 { .. }), "{error:?}");
}

fn template_with_steps(steps: &str) -> String {
    format!(
        r#"
mission:
  key: validation
  name: Validation
  version: "1.0.0"
steps:
{steps}
"#
    )
}

#[test]
fn duplicate_step_ids_are_rejected_across_steps_and_audits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        r#"
mission:
  key: validation
  name: Validation
  version: "1.0.0"
steps:
  - id: repeated
    title: First
audit_steps:
  - id: repeated
    title: Audit
    audit: {trigger_mode: manual, enforcement: advisory}
"#,
    );

    let error = load_template(&path).expect_err("duplicate ids must fail");
    assert!(
        matches!(error, RuntimeError::DuplicateStepId { ref id } if id == "repeated"),
        "{error:?}"
    );
}

#[test]
fn unknown_dependency_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        &template_with_steps(
            r#"  - id: second
    title: Second
    depends_on: [missing]
"#,
        ),
    );

    let error = load_template(&path).expect_err("unknown dependency must fail");
    assert!(
        matches!(error, RuntimeError::UnknownDependency { ref step_id, ref dependency }
            if step_id == "second" && dependency == "missing"),
        "{error:?}"
    );
}

#[test]
fn duplicate_dependency_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        &template_with_steps(
            r#"  - id: first
    title: First
  - id: second
    title: Second
    depends_on: [first, first]
"#,
        ),
    );

    let error = load_template(&path).expect_err("duplicate dependency must fail");
    assert!(
        matches!(error, RuntimeError::DuplicateDependency { ref step_id, ref dependency }
            if step_id == "second" && dependency == "first"),
        "{error:?}"
    );
}

#[test]
fn dependency_cycle_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        &template_with_steps(
            r#"  - id: first
    title: First
    depends_on: [second]
  - id: second
    title: Second
    depends_on: [first]
"#,
        ),
    );

    let error = load_template(&path).expect_err("cycle must fail");
    assert!(
        matches!(error, RuntimeError::DependencyCycle { .. }),
        "{error:?}"
    );
}

#[test]
fn self_dependency_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        &template_with_steps(
            r#"  - id: first
    title: First
    depends_on: [first]
"#,
        ),
    );

    let error = load_template(&path).expect_err("self dependency must fail");
    assert!(
        matches!(error, RuntimeError::DependencyCycle { .. }),
        "{error:?}"
    );
}

#[test]
fn raci_accountable_must_be_human() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        &template_with_steps(
            r#"  - id: gated
    title: Gated
    raci:
      responsible: {actor_type: llm}
      accountable: {actor_type: llm}
    raci_override_reason: "team decision"
"#,
        ),
    );

    let error = load_template(&path).expect_err("non-human accountable must fail");
    assert!(
        matches!(error, RuntimeError::RaciAccountableNotHuman { ref step_id, .. } if step_id == "gated"),
        "{error:?}"
    );
}

#[test]
fn raci_block_requires_override_reason_and_reason_requires_raci() {
    let dir = tempfile::tempdir().expect("tempdir");

    let missing_reason = dir.path().join("missing-reason.yaml");
    write(
        &missing_reason,
        &template_with_steps(
            r#"  - id: gated
    title: Gated
    raci:
      responsible: {actor_type: llm}
      accountable: {actor_type: human}
"#,
        ),
    );
    let error = load_template(&missing_reason).expect_err("reason is required");
    assert!(
        matches!(error, RuntimeError::RaciOverrideReason { .. }),
        "{error:?}"
    );

    let reason_without_raci = dir.path().join("reason-without-raci.yaml");
    write(
        &reason_without_raci,
        &template_with_steps(
            r#"  - id: gated
    title: Gated
    raci_override_reason: "orphan reason"
"#,
        ),
    );
    let error = load_template(&reason_without_raci).expect_err("raci is required");
    assert!(
        matches!(error, RuntimeError::RaciOverrideReason { .. }),
        "{error:?}"
    );
}

#[test]
fn valid_raci_with_human_accountable_is_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        &template_with_steps(
            r#"  - id: gated
    title: Gated
    raci:
      responsible: {actor_type: llm, actor_id: implementer}
      accountable: {actor_type: human, actor_id: owner}
      consulted: [{actor_type: human}]
      informed: []
    raci_override_reason: "owner signs off"
"#,
        ),
    );

    let template = load_template(&path).expect("valid raci should load");
    let raci = template.steps[0].raci.as_ref().unwrap();
    assert_eq!(raci.accountable.actor_id.as_deref(), Some("owner"));
}

#[test]
fn audit_significance_dimensions_are_strict() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        r#"
mission:
  key: validation
  name: Validation
  version: "1.0.0"
audit_steps:
  - id: gate
    title: Gate
    audit: {trigger_mode: manual, enforcement: blocking}
    significance:
      dimensions:
        user_customer_impact: 1
      hard_triggers: []
"#,
    );

    let error = load_template(&path).expect_err("partial dimensions must fail");
    assert!(
        matches!(error, RuntimeError::InvalidSignificance { ref step_id, .. } if step_id == "gate"),
        "{error:?}"
    );
}

#[test]
fn audit_config_rejects_unknown_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mission.yaml");
    write(
        &path,
        r#"
mission:
  key: validation
  name: Validation
  version: "1.0.0"
audit_steps:
  - id: gate
    title: Gate
    audit: {trigger_mode: manual, enforcement: advisory, typo_field: true}
"#,
    );

    let error = load_template(&path).expect_err("unknown audit field must fail");
    assert!(matches!(error, RuntimeError::Malformed { .. }), "{error:?}");
}

#[test]
fn missing_file_is_an_io_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let error = load_template(&dir.path().join("nope.yaml")).expect_err("missing file");
    assert!(matches!(error, RuntimeError::Io { .. }), "{error:?}");
}
