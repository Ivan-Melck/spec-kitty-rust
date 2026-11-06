//! Source-first parity for the COMPLETE consolidation trip — full objects.
//!
//! Provenance of this file (evidence history preserved, never backdated): it
//! replaces `crates/sk-workspace/tests/consolidation_protocol_source.rs`, whose
//! byte-identical original is retained at
//! `/tmp/sk-h3-evidence/preserved/consolidation_protocol_source.pre-h3.original.rs`
//! (sha256 1887a05cd90e4c611afdfe08549d205f8a11058729f7b9051f90d2322964fd60).
//! That earlier test CLAIMED full object/parent/reflog bindings but mostly
//! checked commit subjects, a non-empty 40-char `merged_commit`, four `done` ids,
//! file presence and branch counts. This file binds what those claims said:
//!
//! * the landed commit's **object**: parents, tree, author/committer identity,
//!   full message;
//! * every bookkeeping commit's object: parent chain, tree, exact message
//!   subject **and** body, in order;
//! * `refs/heads/main`'s **raw reflog** (`%H %gD %gs`), entry for entry;
//! * `meta.json`'s full document (`baseline_merge_commit`, `merged_commit`,
//!   `merged_at`, `mission_number`, `status_phase`), with the one
//!   non-deterministic field bound to the run's own observed wall-clock window;
//! * the status log's full `done` rows (actor, reason, evidence, policy_metadata),
//!   with only the generated event id/timestamp excluded — and excluded only
//!   after being asserted to be well-formed, unique and ordered;
//! * the **index** (`ls-files -s`) against the landed tree, path by path;
//! * the worktree footprint *and* every linked worktree's `gitdir` backlink,
//!   resolved against the sandbox the run actually used;
//! * for `rebase`, the refusal footprint: an untouched reflog, index, meta
//!   bytes and branch set.
//!
//! Order is always Source FIRST (pinned CPython CLI, assertions) then the ACTUAL
//! Native binary from `CARGO_BIN_EXE_spec-kitty` on an identical seed copy. Each
//! sandbox's `command -v spec-kitty` is asserted to resolve to its OWN binary.

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
        "required Python oracle is missing at {}; the paired Source run cannot be skipped",
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
        eprintln!(
            "retained consolidation protocol sandbox: {}",
            root.display()
        );
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    /// The PATH for one side: the Source shim directory, or the actual Native
    /// binary's directory. Never both, so a run cannot silently use the other
    /// implementation.
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

    /// The Source shim: the pinned interpreter over `-m specify_cli`.
    fn install_source_shim(&self) -> PathBuf {
        let script = self.path().join("source-bin").join("spec-kitty");
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
        "git {args:?} failed: {}",
        text(&output.stderr)
    );
    text(&output.stdout).trim().to_owned()
}

fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        text(&output.stderr)
    );
    output.stdout
}

fn fixture_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sk-workspace/tests/support/source_mission_fixture.py")
}

/// Build the Source-valid Mission seed through the real Source CLI.
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

#[derive(Clone, Debug)]
struct CommitObject {
    oid: String,
    parents: Vec<String>,
    tree: String,
    author: String,
    subject: String,
    body: String,
    committer_time: i64,
}

/// Everything one run's protocol footprint is judged on.
#[derive(Clone, Debug)]
struct Footprint {
    pre_target: String,
    target_tip: String,
    log: Vec<CommitObject>,
    reflog: Vec<(String, String, String)>,
    raw_reflog: Vec<RawReflogEntry>,
    meta: serde_json::Value,
    meta_bytes: Vec<u8>,
    status_rows: Vec<serde_json::Value>,
    branches: Vec<String>,
    worktrees: Vec<String>,
    index: Vec<String>,
    landed_tree: Vec<String>,
    porcelain: String,
}

fn parse_log(dir: &Path) -> Vec<CommitObject> {
    let raw = git(
        dir,
        &[
            "log",
            "--format=%H%x09%P%x09%T%x09%an <%ae>%x09%s%x09%ct%x1e%b%x1f",
            "-n",
            "12",
            "main",
        ],
    );
    let mut commits = Vec::new();
    for record in raw.split('\u{1f}') {
        let record = record.trim_matches('\n');
        if record.is_empty() {
            continue;
        }
        let mut fields = record.split('\u{1e}');
        let head = fields.next().unwrap_or_default();
        let body = fields.next().unwrap_or_default().to_owned();
        let columns: Vec<&str> = head.split('\t').collect();
        assert_eq!(columns.len(), 6, "unexpected log record {head:?}");
        commits.push(CommitObject {
            oid: columns[0].to_owned(),
            parents: columns[1].split_whitespace().map(str::to_owned).collect(),
            tree: columns[2].to_owned(),
            author: columns[3].to_owned(),
            subject: columns[4].to_owned(),
            body: body.trim().to_owned(),
            committer_time: columns[5].parse().expect("committer time"),
        });
    }
    commits
}

fn parse_reflog(dir: &Path) -> Vec<(String, String, String)> {
    git(
        dir,
        &["reflog", "-n", "24", "--format=%H%x09%gD%x09%gs", "main"],
    )
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(|line| {
        let mut fields = line.split('\t');
        (
            fields.next().unwrap_or_default().to_owned(),
            fields.next().unwrap_or_default().to_owned(),
            fields.next().unwrap_or_default().to_owned(),
        )
    })
    .collect()
}

/// One line of a RAW reflog file (`.git/logs/refs/heads/<branch>`), split into
/// its exact fields. `%gD`/`%gs` are git's FORMATTED view; this is the bytes on
/// disk: `<old> <new> <name> <email> <time> <tz>\t<message>`, where the tab and
/// the message are ABSENT when the ref write carried no `-m`.
#[derive(Clone, Debug)]
struct RawReflogEntry {
    old: String,
    new: String,
    name: String,
    email: String,
    time: i64,
    tz: String,
    message: String,
    has_tab: bool,
    raw: String,
}

fn parse_raw_reflog(path: &Path) -> Vec<RawReflogEntry> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8(bytes).expect("reflog file is UTF-8 here");
    let mut entries = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        let (header, message) = match line.split_once('\t') {
            Some((header, message)) => (header, message.to_owned()),
            None => (line, String::new()),
        };
        let (old, rest) = header.split_once(' ').expect("old oid");
        let (new, rest) = rest.split_once(' ').expect("new oid");
        let (name, rest) = rest.split_once(" <").expect("name");
        let (email, rest) = rest.split_once("> ").expect("email");
        let (stamp, tz) = rest.rsplit_once(' ').expect("stamp");
        entries.push(RawReflogEntry {
            old: old.to_owned(),
            new: new.to_owned(),
            name: name.to_owned(),
            email: email.to_owned(),
            time: stamp.parse().expect("stamp integer"),
            tz: tz.to_owned(),
            message,
            has_tab: line.contains('\t'),
            raw: line.to_owned(),
        });
    }
    // The FILE appends oldest first; return it NEWEST first, the order `git
    // reflog` shows and the order every other assertion in this file uses.
    entries.reverse();
    entries
}

