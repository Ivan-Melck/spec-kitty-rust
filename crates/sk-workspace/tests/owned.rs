//! The ownership fact is mint-only and manifest-backed.
//!
//! No test here forges a fact from a directory name: every positive case loads
//! a real `meta.json`, and every refusal is checked for the stable code.

mod support;

use std::path::PathBuf;

use sk_git::BranchName;
use sk_mission::{Mission, list_missions};
use sk_workspace::{OwnedCheckout, WorkspaceError};

const SPECS_DIR_NAME: &str = "kitty-specs";

use support::{commit_all, init_repo, mission_id, write, write_mission};

fn load_from(checkout: &std::path::Path, slug: &str) -> Mission {
    let missions = list_missions(checkout).expect("mission inventory");
    missions
        .into_iter()
        .find(|mission| mission.slug().as_str() == slug)
        .expect("mission present")
}

fn mint_error(
    fixture: &support::Fixture,
    owned: &std::path::Path,
    mission: &Mission,
) -> WorkspaceError {
    OwnedCheckout::mint(
        fixture.root(),
        owned,
        mission,
        &BranchName::parse("kitty/mission-my-mission-01KNXQS9").unwrap(),
    )
    .expect_err("mint must refuse")
}

#[test]
fn repository_root_can_never_be_an_owned_checkout() {
    let fixture = init_repo();
    write_mission(
        fixture.root(),
        "my-mission",
        &mission_id(1),
        "lanes",
        "main",
    );
    let mission = load_from(fixture.root(), "my-mission");
    let error = mint_error(&fixture, fixture.root(), &mission);
    assert!(matches!(
        error,
        WorkspaceError::OwnedRefused {
            code: "OWNED_CHECKOUT_IS_REPOSITORY_ROOT",
            ..
        }
    ));
}

#[test]
fn a_manifest_from_another_checkout_cannot_back_this_owned_root() {
    let fixture = init_repo();
    write_mission(
        fixture.root(),
        "my-mission",
        &mission_id(2),
        "lanes",
        "main",
    );
    commit_all(fixture.root(), "mission");
    let branch = "kitty/mission-my-mission-01KNXQS9";
    support::git(fixture.root(), &["branch", branch]);
    let owned = fixture.path("owned-checkout");
    support::git(
        fixture.root(),
        &["worktree", "add", owned.to_str().unwrap(), branch],
    );

    // The mission was loaded from the primary checkout, so its canonical
    // directory does not belong to the candidate owned root. Directory-name
    // resemblance is not ownership.
    let from_primary = load_from(fixture.root(), "my-mission");
    let error = mint_error(&fixture, &owned, &from_primary);
    assert!(matches!(
        error,
        WorkspaceError::OwnedRefused {
            code: "OWNED_MISSION_PATH_REFUSED",
            ..
        }
    ));

    // The same mission loaded from the owned checkout's own manifest mints.
    let from_owned = load_from(&owned, "my-mission");
    let fact = OwnedCheckout::mint(
        fixture.root(),
        &owned,
        &from_owned,
        &BranchName::parse(branch).unwrap(),
    )
    .expect("owned fact");
    assert_eq!(fact.owned_root(), std::fs::canonicalize(&owned).unwrap());
    assert_ne!(fact.owned_root(), fact.repository_root());
    assert_eq!(fact.mission_slug().as_str(), "my-mission");
    assert_eq!(fact.mid8(), "01KNXQS9");
    assert_eq!(fact.topology(), sk_mission::MissionTopology::Lanes);
    assert!(fact.handle_names_mission("my-mission"));
    assert!(fact.handle_names_mission("01KNXQS9"));
    assert!(fact.handle_names_mission("01KNXQS9ATWWFXS3K5ZJ9E5002"));
    assert!(!fact.handle_names_mission("01KNXQS9-other-mission"));
    assert!(!fact.handle_names_mission("my-mission-01KNXQS9"));
}

#[test]
fn a_deleted_manifest_fails_closed_after_loading() {
    let fixture = init_repo();
    write_mission(
        fixture.root(),
        "my-mission",
        &mission_id(3),
        "lanes",
        "main",
    );
    commit_all(fixture.root(), "mission");
    let branch = "kitty/mission-my-mission-01KNXQS9";
    support::git(fixture.root(), &["branch", branch]);
    let owned = fixture.path("owned-checkout");
    support::git(
        fixture.root(),
        &["worktree", "add", owned.to_str().unwrap(), branch],
    );
    let mission = load_from(&owned, "my-mission");
    std::fs::remove_file(owned.join(SPECS_DIR_NAME).join("my-mission/meta.json")).unwrap();
    let error = mint_error(&fixture, &owned, &mission);
    assert!(matches!(error, WorkspaceError::MissingManifest { .. }));
}

#[test]
fn files_are_contained_in_the_mission_directory() {
    let fixture = init_repo();
    write_mission(
        fixture.root(),
        "my-mission",
        &mission_id(4),
        "lanes",
        "main",
    );
    write(
        &fixture.root().join("kitty-specs/my-mission/spec.md"),
        "# spec\n",
    );
    commit_all(fixture.root(), "mission");
    let branch = "kitty/mission-my-mission-01KNXQS9";
    support::git(fixture.root(), &["branch", branch]);
    let owned = fixture.path("owned-checkout");
    support::git(
        fixture.root(),
        &["worktree", "add", owned.to_str().unwrap(), branch],
    );
    let mission = load_from(&owned, "my-mission");
    let fact = OwnedCheckout::mint(
        fixture.root(),
        &owned,
        &mission,
        &BranchName::parse(branch).unwrap(),
    )
    .unwrap();

    let allowed = fact
        .files(&[PathBuf::from("kitty-specs/my-mission/spec.md")], false)
        .unwrap();
    assert!(allowed[0].starts_with(fact.mission_dir()));

    let traversal = fact
        .files(
            &[PathBuf::from("kitty-specs/my-mission/../escape.md")],
            false,
        )
        .unwrap_err();
    assert!(matches!(traversal, WorkspaceError::OwnedPathRefused { .. }));

    let outside = fact
        .files(&[fixture.root().join("README.md")], false)
        .unwrap_err();
    assert!(matches!(outside, WorkspaceError::OwnedPathRefused { .. }));

    // A link inside the mission is stored as a link, but a boundary screen
    // that follows links refuses it when it leads outside the mission.
    #[cfg(unix)]
    {
        let link = fact.mission_dir().join("outside-link");
        std::os::unix::fs::symlink(fixture.root().join("README.md"), &link).unwrap();
        let stored = fact.files(
            &[PathBuf::from("kitty-specs/my-mission/outside-link")],
            false,
        );
        assert!(stored.is_ok(), "a stored symlink is inside the mission");
        let followed = fact.files(
            &[PathBuf::from("kitty-specs/my-mission/outside-link")],
            true,
        );
        assert!(matches!(
            followed,
            Err(WorkspaceError::OwnedPathRefused { .. })
        ));
    }
}
