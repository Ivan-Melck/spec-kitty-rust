//! CF69 — paired Source/Native live provenance read for a fresh raw-byte
//! filesystem filename under an actually installed Latin-1 startup locale.
//!
//! Both children are real processes:
//! * Source is the pinned CPython 3.11.15 oracle reading through
//!   `charter.activation._io.load_charter_file` and `os.fsdecode`;
//! * Native is this test binary re-executed, reading the same independent
//!   fixture through `sk_doctrine::load_document_text` and the Kernel
//!   provenance journal.
//!
//! Each child asserts temporary CWD/HOME, all seven XDG roots, TMPDIR/TMP/TEMP,
//! the absolute repository `src` and pinned `packs` roots, `PYTHONHASHSEED=0`,
//! and the requested actual startup codec before any read. The fixture is a
//! fresh raw-byte filename, never copied from a linked repository.
//!
//! Not a whole-command or cross-platform parity claim.

#![cfg(unix)]

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use sk_kernel::encoding_provenance::EncodingProvenanceJournal;
use sk_kernel::python_os_encoding::OsEncodingContext;

const RAW_NAME: &[u8] = b"caf\xe9.md";
const EXTRA_XDG: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

fn fixture(case: &str) -> PathBuf {
    let root = isolated::fixture(case);
    let charter = root.join(".kittify/charter");
    std::fs::create_dir_all(&charter).expect("charter dir");
    std::fs::write(charter.join("charter.md"), "# Charter\n").expect("charter");
    std::fs::write(charter.join(OsStr::from_bytes(RAW_NAME)), "# Raw name\n")
        .expect("raw filename fixture");
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("-b")
        .arg("main")
        .arg(&root)
        .status()
        .expect("git available");
    assert!(status.success(), "git init failed");
    root
}

const SOURCE_PROBE: &str = r#"
import json, os, pathlib, sys

root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["SK_CF69_SOURCE_ROOT"])
assert repo.is_absolute(), repo
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
assert os.environ["PYTHONHASHSEED"] == "0", os.environ["PYTHONHASHSEED"]
assert os.environ["PYTHONUTF8"] == "0", os.environ["PYTHONUTF8"]
assert sys.getfilesystemencoding() == "iso8859-1", sys.getfilesystemencoding()
assert sys.getfilesystemencodeerrors() == "surrogateescape", sys.getfilesystemencodeerrors()

from charter.activation import _io
assert pathlib.Path(_io.__file__).resolve() == repo / "src/charter/activation/_io.py"
load_charter_file = _io.load_charter_file

name = os.fsdecode(b"caf\xe9.md")
path = root / ".kittify/charter" / name
content = load_charter_file(path)
global_log = root / ".kittify/encoding-provenance/global.jsonl"
record = json.loads(global_log.read_text(encoding="utf-8").splitlines()[-1])
print("CF69_RECEIPT " + json.dumps({
    "runner": "source-python-3.11.15",
    "cwd": str(pathlib.Path.cwd()),
    "startup_encoding": sys.getfilesystemencoding(),
    "source_encoding": content.source_encoding,
    "text": [ord(char) for char in content.text],
    "file_path": [ord(char) for char in record["file_path"].replace(str(root), "<ROOT>")],
    "log": str(global_log.relative_to(root)),
    "records": len(global_log.read_text(encoding="utf-8").splitlines()),
}))
"#;

const NATIVE_CHILD: &str = "cf69_native_child";

#[test]
fn cf69_native_child() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    isolated::assert_isolated(&root);
    let repo = support::repository_root();
    assert_eq!(
        std::env::var_os("SK_CF69_SOURCE_ROOT").map(PathBuf::from),
        Some(repo.clone())
    );
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(repo.join("src"))
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repo.join("packs"))
    );
    for (key, name) in EXTRA_XDG {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok("0"),
        "the Native child runs with a pinned hash seed"
    );
    assert_eq!(
        std::env::var("PYTHONUTF8").as_deref(),
        Ok("0"),
        "the Native child runs in the requested startup mode"
    );
    assert_eq!(
        OsEncodingContext::from_environment().label(),
        "iso-8859-1, surrogateescape",
        "the Native child is in the requested actual Latin-1 startup context"
    );

    let journal = EncodingProvenanceJournal::new(&root);
    let path = root
        .join(".kittify/charter")
        .join(OsStr::from_bytes(RAW_NAME));
    let read = sk_doctrine::load_document_text(&path, &journal).expect("Native read");
    let global = root.join(sk_kernel::encoding_provenance::GLOBAL_LOG_RELATIVE);
    let line = std::fs::read_to_string(&global)
        .expect("global log")
        .lines()
        .next_back()
        .expect("record")
        .to_owned();
    let value: JsonValue = serde_json::from_str(&line).expect("record JSON");
    let file_path: Vec<u32> = value["file_path"]
        .as_str()
        .expect("file_path")
        .replace(&root.display().to_string(), "<ROOT>")
        .chars()
        .map(u32::from)
        .collect();
    println!(
        "CF69_RECEIPT {}",
        serde_json::json!({
            "runner": "native",
            "cwd": std::env::current_dir().expect("cwd").display().to_string(),
            "startup_encoding": OsEncodingContext::from_environment().label(),
            "source_encoding": read.source_encoding(),
            "text": read.to_scalar_string().expect("text").chars().map(u32::from).collect::<Vec<_>>(),
            "file_path": file_path,
            "log": sk_kernel::encoding_provenance::GLOBAL_LOG_RELATIVE,
            "records": std::fs::read_to_string(&global).expect("global log").lines().count(),
        })
    );
}

