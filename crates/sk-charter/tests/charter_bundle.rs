//! Acceptance tests for bounded inspection of the `.kittify/charter/` bundle.
//!
//! Only presence, size and digest are exposed plus a YAML well-formedness check
//! for the machine-readable companions. The curated `charter.md` is never
//! parsed and user files are never rewritten.

mod support;

use std::path::Path;

use sk_charter::{CharterBundle, CharterError, Limits};
use support::{mkdir, write};

#[test]
fn absent_charter_directory_is_absence_not_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    mkdir(&project);
    let bundle = CharterBundle::load(&project, &Limits::default()).unwrap();
    assert!(!bundle.is_present());
    assert!(bundle.charter_md.is_none());
    assert!(bundle.charter_yaml.is_none());
    assert!(bundle.activation_ledger.is_none());
    assert!(bundle.synthesis_manifest.is_none());
}

#[test]
fn inspects_the_known_bundle_files_with_sizes_and_sha256_digests() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(&project.join(".kittify/charter/charter.md"), "hello\n");
    write(
        &project.join(".kittify/charter/charter.yaml"),
        "schema_version: '1'\nmission: demo\n",
    );
    write(
        &project.join(".kittify/charter/activation.yaml"),
        "activations: []\n",
    );
    write(
        &project.join(".kittify/charter/synthesis-manifest.yaml"),
        "synthesizer_version: '1'\n",
    );

    let bundle = CharterBundle::load(&project, &Limits::default()).unwrap();
    assert!(bundle.is_present());

    let markdown = bundle.charter_md.as_ref().unwrap();
    assert_eq!(markdown.relative_path, ".kittify/charter/charter.md");
    assert_eq!(markdown.size, 6);
    assert_eq!(
        markdown.sha256,
        "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
    );

    let yaml = bundle.charter_yaml.as_ref().unwrap();
    assert_eq!(yaml.relative_path, ".kittify/charter/charter.yaml");
    assert_eq!(
        bundle.activation_ledger.as_ref().unwrap().relative_path,
        ".kittify/charter/activation.yaml"
    );
    assert_eq!(
        bundle.synthesis_manifest.as_ref().unwrap().relative_path,
        ".kittify/charter/synthesis-manifest.yaml"
    );
    assert_eq!(bundle.files().count(), 4);
}

#[test]
fn corrupt_machine_readable_charter_yaml_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(&project.join(".kittify/charter/charter.md"), "# Charter\n");
    write(
        &project.join(".kittify/charter/charter.yaml"),
        "mission: [unterminated\n",
    );
    let error = CharterBundle::load(&project, &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::InvalidYaml { .. }));
}

#[test]
fn a_non_mapping_charter_companion_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &project.join(".kittify/charter/charter.yaml"),
        "- one\n- two\n",
    );
    let error = CharterBundle::load(&project, &Limits::default()).unwrap_err();
    assert!(matches!(error, CharterError::ExpectedMapping { .. }));
}

#[test]
fn oversized_bundle_files_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &project.join(".kittify/charter/charter.md"),
        &"x".repeat(2048),
    );
    let limits = Limits::default().with_max_charter_file_bytes(1024);
    let error = CharterBundle::load(&project, &limits).unwrap_err();
    match error {
        CharterError::TooLarge { path, limit } => {
            assert!(path.ends_with(".kittify/charter/charter.md"));
            assert_eq!(limit, 1024);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_charter_file_that_escapes_the_bundle_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let outside = temp.path().join("outside.md");
    write(&outside, "# outside\n");
    mkdir(&project.join(".kittify/charter"));
    support::symlink_file(&outside, &project.join(".kittify/charter/charter.md"));

    let error = CharterBundle::load(&project, &Limits::default()).unwrap_err();
    match error {
        CharterError::PathEscape { .. } => {}
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn a_symlinked_charter_directory_outside_the_project_is_refused() {
    #[cfg(unix)]
    {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let outside = temp.path().join("outside-charter");
        write(&outside.join("charter.md"), "# outside\n");
        mkdir(&project.join(".kittify"));
        support::symlink_dir(&outside, &project.join(".kittify/charter"));

        let error = CharterBundle::load(&project, &Limits::default()).unwrap_err();
        assert!(matches!(error, CharterError::PathEscape { .. }));
    }
}

#[test]
fn unknown_companion_keys_are_tolerated_because_only_well_formedness_is_checked() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(
        &project.join(".kittify/charter/charter.yaml"),
        "future_field:\n  nested: true\n",
    );
    let bundle = CharterBundle::load(&project, &Limits::default()).unwrap();
    assert!(bundle.charter_yaml.is_some());
}

#[test]
fn path_and_relative_path_are_absolute_and_project_relative() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    write(&project.join(".kittify/charter/charter.md"), "# Charter\n");
    let bundle = CharterBundle::load(&project, &Limits::default()).unwrap();
    let file = bundle.charter_md.as_ref().unwrap();
    assert!(file.path.is_absolute());
    assert_eq!(file.path, project.join(".kittify/charter/charter.md"));
    assert_eq!(file.relative_path, ".kittify/charter/charter.md");
    let _: &Path = file.path.as_path();
}
