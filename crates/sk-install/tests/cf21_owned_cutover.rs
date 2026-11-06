//! CF21 — the OWNED runtime-cutover leg, driven through the canonical ownership
//! fact and the real public entry point.
//!
//! Source facts pinned here (read from the pinned reference before writing the
//! assertions):
//!
//! * `cutover_mission(..., owned=...)` validates BOTH legs against
//!   `owned.mission_dir` before the residue check, the tracked-metadata notice or
//!   the first seed write (`_runtime_feature_dir` ->
//!   `OWNED_MISSION_PATH_REFUSED`, "Runtime state must use the selected mission
//!   directory."), skips the residue classification, and flips `status_phase` at
//!   the FACT's mission directory (never at `canonicalize_feature_dir`'s
//!   repository copy and never at the coordination surface).
//! * `_resolve_primary_home_or_degrade`'s owned arm answers
//!   `resolve_artifact_surface(..., PRIMARY_METADATA, owned=...)`, which for a
//!   PRIMARY-partition kind is `owned.mission_dir` for every topology — so the
//!   flip target, the placement answer and the lock root all come from the fact.
//! * `mission_write_lock(target, repo_root=owned.repository_root,
//!   fallback_to_dir_name=True)`: the owned lock lives in the repository's lock
//!   directory, not the worktree's.
//!
//! These are Native-only functional smokes: fresh real `git init`/worktree
//! fixtures, no copied linked repo, no Source child process. They are not paired
//! isolation evidence and no equivalence/readiness claim follows.

use std::path::{Path, PathBuf};
use std::process::Command;

use sk_git::BranchName;
use sk_install::migrations::completion_modern::rescue_spine::{
    cutover_corpus, cutover_owned_checkout,
};
use sk_mission::{Mission, list_missions};
use sk_workspace::OwnedCheckout;

const SLUG: &str = "my-mission";
const MISSION_ID: &str = "01KNXQS9ATWWFXS3K5ZJ9E5001";
const MISSION_BRANCH: &str = "kitty/mission-my-mission-01KNXQS9";

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        // Isolate from the user's global/system config and any hooks.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Spec Kitty Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Spec Kitty Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}:\nstdout: {}\nstderr: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, contents).expect("write file");
}

/// A real repository on `main` with one commit.
fn init_repo(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.name", "Spec Kitty Test"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    write(&root.join("README.md"), "# fixture\n");
    git(root, &["add", "README.md"]);
    git(root, &["commit", "-m", "initial"]);
}

/// One legacy mission: a manifest with a STORED topology, a WP whose frontmatter
/// still carries `lane`, and the legacy `status.json` the seed phase reads.
fn write_mission(root: &Path) -> PathBuf {
    let dir = root.join("kitty-specs").join(SLUG);
    let meta = serde_json::json!({
        "mission_id": MISSION_ID,
        "mission_slug": SLUG,
        "topology": "single_branch",
        "target_branch": "main",
        "mission_type": "software-dev",
    });
    write(
        &dir.join("meta.json"),
        &format!("{}\n", serde_json::to_string_pretty(&meta).expect("meta")),
    );
    write(&dir.join("spec.md"), "# spec\n");
    // The seed phase reads the evictable legacy runtime fields; `lane` alone is not
    // evictable state, so a WP must carry the fields the eviction removes.
    write(
        &dir.join("tasks/WP01-task.md"),
        "---\nwork_package_id: WP01\nlane: in_progress\nshell_pid: 4242\nshell_pid_created_at: '2026-03-04T05:00:00+00:00'\nagent: claude\nassignee: ana\ntitle: Task\n---\n\nbody\n",
    );
    dir
}

fn commit_mission(root: &Path) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "mission"]);
}

fn load_owned(owned_root: &Path) -> Mission {
    list_missions(owned_root)
        .expect("mission inventory")
        .into_iter()
        .find(|mission| mission.slug().as_str() == SLUG)
        .expect("mission present in the owned checkout")
}

fn mint(repository_root: &Path, owned_root: &Path) -> OwnedCheckout {
    let mission = load_owned(owned_root);
    OwnedCheckout::mint(
        repository_root,
        owned_root,
        &mission,
        &BranchName::parse(MISSION_BRANCH).expect("branch name"),
    )
    .expect("owned fact")
}

