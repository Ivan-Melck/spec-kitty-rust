//! Contracts for the native Op commit-or-skip policy.
//!
//! Real temporary Git repositories establish the source-observable answers:
//! primary resolution (including `origin/HEAD` targets, dangling targets,
//! local/origin trees and the `main`/`master`/`develop` ladder), the exact
//! Op-record path shape, and the two skip-reason families. A fake `git` at the
//! configured executable boundary establishes only what a real repository
//! cannot produce deterministically: exact resolution order and call counts,
//! arbitrary non-1 exits, signalled children, timeouts and capture overflow.
//!
//! The fake-git cases are Unix-only for the same reason the existing
//! ignore-probe tests are: `/bin/sh` scripts. Real-repository and pure-shape
//! cases run everywhere.
//!
//! Every expected user-facing skip string is compared against the lock-exact
//! Python source as well as asserted literally.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use sk_git::{GitError, GitOptions, GitPath, GitRepository, RefName};
use sk_invocation::{
    COMMON_PRIMARY_BRANCHES, OpCommitProbeError, OpCommitSkipProbe, PrimaryTracking,
    is_op_record_path, op_commit_skip_reason, primary_tracks_op_records, probe_primary_tracking,
    resolve_primary_refs,
};

const RECORD: &str = "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAV.jsonl";
const SPINE: &str = "kitty-ops/op-closures.jsonl";
const INDEX: &str = "kitty-ops/ops-index.jsonl";
const SENTINEL: &str = "kitty-ops/00000000000000000000000000.jsonl";
const EIGHT_ZEROS_ID: &str = "00000000000000000000000001";

