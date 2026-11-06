//! Source-first CLI proof for the origin-freshness gate on `consolidate`.
//!
//! Source runs the gate in `consolidation/executor.py::_enforce_origin_gate` →
//! `origin_gate.check_origin_before_status_dir` → `origin_freshness.run_origin_gate`
//! BEFORE the status directory is resolved and before any branch moves. The
//! gate's own observation FETCHES the listed tips (one bounded `ls-remote`, one
//! bounded `fetch` per remote) and then compares. H4 adds `consolidate
//! --origin-check` and moves the gate onto `GitRepository::refresh_branches`, so
//! this file binds the REAL CLI on a REAL mission, not a gate unit mock:
//!
//! * a code lane branch advanced on a real bare origin (`stale`):
//!   - `enforce` (and the DEFAULT, and an invalid env value) REFUSES with
//!     `ORIGIN_LANE_STALE`, the same headline and the same behind/ahead counts
//!     as Source, and leaves the target, every branch, the index, `meta.json`
//!     and the status log untouched on both sides;
//!   - `warn` reports the same finding as a warning naming the resolved setting
//!     (`origin check is warn, source: flag`) and continues to land;
//! * an origin whose URL does not resolve (`broken`):
//!   - `enforce` refuses `ORIGIN_UNREACHABLE` (never "the branch is absent");
//!   - `off` succeeds and prints Source's not-checked warning — proof that `off`
//!     contacts nothing, because any contact would have failed.
//!
//! The seed is built by the pinned Source CLI itself (`source_mission_fixture.py`),
//! and every Source run happens FIRST, from its own sandbox entry point.

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
    let python = std::env::var_os("SK_KERNEL_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join(".venv-rust-oracle").join(executable));
    assert!(
        python.is_file(),
        "required Python oracle is missing at {}",
        python.display()
    );
    python
}

struct Env {
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("sandbox");
        for directory in [
            "home", "cache", "config", "data", "state", "runtime", "tmp", "cwd",
        ] {
            fs::create_dir_all(temp.path().join(directory)).expect("sandbox root");
        }
        let root = temp.keep();
        eprintln!("retained origin-gate sandbox: {}", root.display());
        Self { root }
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
            ("SPEC_KITTY_WORKTREE_REMOVAL_DELAY".into(), "0".into()),
            (
                "SPEC_KITTY_ALLOW_PROTECTED_BRANCH_COMMITS".into(),
                "1".into(),
            ),
            ("SPEC_KITTY_ENABLE_SAAS_SYNC".into(), "0".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
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
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
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

fn fixture_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sk-workspace/tests/support/source_mission_fixture.py")
}

fn install_source_shim(env: &Env) -> PathBuf {
    let script = env.path().join("source-bin").join("spec-kitty");
    fs::create_dir_all(script.parent().expect("parent")).expect("source bin dir");
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

fn build_seed(env: &Env) -> (PathBuf, String) {
    let shim = install_source_shim(env);
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

/// The lane branch the stale case advances on the origin.
fn stale_lane_branch(seed: &Path, slug: &str) -> String {
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(seed.join(format!("kitty-specs/{slug}/lanes.json"))).expect("lanes read"),
    )
    .expect("lanes json");
    let lane = manifest["lanes"]
        .as_array()
        .expect("lanes")
        .iter()
        .find(|lane| lane["lane_id"].as_str() != Some("lane-planning"))
        .expect("a code lane");
    sk_workspace::naming::lane_branch_name(
        slug,
        lane["lane_id"].as_str().expect("lane id"),
        manifest["target_branch"].as_str().expect("target"),
    )
    .expect("lane branch name")
}

/// One repository's observable state, excluding per-run clocks and the
/// tracking refs the gate's own fetch legitimately moves.
fn snapshot(dir: &Path) -> serde_json::Value {
    let branches = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
        ],
    );
    let mut reflog: Vec<String> = Vec::new();
    for reference in branches.lines() {
        if let Some((name, _)) = reference.split_once(' ') {
            let log = dir.join(".git").join(name.replace("refs/", "logs/"));
            if let Ok(bytes) = fs::read(&log) {
                reflog.push(format!(
                    "{name}#{}",
                    String::from_utf8_lossy(&bytes).lines().count()
                ));
            }
        }
    }
    reflog.sort();
    serde_json::json!({
        "branches": branches,
        "reflog_shapes": reflog,
        "index": git(dir, &["ls-files", "-s"]),
        "porcelain": git(dir, &["status", "--porcelain"]),
        "head": git(dir, &["rev-parse", "HEAD"]),
        "head_branch": git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]),
    })
}

