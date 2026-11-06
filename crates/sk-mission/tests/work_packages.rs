mod support;

use std::fs;
use std::path::Path;

use sk_mission::{FrontmatterError, MissionError, WorkPackage, split_frontmatter};
use support::wp_source;

#[test]
fn parses_a_realistic_fixture_work_package() {
    let path = support::fixtures_project()
        .join("kitty-specs/002-lightweight-pypi-release/tasks/WP02-pypi-release-automation.md");
    let package = WorkPackage::from_file(&path).expect("parse fixture WP02");

    assert_eq!(package.id().as_str(), "WP02");
    assert_eq!(package.title(), Some("PyPI Release Automation"));
    assert_eq!(
        package
            .dependencies()
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["WP01"]
    );
    assert_eq!(package.source(), path);
    assert_eq!(
        package
            .frontmatter_field("mission_id")
            .and_then(|value| value.as_str()),
        Some("01KN2371WNZX28ME4MN97B67PJ")
    );
}

#[test]
fn parses_multiple_dependencies_in_declaration_order() {
    let package = WorkPackage::parse(
        Path::new("WP03-release-readiness-guardrails.md"),
        &wp_source("WP03", "\n- WP01\n- WP02"),
    )
    .expect("parse WP03");

    assert_eq!(
        package
            .dependencies()
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["WP01", "WP02"]
    );
    assert_eq!(package.title(), Some("Fixture WP03"));
}

#[test]
fn legacy_quoted_empty_dependency_list_is_accepted() {
    // Three real work-package prompts in the repository record `dependencies:
    // "[]"` (a quoted YAML string). Accept exactly that spelling; nothing else.
    for dependencies_yaml in ["\"[]\"", "'[]'", "\"  []  \""] {
        let package = WorkPackage::parse(
            Path::new("WP01-legacy.md"),
            &wp_source("WP01", dependencies_yaml),
        )
        .unwrap_or_else(|error| panic!("{dependencies_yaml}: {error}"));
        assert!(package.dependencies().is_empty());
    }
}