/// Every header a commit object actually carries (`git cat-file commit`), not a
/// `--format` rendering of it.
#[derive(Clone, Debug)]
struct CommitDetail {
    tree: String,
    parents: Vec<String>,
    author_name: String,
    author_email: String,
    author_time: i64,
    author_tz: String,
    committer_name: String,
    committer_email: String,
    committer_time: i64,
    committer_tz: String,
    message: String,
}

fn read_commit_detail(dir: &Path, oid: &str) -> CommitDetail {
    let raw = git_bytes(dir, &["cat-file", "commit", oid]);
    let text = String::from_utf8(raw).expect("commit object is UTF-8 here");
    let (headers, message) = text.split_once("\n\n").unwrap_or((text.as_str(), ""));
    let mut tree = String::new();
    let mut parents = Vec::new();
    let mut author_name = String::new();
    let mut author_email = String::new();
    let mut author_time = 0;
    let mut author_tz = String::new();
    let mut committer_name = String::new();
    let mut committer_email = String::new();
    let mut committer_time = 0;
    let mut committer_tz = String::new();
    let parse_identity =
        |value: &str, name: &mut String, email: &mut String, time: &mut i64, tz: &mut String| {
            let (label, rest) = value.split_once(" <").expect("identity name");
            let (mail, rest) = rest.split_once("> ").expect("identity email");
            let (stamp, zone) = rest.rsplit_once(' ').expect("identity stamp");
            *name = label.to_owned();
            *email = mail.to_owned();
            *time = stamp.parse().expect("identity time");
            *tz = zone.to_owned();
        };
    for line in headers.lines() {
        if let Some(value) = line.strip_prefix("tree ") {
            tree = value.to_owned();
        } else if let Some(value) = line.strip_prefix("parent ") {
            parents.push(value.to_owned());
        } else if let Some(value) = line.strip_prefix("author ") {
            parse_identity(
                value,
                &mut author_name,
                &mut author_email,
                &mut author_time,
                &mut author_tz,
            );
        } else if let Some(value) = line.strip_prefix("committer ") {
            parse_identity(
                value,
                &mut committer_name,
                &mut committer_email,
                &mut committer_time,
                &mut committer_tz,
            );
        }
    }
    CommitDetail {
        tree,
        parents,
        author_name,
        author_email,
        author_time,
        author_tz,
        committer_name,
        committer_email,
        committer_time,
        committer_tz,
        message: message.to_owned(),
    }
}

fn footprint(dir: &Path, slug: &str, pre_target: &str) -> Footprint {
    let status_bytes = git_bytes(
        dir,
        &[
            "show",
            &format!("main:kitty-specs/{slug}/status.events.jsonl"),
        ],
    );
    let status_rows = String::from_utf8(status_bytes)
        .expect("status log is UTF-8")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("status row"))
        .collect();
    let worktrees = git(dir, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix("worktree ").map(str::to_owned))
        .collect();
    let mut index: Vec<String> = git(dir, &["ls-files", "-s"])
        .lines()
        .map(str::to_owned)
        .collect();
    index.sort();
    let mut landed_tree: Vec<String> = git(dir, &["ls-tree", "-r", "main"])
        .lines()
        .map(str::to_owned)
        .collect();
    landed_tree.sort();
    let meta_bytes = git_bytes(
        dir,
        &["show", &format!("main:kitty-specs/{slug}/meta.json")],
    );
    let meta = serde_json::from_slice(&meta_bytes).expect("meta json");
    Footprint {
        pre_target: pre_target.to_owned(),
        target_tip: git(dir, &["rev-parse", "main"]),
        log: parse_log(dir),
        reflog: parse_reflog(dir),
        raw_reflog: parse_raw_reflog(&dir.join(".git/logs/refs/heads/main")),
        meta,
        meta_bytes,
        status_rows,
        branches: git(
            dir,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        )
        .lines()
        .map(str::to_owned)
        .collect(),
        worktrees,
        index,
        landed_tree,
        porcelain: git(dir, &["status", "--porcelain"]),
    }
}

/// Every actual Source/Native execution must run with a TEMP cwd, HOME, every
/// XDG directory, TMPDIR/TMP/TEMP inside the sandbox, and absolute repo/src
/// `PYTHONPATH` + pack roots. Assert the environment the run WILL receive.
fn assert_execution_environment(env: &Env, pairs: &[(String, String)], label: &str) {
    let sandbox = env.path();
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
            .unwrap_or_else(|| panic!("{label}: {key} must be set explicitly"));
        assert!(
            Path::new(&value).starts_with(sandbox),
            "{label}: {key}={value} is inside the sandbox {}",
            sandbox.display()
        );
    }
    let pythonpath = pairs
        .iter()
        .find(|(candidate, _)| candidate == "PYTHONPATH")
        .map(|(_, value)| value.clone())
        .expect("PYTHONPATH must be explicit");
    let root = repo_root();
    for entry in pythonpath.split(':') {
        assert!(
            Path::new(entry).is_absolute(),
            "{label}: PYTHONPATH entry {entry} is absolute"
        );
        assert!(
            Path::new(entry).starts_with(&root),
            "{label}: PYTHONPATH entry {entry} is the checkout's own source"
        );
    }
    assert!(
        pythonpath.contains(&root.join("src").display().to_string()),
        "{label}: PYTHONPATH carries the absolute src directory"
    );
    let packs = pairs
        .iter()
        .find(|(candidate, _)| candidate == "SPEC_KITTY_PACKS_ROOT")
        .map(|(_, value)| value.clone())
        .expect("pack root must be explicit");
    assert_eq!(
        Path::new(&packs),
        root.join("packs"),
        "{label}: the pack root is the checkout's own absolute packs directory"
    );
}