const GIT_SELECTORS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_NAMESPACE",
    "GIT_SHALLOW_FILE",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
];

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(branch: &str) -> Self {
        let temp = tempfile::tempdir().expect("create tempdir");
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        fs::create_dir_all(&root).expect("create repository directory");
        fs::create_dir_all(&home).expect("create scratch home");
        let fixture = Self { temp, root, home };
        fixture.git_ok(&["init", "-q", "-b", branch]);
        fixture.git_ok(&["config", "user.name", "Fixture"]);
        fixture.git_ok(&["config", "user.email", "fixture@example.invalid"]);
        fixture.git_ok(&["config", "commit.gpgsign", "false"]);
        fs::write(fixture.root.join("README.md"), b"# fixture\n").expect("write README");
        fixture.git_ok(&["add", "README.md"]);
        fixture.git_ok(&["commit", "-q", "-m", "initial"]);
        fixture
    }

    fn open(&self) -> GitRepository {
        GitRepository::open(&self.root).expect("open fixture repository")
    }

    fn git(&self, args: &[&str]) -> Output {
        let mut command = Command::new("git");
        command
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"))
            .env("GIT_AUTHOR_NAME", "Test Author")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test Committer")
            .env("GIT_COMMITTER_EMAIL", "test@example.com");
        for variable in GIT_SELECTORS {
            command.env_remove(variable);
        }
        command.env_remove("GIT_INDEX_FILE");
        command.args(args).output().expect("run git")
    }

    fn git_ok(&self, args: &[&str]) -> Vec<u8> {
        let output = self.git(args);
        assert!(
            output.status.success(),
            "git {args:?} failed in {}: {}",
            self.root.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn head(&self) -> String {
        String::from_utf8(self.git_ok(&["rev-parse", "HEAD"]))
            .expect("rev-parse HEAD is UTF-8")
            .trim()
            .to_string()
    }

    /// Commit `rel` with `content`, force-added so an ignore rule cannot hide it.
    fn commit_file(&self, rel: &str, content: &str) -> String {
        let target = self.root.join(rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create fixture directory");
        }
        fs::write(&target, content).expect("write fixture file");
        self.git_ok(&["add", "--force", rel]);
        self.git_ok(&["commit", "-q", "-m", &format!("add {rel}")]);
        self.head()
    }

    fn checkout(&self, branch: &str) {
        self.git_ok(&["checkout", "-q", "-b", branch]);
    }

    fn symbolic_ref(&self, name: &str, target: &str) {
        self.git_ok(&["symbolic-ref", name, target]);
    }

    fn update_ref(&self, name: &str, target: &str) {
        self.git_ok(&["update-ref", name, target]);
    }
}

fn path(raw: &str) -> GitPath {
    GitPath::from_git_bytes(raw.as_bytes()).expect("valid fixture path")
}

fn refname(raw: &str) -> RefName {
    RefName::parse(raw).expect("valid fixture ref")
}

fn names(refs: &[RefName]) -> Vec<String> {
    refs.iter()
        .map(|reference| reference.as_str().to_owned())
        .collect()
}

fn expected(tracked: bool, primary: Option<&str>) -> PrimaryTracking {
    PrimaryTracking::new(tracked, primary.map(str::to_owned))
}

#[test]
fn oracle_runs_in_temporary_cwd_with_absolute_source_imports() {
    let result = support::run_python(
        "import json,os,kernel.resolution\nprint(json.dumps([os.getcwd(),kernel.resolution.__file__]))",
        None,
    );
    let paths: Vec<String> = serde_json::from_str(&result).unwrap();
    let root = support::repository_root();
    assert_ne!(std::path::Path::new(&paths[0]), root.as_path());
    assert!(std::path::Path::new(&paths[0]).starts_with(std::env::temp_dir()));
    assert_eq!(
        std::path::Path::new(&paths[1]),
        root.join("src/kernel/resolution.py")
    );
}

#[test]
fn common_primary_branches_keep_the_source_order() {
    assert_eq!(COMMON_PRIMARY_BRANCHES, ["main", "master", "develop"]);
}

#[test]
fn the_op_record_shape_is_26_upper_case_crockford_characters() {
    assert!(is_op_record_path(&path(RECORD)));
    assert!(is_op_record_path(&path(&format!("sub/{RECORD}"))));
    assert!(is_op_record_path(&path(
        "kitty-ops/archive/kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAV.jsonl"
    )));
    assert!(is_op_record_path(&path(&format!(
        "x/kitty-ops/{}.jsonl",
        "Z".repeat(26)
    ))));
    assert!(is_op_record_path(&path(SENTINEL)));

    // A file name ending in a newline still matches: the reference uses
    // `re.search` with `$`, which also matches before one final newline. This
    // is a Python anchor quirk the native predicate preserves deliberately.
    assert!(is_op_record_path(&path(&format!("{RECORD}\n"))));
    assert!(!is_op_record_path(&path(&format!("{RECORD}\n\n"))));

    for rejected in [
        // Not a per-Op record: the index, the closure spine, arbitrary JSON.
        INDEX,
        SPINE,
        "kitty-ops/notes.json",
        "kitty-ops/propagation-errors.jsonl",
        "kitty-ops/lifecycle.jsonl",
        // Wrong id shape.
        "kitty-ops/01arz3ndektsv4rrffq69g5fav.jsonl",
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FA.jsonl",
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAVV.jsonl",
        // `I`, `L`, `O` and `U` are outside the Crockford alphabet.
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAI.jsonl",
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAL.jsonl",
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAO.jsonl",
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAU.jsonl",
        // Wrong directory component.
        "xkitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAV.jsonl",
        "kitty-opsx/01ARZ3NDEKTSV4RRFFQ69G5FAV.jsonl",
        // Wrong suffix.
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAV.jsonl.bak",
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAV.json",
        "",
    ] {
        assert!(!is_op_record_path(&path(rejected)), "{rejected:?}");
    }
}

#[test]
fn an_origin_less_topic_branch_record_is_unresolved_and_skipped() {
    let fixture = Fixture::new("trunk");
    fixture.checkout("fix/x");
    fixture.commit_file(RECORD, "{}\n");

    let repository = fixture.open();
    assert_eq!(
        repository
            .current_branch()
            .expect("current branch")
            .as_deref(),
        Some("fix/x"),
        "the control must run on the branch that holds the record"
    );
    assert_eq!(
        resolve_primary_refs(&repository).expect("resolve"),
        Vec::<RefName>::new(),
        "no current-branch fallback is allowed"
    );
    let tracking = probe_primary_tracking(&repository).expect("probe");
    assert_eq!(tracking, expected(false, None));
    assert_eq!(
        op_commit_skip_reason(&repository, &path(RECORD), &tracking).expect("skip reason"),
        Some("primary branch unresolved, so it cannot be shown to track Op records".to_string())
    );
}

#[test]
fn a_resolved_primary_holding_only_special_files_is_named_but_not_tracked() {
    let fixture = Fixture::new("main");
    for rel in [
        INDEX,
        SPINE,
        "kitty-ops/notes.json",
        "kitty-ops/01arz3ndektsv4rrffq69g5fav.jsonl",
        "kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAI.jsonl",
    ] {
        fixture.commit_file(rel, "{}\n");
    }

    let repository = fixture.open();
    assert_eq!(
        resolve_primary_refs(&repository).expect("resolve"),
        vec![refname("refs/heads/main")]
    );
    let tracking = probe_primary_tracking(&repository).expect("probe");
    assert_eq!(tracking, expected(false, Some("main")));
    assert_eq!(
        op_commit_skip_reason(&repository, &path(RECORD), &tracking).expect("skip reason"),
        Some("the primary branch (main) tracks no Op records under kitty-ops/".to_string())
    );
}

#[test]
fn a_primary_with_one_crockford_record_is_tracked_and_commit_eligible() {
    let fixture = Fixture::new("main");
    fixture.commit_file(&format!("kitty-ops/{}.jsonl", "Z".repeat(26)), "{}\n");
    fixture.commit_file(SENTINEL, "{}\n");
    fixture.checkout("fix/x");

    let repository = fixture.open();
    let tracking = probe_primary_tracking(&repository).expect("probe");
    assert_eq!(tracking, expected(true, Some("main")));
    // The `Z`×26 and `0`×… ids are shape-valid even though they are not genuine
    // ULIDs; neither the predicate nor the tree scan decodes them.
    assert_eq!(
        op_commit_skip_reason(
            &repository,
            &path(&format!("kitty-ops/{EIGHT_ZEROS_ID}.jsonl")),
            &tracking
        )
        .expect("skip reason"),
        None
    );
}

#[test]
fn a_topic_branch_record_does_not_satisfy_a_resolved_primary() {
    let fixture = Fixture::new("main");
    fixture.checkout("fix/x");
    fixture.commit_file(RECORD, "{}\n");

    let repository = fixture.open();
    let tracking = probe_primary_tracking(&repository).expect("probe");
    assert_eq!(tracking, expected(false, Some("main")));
    assert_eq!(
        op_commit_skip_reason(&repository, &path(RECORD), &tracking).expect("skip reason"),
        Some("the primary branch (main) tracks no Op records under kitty-ops/".to_string())
    );
}

#[test]
fn origin_head_selects_the_primary_and_prefers_the_local_ref() {
    let fixture = Fixture::new("trunk");
    let head = fixture.head();
    fixture.update_ref("refs/remotes/origin/trunk", &head);
    fixture.symbolic_ref("refs/remotes/origin/HEAD", "refs/remotes/origin/trunk");
    fixture.commit_file(RECORD, "{}\n");

    let repository = fixture.open();
    assert_eq!(
        names(&resolve_primary_refs(&repository).expect("resolve")),
        vec!["refs/heads/trunk", "refs/remotes/origin/trunk"]
    );
    let tracking = probe_primary_tracking(&repository).expect("probe");
    assert_eq!(tracking, expected(true, Some("trunk")));
}

#[test]
fn source_strips_symbolic_stdout_before_removing_the_origin_prefix() {
    for (name, reference_tracks) in [("unusual\u{a0}", false), ("\u{a0}unusual", true)] {
        let fixture = Fixture::new("main");
        fixture.checkout(name);
        fixture.commit_file(RECORD, "{}\n");
        fixture.update_ref(&format!("refs/remotes/origin/{name}"), &fixture.head());
        fixture.symbolic_ref(
            "refs/remotes/origin/HEAD",
            &format!("refs/remotes/origin/{name}"),
        );
        let request = serde_json::json!({"root": fixture.root});
        let oracle = support::run_python(
            "import json,sys\nfrom pathlib import Path\nfrom specify_cli.invocation.op_commit_policy import resolve_primary_refs,probe_primary_tracking\nr=Path(json.load(sys.stdin)['root'])\nt=probe_primary_tracking(r)\nprint(json.dumps({'refs':resolve_primary_refs(r),'tracked':t.tracked,'primary':t.primary}))",
            Some(&request.to_string()),
        );
        let oracle: serde_json::Value = serde_json::from_str(&oracle).unwrap();
        assert_eq!(
            oracle["tracked"], reference_tracks,
            "live reference positive/negative control"
        );
        let repository = fixture.open();
        assert_eq!(
            repository
                .symbolic_ref_short_target(&refname("refs/remotes/origin/HEAD"))
                .unwrap(),
            Some(format!("origin/{name}")),
            "generic Git read must preserve the real ref spelling"
        );
        let refs = resolve_primary_refs(&repository).unwrap();
        let tracking = probe_primary_tracking(&repository).unwrap();
        let native = serde_json::json!({"refs": names(&refs), "tracked": tracking.tracked(), "primary": tracking.primary()});
        assert_eq!(
            native, oracle,
            "policy must match source strip order for legal ref {name:?}"
        );
    }
}

#[test]
fn an_origin_head_target_without_a_local_branch_still_resolves() {
    let fixture = Fixture::new("work");
    let head = fixture.head();
    fixture.update_ref("refs/remotes/origin/trunk", &head);
    fixture.symbolic_ref("refs/remotes/origin/HEAD", "refs/remotes/origin/trunk");
    fixture.commit_file(RECORD, "{}\n");
    fixture.update_ref("refs/remotes/origin/trunk", &fixture.head());

    let repository = fixture.open();
    assert_eq!(
        names(&resolve_primary_refs(&repository).expect("resolve")),
        vec!["refs/remotes/origin/trunk"],
        "the short name is not a ladder candidate, so it is probed directly"
    );
    assert_eq!(
        probe_primary_tracking(&repository).expect("probe"),
        expected(true, Some("trunk"))
    );
}

#[test]
fn a_dangling_origin_head_falls_through_to_the_ladder() {
    let fixture = Fixture::new("main");
    fixture.commit_file(RECORD, "{}\n");
    fixture.symbolic_ref("refs/remotes/origin/HEAD", "refs/remotes/origin/gone");

    let repository = fixture.open();
    assert_eq!(
        names(&resolve_primary_refs(&repository).expect("resolve")),
        vec!["refs/heads/main"]
    );
    assert_eq!(
        probe_primary_tracking(&repository).expect("probe"),
        expected(true, Some("main"))
    );
}

#[test]
fn the_ladder_uses_main_then_master_then_develop() {
    // Only `develop` exists, reached past `main` and `master`.
    let fixture = Fixture::new("work");
    fixture.commit_file(RECORD, "{}\n");
    fixture.git_ok(&["branch", "develop", "HEAD"]);
    let repository = fixture.open();
    assert_eq!(
        names(&resolve_primary_refs(&repository).expect("resolve")),
        vec!["refs/heads/develop"]
    );
    assert_eq!(
        probe_primary_tracking(&repository).expect("probe"),
        expected(true, Some("develop"))
    );

    // `master` outranks `develop` when both exist.
    fixture.git_ok(&["branch", "master", "HEAD"]);
    assert_eq!(
        names(&resolve_primary_refs(&repository).expect("resolve")),
        vec!["refs/heads/master"]
    );

    // `main` outranks both.
    fixture.git_ok(&["branch", "main", "HEAD"]);
    assert_eq!(
        names(&resolve_primary_refs(&repository).expect("resolve")),
        vec!["refs/heads/main"]
    );
}

#[test]
fn the_remote_tree_alone_can_satisfy_tracking() {
    let fixture = Fixture::new("main");
    let with_record = fixture.commit_file(RECORD, "{}\n");
    fixture.update_ref("refs/remotes/origin/main", &with_record);
    // Local main goes back to the commit without the record.
    fixture.git_ok(&["reset", "--hard", "HEAD~1"]);

    let repository = fixture.open();
    assert_eq!(
        names(&resolve_primary_refs(&repository).expect("resolve")),
        vec!["refs/heads/main", "refs/remotes/origin/main"]
    );
    assert!(
        primary_tracks_op_records(
            &repository,
            &resolve_primary_refs(&repository).expect("resolve")
        )
        .expect("tree probe"),
        "the origin tree holds the only Op record"
    );
    assert_eq!(
        probe_primary_tracking(&repository).expect("probe"),
        expected(true, Some("main"))
    );
}

#[test]
fn the_local_tree_alone_can_satisfy_tracking() {
    let fixture = Fixture::new("main");
    let without_record = fixture.head();
    fixture.commit_file(RECORD, "{}\n");
    fixture.update_ref("refs/remotes/origin/main", &without_record);

    let repository = fixture.open();
    assert_eq!(
        names(&resolve_primary_refs(&repository).expect("resolve")),
        vec!["refs/heads/main", "refs/remotes/origin/main"]
    );
    assert_eq!(
        probe_primary_tracking(&repository).expect("probe"),
        expected(true, Some("main"))
    );
}

#[test]
fn a_tracked_but_ignored_record_is_reported_ignored_first() {
    let fixture = Fixture::new("main");
    fixture.commit_file(RECORD, "{}\n");
    fs::write(fixture.root.join(".gitignore"), "kitty-ops/\n").expect("write ignore rules");
    let repository = fixture.open();
    let tracking = probe_primary_tracking(&repository).expect("probe");
    assert_eq!(tracking, expected(true, Some("main")));

    assert_eq!(
        op_commit_skip_reason(&repository, &path(RECORD), &tracking).expect("skip reason"),
        Some(format!(
            "{RECORD} matches an ignore rule in this repository"
        )),
        "the ignore check comes before the tracking check"
    );
    assert_eq!(
        op_commit_skip_reason(&repository, &path("README.md"), &tracking).expect("skip reason"),
        None
    );
}

#[test]
fn the_admission_sentinel_and_a_real_record_differ_under_an_exact_ignore_rule() {
    let fixture = Fixture::new("main");
    fixture.commit_file(RECORD, "{}\n");
    fs::write(fixture.root.join(".gitignore"), format!("{SENTINEL}\n"))
        .expect("write exact-path ignore rule");
    let repository = fixture.open();
    let tracking = probe_primary_tracking(&repository).expect("probe");

    assert_eq!(
        op_commit_skip_reason(&repository, &path(SENTINEL), &tracking).expect("skip reason"),
        Some(format!(
            "{SENTINEL} matches an ignore rule in this repository"
        )),
        "the admission probe path is skipped"
    );
    assert_eq!(
        op_commit_skip_reason(&repository, &path(RECORD), &tracking).expect("skip reason"),
        None,
        "the actual per-Op record is still commit-eligible"
    );
}

fn git_command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test Committer")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    for variable in GIT_SELECTORS {
        command.env_remove(variable);
    }
    command.env_remove("GIT_INDEX_FILE");
    command
}

