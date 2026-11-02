//! The composed writer/adapter seam must never silently choose two repositories.

use std::path::Path;
use std::process::Command;

const SELECTORS: [&str; 6] = [
    "GIT_DIR",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_WORK_TREE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
];

fn clear_selectors(command: &mut Command) {
    for name in SELECTORS {
        command.env_remove(name);
    }
}

#[test]
fn writer_authority_refuses_inherited_selectors_instead_of_splitting_locks() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    for name in ["repo", "other"] {
        let path = temp.path().join(name);
        let mut git = Command::new("git");
        clear_selectors(&mut git);
        assert!(
            git.args(["init", "--quiet", "--initial-branch=main"])
                .arg(&path)
                .env("HOME", &home)
                .env("XDG_CONFIG_HOME", &home)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .status()
                .unwrap()
                .success()
        );
    }
    let root = temp.path().join("repo");
    let mission = root.join("kitty-specs/mission-01ABCDEF");
    std::fs::create_dir_all(&mission).unwrap();
    std::fs::write(
        mission.join("meta.json"),
        "{\"mission_slug\":\"mission-01ABCDEF\"}",
    )
    .unwrap();
    for selector in std::iter::once(None).chain(SELECTORS.into_iter().map(Some)) {
        let mut child = Command::new(std::env::current_exe().unwrap());
        clear_selectors(&mut child);
        child
            .args([
                "--ignored",
                "--exact",
                "authority_probe_child",
                "--nocapture",
            ])
            .env("SK_AUTHORITY_ROOT", &root)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home);
        if let Some(selector) = selector {
            let value = if selector == "GIT_DISCOVERY_ACROSS_FILESYSTEM" {
                std::ffi::OsString::from("1")
            } else {
                temp.path().join("other/.git").into_os_string()
            };
            child
                .env(selector, value)
                .env("SK_AUTHORITY_SELECTOR", selector);
        } else {
            child.env_remove("SK_AUTHORITY_SELECTOR");
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{selector:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!root.join(".git/spec-kitty-locks").exists());
    assert!(!temp.path().join("other/.git/spec-kitty-locks").exists());
}

#[test]
#[ignore = "subprocess helper exercised by the authority parent"]
fn authority_probe_child() {
    let root = std::env::var_os("SK_AUTHORITY_ROOT").expect("parent fixture root");
    let root = Path::new(&root);
    let mission = root.join("kitty-specs/mission-01ABCDEF");
    let repository = sk_git::GitRepository::open(root).unwrap();
    let expected = root.join(".git").canonicalize().unwrap();
    assert_eq!(repository.git_dirs().unwrap().common_dir(), expected);
    assert_eq!(
        sk_workspace::ProjectWorkspace::open(root)
            .unwrap()
            .common_dir(),
        expected
    );
    let status = sk_status::mutation::mission_lock_path(&mission, Some(root));
    let journal =
        sk_kernel::event_log_append::event_log_lock_path(&mission.join("decisions.events.jsonl"));
    if let Ok(selector) = std::env::var("SK_AUTHORITY_SELECTOR") {
        assert!(status.unwrap_err().to_string().contains(&selector));
        assert!(journal.unwrap_err().to_string().contains(&selector));
        assert!(
            sk_status::mutation::acquire_mission_lock(
                &mission,
                Some(root),
                sk_kernel::LockWait::NonBlocking
            )
            .unwrap_err()
            .to_string()
            .contains(&selector)
        );
        let log = mission.join("decisions.events.jsonl");
        assert!(
            sk_kernel::event_log_append::append_event_log_lines(
                &log,
                &["not-written"],
                &sk_kernel::event_log_append::EventLogAppendOptions::default()
            )
            .is_err()
        );
        assert!(!log.exists());
    } else {
        assert!(status.unwrap().starts_with(&expected));
        assert!(journal.unwrap().starts_with(&expected));
    }
}
