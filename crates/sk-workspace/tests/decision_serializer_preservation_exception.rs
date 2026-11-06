//! Operator-authorized exception ONLY for summary_json surrogate KEY loss in
//! pinned pydantic's JSON-mode projection. Real Source corruption remains
//! asserted separately; it is not relabelled byte parity or an input refusal.
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
        .env("SK_PRESERVATION_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn approved_summary_key_exception_preserves_authored_data_without_masking_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_PRESERVATION_NATIVE") {
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
        for name in ["single", "collision", "full-domain"] {
            let s = root.join("source").join(name);
            let d = root.join("native").join(name);
            fs::create_dir_all(&d).unwrap();
            let b = fs::read(s.join("base")).unwrap();
            let o = fs::read(s.join("original")).unwrap();
            let t = fs::read(s.join("theirs")).unwrap();
            let expected = fs::read(s.join("expected-native-approved-exception")).unwrap();
            let source = fs::read(s.join("ours")).unwrap();
            assert_ne!(
                source, expected,
                "{name}: preserve evidence of the real Source divergence"
            );
            let destination = d.join("ours");
            fs::write(&destination, &o).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();
            }
            for attempt in 0..2 {
                let actual_ours = fs::read(&destination).unwrap();
                let result = sk_workspace::merge_drivers::run_driver_into(
                    "merge-driver-decision-index",
                    sk_workspace::merge_drivers::MergeDriverInputs {
                        base: Some(&b),
                        ours: Some(&actual_ours),
                        theirs: Some(&t),
                    },
                    &destination,
                );
                if attempt == 0 {
                    assert_eq!(
                        result.unwrap().bytes,
                        expected,
                        "{name}: only the authorized field differs from actual Source"
                    );
                } else {
                    let error = result.unwrap_err();
                    assert!(
                        error.to_string().contains("diverged on both sides"),
                        "{name}: real Source retry refuses differing open entries, not invented idempotence: {error}"
                    );
                }
                assert_eq!(fs::read(&destination).unwrap(), expected);
                assert_eq!(fs::read(s.join("base")).unwrap(), b);
                assert_eq!(fs::read(s.join("theirs")).unwrap(), t);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    assert_eq!(
                        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                        0o640
                    );
                }
            }
            // All unaffected bytes are already bound above to actual Source's projected
            // document, with ONLY summary_json restored from its real validated model.
            // Explicit association proof uses the Kernel exact tree, never serde text.
            use sk_kernel::python_json::{PythonJsonValue as V, parse, python_json_dict_equal};
            fn summary(v: &V) -> &sk_kernel::python_json::PythonJsonDict {
                let Some(V::Array(entries)) = v.field("entries") else {
                    panic!("entries")
                };
                let Some(V::ExactObject(summary)) = entries[0].field("summary_json") else {
                    panic!("summary")
                };
                summary
            }
            let original = parse(std::str::from_utf8(&o).unwrap()).unwrap();
            let result = parse(std::str::from_utf8(&expected).unwrap()).unwrap();
            assert!(
                python_json_dict_equal(summary(&original), summary(&result)),
                "{name}: every original key/value association survives"
            );
        }
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("authorized preservation evidence: {}", root.display());
    let source=isolated(&root,&repo,oracle::python()).args(["-c",r#"
import copy,hashlib,json,os,sys
from pathlib import Path
root=Path(os.environ['SK_PRESERVATION_ROOT']);repo=Path(os.environ['PYTHONPATH']).parent
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
assert Path(os.environ['PYTHONPATH'])==repo/'src' and Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
from specify_cli.decisions.models import DecisionIndex
from specify_cli.consolidation.drivers import run_decision_index_driver,union_decision_index,MergeDriverError
entry=dict(decision_id='d',origin_flow='plan',step_id='s',input_key='i',question='Q',status='open',created_at='2026-01-01T00:00:00Z',mission_id='m',mission_slug='demo')
full={chr(i):'value-'+hex(i) for i in range(0xd800,0xe000)}
full.update({'\0':'nul','\ue000':'pua','\ufdd0':'noncharacter','\ufffd'*3:'authored replacement key'})
records=[]
for name,summary in [('single',{'\ud800':'\udfff'}),('collision',{'\ud800':'first','\ud801':'second'}),('full-domain',full)]:
 d=root/'source'/name;d.mkdir(parents=True)
 document=dict(mission_id='m',entries=[dict(entry,summary_json=summary)])
 original=(json.dumps(document)+'\n').encode()
 for n,data in [('base',b'{}'),('ours',original),('original',original),('theirs',original)]: (d/n).write_bytes(data)
 (d/'ours').chmod(0o640)
 raw=union_decision_index(document,document);model=DecisionIndex.model_validate(raw)
 assert raw['entries'][0]['summary_json']==summary and model.entries[0].summary_json==summary
 projected=model.model_dump(mode='json');corrupted=projected['entries'][0]['summary_json']
 assert corrupted!=summary and '\ufffd'*3 in corrupted
 if name=='collision':assert corrupted=={'\ufffd'*3:'second'}
 if name=='full-domain':assert len(summary)==2052 and len(corrupted)==4
 # Explicit operator-authorized expected view, NOT historical Source bytes:
 # no blanket mask, synthetic codec, original-key rename, or Native-derived data.
 expected=copy.deepcopy(projected)
 expected['entries'][0]['summary_json']=dict(model.entries[0].summary_json)
 expected_bytes=(json.dumps(expected,sort_keys=True,indent=2)+'\n').encode()
 (d/'expected-native-approved-exception').write_bytes(expected_bytes)
 run_decision_index_driver(str(d/'base'),str(d/'ours'),str(d/'theirs'))
 written=(d/'ours').read_bytes()
 assert json.loads(written)==projected and written!=expected_bytes
 try:run_decision_index_driver(str(d/'base'),str(d/'ours'),str(d/'theirs'))
 except MergeDriverError as error:assert 'diverged on both sides with no safe precedence' in str(error)
 else:raise AssertionError('Source retry does not silently pick one of two differing open entries')
 assert (d/'ours').read_bytes()==written and (d/'ours').stat().st_mode&0o777==0o640
 assert (d/'base').read_bytes()==b'{}' and (d/'theirs').read_bytes()==original
 records.append(dict(name=name,validated_keys=len(summary),source_written_keys=len(corrupted),exception='summary_json unpaired-surrogate keys only'))
print(json.dumps(dict(records=records,source_sha256=hashlib.sha256((repo/'src/specify_cli/consolidation/drivers.py').read_bytes()).hexdigest(),model_sha256=hashlib.sha256((repo/'src/specify_cli/decisions/models.py').read_bytes()).hexdigest())))
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
            "approved_summary_key_exception_preserves_authored_data_without_masking_source",
            "--nocapture",
        ])
        .env("SK_PRESERVATION_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    assert!(
        native.status.success(),
        "Native preservation: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}