#[test]
fn missing_frontmatter_is_refused() {
    let error = WorkPackage::parse(Path::new("WP01-no-frontmatter.md"), "# Just a prompt\n")
        .expect_err("no frontmatter");
    assert!(
        matches!(error, MissionError::MissingFrontmatter { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn unterminated_frontmatter_is_refused() {
    let error = WorkPackage::parse(
        Path::new("WP01-unterminated.md"),
        "---\nwork_package_id: WP01\nbody without a close\n",
    )
    .expect_err("unterminated frontmatter");
    assert!(
        matches!(error, MissionError::UnterminatedFrontmatter { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn invalid_yaml_is_refused() {
    let error = WorkPackage::parse(
        Path::new("WP01-bad-yaml.md"),
        "---\nwork_package_id: [unclosed\n---\nbody\n",
    )
    .expect_err("invalid YAML");
    assert!(
        matches!(error, MissionError::InvalidFrontmatter { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn non_mapping_frontmatter_is_refused() {
    let error = WorkPackage::parse(Path::new("WP01-sequence.md"), "---\n- WP01\n---\nbody\n")
        .expect_err("sequence frontmatter");
    assert!(
        matches!(error, MissionError::InvalidFrontmatter { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn missing_work_package_id_is_refused() {
    let error = WorkPackage::parse(
        Path::new("WP01-no-id.md"),
        "---\ntitle: No id\ndependencies: []\n---\nbody\n",
    )
    .expect_err("no work_package_id");
    assert!(
        matches!(error, MissionError::MissingWorkPackageId { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn invalid_work_package_id_is_refused() {
    let error = WorkPackage::parse(
        Path::new("WPX-invalid.md"),
        "---\nwork_package_id: WPX\ndependencies: []\n---\nbody\n",
    )
    .expect_err("invalid work_package_id");
    assert!(
        matches!(error, MissionError::InvalidWorkPackageId { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn filename_and_frontmatter_id_mismatch_is_refused() {
    let error = WorkPackage::parse(Path::new("WP02-mislabelled.md"), &wp_source("WP01", "[]"))
        .expect_err("id mismatch");
    match error {
        MissionError::WorkPackageIdMismatch {
            filename,
            frontmatter,
            ..
        } => {
            assert_eq!(filename, "WP02");
            assert_eq!(frontmatter, "WP01");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn invalid_dependency_id_is_refused() {
    let error = WorkPackage::parse(Path::new("WP02-bad-dep.md"), &wp_source("WP02", "\n- WP--"))
        .expect_err("invalid dependency id");
    assert!(
        matches!(error, MissionError::InvalidDependencies { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn duplicate_dependencies_are_refused() {
    let error = WorkPackage::parse(
        Path::new("WP02-duplicate-dep.md"),
        &wp_source("WP02", "\n- WP01\n- WP01"),
    )
    .expect_err("duplicate dependency");
    assert!(
        matches!(error, MissionError::InvalidDependencies { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn string_dependencies_other_than_empty_list_are_refused() {
    let error = WorkPackage::parse(Path::new("WP02-string-dep.md"), &wp_source("WP02", "WP01"))
        .expect_err("string dependency list");
    assert!(
        matches!(error, MissionError::InvalidDependencies { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn parsing_does_not_rewrite_the_source_file() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("WP01-untouched.md");
    let source = wp_source("WP01", "[]");
    fs::write(&path, &source).expect("write prompt");
    let before = fs::read(&path).expect("read before");

    let package = WorkPackage::from_file(&path).expect("parse");
    assert_eq!(package.id().as_str(), "WP01");

    let after = fs::read(&path).expect("read after");
    assert_eq!(
        before, after,
        "parsing must not rewrite the Markdown source"
    );
}

#[test]
fn crlf_and_bom_are_tolerated_without_rewriting() {
    let path = Path::new("WP01-crlf.md");
    let source = "\u{feff}---\r\nwork_package_id: WP01\r\ndependencies: []\r\n---\r\nBody\r\n";
    let package = WorkPackage::parse(path, source).expect("CRLF + BOM");
    assert_eq!(package.id().as_str(), "WP01");

    let frontmatter = split_frontmatter(source).expect("split");
    assert_eq!(frontmatter.body, "Body\r\n");
}

#[test]
fn byte_budget_is_enforced_for_work_packages() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let path = temp.path().join("WP01-large.md");
    fs::write(&path, wp_source("WP01", "[]")).expect("write prompt");

    let error = WorkPackage::from_file_with_limit(&path, 10).expect_err("too large");
    assert!(
        matches!(error, MissionError::WorkPackageTooLarge { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn retired_lane_is_preserved_for_inspection_but_not_interpreted() {
    let path = support::fixtures_project().join(
        "kitty-specs/002-lightweight-pypi-release/tasks/WP03-release-readiness-guardrails.md",
    );
    let package = WorkPackage::from_file(&path).expect("parse fixture WP03");

    // Preserved verbatim (the Markdown is not rewritten)...
    assert_eq!(
        package
            .frontmatter_field("lane")
            .and_then(|value| value.as_str()),
        Some("done")
    );
    // ...but there is no state accessor and no inferred "done".
    assert_eq!(package.dependencies().len(), 2);
}

#[test]
fn split_frontmatter_reports_missing_and_unterminated_delimiters() {
    assert_eq!(
        split_frontmatter("# no frontmatter\n").unwrap_err(),
        FrontmatterError::MissingOpeningDelimiter
    );
    assert_eq!(
        split_frontmatter("---\nkey: value\n").unwrap_err(),
        FrontmatterError::Unterminated
    );
    assert_eq!(
        split_frontmatter("").unwrap_err(),
        FrontmatterError::MissingOpeningDelimiter
    );
}

#[test]
fn split_frontmatter_accepts_dotted_closing_delimiter() {
    let frontmatter = split_frontmatter("---\nkey: value\n...\nbody\n").expect("split");
    assert_eq!(frontmatter.yaml, "key: value\n");
    assert_eq!(frontmatter.body, "body\n");
}
