//! CF35 CLI adapter: the read-only `doctor coordination` route's actual refusals.
//!
//! The root `doctor` dispatch is parent-owned, so these cases call the adapter
//! entry point directly (`compat::doctor::coordination_health::run`) instead of
//! going through the binary. Each case runs in a re-executed child whose CWD is
//! the fixture, because `project_root()` resolves from the process working
//! directory and this crate must not mutate a shared CWD inside a test process.
//!
//! Asserted here:
//!
//! * `--fix` is refused BEFORE any root/repository read (an empty directory
//!   that is not a Git repository still yields the `--fix` code, exit 2);
//! * the read-only route reports the COMPLETE diagnosis (every check composed) and
//!   writes nothing;
//! * `--check-staleness` runs over the same complete surface;
//! * an unknown `--mission` handle is the reference's `MISSION_NOT_FOUND`
//!   domain refusal (exit 1), not a usage error.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const HARNESS_ENV: &str = "SK_CF35_CLI_CASE";
const MISSION_ID: &str = "01KNXQS9ATWWFXS3K5ZJ9E5001";
const SLUG: &str = "cf35-cli";
const COORD_BRANCH: &str = "kitty/mission-cf35-cli-01KNXQS9";
const TEST_NAME: &str = "the_route_refuses_with_the_documented_codes";

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "CF35 CLI Fixture")
        .env("GIT_AUTHOR_EMAIL", "cf35@example.invalid")
        .env("GIT_COMMITTER_NAME", "CF35 CLI Fixture")
        .env("GIT_COMMITTER_EMAIL", "cf35@example.invalid")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?} failed: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A real repository with one healthy coordination worktree.
fn coordinated_fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    git(root, &["init", "--quiet", "--initial-branch=main"]);
    write(&root.join(".gitignore"), ".worktrees/\n");
    write(&root.join("README.md"), "fixture\n");
    write(
        &root.join("kitty-specs").join(SLUG).join("meta.json"),
        &format!(
            "{}\n",
            serde_json::json!({
                "mission_id": MISSION_ID,
                "mission_slug": SLUG,
                "topology": "coord",
                "target_branch": "main",
                "coordination_branch": COORD_BRANCH,
                "mission_type": "software-dev",
            })
        ),
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "--quiet", "-m", "fixture"]);
    git(root, &["branch", COORD_BRANCH]);
    let worktree = root
        .join(".worktrees")
        .join(format!("{SLUG}-01KNXQS9-coord"));
    fs::create_dir_all(worktree.parent().expect("worktrees dir")).expect("worktrees dir");
    git(
        root,
        &[
            "worktree",
            "add",
            "--quiet",
            worktree.to_str().expect("utf-8 path"),
            COORD_BRANCH,
        ],
    );
    temp
}

/// The child: run one adapter case and print its outcome as JSON.
fn child_case(case: &str, root: &Path) {
    use sk_cli::compat::doctor::coordination_health::run;

    assert_eq!(
        std::env::current_dir().expect("cwd"),
        fs::canonicalize(root).expect("fixture root"),
        "the child must run in the fixture"
    );
    let outcome = match case {
        "fix" => run(true, true, false, None),
        "read-only" => run(false, true, false, None),
        "staleness" => run(false, true, true, None),
        "unknown-mission" => run(false, true, false, Some("no-such-mission")),
        other => panic!("unknown case {other}"),
    };
    let payload = match &outcome {
        Ok(code) => serde_json::json!({"kind": "ok", "exit": code}),
        Err(error) => serde_json::json!({
            "kind": "error",
            "exit": error.exit(),
            "payload": error.to_json(),
        }),
    };
    fs::write(
        root.join("cf35-cli-outcome.json"),
        serde_json::to_vec_pretty(&payload).expect("outcome JSON"),
    )
    .expect("write outcome");
}

fn run_case(case: &str, root: &Path) -> Value {
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", TEST_NAME, "--nocapture"])
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env(HARNESS_ENV, case)
        .env("HOME", root)
        .env("LC_ALL", "C.UTF-8")
        .output()
        .expect("run the adapter child");
    assert!(
        output.status.success(),
        "child {case} failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = fs::read(root.join("cf35-cli-outcome.json")).expect("child outcome");
    fs::remove_file(root.join("cf35-cli-outcome.json")).expect("clean the outcome file");
    serde_json::from_slice(&payload).expect("outcome JSON")
}

/// A snapshot of every file under the fixture, to prove no write happened.
fn footprint(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut rows: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        rows.sort();
        for path in rows {
            if path.is_dir() {
                walk(root, &path, files);
            } else if let Ok(bytes) = fs::read(&path) {
                files.push((
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                    bytes,
                ));
            }
        }
    }
    walk(root, root, &mut files);
    files
}

#[test]
fn the_route_refuses_with_the_documented_codes() {
    if let Some(case) = std::env::var_os(HARNESS_ENV) {
        let root = std::env::current_dir().expect("cwd");
        child_case(&case.to_string_lossy(), &root);
        return;
    }

    // `--fix` is mutating: refused before the root or repository is even read.
    let empty = tempfile::tempdir().expect("empty dir");
    let outcome = run_case("fix", empty.path());
    assert_eq!(outcome["kind"], "error");
    assert_eq!(outcome["exit"], 2);
    assert_eq!(
        outcome["payload"]["error_code"],
        "UNSUPPORTED_DOCTOR_COORDINATION_FIX"
    );
    assert!(
        outcome["payload"]["coverage"]
            .as_array()
            .is_some_and(|rows| rows.len() >= 2),
        "the refusal names what is missing: {}",
        outcome["payload"]
    );

    // The read-only route reports a complete diagnosis and writes nothing.
    let fixture = coordinated_fixture();
    let before = footprint(fixture.path());
    let outcome = run_case("read-only", fixture.path());
    let after = footprint(fixture.path());
    assert_eq!(before, after, "the read-only run writes nothing");
    assert_eq!(
        outcome["kind"], "ok",
        "a healthy fixture is a complete verdict: {}",
        outcome["exit"]
    );
    assert_eq!(outcome["exit"], 0);

    // `--check-staleness` runs over the same complete surface.
    let outcome = run_case("staleness", fixture.path());
    assert_eq!(outcome["kind"], "ok");
    assert_eq!(outcome["exit"], 0);

    // An unknown handle is the reference's domain refusal, exit 1.
    let outcome = run_case("unknown-mission", fixture.path());
    assert_eq!(outcome["kind"], "error");
    assert_eq!(outcome["exit"], 1);
    assert_eq!(outcome["payload"]["error_code"], "MISSION_NOT_FOUND");
    assert_eq!(outcome["payload"]["handle"], "no-such-mission");
    assert!(
        outcome["payload"]["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("Mission not found: 'no-such-mission'"),
        "{}",
        outcome["payload"]
    );
}
