//! One bounded raw-service recovery plan, not whole-record/codec/concurrency proof.
mod support;
use serde_json::{Value, json};
use sk_doctrine::{CanonicalOfferingService, OfferingContext, OfferingRoots};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOTS: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn command_env(command: &mut Command, root: &Path) {
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CF84_PARENT_ROOT", root)
        .env("CF84_PARENT_SOURCE", repo())
        .env("PYTHONPATH", repo().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo().join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("LC_ALL", "C.UTF-8")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in ROOTS {
        fs::create_dir_all(root.join(name)).unwrap();
        command.env(key, root.join(name));
    }
}
fn native_guard(root: &Path) {
    assert!(root.is_absolute());
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name))
        );
    }
    for (key, expected) in [
        ("CF84_PARENT_SOURCE", repo()),
        ("PYTHONPATH", repo().join("src")),
        ("SPEC_KITTY_PACKS_ROOT", repo().join("packs")),
    ] {
        assert_eq!(std::env::var_os(key).map(PathBuf::from), Some(expected));
    }
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    let stream = sk_kernel::python_stream::StreamPolicy::for_stderr().unwrap();
    assert_eq!(stream.encoding(), "utf-8");
    assert_eq!(stream.errors(), "backslashreplace");
}
fn native_service(root: &Path) -> CanonicalOfferingService {
    CanonicalOfferingService::new(OfferingContext::new(
        root.to_path_buf(),
        OfferingRoots {
            built_in: Some(repo().join("packs/built-in")),
            org_roots: vec![root.join("org")],
            project: None,
        },
        None,
    ))
}
fn native_business(root: &Path) -> Value {
    let dir = root.join("org/tactics");
    fs::create_dir_all(&dir).unwrap();
    let template =
        fs::read_to_string(repo().join("packs/built-in/tactics/moscow-scoping-lens.tactic.yaml"))
            .unwrap();
    let body = |id: &str| template.replace("id: moscow-scoping-lens", &format!("id: {id}"));
    fs::write(dir.join("a.tactic.yaml"), body("cf84-kale")).unwrap();
    fs::write(dir.join("z.tactic.yaml"), "id: !!timestamp 2021-13-45\n").unwrap();
    let service = native_service(root);
    let first = service.ids_for("activated_tactics").unwrap_err();
    let abort = first.abort().expect("real constructor abort");
    assert!(abort.message.contains("month must be in 1..12"));
    let class = abort.python.to_owned();
    fs::write(dir.join("z.tactic.yaml"), body("cf84-lemon")).unwrap();
    fs::write(dir.join("born.tactic.yaml"), body("cf84-mango")).unwrap();
    let selected = |service: &CanonicalOfferingService| {
        let mut ids: Vec<String> = service
            .ids_for("activated_tactics")
            .unwrap()
            .iter()
            .map(|id| id.to_scalar_string().unwrap())
            .filter(|id| id.starts_with("cf84-"))
            .collect();
        ids.sort();
        ids
    };
    let recovered = selected(&service);
    fs::write(dir.join("a.tactic.yaml"), body("cf84-changed")).unwrap();
    let warm = selected(&service);
    let fresh = selected(&native_service(root));
    json!({"class": class, "recovered": recovered, "warm": warm, "fresh": fresh})
}
const SOURCE: &str = r#"
import os, sys, json
from pathlib import Path
root = Path(os.environ['CF84_PARENT_ROOT'])
expected = Path(os.environ['CF84_PARENT_SOURCE'])
assert root.is_absolute() and expected.is_absolute() and Path.cwd() == root
for key, name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','config-dirs'),('XDG_DATA_DIRS','data-dirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert Path(os.environ[key]) == root / name
assert Path(os.environ['PYTHONPATH']) == expected / 'src'
assert Path(os.environ['SPEC_KITTY_PACKS_ROOT']) == expected / 'packs'
assert os.environ['PYTHONHASHSEED'] == '0'
assert sys.version_info[:3] == (3,11,15)
assert (sys.getfilesystemencoding(),sys.getfilesystemencodeerrors()) == ('utf-8','surrogateescape')
assert (sys.stderr.encoding,sys.stderr.errors) == ('utf-8','backslashreplace')
import charter
assert Path(charter.__file__).resolve().is_relative_to(expected / 'src')
from charter.offering.service import CharterOfferingService
folder = root / 'org/tactics'
folder.mkdir(parents=True)
template = (expected / 'packs/built-in/tactics/moscow-scoping-lens.tactic.yaml').read_text(encoding='utf-8')
def body(name): return template.replace('id: moscow-scoping-lens','id: '+name)
(folder/'a.tactic.yaml').write_text(body('cf84-kale'),encoding='utf-8')
(folder/'z.tactic.yaml').write_text('id: !!timestamp 2021-13-45\n',encoding='utf-8')
def service(): return CharterOfferingService(org_roots=[root/'org'])
svc = service()
try:
    svc.tactics
except ValueError as error:
    assert 'month must be in 1..12' in str(error)
    category = type(error).__name__
else: raise AssertionError('must actually abort')
(folder/'z.tactic.yaml').write_text(body('cf84-lemon'),encoding='utf-8')
(folder/'born.tactic.yaml').write_text(body('cf84-mango'),encoding='utf-8')
def selected(svc): return sorted(item.id for item in svc.tactics.list_all() if item.id.startswith('cf84-'))
recovered = selected(svc)
(folder/'a.tactic.yaml').write_text(body('cf84-changed'),encoding='utf-8')
result = dict(class_=category,recovered=recovered,warm=selected(svc),fresh=selected(service()))
result['class'] = result.pop('class_')
(root/'receipt.json').write_text(json.dumps(result),encoding='utf-8')
"#;

#[test]
fn independent_same_instance_service_recovery() {
    if let Some(root) = std::env::var_os("CF84_PARENT_ROOT").map(PathBuf::from) {
        native_guard(&root);
        fs::write(
            root.join("receipt.json"),
            serde_json::to_vec(&native_business(&root)).unwrap(),
        )
        .unwrap();
        return;
    }
    let source = tempfile::tempdir().unwrap();
    let native = tempfile::tempdir().unwrap();
    assert_ne!(source.path(), native.path());
    let mut source_cmd = Command::new(support::python());
    command_env(&mut source_cmd, source.path());
    let output = source_cmd.args(["-c", SOURCE]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source_receipt: Value =
        serde_json::from_slice(&fs::read(source.path().join("receipt.json")).unwrap()).unwrap();
    assert_eq!(
        source_receipt["recovered"],
        json!(["cf84-kale", "cf84-lemon", "cf84-mango"])
    );
    let mut native_cmd = Command::new(std::env::current_exe().unwrap());
    command_env(&mut native_cmd, native.path());
    let output = native_cmd
        .args([
            "--exact",
            "independent_same_instance_service_recovery",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let native_receipt: Value =
        serde_json::from_slice(&fs::read(native.path().join("receipt.json")).unwrap()).unwrap();
    assert_eq!(source_receipt, native_receipt);
    assert_eq!(source_receipt["class"], "ValueError");
    assert_eq!(source_receipt["warm"], source_receipt["recovered"]);
    assert_eq!(
        source_receipt["fresh"],
        json!(["cf84-changed", "cf84-lemon", "cf84-mango"])
    );
}
