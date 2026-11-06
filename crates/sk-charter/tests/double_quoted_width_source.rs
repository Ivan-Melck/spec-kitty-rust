//! Actual Source width controls escaped/double-quoted scalars, not just plain prose.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
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
fn command(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_WIDTH_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in env_roots() {
        c.env(key, root.join(name));
    }
    c
}
#[test]
fn escaped_scalar_folding_uses_the_requested_source_width() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_WIDTH_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (key, name) in env_roots() {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
        }
        let cases: Value =
            serde_json::from_slice(&fs::read(root.join("cases.json")).unwrap()).unwrap();
        let expected: Value =
            serde_json::from_slice(&fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let mut actual = Vec::new();
        for case in cases.as_array().unwrap() {
            let mut mapping = serde_yaml_ng::Mapping::new();
            mapping.insert("details".into(), case["text"].as_str().unwrap().into());
            let value = serde_yaml_ng::Value::Mapping(mapping);
            let width = case["width"].as_u64().unwrap() as usize;
            let bytes = sk_charter::ordered_yaml_width_normalized(&value, width);
            actual.push(String::from_utf8(bytes).unwrap());
        }
        assert_eq!(
            json!(actual),
            expected,
            "whole Source bytes at every requested width, no layout waiver"
        );
        return;
    }
    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in env_roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let texts = [
        "This detail is deliberately longer than one hundred and twenty characters so the emitter folds it exactly as Source folds a real Finding.details field, and it also carries an embedded newline:\nsecond paragraph with trailing words.",
        "An escaped quote \"inside\" and a backslash \\ keep this long double-quoted text from changing its scalar style while words continue past several folding points with  double spaces and a newline:\nlast paragraph with café 日本語 🎉.",
    ];
    let cases: Vec<_> = [40, 80, 120, 4096]
        .into_iter()
        .flat_map(|width| {
            texts
                .iter()
                .map(move |text| json!({"width":width,"text":text}))
        })
        .collect();
    fs::write(root.join("cases.json"), serde_json::to_vec(&cases).unwrap()).unwrap();
    let source=command(&root,&repo,oracle::python()).args(["-c",r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_WIDTH_ROOT']);assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for key,name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:assert Path(os.environ[key])==root/name
from kernel.yaml_io import serialize_mapping
cases=json.loads((root/'cases.json').read_text());assert len(cases)==8
outputs=[serialize_mapping({'details':case['text']},width=case['width']).decode('utf-8') for case in cases]
assert all('details: "' in text for text in outputs)
assert all(outputs[index]!=outputs[index+2] for index in (0,2,4)),outputs
print(json.dumps(outputs))
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
            "escaped_scalar_folding_uses_the_requested_source_width",
            "--nocapture",
        ])
        .env("SK_WIDTH_NATIVE", &root)
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