fn artifact_bytes(dir: &Path, slug: &str) -> serde_json::Value {
    let mut artifacts = serde_json::Map::new();
    for name in [
        "meta.json",
        "status.events.jsonl",
        "status.json",
        "lanes.json",
    ] {
        let path = dir.join(format!("kitty-specs/{slug}/{name}"));
        let value = fs::read(&path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        artifacts.insert(name.to_owned(), value.into());
    }
    serde_json::Value::Object(artifacts)
}

fn tracking_refs(dir: &Path) -> serde_json::Value {
    let value = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/remotes",
        ],
    );
    value.into()
}

/// The WHOLE refusal operator text, whitespace-normalized.
///
/// Source renders it through Rich's console, which WRAPS long lines at the
/// terminal width; the Native refusal is one message. Collapsing whitespace
/// compares the text itself instead of the renderer's line breaks. The Native
/// `error_codes:` trailer is not part of the operator text, so it is cut.
fn normalize(output: &str) -> String {
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn refusal_block(output: &str) -> Option<String> {
    let normalized = normalize(output);
    // Anchor on a real refusal CODE, not merely the `ORIGIN_` prefix: the
    // invalid-environment warning line names `SPEC_KITTY_ORIGIN_CHECK`, which
    // also starts with that prefix but is not the refusal.
    let start = [
        "ORIGIN_STATUS_STALE",
        "ORIGIN_LANE_STALE",
        "ORIGIN_LANE_DIVERGED",
        "ORIGIN_UNREACHABLE",
        "ORIGIN_REMOTE_AMBIGUOUS",
        "ORIGIN_COMPARE_FAILED",
    ]
    .iter()
    .filter_map(|code| normalized.find(code))
    .min()?;
    let mut block = normalized[start..].to_owned();
    if let Some(cut) = block.find("error_codes: ") {
        block.truncate(cut);
    }
    Some(block.trim_end().to_owned())
}

fn error_codes(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("error_codes: ").map(str::to_owned))
}

fn case_repo(env: &Env, seed: &Path, label: &str) -> PathBuf {
    let repo = env.path().join(label);
    copy_tree(seed, &repo);
    repo
}

/// Point the case repo at the shared bare origin and drop its tracking refs so
/// the gate's fetch has real work to do.
fn attach_origin(repo: &Path, origin: &Path) {
    git(
        repo,
        &["remote", "add", "origin", origin.to_str().expect("origin")],
    );
    let refs = git(
        repo,
        &["for-each-ref", "--format=%(refname)", "refs/remotes"],
    );
    for reference in refs.lines() {
        git(repo, &["update-ref", "-d", reference]);
    }
}

fn create_stale_origin(env: &Env, seed: &Path, lane: &str) -> PathBuf {
    let origin = env.path().join("origin-stale.git");
    git(
        env.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().expect("origin"),
        ],
    );
    let work = env.path().join("origin-work-stale");
    copy_tree(seed, &work);
    git(
        &work,
        &["remote", "add", "origin", origin.to_str().expect("origin")],
    );
    git(&work, &["push", "-q", "origin", "--all"]);
    git(&work, &["checkout", "-q", lane]);
    fs::write(work.join("origin-advance.md"), "ahead\n").expect("write");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-qm", "advance the lane on the origin"]);
    git(&work, &["push", "-q", "origin", lane]);
    origin
}

fn run_cli(
    env: &Env,
    binary_path: &Path,
    repo: &Path,
    slug: &str,
    extra: &[&str],
    environment: &[(&str, &str)],
) -> Output {
    let bin_dir = binary_path.parent().expect("bin dir");
    let mut pairs = env.path_with(bin_dir);
    for (key, value) in environment {
        pairs.retain(|(existing, _)| existing != key);
        pairs.push(((*key).to_owned(), (*value).to_owned()));
    }
    let mut args = vec!["consolidate", "--mission", slug, "--strategy", "squash"];
    args.extend(extra.iter().copied());
    run(binary_path, &args, repo, &pairs)
}

