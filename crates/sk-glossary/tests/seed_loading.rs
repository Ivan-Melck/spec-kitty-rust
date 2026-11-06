use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use sk_glossary::GlossaryError;
use sk_glossary::{
    Glossary, GlossaryScope, SeedFileError, TermStatus, load_seed_file, parse_seed, seed_path,
};
use tempfile::TempDir;

fn fixture_repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo")
}

fn seed_error(result: Result<Vec<sk_glossary::TermSense>, SeedFileError>) -> SeedFileError {
    result.expect_err("seed file should be rejected")
}

#[test]
fn parses_a_realistic_seed_file_with_metadata() {
    let senses = load_seed_file(GlossaryScope::TeamDomain, fixture_repo()).expect("load seed");
    assert_eq!(senses.len(), 4);

    let workspace = &senses[0];
    assert_eq!(workspace.surface().as_str(), "workspace");
    assert_eq!(workspace.scope(), GlossaryScope::TeamDomain);
    assert_eq!(
        workspace.definition(),
        "Team-domain VS Code workspace configuration file"
    );
    assert_eq!(workspace.status(), TermStatus::Active);
    assert!((workspace.confidence() - 0.9).abs() < f64::EPSILON);

    let mission = &senses[1];
    assert!(
        mission
            .definition()
            .starts_with("A unit of governed delivery")
    );
    assert_eq!(mission.synonyms_to_avoid(), ["Feature"]);

    let characterization = &senses[3];
    assert_eq!(characterization.see_also().len(), 2);
    assert_eq!(
        characterization.see_also()[0].get("fr").map(String::as_str),
        Some("FR-008")
    );
    assert_eq!(
        characterization.see_also()[1]
            .get("tactic")
            .map(String::as_str),
        Some("secure-regex-catastrophic-backtracking")
    );
}

#[test]
fn loads_all_scopes_and_preserves_precedence_and_file_order() {
    let glossary = Glossary::load(fixture_repo()).expect("load glossary");
    assert_eq!(glossary.len(), 11);

    let scopes: Vec<_> = glossary.iter().map(|sense| sense.scope()).collect();
    assert_eq!(
        scopes,
        [
            GlossaryScope::MissionLocal,
            GlossaryScope::MissionLocal,
            GlossaryScope::TeamDomain,
            GlossaryScope::TeamDomain,
            GlossaryScope::TeamDomain,
            GlossaryScope::TeamDomain,
            GlossaryScope::AudienceDomain,
            GlossaryScope::AudienceDomain,
            GlossaryScope::SpecKittyCore,
            GlossaryScope::SpecKittyCore,
            GlossaryScope::SpecKittyCore,
        ]
    );

    let mission_local_surfaces: Vec<_> = glossary
        .iter()
        .filter(|sense| sense.scope() == GlossaryScope::MissionLocal)
        .map(|sense| sense.surface().as_str())
        .collect();
    assert_eq!(mission_local_surfaces, ["workspace", "lane"]);
}

#[test]
fn missing_seed_file_is_an_empty_scope() {
    let temp = TempDir::new().expect("temp dir");
    assert!(
        load_seed_file(GlossaryScope::TeamDomain, temp.path())
            .expect("missing seed")
            .is_empty()
    );

    let glossary = Glossary::load(temp.path()).expect("load empty glossary");
    assert!(glossary.is_empty());

    assert!(
        load_seed_file(GlossaryScope::TeamDomain, temp.path().join("missing-repo"))
            .expect("missing repository root")
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn rejects_a_seed_symlink_that_escapes_the_repository_root() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let repo = temp.path().join("repo");
    let glossaries = repo.join(".kittify/glossaries");
    std::fs::create_dir_all(&glossaries).expect("create glossaries");

    let outside = temp.path().join("outside.yaml");
    std::fs::write(
        &outside,
        "terms:\n  - surface: alpha\n    definition: Outside\n",
    )
    .expect("write outside seed");
    symlink(&outside, glossaries.join("team_domain.yaml")).expect("create escaping symlink");

    let error = load_seed_file(GlossaryScope::TeamDomain, &repo).expect_err("path escape");
    assert!(matches!(error, GlossaryError::PathEscape { .. }));
}

#[cfg(unix)]
#[test]
fn allows_a_seed_symlink_that_stays_inside_the_repository_root() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let repo = temp.path().join("repo");
    let glossaries = repo.join(".kittify/glossaries");
    std::fs::create_dir_all(&glossaries).expect("create glossaries");

    let target = glossaries.join("shared.yaml");
    std::fs::write(
        &target,
        "terms:\n  - surface: alpha\n    definition: Inside\n",
    )
    .expect("write shared seed");
    symlink(&target, glossaries.join("team_domain.yaml")).expect("create internal symlink");

    let senses = load_seed_file(GlossaryScope::TeamDomain, &repo).expect("internal symlink");
    assert_eq!(senses.len(), 1);
    assert_eq!(senses[0].definition(), "Inside");
}