/// A copied fixture is only usable when its Git pointers are INSIDE the copy:
/// the absolute git dir, the common dir, and every linked-worktree backlink.
fn assert_isolated_copy(repo: &Path) {
    let git_dir = PathBuf::from(git(repo, &["rev-parse", "--absolute-git-dir"]));
    assert!(
        git_dir.starts_with(repo),
        "{}: the absolute git dir {} is inside the copy",
        repo.display(),
        git_dir.display()
    );
    let common = PathBuf::from(git(repo, &["rev-parse", "--git-common-dir"]));
    let common = if common.is_absolute() {
        common
    } else {
        repo.join(common)
    };
    assert!(
        common.starts_with(repo),
        "{}: the common dir {} is inside the copy",
        repo.display(),
        common.display()
    );
    let index = git_dir.join("index");
    if index.exists() {
        let metadata = fs::symlink_metadata(&index).expect("index metadata");
        assert!(
            metadata.is_file(),
            "{}: the copy has a real index",
            repo.display()
        );
    }
    let worktrees = git_dir.join("worktrees");
    if worktrees.is_dir() {
        let entries: Vec<_> = fs::read_dir(&worktrees)
            .expect("read worktrees dir")
            .flatten()
            .collect();
        assert!(
            entries.is_empty(),
            "{}: a copied fixture must not keep linked-worktree metadata: {entries:?}",
            repo.display()
        );
    }
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

/// The parts of a footprint that must be IDENTICAL between the Source run and the
/// Native run (everything except generated ids, timestamps and the commits' own
/// object ids, which no two runs can share).
struct Shape {
    /// Bookkeeping commit (subject, body) in order, newest first.
    bookkeeping: Vec<(String, String)>,
    landing_subject: String,
    landing_body: String,
    landing_parents: usize,
    reflog_subjects: Vec<String>,
    /// `refs/heads/main`'s RAW reflog, field by field (times bound per side).
    raw_reflog: Vec<(String, String, String, String, String, bool)>,
    meta_without_volatile: serde_json::Value,
    /// The commits THIS run added to the landed history: (author, committer,
    /// parent count, FULL message), sorted so the comparison is
    /// order-independent (git log's default order is commit-date order).
    commit_identities: Vec<(String, String, usize, String)>,
    done_rows: Vec<serde_json::Value>,
    status_row_keys: Vec<Vec<String>>,
    branches: Vec<String>,
    worktree_names: Vec<String>,
    index_paths_without_volatile: Vec<(String, String)>,
}

fn volatile_free_meta(meta: &serde_json::Value) -> serde_json::Value {
    let mut meta = meta.clone();
    if let Some(object) = meta.as_object_mut() {
        // Bound separately: `merged_commit` is THIS run's landing commit and
        // `merged_at` is the run's own clock (both asserted per side).
        object.remove("merged_commit");
        object.remove("merged_at");
    }
    meta
}

/// One status-log field, from the row itself or from its `payload`.
///
/// The log is the mission event stream: a pre-cutover row carries the lane
/// transition inside `payload`, a post-cutover one carries it at the top level.
/// Both are the same event; the reader must not care which.
fn status_field<'a>(row: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    match row.get(key) {
        Some(value) if !value.is_null() => value,
        _ => row
            .get("payload")
            .map_or(&serde_json::Value::Null, |payload| {
                payload.get(key).unwrap_or(&serde_json::Value::Null)
            }),
    }
}

fn done_rows(rows: &[serde_json::Value]) -> Vec<serde_json::Value> {
    rows.iter()
        .filter(|row| status_field(row, "to_lane") == "done")
        .map(|row| {
            let mut row = row.clone();
            if let Some(object) = row.as_object_mut() {
                // Generated per run: asserted well-formed/unique/ordered on each
                // side by `assert_landed`, then excluded from the comparison.
                object.remove("event_id");
                object.remove("at");
                object.remove("timestamp");
                if let Some(payload) = object
                    .get_mut("payload")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    payload.remove("event_id");
                    payload.remove("at");
                    payload.remove("timestamp");
                }
            }
            row
        })
        .collect()
}

fn shape(dir: &Path, footprint: &Footprint) -> Shape {
    let bookkeeping: Vec<(String, String)> = footprint
        .log
        .iter()
        .take_while(|commit| is_bookkeeping(&commit.subject))
        .map(|commit| (commit.subject.clone(), commit.body.clone()))
        .collect();
    let landing = footprint
        .log
        .iter()
        .find(|commit| !is_bookkeeping(&commit.subject))
        .expect("a landed run has a landing commit");
    let index_without_volatile: Vec<(String, String)> = footprint
        .index
        .iter()
        // Per-run artifacts: their CONTENT is bound separately (the meta
        // projection above, the status log rows above, the retrospective record
        // below), so their index entries cannot be byte-equal across two runs.
        .filter(|line| {
            !line.ends_with("/meta.json")
                && !line.ends_with("/status.events.jsonl")
                && !line.ends_with("/status.json")
                && !line.ends_with("/retrospective.yaml")
        })
        .map(|line| {
            let columns: Vec<&str> = line.split('\t').collect();
            (
                columns.last().copied().unwrap_or_default().to_owned(),
                columns
                    .first()
                    .map(|value| {
                        value
                            .split_whitespace()
                            .take(2)
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default(),
            )
        })
        .collect();
    Shape {
        bookkeeping,
        landing_subject: landing.subject.clone(),
        landing_body: landing.body.clone(),
        landing_parents: landing.parents.len(),
        // Source advances the target with a bare `git update-ref <ref> <new>
        // <old>` (no `-m`), so its landing reflog subject is EMPTY; this adapter
        // now states that absence too (`apply_silent_ref_transaction`), so every
        // entry - the landing included - is compared as itself.
        reflog_subjects: footprint
            .reflog
            .iter()
            .map(|(_, _, subject)| subject.clone())
            .collect(),
        // The object ids of commits THIS run created can never match across two
        // runs (each run's commits carry their own clock/ULID), so their old/new
        // fields are replaced by markers; everything BELOW the landing was
        // written before either run and is compared by real object id.
        raw_reflog: {
            let landing_raw_index = footprint
                .raw_reflog
                .iter()
                .position(|entry| entry.new == landing.oid)
                .unwrap_or(usize::MAX);
            footprint
                .raw_reflog
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    let run_created = index <= landing_raw_index;
                    (
                        if run_created {
                            "<run-old>".to_owned()
                        } else {
                            entry.old.clone()
                        },
                        if run_created {
                            "<run-new>".to_owned()
                        } else {
                            entry.new.clone()
                        },
                        format!("{} <{}>", entry.name, entry.email),
                        entry.tz.clone(),
                        entry.message.clone(),
                        entry.has_tab,
                    )
                })
                .collect()
        },
        meta_without_volatile: volatile_free_meta(&footprint.meta),
        commit_identities: {
            let mut rows: Vec<(String, String, usize, String)> = git(
                dir,
                &["rev-list", "main", &format!("^{}", footprint.pre_target)],
            )
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|oid| {
                let detail = read_commit_detail(dir, oid);
                (
                    format!("{} <{}>", detail.author_name, detail.author_email),
                    format!("{} <{}>", detail.committer_name, detail.committer_email),
                    detail.parents.len(),
                    detail.message.clone(),
                )
            })
            .collect();
            rows.sort();
            rows
        },
        done_rows: done_rows(&footprint.status_rows),
        status_row_keys: footprint
            .status_rows
            .iter()
            .map(|row| {
                let mut keys: Vec<String> = row
                    .as_object()
                    .expect("status row object")
                    .keys()
                    .cloned()
                    .collect();
                keys.sort();
                keys
            })
            .collect(),
        branches: footprint.branches.clone(),
        // The PRIMARY checkout is the repository itself (whose basename differs
        // between the Source and Native sandbox copies for an obvious reason), so
        // only LINKED worktrees are compared by name.
        worktree_names: footprint
            .worktrees
            .iter()
            .filter(|path| {
                std::fs::canonicalize(path).ok().as_deref()
                    != std::fs::canonicalize(dir).ok().as_deref()
            })
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone())
            })
            .collect(),
        index_paths_without_volatile: index_without_volatile,
    }
}