fn status_phase(mission_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(mission_dir.join("meta.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("status_phase")
        .and_then(|phase| phase.as_str())
        .map(str::to_string)
}

/// repository root checkout (holds the PRIMARY copy) + a real mission worktree
/// (the owned checkout), on fresh temp storage.
struct OwnedFixture {
    _temp: tempfile::TempDir,
    repository_root: PathBuf,
    owned_root: PathBuf,
}

fn owned_fixture() -> OwnedFixture {
    let temp = tempfile::tempdir().expect("temp");
    let repository_root = temp.path().join("repo");
    init_repo(&repository_root);
    write_mission(&repository_root);
    commit_mission(&repository_root);
    git(&repository_root, &["branch", MISSION_BRANCH]);
    let owned_root = temp.path().join("owned-checkout");
    git(
        &repository_root,
        &[
            "worktree",
            "add",
            owned_root.to_str().expect("utf8"),
            MISSION_BRANCH,
        ],
    );
    OwnedFixture {
        _temp: temp,
        repository_root,
        owned_root,
    }
}

#[test]
fn the_owned_leg_flips_the_owned_copy_and_leaves_the_repository_copy_alone() {
    let fixture = owned_fixture();
    let fact = mint(&fixture.repository_root, &fixture.owned_root);

    // A PREVIEW on the owned leg writes nothing at all.
    let preview = cutover_owned_checkout(&fixture.repository_root, &fact, true).expect("preview");
    assert!(preview.abort.is_none(), "{preview:?}");
    assert_eq!(preview.flipped(), 0, "{preview:?}");
    assert!(preview.partial_writes.is_empty(), "{preview:?}");
    assert!(status_phase(fact.mission_dir()).is_none(), "preview wrote");
    assert!(
        !fixture
            .repository_root
            .join("kitty-specs")
            .join(SLUG)
            .join("status.events.jsonl")
            .exists()
    );

    let report = cutover_owned_checkout(&fixture.repository_root, &fact, false).expect("apply");
    assert!(report.abort.is_none(), "{report:?}");
    assert_eq!(report.outcomes.len(), 1, "{report:?}");
    let outcome = &report.outcomes[0];
    assert!(outcome.error.is_none(), "{outcome:?}");
    assert!(!outcome.residue, "{outcome:?}");
    assert_eq!(outcome.slug, SLUG);
    // The flip, the seed events and the derived views all landed on the FACT's
    // mission directory.
    assert_eq!(
        status_phase(fact.mission_dir()).as_deref(),
        Some("1"),
        "{outcome:?}"
    );
    assert!(
        fact.mission_dir().join("status.events.jsonl").is_file(),
        "{outcome:?}"
    );
    // The repository root checkout's copy of the same mission is untouched.
    assert!(status_phase(&fixture.repository_root.join("kitty-specs").join(SLUG)).is_none());

    // Retry is idempotent: no new seeds, the flip short-circuits, still `flipped`.
    let retry = cutover_owned_checkout(&fixture.repository_root, &fact, false).expect("retry");
    assert!(retry.abort.is_none(), "{retry:?}");
    let second = &retry.outcomes[0];
    assert!(second.flipped, "{second:?}");
    assert!(second.already_migrated, "{second:?}");
    assert_eq!(second.seeded_count, 0, "{second:?}");
}

#[test]
fn the_unowned_walk_flips_the_repository_copy_while_the_owned_leg_flips_the_worktree_copy() {
    let fixture = owned_fixture();
    let fact = mint(&fixture.repository_root, &fixture.owned_root);

    // Unowned leg (`cutover_corpus`): `canonicalize_feature_dir` rewrites a
    // worktree-rooted mission to the canonical repository copy, so that is where
    // the unowned flip lands.
    let report = cutover_corpus(&fixture.repository_root, false).expect("corpus");
    assert!(report.abort.is_none(), "{report:?}");
    assert_eq!(
        status_phase(&fixture.repository_root.join("kitty-specs").join(SLUG)).as_deref(),
        Some("1"),
        "{report:?}"
    );
    assert!(
        status_phase(fact.mission_dir()).is_none(),
        "the unowned walk never writes the owned checkout's copy"
    );

    // Owned leg: the flip target is the FACT's mission directory.
    let owned = cutover_owned_checkout(&fixture.repository_root, &fact, false).expect("owned");
    assert!(owned.abort.is_none(), "{owned:?}");
    assert_eq!(
        status_phase(fact.mission_dir()).as_deref(),
        Some("1"),
        "{owned:?}"
    );
}

#[test]
fn the_owned_leg_refuses_a_fact_whose_mission_directory_is_not_the_leg() {
    let fixture = owned_fixture();
    let fact = mint(&fixture.repository_root, &fixture.owned_root);
    // A mission directory that exists but is not the fact's own: the reference
    // refuses with `OWNED_MISSION_PATH_REFUSED` instead of re-rooting silently.
    let foreign = fixture.repository_root.join("kitty-specs").join(SLUG);
    let error = sk_mission::migration_rescue::cutover_mission(
        sk_mission::migration_rescue::CutoverRequest {
            feature_dir: &foreign,
            status_feature_dir: None,
            dry_run: true,
            placement: &sk_mission::migration_rescue::PlacementPort::Degraded(
                "unused on the owned leg".to_string(),
            ),
            residue: None,
            owned: Some(sk_mission::migration_rescue::OwnedMission::new(
                fact.repository_root(),
                fact.mission_dir(),
                fact.mission_slug().as_str(),
                fact.topology(),
            )),
        },
    );
    assert!(!error.flipped, "{error:?}");
    assert_eq!(
        error.error.as_deref(),
        Some("Runtime state must use the selected mission directory."),
        "{error:?}"
    );
}