#[test]
fn a_bare_terms_key_is_an_empty_glossary() {
    let senses = parse_seed(GlossaryScope::TeamDomain, "team_domain.yaml", "terms:\n")
        .expect("bare terms key");
    assert!(senses.is_empty());

    let senses = parse_seed(GlossaryScope::TeamDomain, "team_domain.yaml", "terms: []\n")
        .expect("empty terms list");
    assert!(senses.is_empty());
}

#[test]
fn reports_malformed_yaml_as_a_file_level_error() {
    let error = seed_error(parse_seed(
        GlossaryScope::SpecKittyCore,
        "spec_kitty_core.yaml",
        "terms:\n  - surface: alpha\n    definition: ok\n    confidence: [unterminated\n",
    ));

    assert_eq!(error.errors().len(), 1);
    let validation = &error.errors()[0];
    assert_eq!(validation.term_index, None);
    assert_eq!(validation.field, None);
    assert!(validation.message.contains("YAML parse error"));
    assert_eq!(validation.file_path, Path::new("spec_kitty_core.yaml"));
}

#[test]
fn rejects_missing_terms_and_non_mapping_roots() {
    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "not_terms: []\n",
    ));
    assert!(
        error
            .errors()
            .iter()
            .any(|entry| entry.field.as_deref() == Some("terms"))
    );

    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "- surface: alpha\n  definition: ok\n",
    ));
    assert_eq!(error.errors().len(), 1);
    assert!(error.errors()[0].message.contains("root must be a mapping"));
}

#[test]
fn rejects_unknown_fields_at_root_and_term_level() {
    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms: []\nbogus: true\n",
    ));
    assert_eq!(error.errors().len(), 1);
    assert_eq!(error.errors()[0].field.as_deref(), Some("bogus"));
    assert_eq!(error.errors()[0].term_index, None);

    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: alpha\n    definition: ok\n    bogus: true\n",
    ));
    assert_eq!(error.errors().len(), 1);
    assert_eq!(error.errors()[0].field.as_deref(), Some("bogus"));
    assert_eq!(error.errors()[0].term_index, Some(0));
    assert_eq!(error.errors()[0].term_surface.as_deref(), Some("alpha"));
}

#[test]
fn rejects_invalid_surface_and_definition_with_location_context() {
    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: WorkSpace\n    definition: ok\n",
    ));
    assert!(error.errors().iter().any(|entry| {
        entry.term_index == Some(0)
            && entry.term_surface.as_deref() == Some("WorkSpace")
            && entry.field.as_deref() == Some("surface")
            && entry.message.contains("normalized")
    }));

    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: \"\"\n    definition: ok\n",
    ));
    assert!(
        error.errors().iter().any(|entry| {
            entry.term_index == Some(0) && entry.field.as_deref() == Some("surface")
        })
    );

    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: alpha\n    definition: \"   \"\n",
    ));
    assert!(error.errors().iter().any(|entry| {
        entry.term_index == Some(0)
            && entry.field.as_deref() == Some("definition")
            && entry.message.contains("must not be empty")
    }));
}

