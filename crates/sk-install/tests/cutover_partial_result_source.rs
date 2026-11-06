//! Actual Source cutover abort after one move. File existence is not a Source
//! PartialWrite record; this checks that narrow result contract and footprint.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn roots() -> [(&'static str, &'static str); 9] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}
fn command(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
    command
}
#[test]
fn completed_move_before_abort_does_not_invent_source_partial_write_records() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_CUTOVER_PARTIAL_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (key, name) in roots() {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
        }
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let project = root.join("native-project");
        for (relative, data) in [
            (
                ".kittify/doctrine/directives/001-first.directive.yaml",
                "id: FIRST\n",
            ),
            (
                ".kittify/doctrine/tactics/999-last.tactic.yaml",
                "id: LAST\n",
            ),
            (".kittify/charter-packs/tactics", "operator file\n"),
        ] {
            let path = project.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, data).unwrap();
        }
        let outcome =
            sk_install::migrations::completion_modern::charter_synthesis::cutover::apply(&project)
                .unwrap();
        assert!(!outcome.success);
        assert!(
            outcome.changed,
            "Native convenience retains actual partial change"
        );
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(
            fs::read_to_string(
                project.join(".kittify/charter-packs/directives/001-first.directive.yaml")
            )
            .unwrap(),
            "id: FIRST\n"
        );
        assert!(
            !project
                .join(".kittify/doctrine/directives/001-first.directive.yaml")
                .exists()
        );
        assert!(
            project
                .join(".kittify/doctrine/tactics/999-last.tactic.yaml")
                .is_file()
        );
        let report: serde_json::Value = serde_json::from_str(&outcome.changes[0]).unwrap();
        assert_eq!(report["moved"].as_array().unwrap().len(), 1);
        assert_eq!(
            serde_json::to_value(&outcome.partial_writes).unwrap(),
            expected["result"]["partial_writes"],
            "never infer Source records from an existing destination or assign an empty mission owner"
        );
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    eprintln!("cutover partial-result evidence: {}", root.display());
    let source=command(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
from dataclasses import asdict
root=Path.cwd();assert sys.version_info[:3]==(3,11,15)
for key,name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]: assert Path(os.environ[key])==root/name
from specify_cli.upgrade.migrations.m_4_0_0rc6_charter_pack_cutover import CharterPackCutoverMigration
project=root/'source-project'
for relative,data in [('.kittify/doctrine/directives/001-first.directive.yaml','id: FIRST\n'),('.kittify/doctrine/tactics/999-last.tactic.yaml','id: LAST\n'),('.kittify/charter-packs/tactics','operator file\n')]:
    p=project/relative;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(data)
result=CharterPackCutoverMigration().apply(project)
assert not result.success and len(result.errors)==1,asdict(result)
assert (project/'.kittify/charter-packs/directives/001-first.directive.yaml').read_text()=='id: FIRST\n'
assert not (project/'.kittify/doctrine/directives/001-first.directive.yaml').exists()
assert (project/'.kittify/doctrine/tactics/999-last.tactic.yaml').is_file()
report=json.loads(result.changes_made[0]);assert len(report['moved'])==1 and len(report['errors'])==1
assert result.partial_writes==[],asdict(result)
print(json.dumps({'result':asdict(result),'report':report}))
"#]).output().unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source first: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    let native = command(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "completed_move_before_abort_does_not_invent_source_partial_write_records",
            "--nocapture",
        ])
        .env("SK_CUTOVER_PARTIAL_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}
