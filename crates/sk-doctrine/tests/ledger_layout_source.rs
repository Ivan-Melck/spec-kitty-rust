//! Source-first canonical ledger block indentation; Native lossless reads stay distinct.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CHARTER: &str = "governance:\n  charter:\n    selected_directives:\n    - 001-mission-type-scope-directive\ncatalog:\n  mission: software-dev\n  references: []\nmetadata:\n  bundle_schema_version: 2\n";
const INPUTS: &[&str] = &[
    "activated_directives:\n- 001-mission-type-scope-directive\ndeactivated_directives:\n- 001-mission-type-scope-directive\n",
    "activated_directives:\n  - 001-mission-type-scope-directive\ndeactivated_directives:\n  - 001-mission-type-scope-directive\n",
    "activated_directives: [001-mission-type-scope-directive]\ndeactivated_directives: [001-mission-type-scope-directive]\n",
];

fn environment_paths() -> [(&'static str, &'static str); 9] {
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
        .env("SK_LEDGER_ROOT", root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"));
    for (key, name) in environment_paths() {
        command.env(key, root.join(name));
    }
    command
}

#[test]
fn canonical_ledger_replacements_match_source_indentless_bytes() {
    if let Some(root) = std::env::var_os("SK_LEDGER_NATIVE_CHILD") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (key, name) in environment_paths() {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
        }
        let expected: Vec<String> =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        for (index, input) in INPUTS.iter().enumerate() {
            let project = root.join(format!("native-{index}"));
            let directory = project.join(".kittify/charter");
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("charter.yaml"), CHARTER).unwrap();
            let path = directory.join("activation.yaml");
            fs::write(&path, input).unwrap();
            let read = sk_doctrine::YamlDocument::open(&path).unwrap();
            assert_eq!(
                read.to_text(),
                *input,
                "Native read-only format is lossless"
            );
            assert!(!read.changed());
            let values = BTreeMap::from([
                (
                    "activated_directives".to_owned(),
                    Some(vec!["001-project-intent-directive".to_owned()]),
                ),
                (
                    "deactivated_directives".to_owned(),
                    Some(vec!["001-project-intent-directive".to_owned()]),
                ),
            ]);
            let prepared = sk_doctrine::prepare_ledger_write(
                &project,
                &values,
                &[],
                Some(sk_doctrine::DenyListChoice::Rename),
            )
            .unwrap();
            sk_doctrine::apply_ledger_write(prepared).unwrap();
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                expected[index],
                "authored style {index}"
            );
        }
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in environment_paths() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let source = command(&root, &repo, oracle::python()).args(["-c", r#"
import json, os, sys
from pathlib import Path
root=Path(os.environ['SK_LEDGER_ROOT'])
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for key,name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]: assert Path(os.environ[key])==root/name
from charter.activation.api import rename_legacy_project_intent_directive
charter='governance:\n  charter:\n    selected_directives:\n    - 001-mission-type-scope-directive\ncatalog:\n  mission: software-dev\n  references: []\nmetadata:\n  bundle_schema_version: 2\n'
inputs=[
'activated_directives:\n- 001-mission-type-scope-directive\ndeactivated_directives:\n- 001-mission-type-scope-directive\n',
'activated_directives:\n  - 001-mission-type-scope-directive\ndeactivated_directives:\n  - 001-mission-type-scope-directive\n',
'activated_directives: [001-mission-type-scope-directive]\ndeactivated_directives: [001-mission-type-scope-directive]\n']
outputs=[]
for index,text in enumerate(inputs):
    project=root/f'source-{index}'; directory=project/'.kittify/charter'; directory.mkdir(parents=True)
    (directory/'charter.yaml').write_text(charter)
    path=directory/'activation.yaml'; path.write_text(text)
    rename_legacy_project_intent_directive(project,dry_run=False)
    output=path.read_text()
    assert output=='activated_directives:\n- 001-project-intent-directive\ndeactivated_directives:\n- 001-project-intent-directive\n', repr(output)
    outputs.append(output)
assert len(set(outputs))==1
print(json.dumps(outputs))
"#]).output().unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source assertions FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    let native = command(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "canonical_ledger_replacements_match_source_indentless_bytes",
            "--nocapture",
        ])
        .env("SK_LEDGER_NATIVE_CHILD", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    assert!(
        native.status.success(),
        "Native bytes: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}
