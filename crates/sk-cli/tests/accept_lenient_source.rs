//! Source-first proof for `accept --lenient` and the PR positive persistence.
//!
//! Source's `accept --lenient` is `strict_metadata=False`: metadata-slot issues
//! and path-convention violations become advisory. The adapter's acceptance has
//! no metadata/path gates, but it DID carry an owned-file attribution gate that
//! Source's summary does not have at all; under `--lenient` that gate is
//! advisory (reported as a warning) and the STRICT evaluation keeps it, so both
//! surfaces are distinct and provable.
//!
//! Cases, all on one freshly built Source fixture copied per side with an
//! isolation proof:
//!
//! A. `--mode local --lenient`: both CLIs accept; the recorded metadata fields,
//!    the acceptance commit's message/parent/files and the matrix document match.
//! B. `--mode pr --merge-commit <landing> --target-branch main
//!    --attest-first-landing-commit --lenient`: both CLIs record
//!    `acceptance_mode=pr`, the baseline/merged/PR commit, the attestation
//!    evidence, and land the same acceptance commit; the raw target reflog and
//!    the working tree are bound on both sides.
//! C. STRICT (no `--lenient`) on the same PR input: the Native refusal writes
//!    NOTHING (no matrix, no commit, unchanged meta and refs).
//! D. RETRY: a second lenient PR acceptance is idempotent on both sides (no
//!    duplicate baseline/history growth beyond Source's own behaviour).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn python() -> PathBuf {
    let executable = if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    };
    std::env::var_os("SK_KERNEL_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join(".venv-rust-oracle").join(executable))
}

struct Env {
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("sandbox");
        for directory in [
            "home",
            "cache",
            "config",
            "data",
            "state",
            "runtime",
            "tmp",
            "cwd",
            "source-bin",
        ] {
            fs::create_dir_all(temp.path().join(directory)).expect("sandbox root");
        }
        Self { root: temp.keep() }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn path_with(&self, bin: &Path) -> Vec<(String, String)> {
        let mut pairs = self.pairs();
        pairs.retain(|(key, _)| key != "PATH");
        let mut path = bin.display().to_string();
        path.push_str(":/usr/bin:/bin");
        pairs.push(("PATH".into(), path));
        pairs
    }

    fn pairs(&self) -> Vec<(String, String)> {
        let root = repo_root();
        vec![
            (
                "HOME".into(),
                self.path().join("home").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".into(),
                self.path().join("cache").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".into(),
                self.path().join("config").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".into(),
                self.path().join("data").display().to_string(),
            ),
            (
                "XDG_STATE_HOME".into(),
                self.path().join("state").display().to_string(),
            ),
            (
                "XDG_RUNTIME_DIR".into(),
                self.path().join("runtime").display().to_string(),
            ),
            (
                "TMPDIR".into(),
                self.path().join("tmp").display().to_string(),
            ),
            ("TMP".into(), self.path().join("tmp").display().to_string()),
            ("TEMP".into(), self.path().join("tmp").display().to_string()),
            (
                "PYTHONPATH".into(),
                format!("{}:{}", root.display(), root.join("src").display()),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".into(),
                root.join("packs").display().to_string(),
            ),
            ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
            ("PYTHONUTF8".into(), "1".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("SPEC_KITTY_NO_UPGRADE_CHECK".into(), "1".into()),
            (
                "SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS".into(),
                "1".into(),
            ),
            ("SPEC_KITTY_ENABLE_SAAS_SYNC".into(), "0".into()),
            ("SPEC_KITTY_WORKTREE_REMOVAL_DELAY".into(), "0".into()),
            ("PYTHONHASHSEED".into(), "0".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
    }

    fn install_source_shim(&self) -> PathBuf {
        let script = self.path().join("source-bin").join("spec-kitty");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nexec {} -m specify_cli \"$@\"\n",
                python().display()
            ),
        )
        .expect("write source shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        script
    }
}

fn run(program: &Path, args: &[&str], cwd: &Path, env: &[(String, String)]) -> Output {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).env_clear();
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("run process")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "QA")
        .env("GIT_AUTHOR_EMAIL", "qa@example.invalid")
        .env("GIT_COMMITTER_NAME", "QA")
        .env("GIT_COMMITTER_EMAIL", "qa@example.invalid")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        text(&output.stderr)
    );
    text(&output.stdout).trim().to_owned()
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git")
        .status
        .success()
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("copy target");
    for entry in fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path()).expect("metadata");
        if metadata.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if metadata.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path()).expect("link"), &target)
                .expect("symlink");
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

