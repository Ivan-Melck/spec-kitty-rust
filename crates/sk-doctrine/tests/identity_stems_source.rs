//! Identity-table enumeration, not a claim of effective-set resolution.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use sk_charter::ArtifactKind;
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
        .env("SK_IDENTITY_ROOT", root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"));
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
    command
}
#[test]
fn stem_enumeration_matches_source_insertion_order_without_rescanning() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_IDENTITY_NATIVE_CHILD") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (key, name) in roots() {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
        }
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let index = sk_doctrine::IdentityIndex::from_roots(
            &repo.join("packs/built-in"),
            &[root.join("org-first"), root.join("org-last")],
            Some(&root.join("project")),
        );
        assert_eq!(
            serde_json::to_value(index.stems(ArtifactKind::Directive)).unwrap(),
            expected["layered"]
        );
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "same"),
            Some("PROJECT_SAME".to_owned())
        );
        // Removing an indexed file proves enumeration borrows the held table,
        // rather than implementing a second filesystem scan.
        fs::remove_file(root.join("project/directive/z-project.directive.yaml")).unwrap();
        assert_eq!(
            serde_json::to_value(index.stems(ArtifactKind::Directive)).unwrap(),
            expected["layered"]
        );
        let builtin =
            sk_doctrine::IdentityIndex::from_roots(&repo.join("packs/built-in"), &[], None);
        for (kind, key) in [
            (ArtifactKind::Directive, "builtin_directives"),
            (ArtifactKind::Tactic, "builtin_tactics"),
        ] {
            assert_eq!(
                serde_json::to_value(builtin.stems(kind)).unwrap(),
                expected[key]
            );
        }
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let source=command(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_IDENTITY_ROOT']); assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for key,name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]: assert Path(os.environ[key])==root/name
from charter.activation.identity_index import IdentityIndex
from charter.offering.artifact_kinds import ArtifactKind
repo=Path(os.environ['SPEC_KITTY_PACKS_ROOT']).parent
for layer,entries in [('project',[('same','PROJECT_SAME'),('z-project','PROJECT_Z')]),('org-last',[('same','LAST_SAME'),('a-last','LAST_A')]),('org-first',[('same','FIRST_SAME'),('a-first','FIRST_A')])]:
    for stem,declared in entries:
        directory='directive' if layer=='project' else 'directives'
        path=root/layer/directory/f'{stem}.directive.yaml'; path.parent.mkdir(parents=True,exist_ok=True); path.write_text(f'id: {declared}\n')
index=IdentityIndex(org_roots=[root/'org-first',root/'org-last'],project_root=root/'project',offering_root=repo/'packs/built-in')
ordered=list(index._table(ArtifactKind.DIRECTIVE).by_stem)
assert ordered[:4]==['same','z-project','a-last','a-first'],ordered
assert index.declared_id(ArtifactKind.DIRECTIVE,'same')=='PROJECT_SAME'
repo=Path(os.environ['SPEC_KITTY_PACKS_ROOT']).parent
builtin=IdentityIndex(org_roots=[],offering_root=repo/'packs/built-in')
result={'layered':ordered,'builtin_directives':list(builtin._table(ArtifactKind.DIRECTIVE).by_stem),'builtin_tactics':list(builtin._table(ArtifactKind.TACTIC).by_stem)}
assert result['builtin_directives'] and result['builtin_tactics']
print(json.dumps(result))
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
            "stem_enumeration_matches_source_insertion_order_without_rescanning",
            "--nocapture",
        ])
        .env("SK_IDENTITY_NATIVE_CHILD", &root)
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
