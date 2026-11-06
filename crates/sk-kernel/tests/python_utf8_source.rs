//! Actual strict CPython decoder objects/ranges/diagnostics, not Rust error prose.
#[path = "support/oracle.rs"]
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
        .env("SK_UTF8_ROOT", root)
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
fn outcome(bytes: &[u8], sig: bool) -> serde_json::Value {
    let decoded = if sig {
        sk_kernel::python_utf8::decode_utf8_sig(bytes)
    } else {
        sk_kernel::python_utf8::decode_utf8(bytes)
    };
    match decoded {
        Ok(text) => serde_json::json!({"ok":text.chars().map(u32::from).collect::<Vec<_>>()}),
        Err(error) => {
            serde_json::json!({"object":error.object(),"encoding":error.encoding(),"start":error.start,"end":error.end,"reason":error.reason,"message":error.to_string()})
        }
    }
}
#[test]
fn strict_utf8_and_sig_outcomes_match_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_UTF8_NATIVE") {
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
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        assert!(rows.len() > 70000);
        for row in &rows {
            let bytes: Vec<u8> = serde_json::from_value(row["bytes"].clone()).unwrap();
            for (key, sig) in [("utf8", false), ("sig", true)] {
                assert_eq!(outcome(&bytes, sig), row[key], "{key} bytes {bytes:?}");
            }
        }
        println!(
            "{} byte sequences, both UTF-8 codecs, exact exception object/range/reason/message",
            rows.len()
        );
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("UTF8 evidence: {}", root.display());
    let source=child(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_UTF8_ROOT']);repo=Path(os.environ['PYTHONPATH']).parent
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
assert Path(os.environ['PYTHONPATH'])==repo/'src' and Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
def judge(data,codec):
 try:return {'ok':list(map(ord,data.decode(codec)))}
 except UnicodeDecodeError as e:return dict(object=list(e.object),encoding=e.encoding,start=e.start,end=e.end,reason=e.reason,message=str(e))
assert judge(b'1234567890123\xff','utf-8')['message']=="'utf-8' codec can't decode byte 0xff in position 13: invalid start byte"
assert judge(b'\xef\xbb\xbf\xe1\x80','utf-8-sig')['object']==[225,128]
assert judge(b'\xe1\x80X','utf-8')['end']==2
cases=[b'']+[bytes([n]) for n in range(256)]+[bytes([a,b]) for a in range(256) for b in range(256)]
seeds=[b'\xe0\xa0\x80',b'\xed\x9f\xbf',b'\xef\xbf\xbf',b'\xf0\x90\x80\x80',b'\xf4\x8f\xbf\xbf']
for seed in seeds:
 for pos in range(len(seed)):
  for byte in range(256):cases.append(b'prefix\x00'+seed[:pos]+bytes([byte])+seed[pos+1:]+b'\r\n')
 for length in range(len(seed)+1):cases.append(seed[:length])
state=0x814d19
for _ in range(4096):
 data=[]
 for _ in range(state%9):
  state=(1664525*state+1013904223)&0xffffffff;data.append(state>>24)
 state=(1664525*state+1013904223)&0xffffffff
 cases.append(bytes(data))
cases.extend(bytes([0xef,0xbb,0xbf])+tail for tail in [b'',b'\xff',b'\xe1',b'\xe1\x80X',b'\xef\xbb\xbf',b'\x00\r\n'])
cases.append(''.join(map(chr,[0,0x7f,0x80,0x7ff,0x800,0xd7ff,0xe000,0xffff,0x10000,0x10ffff])).encode())
print(json.dumps([dict(bytes=list(data),utf8=judge(data,'utf-8'),sig=judge(data,'utf-8-sig')) for data in cases],separators=(',',':')))
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
            "strict_utf8_and_sig_outcomes_match_source",
            "--nocapture",
        ])
        .env("SK_UTF8_NATIVE", &root)
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