fn snapshot_worktree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let entries = fs::read_dir(directory).expect("read directory");
        for entry in entries {
            let entry = entry.expect("read directory entry");
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            let metadata = entry.metadata().expect("stat directory entry");
            if metadata.is_dir() {
                walk(base, &path, files);
            } else if metadata.is_file() {
                files.insert(
                    path.strip_prefix(base)
                        .expect("path is under root")
                        .to_owned(),
                    fs::read(&path).expect("read worktree file"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

#[test]
fn the_probes_leave_head_index_staging_and_worktree_unchanged() {
    let fixture = Fixture::new("main");
    fixture.commit_file(RECORD, "{}\n");
    fs::write(fixture.root.join(".gitignore"), "kitty-ops/\n").expect("write ignore rules");
    fs::write(fixture.root.join("README.md"), b"# modified\n").expect("modify README");
    fs::write(fixture.root.join("untracked.txt"), b"untracked\n").expect("write untracked");

    let head_before = fixture.git_ok(&["rev-parse", "HEAD"]);
    let index_before = fs::read(fixture.root.join(".git/index")).expect("read index");
    let staged_before = fixture.git_ok(&["ls-files", "-s", "-z"]);
    let worktree_before = snapshot_worktree(&fixture.root);

    let repository = fixture.open();
    let tracking = probe_primary_tracking(&repository).expect("probe");
    let _ = resolve_primary_refs(&repository).expect("resolve");
    let _ = op_commit_skip_reason(&repository, &path(RECORD), &tracking).expect("skip reason");

    assert_eq!(
        fixture.git_ok(&["rev-parse", "HEAD"]),
        head_before,
        "HEAD must not change"
    );
    assert_eq!(
        fs::read(fixture.root.join(".git/index")).expect("read index"),
        index_before,
        "the index must not change"
    );
    assert_eq!(
        fixture.git_ok(&["ls-files", "-s", "-z"]),
        staged_before,
        "staging must not change"
    );
    assert_eq!(
        snapshot_worktree(&fixture.root),
        worktree_before,
        "no worktree byte may change"
    );
}

#[cfg(unix)]
fn write_fake_git(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("fake-git");
    fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write fake git");
    let mut permissions = fs::metadata(&path).expect("stat fake git").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("make fake git executable");
    path
}

#[cfg(unix)]
const FAKE_DISCOVERY: &str = r#"
case "$1" in
  rev-parse)
    case "$2" in
      --is-inside-work-tree) printf 'true\n'; exit 0 ;;
      --show-toplevel) pwd; exit 0 ;;
      --is-bare-repository) printf 'false\n'; exit 0 ;;
    esac
    ;;
