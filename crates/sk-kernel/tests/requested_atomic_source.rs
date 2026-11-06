#![cfg(unix)]
//! Source requested-creation-mode execution is distinct from Native no-op/
//! destination-preserving atomic preparation. No process-global umask changes.
#[path = "support/oracle.rs"]
mod oracle;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};
const PAYLOAD: &[u8] = b"payload: body\n";
fn env_roots() -> [(&'static str, &'static str); 9] {
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
fn command(root: &Path, repo: &Path, mask: &str, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new("/bin/bash");
    c.args([
        "-c",
        "umask \"$1\"; shift; exec \"$@\"",
        "requested-mode-child",
        mask,
    ])
    .arg(program)
    .current_dir(root)
    .env_clear()
    .env("PATH", "/usr/bin:/bin")
    .env("SK_ATOMIC_ROOT", root)
    .env("PYTHONPATH", repo.join("src"))
    .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
    .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in env_roots() {
        c.env(key, root.join(name));
    }
    c
}
#[test]
fn requested_mode_matches_source_replacement_umask_and_failure_phases() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_ATOMIC_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (key, name) in env_roots() {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
        }
        let expected: Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let base = root.join("native");
        fs::create_dir_all(&base).unwrap();
        let mut rows = Vec::new();
        for kind in [
            "fresh",
            "existing",
            "equal",
            "symlink",
            "fifo",
            "blocked",
            "parent-sync-denied",
        ] {
            let directory = base.join(kind);
            fs::create_dir(&directory).unwrap();
            let path = directory.join("record.yaml");
            let mut before = None;
            match kind {
                "existing" => {
                    fs::write(&path, b"old").unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
                }
                "equal" => {
                    fs::write(&path, PAYLOAD).unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
                    use std::os::unix::fs::MetadataExt;
                    before = Some(fs::metadata(&path).unwrap().ino());
                }
                "symlink" => {
                    fs::write(directory.join("victim"), b"victim bytes").unwrap();
                    symlink("victim", &path).unwrap();
                }
                "fifo" => {
                    assert!(
                        Command::new("mkfifo")
                            .arg(&path)
                            .status()
                            .unwrap()
                            .success()
                    );
                }
                "blocked" => {
                    fs::create_dir(&path).unwrap();
                }
                "parent-sync-denied" => {
                    fs::set_permissions(&directory, fs::Permissions::from_mode(0o333)).unwrap();
                    assert!(
                        fs::File::open(&directory).is_err(),
                        "intended parent-sync cause"
                    );
                }
                _ => {}
            }
            let result = sk_kernel::atomic_write_with_requested_mode(&path, PAYLOAD, 0o644);
            if kind == "parent-sync-denied" {
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
            }
            if kind == "blocked" {
                assert!(result.is_err());
                assert!(path.is_dir());
                rows.push(json!({"kind":kind,"failed":true}));
            } else {
                assert!(result.unwrap(), "Source always publishes, even equal bytes");
                assert_eq!(fs::read(&path).unwrap(), PAYLOAD);
                assert!(
                    !fs::symlink_metadata(&path)
                        .unwrap()
                        .file_type()
                        .is_symlink()
                );
                let mut row = json!({"kind":kind,"bytes":fs::read(&path).unwrap(),"mode":fs::metadata(&path).unwrap().permissions().mode()&0o777});
                if let Some(before) = before {
                    use std::os::unix::fs::MetadataExt;
                    row["replaced"] = json!(fs::metadata(&path).unwrap().ino() != before);
                    assert_eq!(row["replaced"], true);
                }
                if kind == "symlink" {
                    assert_eq!(fs::read(directory.join("victim")).unwrap(), b"victim bytes");
                    row["victim_unchanged"] = json!(true);
                }
                rows.push(row);
            }
            let names: Vec<_> = fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert!(
                names
                    .iter()
                    .all(|name| name == "record.yaml" || name == "victim"),
                "no leaked temp files: {names:?}"
            );
        }
        assert_eq!(
            json!(rows),
            expected,
            "whole observed Source IO/mode outcomes"
        );
        return;
    }
    for mask in ["002", "027", "077"] {
        let root = tempfile::tempdir().unwrap().keep();
        for (_, name) in env_roots() {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        eprintln!("requested-mode evidence ({mask}): {}", root.display());
        let source=command(&root,&repo,mask,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_ATOMIC_ROOT']);assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for key,name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[key])==root/name
from kernel.yaml_io import write_mapping_atomic
base=root/'source';base.mkdir();rows=[]
for kind in ['fresh','existing','equal','symlink','fifo','blocked','parent-sync-denied']:
    directory=base/kind;directory.mkdir();path=directory/'record.yaml';before=None
    if kind=='existing':path.write_bytes(b'old');path.chmod(0o751)
    if kind=='equal':path.write_bytes(b'payload: body\n');path.chmod(0o755);before=path.stat().st_ino
    if kind=='symlink':(directory/'victim').write_bytes(b'victim bytes');path.symlink_to('victim')
    if kind=='fifo':os.mkfifo(path)
    if kind=='blocked':path.mkdir()
    if kind=='parent-sync-denied':
        directory.chmod(0o333)
        try:fd=os.open(directory,os.O_RDONLY)
        except PermissionError:pass
        else:os.close(fd);raise AssertionError('parent-sync denial premise failed')
    try:write_mapping_atomic({'payload':'body'},path)
    except OSError:
        assert kind=='blocked';assert path.is_dir();rows.append({'kind':kind,'failed':True})
    else:
        assert kind!='blocked';assert path.read_bytes()==b'payload: body\n';assert not path.is_symlink()
        row={'kind':kind,'bytes':list(path.read_bytes()),'mode':path.stat().st_mode&0o777}
        if before is not None:row['replaced']=path.stat().st_ino!=before;assert row['replaced']
        if kind=='symlink':assert (directory/'victim').read_bytes()==b'victim bytes';row['victim_unchanged']=True
        rows.append(row)
    finally:
        if kind=='parent-sync-denied':directory.chmod(0o700)
    assert all(p.name in ('record.yaml','victim') for p in directory.iterdir()),list(directory.iterdir())
print(json.dumps(rows))
"#]).output().unwrap();
        fs::write(root.join("source.stdout"), &source.stdout).unwrap();
        fs::write(root.join("source.stderr"), &source.stderr).unwrap();
        assert!(
            source.status.success(),
            "Source FIRST: {}",
            String::from_utf8_lossy(&source.stderr)
        );
        let native = command(&root, &repo, mask, std::env::current_exe().unwrap())
            .args([
                "--exact",
                "requested_mode_matches_source_replacement_umask_and_failure_phases",
                "--nocapture",
            ])
            .env("SK_ATOMIC_NATIVE", &root)
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
}
