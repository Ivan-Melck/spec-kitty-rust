//! E5 small identity defects: malformed files are not identities; valid missing
//! IDs still fall back, directive layer lookup normalises, floats use one codec.
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
        .env("SK_ID_REVIEW_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (k, n) in roots() {
        c.env(k, root.join(n));
    }
    c
}
#[test]
fn identity_admission_normalisation_and_numbers_match_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_ID_REVIEW_NATIVE") {
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
        let index = sk_doctrine::IdentityIndex::from_roots(
            &repo.join("packs/built-in"),
            &[],
            Some(&root.join("project")),
        );
        for row in expected["rows"].as_array().unwrap() {
            let stem = row["stem"].as_str().unwrap();
            assert_eq!(
                index
                    .declared_id(sk_charter::ArtifactKind::Directive, stem)
                    .as_deref(),
                row["id"].as_str(),
                "{stem}"
            );
            assert_eq!(
                index.knows(sk_charter::ArtifactKind::Directive, stem),
                row["known"].as_bool().unwrap(),
                "{stem}"
            );
        }
        for spelling in ["1", "directive:1", "DIRECTIVE_001", "900-normal"] {
            assert_eq!(
                index.source_layer(sk_charter::ArtifactKind::Directive, spelling),
                Some(sk_doctrine::IdentityLayer::Project),
                "{spelling}: canonical normalised lookup applies to layer too"
            );
        }
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, n) in roots() {
        fs::create_dir_all(root.join(n)).unwrap();
    }
    eprintln!("identity review evidence: {}", root.display());
    let source=child(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_ID_REVIEW_ROOT']);repo=Path(os.environ['PYTHONPATH']).parent
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for k,n in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[k])==root/n
assert Path(os.environ['PYTHONPATH'])==repo/'src' and Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
from charter.activation.identity_index import IdentityIndex
from charter.offering.artifact_kinds import ArtifactKind
bodies={'900-normal':'id: DIRECTIVE_001\n','901-malformed':'id: [broken\n','902-missing':'title: no id\n','903-blank':"id: '   '\n",'904-scalar':'not a mapping\n','905-float':'id: 1e20\n','906-small':'id: 1e-7\n','907-nan':'id: .nan\n'}
d=root/'project/directive';d.mkdir(parents=True)
for stem,body in bodies.items():(d/(stem+'.directive.yaml')).write_text(body)
index=IdentityIndex(org_roots=[],project_root=root/'project',offering_root=repo/'packs/built-in')
kind=ArtifactKind.DIRECTIVE
rows=[dict(stem=stem,id=index.declared_id(kind,stem),known=index.knows(kind,stem)) for stem in bodies]
by={r['stem']:r for r in rows}
assert by['901-malformed']==dict(stem='901-malformed',id=None,known=False)
for stem in ['902-missing','903-blank','904-scalar']:assert by[stem]['id']==stem and by[stem]['known']
assert by['905-float']['id']=='1e+20' and by['906-small']['id']=='1e-07' and by['907-nan']['id']=='nan'
for spelling in ['1','directive:1','DIRECTIVE_001','900-normal']:assert index.source_layer(kind,spelling)=='project'
print(json.dumps(dict(rows=rows)))
"#]).output().unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    let native = child(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "identity_admission_normalisation_and_numbers_match_source",
            "--nocapture",
        ])
        .env("SK_ID_REVIEW_NATIVE", &root)
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
