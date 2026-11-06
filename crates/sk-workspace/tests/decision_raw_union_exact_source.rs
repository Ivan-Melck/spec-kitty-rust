//! H7 F4: the public raw union uses Python str() ordering, not typed dates or
//! scalar-only strings. Exact and scalar facades must share ONE rule.
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
fn isolated(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_RAW_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn raw_union_keeps_exact_domain_and_python_str_order() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_RAW_NATIVE") {
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
        let cases: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(root.join("cases.json")).unwrap()).unwrap();
        use sk_kernel::python_json::{PythonJsonRenderOptions, PythonJsonValue as V, parse};
        let render = |v: &V| {
            v.render_python_with(PythonJsonRenderOptions {
                sort_keys: true,
                ..Default::default()
            })
            .to_scalar_string()
            .unwrap()
        };
        for c in cases {
            let ours_text = c["ours"].as_str().unwrap();
            let theirs_text = c["theirs"].as_str().unwrap();
            let V::ExactObject(ours) = parse(ours_text).unwrap() else {
                panic!("ours")
            };
            let V::ExactObject(theirs) = parse(theirs_text).unwrap() else {
                panic!("theirs")
            };
            let result =
                sk_workspace::merge_drivers::union_decision_index_exact(&ours, &theirs).unwrap();
            assert_eq!(
                render(&result),
                c["expected"].as_str().unwrap(),
                "{} exact raw union",
                c["name"]
            );
            if c["scalar"].as_bool().unwrap() {
                let ours: serde_json::Value = serde_json::from_str(ours_text).unwrap();
                let theirs: serde_json::Value = serde_json::from_str(theirs_text).unwrap();
                let result = sk_workspace::merge_drivers::union_decision_index(
                    ours.as_object().unwrap(),
                    theirs.as_object().unwrap(),
                )
                .unwrap();
                let v = parse(&serde_json::to_string(&result).unwrap()).unwrap();
                assert_eq!(
                    render(&v),
                    c["expected"].as_str().unwrap(),
                    "{} scalar facade",
                    c["name"]
                );
            }
        }
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("exact raw union evidence: {}", root.display());
    let source=isolated(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_RAW_ROOT']);repo=Path(os.environ['PYTHONPATH']).parent
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
assert Path(os.environ['PYTHONPATH'])==repo/'src' and Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
from specify_cli.consolidation.drivers import union_decision_index
cases=[]
def case(name,left,right,order,scalar=True,extras=None):
 ours=dict(entries=[dict(decision_id='B',created_at=left)])
 theirs=dict(entries=[dict(decision_id='A',created_at=right)])
 if extras:ours.update(extras)
 result=union_decision_index(ours,theirs)
 assert [row['decision_id'] for row in result['entries']]==order,(name,result)
 assert 'version' not in result,'raw union must not inject typed defaults'
 cases.append(dict(name=name,ours=json.dumps(ours),theirs=json.dumps(theirs),expected=json.dumps(result,sort_keys=True),scalar=scalar))
case('numeric',1700000000,9,['B','A'])
case('bool',True,'2020-01-01',['A','B'])
case('float',1.5,'0001-01-01',['A','B'])
case('null',None,0,['A','B'])
case('list',[1],'z',['B','A'])
case('dict',{'z':1,'a':0},'zz',['A','B'])
case('bigint',10**200,'9',['B','A'],False)
case('nan',float('nan'),'z',['B','A'],False)
case('infinity',float('inf'),'z',['B','A'],False)
case('exact-cs','\ud800','\udfff',['B','A'],False,{'\ud800':'\udfff','\0':'nul','\ue000':'pua','\ufdd0':'noncharacter'})
(root/'cases.json').write_text(json.dumps(cases))
print(json.dumps(dict(cases=len(cases),api='actual public raw union, no model projection or write')))
"#]).output().unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    let native = isolated(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "raw_union_keeps_exact_domain_and_python_str_order",
            "--nocapture",
        ])
        .env("SK_RAW_NATIVE", &root)
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
