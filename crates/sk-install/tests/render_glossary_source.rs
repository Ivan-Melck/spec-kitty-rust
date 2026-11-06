//! Actual Source-first matching, ancestry and byte parity for glossary rendering.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use std::path::{Path, PathBuf};
use std::process::Command;

fn command(program: &Path, root: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("SK_RENDER_ROOT", root)
        .env("HOME", root.join("home"))
        .env("TMPDIR", root.join("tmp"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(root);
    command
}
#[test]
fn glossary_regex_and_ancestry_match_actual_source() {
    let root = tempfile::tempdir().unwrap().keep();
    for directory in ["home", "tmp", "config", "cache", "data", "state"] {
        std::fs::create_dir(root.join(directory)).unwrap();
    }
    let cases = [
        ("sis", "Sıſ"),
        ("i", "İ"),
        ("σ", "ς"),
        ("μ", "µ"),
        ("c++", "C++x"),
        (".net", "a.NET "),
        ("mission", "ordinary Mission here"),
    ];
    let mut rows = Vec::new();
    for (index, (surface, content)) in cases.into_iter().enumerate() {
        let project = root.join(format!("case-{index}"));
        std::fs::create_dir_all(project.join(".kittify/glossaries")).unwrap();
        std::fs::create_dir(project.join("templates")).unwrap();
        std::fs::write(project.join(".kittify/glossaries/spec_kitty_core.yaml"),format!("terms:\n  - surface: '{surface}'\n    definition: A term.\n    confidence: 0.9\n    status: active\n")).unwrap();
        rows.push(serde_json::json!([
            surface,
            content,
            project.join("templates/prompt.md")
        ]));
    }
    // None-path Source resolves cwd.parents, NOT the cwd itself.
    std::fs::create_dir_all(root.join(".kittify/glossaries")).unwrap();
    std::fs::write(root.join(".kittify/glossaries/spec_kitty_core.yaml"),"terms:\n  - surface: mission\n    definition: A term.\n    confidence: 0.9\n    status: active\n").unwrap();
    std::fs::write(root.join("cases.json"), serde_json::to_vec(&rows).unwrap()).unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source=command(&oracle::python(),&root).env("PYTHONPATH",repository.join("src"))
        .args(["-c",r#"
import json,os,sys
from pathlib import Path
from specify_cli.template.renderer import _annotate_glossary_refs_from_store
root=Path(os.environ['SK_RENDER_ROOT']);assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME'):assert Path(os.environ[key]).parent==root
rows=json.loads((root/'cases.json').read_text())
results=[_annotate_glossary_refs_from_store(content,Path(path)) for surface,content,path in rows]
for (surface,content,path),result in zip(rows,results):assert '<!-- glossary:glossary:'+surface+' -->' in result,(surface,result)
none=_annotate_glossary_refs_from_store('mission',None)
assert none=='mission',none
print(json.dumps([results,none]))
"#]).output().unwrap();
    std::fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    std::fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source failed before Native: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    let expected: serde_json::Value = serde_json::from_slice(&source.stdout).unwrap();
    assert_eq!(expected[0].as_array().unwrap().len(), 7);
    let native = command(&std::env::current_exe().unwrap(), &root)
        .args([
            "--ignored",
            "--exact",
            "native_glossary_helper",
            "--nocapture",
        ])
        .output()
        .unwrap();
    std::fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    std::fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    assert!(
        native.status.success(),
        "Native failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let output = String::from_utf8(native.stdout).unwrap();
    let line = output
        .lines()
        .find_map(|line| line.strip_prefix("GLOSSARY_RESULT="))
        .expect("Native results");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(line).unwrap(),
        expected
    );
}
#[test]
#[ignore = "exercised by isolated Source-first parent"]
fn native_glossary_helper() {
    let root = PathBuf::from(std::env::var_os("SK_RENDER_ROOT").expect("root"));
    assert_eq!(std::env::current_dir().unwrap(), root);
    for key in [
        "HOME",
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
    ] {
        assert_eq!(
            PathBuf::from(std::env::var_os(key).unwrap()).parent(),
            Some(root.as_path())
        );
    }
    let rows: Vec<(String, String, PathBuf)> =
        serde_json::from_slice(&std::fs::read(root.join("cases.json")).unwrap()).unwrap();
    let results: Vec<_> = rows
        .iter()
        .map(|(_, content, path)| sk_install::render::annotate_glossary_refs(content, Some(path)))
        .collect();
    let none = sk_install::render::annotate_glossary_refs("mission", None);
    println!("GLOSSARY_RESULT={}", serde_json::json!([results, none]));
}
