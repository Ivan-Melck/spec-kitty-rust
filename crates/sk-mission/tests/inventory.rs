mod support;

use std::fs;

use sk_mission::{
    DEFAULT_META_BYTE_LIMIT, MissionError, MissionId, MissionTopology, list_missions,
};
use support::{fixtures_project, meta_json, mission_dir, temp_project, write, write_mission};

#[test]
fn lists_fixture_missions_sorted_and_skips_residue() {
    let project = fixtures_project();
    let missions = list_missions(&project).expect("fixture inventory");

    assert_eq!(missions.len(), 2);
    assert_eq!(missions[0].slug().as_str(), "002-lightweight-pypi-release");
    assert_eq!(missions[1].slug().as_str(), "003-auto-protect-agent");
    assert!(
        !missions
            .iter()
            .any(|mission| mission.slug().as_str() == "planning-scratch"),
        "a directory without meta.json is residue, not a Mission"
    );
}

#[test]
fn preserves_every_meta_field_including_extensions() {
    let missions = list_missions(&fixtures_project()).expect("fixture inventory");
    let mission = &missions[0];

    assert_eq!(mission.id().as_str(), "01KN2371WNZX28ME4MN97B67PJ");
    assert_eq!(mission.mid8(), "01KN2371");
    assert_eq!(mission.mission_type(), Some("research"));
    assert_eq!(
        mission.friendly_name(),
        Some("Lightweight PyPI Release Workflow")
    );
    assert_eq!(mission.display_name(), "Lightweight PyPI Release Workflow");
    assert_eq!(mission.mission_number(), Some(2));
    assert_eq!(mission.mission_number_display().as_deref(), Some("2"));
    assert_eq!(mission.topology(), Some(MissionTopology::SingleBranch));
    assert!(!mission.flattened());

    // Known fields and extension fields both survive verbatim.
    assert_eq!(
        mission
            .meta_field("target_branch")
            .and_then(|value| value.as_str()),
        Some("main")
    );
    let extension = mission
        .meta_field("review_playbook")
        .expect("extension field preserved");
    assert_eq!(extension["revision"], 4);
    assert_eq!(extension["notes"], "extension field preserved verbatim");
    assert_eq!(
        mission.meta()["origin_ticket"]["number"],
        1234,
        "nested extension fields are preserved too"
    );

    // The raw meta slug is preserved, but the directory name is the canonical
    // slug (real fixtures contain legacy slugs without the numeric prefix).
    assert_eq!(
        mission.spec_dir(),
        project_mission_dir(&fixtures_project(), "002-lightweight-pypi-release")
    );
}

#[test]
fn null_mission_number_stays_absent() {
    let missions = list_missions(&fixtures_project()).expect("fixture inventory");
    let mission = &missions[1];

    assert_eq!(mission.id().as_str(), "01KN2371WNZX28ME4MN97B67PK");
    assert_eq!(mission.mission_number(), None);
    assert_eq!(mission.mission_number_display(), None);
    assert_eq!(mission.topology(), Some(MissionTopology::Lanes));
}

#[test]
fn legacy_numeric_string_mission_number_is_coerced_for_display() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "050-legacy-number");
    write(
        &dir.join("meta.json"),
        &meta_json(
            "01KZ6G6H4R8Y9J2N4Q7V3B6M1C",
            "050-legacy-number",
            None,
            "single_branch",
        )
        .replace("\"mission_number\": null", "\"mission_number\": \"0042\""),
    );

    let missions = list_missions(temp.path()).expect("inventory");
    assert_eq!(missions[0].mission_number(), Some(42));
    assert_eq!(missions[0].mission_number_display().as_deref(), Some("42"));
}