/// One paired case: Source first (with its own assertions), then Native on an
/// identical copy, comparing refusals or warnings field by field.
fn paired_case(
    env: &Env,
    seed: &Path,
    slug: &str,
    label: &str,
    prepare: impl Fn(&Path),
    extra: &[&str],
    environment: &[(&str, &str)],
    expect_success: bool,
) -> (String, String, bool) {
    let source_repo = case_repo(env, seed, &format!("source-{label}"));
    prepare(&source_repo);
    let native_repo = case_repo(env, seed, &format!("native-{label}"));
    prepare(&native_repo);
    assert_eq!(
        snapshot(&source_repo),
        snapshot(&native_repo),
        "{label}: both sides start from the same branch/index/worktree state"
    );

    let shim = env.path().join("source-bin/spec-kitty");
    let source_before = (snapshot(&source_repo), artifact_bytes(&source_repo, slug));
    let source = run_cli(env, &shim, &source_repo, slug, extra, environment);
    let source_out = format!("{}\n{}", text(&source.stdout), text(&source.stderr));
    let source_after = (snapshot(&source_repo), artifact_bytes(&source_repo, slug));
    assert_eq!(
        source.status.success(),
        expect_success,
        "{label}: SOURCE outcome expected success={expect_success}: {source_out}"
    );

    let native_before = (snapshot(&native_repo), artifact_bytes(&native_repo, slug));
    assert_eq!(
        source_before, native_before,
        "{label}: the two copies start identical (branches, reflogs, index, artifacts)"
    );
    let native = run_cli(
        env,
        Path::new(binary()),
        &native_repo,
        slug,
        extra,
        environment,
    );
    let native_out = format!("{}\n{}", text(&native.stdout), text(&native.stderr));
    let native_after = (snapshot(&native_repo), artifact_bytes(&native_repo, slug));
    assert_eq!(
        native.status.success(),
        source.status.success(),
        "{label}: the Native exit class matches Source\nnative: {native_out}\nsource: {source_out}"
    );

    if !expect_success {
        let source_block = refusal_block(&source_out)
            .unwrap_or_else(|| panic!("{label}: SOURCE refusal text missing: {source_out}"));
        let native_block = refusal_block(&native_out)
            .unwrap_or_else(|| panic!("{label}: Native refusal text missing: {native_out}"));
        assert_eq!(
            native_block, source_block,
            "{label}: the FULL refusal operator text (headline with behind/ahead counts, remedy, opt-out line) matches Source"
        );
        let source_codes = error_codes(&source_out);
        let native_codes = error_codes(&native_out);
        match (source_codes, native_codes) {
            (Some(source_codes), Some(native_codes)) => assert_eq!(
                native_codes, source_codes,
                "{label}: the error_codes list matches Source"
            ),
            (_, native_codes) => assert!(
                source_block.starts_with(native_codes.as_deref().unwrap_or("")),
                "{label}: the Native code is the Source refusal's code"
            ),
        }
        // NOTHING MOVED: no branch, reflog entry, index entry, worktree change or
        // committed artifact differs from before the refused run.
        assert_eq!(
            source_before, source_after,
            "{label}: a refused SOURCE run moves nothing"
        );
        assert_eq!(
            native_before, native_after,
            "{label}: a refused Native run moves nothing"
        );
    } else {
        // The gate reported, then the run landed: the target advanced on both.
        assert_ne!(
            source_after.0["head"], source_before.0["head"],
            "{label}: a successful Source run lands"
        );
        assert_ne!(
            native_after.0["head"], native_before.0["head"],
            "{label}: a successful Native run lands"
        );
    }
    // The gate's own fetch may move tracking refs — and it must move them the
    // SAME way on both sides, because both performed the same bounded fetch.
    assert_eq!(
        tracking_refs(&native_repo),
        tracking_refs(&source_repo),
        "{label}: the tracking refs the gate's fetch produced match Source's"
    );
    (source_out, native_out, source.status.success())
}

