//! Authorized uppercase data-domain proof, not a workflow/performance claim.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
const ROOT_ENV: &str = "CF76_PARENT_DOMAIN_ROOT";
const REPO_ENV: &str = "CF76_PARENT_DOMAIN_REPO";
const ENV: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_CONFIG_HOME", "config"),
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
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
fn assert_environment(root: &Path) {
    assert!(root.is_absolute());
    assert_eq!(std::env::current_dir().unwrap(), root);
    assert_eq!(PathBuf::from(std::env::var_os(REPO_ENV).unwrap()), repo());
    for (key, name) in ENV {
        assert_eq!(
            PathBuf::from(std::env::var_os(key).unwrap()),
            root.join(name),
            "{key}"
        );
    }
    assert_eq!(
        PathBuf::from(std::env::var_os("PYTHONPATH").unwrap()),
        repo().join("src")
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap()),
        repo().join("packs")
    );
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
}
fn command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    fs::create_dir_all(root).unwrap();
    for (_, name) in ENV {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let mut c = Command::new(program);
    c.env_clear()
        .current_dir(root)
        .env(ROOT_ENV, root)
        .env(REPO_ENV, repo())
        .env("PYTHONPATH", repo().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo().join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("PYTHONCOERCECLOCALE", "0");
    for (key, name) in ENV {
        c.env(key, root.join(name));
    }
    c
}
fn digest(output: Output) -> String {
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("UPPER_DOMAIN_SHA="))
        .expect("actual child digest")
        .to_owned()
}
#[test]
fn native_full_upper_domain_child() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    assert_environment(&root);
    let mut bytes = Vec::new();
    for cp in 0u32..0x110000 {
        let upper = sk_kernel::python_text::upper_codepoints(&[cp]);
        bytes.extend_from_slice(&cp.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(upper.len()).unwrap().to_le_bytes());
        for value in upper {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    println!(
        "UPPER_DOMAIN_SHA={}",
        sk_kernel::content_digest::sha256_hex(&bytes)
    );
}
const SOURCE: &str = r#"
import sys, os, pathlib, hashlib, unicodedata
root=pathlib.Path(os.environ['CF76_PARENT_DOMAIN_ROOT'])
repo=pathlib.Path(os.environ['CF76_PARENT_DOMAIN_REPO'])
assert root.is_absolute() and repo.is_absolute()
assert pathlib.Path.cwd()==root
for key,name in [('HOME','home'),('XDG_CACHE_HOME','cache'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','config-dirs'),('XDG_DATA_DIRS','data-dirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
 assert pathlib.Path(os.environ[key])==root/name,(key,os.environ[key])
assert pathlib.Path(os.environ['PYTHONPATH'])==repo/'src'
assert pathlib.Path(os.environ['SPEC_KITTY_PACKS_ROOT'])==repo/'packs'
assert os.environ['PYTHONHASHSEED']=='0'
assert sys.version_info[:3]==(3,11,15)
assert unicodedata.unidata_version=='14.0.0'
assert pathlib.Path(sys.prefix)==repo/'.venv-rust-oracle'
assert sys.getfilesystemencoding()=='utf-8' and sys.getfilesystemencodeerrors()=='surrogateescape'
assert str.upper.__objclass__ is str
h=hashlib.sha256()
for cp in range(0x110000):
 upper=chr(cp).upper()
 h.update(cp.to_bytes(4,'little'));h.update(len(upper).to_bytes(4,'little'))
 for value in upper:h.update(ord(value).to_bytes(4,'little'))
print('UPPER_DOMAIN_SHA='+h.hexdigest())
"#;
#[test]
fn full_upper_domain_matches_actual_pinned_interpreter() {
    let workspace = tempfile::tempdir().unwrap();
    let python = repo().join(".venv-rust-oracle/bin/python");
    assert!(python.is_file());
    let mut source = command(&workspace.path().join("source"), python);
    source.args(["-c", SOURCE]);
    let expected = digest(source.output().unwrap());
    let mut native = command(
        &workspace.path().join("native"),
        std::env::current_exe().unwrap(),
    );
    native.args(["--exact", "native_full_upper_domain_child", "--nocapture"]);
    let actual = digest(native.output().unwrap());
    assert_eq!(actual, expected);
    println!("actual Source/Native uppercase all1114112 length-framed codepoint digest={actual}");
}