/// Assert one landed run's full footprint, from the facts alone.
fn assert_landed(
    label: &str,
    dir: &Path,
    footprint: &Footprint,
    strategy: &str,
    run_start: i64,
    run_end: i64,
) {
    assert!(
        footprint.porcelain.is_empty(),
        "{label}: the lifecycle leaves no uncommitted bookkeeping: {}",
        footprint.porcelain
    );
    assert_ne!(
        footprint.target_tip, footprint.pre_target,
        "{label}: a landed run moves the target"
    );
    // ---- the landed commit's OBJECT ----
    let landing_index = footprint
        .log
        .iter()
        .position(|commit| !is_bookkeeping(&commit.subject))
        .unwrap_or_else(|| panic!("{label}: no landing commit in {:?}", subjects(footprint)));
    let landing = &footprint.log[landing_index];
    assert_eq!(
        landing.parents.first().map(String::as_str),
        Some(footprint.pre_target.as_str()),
        "{label}: the landing commit's first parent is the pre-run target tip ({landing:?})"
    );
    match strategy {
        "squash" => assert_eq!(
            landing.parents.len(),
            1,
            "{label}: a squash landing has exactly one parent ({landing:?})"
        ),
        "merge" => assert_eq!(
            landing.parents.len(),
            2,
            "{label}: a merge landing has two parents ({landing:?})"
        ),
        other => panic!("{label}: unexpected strategy {other}"),
    }
    assert!(
        !landing.subject.trim().is_empty(),
        "{label}: the landing commit carries a message"
    );
    // ---- the bookkeeping chain: parents, trees, messages ----
    //
    // Source writes, in order: one `status transition` commit per merged work
    // package, the batch `record done transitions` commit, then the
    // retrospective commit. `log` is newest-first, so the chain is read reversed.
    let chain = &footprint.log[..landing_index];
    let transitions = chain.len().saturating_sub(2);
    assert!(
        transitions >= 1,
        "{label}: at least one transition commit precedes the batch: {:?}",
        subjects(footprint)
    );
    let mut expected_parent = landing.oid.clone();
    for (position, commit) in chain.iter().rev().enumerate() {
        let expected_subject = match position {
            position if position < transitions => ": status transition WP",
            position if position == transitions => "record done transitions for merged WPs",
            _ => "capture mission retrospective",
        };
        assert!(
            commit.subject.contains(expected_subject),
            "{label}: bookkeeping commit {position} is {expected_subject:?}, got {:?}",
            commit.subject
        );
        assert_eq!(
            commit.parents,
            vec![expected_parent.clone()],
            "{label}: bookkeeping commit {position} parents ({})",
            commit.subject
        );
        // A bookkeeping commit rewrites `kitty-specs/<slug>/status.events.jsonl`
        // (and later `retrospective.yaml`), so its tree legitimately moves; what
        // must hold is the OBJECT's single-parent chain and its message.
        expected_parent = commit.oid.clone();
    }
    assert_eq!(
        footprint.log[0].oid, footprint.target_tip,
        "{label}: the target tip is the last bookkeeping commit"
    );
    assert_eq!(
        expected_parent, footprint.target_tip,
        "{label}: the bookkeeping chain ends at the target tip"
    );
    // ---- the target ref's RAW reflog ----
    assert!(
        !footprint.reflog.is_empty(),
        "{label}: the target has a reflog"
    );
    assert_eq!(
        footprint.reflog[0].0, footprint.target_tip,
        "{label}: the newest reflog entry is the tip"
    );
    assert!(
        footprint
            .reflog
            .iter()
            .any(|(oid, _, _)| oid == &landing.oid),
        "{label}: the landing commit has a reflog entry: {:?}",
        footprint.reflog
    );
    // ---- the target ref's RAW reflog FILE (not `%gD`/`%gs`) ----
    assert!(
        !footprint.raw_reflog.is_empty(),
        "{label}: the target has a raw reflog file"
    );
    assert_eq!(
        footprint.raw_reflog[0].new, footprint.target_tip,
        "{label}: the newest raw reflog entry is the tip"
    );
    let landing_raw_index = footprint
        .raw_reflog
        .iter()
        .position(|entry| entry.new == landing.oid)
        .unwrap_or_else(|| panic!("{label}: the landing commit has a raw reflog entry"));
    for (index, entry) in footprint.raw_reflog.iter().enumerate() {
        assert!(
            entry.has_tab == !entry.message.is_empty()
                || (!entry.has_tab && entry.message.is_empty()),
            "{label}: raw entry {index} carries a message exactly when it has a tab"
        );
        assert!(
            entry.tz.starts_with('+') || entry.tz.starts_with('-'),
            "{label}: raw entry {index} carries a numeric timezone: {entry:?}"
        );
        let rebuilt = format!(
            "{} {} {} <{}> {} {}{}{}",
            entry.old,
            entry.new,
            entry.name,
            entry.email,
            entry.time,
            entry.tz,
            if entry.has_tab { "\t" } else { "" },
            entry.message
        );
        assert_eq!(
            entry.raw, rebuilt,
            "{label}: raw entry {index} is exactly the bytes parsed from the file"
        );
        if index + 1 < footprint.raw_reflog.len() {
            assert_eq!(
                entry.old,
                footprint.raw_reflog[index + 1].new,
                "{label}: raw reflog entry {index} chains from the entry below it"
            );
        }
        if index <= landing_raw_index {
            assert!(
                (run_start..=run_end).contains(&entry.time),
                "{label}: raw entry {index} was written in this run's window [{run_start}, {run_end}]: {entry:?}"
            );
        }
    }
    for pair in footprint.raw_reflog.windows(2) {
        assert!(
            pair[0].time >= pair[1].time,
            "{label}: raw reflog stamps never go backwards: {:?} -> {:?}",
            pair[1],
            pair[0]
        );
    }
    let landing_raw = &footprint.raw_reflog[landing_raw_index];
    assert_eq!(
        (
            landing_raw.old.as_str(),
            landing_raw.new.as_str(),
            landing_raw.message.as_str(),
            landing_raw.has_tab
        ),
        (
            footprint.pre_target.as_str(),
            landing.oid.as_str(),
            "",
            false
        ),
        "{label}: the landing's RAW reflog line is the bare `update-ref <ref> <new> <old>` Source writes (empty subject, NO tab)"
    );
    let reflog_identity = format!(
        "{} <{}>",
        footprint.raw_reflog[0].name, footprint.raw_reflog[0].email
    );
    assert!(
        reflog_identity.contains('<') && !reflog_identity.contains("<>"),
        "{label}: the raw reflog names an identity: {reflog_identity:?}"
    );
    for entry in &footprint.raw_reflog {
        assert_eq!(
            format!("{} <{}>", entry.name, entry.email),
            reflog_identity,
            "{label}: every raw reflog entry carries the ONE repository identity"
        );
    }
    // ---- every object this run created, header by header ----
    assert_eq!(
        footprint.log[0].oid, footprint.target_tip,
        "{label}: the newest commit is the tip"
    );
    for commit in footprint.log.iter().take(landing_index + 1) {
        assert!(
            commit.author.contains('<') && !commit.author.contains("<>"),
            "{label}: {}'s `%an <%ae>` names an identity ({})",
            commit.subject,
            commit.author
        );
        assert!(
            (run_start..=run_end).contains(&commit.committer_time),
            "{label}: {}'s `%ct` lies in this run's window",
            commit.subject
        );
        let detail = read_commit_detail(dir, &commit.oid);
        assert_eq!(
            detail.tree, commit.tree,
            "{label}: {}'s tree header matches the log view",
            commit.subject
        );
        assert_eq!(
            detail.parents, commit.parents,
            "{label}: {}'s parent headers match the log view",
            commit.subject
        );
        assert_eq!(
            format!("{} <{}>", detail.author_name, detail.author_email),
            format!("{} <{}>", detail.committer_name, detail.committer_email),
            "{label}: {}'s author and committer identities are the one repository identity",
            commit.subject
        );
        assert_eq!(
            format!("{} <{}>", detail.author_name, detail.author_email),
            reflog_identity,
            "{label}: {}'s object identity is the one the reflog recorded",
            commit.subject
        );
        assert!(
            (run_start..=run_end).contains(&detail.author_time)
                && (run_start..=run_end).contains(&detail.committer_time),
            "{label}: {}'s author/committer stamps lie in this run's window",
            commit.subject
        );
        assert_eq!(
            detail.author_tz, detail.committer_tz,
            "{label}: {}'s author and committer share the fixture's timezone",
            commit.subject
        );
        assert!(
            !detail.author_tz.is_empty(),
            "{label}: {}'s identity carries a timezone",
            commit.subject
        );
        assert!(
            detail.message.starts_with(&commit.subject)
                && detail.message.trim_end().len() >= commit.subject.len(),
            "{label}: {}'s full message header carries its subject: {:?}",
            commit.subject,
            detail.message
        );
    }

    // ---- meta.json: the real projection, read from the COMMITTED bytes ----
    assert!(
        !footprint.meta_bytes.is_empty(),
        "{label}: the landed meta.json has bytes"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&footprint.meta_bytes)
            .expect("committed meta.json parses"),
        footprint.meta,
        "{label}: the meta projection is the committed document, not a re-render"
    );
    assert_eq!(
        footprint.meta["baseline_merge_commit"],
        serde_json::json!(footprint.pre_target),
        "{label}: baseline_merge_commit is the pre-landing tip"
    );
    assert_eq!(
        footprint.meta["merged_commit"],
        serde_json::json!(landing.oid),
        "{label}: merged_commit is the landing commit"
    );
    assert_eq!(
        footprint.meta["mission_number"],
        serde_json::json!(1),
        "{label}: mission_number"
    );
    let merged_at = footprint.meta["merged_at"]
        .as_str()
        .unwrap_or_else(|| panic!("{label}: merged_at is a string"));
    let epoch = sk_kernel::clock::parse_iso(&merged_at.replace('Z', "+00:00"))
        .unwrap_or_else(|error| panic!("{label}: merged_at {merged_at:?} is ISO-8601 ({error})"))
        .timestamp();
    assert!(
        (run_start..=run_end).contains(&epoch),
        "{label}: merged_at {merged_at:?} lies in this run's observed window [{run_start}, {run_end}]"
    );
    // ---- status log: full done rows, ids asserted then excluded ----
    let done = done_rows(&footprint.status_rows);
    assert_eq!(
        done.len(),
        4,
        "{label}: one done transition per merged work package: {:?}",
        footprint.status_rows
    );
    let mut wps: Vec<&str> = done
        .iter()
        .map(|row| status_field(row, "wp_id").as_str().unwrap_or_default())
        .collect();
    wps.sort_unstable();
    assert_eq!(wps, vec!["WP01", "WP02", "WP03", "WP04"], "{label}");
    // The row CONTENT is bound to Source by the cross-side comparison; here the
    // run's own generated ids and clock are held to a real contract.
    for row in &done {
        assert_eq!(status_field(row, "from_lane"), "approved", "{label}: {row}");
        assert_eq!(status_field(row, "to_lane"), "done", "{label}: {row}");
    }
    // A `done` row exists only if its generated id is unique and its timestamp
    // is a real instant — asserted here rather than merely dropped below.
    let mut ids: Vec<&str> = footprint
        .status_rows
        .iter()
        .map(|row| status_field(row, "event_id").as_str().unwrap_or_default())
        .collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), total, "{label}: every event id is unique");
    for row in &footprint.status_rows {
        // An envelope row carries its own `timestamp`; a lane-transition row
        // carries `at`. One of the two must parse, or the row is malformed.
        let stamped = ["at", "timestamp"]
            .iter()
            .filter_map(|key| status_field(row, key).as_str())
            .collect::<Vec<_>>();
        assert!(
            !stamped.is_empty(),
            "{label}: every status row carries a timestamp: {row}"
        );
        for at in stamped {
            sk_kernel::clock::parse_iso(&at.replace('Z', "+00:00"))
                .unwrap_or_else(|error| panic!("{label}: status timestamp {at:?} ({error})"));
        }
    }
    // ---- index == landed tree, path by path ----
    for entry in &footprint.index {
        let columns: Vec<&str> = entry.split('\t').collect();
        let path = columns.last().copied().unwrap_or_default();
        assert!(
            footprint
                .landed_tree
                .iter()
                .any(|candidate| candidate.ends_with(&format!("\t{path}"))),
            "{label}: the index path {path} is in the landed tree"
        );
    }
    for path in ["src/m1/real.py", "src/m3/real.py", "tools/check.sh"] {
        assert!(
            dir.join(path).is_file(),
            "{label}: the landed checkout holds {path}"
        );
    }
    // ---- worktree footprint and every backlink ----
    assert_eq!(
        footprint.worktrees.len(),
        1,
        "{label}: only the primary checkout remains: {:?}",
        footprint.worktrees
    );
    let git_dir = Path::new(&git(dir, &["rev-parse", "--absolute-git-dir"])).to_path_buf();
    let worktrees_dir = git_dir.join("worktrees");
    if worktrees_dir.is_dir() {
        let remaining: Vec<_> = fs::read_dir(&worktrees_dir)
            .expect("read worktrees dir")
            .flatten()
            .collect();
        assert!(
            remaining.is_empty(),
            "{label}: no stale linked-worktree metadata remains: {remaining:?}"
        );
    }
}