#[test]
fn consolidate_origin_check_gate_matches_the_pinned_source_cli() {
    let env = Env::new();
    let (seed, slug) = build_seed(&env);
    let lane = stale_lane_branch(&seed, &slug);
    eprintln!("stale lane branch: {lane}");
    let stale_origin = create_stale_origin(&env, &seed, &lane);

    // ---------------- stale lane: enforce / default / env-invalid / warn ------
    let attach_stale = |repo: &Path| attach_origin(repo, &stale_origin);

    // (1) explicit `enforce`
    let (source_out, native_out, _) = paired_case(
        &env,
        &seed,
        &slug,
        "enforce",
        attach_stale,
        &["--origin-check", "enforce"],
        &[],
        false,
    );
    assert!(
        source_out.contains("ORIGIN_LANE_STALE"),
        "SOURCE refuses a stale lane in enforce mode: {source_out}"
    );
    assert!(
        native_out.contains("ORIGIN_LANE_STALE"),
        "the Native refusal names the same code: {native_out}"
    );
    assert!(
        source_out.contains("Opt out (not recommended): --origin-check warn"),
        "SOURCE prints the merge-path opt-out line: {source_out}"
    );
    assert!(
        native_out.contains("Opt out (not recommended): --origin-check warn"),
        "the Native refusal prints the same operator text: {native_out}"
    );

    // (2) DEFAULT: no flag, no environment ⇒ enforce
    let (source_out, native_out, _) =
        paired_case(&env, &seed, &slug, "default", attach_stale, &[], &[], false);
    assert!(
        source_out.contains("ORIGIN_LANE_STALE"),
        "SOURCE's default mode enforces: {source_out}"
    );
    assert!(
        native_out.contains("ORIGIN_LANE_STALE"),
        "the Native default mode enforces too: {native_out}"
    );

    // (3) invalid environment value ⇒ warn about the value, then enforce
    let (source_out, native_out, _) = paired_case(
        &env,
        &seed,
        &slug,
        "env-invalid",
        attach_stale,
        &[],
        &[("SPEC_KITTY_ORIGIN_CHECK", "sometimes")],
        false,
    );
    for (label, output) in [("source", &source_out), ("native", &native_out)] {
        let output = normalize(output);
        assert!(
            output.contains("SPEC_KITTY_ORIGIN_CHECK='sometimes'")
                || output.contains("SPEC_KITTY_ORIGIN_CHECK=sometimes"),
            "{label}: the invalid environment value is named: {output}"
        );
        assert!(
            output.contains("enforcing the origin freshness check"),
            "{label}: an invalid environment value enforces: {output}"
        );
    }

    // (4) warn ⇒ the same finding as a warning naming the setting, then land
    let (source_out, native_out, source_ok) = paired_case(
        &env,
        &seed,
        &slug,
        "warn",
        attach_stale,
        &["--origin-check", "warn"],
        &[],
        true,
    );
    assert!(source_ok, "SOURCE continues in warn mode");
    for (label, output) in [("source", &source_out), ("native", &native_out)] {
        let output = normalize(output);
        assert!(
            output.contains("origin check is warn, source: flag; continuing"),
            "{label}: the warn text names the mode and its source: {output}"
        );
        assert!(
            output.contains("ORIGIN_LANE_STALE"),
            "{label}: the warning names the finding: {output}"
        );
    }

    // ---------------- unreachable origin: enforce vs off ---------------------
    // `off` MUST contact nothing: the remote URL cannot be resolved, so any
    // contact would fail and a successful run proves the opt-out is real.
    let broken = env.path().join("origin-broken.git");
    let attach_broken = |repo: &Path| {
        git(
            repo,
            &[
                "remote",
                "add",
                "origin",
                broken.to_str().expect("broken origin"),
            ],
        );
    };
    let (source_out, native_out, _) = paired_case(
        &env,
        &seed,
        &slug,
        "broken-enforce",
        attach_broken,
        &["--origin-check", "enforce"],
        &[],
        false,
    );
    for (label, output) in [("source", &source_out), ("native", &native_out)] {
        assert!(
            output.contains("ORIGIN_UNREACHABLE"),
            "{label}: an unreachable origin refuses with ORIGIN_UNREACHABLE, never a missing branch: {output}"
        );
    }
    let (source_out, native_out, source_ok) = paired_case(
        &env,
        &seed,
        &slug,
        "broken-off",
        attach_broken,
        &["--origin-check", "off"],
        &[],
        true,
    );
    assert!(
        source_ok,
        "SOURCE contacts nothing in off mode, so the broken remote is harmless: {source_out}"
    );
    for (label, output) in [("source", &source_out), ("native", &native_out)] {
        let output = normalize(output);
        assert!(
            output.contains("Origin was not checked (origin check is off, source: flag)"),
            "{label}: the off warning names the mode and its source: {output}"
        );
    }
}
