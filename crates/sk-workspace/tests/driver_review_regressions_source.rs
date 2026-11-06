//! Source-first regressions for independent review H5: history identity/order,
//! Unicode-blank meta sides, empty-base semantics and event ordering.
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
        .env("SK_H5_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn history_blankness_and_event_sort_match_actual_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_H5_NATIVE") {
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
        let source: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let spaces: Vec<u32> = serde_json::from_value(source["spaces"].clone()).unwrap();
        let native_spaces: Vec<u32> = (0..=0x10ffff)
            .filter(|code| sk_kernel::python_text::is_whitespace(*code))
            .collect();
        assert_eq!(
            native_spaces, spaces,
            "all 1,114,112 Python code points, including every surrogate"
        );
        for file in ["pure-union.json", "pure-union-fallback.json"] {
            let union: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join(file)).unwrap()).unwrap();
            let actual = sk_workspace::merge_drivers::union_decision_index(
                union["ours"].as_object().unwrap(),
                union["theirs"].as_object().unwrap(),
            )
            .unwrap();
            assert_eq!(
                serde_json::Value::Object(actual),
                union["expected"],
                "{file}: raw public union: empty-string fallback, and NO invented version default"
            );
        }
        let cases: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(root.join("cases.json")).unwrap()).unwrap();
        for (i, c) in cases.iter().enumerate() {
            let d = root.join(format!("native-{i}"));
            fs::create_dir(&d).unwrap();
            let input = |field: &str| -> Vec<u8> {
                match c[field].as_str() {
                    Some(text) => text.as_bytes().to_vec(),
                    None => serde_json::from_value(c[field].clone()).unwrap(),
                }
            };
            let b = input("base");
            let o = input("ours");
            let t = input("theirs");
            let destination = d.join("ours");
            fs::write(&destination, &o).unwrap();
            let actual = sk_workspace::merge_drivers::run_driver_into(
                c["driver"].as_str().unwrap(),
                sk_workspace::merge_drivers::MergeDriverInputs {
                    base: Some(&b),
                    ours: Some(&o),
                    theirs: Some(&t),
                },
                &destination,
            );
            assert_eq!(
                actual.is_ok(),
                c["ok"].as_bool().unwrap(),
                "{}: {actual:?}",
                c["name"]
            );
            if let Err(error) = &actual {
                let name = c["name"].as_str().unwrap();
                if !name.starts_with("unicode-blank-") && !name.starts_with("review-utf8-") {
                    assert_eq!(
                        error.to_string(),
                        c["error"].as_str().unwrap(),
                        "{name}: exact refusal text"
                    );
                }
            }
            let expected: Vec<u8> = serde_json::from_value(c["bytes"].clone()).unwrap();
            assert_eq!(fs::read(&destination).unwrap(), expected, "{}", c["name"]);
            if let Ok(outcome) = actual {
                assert_eq!(outcome.bytes, expected, "{} returned bytes", c["name"]);
                if let Some(present) = c.get("notice_present") {
                    assert_eq!(
                        outcome.notice.is_some(),
                        present.as_bool().unwrap(),
                        "{} notice presence (not its still-open wire text)",
                        c["name"]
                    );
                }
            }
        }
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("H5 regression evidence: {}", root.display());
    let source=isolated(&root,&repo,oracle::python()).args(["-c",r#"
import hashlib,json,os,sys
from pathlib import Path
root=Path(os.environ['SK_H5_ROOT'])
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
repo=Path(os.environ['PYTHONPATH']).parent
assert Path(os.environ['PYTHONPATH'])==repo/'src'
assert Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
from specify_cli.consolidation.drivers import MERGE_DRIVER_BODIES,MergeDriverError
cases=[]
def case(name,driver,base,ours,theirs,ok,check):
    d=root/('source-'+str(len(cases)));d.mkdir()
    for n,v in [('base',base),('ours',ours),('theirs',theirs)]: (d/n).write_bytes(v if isinstance(v,bytes) else v.encode('utf-8'))
    before={n:(d/n).read_bytes() for n in ['base','theirs']}
    error=None;outcome=None
    try: outcome=MERGE_DRIVER_BODIES[driver](str(d/'base'),str(d/'ours'),str(d/'theirs'))
    except (MergeDriverError,UnicodeDecodeError) as e:error=str(e)
    assert (error is None)==ok,(name,error)
    result=(d/'ours').read_bytes();check(result,error)
    assert all((d/n).read_bytes()==v for n,v in before.items())
    serial=lambda v:list(v) if isinstance(v,bytes) else v
    cases.append(dict(name=name,driver=driver,base=serial(base),ours=serial(ours),theirs=serial(theirs),ok=ok,bytes=list(result),error=error,notice_present=bool(outcome and outcome.notice is not None)))
def two_history(result,error):
    assert len(json.loads(result)['acceptance_history'])==2
for x,y in [('1','1.0'),('true','1'),('0.0','-0.0')]:
    case('history-'+x+'-'+y,'merge-driver-meta','{"seed":1}',
         '{"seed":1,"acceptance_history":[{"accepted_at":'+x+',"accepted_by":"x"}]}',
         '{"seed":1,"acceptance_history":[{"accepted_at":'+y+',"accepted_by":"x"}]}',True,two_history)
def history_order(result,error):
    assert [str(e['accepted_at']) for e in json.loads(result)['acceptance_history']]==['1','3','None','True','[1]','{}']
case('history-python-str-sort','merge-driver-meta','{"seed":1}',
     '{"seed":1,"acceptance_history":[{"accepted_at":true},{"accepted_at":3},{"accepted_at":"1"},{"accepted_at":null}]}',
     '{"seed":1,"acceptance_history":[{"accepted_at":[1]},{"accepted_at":{}}]}',True,history_order)
spaces=[chr(i) for i in range(0x110000) if chr(i).isspace()]
assert len(spaces)==29
for char in spaces:
    def blank_unchanged(result,error,char=char):
        assert result==char.encode() and 'ours meta.json is empty' in error
    case('unicode-blank-'+hex(ord(char)),'merge-driver-meta','{"seed":1}',char,'{"seed":1,"theirs":2}',False,blank_unchanged)
def empty_base(result,error):assert json.loads(result)=={'theirs':2}
case('empty-object-base-two-way','merge-driver-meta','{}','\u00a0','{"theirs":2}',True,empty_base)
def event_order(result,error):assert [json.loads(line)['event_id'] for line in result.splitlines()]==['e2','e1']
for at in ['',None,False,5,0,0.0,-0.0,True,[],{},[1],{'n':1},float('nan'),float('inf')]:
    row=lambda eid,at,ts:json.dumps(dict(event_id=eid,at=at,timestamp=ts,from_lane='planned',to_lane='claimed',wp_id='W1'))+'\n'
    other='3' if at else 'a'
    case('event-falsy-or-str-'+repr(at),'merge-driver-event-log','',row('e1',at,'z'),row('e2',other,''),True,event_order)
def matrix(criteria,invariants=None):return json.dumps(dict(mission_slug='demo',criteria=criteria,negative_invariants=invariants or []))
old_unit=matrix([dict(criterion_id='FR-001',pass_fail='pass')])
def old_unit_refused(result,error):assert result==old_unit.encode() and "'description' and 'proof_type'" in error
case('old-unit-fixture-is-source-invalid','merge-driver-acceptance-matrix','{}',old_unit,old_unit,False,old_unit_refused)
unit_ours=matrix([dict(criterion_id='FR-001',description='Verify FR-001',proof_type='code_review',pass_fail='pass')])
unit_theirs=matrix([dict(criterion_id='FR-002',description='Verify FR-002',proof_type='code_review',pass_fail='pending')])
def repaired_unit(result,error):
    document=json.loads(result)
    assert document['overall_verdict']=='pending' and [row['criterion_id'] for row in document['criteria']]==['FR-001','FR-002']
case('repaired-unit-fixture-is-source-valid','merge-driver-acceptance-matrix','{}',unit_ours,unit_theirs,True,repaired_unit)
criterion=dict(criterion_id='c',description='real requirement',proof_type='code_review',pass_fail='pass')
for fields,label in [({'notes':'======='},'marker'),({'description':None},'description-null-is-not-missing')]:
    row={**criterion,**fields}
    success=label!='marker'
    def check(result,error,success=success):
        if success:assert json.loads(result)['overall_verdict']=='pass'
        else:assert 'unresolved conflict marker' in error
    case('acceptance-'+label,'merge-driver-acceptance-matrix','{}',matrix([row]),matrix([row]),success,check)
for field in ['description','proof_type']:
    row={k:v for k,v in criterion.items() if k!=field}
    original=matrix([row])
    def missing(result,error,original=original,field=field):
        assert result==original.encode() and 'required positional argument' in error and field in error
    case('criterion-missing-'+field,'merge-driver-acceptance-matrix','{}',original,original,False,missing)
invariant=dict(invariant_id='n',description='no X')
def missing_invariant(result,error):assert 'verification_method' in error and 'required positional argument' in error
case('invariant-missing-method','merge-driver-acceptance-matrix','{}',matrix([],[invariant]),matrix([],[invariant]),False,missing_invariant)
scaffold={**criterion,'criterion_id':'placeholder','description':'TODO: replace with a real acceptance criterion','pass_fail':'pending'}
for rows,expected,label in [([scaffold,criterion],'pass','real-pass'),([scaffold],'pending','only-scaffold'),([{**criterion,'notes':'TODO: replace with a real acceptance criterion','pass_fail':'pending'}],'pending','notes-not-description'),([{**criterion,'pass_fail':None}],'fail','non-string-verdict')]:
    def verdict(result,error,expected=expected):assert json.loads(result)['overall_verdict']==expected
    case('scaffold-'+label,'merge-driver-acceptance-matrix','{}',matrix(rows),matrix(rows),True,verdict)
def ignored_marker(result,error):assert json.loads(result)['criteria']==[]
removed=matrix([{**criterion,'notes':'======='}])
case('discarded-marker-is-not-early-refusal','merge-driver-acceptance-matrix',removed,matrix([]),removed,True,ignored_marker)
def extras_marker(result,error):assert json.loads(result)['criteria'][0]['custom']=='======='
extras=matrix([{**criterion,'custom':'======='}])
case('extras-marker-is-authored-not-guarded','merge-driver-acceptance-matrix','{}',extras,extras,True,extras_marker)
for rows in ['abc','',False,0,0.0]:
    def ignored_rows(result,error):assert json.loads(result)=={'schema_version':1,'rows':{}}
    issue=json.dumps({'rows':rows})
    case('issue-iterable-or-falsy-'+repr(rows),'merge-driver-issue-matrix','{}',issue,issue,True,ignored_rows)
valid_invariant=dict(invariant_id='n',description='no X',verification_method='grep_absence')
for section,template,fields in [('criteria',criterion,['criterion_id','description','proof_type','evidence','pass_fail','verified_by','verified_at','notes']),('negative_invariants',valid_invariant,['invariant_id','description','verification_method','verification_command','result','evidence','scope','verified_ref','verified_surface_kind','deferred_reason','deferred_to_phase','provenance_origin'])]:
    for field in fields:
        for marker in ['<<<<<<<','=======','>>>>>>>']:
            row={**template,field:marker}
            document=matrix([row]) if section=='criteria' else matrix([],[row])
            def guarded(result,error):assert 'unresolved conflict marker' in error
            case('guard-'+section+'-'+field+'-'+marker,'merge-driver-acceptance-matrix','{}',document,document,False,guarded)
def null_origin(result,error):assert json.loads(result)['negative_invariants'][0]['provenance_origin'] is None
ni=matrix([],[{**valid_invariant,'provenance_origin':None}])
case('explicit-null-origin-is-authored','merge-driver-acceptance-matrix','{}',ni,ni,True,null_origin)
def invalid_review(result,error):assert result==b'bad \x80\r\n' and 'decode' in error
case('review-utf8-invalid','merge-driver-review-cycle','',b'bad \x80\r\n','other\n',False,invalid_review)
for text in ['a\r\nb\r','a\rb\r','a\nb\n']:
    def normalized_review(result,error):assert result==b'a\nb\n'
    case('review-newline-'+repr(text),'merge-driver-review-cycle','',text,'a\nb\n',True,normalized_review)
# A sentinel belongs to its named verdict field, not every field on that row.
for section,template,extra in [('criteria',criterion,'result'),('negative_invariants',valid_invariant,'pass_fail')]:
    for field in ['description',extra]:
        docs=[]
        for value in ['base','target','incoming']:
            row={**template,field:value}
            docs.append(matrix([row]) if section=='criteria' else matrix([],[row]))
        def target_wins(result,error,section=section,field=field):assert json.loads(result)[section][0][field]=='target'
        case('non-verdict-'+section+'-'+field,'merge-driver-acceptance-matrix',*docs,True,target_wins)
for field in ['notes','pass_fail']:
    docs=[json.dumps({'rows':{'#1':{'verdict':'pass',field:value}}}) for value in ['base','target','incoming']]
    def issue_target(result,error,field=field):assert json.loads(result)['rows']['#1'][field]=='target'
    case('non-verdict-issue-'+field,'merge-driver-issue-matrix',*docs,True,issue_target)
for row_id,left,right in [('c','pass','fail'),('\ud800',"a'b",'c"d')]:
    documents=[matrix([{**criterion,'criterion_id':row_id,'pass_fail':value}]) for value in ['pending',left,right]]
    def verdict_refused(result,error):assert 'verdict field' in error and 'diverged on both sides' in error
    case('verdict-source-repr-'+repr(row_id),'merge-driver-acceptance-matrix',*documents,False,verdict_refused)
for raw_key in ['#1','\ud800']:
    dup=json.dumps({'rows':{raw_key:{'verdict':'pass'},' '+raw_key:{'verdict':'fail'}}})
    def duplicate_refused(result,error):assert 'intra-side duplicate row key' in error and '(#2970-adjacent row-merge guard)' in error
    case('duplicate-refusal-'+repr(raw_key),'merge-driver-issue-matrix','{}',dup,'{}',False,duplicate_refused)
ours_fork=json.dumps(dict(event_id='e1',wp_id='W1',from_lane='in_progress',to_lane='canceled'))+'\n'
theirs_fork=json.dumps(dict(event_id='e2',wp_id='W1',from_lane='in_progress',to_lane='for_review'))+'\n'
def fork_refused(result,error):assert "['canceled', 'for_review']" in error
case('fork-source-repr-refusal','merge-driver-event-log','',ours_fork,theirs_fork,False,fork_refused)
from specify_cli.consolidation.drivers import union_decision_index
uours={'mission_id':'','version':''};utheirs={'mission_id':'01KN2371WNZX28ME4MN97B67PJ','version':2,'custom':1}
uexpected=union_decision_index(uours,utheirs)
assert uexpected['mission_id']==utheirs['mission_id'] and uexpected['version']==2
# A SEPARATE direct call proves absence of version is not a driver default.
unversioned=union_decision_index({}, {})
assert unversioned=={'entries':[]}
(root/'pure-union.json').write_text(json.dumps(dict(ours={},theirs={},expected=unversioned)))
(root/'pure-union-fallback.json').write_text(json.dumps(dict(ours=uours,theirs=utheirs,expected=uexpected)))
(root/'cases.json').write_text(json.dumps(cases))
print(json.dumps(dict(cases=len(cases),source_sha256=hashlib.sha256((repo/'src/specify_cli/consolidation/drivers.py').read_bytes()).hexdigest(),version=sys.version,spaces=[ord(c) for c in spaces])))
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
            "history_blankness_and_event_sort_match_actual_source",
            "--nocapture",
        ])
        .env("SK_H5_NATIVE", &root)
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
