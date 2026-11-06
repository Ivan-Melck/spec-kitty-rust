//! End-to-end tests for the full native `spec-kitty init` delivery.
//!
//! Every test executes the real binary in a temporary project with a temporary
//! `HOME`/`XDG_STATE_HOME`/`TMPDIR`, so nothing touches this checkout or the
//! user's home directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_spec-kitty")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn packs_root() -> PathBuf {
    repo_root().join("packs/built-in")
}

struct Sandbox {
    project: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            project: tempfile::tempdir().expect("project"),
            home: tempfile::tempdir().expect("home"),
        }
    }

    fn root(&self) -> &Path {
        self.project.path()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(binary());
        command
            .current_dir(self.root())
            .env("HOME", self.home.path())
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("TMPDIR", self.home.path().join("tmp"))
            .env("SPEC_KITTY_PACKS_ROOT", packs_root())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run spec-kitty")
    }

    fn init(&self, agents: &str) -> Output {
        self.run(&["init", "--ai", agents, "--non-interactive"])
    }

    fn file(&self, relative: &str) -> PathBuf {
        self.root().join(relative)
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.file(relative))
            .unwrap_or_else(|error| panic!("read {relative}: {error}"))
    }
}

fn assert_success(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn init_delivers_the_full_surface_and_leaves_no_residue() {
    let sandbox = Sandbox::new();
    assert_success(&sandbox.init("claude,codex,vibe"));

    // Project scaffold and template tree.
    for relative in [
        ".kittify/config.yaml",
        ".kittify/metadata.yaml",
        ".kittify/missions/README.md",
        ".kittify/missions/software-dev/mission.yaml",
        ".kittify/memory/README.md",
        ".kittify/charter/activation.yaml",
        ".kittify/canonical-events.jsonl",
        ".kittify/migration-ledger.jsonl",
        ".kittify/skills-manifest.json",
        ".kittify/command-skills-manifest.json",
        ".kittify/agent_profiles_manifest.json",
        ".gitattributes",
        ".gitignore",
        ".vibe/config.toml",
    ] {
        assert!(sandbox.file(relative).exists(), "missing {relative}");
    }
    // Current upstream surface: command skills, not slash-command files.
    assert!(
        sandbox
            .file(".agents/skills/spec-kitty-specify/SKILL.md")
            .is_file()
    );
    assert!(!sandbox.file(".claude/commands").exists());
    // Charter skills in both roots.
    assert!(
        sandbox
            .file(".claude/skills/adversarial-squad/SKILL.md")
            .is_file()
    );
    assert!(
        sandbox
            .file(".agents/skills/adversarial-squad/SKILL.md")
            .is_file()
    );
    // Agent profiles and session presence.
    assert!(sandbox.file(".claude/agents/analyst-annie.md").is_file());
    assert!(sandbox.file(".codex/agents/analyst-annie.toml").is_file());
    assert!(sandbox.file(".claude/CLAUDE.md").is_file());
    assert!(sandbox.file("AGENTS.md").is_file());
    assert!(sandbox.file(".claude/settings.json").is_file());
    assert!(sandbox.read("AGENTS.md").contains("spec-kitty:orientation"));

    // No project-scoped lock leftover and no checkout-mode `.claudeignore`.
    assert!(!sandbox.file(".kittify/.install.lock").exists());
    assert!(!sandbox.file(".claudeignore").exists());
    assert!(!sandbox.file(".kittify/spec-kitty-locks").exists());

    // Identity, event and ledger are all present and consistent.
    let config = sandbox.read(".kittify/config.yaml");
    assert!(config.starts_with("project:\n"));
    assert!(config.contains("vcs:\n  type: git\n"));
    assert!(config.contains("agents:\n  available:\n  - claude\n  - codex\n  - vibe\n"));
    let event: serde_json::Value =
        serde_json::from_str(sandbox.read(".kittify/canonical-events.jsonl").trim()).expect("json");
    assert_eq!(event["event_type"], "ProjectInitialized");
    assert_eq!(event["schema_version"], "5.0.0");
    assert!(
        event["payload"]["initialized_at"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    let ledger_lines = sandbox
        .read(".kittify/migration-ledger.jsonl")
        .lines()
        .count();
    assert_eq!(ledger_lines, 124);
    let skills: serde_json::Value =
        serde_json::from_str(&sandbox.read(".kittify/skills-manifest.json")).expect("json");
    assert_eq!(skills["spec_kitty_version"], "4.0.0rc8");
    assert_eq!(skills["entries"].as_array().unwrap().len(), 222);
    let profiles: serde_json::Value =
        serde_json::from_str(&sandbox.read(".kittify/agent_profiles_manifest.json")).expect("json");
    assert_eq!(profiles["entries"].as_array().unwrap().len(), 50);
    assert_eq!(profiles["schema_version"], 1);

    // A second run is a no-op refresh with the same outcome class.
    let second = sandbox.init("claude,codex,vibe");
    assert_success(&second);
    let text = String::from_utf8_lossy(&second.stdout).to_lowercase();
    assert!(text.contains("already initialized"), "output: {text}");
    assert_eq!(
        sandbox
            .read(".kittify/migration-ledger.jsonl")
            .lines()
            .count(),
        124
    );
}

#[test]
fn concurrent_fresh_inits_serialize_without_backup_residue() {
    let sandbox = Sandbox::new();
    // Both processes start together (no sleep, no polling): the canonical
    // project lock must serialize them, and the second run must observe the
    // first run's `config.yaml` and refresh instead of re-scaffolding.
    let first = sandbox
        .command(&["init", "--ai", "claude,codex,vibe", "--non-interactive"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn first init");
    let second = sandbox
        .command(&["init", "--ai", "claude,codex,vibe", "--non-interactive"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn second init");
    let first_output = first.wait_with_output().expect("first init");
    let second_output = second.wait_with_output().expect("second init");
    assert_success(&first_output);
    assert_success(&second_output);

    let first_text = String::from_utf8_lossy(&first_output.stdout).to_lowercase();
    let second_text = String::from_utf8_lossy(&second_output.stdout).to_lowercase();
    let initialized = [&first_text, &second_text]
        .iter()
        .filter(|text| text.contains("initialized spec kitty project"))
        .count();
    let refreshed = [&first_text, &second_text]
        .iter()
        .filter(|text| text.contains("already initialized"))
        .count();
    assert_eq!(initialized, 1, "first: {first_text}\nsecond: {second_text}");
    assert_eq!(refreshed, 1, "first: {first_text}\nsecond: {second_text}");

    // The loser never moves the winner's freshly copied trees aside.
    let backups: Vec<String> = fs::read_dir(sandbox.file(".kittify"))
        .expect("read .kittify")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".backup-"))
        .collect();
    assert!(backups.is_empty(), "backup residue: {backups:?}");
    assert!(
        !sandbox
            .file(".kittify/init-command-skills.pending.json")
            .exists()
    );
    assert_eq!(
        sandbox
            .read(".kittify/migration-ledger.jsonl")
            .lines()
            .count(),
        124
    );
    let events = sandbox.read(".kittify/canonical-events.jsonl");
    assert_eq!(events.lines().count(), 1, "events: {events}");
}

#[test]
fn a_slash_command_agent_receives_the_charter_skill_catalog_not_command_files() {
    // Pinned-source contract: `init` installs the charter skill catalog into each
    // agent's native root and never generates slash-command files; those are
    // owned by `spec-kitty upgrade` migrations. This is the corrected expectation
    // for the coordinator-owned `tests/compat.rs` slash-command case.
    let sandbox = Sandbox::new();
    assert_success(&sandbox.init("claude"));
    assert!(sandbox.file(".claude/skills/spec-kitty/SKILL.md").is_file());
    assert!(
        sandbox
            .file(".claude/skills/adversarial-squad/SKILL.md")
            .is_file()
    );
    assert!(sandbox.file(".claude/agents/analyst-annie.md").is_file());
    assert!(sandbox.file(".claude/CLAUDE.md").is_file());
    assert!(
        !sandbox
            .file(".claude/commands/spec-kitty.specify.md")
            .exists()
    );
    assert!(!sandbox.file(".claude/commands").exists());
}

#[test]
fn the_documented_packs_root_shape_locates_the_built_in_pack() {
    // Reproduces the documented `SPEC_KITTY_PACKS_ROOT=$PWD/packs` convention: a
    // checkout whose own `packs/` tree is absent must resolve missions,
    // presets and agent profiles from the configured packs root.
    let sandbox = Sandbox::new();
    let checkout = tempfile::tempdir().expect("checkout");
    let offering = checkout.path().join("src/charter/offering");
    fs::create_dir_all(&offering).expect("offering");
    for name in ["templates", "skills"] {
        let status = Command::new("cp")
            .args([
                "-R",
                repo_root()
                    .join("src/charter/offering")
                    .join(name)
                    .to_str()
                    .expect("path"),
                offering.to_str().expect("path"),
            ])
            .status()
            .expect("copy offering assets");
        assert!(status.success(), "cp -R {name} failed");
    }
    let output = Command::new(binary())
        .current_dir(sandbox.root())
        .env("HOME", sandbox.home.path())
        .env("XDG_STATE_HOME", sandbox.home.path().join("state"))
        .env("TMPDIR", sandbox.home.path().join("tmp"))
        .env("SPEC_KITTY_TEMPLATE_ROOT", checkout.path())
        .env("SPEC_KITTY_PACKS_ROOT", repo_root().join("packs"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["init", "--ai", "codex", "--non-interactive"])
        .output()
        .expect("run spec-kitty");
    assert_success(&output);
    assert!(
        sandbox
            .file(".kittify/missions/software-dev/mission.yaml")
            .is_file()
    );
    assert!(sandbox.file(".kittify/charter/activation.yaml").is_file());
}

#[test]
fn init_outside_git_seeds_no_clone_local_records_and_creates_no_git_state() {
    let sandbox = Sandbox::new();
    assert_success(&sandbox.init("codex"));
    assert!(!sandbox.file(".git").exists());
    assert!(
        !sandbox
            .file(".kittify/migrations/clone-local.jsonl")
            .exists()
    );
}

#[test]
fn init_inside_a_git_repo_seeds_proven_clone_local_records_and_wires_drivers() {
    let sandbox = Sandbox::new();
    git(sandbox.root(), &["init", "-q", "-b", "main"]);
    assert_success(&sandbox.init("claude,codex,vibe"));
    let clone_local = sandbox.read(".kittify/migrations/clone-local.jsonl");
    assert_eq!(clone_local.lines().count(), 8);
    let config = sandbox.read(".git/config");
    assert!(config.contains("[merge \"spec-kitty-event-log\"]"));
    assert!(config.contains("driver = spec-kitty merge-driver-event-log %O %A %B"));
    assert_eq!(
        sandbox
            .read(".kittify/migration-ledger.jsonl")
            .lines()
            .count(),
        124
    );
}

#[test]
fn a_pending_delivery_is_completed_instead_of_reported_as_success() {
    let sandbox = Sandbox::new();
    assert_success(&sandbox.init("claude,codex,vibe"));
    for relative in [
        ".kittify/charter",
        ".kittify/canonical-events.jsonl",
        ".kittify/migration-ledger.jsonl",
        ".kittify/skills-manifest.json",
        ".kittify/agent_profiles_manifest.json",
        ".claude/skills",
        ".claude/agents",
        ".claude/settings.json",
        ".claude/CLAUDE.md",
        "AGENTS.md",
    ] {
        let path = sandbox.file(relative);
        if path.is_dir() {
            fs::remove_dir_all(path).expect("remove dir");
        } else if path.exists() {
            fs::remove_file(path).expect("remove file");
        }
    }
    fs::write(
        sandbox.file(".kittify/init-command-skills.pending.json"),
        "{\"schema_version\": 1, \"agents\": [\"codex\", \"vibe\"]}\n",
    )
    .expect("pending");
    let output = sandbox.init("claude,codex,vibe");
    assert_success(&output);
    let text = String::from_utf8_lossy(&output.stdout).to_lowercase();
    assert!(text.contains("resumed"), "output: {text}");
    assert!(
        !sandbox
            .file(".kittify/init-command-skills.pending.json")
            .exists()
    );
    for relative in [
        ".kittify/charter/activation.yaml",
        ".kittify/canonical-events.jsonl",
        ".kittify/migration-ledger.jsonl",
        ".kittify/skills-manifest.json",
        ".kittify/agent_profiles_manifest.json",
        ".claude/agents/analyst-annie.md",
        ".claude/settings.json",
        "AGENTS.md",
    ] {
        assert!(sandbox.file(relative).exists(), "missing {relative}");
    }
}

#[test]
fn a_mismatched_pending_selection_is_refused_before_mutation() {
    let sandbox = Sandbox::new();
    assert_success(&sandbox.init("claude,codex,vibe"));
    fs::write(
        sandbox.file(".kittify/init-command-skills.pending.json"),
        "{\"schema_version\": 1, \"agents\": [\"codex\"]}\n",
    )
    .expect("pending");
    let output = sandbox.run(&["init", "--ai", "vibe", "--non-interactive"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(stderr.contains("interrupted delivery"), "stderr: {stderr}");
}
