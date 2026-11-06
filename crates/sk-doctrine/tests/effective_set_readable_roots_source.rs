//! Readable-root fallback must not drop authored project artifacts when the
//! org registry is corrupt or a declared org pack is missing.
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
fn command(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_READABLE_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn malformed_org_registry_retains_project_fallback_like_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_READABLE_NATIVE") {
        let root = std::path::PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (k, n) in roots() {
            assert_eq!(std::env::var_os(k).unwrap(), root.join(n));
        }
        assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
        assert_eq!(
            std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
            repo.join("packs")
        );
        let project = root.join("project");
        let config = fs::read(project.join(".kittify/config.yaml")).unwrap();
        let artifact = project.join(".kittify/charter-packs/directive/900-local.directive.yaml");
        let authored = fs::read(&artifact).unwrap();
        let result = sk_doctrine::resolve_effective_sets(
            &project,
            &["activated_directives"],
            &sk_doctrine::NATIVE_OFFERING_SERVICE,
        )
        .unwrap();
        let expected: Vec<String> =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        assert!(!result[0].resolved);
        assert!(
            result[0]
                .reason
                .as_ref()
                .unwrap()
                .contains("org pack registry")
        );
        // The DTO now carries exact Kernel PythonStrings; this is its explicit
        // scalar projection, and every id here is scalar, so the comparison is
        // the same assertion (a non-scalar id would fail loudly, not be dropped).
        let actual: Vec<String> = result[0]
            .fallback_ids
            .iter()
            .map(|id| id.to_scalar_string().expect("scalar fallback id"))
            .collect();
        assert_eq!(
            actual, expected,
            "a broken org registry must not erase the readable project layer"
        );
        assert_eq!(
            fs::read(project.join(".kittify/config.yaml")).unwrap(),
            config
        );
        assert_eq!(fs::read(artifact).unwrap(), authored);
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("readable roots evidence: {}", root.display());
    let source=command(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_READABLE_ROOT']);repo=Path(os.environ['PYTHONPATH']).parent
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
assert Path(os.environ['PYTHONPATH'])==repo/'src' and Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
from charter.activation.effective_set import resolve_effective_sets
project=root/'project';pack=project/'.kittify/charter-packs/directive';pack.mkdir(parents=True)
config=project/'.kittify/config.yaml';config.write_text('charter_packs: [unterminated\n')
artifact=pack/'900-local.directive.yaml';artifact.write_text('id: LOCAL_POLICY\n')
config_before=config.read_bytes();authored=artifact.read_bytes()
entry=resolve_effective_sets(project,['activated_directives'])['activated_directives']
assert not entry.resolved and 'org pack registry' in entry.reason,entry
assert '900-local' in entry.fallback_ids and len(entry.fallback_ids)>1,entry
assert config.read_bytes()==config_before and artifact.read_bytes()==authored
print(json.dumps(sorted(entry.fallback_ids)))
"#]).output().unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    let native = command(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "malformed_org_registry_retains_project_fallback_like_source",
            "--nocapture",
        ])
        .env("SK_READABLE_NATIVE", &root)
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
