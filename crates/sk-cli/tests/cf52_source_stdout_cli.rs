//! CF52 — the real `charter context --include` binary: physical stdout policy.
//!
//! The in-memory rendered value is exact (CF43). This file proves the **physical
//! stdout step** against the pinned Source child: the startup `sys.stdout`
//! encoding and error handler decide the bytes, so an authored `\udc80..\udcff`
//! unit is a raw byte under the default `surrogateescape`, a high `\ud800` still
//! fails there, `PYTHONIOENCODING` really overrides codec *and* handler, and an
//! `ensure_ascii=True` JSON document can still be written as UTF-16.
//!
//! Every case runs BOTH actual children with one isolated environment: the
//! Source child asserts its own version/CWD and every isolation variable, and
//! the same vector is handed to the native binary (native-only smoke is not the
//! proof here — the paired bytes are). Source runs first; its result is asserted
//! against the measured reference contract before the native comparison, so a
//! native divergence can never redefine the expectation.
//!
//! Deliberately out of scope (recorded debt, not claimed parity): Rich's console
//! *layout* for human text (word wrap at `COLUMNS`/80, tab expansion). Every
//! payload below keeps rendered lines short and tab-free so this file measures
//! the encoding/error phase only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

const GLOSSARY: &str = "glossary-pack:spec-kitty-core";
const ENV_FIELDS: [&str; 11] = [
    "HOME",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    "TMPDIR",
    "TMP",
    "TEMP",
];

/// One project fixture; `pack` is the glossary YAML body.
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(pack: &str) -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let root = temp.path().canonicalize().expect("canonical root");
        for directory in [
            "home",
            "cache",
            "config",
            "data",
            "state",
            "runtime",
            "config_dir1",
            "config_dir2",
            "data_dir1",
            "data_dir2",
            "tmp",
        ] {
            fs::create_dir_all(root.join(directory)).expect("isolated directory");
        }
        write(
            &root.join(".kittify/charter/charter.md"),
            b"# Fixture Charter\n",
        );
        write(
            &root.join(".kittify/config.yaml"),
            b"# authored project config\ncustom_setting: keep-me\n",
        );
        write(
            &root.join(".kittify/charter-packs/glossary_packs/spec-kitty-core.glossary-pack.yaml"),
            pack.as_bytes(),
        );
        Self { _temp: temp, root }
    }

    /// The one isolated environment vector every child receives.
    fn env(&self) -> Vec<(String, String)> {
        let root = &self.root;
        let repo = repository_root();
        let mut env = vec![
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("PYTHONHASHSEED".to_owned(), "0".to_owned()),
            ("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()),
            (
                "PYTHONPATH".to_owned(),
                repo.join("src").display().to_string(),
            ),
            (
                "SPEC_KITTY_PACKS_ROOT".to_owned(),
                repo.join("packs").display().to_string(),
            ),
            ("CF52_ROOT".to_owned(), root.display().to_string()),
        ];
        for field in ENV_FIELDS {
            let path = match field {
                "XDG_CONFIG_DIRS" => {
                    format!(
                        "{0}:{1}",
                        root.join("config_dir1").display(),
                        root.join("config_dir2").display()
                    )
                }
                "XDG_DATA_DIRS" => {
                    format!(
                        "{0}:{1}",
                        root.join("data_dir1").display(),
                        root.join("data_dir2").display()
                    )
                }
                "HOME" => root.join("home").display().to_string(),
                "XDG_CACHE_HOME" => root.join("cache").display().to_string(),
                "XDG_CONFIG_HOME" => root.join("config").display().to_string(),
                "XDG_DATA_HOME" => root.join("data").display().to_string(),
                "XDG_STATE_HOME" => root.join("state").display().to_string(),
                "XDG_RUNTIME_DIR" => root.join("runtime").display().to_string(),
                _ => root.join("tmp").display().to_string(),
            };
            env.push((field.to_owned(), path));
        }
        env
    }
}

fn write(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn python() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let root = repository_root();
            let python = std::env::var_os("SK_DOCTRINE_PYTHON")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join(".venv-rust-oracle/bin/python"));
            assert!(python.is_file(), "required Python oracle is missing");
            let result = Command::new(&python)
                .arg(root.join("scripts/rust/verify_oracle.py"))
                .arg("--root")
                .arg(&root)
                .output()
                .expect("execute required Python oracle verifier");
            assert!(
                result.status.success(),
                "Python oracle is not lock-exact: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            python
        })
        .clone()
}