esac
"#;

#[cfg(unix)]
fn fake_repository(
    temp: &tempfile::TempDir,
    body: &str,
    timeout: Duration,
) -> (PathBuf, GitRepository) {
    let program = write_fake_git(temp.path(), &format!("{FAKE_DISCOVERY}\n{body}"));
    let options = GitOptions {
        program: program.clone(),
        timeout,
    };
    let repository =
        GitRepository::open_with_options(temp.path(), &options).expect("open with fake git");
    (program, repository)
}

#[cfg(unix)]
fn recorded(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .expect("read recorded calls")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(unix)]
fn tree_calls(calls: &[String]) -> Vec<&String> {
    // `tree_paths_at` prefixes its argv with `--literal-pathspecs`.
    calls
        .iter()
        .filter(|line| line.starts_with("--literal-pathspecs ls-tree "))
        .collect()
}

#[cfg(unix)]
fn resolution_calls(calls: &[String]) -> Vec<&str> {
    calls
        .iter()
        .filter(|line| line.starts_with("symbolic-ref ") || line.starts_with("rev-parse --verify "))
        .map(String::as_str)
        .collect()
}

#[cfg(unix)]
#[test]
fn a_dangling_symbolic_name_costs_at_most_nine_resolution_calls() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let log = temp.path().join("calls");
    let body = format!(
        r#"
case "$1" in
  symbolic-ref) printf '%s\n' "$*" >> '{log}'; printf 'origin/topic\n'; exit 0 ;;
  rev-parse)
    case "$2" in
      --verify) printf '%s\n' "$*" >> '{log}'; exit 1 ;;
    esac ;;
  *)
    if [ "$2" = ls-tree ]; then printf '%s\n' "$*" >> '{log}'; exit 0; fi ;;
