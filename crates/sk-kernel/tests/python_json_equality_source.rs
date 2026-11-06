//! Default-json equality, including its parsed-NaN container identity shortcut.
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
        .env("SK_EQUAL_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn canonical_equality_matches_actual_source_numbers_containers_and_exact_keys() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_EQUAL_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (k, n) in roots() {
            assert_eq!(std::env::var_os(k).unwrap(), root.join(n));
        }
        let cases: Vec<(String, String, bool)> =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        assert!(cases.len() > 15000);
        for (left, right, expected) in cases {
            let a = sk_kernel::python_json::parse(&left).unwrap();
            let b = sk_kernel::python_json::parse(&right).unwrap();
            assert_eq!(
                sk_kernel::python_json::python_json_equal(&a, &b),
                expected,
                "{left} == {right}"
            );
            assert_eq!(
                sk_kernel::python_json::python_json_equal(&b, &a),
                expected,
                "symmetric: {right} == {left}"
            );
            if let (
                sk_kernel::python_json::PythonJsonValue::ExactObject(a),
                sk_kernel::python_json::PythonJsonValue::ExactObject(b),
            ) = (&a, &b)
            {
                assert_eq!(
                    sk_kernel::python_json::python_json_dict_equal(a, b),
                    expected
                );
            }
            // Legacy producer views share the rule instead of introducing an adapter comparator.
            if matches!(
                a,
                sk_kernel::python_json::PythonJsonValue::Integer(_)
                    | sk_kernel::python_json::PythonJsonValue::Float(_)
            ) {
                let raw = sk_kernel::python_json::PythonJsonValue::Number(left.clone());
                assert_eq!(
                    sk_kernel::python_json::python_json_equal(&raw, &b),
                    expected,
                    "producer number {left} == {right}"
                );
            }
        }
        type OptionalCase = (Vec<Option<String>>, Vec<Option<String>>, bool);
        let cases: Vec<OptionalCase> =
            serde_json::from_slice(&fs::read(root.join("optional.json")).unwrap()).unwrap();
        for (left, right, expected) in cases {
            let project = |values: Vec<Option<String>>| {
                values
                    .into_iter()
                    .map(|value| value.map(|token| sk_kernel::python_json::parse(&token).unwrap()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                sk_kernel::python_json::python_json_optional_sequence_equal(
                    &project(left),
                    &project(right)
                ),
                expected
            );
        }
        let legacy = sk_kernel::python_json::PythonJsonValue::Object(vec![
            (
                "a".to_owned(),
                sk_kernel::python_json::PythonJsonValue::Bool(false),
            ),
            (
                "a".to_owned(),
                sk_kernel::python_json::PythonJsonValue::Integer("2".to_owned()),
            ),
        ]);
        assert!(sk_kernel::python_json::python_json_equal(
            &legacy,
            &sk_kernel::python_json::parse("{\"a\":2.0}").unwrap()
        ));
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("equality evidence: {}", root.display());
    let source=command(&root,&repo,oracle::python()).args(["-c",r#"
import json,math,os,random,struct,sys
from pathlib import Path
root=Path(os.environ['SK_EQUAL_ROOT']);assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
load=json.loads
assert load('NaN')!=load('NaN')
assert load('NaN') is load('NaN')
assert load('[NaN]')==load('[NaN]') and load('{"x":NaN}')==load('{"x":NaN}')
rows=[]
def add(a,b):rows.append((a,b,load(a)==load(b)))
base=['null','false','true','0','-0','0.0','-0.0','1','1.0','-1','1.5','9007199254740992','9007199254740993','9007199254740992.0','1e400','-1e400','Infinity','-Infinity','NaN',str(10**400),'"1"','[]','{}']
for a in base:
 for b in base:
  add(a,b);add('['+a+']','['+b+']');add('{"x":'+a+'}','{"x":'+b+'}')
for a,b in [('{"a":1,"b":false}','{"b":0.0,"a":true}'),('{"a":1,"a":2}','{"a":2.0}'),('[]','{}'),('[NaN,[NaN],{"x":NaN}]','[NaN,[NaN],{"x":NaN}]')]:add(a,b)
rng=random.Random(6153)
bits=[0,1,0x8000000000000000,0x000fffffffffffff,0x0010000000000000,0x3ff0000000000000,0x4340000000000000,0x7fefffffffffffff]+[rng.getrandbits(64) for _ in range(2048)]
for bits_value in bits:
 f=struct.unpack('>d',bits_value.to_bytes(8,'big'))[0];token=json.dumps(f)
 if math.isfinite(f):
  integer=int(f)
  for delta in (-1,0,1):
   other=str(integer+delta);add(other,token);add('['+other+']','['+token+']')
 else:add(token,token);add('['+token+']','['+token+']')
for code in range(0xd800,0xe000):
 key=json.dumps(chr(code));other=json.dumps(chr(code^1))
 add(key,key);add(key,other)
 add('{'+key+':1,"x":false}','{"x":0.0,'+key+':true}')
 add('{'+key+':1}','{'+other+':1}')
for text in ['\0','\ue000','\ufdd0','\U0010ffff','literal \\ud800']:
 token=json.dumps(text);add(token,token)
assert len(rows)>15000
missing=object()
optional=[]
for a,b in [([None,'NaN'],[None,'NaN']),([None],['null']),(['true','1'],['1.0','true']),(['9007199254740993'],['9007199254740992.0']),(['null'],['null'])]:
 left=tuple(missing if value is None else load(value) for value in a)
 right=tuple(missing if value is None else load(value) for value in b)
 optional.append([a,b,left==right])
(root/'optional.json').write_text(json.dumps(optional))
print(json.dumps(rows))
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
            "canonical_equality_matches_actual_source_numbers_containers_and_exact_keys",
            "--nocapture",
        ])
        .env("SK_EQUAL_NATIVE", &root)
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
