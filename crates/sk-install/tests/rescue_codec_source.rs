//! B7 F13 actual migration callback: preserve strict CPython codec diagnostics
//! and the read-before-write failure footprint, not just a generic error prefix.
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
fn child(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_RESCUE_CODEC_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
fn fixture(project: &Path) {
    fs::create_dir_all(project.join(".kittify/charter")).unwrap();
    fs::write(project.join(".kittify/charter/charter.md"), b"# charter\n").unwrap();
    for (name, bytes) in [
        (
            "binary.md",
            b"constitution \xff\xfe invalid utf8\n".as_slice(),
        ),
        (
            "continuation.md",
            b"\xef\xbb\xbfconstitution \xe1\x80X".as_slice(),
        ),
        ("truncated.md", b"constitution \xf0\x90\x80".as_slice()),
    ] {
        fs::write(project.join(".kittify/charter").join(name), bytes).unwrap();
    }
}
#[derive(Debug, PartialEq, Eq)]
struct ProofFile {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
    permissions: fs::Permissions,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    inode: u64,
}
fn footprint(root: &Path) -> Vec<ProofFile> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(footprint(&p));
        } else {
            let meta = fs::metadata(&p).unwrap();
            out.push(ProofFile {
                path: p.clone(),
                bytes: fs::read(p).unwrap(),
                permissions: meta.permissions(),
                modified: meta.modified().unwrap(),
                #[cfg(unix)]
                inode: {
                    use std::os::unix::fs::MetadataExt;
                    meta.ino()
                },
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}
#[test]
fn charter_rename_preserves_source_codec_errors_and_invalid_input_bytes() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_RESCUE_CODEC_NATIVE") {
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
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let project = root.join("native-project");
        fixture(&project);
        let before = footprint(&project);
        let callback = sk_install::migrations::completion_modern::rescue_spine::IMPLEMENTATIONS
            .iter()
            .find(|m| m.id == "3.1.1_charter_rename")
            .unwrap();
        let result = (callback.apply)(&project).unwrap();
        assert!(!result.success);
        assert_eq!(
            serde_json::to_value(&result.errors).unwrap(),
            expected["errors"]
        );
        assert_eq!(
            serde_json::to_value(&result.changes).unwrap(),
            expected["changes"]
        );
        assert_eq!(
            serde_json::to_value(&result.warnings).unwrap(),
            expected["warnings"]
        );
        assert_eq!(
            footprint(&project),
            before,
            "a decode error must not mutate the artifact tree"
        );
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("rescue codec evidence: {}", root.display());
    let source=child(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_RESCUE_CODEC_ROOT']);repo=Path(os.environ['PYTHONPATH']).parent
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
assert Path(os.environ['PYTHONPATH'])==repo/'src' and Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
from specify_cli.upgrade.migrations.m_3_1_1_charter_rename import CharterRenameMigration
p=root/'source-project';d=p/'.kittify/charter';d.mkdir(parents=True);(d/'charter.md').write_bytes(b'# charter\n')
for name,data in [('binary.md',b'constitution \xff\xfe invalid utf8\n'),('continuation.md',b'\xef\xbb\xbfconstitution \xe1\x80X'),('truncated.md',b'constitution \xf0\x90\x80')]: (d/name).write_bytes(data)
def snapshot():return {str(f.relative_to(p)):(f.read_bytes(),f.stat().st_mode,f.stat().st_mtime_ns,f.stat().st_ino) for f in p.rglob('*') if f.is_file()}
before=snapshot();result=CharterRenameMigration().apply(p)
assert not result.success and len(result.errors)==3,result
assert result.errors[0]=="Failed to read .kittify/charter/binary.md: 'utf-8' codec can't decode byte 0xff in position 13: invalid start byte",result.errors
assert snapshot()==before,'Source changed files on codec failure'
print(json.dumps(dict(errors=result.errors,changes=result.changes_made,warnings=result.warnings)))
"#]).output().unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    fs::write(
        root.join("source.status"),
        format!("{:?}\n", source.status.code()),
    )
    .unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    let native = child(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "charter_rename_preserves_source_codec_errors_and_invalid_input_bytes",
            "--nocapture",
        ])
        .env("SK_RESCUE_CODEC_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    fs::write(
        root.join("native.status"),
        format!("{:?}\n", native.status.code()),
    )
    .unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}