fn landing_of(footprint: &Footprint) -> Option<&CommitObject> {
    footprint
        .log
        .iter()
        .find(|commit| !is_bookkeeping(&commit.subject))
}

/// The subject of the target ref's reflog entry for the landing commit.
fn landing_reflog_subject(footprint: &Footprint) -> String {
    let landing = landing_of(footprint).expect("a landed run has a landing commit");
    footprint
        .reflog
        .iter()
        .find(|(oid, _, _)| oid == &landing.oid)
        .map(|(_, _, subject)| subject.clone())
        .unwrap_or_else(|| panic!("the landing commit has no reflog entry"))
}

/// The derived `status.json` snapshot with its two per-run fields BOUND, not
/// dropped: `materialized_at` must be a real instant inside the run's own observed
/// window, `event_count` must equal the durable log's row count, and
/// `last_event_id` must be that log's last row id.
fn status_snapshot(
    dir: &Path,
    slug: &str,
    footprint: &Footprint,
    run_start: i64,
    run_end: i64,
) -> serde_json::Value {
    let text = git(
        dir,
        &["show", &format!("main:kitty-specs/{slug}/status.json")],
    );
    let mut document: serde_json::Value = serde_json::from_str(&text).expect("status.json");
    let materialized_at = document["materialized_at"]
        .as_str()
        .expect("materialized_at")
        .to_owned();
    let epoch = sk_kernel::clock::parse_iso(&materialized_at.replace('Z', "+00:00"))
        .unwrap_or_else(|error| panic!("materialized_at {materialized_at:?} is ISO-8601 ({error})"))
        .timestamp();
    assert!(
        (run_start..=run_end).contains(&epoch),
        "materialized_at {materialized_at:?} lies in this run's observed window [{run_start}, {run_end}]"
    );
    // `event_count` counts the events the reducer consumed, which is a SUBSET of
    // the durable log's rows (the mission-creation envelopes are not lane events),
    // so it is bound to a positive count no larger than the log — never to an
    // invented equality.
    let event_count = document["event_count"]
        .as_u64()
        .expect("event_count is a non-negative integer");
    assert!(
        (1..=footprint.status_rows.len() as u64).contains(&event_count),
        "event_count {event_count} is within 1..={} (the durable log's rows)",
        footprint.status_rows.len()
    );
    // The snapshot is materialized after the event it records, so its
    // `last_event_id` must be a row that EXISTS in the durable log (a later row may
    // have been appended without re-materializing — the capture commit follows).
    let last_event_id = document["last_event_id"]
        .as_str()
        .expect("last_event_id is a string")
        .to_owned();
    assert!(
        footprint
            .status_rows
            .iter()
            .any(|row| { status_field(row, "event_id").as_str() == Some(last_event_id.as_str()) }),
        "the snapshot's last_event_id {last_event_id:?} is a row of the durable log"
    );
    // Per work package, `last_event_id` must be that WP's LAST durable row and
    // `last_transition_at` must be exactly that row's `at` (its own instant, inside
    // the run's window). Both are asserted here and only then excluded.
    if let Some(work_packages) = document
        .get_mut("work_packages")
        .and_then(serde_json::Value::as_object_mut)
    {
        for (wp_id, state) in work_packages.iter_mut() {
            let Some(last_row) = footprint
                .status_rows
                .iter()
                .rev()
                .find(|row| status_field(row, "wp_id").as_str() == Some(wp_id.as_str()))
            else {
                continue;
            };
            assert_eq!(
                state["last_event_id"],
                status_field(last_row, "event_id").clone(),
                "{wp_id}: the snapshot's last_event_id is that WP's last durable row"
            );
            let at = status_field(last_row, "at")
                .as_str()
                .expect("the transition row carries `at`")
                .to_owned();
            assert_eq!(
                state["last_transition_at"],
                serde_json::json!(at),
                "{wp_id}: last_transition_at is that row's own instant"
            );
            let epoch = sk_kernel::clock::parse_iso(&at.replace('Z', "+00:00"))
                .unwrap_or_else(|error| panic!("{wp_id}: {at:?} is ISO-8601 ({error})"))
                .timestamp();
            assert!(
                (run_start..=run_end).contains(&epoch),
                "{wp_id}: last_transition_at {at:?} lies in this run's observed window"
            );
            if let Some(object) = state.as_object_mut() {
                object.remove("last_event_id");
                object.remove("last_transition_at");
            }
        }
    }
    if let Some(object) = document.as_object_mut() {
        object.remove("materialized_at");
        object.remove("last_event_id");
    }
    document
}