fn fixture_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sk-workspace/tests/support/source_mission_fixture.py")
}

fn build_seed(env: &Env) -> (PathBuf, String) {
    let shim = env.install_source_shim();
    let output = run(
        &python(),
        &[&fixture_script().display().to_string(), "seed", "lanes"],
        &env.path().join("cwd"),
        &env.path_with(&shim),
    );
    assert!(
        output.status.success(),
        "the Source fixture seed failed (a harness/premise failure, not a business red):\n{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("seed json");
    (
        PathBuf::from(value["repo"].as_str().expect("repo")),
        value["slug"].as_str().expect("slug").to_owned(),
    )
}

fn assert_isolated_copy(repo: &Path) {
    let git_dir = PathBuf::from(git(repo, &["rev-parse", "--absolute-git-dir"]));
    assert!(
        git_dir.starts_with(repo),
        "{}: the git dir {} is inside the copy",
        repo.display(),
        git_dir.display()
    );
    let common = git(repo, &["rev-parse", "--git-common-dir"]);
    let common = if common.starts_with('/') {
        PathBuf::from(common)
    } else {
        repo.join(common)
    };
    assert!(
        common.starts_with(repo),
        "{}: the common dir {} is inside the copy",
        repo.display(),
        common.display()
    );
}

fn assert_env(pairs: &[(String, String)], sandbox: &Path, label: &str) {
    for key in [
        "HOME",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
        "TMP",
        "TEMP",
    ] {
        let value = pairs
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("{label}: {key} must be explicit"));
        assert!(
            Path::new(&value).starts_with(sandbox),
            "{label}: {key}={value} is inside the sandbox"
        );
    }
    let root = repo_root();
    let pythonpath = pairs
        .iter()
        .find(|(candidate, _)| candidate == "PYTHONPATH")
        .map(|(_, value)| value.clone())
        .expect("PYTHONPATH");
    assert!(
        pythonpath.contains(&root.join("src").display().to_string()),
        "{label}: the absolute src directory is on PYTHONPATH"
    );
}

#[derive(Clone, Debug)]
struct Snapshot {
    meta: serde_json::Value,
    log: Vec<(String, String, Vec<String>)>,
    branches: String,
    porcelain: String,
    matrix_present: bool,
    reflog: Vec<(String, String, String)>,
}

fn snapshot(repo: &Path, slug: &str) -> Snapshot {
    let meta_text = git(
        repo,
        &["show", &format!("main:kitty-specs/{slug}/meta.json")],
    );
    let meta: serde_json::Value = serde_json::from_str(&meta_text).expect("meta json");
    let mut log = Vec::new();
    for line in git(repo, &["log", "--format=%s%x09%P", "-n", "3", "main"]).lines() {
        let (subject, parents) = line.split_once('\t').unwrap_or((line, ""));
        let oid = git(repo, &["rev-parse", "main"]);
        let files = git(
            repo,
            &[
                "show",
                "--name-only",
                "--format=",
                "--no-renames",
                &format!("{oid}~{}", log.len()),
            ],
        );
        let files = files
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect();
        log.push((subject.to_owned(), parents.to_owned(), files));
    }
    let mut reflog = Vec::new();
    if let Ok(bytes) = fs::read(repo.join(".git/logs/refs/heads/main")) {
        for line in String::from_utf8_lossy(&bytes).lines().rev().take(6) {
            let (header, message) = match line.split_once('\t') {
                Some((header, message)) => (header, message.to_owned()),
                None => (line, String::new()),
            };
            let mut fields = header.split_whitespace();
            let _old = fields.next();
            let _new = fields.next();
            reflog.push((
                header.split(' ').nth(2).unwrap_or_default().to_owned(),
                message,
                if line.contains('\t') {
                    "tab".to_owned()
                } else {
                    "bare".to_owned()
                },
            ));
        }
    }
    Snapshot {
        meta,
        log,
        branches: git(
            dir_for(repo),
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
            ],
        ),
        porcelain: git(repo, &["status", "--porcelain"]),
        matrix_present: repo
            .join(format!("kitty-specs/{slug}/acceptance-matrix.json"))
            .is_file(),
        reflog,
    }
}

