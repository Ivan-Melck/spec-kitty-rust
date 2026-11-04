//! Shared test support: the fail-closed development oracle, record builders,
//! and isolated project/Git fixtures.
//!
//! Never production code. The oracle selection is lock-exact and fails closed:
//! a missing or drifted `.venv-rust-oracle` panics instead of turning a
//! comparison test into a passing zero-case result.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use sk_invocation::{
    ClosedBy, InvocationId, ModeOfWork, OpCompletedEvent, OpCompletedFields, OpOutcome,
    OpStartedEvent, OpStartedFields,
};

/// The repository root (`spec-kitty-rust/`).
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root resolves")
}

/// The verified Python development oracle (`.venv-rust-oracle`, Python 3.11).
pub fn python() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let root = repository_root();
            let executable = if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            };
            let python = std::env::var_os("SK_INVOCATION_PYTHON")
                .or_else(|| std::env::var_os("SK_KERNEL_PYTHON"))
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join(".venv-rust-oracle").join(executable));
            assert!(
                python.is_file(),
                "required Python oracle is missing at {}; provision .venv-rust-oracle with \
                 uv sync --frozen --no-dev --python 3.11 \
                 (UV_PROJECT_ENVIRONMENT=.venv-rust-oracle)",
                python.display()
            );
            let scratch = tempfile::tempdir().expect("isolated oracle verification CWD");
            let sandbox = scratch.path();
            for directory in ["home", "cache", "config", "data", "state", "tmp"] {
                std::fs::create_dir_all(sandbox.join(directory))
                    .expect("create isolated verifier root");
            }
            let result = Command::new(&python)
                .current_dir(sandbox)
                .env("HOME", sandbox.join("home"))
                .env("XDG_CACHE_HOME", sandbox.join("cache"))
                .env("XDG_CONFIG_HOME", sandbox.join("config"))
                .env("XDG_DATA_HOME", sandbox.join("data"))
                .env("XDG_STATE_HOME", sandbox.join("state"))
                .env("TMPDIR", sandbox.join("tmp"))
                // The closure check reads `uv.lock`/metadata as text; run the
                // verifier itself in a UTF-8 startup context so the locale of
                // the behaviour under test never turns a pinned, unmodified
                // oracle into a false failure. The product probe below still
                // inherits the caller's locale/PYTHONUTF8 context.
                .env("PYTHONUTF8", "1")
                .env("LC_ALL", "C.UTF-8")
                .env_remove("PYTHONCOERCECLOCALE")
                .arg(root.join("scripts/rust/verify_oracle.py"))
                .arg("--root")
                .arg(&root)
                .output()
                .expect("execute required Python oracle verifier");
            assert!(
                result.status.success(),
                "Python oracle is not lock-exact: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            python
        })
        .clone()
}

/// Run a Python snippet against the pinned source tree with optional stdin.
///
/// The probe runs in its own temporary CWD with its own `HOME`, every `XDG_*`
/// root and `TMPDIR`, so no operator configuration or scratch state leaks into
/// the reference behaviour it measures. The caller's locale/`PYTHONUTF8`
/// environment is inherited deliberately: it is part of the startup context
/// under test.
pub fn run_python(script: &str, stdin: Option<&str>) -> String {
    let root = repository_root();
    let scratch = tempfile::tempdir().expect("isolated Python probe CWD");
    let sandbox = scratch.path();
    for directory in ["home", "cache", "config", "data", "state", "tmp"] {
        std::fs::create_dir_all(sandbox.join(directory)).expect("create isolated probe root");
    }
    let mut child = Command::new(python())
        .arg("-c")
        .arg(script)
        .current_dir(sandbox)
        .env("HOME", sandbox.join("home"))
        .env("XDG_CACHE_HOME", sandbox.join("cache"))
        .env("XDG_CONFIG_HOME", sandbox.join("config"))
        .env("XDG_DATA_HOME", sandbox.join("data"))
        .env("XDG_STATE_HOME", sandbox.join("state"))
        .env("TMPDIR", sandbox.join("tmp"))
        .env("PYTHONPATH", root.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", root.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the pinned Python oracle");
    if let Some(stdin_text) = stdin {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("piped stdin")
            .write_all(stdin_text.as_bytes())
            .expect("write oracle stdin");
    }
    let output = child.wait_with_output().expect("wait for the oracle");
    assert!(
        output.status.success(),
        "oracle script failed: {}\nstderr: {}",
        script,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A `started` record with the reference fixture defaults.
pub fn started(invocation_id: &str) -> OpStartedEvent {
    started_with(invocation_id, |_| {})
}

/// A `started` record with a mutation hook for the optional fields.
pub fn started_with(
    invocation_id: &str,
    mutate: impl FnOnce(&mut OpStartedFields),
) -> OpStartedEvent {
    let mut fields = OpStartedFields {
        invocation_id: InvocationId::parse(invocation_id).expect("valid fixture id"),
        profile_id: "implementer-fixture".to_owned(),
        action: "implement".to_owned(),
        request_text: "implement the work package".to_owned(),
        actor: "claude".to_owned(),
        mode_of_work: ModeOfWork::TaskExecution,
        governance_context_hash: "abcdef0123456789".to_owned(),
        governance_context_available: true,
        router_confidence: None,
        started_at: "2026-04-21T10:00:00+00:00".to_owned(),
        mission_id: None,
        wp_id: None,
        model_id: None,
    };
    mutate(&mut fields);
    OpStartedEvent::new(fields).expect("valid fixture record")
}

/// A `completed` record with the reference fixture defaults.
pub fn completed(invocation_id: &str) -> OpCompletedEvent {
    completed_with(invocation_id, |_| {})
}

/// A `completed` record with a mutation hook.
pub fn completed_with(
    invocation_id: &str,
    mutate: impl FnOnce(&mut OpCompletedFields),
) -> OpCompletedEvent {
    let mut fields = OpCompletedFields {
        invocation_id: InvocationId::parse(invocation_id).expect("valid fixture id"),
        completed_at: "2026-04-21T10:05:00+00:00".to_owned(),
        outcome: OpOutcome::Done,
        closed_by: ClosedBy::Agent,
        evidence_ref: None,
    };
    mutate(&mut fields);
    OpCompletedEvent::new(fields).expect("valid fixture record")
}

/// Initialize a real Git repository with one empty commit, isolated from the
/// machine's Git configuration.
pub fn init_repository(path: &Path) {
    std::fs::create_dir_all(path).expect("create repository directory");
    git(path, &["init", "-b", "main"]);
    git(
        path,
        &[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=T",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
}

/// Run Git in `cwd` with a scratch HOME and no system/global config.
pub fn git(cwd: &Path, arguments: &[&str]) {
    let home = repository_root()
        .join("target")
        .join("sk-invocation-git-home");
    std::fs::create_dir_all(&home).expect("create scratch git home");
    let output = Command::new("git")
        .args(arguments)
        .current_dir(cwd)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("execute git");
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}{}",
        arguments,
        cwd.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