esac
exit 0
"#,
        log = log.display()
    );
    let (_program, repository) = fake_repository(&temp, &body, Duration::from_secs(2));

    let tracking = probe_primary_tracking(&repository).expect("probe");
    assert_eq!(tracking, expected(false, None));

    let calls = recorded(&log);
    let resolution = resolution_calls(&calls);
    assert_eq!(
        resolution,
        vec![
            "symbolic-ref --quiet --short refs/remotes/origin/HEAD",
            "rev-parse --verify --quiet --end-of-options refs/heads/topic^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/remotes/origin/topic^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/heads/main^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/remotes/origin/main^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/heads/master^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/remotes/origin/master^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/heads/develop^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/remotes/origin/develop^{commit}",
        ],
        "1 symbolic + 2 dangling-name + 6 ladder calls, in source order"
    );
    assert!(
        tree_calls(&calls).is_empty(),
        "an unresolved primary must not list any tree"
    );
}

#[cfg(unix)]
#[test]
fn the_already_probed_symbolic_name_is_skipped_in_the_ladder() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let log = temp.path().join("calls");
    let body = format!(
        r#"
case "$1" in
  symbolic-ref) printf '%s\n' "$*" >> '{log}'; printf 'origin/main\n'; exit 0 ;;
  rev-parse)
    case "$2" in
      --verify) printf '%s\n' "$*" >> '{log}'; exit 1 ;;
    esac ;;
  *)
    if [ "$2" = ls-tree ]; then printf '%s\n' "$*" >> '{log}'; exit 0; fi ;;
esac
exit 0
"#,
        log = log.display()
    );
    let (_program, repository) = fake_repository(&temp, &body, Duration::from_secs(2));

    assert_eq!(
        probe_primary_tracking(&repository).expect("probe"),
        expected(false, None)
    );
    let calls = recorded(&log);
    let resolution = resolution_calls(&calls);
    assert_eq!(
        resolution,
        vec![
            "symbolic-ref --quiet --short refs/remotes/origin/HEAD",
            "rev-parse --verify --quiet --end-of-options refs/heads/main^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/remotes/origin/main^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/heads/master^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/remotes/origin/master^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/heads/develop^{commit}",
            "rev-parse --verify --quiet --end-of-options refs/remotes/origin/develop^{commit}",
        ],
        "main is probed once as the symbolic name and never again in the ladder"
    );
}

#[cfg(unix)]
#[test]
fn resolution_precedes_and_bounds_the_tree_reads() {
    const OID: &str = "0123456789abcdef0123456789abcdef01234567";
    let temp = tempfile::tempdir().expect("create tempdir");
    let log = temp.path().join("calls");
    let body = format!(
        r#"
case "$1" in
  symbolic-ref) printf '%s\n' "$*" >> '{log}'; printf 'origin/main\n'; exit 0 ;;
  rev-parse)
    case "$2" in
      --verify)
        printf '%s\n' "$*" >> '{log}'
        case "$*" in
          *refs/heads/main*|*refs/remotes/origin/main*) printf '{oid}\n'; exit 0 ;;
        esac
        exit 1 ;;
    esac ;;
  *)
    if [ "$2" != ls-tree ]; then exit 0; fi
    printf '%s\n' "$*" >> '{log}'
    case "$*" in
      *refs/remotes/origin/main*) printf 'kitty-ops/01ARZ3NDEKTSV4RRFFQ69G5FAV.jsonl\0' ;;
      *) printf 'kitty-ops/op-closures.jsonl\0' ;;
    esac
    exit 0 ;;
esac
exit 0
"#,
        log = log.display(),
        oid = OID
    );
    let (_program, repository) = fake_repository(&temp, &body, Duration::from_secs(2));

    assert_eq!(
        probe_primary_tracking(&repository).expect("probe"),
        expected(true, Some("main"))
    );
    let calls = recorded(&log);
    assert_eq!(
        resolution_calls(&calls).len(),
        3,
        "one symbolic + the two refs of the named branch"
    );
    let trees = tree_calls(&calls);
    assert_eq!(trees.len(), 2, "one tree read per resolved ref, no more");
    assert!(trees[0].contains("refs/heads/main"), "{:?}", trees[0]);
    assert!(
        trees[1].contains("refs/remotes/origin/main"),
        "{:?}",
        trees[1]
    );
    assert!(
        calls
            .iter()
            .position(|line| line.starts_with("--literal-pathspecs ls-tree "))
            .expect("a tree read")
            > calls
                .iter()
                .rposition(|line| line.starts_with("rev-parse --verify "))
                .expect("resolution reads"),
        "every tree read happens after resolution"
    );
}

#[cfg(unix)]
const OID: &str = "0123456789abcdef0123456789abcdef01234567";

/// A fake repository whose primary resolves to `main` with a local ref, whose
/// ignore probe and tree listing are controlled by marker files.
#[cfg(unix)]
fn fake_primary_main(temp: &tempfile::TempDir, body: &str) -> (PathBuf, GitRepository) {
    let log = temp.path().join("calls");
    let script = format!(
        r#"
case "$1" in
  symbolic-ref) printf '%s\n' "$*" >> '{log}'; printf 'origin/main\n'; exit 0 ;;
  rev-parse)
    case "$2" in
      --verify)
        printf '%s\n' "$*" >> '{log}'
        case "$*" in
          *refs/heads/main*) printf '{oid}\n'; exit 0 ;;
        esac
        exit 1 ;;
    esac ;;
esac
{body}
exit 0
"#,
        log = log.display(),
        oid = OID,
        body = body
    );
    let (_program, repository) = fake_repository(temp, &script, Duration::from_secs(2));
    (log, repository)
}

