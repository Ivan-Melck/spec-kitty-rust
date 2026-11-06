//! Source-first native temp-name bytes and raw-filename collision guard.
//! Root/dot/.. semantics and broader publication/error-wire fidelity remain open.
#![cfg(unix)]
#[path = "support/oracle.rs"]
mod oracle;
use std::{
    fs,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::Command,
};
fn roots() -> [(&'static str, &'static str); 11] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("XDG_CONFIG_DIRS", "config-dirs"),
        ("XDG_DATA_DIRS", "data-dirs"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}
fn child(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8");
    for (key, name) in roots() {
        cmd.env(key, root.join(name));
    }
    cmd
}
#[test]
fn temp_path_helpers_preserve_source_native_filename_bytes() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_SOURCE_TEMP_PATH_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (key, name) in roots() {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
        }
        assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
        assert_eq!(
            std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
            repo.join("packs")
        );
        let data: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let rows = data["rows"].as_array().unwrap();
        for row in rows {
            let name: Vec<u8> = serde_json::from_value(row["name"].clone()).unwrap();
            let path = PathBuf::from(std::ffi::OsString::from_vec(name.clone()));
            assert!(
                path.file_name().is_some(),
                "this proof only covers native filenames, not root/dot/.. semantics"
            );
            let mut expected: Vec<u8> =
                serde_json::from_value(row["generated_prefix"].clone()).unwrap();
            expected.extend_from_slice(std::process::id().to_string().as_bytes());
            assert_eq!(
                sk_kernel::generated_temporary_path(&path)
                    .as_os_str()
                    .as_bytes(),
                expected,
                "generated {name:?}"
            );
            let derived: Vec<u8> = serde_json::from_value(row["derived"].clone()).unwrap();
            assert_eq!(
                sk_kernel::derived_json_temporary_path(&path)
                    .as_os_str()
                    .as_bytes(),
                derived,
                "derived {name:?}"
            );
            let rebuild: Vec<u8> = serde_json::from_value(row["rebuild"].clone()).unwrap();
            assert_eq!(
                sk_kernel::rebuild_log_temporary_path(&path)
                    .as_os_str()
                    .as_bytes(),
                rebuild,
                "rebuild {name:?}"
            );
        }
        let directory = root.join("native-collision");
        fs::create_dir(&directory).unwrap();
        let name = std::ffi::OsString::from_vec(b"file\xff.md".to_vec());
        let destination = directory.join(&name);
        let mut sibling = std::ffi::OsString::from(".");
        sibling.push(&name);
        sibling.push(format!(".tmp{}", std::process::id()));
        let artifact = directory.join(sibling);
        fs::write(&destination, b"operator destination").unwrap();
        fs::write(&artifact, b"operator artifact\0\xff").unwrap();
        let state = |path: &Path| {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(path).unwrap();
            (
                fs::read(path).unwrap(),
                metadata.mode(),
                metadata.ino(),
                metadata.mtime(),
                metadata.mtime_nsec(),
            )
        };
        let before = (state(&destination), state(&artifact));
        let error = sk_kernel::write_source(
            sk_kernel::WriterOperation::generated(false),
            &destination,
            sk_kernel::SourcePayload::Bytes(b"new"),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(error, sk_kernel::SourceWriteError::UnprovenArtifact { path } if path == artifact)
        );
        assert_eq!(
            (state(&destination), state(&artifact)),
            before,
            "raw-path collision guard must preserve authored bytes/metadata"
        );
        assert_eq!(data["raw_collision"], true);
        println!(
            "{} Source temp-name cases preserve exact bytes and actual Native PID",
            rows.len()
        );
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    eprintln!("temp path evidence: {}", root.display());
    let source = child(&root, &repo, oracle::python()).arg("-c").arg(r#"
import json, os, sys
from pathlib import Path
root=Path(sys.argv[1]); repo=Path(sys.argv[2])
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','config-dirs'),('XDG_DATA_DIRS','data-dirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]: assert os.environ[k]==str(root/n)
assert os.environ['PYTHONPATH']==str(repo/'src') and os.environ['SPEC_KITTY_PACKS_ROOT']==str(repo/'packs')
from specify_cli.runtime.generated_writer import generated_temporary_path, write_generated_file
names=[b'f'+bytes([b])+b'.md' for b in range(256) if b!=47]+[b'foo',b'foo.',b'foo..',b'.hidden',b'.hidden.ext',b'..hidden',b'a.b.c', '😀.md'.encode(),b'file\xff.',b'.\xff.ext']
rows=[]; before=set(root.iterdir()); pid=str(os.getpid()).encode()
for name in names:
 p=Path(os.fsdecode(name)); g=os.fsencode(generated_temporary_path(p)); d=os.fsencode(p.with_suffix('.json.tmp')); r=os.fsencode(p.parent/f'{p.name}.tmp')
 assert g==b'.'+name+b'.tmp'+pid, (name,g)
 assert r==name+b'.tmp'
 if name==b'foo.': assert d==b'foo..json.tmp'
 rows.append(dict(name=list(name),generated=list(g),generated_prefix=list(g[:-len(pid)]),derived=list(d),rebuild=list(r)))
assert set(root.iterdir())==before,'pure path helpers performed I/O'
directory=root/'source-collision'; directory.mkdir()
destination=directory/os.fsdecode(b'file\xff.md'); artifact=generated_temporary_path(destination)
destination.write_bytes(b'operator destination'); artifact.write_bytes(b'operator artifact\x00\xff')
def state(p):
 m=p.stat(); return p.read_bytes(),m.st_mode,m.st_ino,m.st_mtime_ns
before=(state(destination),state(artifact)); caught=None
try: write_generated_file(destination,b'new',read_only=False)
except ValueError as error: caught=error
assert caught is not None and str(caught).startswith('Refusing to replace an unproven atomic writer artifact: ')
assert (state(destination),state(artifact))==before
print(json.dumps(dict(pid=os.getpid(),rows=rows,raw_collision=True)))
"#).arg(&root).arg(&repo).output().unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    fs::write(
        root.join("source.status"),
        format!("{:?}", source.status.code()),
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
            "temp_path_helpers_preserve_source_native_filename_bytes",
            "--nocapture",
        ])
        .env("SK_SOURCE_TEMP_PATH_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    fs::write(
        root.join("native.status"),
        format!("{:?}", native.status.code()),
    )
    .unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}
