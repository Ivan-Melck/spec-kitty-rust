//! Real Source-first index-query parity, including conflict stages and raw paths.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn environment(root: &Path) -> Vec<(&'static str, PathBuf)> {
    [
        ("HOME", "home"),
        ("TMPDIR", "tmp"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
    ]
    .into_iter()
    .map(|(key, name)| (key, root.join(name)))
    .collect()
}
fn command(program: &Path, root: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(environment(root))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("SK_INDEX_ROOT", root)
        .env("SK_INDEX_REPO", root.join("repo"))
        .current_dir(root.join("repo"));
    command
}
fn git(root: &Path, args: &[&str], input: &[u8]) -> Vec<u8> {
    let mut child = command(Path::new("/usr/bin/git"), root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fixture git");
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
#[test]
fn literal_index_entries_match_source_before_native_reads() {
    let root = tempfile::tempdir().unwrap().keep();
    for path in ["repo", "home", "tmp", "config", "cache", "data", "state"] {
        std::fs::create_dir(root.join(path)).unwrap();
    }
    git(&root, &["init", "-q", "-b", "main"], b"");
    std::fs::create_dir(root.join("repo/dir")).unwrap();
    for name in [
        "dir/one\tline\n.txt",
        "dir/two.txt",
        "plain.txt",
        ":(glob)*",
    ] {
        std::fs::write(root.join("repo").join(name), name.as_bytes()).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::fs::write(
            root.join("repo").join(OsStr::from_bytes(b"raw-\xff")),
            b"raw",
        )
        .unwrap();
    }
    git(&root, &["add", "--all"], b"");
    let mut stages = Vec::new();
    for (stage, content) in [
        (1, b"base".as_slice()),
        (2, b"ours".as_slice()),
        (3, b"theirs".as_slice()),
    ] {
        let oid =
            String::from_utf8(git(&root, &["hash-object", "-w", "--stdin"], content)).unwrap();
        stages
            .extend_from_slice(format!("100644 {} {stage}\tconflict.txt\0", oid.trim()).as_bytes());
    }
    git(&root, &["update-index", "-z", "--index-info"], &stages);
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output=command(&oracle::python(),&root)
        .env("PYTHONPATH",repository.join("src")).env("PYTHONDONTWRITEBYTECODE","1")
        .args(["-c",r#"
import json,os,sys
from pathlib import Path
from kernel.git import index_entries
root=Path(os.environ['SK_INDEX_ROOT']);repo=Path(os.environ['SK_INDEX_REPO'])
assert Path.cwd()==repo and sys.version_info[:3]==(3,11,15)
for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME'):assert Path(os.environ[key]).parent==root
queries=[(),('dir',),(':(glob)*',)]
rows=[[[list(str(e.path).encode('utf-8','surrogateescape')),e.mode,e.oid,e.stage] for e in index_entries(repo,pathspecs=q,timeout=3)] for q in queries]
assert sorted(e[3] for e in rows[0] if bytes(e[0])==b'conflict.txt')==[1,2,3]
assert len(rows[1])==2
assert len(rows[2])==1 and bytes(rows[2][0][0])==b':(glob)*'
print(json.dumps(rows))
"#]).output().unwrap();
    std::fs::write(root.join("source.stdout"), &output.stdout).unwrap();
    std::fs::write(root.join("source.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "Source failed before Native: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(expected.as_array().unwrap().len(), 3);
    let output = command(&std::env::current_exe().unwrap(), &root)
        .args([
            "--ignored",
            "--exact",
            "native_index_query_helper",
            "--nocapture",
        ])
        .output()
        .unwrap();
    std::fs::write(root.join("native.stdout"), &output.stdout).unwrap();
    std::fs::write(root.join("native.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "Native failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix("INDEX_RESULT="))
        .expect("Native rows");
    let actual: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(actual, expected);
}
#[test]
#[ignore = "exercised by isolated Source-first parent"]
fn native_index_query_helper() {
    let root = PathBuf::from(std::env::var_os("SK_INDEX_ROOT").expect("root"));
    assert_eq!(std::env::current_dir().unwrap(), root.join("repo"));
    for (key, value) in environment(&root) {
        assert_eq!(std::env::var_os(key).unwrap(), value.as_os_str());
    }
    let repo = sk_git::GitRepository::open(root.join("repo")).unwrap();
    let queries: [&[&OsStr]; 3] = [&[], &[OsStr::new("dir")], &[OsStr::new(":(glob)*")]];
    let rows: Vec<_> = queries
        .into_iter()
        .map(|query| {
            repo.index_entries(query)
                .unwrap()
                .into_iter()
                .map(|entry| {
                    #[cfg(unix)]
                    let bytes = {
                        use std::os::unix::ffi::OsStrExt;
                        entry.path().as_os_str().as_bytes().to_vec()
                    };
                    #[cfg(not(unix))]
                    let bytes = entry
                        .path()
                        .as_os_str()
                        .to_str()
                        .unwrap()
                        .as_bytes()
                        .to_vec();
                    serde_json::json!([bytes, entry.mode(), entry.oid().as_str(), entry.stage()])
                })
                .collect::<Vec<_>>()
        })
        .collect();
    println!("INDEX_RESULT={}", serde_json::to_string(&rows).unwrap());
}