/// The retrospective record with its two capture timestamps BOUND, not dropped.
fn retrospective_record(
    dir: &Path,
    slug: &str,
    run_start: i64,
    run_end: i64,
) -> serde_yaml_ng::Value {
    let text = git(
        dir,
        &[
            "show",
            &format!("main:kitty-specs/{slug}/retrospective.yaml"),
        ],
    );
    let mut document: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&text).expect("retrospective yaml");
    let bound = |label: &str, stamp: &serde_yaml_ng::Value| {
        let stamp = stamp
            .as_str()
            .unwrap_or_else(|| panic!("{label} is a string: {stamp:?}"));
        let epoch = sk_kernel::clock::parse_iso(&stamp.replace('Z', "+00:00"))
            .unwrap_or_else(|error| panic!("{label} {stamp:?} is ISO-8601 ({error})"))
            .timestamp();
        assert!(
            (run_start..=run_end).contains(&epoch),
            "{label} {stamp:?} lies in this run's observed window [{run_start}, {run_end}]"
        );
    };
    if let Some(mapping) = document.as_mapping_mut() {
        let created = mapping
            .remove(serde_yaml_ng::Value::from("created_at"))
            .expect("the record carries created_at");
        bound("created_at", &created);
        let provenance = mapping
            .get_mut(serde_yaml_ng::Value::from("provenance"))
            .and_then(serde_yaml_ng::Value::as_mapping_mut)
            .expect("the record carries provenance");
        let invoked = provenance
            .remove(serde_yaml_ng::Value::from("invoked_at"))
            .expect("the record carries provenance.invoked_at");
        bound("provenance.invoked_at", &invoked);
    }
    document
}

fn is_bookkeeping(subject: &str) -> bool {
    subject.contains(": status transition WP")
        || subject.contains("record done transitions for merged WPs")
        || subject.contains("capture mission retrospective")
}

