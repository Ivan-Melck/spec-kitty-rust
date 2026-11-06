//! Successful in-memory driver APIs must never return a placeholder empty file.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
const BASE: &[u8] = br#"{"anchor":0}"#;
const OURS: &[u8] = br#"{"anchor":0,"ours":1}"#;
const THEIRS: &[u8] = br#"{"anchor":0,"theirs":2}"#;
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
        .env("SK_DRIVER_MEMORY_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn source_merge_bytes_are_preserved_by_generic_and_named_memory_apis() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_DRIVER_MEMORY_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (k, n) in roots() {
            assert_eq!(std::env::var_os(k).unwrap(), root.join(n));
        }
        let expected: Vec<u8> =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        assert!(!expected.is_empty());
        let inputs = || sk_workspace::merge_drivers::MergeDriverInputs {
            base: Some(BASE),
            ours: Some(OURS),
            theirs: Some(THEIRS),
        };
        let generic =
            sk_workspace::merge_drivers::run_driver("merge-driver-meta", inputs()).unwrap();
        assert_eq!(
            generic.bytes, expected,
            "in-process conflict staging consumes this payload; Ok(empty) would erase authored data"
        );
        let named = sk_workspace::merge_drivers::run_meta_driver(inputs()).unwrap();
        assert_eq!(
            named.bytes, expected,
            "named scalar API has the same contract"
        );
        let destination = root.join("in-process-output");
        fs::write(&destination, OURS).unwrap();
        let published = sk_workspace::merge_drivers::run_driver_into(
            "merge-driver-meta",
            inputs(),
            &destination,
        )
        .unwrap();
        assert_eq!(
            published.bytes, expected,
            "physical execution also returns actual bytes"
        );
        assert_eq!(fs::read(destination).unwrap(), expected);
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("memory payload evidence: {}", root.display());
    let source=command(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_DRIVER_MEMORY_ROOT']);assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
from specify_cli.consolidation.drivers import run_meta_driver
base=root/'base';ours=root/'ours';theirs=root/'theirs'
base.write_text('{"anchor":0}');ours.write_text('{"anchor":0,"ours":1}');theirs.write_text('{"anchor":0,"theirs":2}')
run_meta_driver(str(base),str(ours),str(theirs))
result=ours.read_bytes();assert result and json.loads(result)=={'anchor':0,'ours':1,'theirs':2},result
print(json.dumps(list(result)))
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
            "source_merge_bytes_are_preserved_by_generic_and_named_memory_apis",
            "--nocapture",
        ])
        .env("SK_DRIVER_MEMORY_NATIVE", &root)
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
