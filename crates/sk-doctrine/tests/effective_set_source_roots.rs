//! E5 F8/F9: Source lexical project roots differ from contained inspection,
//! and package resources must not require the project's ancestor/env layout.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use std::{fs, path::Path, process::Command};
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
fn child(root: &Path, repo: &Path, case: &str, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_SOURCE_ROOT", root)
        .env("SK_SOURCE_REPO", repo)
        .env("SK_ROOT_CASE", case)
        .env("PYTHONPATH", repo.join("src"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    if case != "noenv" {
        c.env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"));
    }
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn source_roots_accept_lexical_symlink_and_package_anchor() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_ROOT_NATIVE") {
        let root = std::path::PathBuf::from(root);
        let case = std::env::var("SK_ROOT_CASE").unwrap();
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (k, n) in roots() {
            assert_eq!(std::env::var_os(k).unwrap(), root.join(n));
        }
        assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
        if case == "noenv" {
            assert!(std::env::var_os("SPEC_KITTY_PACKS_ROOT").is_none());
        } else {
            assert_eq!(
                std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
                repo.join("packs")
            );
        }
        let project = root.join("project");
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let result = sk_doctrine::resolve_effective_sets(
            &project,
            &["activated_directives"],
            &sk_doctrine::NATIVE_OFFERING_SERVICE,
        )
        .unwrap();
        let result = &result[0];
        assert_eq!(
            result.resolved,
            expected["resolved"].as_bool().unwrap(),
            "{case}: {:?}",
            result.reason
        );
        assert_eq!(
            serde_json::to_value(result.scalar_ids().unwrap()).unwrap(),
            expected["ids"],
            "{case} ids"
        );
        assert_eq!(
            serde_json::to_value(result.scalar_fallback_ids().unwrap()).unwrap(),
            expected["fallback"],
            "{case} readable fallback"
        );
        assert_eq!(
            fs::read(project.join(".kittify/charter-packs/directive/900-local.directive.yaml"))
                .unwrap(),
            b"id: LOCAL_POLICY\n"
        );
        return;
    }
    for case in ["symlink", "symlink-malformed", "noenv"] {
        let root = tempfile::tempdir().unwrap().keep();
        for (_, n) in roots() {
            fs::create_dir_all(root.join(n)).unwrap();
        }
        eprintln!("source roots {case}: {}", root.display());
        let source=child(&root,&repo,case,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_SOURCE_ROOT']);repo=Path(os.environ['SK_SOURCE_REPO']);case=os.environ['SK_ROOT_CASE']
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
assert Path(os.environ['PYTHONPATH'])==repo/'src'
if case=='noenv':assert 'SPEC_KITTY_PACKS_ROOT' not in os.environ
else:assert Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
from charter.offering.pack_paths import built_in_dir
from charter.offering.artifact_kinds import ArtifactKind
assert built_in_dir(ArtifactKind.DIRECTIVE).resolve()==(repo/'packs/built-in/directives').resolve()
from charter.activation.effective_set import resolve_effective_sets
project=root/'project';(project/'.kittify').mkdir(parents=True)
if case.startswith('symlink'):
 external=root/'external-pack';(external/'directive').mkdir(parents=True)
 (project/'.kittify/charter-packs').symlink_to(external,target_is_directory=True)
else:(project/'.kittify/charter-packs/directive').mkdir(parents=True)
artifact=project/'.kittify/charter-packs/directive/900-local.directive.yaml';artifact.write_text('id: LOCAL_POLICY\n')
if case=='symlink-malformed':(project/'.kittify/config.yaml').write_text('charter_packs: [broken\n')
entry=resolve_effective_sets(project,['activated_directives'])['activated_directives']
assert entry.resolved==(case!='symlink-malformed'),entry
assert '900-local' in (entry.ids if entry.resolved else entry.fallback_ids),entry
assert artifact.read_text()=='id: LOCAL_POLICY\n'
print(json.dumps(dict(resolved=entry.resolved,ids=sorted(entry.ids),fallback=sorted(entry.fallback_ids))))
"#]).output().unwrap();
        fs::write(root.join("source.stdout"), &source.stdout).unwrap();
        fs::write(root.join("source.stderr"), &source.stderr).unwrap();
        assert!(
            source.status.success(),
            "Source FIRST {case}: {}",
            String::from_utf8_lossy(&source.stderr)
        );
        let native = child(&root, &repo, case, std::env::current_exe().unwrap())
            .args([
                "--exact",
                "source_roots_accept_lexical_symlink_and_package_anchor",
                "--nocapture",
            ])
            .env("SK_ROOT_NATIVE", &root)
            .output()
            .unwrap();
        fs::write(root.join("native.stdout"), &native.stdout).unwrap();
        fs::write(root.join("native.stderr"), &native.stderr).unwrap();
        assert!(
            native.status.success(),
            "Native {case}: {} {}",
            String::from_utf8_lossy(&native.stdout),
            String::from_utf8_lossy(&native.stderr)
        );
    }
}