fn latin1_host_available() -> bool {
    use sk_kernel::os_locale::{LocaleFact, resolve};
    let lookup = |name: &str| match name {
        "LC_ALL" => Some(std::ffi::OsString::from("en_US.ISO-8859-1")),
        _ => None,
    };
    matches!(
        resolve(&lookup, Some("en_US.ISO-8859-1")),
        LocaleFact::Resolved { .. }
    )
}

fn extra_env(root: &Path) -> Vec<(String, String)> {
    for name in ["config-dirs", "data-dirs"] {
        std::fs::create_dir_all(root.join(name)).expect("extra XDG root");
    }
    vec![
        (
            "SK_CF69_SOURCE_ROOT".to_owned(),
            support::repository_root().display().to_string(),
        ),
        ("LC_ALL".to_owned(), "en_US.ISO-8859-1".to_owned()),
        ("PYTHONUTF8".to_owned(), "0".to_owned()),
        ("PYTHONCOERCECLOCALE".to_owned(), "0".to_owned()),
        (
            "XDG_CONFIG_DIRS".to_owned(),
            root.join("config-dirs").display().to_string(),
        ),
        (
            "XDG_DATA_DIRS".to_owned(),
            root.join("data-dirs").display().to_string(),
        ),
        ("PYTHONHASHSEED".to_owned(), "0".to_owned()),
    ]
}

#[test]
fn latin1_raw_filename_matches_source_fsdecode_and_live_journal() {
    if !latin1_host_available() {
        eprintln!("skip: actual en_US.ISO-8859-1 locale data is not installed");
        return;
    }
    let source_root = fixture("cf69-source-latin1");
    let native_root = fixture("cf69-native-latin1");
    let source_env = extra_env(&source_root);
    let source_env: Vec<(&str, &str)> = source_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let source_stdout = isolated::source_script(&source_root, SOURCE_PROBE, &source_env);
    let source = isolated::receipt_json(&source_stdout, "CF69_RECEIPT ");
    assert_eq!(source["runner"], "source-python-3.11.15");
    assert_eq!(source["startup_encoding"], "iso8859-1");
    assert_eq!(
        source["file_path"]
            .as_array()
            .expect("source file_path")
            .iter()
            .rev()
            .take(7)
            .rev()
            .map(|value| value.as_u64().expect("codepoint"))
            .collect::<Vec<_>>(),
        vec![0x63, 0x61, 0x66, 0x00e9, 0x2e, 0x6d, 0x64],
        "Source os.fsdecode spells byte 0xE9 as U+00E9"
    );
    assert_eq!(source["source_encoding"], "utf-8");

    let native_env = extra_env(&native_root);
    let native_env: Vec<(&str, &str)> = native_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let native_stdout = isolated::native_reexec(&native_root, NATIVE_CHILD, &native_env);
    let native = isolated::receipt_json(&native_stdout, "CF69_RECEIPT ");
    assert_eq!(native["runner"], "native");
    assert_eq!(native["startup_encoding"], "iso-8859-1, surrogateescape");
    for field in ["text", "source_encoding", "file_path", "log", "records"] {
        assert_eq!(native[field], source[field], "{field}");
    }
    assert_eq!(
        native["file_path"]
            .as_array()
            .expect("native file_path")
            .iter()
            .rev()
            .take(7)
            .rev()
            .map(|value| value.as_u64().expect("codepoint"))
            .collect::<Vec<_>>(),
        vec![0x63, 0x61, 0x66, 0x00e9, 0x2e, 0x6d, 0x64],
        "Native file_path retains the same U+00E9 spelling"
    );
    assert!(
        !native["file_path"]
            .as_array()
            .expect("native file_path")
            .iter()
            .any(|value| value.as_u64() == Some(0xdce9)),
        "no hardcoded UTF-8 surrogateescape spelling remains"
    );
}