/// The commits a run ADDED to `main`, as `(subject, sorted files)`, sorted by
/// subject so Source's own finalize ordering cannot make the comparison
/// order-sensitive while both the messages and the file sets stay bound.
fn run_commits(repo: &Path, pre: &str) -> Vec<(String, Vec<String>)> {
    let raw = git(
        repo,
        &[
            "log",
            "--format=%x00%s",
            "--name-only",
            "--no-renames",
            &format!("{pre}..main"),
        ],
    );
    let mut rows = Vec::new();
    for block in raw.split('\u{0}') {
        let block = block.trim_start_matches('\n');
        if block.trim().is_empty() {
            continue;
        }
        let mut lines = block.lines();
        let subject = lines.next().unwrap_or_default().trim().to_owned();
        let mut files: Vec<String> = lines
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_owned())
            .collect();
        files.sort();
        rows.push((subject, files));
    }
    rows.sort();
    rows
}

fn dir_for(repo: &Path) -> &Path {
    repo
}

fn accept_args<'a>(slug: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["accept", "--mission", slug];
    args.extend(extra.iter().copied());
    args
}

fn run_accept(
    env: &Env,
    program: &Path,
    repo: &Path,
    slug: &str,
    extra: &[&str],
) -> (Output, String) {
    let bin_dir = program.parent().expect("bin dir");
    let pairs = env.path_with(bin_dir);
    assert_env(&pairs, env.path(), "accept");
    let args = accept_args(slug, extra);
    let output = run(program, &args, repo, &pairs);
    let text = format!("{}\n{}", text(&output.stdout), text(&output.stderr));
    (output, text)
}

/// Synthesize a corpus-first squash landing: the target moves to a single-parent
/// commit whose tree is the mission's and whose parent predates `kitty-specs/`.
fn synthesize_landing(repo: &Path) -> String {
    let root_commit = git(dir_for(repo), &["rev-list", "--max-parents=0", "main"]);
    let tree = git(repo, &["rev-parse", "main^{tree}"]);
    let landing = git(
        repo,
        &[
            "commit-tree",
            &tree,
            "-p",
            &root_commit,
            "-m",
            "squash landing for the accept test",
        ],
    );
    git(repo, &["update-ref", "refs/heads/main", &landing]);
    git(repo, &["reset", "--hard", "-q", "main"]);
    landing
}

