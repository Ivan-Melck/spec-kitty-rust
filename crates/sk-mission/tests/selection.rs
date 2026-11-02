mod support;

use sk_mission::{Mission, MissionError, list_missions, select_mission};
use support::{fixtures_project, meta_json, mission_dir, temp_project, write};

fn fixture_missions() -> Vec<Mission> {
    list_missions(&fixtures_project()).expect("fixture inventory")
}

#[test]
fn selects_by_full_mission_id() {
    let missions = fixture_missions();
    let mission = select_mission(&missions, "01KN2371WNZX28ME4MN97B67PK").expect("select by id");
    assert_eq!(mission.slug().as_str(), "003-auto-protect-agent");
}

#[test]
fn selects_by_mid8() {
    let temp = temp_project();
    support::write_mission(temp.path(), "072-single-mid8", "01KZ6G6H4R8Y9J2N4Q7V3B6M1F");
    let missions = list_missions(temp.path()).expect("inventory");

    let mission = select_mission(&missions, "01KZ6G6H").expect("select by mid8");
    assert_eq!(mission.id().as_str(), "01KZ6G6H4R8Y9J2N4Q7V3B6M1F");
}

#[test]
fn fixture_pair_with_a_shared_mid8_is_ambiguous_not_guessed() {
    // The two real fixture ids happen to share the "01KN2371" prefix (as some
    // real repository Missions do), so a mid8 selector must refuse to guess.
    let missions = fixture_missions();
    match select_mission(&missions, "01KN2371").expect_err("shared mid8") {
        MissionError::AmbiguousMission { selector, count } => {
            assert_eq!(selector, "01KN2371");
            assert_eq!(count, 2);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn selects_by_exact_slug() {
    let missions = fixture_missions();
    let mission =
        select_mission(&missions, "002-lightweight-pypi-release").expect("select by slug");
    assert_eq!(mission.id().as_str(), "01KN2371WNZX28ME4MN97B67PJ");
}

#[test]
fn numeric_prefix_is_not_a_selector() {
    let missions = fixture_missions();
    let error = select_mission(&missions, "002").expect_err("numeric prefix must not select");
    assert!(
        matches!(error, MissionError::MissionNotFound { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn slug_tail_is_not_a_selector() {
    let missions = fixture_missions();
    let error = select_mission(&missions, "lightweight-pypi-release")
        .expect_err("slug tail must not select");
    assert!(
        matches!(error, MissionError::MissionNotFound { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn mission_number_is_not_a_selector() {
    let missions = fixture_missions();
    let error = select_mission(&missions, "2").expect_err("mission_number must not select");
    assert!(
        matches!(error, MissionError::MissionNotFound { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn unknown_identity_is_not_found() {
    let missions = fixture_missions();
    let error = select_mission(&missions, "01KZ6G6H4R8Y9J2N4Q7V3B6M1Z").expect_err("unknown id");
    assert!(
        matches!(error, MissionError::MissionNotFound { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn ambiguous_mid8_is_refused() {
    let temp = temp_project();
    // Two canonical ULIDs sharing the same 8-character prefix.
    support::write_mission(
        temp.path(),
        "070-first-prefix",
        "01KZ6G6H4R8Y9J2N4Q7V3B6M1A",
    );
    support::write_mission(
        temp.path(),
        "071-second-prefix",
        "01KZ6G6H4R8Y9J2N4Q7V3B6M2B",
    );
    let missions = list_missions(temp.path()).expect("inventory");

    let error = select_mission(&missions, "01KZ6G6H").expect_err("ambiguous mid8");
    match error {
        MissionError::AmbiguousMission { count, .. } => assert_eq!(count, 2),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn mid8_identity_wins_over_a_matching_slug_and_never_falls_back() {
    let temp = temp_project();
    // A slug that is literally 8 uppercase characters would collide with a
    // mid8 selector. Identity wins; when the mid8 matches nothing there is no
    // fallback to the slug.
    let dir = mission_dir(temp.path(), "01KZ6G6H");
    write(
        &dir.join("meta.json"),
        &meta_json(
            "01KZ6G6H4R8Y9J2N4Q7V3B6M1C",
            "01KZ6G6H",
            None,
            "single_branch",
        ),
    );
    let other = mission_dir(temp.path(), "080-other-mission");
    write(
        &other.join("meta.json"),
        &meta_json(
            "01KZ6G6J4R8Y9J2N4Q7V3B6M1D",
            "080-other-mission",
            None,
            "single_branch",
        ),
    );
    let missions = list_missions(temp.path()).expect("inventory");

    // Mid8 "01KZ6G6H" matches the first mission's id prefix.
    assert_eq!(
        select_mission(&missions, "01KZ6G6H")
            .expect("mid8 selection")
            .id()
            .as_str(),
        "01KZ6G6H4R8Y9J2N4Q7V3B6M1C"
    );

    // "01KZ6G6X" does not match any id prefix; the exact slug is *not* used as
    // a fallback, so this is NotFound rather than a hidden ambiguity.
    assert!(matches!(
        select_mission(&missions, "01KZ6G6X").expect_err("no fallback"),
        MissionError::MissionNotFound { .. }
    ));
}

#[test]
fn twenty_six_character_slug_is_still_selectable() {
    let temp = temp_project();
    let slug = "012-documentation-missions";
    assert_eq!(slug.len(), 26, "this test pins the 26-character slug case");
    support::write_mission(temp.path(), slug, "01KZ6G6H4R8Y9J2N4Q7V3B6M1E");
    let missions = list_missions(temp.path()).expect("inventory");

    assert_eq!(
        select_mission(&missions, slug)
            .expect("slug selection")
            .slug()
            .as_str(),
        slug
    );
}

#[test]
fn invalid_selectors_are_refused() {
    let missions = fixture_missions();
    for selector in [
        "",
        " 002-lightweight-pypi-release",
        "002-lightweight-pypi-release ",
        "../escape",
        "a/b",
        "a\\b",
        ".",
        "..",
    ] {
        let error = select_mission(&missions, selector).expect_err(selector);
        assert!(
            matches!(error, MissionError::InvalidSelector { .. }),
            "{selector:?}: unexpected error: {error}"
        );
    }
}

#[test]
fn empty_inventory_selects_nothing() {
    let error = select_mission(&[], "002-lightweight-pypi-release").expect_err("no missions");
    assert!(matches!(error, MissionError::MissionNotFound { .. }));
}
