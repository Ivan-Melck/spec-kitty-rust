//! Exhaustive pinned Source decimal membership/value, distinct from numeric/word.
#[path = "support/oracle.rs"]
mod oracle;
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
    let mut c = Command::new(program);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_DECIMAL_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn decimal_membership_value_and_exact_string_rule_match_all_source_codepoints() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_DECIMAL_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (k, n) in roots() {
            assert_eq!(std::env::var_os(k).unwrap(), root.join(n));
        }
        let source: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let mut digits = source["digits"].as_array().unwrap().iter().peekable();
        assert_eq!(digits.len(), 660);
        for code in 0..0x110000u32 {
            let expected = if digits
                .peek()
                .is_some_and(|row| row[0].as_u64() == Some(u64::from(code)))
            {
                let row = digits.next().unwrap();
                Some(u8::try_from(row[1].as_u64().unwrap()).unwrap())
            } else {
                None
            };
            assert_eq!(
                sk_kernel::python_text::decimal_value(code),
                expected,
                "U+{code:04X}"
            );
            assert_eq!(sk_kernel::python_text::is_decimal(code), expected.is_some());
        }
        for case in source["strings"].as_array().unwrap() {
            let codepoints: Vec<u32> = serde_json::from_value(case[0].clone()).unwrap();
            let value = sk_kernel::PythonString::from_codepoints(&codepoints).unwrap();
            assert_eq!(
                sk_kernel::python_text::is_decimal_codepoints(value.codepoints()),
                case[1].as_bool().unwrap()
            );
        }
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    let source=command(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys,unicodedata
from pathlib import Path
root=Path(os.environ['SK_DECIMAL_ROOT']);assert Path.cwd()==root and sys.version_info[:3]==(3,11,15) and unicodedata.unidata_version=='14.0.0'
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
rows=[]
for code in range(0x110000):
 value=chr(code);assert value.isdecimal()==(unicodedata.category(value)=='Nd')
 if value.isdecimal():rows.append([code,unicodedata.decimal(value)])
assert len(rows)==660
samples=[[],[0x30],[0x0661,0x0968],[0x1d7ce,0x1d7d7],[0xb2],[0x2169],[0xd800],[0x31,0x00],[0x20,0x31],[0x31,0xe000]]
strings=[[row,''.join(map(chr,row)).isdecimal()] for row in samples]
print(json.dumps({'digits':rows,'strings':strings}))
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
            "decimal_membership_value_and_exact_string_rule_match_all_source_codepoints",
            "--nocapture",
        ])
        .env("SK_DECIMAL_NATIVE", &root)
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