/// The Source child's own isolation assertion, then the real CLI.
const SOURCE_SCRIPT: &str = r#"
import os, sys
from pathlib import Path
assert sys.version.split()[0] == "3.11.15", sys.version
root = Path(os.environ["CF52_ROOT"]).resolve()
assert Path.cwd().resolve() == root, (Path.cwd(), root)
for name in ("HOME", "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
             "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"):
    value = os.environ.get(name)
    assert value, name
    assert Path(value).resolve().is_relative_to(root), (name, value)
for name in ("XDG_CONFIG_DIRS", "XDG_DATA_DIRS"):
    value = os.environ.get(name)
    assert value, name
    for part in value.split(":"):
        assert Path(part).resolve().is_relative_to(root), (name, value)
assert os.environ["PYTHONHASHSEED"] == "0"
assert Path(os.environ["PYTHONPATH"]).resolve().is_relative_to(Path(os.environ["CF52_REPO"]))
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]).is_dir()
from specify_cli import main
main()
"#;

fn base_env(fixture: &Fixture, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env = fixture.env();
    env.push((
        "CF52_REPO".to_owned(),
        repository_root().display().to_string(),
    ));
    for (name, value) in extra {
        env.retain(|(existing, _)| existing != name);
        env.push(((*name).to_owned(), (*value).to_owned()));
    }
    env
}

fn run_source(fixture: &Fixture, args: &[&str], extra: &[(&str, &str)], stdout: Stdio) -> Output {
    let mut command = Command::new(python());
    command
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .args(args)
        .env_clear()
        .envs(base_env(fixture, extra))
        .current_dir(&fixture.root)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped());
    command.output().expect("run source child")
}

fn run_native(fixture: &Fixture, args: &[&str], extra: &[(&str, &str)], stdout: Stdio) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
    command
        .args(args)
        .env_clear()
        .envs(base_env(fixture, extra))
        .current_dir(&fixture.root)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped());
    command.output().expect("run native child")
}