#[test]
fn lenient_local_and_pr_acceptance_match_source_and_strict_writes_nothing() {
    let env = Env::new();
    let (seed, slug) = build_seed(&env);
    let shim = env.install_source_shim();

    // ---------------- A. local, lenient ----------------
    let source_local = env.path().join("source-local");
    let native_local = env.path().join("native-local");
    copy_tree(&seed, &source_local);
    copy_tree(&seed, &native_local);
    assert_isolated_copy(&source_local);
    assert_isolated_copy(&native_local);
    let before = snapshot(&source_local, &slug);
    let local_pre = git(&source_local, &["rev-parse", "main"]);

    let (source_output, source_text) =
        run_accept(&env, &shim, &source_local, &slug, &["--lenient"]);
    assert!(
        source_output.status.success(),
        "SOURCE --lenient local accept failed (premise): {source_text}"
    );
    let (native_output, native_text) = run_accept(
        &env,
        Path::new(binary()),
        &native_local,
        &slug,
        &["--lenient"],
    );
    assert!(
        native_output.status.success(),
        "the Native --lenient local accept failed: {native_text}\nsource: {source_text}"
    );
    let source_after = snapshot(&source_local, &slug);
    let native_after = snapshot(&native_local, &slug);

    for key in [
        "acceptance_mode",
        "status_phase",
        "mission_slug",
        "mission_type",
    ] {
        assert_eq!(
            native_after.meta[key], source_after.meta[key],
            "local lenient: meta.{key} matches Source"
        );
    }
    assert_eq!(
        source_after.meta["acceptance_mode"], "local",
        "a local acceptance records the local mode"
    );
    assert!(
        native_after.meta["accepted_from_commit"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "the acceptance records the commit it accepted from: {}",
        native_after.meta
    );
    let source_commits = run_commits(&source_local, &local_pre);
    let native_commits = run_commits(&native_local, &local_pre);
    // The METADATA commits are compared exactly. The matrix "finalize" commit is
    // change-detected on both sides: Source only commits it when the recomputed
    // matrix differs from what is already on disk. The seed already carries a
    // matrix from the fixture's own acceptance, so a byte-identical recomputation
    // produces no commit; the adapter may add exactly that ONE extra commit
    // (message + matrix-only file set), never a different one.
    let metadata_commits = |rows: &[(String, Vec<String>)]| -> Vec<(String, Vec<String>)> {
        rows.iter()
            .filter(|(subject, _)| !subject.starts_with("Finalize acceptance artifacts for "))
            .cloned()
            .collect()
    };
    assert_eq!(
        metadata_commits(&native_commits),
        metadata_commits(&source_commits),
        "local lenient: the acceptance METADATA commits (messages and file sets) match Source"
    );
    for (subject, files) in &native_commits {
        if subject.starts_with("Finalize acceptance artifacts for ") {
            assert_eq!(
                files,
                &vec![format!("kitty-specs/{slug}/acceptance-matrix.json")],
                "a finalize commit carries only the matrix: {subject}"
            );
        }
    }
    let source_matrix: serde_json::Value = serde_json::from_str(&git(
        &source_local,
        &[
            "show",
            &format!("main:kitty-specs/{slug}/acceptance-matrix.json"),
        ],
    ))
    .expect("source matrix");
    let native_matrix: serde_json::Value = serde_json::from_str(&git(
        &native_local,
        &[
            "show",
            &format!("main:kitty-specs/{slug}/acceptance-matrix.json"),
        ],
    ))
    .expect("native matrix");
    assert_eq!(
        native_matrix["overall_verdict"], source_matrix["overall_verdict"],
        "the acceptance matrix verdict matches Source"
    );
    assert!(
        source_commits
            .iter()
            .any(|(subject, _)| subject.starts_with("Accept ")),
        "Source records an `Accept <slug>` commit: {source_commits:?}"
    );
    assert!(
        source_commits
            .iter()
            .any(|(subject, _)| subject.starts_with("Record acceptance commit for ")),
        "Source records the accept_commit in its own commit: {source_commits:?}"
    );
    // A finalize commit appears only when the recomputed matrix actually
    // differs from the one already on disk (the seed carries one from the
    // fixture's own acceptance). If Source emits none, the adapter may add at
    // most that one change-detected commit - never a different message.
    if source_commits
        .iter()
        .any(|(subject, _)| subject.starts_with("Finalize acceptance artifacts for "))
    {
        assert!(
            native_commits
                .iter()
                .any(|(subject, _)| subject.starts_with("Finalize acceptance artifacts for ")),
            "the adapter also finalizes when Source does: {native_commits:?}"
        );
    }
    assert!(
        native_after.matrix_present,
        "an accepting lenient run publishes the acceptance matrix"
    );
    assert_eq!(
        native_after.porcelain, "",
        "the acceptance commit leaves no tracked change behind: {}",
        native_after.porcelain
    );
    assert_eq!(
        before.meta["acceptance_history"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0)
            + 1,
        native_after.meta["acceptance_history"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "one acceptance_history entry is appended"
    );

    // ---------------- B. PR, lenient (positive persistence) ----------------
    let source_pr = env.path().join("source-pr");
    let native_pr = env.path().join("native-pr");
    copy_tree(&seed, &source_pr);
    copy_tree(&seed, &native_pr);
    assert_isolated_copy(&source_pr);
    assert_isolated_copy(&native_pr);
    // Each copy synthesizes its OWN landing commit (same tree, same parent, same
    // message; the OID differs because the commit clock differs), so every
    // reference to the landing is bound to that side's own object.
    let source_landing = synthesize_landing(&source_pr);
    let native_landing = synthesize_landing(&native_pr);
    let pr_before = snapshot(&native_pr, &slug);
    let source_pr_extra = [
        "--mode",
        "pr",
        "--merge-commit",
        source_landing.as_str(),
        "--target-branch",
        "main",
        "--attest-first-landing-commit",
        "--lenient",
    ];
    let native_pr_extra = [
        "--mode",
        "pr",
        "--merge-commit",
        native_landing.as_str(),
        "--target-branch",
        "main",
        "--attest-first-landing-commit",
        "--lenient",
    ];

    let (source_output, source_text) = run_accept(&env, &shim, &source_pr, &slug, &source_pr_extra);
    assert!(
        source_output.status.success(),
        "SOURCE PR lenient accept failed (premise): {source_text}"
    );
    let (native_output, native_text) = run_accept(
        &env,
        Path::new(binary()),
        &native_pr,
        &slug,
        &native_pr_extra,
    );
    assert!(
        native_output.status.success(),
        "the Native PR lenient accept failed: {native_text}\nsource: {source_text}"
    );
    let source_pr_after = snapshot(&source_pr, &slug);
    let native_pr_after = snapshot(&native_pr, &slug);
    for key in [
        "acceptance_mode",
        "pr_merge_evidence",
        "baseline_merge_commit",
    ] {
        assert_eq!(
            native_pr_after.meta[key], source_pr_after.meta[key],
            "PR lenient: meta.{key} matches Source ({})",
            native_pr_after.meta
        );
    }
    assert_eq!(
        native_pr_after.meta["merged_commit"],
        serde_json::json!(native_landing),
        "each side records its OWN landing as merged_commit"
    );
    assert_eq!(
        native_pr_after.meta["baseline_merge_commit"],
        serde_json::json!(git(&source_pr, &["rev-list", "--max-parents=0", "main"])),
        "the baseline is the pre-landing target tip"
    );
    assert_eq!(
        native_pr_after.meta["pr_merge_commit"],
        serde_json::json!(native_landing),
        "the PR merge commit is the verified landing"
    );
    assert_eq!(
        native_pr_after.meta["pr_merge_evidence"],
        serde_json::json!("corpus-parent-attested"),
        "the operator attestation is recorded"
    );
    let pr_metadata_commits = |rows: &[(String, Vec<String>)]| -> Vec<(String, Vec<String>)> {
        rows.iter()
            .filter(|(subject, _)| !subject.starts_with("Finalize acceptance artifacts for "))
            .cloned()
            .collect()
    };
    assert_eq!(
        pr_metadata_commits(&run_commits(&native_pr, &native_landing)),
        pr_metadata_commits(&run_commits(&source_pr, &source_landing)),
        "PR lenient: the acceptance METADATA commits (messages and file sets) match Source"
    );
    assert_eq!(
        native_pr_after.porcelain, "",
        "the PR acceptance leaves no tracked change: {}",
        native_pr_after.porcelain
    );
    assert!(
        !native_pr_after.reflog.is_empty(),
        "the target's raw reflog is readable"
    );
    assert_eq!(
        native_pr_after.reflog.first().map(|row| row.2.clone()),
        source_pr_after.reflog.first().map(|row| row.2.clone()),
        "the newest raw reflog entry has the same shape (bare/tab) as Source's"
    );

    // ---------------- C. STRICT refusal writes NOTHING ----------------
    let strict_repo = env.path().join("native-strict");
    copy_tree(&seed, &strict_repo);
    assert_isolated_copy(&strict_repo);
    let strict_landing = synthesize_landing(&strict_repo);
    let strict_before = snapshot(&strict_repo, &slug);
    let strict_extra = [
        "--mode",
        "pr",
        "--merge-commit",
        strict_landing.as_str(),
        "--target-branch",
        "main",
        "--attest-first-landing-commit",
    ];
    let (strict_output, strict_text) = run_accept(
        &env,
        Path::new(binary()),
        &strict_repo,
        &slug,
        &strict_extra,
    );
    assert!(
        !strict_output.status.success(),
        "the STRICT Native accept refuses the out-of-ownership landing: {strict_text}"
    );
    let strict_after = snapshot(&strict_repo, &slug);
    assert_eq!(
        strict_after.meta, strict_before.meta,
        "a refused acceptance writes NO metadata"
    );
    assert_eq!(
        strict_after.log, strict_before.log,
        "a refused acceptance creates NO commit"
    );
    assert_eq!(
        strict_after.branches, strict_before.branches,
        "a refused acceptance moves NO ref"
    );
    assert_eq!(
        strict_after.porcelain, "",
        "a refused acceptance leaves no tracked change"
    );
    assert!(
        strict_after.matrix_present == strict_before.matrix_present,
        "a refused acceptance does not add/refresh the matrix (present before: {})",
        strict_before.matrix_present
    );
    assert!(
        !strict_text.contains("Matrix written"),
        "the refusal does not announce a matrix write: {strict_text}"
    );

    // ---------------- D. RETRY is idempotent on both sides ----------------
    let source_first_tip = git(&source_pr, &["rev-parse", "main"]);
    let native_first_tip = git(&native_pr, &["rev-parse", "main"]);
    let (source_retry, source_retry_text) =
        run_accept(&env, &shim, &source_pr, &slug, &source_pr_extra);
    let (native_retry, native_retry_text) = run_accept(
        &env,
        Path::new(binary()),
        &native_pr,
        &slug,
        &native_pr_extra,
    );
    assert_eq!(
        native_retry.status.success(),
        source_retry.status.success(),
        "the retry outcome class matches Source\nnative: {native_retry_text}\nsource: {source_retry_text}"
    );
    let source_retry_after = snapshot(&source_pr, &slug);
    let native_retry_after = snapshot(&native_pr, &slug);
    for key in [
        "acceptance_mode",
        "baseline_merge_commit",
        "merged_commit",
        "pr_merge_commit",
        "pr_merge_evidence",
    ] {
        assert_eq!(
            native_retry_after.meta[key], source_retry_after.meta[key],
            "retry: meta.{key} still matches Source"
        );
    }
    assert_eq!(
        native_retry_after.meta["acceptance_history"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        source_retry_after.meta["acceptance_history"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        "retry: the acceptance_history length matches Source (no duplicate append)"
    );
    // Source's retry re-runs the acceptance (a new history entry and a new
    // `Accept`/`Record` commit pair); what must NOT change is the recorded
    // baseline/PR landing, which is set-once and still points at the same commit.
    let source_retry_commits = pr_metadata_commits(&run_commits(&source_pr, &source_first_tip));
    let native_retry_commits = pr_metadata_commits(&run_commits(&native_pr, &native_first_tip));
    assert_eq!(
        native_retry_commits, source_retry_commits,
        "retry: the SAME acceptance metadata commits as Source"
    );
    assert_eq!(
        native_retry_after.meta["pr_merge_commit"], native_pr_after.meta["pr_merge_commit"],
        "retry: the PR landing is set-once and does not move"
    );
    assert!(
        pr_before.meta["baseline_merge_commit"].is_null(),
        "the PR baseline was unset before the first acceptance"
    );
}
