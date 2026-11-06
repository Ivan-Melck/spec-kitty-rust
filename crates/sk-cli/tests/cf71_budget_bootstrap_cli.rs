//! CF71 prospective guarded Source CLI-handler / Native CLI-facade control.
//! The former subprocess launchers were executable smoke, not in-business-child
//! assertions, and shared a reset fixture. This version uses independent trees
//! and actual business children. It compares substitution predicates only, NOT
//! whole text, length, warning count, console transport or binary parity.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
const ROOT_ENV: &str = "CF71_CLI_ROOT";
const REPO_ENV: &str = "CF71_CLI_REPO";
const BIG_BODY: &str = "the canonical terminology rule is applied to every reviewed change";
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn env_roots() -> [(&'static str, &'static str); 11] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CONFIG_DIRS", "config_dirs"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_DATA_DIRS", "data_dirs"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}
fn child_command(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .env(ROOT_ENV, root)
        .env(REPO_ENV, repo);
    for (key, name) in env_roots() {
        c.env(key, root.join(name));
    }
    c
}
fn assert_child_environment(root: &Path, repo: &Path) {
    assert!(repo.is_absolute());
    assert_eq!(std::env::current_dir().unwrap(), root);
    assert_eq!(
        std::env::var_os(REPO_ENV).map(PathBuf::from),
        Some(repo.to_path_buf())
    );
    for (key, name) in env_roots() {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key}"
        );
    }
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(repo.join("src"))
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repo.join("packs"))
    );
    assert_eq!(std::env::var("PYTHONHASHSEED").as_deref(), Ok("0"));
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
}
fn fixture() -> PathBuf {
    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in env_roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let project = root.join("project");
    fs::create_dir_all(project.join(".kittify/charter")).unwrap();
    fs::write(project.join(".kittify/config.yaml"), "# CF71 fixture\n").unwrap();
    fs::write(project.join(".kittify/charter/charter.yaml"),"schema_version: \"1.1.0\"\ncatalog:\n  references: []\ngovernance:\n  authority_paths: [docs/context]\n").unwrap();
    let big = format!("{BIG_BODY} ").repeat(800);
    fs::write(project.join(".kittify/charter/charter.md"),format!("# CF71 Fixture Charter\n\n## Terminology Canon\n\n{big}\n\n## Code Review Checklist\n\n- review every changed line\n\n## Regression Vigilance\n\n- keep the anchors inline\n")).unwrap();
    assert!(
        Command::new("git")
            .current_dir(&project)
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap()
            .success()
    );
    root
}
const SOURCE: &str = r#"
import contextlib, importlib, io, os, sys
from pathlib import Path
root=Path(os.environ['CF71_CLI_ROOT'])
repo=Path(os.environ['CF71_CLI_REPO'])
assert repo.is_absolute()
assert Path.cwd()==root
for key,name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CONFIG_DIRS','config_dirs'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_DATA_DIRS','data_dirs'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
 assert Path(os.environ[key])==root/name
assert Path(os.environ['PYTHONPATH'])==repo/'src'
assert Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
assert os.environ['PYTHONHASHSEED']=='0'
assert sys.version_info[:3]==(3,11,15)
assert sys.getfilesystemencoding()=='utf-8'
assert sys.getfilesystemencodeerrors()=='surrogateescape'
module=importlib.import_module('specify_cli.cli.commands.charter.context')
assert Path(module.__file__).resolve()==repo/'src/specify_cli/cli/commands/charter/context.py'
os.chdir(root/'project')
assert Path.cwd()==root/'project'
out,err=io.StringIO(),io.StringIO()
with contextlib.redirect_stdout(out),contextlib.redirect_stderr(err):
 module.context(owned_checkout=None,mission=None,action='implement',include=None,mark_loaded=True,profile=None,mission_type=None,json_output=False,include_all=False)
(root/'source.stdout').write_text(out.getvalue(),encoding='utf-8')
(root/'source.stderr').write_text(err.getvalue(),encoding='utf-8')
"#;
fn assert_budgeted(output: &str, label: &str, keeps_small: bool) {
    assert!(
        !output.contains(BIG_BODY),
        "{label}: oversized body must be substituted"
    );
    assert!(
        output.contains("# Governance payload: ")
            && output.contains(" sections substituted with fetch commands (budget=40000)."),
        "{label}: real substitution warning"
    );
    assert!(
        output.contains("--include section:terminology-canon"),
        "{label}: recovery rail"
    );
    if keeps_small {
        assert!(output.contains("- review every changed line"));
    }
}
#[test]
fn oversized_critical_section_is_budgeted_by_the_cli_facade() {
    let repo = repo();
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        let root = PathBuf::from(root);
        assert_child_environment(&root, &repo);
        let project = root.join("project");
        std::env::set_current_dir(&project).unwrap();
        assert_eq!(std::env::current_dir().unwrap(), project);
        let result = sk_cli::compat::charter::execute_context(
            &project,
            "implement",
            None,
            None,
            false,
            true,
            false,
        )
        .unwrap();
        fs::write(
            root.join("native.stdout"),
            result.text.to_scalar_string().expect("scalar fixture"),
        )
        .unwrap();
        return;
    }
    let source_root = fixture();
    let native_root = fixture();
    let source = child_command(&source_root, &repo, oracle::python())
        .arg("-c")
        .arg(SOURCE)
        .output()
        .unwrap();
    fs::write(source_root.join("source.launcher.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source: {} {}",
        String::from_utf8_lossy(&source.stdout),
        String::from_utf8_lossy(&source.stderr)
    );
    assert_budgeted(
        &fs::read_to_string(source_root.join("source.stdout")).unwrap(),
        "Source",
        true,
    );
    let native = child_command(&native_root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "oversized_critical_section_is_budgeted_by_the_cli_facade",
            "--nocapture",
        ])
        .output()
        .unwrap();
    fs::write(native_root.join("native.launcher.stdout"), &native.stdout).unwrap();
    fs::write(native_root.join("native.launcher.stderr"), &native.stderr).unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
    assert_budgeted(
        &fs::read_to_string(native_root.join("native.stdout")).unwrap(),
        "Native",
        false,
    );
}