#[test]
fn mission_number_sentinels_fail_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "051-sentinel-number");
    write(
        &dir.join("meta.json"),
        &meta_json(
            "01KZ6G6H4R8Y9J2N4Q7V3B6M1D",
            "051-sentinel-number",
            None,
            "single_branch",
        )
        .replace(
            "\"mission_number\": null",
            "\"mission_number\": \"pending\"",
        ),
    );

    let error = list_missions(temp.path()).expect_err("sentinel number must fail closed");
    assert!(
        matches!(error, MissionError::InvalidMissionNumber { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn absent_specs_directory_is_an_empty_inventory() {
    let temp = tempfile::tempdir().expect("temporary root");
    let missions = list_missions(temp.path()).expect("no kitty-specs is an empty inventory");
    assert!(missions.is_empty());
}

#[test]
fn missing_root_is_refused() {
    let temp = tempfile::tempdir().expect("temporary root");
    let error = list_missions(&temp.path().join("missing")).expect_err("missing root");
    assert!(
        matches!(error, MissionError::RootNotDirectory { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn corrupt_metadata_fails_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "052-corrupt");
    write(&dir.join("meta.json"), "{ \"mission_id\": ");

    let error = list_missions(temp.path()).expect_err("corrupt JSON");
    assert!(
        matches!(error, MissionError::MalformedMetadata { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn non_object_metadata_fails_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "053-array");
    write(&dir.join("meta.json"), "[\"not\", \"an\", \"object\"]");

    let error = list_missions(temp.path()).expect_err("array metadata");
    assert!(
        matches!(error, MissionError::MetadataNotObject { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn non_utf8_metadata_fails_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "054-not-utf8");
    fs::write(dir.join("meta.json"), [0xff, 0xfe, 0x00, 0x01]).expect("binary metadata");

    let error = list_missions(temp.path()).expect_err("binary metadata");
    assert!(
        matches!(error, MissionError::MalformedMetadata { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn oversized_metadata_fails_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "055-oversized");
    let padding = " ".repeat(DEFAULT_META_BYTE_LIMIT as usize + 1);
    write(
        &dir.join("meta.json"),
        &format!(
            "{{\"mission_id\": \"01KZ6G6H4R8Y9J2N4Q7V3B6M1E\", \"padding\": \"{}\"}}",
            padding
        ),
    );

    let error = list_missions(temp.path()).expect_err("oversized metadata");
    match error {
        MissionError::MetadataTooLarge { limit, .. } => {
            assert_eq!(limit, DEFAULT_META_BYTE_LIMIT);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn missing_mission_id_fails_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "056-no-identity");
    write(
        &dir.join("meta.json"),
        "{\n  \"slug\": \"056-no-identity\",\n  \"mission_slug\": \"056-no-identity\"\n}\n",
    );

    let error = list_missions(temp.path()).expect_err("no mission_id");
    assert!(
        matches!(error, MissionError::MissingMissionId { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn invalid_mission_id_fails_closed() {
    for (slug, id) in [
        ("057-lowercase-id", "01kz6g6h4r8y9j2n4q7v3b6m1f"),
        ("058-short-id", "01KZ6G6H4R8Y9J2N4Q7V3B6M1"),
        ("059-illegal-char", "01KZ6G6H4R8Y9J2N4Q7V3B6M1I"),
    ] {
        let temp = temp_project();
        let dir = mission_dir(temp.path(), slug);
        write(
            &dir.join("meta.json"),
            &meta_json(id, slug, None, "single_branch"),
        );

        let error = list_missions(temp.path()).expect_err("invalid mission id");
        assert!(
            matches!(error, MissionError::InvalidMissionId { .. }),
            "{slug}: unexpected error: {error}"
        );
    }
}

#[test]
fn non_string_mission_id_fails_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "060-numeric-id");
    write(
        &dir.join("meta.json"),
        &meta_json(
            "01KZ6G6H4R8Y9J2N4Q7V3B6M1G",
            "060-numeric-id",
            None,
            "single_branch",
        )
        .replace(
            "\"mission_id\": \"01KZ6G6H4R8Y9J2N4Q7V3B6M1G\"",
            "\"mission_id\": 1234",
        ),
    );

    let error = list_missions(temp.path()).expect_err("numeric mission_id");
    assert!(
        matches!(error, MissionError::InvalidMissionIdType { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn duplicate_mission_ids_fail_closed() {
    let temp = temp_project();
    let id = "01KZ6G6H4R8Y9J2N4Q7V3B6M1H";
    write_mission(temp.path(), "061-first-copy", id);
    write_mission(temp.path(), "062-second-copy", id);

    let error = list_missions(temp.path()).expect_err("duplicate identities");
    assert!(
        matches!(error, MissionError::DuplicateMissionId { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn unsafe_mission_directory_name_fails_closed() {
    let temp = temp_project();
    let dir = mission_dir(temp.path(), "..not-a-slug");
    write(
        &dir.join("meta.json"),
        &meta_json(
            "01KZ6G6H4R8Y9J2N4Q7V3B6M1J",
            "..not-a-slug",
            None,
            "single_branch",
        ),
    );

    let error = list_missions(temp.path()).expect_err("unsafe directory name");
    assert!(
        matches!(error, MissionError::InvalidMissionDirectoryName { .. }),
        "unexpected error: {error}"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_mission_directory_is_refused() {
    use std::os::unix::fs::symlink;

    let temp = temp_project();
    let outside = tempfile::tempdir().expect("outside directory");
    let outside_mission = outside.path().join("063-outside");
    fs::create_dir_all(&outside_mission).expect("outside mission");
    write(
        &outside_mission.join("meta.json"),
        &meta_json(
            "01KZ6G6H4R8Y9J2N4Q7V3B6M1K",
            "063-outside",
            None,
            "single_branch",
        ),
    );
    symlink(&outside_mission, temp.path().join("kitty-specs/063-linked")).expect("symlink");

    let error = list_missions(temp.path()).expect_err("symlinked mission");
    assert!(
        matches!(error, MissionError::SymlinkedEntry { .. }),
        "unexpected error: {error}"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_meta_file_is_refused() {
    use std::os::unix::fs::symlink;

    let temp = temp_project();
    let dir = mission_dir(temp.path(), "064-linked-meta");
    let outside = tempfile::tempdir().expect("outside directory");
    let target = outside.path().join("meta.json");
    write(
        &target,
        &meta_json(
            "01KZ6G6H4R8Y9J2N4Q7V3B6M1M",
            "064-linked-meta",
            None,
            "single_branch",
        ),
    );
    symlink(&target, dir.join("meta.json")).expect("symlink meta");

    let error = list_missions(temp.path()).expect_err("symlinked meta");
    assert!(
        matches!(error, MissionError::SymlinkedEntry { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn inspection_model_serializes() {
    let missions = list_missions(&fixtures_project()).expect("fixture inventory");
    let value = serde_json::to_value(&missions[0]).expect("serialize mission");

    assert_eq!(value["slug"], "002-lightweight-pypi-release");
    assert_eq!(value["id"], "01KN2371WNZX28ME4MN97B67PJ");
    assert_eq!(value["mid8"], "01KN2371");
    assert_eq!(value["mission_number"], 2);
    assert_eq!(value["topology"], "single_branch");
    assert_eq!(value["meta"]["review_playbook"]["revision"], 4);
}

#[test]
fn real_repository_kitty_specs_are_indexable() {
    let root = support::repo_root();
    if !root.join("kitty-specs").is_dir() {
        eprintln!("skipping: {} has no kitty-specs", root.display());
        return;
    }

    let missions = list_missions(&root).expect("real repository inventory");
    assert!(
        missions.len() > 500,
        "expected the repository's real Mission fixtures, found {}",
        missions.len()
    );

    let mut previous: Option<&str> = None;
    for mission in &missions {
        let id = mission.id().as_str();
        assert_eq!(MissionId::validate(id), Ok(()), "non-canonical id {id}");
        assert_eq!(mission.mid8(), &id[..8]);
        assert_eq!(
            mission.slug().as_str(),
            mission.spec_dir().file_name().unwrap().to_str().unwrap()
        );
        if let Some(previous) = previous {
            assert!(
                previous < mission.slug().as_str(),
                "inventory must be sorted"
            );
        }
        previous = Some(mission.slug().as_str());
    }
}

fn project_mission_dir(root: &std::path::Path, slug: &str) -> std::path::PathBuf {
    fs::canonicalize(root)
        .expect("canonical fixture root")
        .join("kitty-specs")
        .join(slug)
}