#[cfg(unix)]
#[test]
fn a_failed_ignore_probe_keeps_the_successful_tracking_cache() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let fail_ignore = temp.path().join("fail-ignore");
    let log = temp.path().join("calls");
    let body = format!(
        r#"
if [ "$1" = check-ignore ]; then
  if [ -f '{fail}' ]; then printf 'ignore probe exploded\n' >&2; exit 3; fi
  exit 1
fi
if [ "$2" = ls-tree ]; then printf '%s\n' "$*" >> '{log}'; exit 0; fi
"#,
        fail = fail_ignore.display(),
        log = log.display()
    );
    let script = format!(
        r#"
case "$1" in
  symbolic-ref) printf '%s\n' "$*" >> '{log}'; printf 'origin/main\n'; exit 0 ;;
  rev-parse)
    case "$2" in
      --verify)
        printf '%s\n' "$*" >> '{log}'
        case "$*" in
          *refs/heads/main*) printf '{oid}\n'; exit 0 ;;
        esac
        exit 1 ;;
    esac ;;
esac
{body}
exit 0
"#,
        log = log.display(),
        oid = OID,
        body = body
    );
    let (_program, repository) = fake_repository(&temp, &script, Duration::from_secs(2));
    let mut probe = OpCommitSkipProbe::new(&repository);

    // First decision: tracking resolves (no Op records) and the path is not
    // ignored, so the source's named-primary skip reason applies.
    assert_eq!(
        probe.skip_reason(&path(RECORD)),
        Some("the primary branch (main) tracks no Op records under kitty-ops/".to_string())
    );
    assert_eq!(probe.tracking(), Some(&expected(false, Some("main"))));
    let cached_calls = recorded(&log);

    // A later ignore failure becomes the best-effort reason, not a commit and
    // not a refusal; the tracking cache must survive it.
    fs::write(&fail_ignore, b"fail\n").expect("write control marker");
    let reason = probe
        .skip_reason(&path(RECORD))
        .expect("best-effort reason");
    assert!(
        reason.starts_with("git probe failed while deciding whether to commit the Op record:"),
        "{reason:?}"
    );
    assert!(reason.contains("check-ignore"), "{reason:?}");
    assert_eq!(
        probe.tracking(),
        Some(&expected(false, Some("main"))),
        "a later ignore failure must not discard the successful tracking cache"
    );
    assert_eq!(
        recorded(&log),
        cached_calls,
        "the cached primary must not be probed again"
    );

    // With the ignore probe healthy again the cached named-primary reason
    // returns; no new resolution or tree read happened.
    fs::remove_file(&fail_ignore).expect("remove control marker");
    assert_eq!(
        probe.skip_reason(&path(RECORD)),
        Some("the primary branch (main) tracks no Op records under kitty-ops/".to_string())
    );
    assert_eq!(recorded(&log), cached_calls);
}

#[cfg(unix)]
#[test]
fn a_failed_tracking_probe_is_not_cached_and_is_retried() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let fail_tree = temp.path().join("fail-tree");
    let log = temp.path().join("calls");
    let body = format!(
        r#"
if [ "$2" = ls-tree ]; then
  printf '%s\n' "$*" >> '{log}'
  if [ -f '{fail}' ]; then printf 'tree read exploded\n' >&2; exit 2; fi
  exit 0
fi
if [ "$1" = check-ignore ]; then exit 1; fi
"#,
        fail = fail_tree.display(),
        log = log.display()
    );
    let script = format!(
        r#"
case "$1" in
  symbolic-ref) printf '%s\n' "$*" >> '{log}'; printf 'origin/main\n'; exit 0 ;;
  rev-parse)
    case "$2" in
      --verify)
        printf '%s\n' "$*" >> '{log}'
        case "$*" in
          *refs/heads/main*) printf '{oid}\n'; exit 0 ;;
        esac
        exit 1 ;;
    esac ;;
esac
{body}
exit 0
"#,
        log = log.display(),
        oid = OID,
        body = body
    );
    let (_program, repository) = fake_repository(&temp, &script, Duration::from_secs(2));
    let mut probe = OpCommitSkipProbe::new(&repository);

    fs::write(&fail_tree, b"fail\n").expect("write control marker");
    let reason = probe
        .skip_reason(&path(RECORD))
        .expect("best-effort reason");
    assert!(
        reason.starts_with("git probe failed while deciding whether to commit the Op record:"),
        "{reason:?}"
    );
    assert!(reason.contains("listing kitty-ops/"), "{reason:?}");
    assert_eq!(probe.tracking(), None, "failed tracking is not cached");
    let failed_calls = recorded(&log);

    fs::remove_file(&fail_tree).expect("remove control marker");
    assert_eq!(
        probe.skip_reason(&path(RECORD)),
        Some("the primary branch (main) tracks no Op records under kitty-ops/".to_string()),
        "the next decision retries the primary probe"
    );
    assert_eq!(probe.tracking(), Some(&expected(false, Some("main"))));
    let retried_calls = recorded(&log);
    assert_eq!(
        retried_calls.len(),
        failed_calls.len() + 4,
        "the retry repeated the symbolic and two ref probes and listed the tree once"
    );
    assert_eq!(
        probe.skip_reason(&path(RECORD)),
        Some("the primary branch (main) tracks no Op records under kitty-ops/".to_string())
    );
    assert_eq!(
        recorded(&log),
        retried_calls,
        "a cached successful tracking decision is not re-probed"
    );
}

#[cfg(unix)]
#[test]
fn a_timeout_during_primary_resolution_is_a_typed_probe_error() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let hang = temp.path().join("hang");
    let log = temp.path().join("calls");
    let script = format!(
        r#"
case "$1" in
  symbolic-ref)
    printf '%s\n' "$*" >> '{log}'
    if [ -f '{hang}' ]; then exec sleep 30; fi
    exit 1 ;;
  rev-parse)
    case "$2" in
      --verify) printf '%s\n' "$*" >> '{log}'; exit 1 ;;
    esac ;;