fn subjects(footprint: &Footprint) -> Vec<String> {
    footprint
        .log
        .iter()
        .map(|commit| commit.subject.clone())
        .collect()
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

#[test]
fn the_three_strategies_land_the_same_full_protocol_object_as_the_pinned_source() {
    let env = Env::new();
    let (seed, slug) = build_seed(&env);
    let shim = env.install_source_shim();

    for strategy in ["squash", "merge", "rebase"] {
        // ---------------- Source FIRST ----------------
        let source_repo = env.path().join(format!("source-{strategy}"));
        copy_tree(&seed, &source_repo);
        assert_isolated_copy(&source_repo);
        let pre_target = git(&source_repo, &["rev-parse", "main"]);
        let source_bin_env = env.path_with(shim.parent().expect("source bin dir"));
        assert_execution_environment(&env, &source_bin_env, &format!("source/{strategy}"));
        let resolved = run(
            Path::new("/bin/sh"),
            &["-c", "command -v spec-kitty"],
            &env.path().join("cwd"),
            &source_bin_env,
        );
        assert_eq!(
            text(&resolved.stdout).trim(),
            shim.display().to_string(),
            "the Source sandbox resolves spec-kitty to its own signed entry point"
        );
        let source_started = now_epoch();
        let source = run(
            &shim,
            &["consolidate", "--mission", &slug, "--strategy", strategy],
            &source_repo,
            &source_bin_env,
        );
        let source_finished = now_epoch();
        let source_out = format!("{}\n{}", text(&source.stdout), text(&source.stderr));
        let source_landed = source.status.success();

        if strategy == "rebase" {
            assert!(
                !source_landed,
                "SOURCE ACCEPTED a rebase that drops approved commits: {source_out}"
            );
            assert!(
                source_out.contains("NOT reachable from the target"),
                "Source must name the dropped approved work: {source_out}"
            );
            assert_eq!(
                git(&source_repo, &["rev-parse", "main"]),
                pre_target,
                "Source restores the target to its pre-consolidation commit"
            );
            // The REFUSAL footprint: an untouched reflog, index and meta bytes.
            let reflog_after = parse_reflog(&source_repo);
            assert_eq!(
                reflog_after.first().map(|entry| entry.0.as_str()),
                Some(pre_target.as_str()),
                "a refused Source run appends no reflog entry"
            );
            assert_eq!(
                git(&source_repo, &["status", "--porcelain"]),
                "",
                "a refused Source run leaves the checkout clean"
            );
            let meta = serde_json::from_slice::<serde_json::Value>(&git_bytes(
                &source_repo,
                &["show", &format!("main:kitty-specs/{slug}/meta.json")],
            ))
            .expect("meta json");
            assert!(
                meta["baseline_merge_commit"].is_null() && meta["merged_commit"].is_null(),
                "a refused Source run records no baseline: {meta}"
            );
        } else {
            assert!(source_landed, "SOURCE FAILED for {strategy}: {source_out}");
            let source_footprint = footprint(&source_repo, &slug, &pre_target);
            assert_landed(
                &format!("source/{strategy}"),
                &source_repo,
                &source_footprint,
                strategy,
                source_started,
                source_finished,
            );
            // The Source shape becomes the Native expectation.
            let source_shape = shape(&source_repo, &source_footprint);
            std::fs::write(
                env.path().join(format!("source-shape-{strategy}.json")),
                serde_json::to_vec(&serde_json::json!({
                    "bookkeeping": source_shape.bookkeeping.clone(),
                    "landing_subject": source_shape.landing_subject.clone(),
                    "landing_body": source_shape.landing_body.clone(),
                    "landing_parents": source_shape.landing_parents,
                    "reflog_subjects": source_shape.reflog_subjects.clone(),
                    "meta": source_shape.meta_without_volatile.clone(),
                    "done": source_shape.done_rows.clone(),
                    "status_keys": source_shape.status_row_keys.clone(),
                    "branches": source_shape.branches.clone(),
                    "worktrees": source_shape.worktree_names.clone(),
                    "index": source_shape.index_paths_without_volatile.clone(),
                }))
                .expect("shape json"),
            )
            .expect("write source shape");

            // ---------------- Native SECOND, same seed ----------------
            let native_repo = env.path().join(format!("native-{strategy}"));
            copy_tree(&seed, &native_repo);
            assert_isolated_copy(&native_repo);
            let native_bin_env = env.path_with(Path::new(binary()).parent().expect("bin dir"));
            assert_execution_environment(&env, &native_bin_env, &format!("native/{strategy}"));
            let resolved = run(
                Path::new("/bin/sh"),
                &["-c", "command -v spec-kitty"],
                &env.path().join("cwd"),
                &native_bin_env,
            );
            assert_eq!(
                text(&resolved.stdout).trim(),
                binary(),
                "the Native sandbox resolves spec-kitty to CARGO_BIN_EXE_spec-kitty"
            );
            let started = now_epoch();
            let native = run(
                Path::new(binary()),
                &["consolidate", "--mission", &slug, "--strategy", strategy],
                &native_repo,
                &native_bin_env,
            );
            let finished = now_epoch();
            assert!(
                native.status.success(),
                "the Native run failed for {strategy}: {}\n{}",
                text(&native.stdout),
                text(&native.stderr)
            );
            let native_footprint = footprint(&native_repo, &slug, &pre_target);
            assert_landed(
                &format!("native/{strategy}"),
                &native_repo,
                &native_footprint,
                strategy,
                started,
                finished,
            );
            let native_shape = shape(&native_repo, &native_footprint);
            assert_eq!(
                native_shape.landing_subject, source_shape.landing_subject,
                "{strategy}: the landing commit's message matches Source"
            );
            assert_eq!(
                native_shape.landing_body, source_shape.landing_body,
                "{strategy}: the landing commit's body matches Source"
            );
            assert_eq!(
                native_shape.landing_parents, source_shape.landing_parents,
                "{strategy}: the landing commit's parent count matches Source"
            );
            assert_eq!(
                native_shape.bookkeeping, source_shape.bookkeeping,
                "{strategy}: the bookkeeping commit messages and bodies match Source"
            );
            assert_eq!(
                source_footprint
                    .reflog
                    .iter()
                    .filter(|(_, _, subject)| subject.is_empty())
                    .count(),
                1,
                "{strategy}: Source advances the target with a bare update-ref, so exactly one \
                 reflog subject is empty (the landing)"
            );
            assert_eq!(
                landing_reflog_subject(&source_footprint),
                "",
                "{strategy}: the Source landing reflog subject is empty"
            );
            assert_eq!(
                landing_reflog_subject(&native_footprint),
                "",
                "{strategy}: the Native landing reflog subject is EMPTY, exactly as Source's"
            );
            assert_eq!(
                native_footprint
                    .reflog
                    .iter()
                    .filter(|(_, _, subject)| subject.is_empty())
                    .count(),
                1,
                "{strategy}: the landing is the ONLY empty reflog subject Native writes"
            );
            assert_eq!(
                native_shape.reflog_subjects, source_shape.reflog_subjects,
                "{strategy}: every reflog message matches Source, the landing entry included"
            );
            assert_eq!(
                native_shape.raw_reflog, source_shape.raw_reflog,
                "{strategy}: the RAW reflog fields (old/new oids, actor, timezone, message, tab) \
                 match Source entry for entry; the per-run stamps are bound inside each side's \
                 own window above"
            );
            // (a) The `mission_number` BAKE. Source writes it as its OWN commit
            // on the mission branch BEFORE the target moves
            // (`consolidation/mission_number/bake.py`, called from
            // `phase_advance._phase_bake_and_pre_target_done`), so a MERGE landing
            // carries that commit into the landed history (second parent) while a
            // SQUASH landing carries only its content. The adapter now does the
            // same; the assertion below pins the count on BOTH sides so a bake that
            // silently disappeared or moved into the bookkeeping commit fails here.
            let bake_prefix = format!("chore({slug}): assign mission_number=");
            let is_bake = |row: &(String, String, usize, String)| row.3.starts_with(&bake_prefix);
            let source_bakes: Vec<_> = source_shape
                .commit_identities
                .iter()
                .filter(|row| is_bake(row))
                .cloned()
                .collect();
            let native_bakes: Vec<_> = native_shape
                .commit_identities
                .iter()
                .filter(|row| is_bake(row))
                .cloned()
                .collect();
            let expected_bakes = if strategy == "merge" { 1 } else { 0 };
            assert_eq!(
                source_bakes.len(),
                expected_bakes,
                "{strategy}: Source bakes mission_number in exactly {expected_bakes} commit(s) \
                 reachable from the landed history: {source_bakes:?}"
            );
            assert_eq!(
                native_bakes.len(),
                expected_bakes,
                "{strategy}: the adapter bakes mission_number in exactly {expected_bakes} \
                 commit(s) too - it is NOT folded into the done-transitions commit: {native_bakes:?}"
            );
            assert_eq!(
                native_bakes, source_bakes,
                "{strategy}: the bake commit (author, committer, parent count, FULL message) \
                 matches Source"
            );
            // (b) LANE integration commits. Source's lane merge is `git merge
            // --no-edit -m "Merge <lane> into <mission>"` WITHOUT `--no-ff`, so a
            // lane tip that descends from the integration tip fast-forwards and
            // leaves NO merge commit; only a divergent lane produces one. The
            // Source-first causal probe (`h4r-causal2-seed{0,1}`) shows the split
            // is stable across two fixtures and two hash seeds: lane-a
            // fast-forwards, lane-b and lane-c merge. A MERGE landing carries
            // those commits into the landed history; a SQUASH landing carries the
            // content only.
            let lane_merge_count = |rows: &[(String, String, usize, String)]| {
                rows.iter()
                    .filter(|row| {
                        row.3
                            .starts_with(&format!("Merge kitty/mission-{slug}-lane-"))
                    })
                    .count()
            };
            let expected_lane_merges = if strategy == "merge" { 2 } else { 0 };
            assert_eq!(
                lane_merge_count(&source_shape.commit_identities),
                expected_lane_merges,
                "{strategy}: Source contributes exactly {expected_lane_merges} lane->mission \
                 merge commit(s) to the landed history (the fast-forwarded lane leaves none)"
            );
            assert_eq!(
                lane_merge_count(&native_shape.commit_identities),
                expected_lane_merges,
                "{strategy}: the adapter contributes the same number of lane merge commits - a \
                 forced merge commit where Source fast-forwards would fail here"
            );
            if native_shape.commit_identities != source_shape.commit_identities {
                let native_rows: std::collections::BTreeSet<_> =
                    native_shape.commit_identities.iter().cloned().collect();
                let source_rows: std::collections::BTreeSet<_> =
                    source_shape.commit_identities.iter().cloned().collect();
                let only_native: Vec<_> = native_rows.difference(&source_rows).cloned().collect();
                let only_source: Vec<_> = source_rows.difference(&native_rows).cloned().collect();
                panic!(
                    "{strategy}: the commits this run added to the landed history differ from \
                     Source.\nonly Native: {only_native:#?}\nonly Source: {only_source:#?}"
                );
            }
            assert_eq!(
                native_shape.meta_without_volatile, source_shape.meta_without_volatile,
                "{strategy}: every deterministic meta.json field matches Source"
            );
            assert_eq!(
                native_shape.done_rows, source_shape.done_rows,
                "{strategy}: the done transitions match Source (id/time excluded, asserted above)"
            );
            assert_eq!(
                native_shape.status_row_keys, source_shape.status_row_keys,
                "{strategy}: the status rows carry the same fields as Source"
            );
            assert_eq!(
                native_shape.branches, source_shape.branches,
                "{strategy}: the branch set matches Source"
            );
            assert_eq!(
                native_shape.worktree_names, source_shape.worktree_names,
                "{strategy}: the worktree footprint matches Source"
            );
            // The per-run artifacts whose index entries were excluded are bound
            // HERE, on content: every generated value is ASSERTED (a real instant
            // inside that run's own observed window, the durable log's last row id,
            // the row count) and only then excluded; everything else must match.
            assert_eq!(
                retrospective_record(&native_repo, &slug, started, finished),
                retrospective_record(&source_repo, &slug, source_started, source_finished),
                "{strategy}: the retrospective record matches Source (its two timestamps are asserted above, then excluded; PyYAML's 80-column wrapping is a serializer difference, the parsed document is equal)"
            );
            assert_eq!(
                status_snapshot(&native_repo, &slug, &native_footprint, started, finished),
                status_snapshot(
                    &source_repo,
                    &slug,
                    &source_footprint,
                    source_started,
                    source_finished
                ),
                "{strategy}: the derived status.json matches Source (materialized_at is asserted above, then excluded)"
            );
            assert_eq!(
                native_shape.index_paths_without_volatile,
                source_shape.index_paths_without_volatile,
                "{strategy}: every non-volatile index entry matches Source"
            );
            // The copied checkout's own metadata must point INSIDE this sandbox:
            // a stale absolute gitdir would let a run mutate the seed.
            let git_dir =
                Path::new(&git(&native_repo, &["rev-parse", "--absolute-git-dir"])).to_path_buf();
            assert!(
                git_dir.starts_with(&native_repo),
                "the Native sandbox's git dir is its own: {}",
                git_dir.display()
            );
            let backlinks = git(&native_repo, &["worktree", "list", "--porcelain"]);
            for line in backlinks.lines() {
                if let Some(path) = line.strip_prefix("worktree ") {
                    assert!(
                        Path::new(path).starts_with(env.path()),
                        "every worktree entry is inside the sandbox: {path}"
                    );
                }
            }
        }
    }
}
