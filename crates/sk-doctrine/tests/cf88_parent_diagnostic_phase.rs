//! Parent bounded controls: real relative-path phase comparison, no output rewriting.
//! Non-scalar diagnostic support is still missing; its failure must be Native, not
//! an ordinary offering failure that downstream callers may treat as unresolved.
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use sk_charter::ArtifactKind;
use sk_doctrine::{CanonicalOfferingService, OfferingContext, OfferingRoots};

const ROOTS: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "configdirs"),
    ("XDG_DATA_DIRS", "datadirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];
const BAD: &str = "builtin/directives/bad.directive.yaml";
const INLINE: &str = "id: DIRECTIVE_888\ntactic_refs: []\n";
const SKIP: &str = "- nonmapping\n";
const SURROGATE: &str = "id: \"\\uD800\"\ntactic_refs: []\n";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (_, name) in ROOTS {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
    }
    std::fs::create_dir_all(root.path().join("builtin/directives")).unwrap();
    std::fs::write(
        root.path().join("builtin/directives/good.directive.yaml"),
        "schema_version: '1.0'\nid: DIRECTIVE_001\ntitle: Good\nintent: I\nenforcement: required\n",
    )
    .unwrap();
    std::fs::write(root.path().join(BAD), INLINE).unwrap();
    root
}

fn service(root: &Path, builtin: PathBuf) -> CanonicalOfferingService {
    CanonicalOfferingService::new(OfferingContext::new(
        root.to_path_buf(),
        OfferingRoots {
            built_in: Some(builtin),
            org_roots: vec![],
            project: None,
        },
        None,
    ))
}

fn environment(command: &mut Command, root: &Path) {
    let expected = repo();
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CF88_PARENT_ROOT", root)
        .env("CF88_PARENT_SOURCE", &expected)
        .env("PYTHONPATH", expected.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", expected.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in ROOTS {
        command.env(key, root.join(name));
    }
}

fn native_guards(root: &Path) {
    assert!(root.is_absolute());
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name))
        );
    }
    let expected = repo();
    assert_eq!(
        std::env::var_os("CF88_PARENT_SOURCE").map(PathBuf::from),
        Some(expected.clone())
    );
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(expected.join("src"))
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(expected.join("packs"))
    );
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    let stdout = sk_kernel::python_stream::StreamPolicy::from_environment().unwrap();
    assert_eq!(
        (stdout.encoding(), stdout.errors()),
        ("utf-8", "surrogateescape")
    );
    let stderr = sk_kernel::python_stream::StreamPolicy::for_stderr().unwrap();
    assert_eq!(
        (stderr.encoding(), stderr.errors()),
        ("utf-8", "backslashreplace")
    );
    let output = Command::new(expected.join(".venv-rust-oracle/bin/python"))
        .arg(expected.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(&expected)
        .current_dir(&expected)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let proof: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["python"], "3.11.15");
    assert_eq!(proof["checked_runtime_requirements"], 53);
}

const SOURCE: &str = r#"
import json, os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['CF88_PARENT_ROOT'])
expected = pathlib.Path(os.environ['CF88_PARENT_SOURCE'])
assert root.is_absolute() and pathlib.Path.cwd() == root
assert expected.is_absolute() and expected.is_dir()
for key, name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','configdirs'),('XDG_DATA_DIRS','datadirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert pathlib.Path(os.environ[key]) == root / name
assert pathlib.Path(os.environ['PYTHONPATH']) == expected / 'src'
assert pathlib.Path(os.environ['SPEC_KITTY_PACKS_ROOT']) == expected / 'packs'
assert os.environ['PYTHONHASHSEED'] == '0'
assert sys.version_info[:3] == (3,11,15)
assert (sys.getfilesystemencoding(), sys.getfilesystemencodeerrors()) == ('utf-8','surrogateescape')
assert (sys.stdout.encoding, sys.stdout.errors) == ('utf-8','surrogateescape')
assert (sys.stderr.encoding, sys.stderr.errors) == ('utf-8','backslashreplace')
import charter
assert pathlib.Path(charter.__file__).resolve().is_relative_to(expected / 'src')
proof = json.loads(subprocess.check_output([str(expected / '.venv-rust-oracle/bin/python'), str(expected / 'scripts/rust/verify_oracle.py'), '--root', str(expected)], cwd=expected, text=True))
assert proof['ok'] and proof['python'] == '3.11.15' and proof['checked_runtime_requirements'] == 53
from charter.offering.directives.repository import DirectiveRepository
path = pathlib.Path('builtin/directives/bad.directive.yaml')
def load():
    return DirectiveRepository(built_in_dir=pathlib.Path('builtin/directives'))
try:
    load()
except Exception as exc:
    answer = {'class': type(exc).__name__, 'message': str(exc)}
else:
    raise AssertionError('Source must abort before schema validation')
assert answer['class'] == 'InlineReferenceRejectedError'
assert 'builtin/directives/bad.directive.yaml' in answer['message']
path.write_text('- nonmapping\n', encoding='utf-8')
answer['skip'] = sorted(item.id for item in load().list_all())
path.write_text('id: "\\uD800"\ntactic_refs: []\n', encoding='utf-8')
try:
    load()