esac
if [ "$1" = check-ignore ]; then exit 1; fi
exit 0
"#,
        hang = hang.display(),
        log = log.display()
    );
    let (_program, repository) = fake_repository(&temp, &script, Duration::from_millis(120));
    let mut probe = OpCommitSkipProbe::new(&repository);

    fs::write(&hang, b"hang\n").expect("write control marker");
    match probe_primary_tracking(&repository) {
        Err(OpCommitProbeError::SymbolicRef {
            source: GitError::Timeout { .. },
        }) => {}
        other => panic!("expected a typed symbolic-ref timeout, got {other:?}"),
    }
    let reason = probe
        .skip_reason(&path(RECORD))
        .expect("best-effort reason");
    assert!(
        reason.starts_with("git probe failed while deciding whether to commit the Op record:"),
        "{reason:?}"
    );
    assert_eq!(probe.tracking(), None);

    fs::remove_file(&hang).expect("remove control marker");
    assert_eq!(
        probe.skip_reason(&path(RECORD)),
        Some("primary branch unresolved, so it cannot be shown to track Op records".to_string()),
        "the failed primary probe was not cached"
    );
}

#[cfg(unix)]
#[test]
fn a_signalled_ref_answer_is_false_not_a_probe_error() {
    // The reference treats every *completed* non-zero ref answer as "absent",
    // including a signalled child; only never-started/timeout/capture failures
    // raise. The generic read is strict, the Op policy is tolerant.
    let temp = tempfile::tempdir().expect("create tempdir");
    let script = r#"
case "$1" in
  symbolic-ref) printf 'origin/topic\n'; exit 0 ;;
  rev-parse)
    case "$2" in
      --verify) kill -TERM $$ ;;
    esac ;;
esac
exit 0
"#;
    let (_program, repository) = fake_repository(&temp, script, Duration::from_secs(2));

    assert_eq!(
        resolve_primary_refs(&repository).expect("signalled refs are absent"),
        Vec::<RefName>::new()
    );
    assert_eq!(
        probe_primary_tracking(&repository).expect("probe"),
        expected(false, None)
    );
}

#[cfg(unix)]
#[test]
fn a_timeout_while_probing_ref_existence_is_a_typed_probe_error() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let hang = temp.path().join("hang");
    let script = format!(
        r#"
case "$1" in
  symbolic-ref) printf 'origin/main\n'; exit 0 ;;
  rev-parse)
    case "$2" in
      --verify)
        if [ -f '{hang}' ]; then exec sleep 30; fi
        exit 1 ;;
    esac ;;