fn stdout_bytes(output: &Output) -> &[u8] {
    &output.stdout
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Assert the Source expectation first, then the paired native comparison.
fn assert_pair(fixture: &Fixture, args: &[&str], extra: &[(&str, &str)], expectation: Expectation) {
    let source = run_source(fixture, args, extra, Stdio::piped());
    assert_source(&source, expectation, args, extra);
    let native = run_native(fixture, args, extra, Stdio::piped());
    assert_native(&source, &native, expectation, args, extra);
}

#[derive(Clone, Copy, Debug)]
enum Expectation {
    /// Both children print identical bytes and exit 0.
    Success,
    /// The late output-encoding phase raises in both; stdout stays empty.
    LateEncodeFailure,
    /// The stored handler name is unknown; it fails only when it is needed.
    UnknownHandler,
    /// A startup codec failure, before any output.
    StartupFatal,
}

fn assert_source(output: &Output, expectation: Expectation, args: &[&str], extra: &[(&str, &str)]) {
    let stderr = stderr_text(output);
    match expectation {
        Expectation::Success => assert_eq!(
            output.status.code(),
            Some(0),
            "source must succeed for {args:?} {extra:?}: {stderr}"
        ),
        Expectation::LateEncodeFailure => {
            assert_eq!(output.status.code(), Some(1), "source exit: {stderr}");
            assert!(
                output.stdout.is_empty(),
                "the reference writes no byte when the encode raises"
            );
            assert!(
                stderr.contains("UnicodeEncodeError"),
                "the reference raises UnicodeEncodeError: {stderr}"
            );
            assert!(
                stderr.contains("PYTHONIOENCODING=utf-8"),
                "Rich appends its own hint to the error: {stderr}"
            );
        }
        Expectation::UnknownHandler => {
            assert_eq!(output.status.code(), Some(1), "source exit: {stderr}");
            assert!(output.stdout.is_empty(), "no byte when the handler raises");
            assert!(
                stderr.contains("unknown error handler name"),
                "the reference raises LookupError for the handler: {stderr}"
            );
        }
        Expectation::StartupFatal => {
            assert_eq!(output.status.code(), Some(1), "source exit: {stderr}");
            assert!(
                output.stdout.is_empty(),
                "a startup codec failure writes nothing"
            );
            assert!(
                stderr.contains("unknown encoding"),
                "the reference fails at stdio startup: {stderr}"
            );
        }
    }
}

fn assert_native(
    source: &Output,
    native: &Output,
    expectation: Expectation,
    args: &[&str],
    extra: &[(&str, &str)],
) {
    let source_stderr = stderr_text(source);
    let native_stderr = stderr_text(native);
    assert_eq!(
        native.status.code(),
        source.status.code(),
        "exit status must match the source for {args:?} {extra:?}\nsource stderr: {source_stderr}\nnative stderr: {native_stderr}"
    );
    match expectation {
        Expectation::Success => assert_eq!(
            stdout_bytes(native),
            stdout_bytes(source),
            "physical stdout bytes must match the source for {args:?} {extra:?}"
        ),
        Expectation::LateEncodeFailure => {
            assert!(
                native.stdout.is_empty(),
                "nothing partial is printed for {args:?} {extra:?}"
            );
            assert!(
                native_stderr.contains("cannot print the rendered text of --include")
                    && native_stderr.contains("--json"),
                "the refusal names the phase and the route: {native_stderr}"
            );
            assert!(
                native_stderr.contains("PYTHONIOENCODING=utf-8"),
                "the refusal keeps the reference's hint: {native_stderr}"
            );
        }
        Expectation::UnknownHandler => {
            assert!(
                native_stderr.contains("unknown error handler name"),
                "{native_stderr}"
            );
        }
        Expectation::StartupFatal => {
            assert!(
                native_stderr.contains("unknown encoding"),
                "{native_stderr}"
            );
        }
    }
}

fn text_args() -> [&'static str; 4] {
    ["charter", "context", "--include", GLOSSARY]
}

fn json_args() -> [&'static str; 5] {
    ["charter", "context", "--include", GLOSSARY, "--json"]
}

/// `\udc80`/`\udcff` (escaped by the default handler) plus ordinary non-ASCII.
/// Short lines and no tabs keep Rich's layout phase out of this proof.
const LOW_PACK: &str = "id: spec-kitty-core\n\
     description: \"low \\udc80 and \\udcff and h\\u00e9llo \\u2603.\"\n\
     terms:\n\
     \x20 - surface: low-term\n\
     \x20   definition: \"a \\udc80 b \\udcff c h\\u00e9llo d \\u2603 e\"\n\
     \x20   confidence: 0.5\n\
     \x20   status: active\n";

/// A high lone surrogate: `surrogateescape` cannot carry it.
const HIGH_PACK: &str = "id: spec-kitty-core\n\
     description: \"high \\ud800 only and h\\u00e9llo.\"\n\
     terms:\n\
     \x20 - surface: high-term\n\
     \x20   definition: \"a \\ud800 b h\\u00e9llo c\"\n\
     \x20   confidence: 0.5\n\
     \x20   status: active\n";

/// All-ASCII payload: no handler is ever needed, so an unknown one is dormant.
const ASCII_PACK: &str = "id: spec-kitty-core\n\
     description: \"plain ascii only.\"\n\
     terms:\n\
     \x20 - surface: ascii-term\n\
     \x20   definition: \"a b c\"\n\
     \x20   confidence: 0.5\n\
     \x20   status: active\n";

const C_LOCALE: &[(&str, &str)] = &[("LC_ALL", "C.UTF-8")];

#[test]
fn default_text_policy_writes_the_reference_bytes() {
    let fixture = Fixture::new(LOW_PACK);
    // `C.UTF-8` with no explicit mode is the measured UTF-8 + surrogateescape
    // policy: the escaped units become raw bytes and non-ASCII stays UTF-8.
    assert_pair(&fixture, &text_args(), C_LOCALE, Expectation::Success);
    let output = run_source(&fixture, &text_args(), C_LOCALE, Stdio::piped());
    let bytes = stdout_bytes(&output);
    assert!(
        bytes.windows(1).any(|window| window == [0x80]),
        "the source writes the raw escaped byte 0x80: {bytes:?}"
    );
    assert!(
        bytes.windows(1).any(|window| window == [0xFF]),
        "the source writes the raw escaped byte 0xFF: {bytes:?}"
    );
    assert!(
        !output.stdout.starts_with(&[0xEF, 0xBB, 0xBF]),
        "no invented BOM on the UTF-8 stream"
    );
}

#[test]
fn explicit_utf8_handlers_match_the_reference_byte_for_byte() {
    let fixture = Fixture::new(LOW_PACK);
    for handler in [
        "strict",
        "surrogateescape",
        "backslashreplace",
        "replace",
        "ignore",
        "xmlcharrefreplace",
    ] {
        let setting = format!("utf-8:{handler}");
        let extra: &[(&str, &str)] = &[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", &setting)];
        let expectation = if handler == "strict" {
            Expectation::LateEncodeFailure
        } else {
            Expectation::Success
        };
        assert_pair(&fixture, &text_args(), extra, expectation);
    }
    // `PYTHONIOENCODING=utf-8` without a handler keeps `strict`.
    assert_pair(
        &fixture,
        &text_args(),
        &[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", "utf-8")],
        Expectation::LateEncodeFailure,
    );
}

#[test]
fn ascii_and_latin_handlers_match_the_reference_byte_for_byte() {
    let fixture = Fixture::new(LOW_PACK);
    for (setting, expectation) in [
        ("ascii:strict", Expectation::LateEncodeFailure),
        ("ascii:replace", Expectation::Success),
        ("ascii:backslashreplace", Expectation::Success),
        ("ascii:surrogateescape", Expectation::LateEncodeFailure),
        ("latin-1:strict", Expectation::LateEncodeFailure),
        ("latin-1:replace", Expectation::Success),
        ("iso8859-15:strict", Expectation::LateEncodeFailure),
        ("iso8859-15:replace", Expectation::Success),
    ] {
        let extra: &[(&str, &str)] = &[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", setting)];
        assert_pair(&fixture, &text_args(), extra, expectation);
    }
}

#[test]
fn a_high_surrogate_fails_under_the_default_policy_and_prints_under_a_replacement_handler() {
    let fixture = Fixture::new(HIGH_PACK);
    assert_pair(
        &fixture,
        &text_args(),
        C_LOCALE,
        Expectation::LateEncodeFailure,
    );
    for handler in ["backslashreplace", "replace", "ignore"] {
        let setting = format!("utf-8:{handler}");
        let extra: &[(&str, &str)] = &[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", &setting)];
        assert_pair(&fixture, &text_args(), extra, Expectation::Success);
    }
    // The same payload is always JSON-safe: `ensure_ascii` spells it as ASCII.
    assert_pair(&fixture, &json_args(), C_LOCALE, Expectation::Success);
}

#[test]
fn unknown_handlers_and_codecs_fail_in_the_reference_phase() {
    let fixture = Fixture::new(LOW_PACK);
    for setting in ["utf-8:no-such-handler", "utf-8:REPLACE", "utf-8: strict"] {
        let extra: &[(&str, &str)] = &[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", setting)];
        assert_pair(&fixture, &text_args(), extra, Expectation::UnknownHandler);
    }
    // A handler that is never needed does not fail: this payload is all-ASCII.
    let ascii_fixture = Fixture::new(ASCII_PACK);
    assert_pair(
        &ascii_fixture,
        &text_args(),
        &[
            ("LC_ALL", "C.UTF-8"),
            ("PYTHONIOENCODING", "ascii:no-such-handler"),
        ],
        Expectation::Success,
    );
    for setting in ["no-such-codec", "no-such-codec:replace"] {
        let extra: &[(&str, &str)] = &[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", setting)];
        assert_pair(&fixture, &text_args(), extra, Expectation::StartupFatal);
    }
}

#[test]
fn json_is_ascii_but_the_physical_stream_codec_still_applies() {
    let fixture = Fixture::new(LOW_PACK);
    // The JSON document is ASCII in every case; the stream bytes are not.
    for setting in [
        "utf-16",
        "utf-16-le",
        "utf-16-be",
        "utf-32",
        "ascii:strict",
        "iso8859-15:strict",
        "latin-1:replace",
    ] {
        let extra: &[(&str, &str)] = &[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", setting)];
        assert_pair(&fixture, &json_args(), extra, Expectation::Success);
    }
    let extra: &[(&str, &str)] = &[("LC_ALL", "C.UTF-8"), ("PYTHONIOENCODING", "utf-16")];
    let output = run_source(&fixture, &json_args(), extra, Stdio::piped());
    let bytes = stdout_bytes(&output);
    assert!(
        bytes.starts_with(&[b'{', 0x00]) || bytes.starts_with(&[0x00, b'{']),
        "the physical UTF-16 stream carries the ASCII document: {bytes:?}"
    );
    assert!(
        !bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
        "no UTF-8 BOM appears on a UTF-16 stream"
    );
}

#[test]
fn real_locales_select_the_reference_policy() {
    // A real UTF-8 locale is `strict`: the escaped unit fails in both children.
    let fixture = Fixture::new(LOW_PACK);
    assert_pair(
        &fixture,
        &text_args(),
        &[("LC_ALL", "en_US.UTF-8")],
        Expectation::LateEncodeFailure,
    );
    // The installed Latin-9 locale really is `iso8859-15`, and the escaped unit
    // cannot be carried by `strict` there either.
    assert_pair(
        &fixture,
        &text_args(),
        &[("LC_ALL", "en_US.iso885915")],
        Expectation::LateEncodeFailure,
    );
    // The same Latin-9 locale encodes `é` as one byte under a replacement handler.
    assert_pair(
        &fixture,
        &text_args(),
        &[
            ("LC_ALL", "en_US.iso885915"),
            ("PYTHONIOENCODING", "iso8859-15:replace"),
        ],
        Expectation::Success,
    );
}

/// A real write failure (ENOSPC from `/dev/full`) must be a failure, never a
/// silent success on either side. `/dev/full` is deterministic on Linux; a
/// *closed* fd 1 is not used here because bash reopens `/dev/null` on it.
#[cfg(target_os = "linux")]
#[test]
fn an_unwritable_stdout_is_a_failure_in_both_children() {
    let fixture = Fixture::new(LOW_PACK);
    for args in [text_args().as_slice(), json_args().as_slice()] {
        let run = |mut command: Command| {
            let full = fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .expect("/dev/full");
            command
                .stdin(Stdio::null())
                .stdout(Stdio::from(full))
                .stderr(Stdio::piped())
                .output()
                .expect("unwritable-stdout child")
        };
        let source = run(source_command(&fixture, args, C_LOCALE));
        assert_eq!(
            source.status.code(),
            Some(120),
            "the reference ends on the failed shutdown flush: {}",
            String::from_utf8_lossy(&source.stderr)
        );
        let native = run(native_command(&fixture, args, C_LOCALE));
        assert_eq!(
            native.status.code(),
            source.status.code(),
            "native must match the failed-flush exit for {args:?}: {}",
            String::from_utf8_lossy(&native.stderr)
        );
    }
}

fn source_command(fixture: &Fixture, args: &[&str], extra: &[(&str, &str)]) -> Command {
    let mut command = Command::new(python());
    command
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .args(args)
        .env_clear()
        .envs(base_env(fixture, extra))
        .current_dir(&fixture.root);
    command
}

fn native_command(fixture: &Fixture, args: &[&str], extra: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
    command
        .args(args)
        .env_clear()
        .envs(base_env(fixture, extra))
        .current_dir(&fixture.root);
    command
}

/// Run one child with its stdout connected to a pipe whose read end is closed,
/// so the first write really fails with `EPIPE` in both children.
#[cfg(target_os = "linux")]
fn run_with_broken_pipe(mut command: Command, root: &Path) -> Output {
    use std::os::unix::fs::OpenOptionsExt;
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let fifo = root.join(format!(
        "stdout-{}-{}.fifo",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_file(&fifo);
    let status = Command::new("mkfifo").arg(&fifo).status().expect("mkfifo");
    assert!(status.success(), "mkfifo failed");
    // O_NONBLOCK = 0o4000 on Linux: the read side opens without waiting for a
    // writer; the child's write side then opens because this reader exists.
    let reader = fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o4000)
        .open(&fifo)
        .expect("non-blocking fifo reader");
    let writer = fs::OpenOptions::new()
        .write(true)
        .open(&fifo)
        .expect("fifo writer");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::piped());
    let child = command.spawn().expect("broken-pipe child");
    drop(reader);
    child.wait_with_output().expect("broken-pipe output")
}

#[cfg(target_os = "linux")]
#[test]
fn a_broken_pipe_keeps_the_reference_exit_class() {
    let fixture = Fixture::new(LOW_PACK);
    // Rich's console exits 1 on the text arm.
    let source = run_with_broken_pipe(
        source_command(&fixture, &text_args(), C_LOCALE),
        &fixture.root,
    );
    assert_eq!(source.status.code(), Some(1), "source text arm");
    let native = run_with_broken_pipe(
        native_command(&fixture, &text_args(), C_LOCALE),
        &fixture.root,
    );
    assert_eq!(
        native.status.code(),
        source.status.code(),
        "native text arm"
    );
    assert!(
        native.stdout.is_empty(),
        "no captured stdout on a broken pipe"
    );

    // Builtin `print()` reports the ignored flush at shutdown: exit 120.
    let source = run_with_broken_pipe(
        source_command(&fixture, &json_args(), C_LOCALE),
        &fixture.root,
    );
    assert_eq!(source.status.code(), Some(120), "source json arm");
    let native = run_with_broken_pipe(
        native_command(&fixture, &json_args(), C_LOCALE),
        &fixture.root,
    );
    assert_eq!(
        native.status.code(),
        source.status.code(),
        "native json arm"
    );
}