except Exception as exc:
    assert type(exc).__name__ == 'InlineReferenceRejectedError'
    assert '\ud800' in str(exc)
else:
    raise AssertionError('Source surrogate diagnostic must abort')
path.unlink()
raw_path = pathlib.Path('builtin/directives/bad\udcff.directive.yaml')
raw_path.write_text('id: DIRECTIVE_888\ntactic_refs: []\n', encoding='utf-8')
try:
    load()
except Exception as exc:
    assert type(exc).__name__ == 'InlineReferenceRejectedError'
    assert '\udcff' in str(exc)
else:
    raise AssertionError('Source raw filename prefix must not hide the directive')
print('CF88_PARENT ' + json.dumps(answer))
"#;

#[test]
fn independent_relative_path_prevalidation_plan() {
    if std::env::var_os("CF88_PARENT_NATIVE").is_some() {
        let root = PathBuf::from(std::env::var_os("CF88_PARENT_ROOT").unwrap());
        native_guards(&root);
        let error = service(Path::new("."), PathBuf::from("builtin"))
            .artifact_ids(ArtifactKind::Directive)
            .unwrap_err();
        let abort = error.abort().expect("Source abort, not skipped mapping");
        let class = abort.python;
        let message = abort.message.clone();
        std::fs::write(BAD, SKIP).unwrap();
        let skip = service(Path::new("."), PathBuf::from("builtin"))
            .artifact_ids(ArtifactKind::Directive)
            .unwrap();
        let skip: Vec<_> = skip.iter().map(|s| s.to_scalar_string().unwrap()).collect();
        println!(
            "CF88_PARENT {}",
            serde_json::json!({"class":class,"message":message,"skip":skip})
        );
        return;
    }
    let source_root = fixture();
    let mut source = Command::new(support::python());
    environment(&mut source, source_root.path());
    let output = source.arg("-c").arg(SOURCE).output().unwrap();
    assert!(
        output.status.success(),
        "Source: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = |bytes: &[u8]| -> serde_json::Value {
        let text = std::str::from_utf8(bytes).unwrap();
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix("CF88_PARENT "))
            .unwrap();
        serde_json::from_str(line).unwrap()
    };
    let reference = receipt(&output.stdout);
    assert_eq!(reference["class"], "InlineReferenceRejectedError");
    assert_eq!(reference["skip"], serde_json::json!(["DIRECTIVE_001"]));
    let native_root = fixture();
    assert_ne!(source_root.path(), native_root.path());
    let mut native = Command::new(std::env::current_exe().unwrap());
    environment(&mut native, native_root.path());
    let output = native
        .env("CF88_PARENT_NATIVE", "1")
        .args([
            "--exact",
            "independent_relative_path_prevalidation_plan",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        receipt(&output.stdout),
        reference,
        "raw relative-path messages, no output rewriting"
    );
}

#[test]
fn non_scalar_prevalidation_is_native_not_an_ordinary_failed_load() {
    let root = fixture();
    std::fs::write(root.path().join(BAD), SURROGATE).unwrap();
    let service = service(root.path(), root.path().join("builtin"));
    let error = service.artifact_ids(ArtifactKind::Directive).unwrap_err();
    assert!(
        error.native().is_some(),
        "a scalar-only diagnostic limit is Native, not Failed: {error:?}"
    );
    assert!(error.abort().is_none());
    assert!(std::error::Error::source(&error).is_some());
    match error.native().unwrap() {
        sk_doctrine::NativeOfferingFailure::DiagnosticProjection {
            exact,
            codepoint,
            path,
            ..
        } => {
            assert_eq!(*codepoint, 0xd800);
            assert_eq!(exact.codepoints(), &[0xd800]);
            assert_eq!(path, &root.path().join(BAD));
        }
        other => panic!("wrong Native cause: {other:?}"),
    }
    std::fs::remove_file(root.path().join(BAD)).unwrap();
    assert_eq!(
        service.artifact_ids(ArtifactKind::Directive).unwrap().len(),
        1,
        "failed construction must retry on the same service"
    );
}

#[cfg(unix)]
#[test]
fn undecodable_path_prevalidation_is_native_not_an_ordinary_failed_load() {
    use std::os::unix::ffi::OsStrExt;
    let root = fixture();
    std::fs::remove_file(root.path().join(BAD)).unwrap();
    let name = std::ffi::OsStr::from_bytes(b"bad\xff.directive.yaml");
    std::fs::write(root.path().join("builtin/directives").join(name), INLINE).unwrap();
    let error = service(root.path(), root.path().join("builtin"))
        .artifact_ids(ArtifactKind::Directive)
        .unwrap_err();
    assert!(
        error.native().is_some(),
        "surrogateescape diagnostic cannot become ordinary Failed: {error:?}"
    );
    assert!(error.abort().is_none());
    match error.native().unwrap() {
        sk_doctrine::NativeOfferingFailure::DiagnosticProjection {
            exact,
            codepoint,
            path,
            ..
        } => {
            assert_eq!(*codepoint, 0xdcff);
            assert!(exact.codepoints().contains(&0xdcff));
            assert_eq!(path.file_name(), Some(name));
        }
        other => panic!("wrong Native cause: {other:?}"),
    }
}