esac
exit 0
"#,
        hang = hang.display()
    );
    let (_program, repository) = fake_repository(&temp, &script, Duration::from_millis(120));

    fs::write(&hang, b"hang\n").expect("write control marker");
    match resolve_primary_refs(&repository) {
        Err(OpCommitProbeError::RefExistence {
            source: GitError::Timeout { .. },
            ..
        }) => {}
        other => panic!("expected a typed ref-existence timeout, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn an_ignore_signal_and_capture_failure_are_typed_probe_errors() {
    for body in [
        "if [ \"$1\" = check-ignore ]; then kill -TERM $$; fi\nexit 0\n",
        "if [ \"$1\" = check-ignore ]; then head -c 16777217 /dev/zero; exit 1; fi\nexit 0\n",
    ] {
        let temp = tempfile::tempdir().expect("create tempdir");
        let (_log, repository) = fake_primary_main(&temp, body);
        let tracking = expected(true, Some("main"));
        match op_commit_skip_reason(&repository, &path(RECORD), &tracking) {
            Err(OpCommitProbeError::IgnoreProbe { path, .. }) => assert_eq!(path, RECORD),
            other => panic!("expected a typed ignore-probe error, got {other:?}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn a_non_1_ignore_exit_is_a_typed_probe_error() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let (_log, repository) = fake_primary_main(
        &temp,
        "if [ \"$1\" = check-ignore ]; then exit 3; fi\nexit 0\n",
    );
    match op_commit_skip_reason(&repository, &path(RECORD), &expected(true, Some("main"))) {
        Err(OpCommitProbeError::IgnoreProbe { source, .. }) => {
            assert!(matches!(
                source,
                GitError::CommandFailed {
                    status: Some(3),
                    ..
                }
            ));
        }
        other => panic!("expected IgnoreProbe, got {other:?}"),
    }
}

#[test]
fn full_values_match_the_pinned_python_source() {
    // Fixture A: a local main with a record, observed from a topic branch.
    let local = Fixture::new("main");
    local.commit_file(RECORD, "{}\n");
    local.checkout("fix/x");

    // Fixture B: the record is only on origin/main.
    let remote = Fixture::new("main");
    let with_record = remote.commit_file(RECORD, "{}\n");
    remote.update_ref("refs/remotes/origin/main", &with_record);
    remote.git_ok(&["reset", "--hard", "HEAD~1"]);

    // Fixture C: a dangling origin/HEAD falls through to main.
    let dangling = Fixture::new("main");
    dangling.commit_file(RECORD, "{}\n");
    dangling.symbolic_ref("refs/remotes/origin/HEAD", "refs/remotes/origin/gone");

    // Fixture D: a tracked-but-ignored record.
    let ignored = Fixture::new("main");
    ignored.commit_file(RECORD, "{}\n");
    fs::write(ignored.root.join(".gitignore"), "kitty-ops/\n").expect("write ignore rules");

    // Fixture E: no primary at all.
    let unresolved = Fixture::new("trunk");
    unresolved.checkout("fix/x");
    unresolved.commit_file(RECORD, "{}\n");

    // Fixture F: a resolved primary that tracks only the closure spine.
    let spine_only = Fixture::new("main");
    spine_only.commit_file(SPINE, "{}\n");

    // Fixture G: an exact-path ignore rule for the sentinel probe only.
    let sentinel_ignored = Fixture::new("main");
    sentinel_ignored.commit_file(RECORD, "{}\n");
    fs::write(
        sentinel_ignored.root.join(".gitignore"),
        format!("{SENTINEL}\n"),
    )
    .expect("write exact-path ignore rule");

    let cases: Vec<(&Fixture, Vec<&str>)> = vec![
        (&local, vec![RECORD, SPINE, SENTINEL, "README.md"]),
        (&remote, vec![RECORD]),
        (&dangling, vec![RECORD]),
        (&ignored, vec![RECORD, "README.md"]),
        (&unresolved, vec![RECORD]),
        (&spine_only, vec![RECORD]),
        (&sentinel_ignored, vec![SENTINEL, RECORD]),
    ];

    let script = r#"
import sys
from pathlib import Path
from specify_cli.invocation.op_commit_policy import (
    op_commit_skip_reason,
    probe_primary_tracking,
    resolve_primary_refs,
)

lines = [line.rstrip("\n") for line in sys.stdin]
root = Path(lines[0]).resolve()
paths = lines[1:]
refs = resolve_primary_refs(root)
tracking = probe_primary_tracking(root)
print("refs=" + ",".join(refs))
print("tracked=" + ("true" if tracking.tracked else "false"))
print("primary=" + ("" if tracking.primary is None else tracking.primary))
for rel in paths:
    reason = op_commit_skip_reason(root, rel, tracking=tracking)
    print("reason=" + rel + "\t" + ("" if reason is None else reason))
"#;

    for (fixture, paths) in cases {
        let mut stdin = fixture.root.display().to_string();
        for rel in &paths {
            stdin.push('\n');
            stdin.push_str(rel);
        }
        let oracle = support::run_python(script, Some(&stdin));
        let repository = fixture.open();
        let refs = resolve_primary_refs(&repository).expect("resolve");
        let tracking = probe_primary_tracking(&repository).expect("probe");
        let mut native = String::new();
        native.push_str(&format!("refs={}\n", names(&refs).join(",")));
        native.push_str(&format!(
            "tracked={}\n",
            if tracking.tracked() { "true" } else { "false" }
        ));
        native.push_str(&format!(
            "primary={}\n",
            tracking.primary().unwrap_or_default()
        ));
        for rel in paths {
            let reason =
                op_commit_skip_reason(&repository, &path(rel), &tracking).expect("skip reason");
            native.push_str(&format!("reason={rel}\t{}\n", reason.unwrap_or_default()));
        }
        assert_eq!(
            native.trim_end(),
            oracle.trim_end(),
            "native full values disagree for {}",
            fixture.root.display()
        );
    }
}

#[cfg(unix)]
fn run_captured_index_child(root: &Path, index: &Path) -> String {
    let mut command = Command::new(std::env::current_exe().expect("current test executable"));
    command
        .args([
            "--exact",
            "captured_index_commit_probe_child",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(root)
        .env("SK_COMMIT_PROBE_ROOT", root)
        .env("GIT_INDEX_FILE", index);
    for variable in GIT_SELECTORS {
        command.env_remove(variable);
    }
    let output = command.output().expect("run captured-index child");
    assert!(
        output.status.success(),
        "captured-index child failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("child output is UTF-8")
}

#[cfg(unix)]
#[test]
fn a_captured_alternate_index_stays_bound_and_the_probes_write_nothing() {
    let fixture = Fixture::new("main");
    fixture.commit_file(RECORD, "{}\n");
    fs::write(fixture.root.join(".gitignore"), "kitty-ops/\n").expect("write ignore rules");

    let custom = fixture.temp.path().join("custom-index");
    let mut command = git_command(&fixture.root);
    command
        .env("GIT_INDEX_FILE", &custom)
        .args(["read-tree", "HEAD"]);
    let output = command.output().expect("build custom index");
    assert!(
        output.status.success(),
        "git read-tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let custom_before = fs::read(&custom).expect("read custom index");
    let default_index = fixture.root.join(".git/index");
    let default_before = fs::read(&default_index).expect("read default index");
    let head_before = fixture.git_ok(&["rev-parse", "HEAD"]);
    let staged_before = fixture.git_ok(&["ls-files", "-s", "-z"]);
    let worktree_before = snapshot_worktree(&fixture.root);

    let stdout = run_captured_index_child(&fixture.root, &custom);
    let field = |name: &str| {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("child did not print {name}:\n{stdout}"))
            .to_string()
    };
    assert_eq!(field("captured"), "true");
    assert_eq!(field("tracked"), "true");
    assert_eq!(field("primary"), "main");
    assert_eq!(
        field("reason"),
        format!("{RECORD} matches an ignore rule in this repository")
    );

    assert_eq!(
        fs::read(&custom).expect("read custom index"),
        custom_before,
        "the captured alternate index must not change"
    );
    assert_eq!(
        fs::read(&default_index).expect("read default index"),
        default_before
    );
    assert_eq!(fixture.git_ok(&["rev-parse", "HEAD"]), head_before);
    assert_eq!(fixture.git_ok(&["ls-files", "-s", "-z"]), staged_before);
    assert_eq!(snapshot_worktree(&fixture.root), worktree_before);
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess helper; the parent supplies the captured environment"]
fn captured_index_commit_probe_child() {
    let root =
        PathBuf::from(std::env::var_os("SK_COMMIT_PROBE_ROOT").expect("SK_COMMIT_PROBE_ROOT"));
    let repository = GitRepository::open(&root).expect("open repository");
    println!("captured={}", repository.has_captured_index());
    let tracking = probe_primary_tracking(&repository).expect("probe");
    println!(
        "tracked={}",
        if tracking.tracked() { "true" } else { "false" }
    );
    println!("primary={}", tracking.primary().unwrap_or_default());
    println!(
        "reason={}",
        op_commit_skip_reason(&repository, &path(RECORD), &tracking)
            .expect("skip reason")
            .unwrap_or_default()
    );
}
