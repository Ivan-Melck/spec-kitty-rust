//! Source-first parity for the SEVEN registered merge-driver subprocesses.
//!
//! Provenance note (evidence history preserved, never backdated): the earlier
//! `consolidation_drivers_source.rs` file in this crate is a Native-only control
//! — it constructs Native fixtures and runs the Native engine, and its original
//! "Source-first" title was wrong. This file is the real paired test: for every
//! driver the PINNED Source CLI runs and asserts FIRST, then the ACTUAL Native
//! `spec-kitty` binary runs the same bytes and must agree on exit status,
//! stderr emptiness/refusal text and the written `%A` body.
//!
//! Two bindings are asserted separately, because a wired CLI is not proof that a
//! bare `spec-kitty` in Git config reaches it:
//!
//! * the Source sandbox resolves `spec-kitty` to the pinned CPython entry point
//!   (`command -v spec-kitty` is the Source console script / module shim);
//! * the Native sandbox resolves `spec-kitty` to `CARGO_BIN_EXE_spec-kitty`, and
//!   a real `git merge` whose `merge.spec-kitty-<name>.driver` is the bare
//!   `spec-kitty merge-driver-<name> %O %A %B` command invokes THAT binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MISSION_ID: &str = "01KN2371WNZX28ME4MN97B67PJ";
const SLUG: &str = "demo";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn oracle_python() -> PathBuf {
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
    let verified = Command::new(&python)
        .arg(repo_root().join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(repo_root())
        .output()
        .expect("run the oracle verifier");
    assert!(
        verified.status.success(),
        "the Python oracle is not lock-exact: {} {}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    python
}

/// A sandbox with its own HOME, every XDG root and TMPDIR, plus a `bin/` the
/// test controls.
struct Sandbox {
    temp: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("sandbox");
        for directory in [
            "home", "cache", "config", "data", "state", "tmp", "cwd", "bin",
        ] {
            fs::create_dir_all(temp.path().join(directory)).expect("sandbox root");
        }
        Self { temp }
    }

    fn path(&self) -> &Path {
        self.temp.path()
    }

    fn source_env(&self) -> Vec<(String, String)> {
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
                "TMPDIR".into(),
                self.path().join("tmp").display().to_string(),
            ),
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
            ("LANG".into(), "C.UTF-8".into()),
            ("SPEC_KITTY_NO_UPGRADE_CHECK".into(), "1".into()),
        ]
    }

    fn native_env(&self) -> Vec<(String, String)> {
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
                "TMPDIR".into(),
                self.path().join("tmp").display().to_string(),
            ),
            ("SPEC_KITTY_NO_UPGRADE_CHECK".into(), "1".into()),
        ]
    }

    /// The Source `spec-kitty` binding: the pinned interpreter plus `-m`, exposed
    /// as a real `spec-kitty` executable on a PATH only this sandbox uses.
    fn install_source_spec_kitty(&self) -> PathBuf {
        let root = repo_root();
        let script = self.path().join("source-bin").join("spec-kitty");
        fs::create_dir_all(script.parent().expect("parent")).expect("source bin dir");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nexec {} -m specify_cli \"$@\"\n",
                oracle_python().display()
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

/// One driver case: three sibling files plus the expected outcome.
struct DriverCase {
    name: &'static str,
    base: Option<String>,
    ours: String,
    theirs: String,
    /// `true` when the driver must resolve (exit 0) and `false` when it refuses.
    resolves: bool,
}

fn event(event_id: &str, at: &str, wp: &str, from: &str, to: &str) -> String {
    format!(
        "{{\"event_id\":\"{event_id}\",\"at\":\"{at}\",\"wp_id\":\"{wp}\",\"from_lane\":\"{from}\",\"to_lane\":\"{to}\",\"actor\":\"agent\"}}\n"
    )
}

fn meta_json(planning: &str, accepted: Option<&str>) -> String {
    let mut value = serde_json::json!({
        "mission_id": MISSION_ID,
        "mission_slug": SLUG,
        "mission_number": 3,
        "planning": planning,
        "target_branch": "main",
    });
    if let Some(accepted) = accepted {
        value["accepted_at"] = serde_json::json!(accepted);
    }
    serde_json::to_string_pretty(&value).expect("meta json")
}

fn cases() -> Vec<DriverCase> {
    vec![
        DriverCase {
            name: "merge-driver-event-log",
            base: Some(event("e1", "1", "WP01", "planned", "in_progress")),
            ours: format!(
                "{}{}",
                event("e1", "1", "WP01", "planned", "in_progress"),
                event("e2", "2", "WP01", "in_progress", "for_review")
            ),
            theirs: format!(
                "{}{}",
                event("e1", "1", "WP01", "planned", "in_progress"),
                event("e3", "3", "WP02", "planned", "in_progress")
            ),
            resolves: true,
        },
        DriverCase {
            name: "merge-driver-event-log",
            base: Some(event("e1", "1", "WP01", "planned", "in_progress")),
            ours: format!(
                "{}{}",
                event("e1", "1", "WP01", "planned", "in_progress"),
                event("e4", "4", "WP01", "for_review", "approved")
            ),
            theirs: format!(
                "{}{}",
                event("e1", "1", "WP01", "planned", "in_progress"),
                event("e5", "5", "WP01", "for_review", "canceled")
            ),
            resolves: false,
        },
        DriverCase {
            name: "merge-driver-meta",
            base: Some(meta_json("base", None)),
            ours: meta_json("ours", Some("2026-11-02T10:00:00Z")),
            theirs: meta_json("theirs", None),
            resolves: true,
        },
        DriverCase {
            name: "merge-driver-traces",
            base: Some("# Trace\n\n## section a\n- one\n".to_owned()),
            ours: "# Trace\n\n## section a\n- one\n- ours\n".to_owned(),
            theirs: "# Trace\n\n## section b\n- theirs\n".to_owned(),
            resolves: true,
        },
        DriverCase {
            // The full Source `to_dict()` shape: identity strings, every typed
            // criterion slot, an invariant whose provenance is `recorded`, and
            // non-ASCII text (which Source escapes with `ensure_ascii=True`).
            name: "merge-driver-acceptance-matrix",
            base: Some(
                "{\"mission_slug\":\"demo\",\"mission_number\":\"7\",\"mission_type\":\"software-dev\",\"criteria\":[{\"criterion_id\":\"AC-001\",\"description\":\"café ☃ works\",\"proof_type\":\"code_review\",\"pass_fail\":\"pending\"}],\"negative_invariants\":[{\"invariant_id\":\"NI-1\",\"description\":\"no legacy marker\",\"verification_method\":\"grep_absence\",\"verification_command\":\"legacy\",\"result\":\"pending\"}]}"
                    .to_owned(),
            ),
            ours: "{\"mission_slug\":\"demo\",\"mission_number\":\"7\",\"mission_type\":\"software-dev\",\"criteria\":[{\"criterion_id\":\"AC-001\",\"description\":\"café ☃ works\",\"proof_type\":\"code_review\",\"evidence\":\"review-cycle-1\",\"pass_fail\":\"pass\",\"verified_by\":\"bob\",\"verified_at\":\"2026-11-02T10:00:00+00:00\",\"notes\":\"ok\"}],\"negative_invariants\":[{\"invariant_id\":\"NI-1\",\"description\":\"no legacy marker\",\"verification_method\":\"grep_absence\",\"verification_command\":\"legacy\",\"result\":\"confirmed_absent\",\"evidence\":\"grep found zero matches\",\"verified_ref\":\"0f0f0f\",\"verified_surface_kind\":\"primary\",\"provenance_origin\":\"recorded\"}]}"
                .to_owned(),
            theirs: "{\"mission_slug\":\"demo\",\"mission_number\":\"7\",\"mission_type\":\"software-dev\",\"criteria\":[{\"criterion_id\":\"AC-002\",\"description\":\"second criterion\",\"proof_type\":\"manual_qa\",\"pass_fail\":\"pass\"}],\"negative_invariants\":[]}"
                .to_owned(),
            resolves: true,
        },
        DriverCase {
            name: "merge-driver-issue-matrix",
            base: Some("{\"rows\":[{\"issue_ref\":\"#12\",\"verdict\":\"pending\"}]}".to_owned()),
            ours: "{\"rows\":[{\"issue_ref\":\"#12\",\"verdict\":\"resolved\"}]}".to_owned(),
            theirs: "{\"rows\":[{\"issue_ref\":\"#13\",\"verdict\":\"resolved\"}]}".to_owned(),
            resolves: true,
        },
        DriverCase {
            // Source's review-cycle body is deliberately NON-aborting: a genuine
            // collision embeds both documents behind conflict markers and exits 0
            // (the `.md` render is un-adjudicated prose). Source asserts that
            // first; the Native body must agree byte-for-byte.
            name: "merge-driver-review-cycle",
            base: Some("cycle: 1\nverdict: approved\n".to_owned()),
            ours: "cycle: 1\nverdict: approved\n".to_owned(),
            theirs: "cycle: 1\nverdict: rejected\n".to_owned(),
            resolves: true,
        },
        DriverCase {
            // A Source-VALID `decisions/index.json`: `DecisionIndex` is
            // `extra="forbid"` with `version`/`mission_id`/`entries`, so the case
            // carries the real document shape rather than a `decisions` list.
            name: "merge-driver-decision-index",
            base: Some("{\"version\": 1, \"mission_id\": \"01KN2371WNZX28ME4MN97B67PJ\", \"entries\": [{\"decision_id\": \"d1\", \"origin_flow\": \"plan\", \"step_id\": \"implement\", \"input_key\": \"scope\", \"question\": \"Which scope?\", \"options\": [\"a\", \"b\"], \"status\": \"open\", \"created_at\": \"2026-11-02T10:00:00Z\", \"mission_id\": \"01KN2371WNZX28ME4MN97B67PJ\", \"mission_slug\": \"demo\"}]}".to_owned()),
            ours: "{\"version\": 1, \"mission_id\": \"01KN2371WNZX28ME4MN97B67PJ\", \"entries\": [{\"decision_id\": \"d1\", \"origin_flow\": \"plan\", \"step_id\": \"implement\", \"input_key\": \"scope\", \"question\": \"Which scope?\", \"options\": [\"a\", \"b\"], \"status\": \"resolved\", \"created_at\": \"2026-11-02T10:00:00Z\", \"mission_id\": \"01KN2371WNZX28ME4MN97B67PJ\", \"mission_slug\": \"demo\", \"final_answer\": \"a\"}]}".to_owned(),
            theirs: "{\"version\": 1, \"mission_id\": \"01KN2371WNZX28ME4MN97B67PJ\", \"entries\": [{\"decision_id\": \"d1\", \"origin_flow\": \"plan\", \"step_id\": \"implement\", \"input_key\": \"scope\", \"question\": \"Which scope?\", \"options\": [\"a\", \"b\"], \"status\": \"open\", \"created_at\": \"2026-11-02T10:00:00Z\", \"mission_id\": \"01KN2371WNZX28ME4MN97B67PJ\", \"mission_slug\": \"demo\"}]}".to_owned(),
            resolves: true,
        },
    ]
}

struct Prepared {
    base: PathBuf,
    ours: PathBuf,
    theirs: PathBuf,
}

fn prepare(sandbox: &Sandbox, case: &DriverCase, into: &Path) -> Prepared {
    fs::create_dir_all(into).expect("case dir");
    let base = into.join("base");
    let ours = into.join("ours");
    let theirs = into.join("theirs");
    if let Some(body) = &case.base {
        fs::write(&base, body).expect("base");
    } else {
        fs::write(&base, "").expect("base");
    }
    fs::write(&ours, &case.ours).expect("ours");
    fs::write(&theirs, &case.theirs).expect("theirs");
    fs::create_dir_all(sandbox.path().join("cwd")).expect("cwd");
    Prepared { base, ours, theirs }
}

#[test]
fn every_registered_driver_subprocess_agrees_with_the_pinned_source() {
    let root = repo_root();
    assert!(
        root.join("src/specify_cli/cli").is_dir(),
        "the pinned Source tree must be present for the paired run"
    );
    for (index, case) in cases().into_iter().enumerate() {
        let label = format!("{}-{index}", case.name);

        // ---------- Source FIRST ----------
        let source_sandbox = Sandbox::new();
        let source_script = source_sandbox.install_source_spec_kitty();
        let source_case = prepare(&source_sandbox, &case, &source_sandbox.path().join("case"));
        let mut source_path = source_sandbox
            .path()
            .join("source-bin")
            .display()
            .to_string();
        source_path.push_str(":/usr/bin:/bin");
        let mut source_env = source_sandbox.source_env();
        source_env.push(("PATH".into(), source_path));
        // The binding is asserted, not assumed: a bare `spec-kitty` on this PATH
        // is the Source shim wrapping the pinned interpreter.
        let resolved = run(
            Path::new("/bin/sh"),
            &["-c", "command -v spec-kitty"],
            &source_sandbox.path().join("cwd"),
            &source_env,
        );
        assert!(resolved.status.success(), "Source PATH has no spec-kitty");
        assert_eq!(
            text(&resolved.stdout).trim(),
            source_script.display().to_string(),
            "the Source sandbox must resolve spec-kitty to the pinned Source entry point"
        );
        let source = run(
            &source_script,
            &[
                case.name,
                &source_case.base.display().to_string(),
                &source_case.ours.display().to_string(),
                &source_case.theirs.display().to_string(),
            ],
            &source_sandbox.path().join("cwd"),
            &source_env,
        );
        let source_ours = fs::read(&source_case.ours).expect("source ours");
        let source_stderr = text(&source.stderr);
        if case.resolves {
            assert!(
                source.status.success(),
                "SOURCE FAILED FIRST for {label}: stderr={source_stderr}"
            );
        } else {
            assert!(
                !source.status.success(),
                "SOURCE ACCEPTED FIRST for {label}: {source_stderr}"
            );
        }

        // ---------- Native SECOND, same input bytes ----------
        let native_sandbox = Sandbox::new();
        let native_bin_dir = Path::new(binary()).parent().expect("binary dir");
        let mut native_path = native_bin_dir.display().to_string();
        native_path.push_str(":/usr/bin:/bin");
        let mut native_env = native_sandbox.native_env();
        native_env.push(("PATH".into(), native_path.clone()));
        let native_case = prepare(&native_sandbox, &case, &native_sandbox.path().join("case"));
        assert_eq!(
            fs::read(&native_case.ours).expect("native ours before"),
            case.ours.as_bytes(),
            "the Native case must start from the same bytes the Source case did"
        );
        let resolved_native = run(
            Path::new("/bin/sh"),
            &["-c", "command -v spec-kitty"],
            &native_sandbox.path().join("cwd"),
            &native_env,
        );
        assert!(
            resolved_native.status.success(),
            "Native PATH has no spec-kitty"
        );
        assert_eq!(
            text(&resolved_native.stdout).trim(),
            binary(),
            "the Native sandbox must resolve spec-kitty to the actual Native binary"
        );
        let native = run(
            Path::new(binary()),
            &[
                case.name,
                &native_case.base.display().to_string(),
                &native_case.ours.display().to_string(),
                &native_case.theirs.display().to_string(),
            ],
            &native_sandbox.path().join("cwd"),
            &native_env,
        );
        let native_ours = fs::read(&native_case.ours).expect("native ours");

        assert_eq!(
            native.status.code(),
            source.status.code(),
            "exit status diverged for {label}: source stderr={source_stderr} native stderr={}",
            text(&native.stderr)
        );
        assert_eq!(
            text(&native.stderr).trim().is_empty(),
            source_stderr.trim().is_empty(),
            "stderr shape diverged for {label}: source={source_stderr} native={}",
            text(&native.stderr)
        );
        if case.resolves {
            // EVERY resolving body is compared as BYTES, including the three
            // Python dataclass round-trips: their field order, explicit `null`
            // slots, omitted unset optionals and `ensure_ascii` escapes all come
            // from Source's own writer, and the Native writer now renders through
            // the one Kernel `json.dumps` encoder in the same order.
            assert_eq!(
                native_ours,
                source_ours,
                "the written %A bytes diverged for {label}: source={:?} native={:?}",
                text(&source_ours),
                text(&native_ours)
            );
        } else {
            assert_eq!(
                native_ours,
                case.ours.as_bytes().to_vec(),
                "a refusing driver must leave %A untouched for {label}"
            );
            assert!(
                !text(&native.stderr).trim().is_empty(),
                "a refusing Native driver must explain itself for {label}"
            );
        }
    }
}

#[test]
fn a_bare_spec_kitty_git_driver_command_invokes_the_actual_native_binary() {
    // The end-to-end binding: Git itself runs `spec-kitty merge-driver-meta
    // %O %A %B` from a real merge, on a PATH where the only `spec-kitty` is the
    // actual Native binary.
    let sandbox = Sandbox::new();
    let root = sandbox.path().join("repo");
    fs::create_dir_all(&root).expect("repo");
    let native_bin_dir = Path::new(binary()).parent().expect("binary dir");
    let mut path = native_bin_dir.display().to_string();
    path.push_str(":/usr/bin:/bin");
    let mut env = sandbox.native_env();
    env.push(("PATH".into(), path));
    let git =
        |dir: &Path, args: &[&str]| -> Output { run(Path::new("/usr/bin/git"), args, dir, &env) };
    assert!(git(&root, &["init", "-q", "-b", "main"]).status.success());
    fs::write(root.join("meta.json"), meta_json("base", None)).expect("meta");
    assert!(git(&root, &["add", "meta.json"]).status.success());
    assert!(
        git(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        )
        .status
        .success()
    );
    assert!(
        git(
            &root,
            &[
                "config",
                "merge.spec-kitty-meta.name",
                "Spec Kitty mission meta field merge"
            ]
        )
        .status
        .success()
    );
    assert!(
        git(
            &root,
            &[
                "config",
                "merge.spec-kitty-meta.driver",
                "spec-kitty merge-driver-meta %O %A %B"
            ],
        )
        .status
        .success()
    );
    fs::write(
        root.join(".gitattributes"),
        "meta.json merge=spec-kitty-meta\n",
    )
    .expect("attrs");
    assert!(git(&root, &["add", ".gitattributes"]).status.success());
    assert!(
        git(
            &root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "attributes",
            ],
        )
        .status
        .success()
    );
    assert!(
        git(&root, &["checkout", "-q", "-b", "theirs"])
            .status
            .success()
    );
    fs::write(root.join("meta.json"), meta_json("theirs", None)).expect("theirs");
    assert!(git(&root, &["commit", "-qam", "theirs"]).status.success());
    assert!(git(&root, &["checkout", "-q", "main"]).status.success());
    fs::write(
        root.join("meta.json"),
        meta_json("ours", Some("2026-11-02T10:00:00Z")),
    )
    .expect("ours");
    assert!(git(&root, &["commit", "-qam", "ours"]).status.success());

    let merged = git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "merge",
            "--no-edit",
            "theirs",
        ],
    );
    assert!(
        merged.status.success(),
        "the git merge through the bare driver command must succeed: {}\n{}",
        text(&merged.stdout),
        text(&merged.stderr)
    );
    // Which executable produced the landed bytes: the same three inputs, run
    // through the ACTUAL Native binary directly, must produce exactly the bytes
    // Git's bare driver command produced.
    let expected_dir = sandbox.path().join("expected");
    fs::create_dir_all(&expected_dir).expect("expected dir");
    fs::write(expected_dir.join("base"), meta_json("base", None)).expect("base");
    fs::write(
        expected_dir.join("ours"),
        meta_json("ours", Some("2026-11-02T10:00:00Z")),
    )
    .expect("ours");
    fs::write(expected_dir.join("theirs"), meta_json("theirs", None)).expect("theirs");
    let direct = run(
        Path::new(binary()),
        &[
            "merge-driver-meta",
            &expected_dir.join("base").display().to_string(),
            &expected_dir.join("ours").display().to_string(),
            &expected_dir.join("theirs").display().to_string(),
        ],
        &root,
        &env,
    );
    assert!(direct.status.success(), "{}", text(&direct.stderr));
    let driver_bytes = fs::read(expected_dir.join("ours")).expect("driver output");
    let landed_bytes = fs::read(root.join("meta.json")).expect("landed meta");
    assert_eq!(
        landed_bytes, driver_bytes,
        "the git-invoked driver must be the actual Native binary (its own output bytes)"
    );
    let landed: serde_json::Value = serde_json::from_slice(&landed_bytes).expect("json");
    assert_eq!(landed["mission_number"], serde_json::json!(3));
    assert_eq!(
        landed["accepted_at"],
        serde_json::json!("2026-11-02T10:00:00Z")
    );
    let unmerged = git(&root, &["ls-files", "-u"]);
    assert!(
        text(&unmerged.stdout).trim().is_empty(),
        "the merge must leave no unmerged path: {}",
        text(&unmerged.stdout)
    );
}