#[test]
fn rejects_invalid_confidence_and_status_values() {
    for (confidence, expected_fragment) in [
        ("2.0", "0.0..1.0"),
        ("-0.1", "0.0..1.0"),
        ("\"0.5\"", "must be a number"),
        ("true", "must be a number"),
        ("null", "must be a number"),
    ] {
        let contents = format!(
            "terms:\n  - surface: alpha\n    definition: ok\n    confidence: {confidence}\n"
        );
        let error = seed_error(parse_seed(
            GlossaryScope::TeamDomain,
            "team_domain.yaml",
            &contents,
        ));
        assert!(
            error.errors().iter().any(|entry| {
                entry.term_index == Some(0)
                    && entry.field.as_deref() == Some("confidence")
                    && entry.message.contains(expected_fragment)
            }),
            "confidence {confidence:?} produced {:#?}",
            error.errors()
        );
    }

    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: alpha\n    definition: ok\n    status: archived\n",
    ));
    assert!(
        error.errors().iter().any(|entry| {
            entry.term_index == Some(0) && entry.field.as_deref() == Some("status")
        })
    );
}

#[test]
fn rejects_malformed_metadata_fields() {
    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: alpha\n    definition: ok\n    see_also: [42]\n",
    ));
    assert!(error.errors().iter().any(|entry| {
        entry.term_index == Some(0) && entry.field.as_deref() == Some("see_also")
    }));

    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: alpha\n    definition: ok\n    synonyms_to_avoid: [42]\n",
    ));
    assert!(error.errors().iter().any(|entry| {
        entry.term_index == Some(0) && entry.field.as_deref() == Some("synonyms_to_avoid")
    }));

    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: alpha\n    definition: ok\n    introduced_in_mission: 42\n",
    ));
    assert!(error.errors().iter().any(|entry| {
        entry.term_index == Some(0) && entry.field.as_deref() == Some("introduced_in_mission")
    }));
}

#[test]
fn accepts_null_optional_metadata_and_integer_confidence() {
    let senses = parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: alpha\n    definition: ok\n    confidence: 1\n    see_also: null\n    synonyms_to_avoid: null\n    introduced_in_mission: null\n",
    )
    .expect("optional nulls");
    assert_eq!(senses.len(), 1);
    assert!((senses[0].confidence() - 1.0).abs() < f64::EPSILON);
    assert!(senses[0].see_also().is_empty());
    assert!(senses[0].synonyms_to_avoid().is_empty());
    assert_eq!(senses[0].introduced_in_mission(), None);
}

#[test]
fn collects_multiple_validation_errors_with_term_indices() {
    let error = seed_error(parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: Alpha\n    definition: ok\n  - surface: beta\n    definition: \" \"\n",
    ));
    assert!(error.errors().len() >= 2);
    let indices: Vec<_> = error
        .errors()
        .iter()
        .filter_map(|entry| entry.term_index)
        .collect();
    assert!(indices.contains(&0));
    assert!(indices.contains(&1));
    let rendered = error.to_string();
    assert!(rendered.contains("validation error(s)"));
    assert!(rendered.contains("term[0] 'Alpha'"));
}

#[test]
fn file_path_is_propagated_to_every_error() {
    let path = Path::new("/custom/glossaries/team_domain.yaml");
    let error = parse_seed(
        GlossaryScope::TeamDomain,
        path,
        "terms:\n  - surface: Alpha\n    definition: ok\n",
    )
    .expect_err("invalid seed");
    assert_eq!(error.file_path(), path);
    assert!(error.errors().iter().all(|entry| entry.file_path == path));
}

#[test]
fn seed_path_is_the_upstream_location() {
    assert_eq!(
        seed_path(GlossaryScope::SpecKittyCore, Path::new("/repo")),
        Path::new("/repo/.kittify/glossaries/spec_kitty_core.yaml")
    );
}

#[test]
fn see_also_maps_are_deterministically_ordered() {
    let senses = parse_seed(
        GlossaryScope::TeamDomain,
        "team_domain.yaml",
        "terms:\n  - surface: alpha\n    definition: ok\n    see_also:\n      - zeta: last\n        alpha: first\n",
    )
    .expect("valid metadata");
    let mut expected = BTreeMap::new();
    expected.insert("alpha".to_owned(), "first".to_owned());
    expected.insert("zeta".to_owned(), "last".to_owned());
    assert_eq!(senses[0].see_also(), [expected]);
}
