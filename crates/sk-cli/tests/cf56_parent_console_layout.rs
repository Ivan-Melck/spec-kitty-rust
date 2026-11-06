//! Real include consumption of Source console layout, separate from JSON encoding.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;
use sk_kernel::{
    PythonString,
    python_console::{cell_len, layout_console_text},
};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(".kittify/config.yaml", "# fixture\n");
    write(
        ".kittify/charter-packs/styleguide/parent-console.styleguide.yaml",
        "id: parent-console\nschema_version: '1.0'\ntitle: \"Parent\\tconsole\"\nscope: code\nprinciples:\n  - alpha beta gamma delta epsilon\n  - \"control\\u0000X\\u001b[31mY\\u001b[0mZ\"\n",
    );
    assert!(
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    root
}

fn run(root: &Path, columns: &str, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spec-kitty"));
    command
        .current_dir(root)
        .env("SPECIFY_REPO_ROOT", root)
        .env("COLUMNS", columns)
        .env("PYTHONIOENCODING", "utf-8:strict")
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .env_remove("FORCE_COLOR")
        .args([
            "charter",
            "context",
            "--include",
            "styleguide:parent-console",
        ]);
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}

#[test]
fn real_include_wraps_at_the_source_console_width() {
    let root = fixture();
    let output = run(root.path(), "10", false);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.is_empty());
    assert!(text.lines().all(|line| cell_len(line) <= 10), "{text}");
}

#[test]
fn real_include_expands_tabs_and_sanitizes_cli_terminal_controls() {
    let root = fixture();
    let output = run(root.path(), "80", false);
    assert!(output.status.success());
    assert!(
        !output.stdout.iter().any(|byte| [0, 9, 27].contains(byte)),
        "raw control escaped the CLI console policy"
    );
}

#[test]
fn zero_console_width_suppresses_text_but_not_machine_json() {
    let root = fixture();
    let output = run(root.path(), "0", false);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let machine = run(root.path(), "0", true);
    assert!(machine.status.success());
    let value: serde_json::Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert!(
        value["text"].as_str().unwrap().contains('\0'),
        "machine JSON must bypass sanitization"
    );
}

#[test]
fn unicode_decimal_columns_uses_the_same_source_width() {
    let root = fixture();
    let ascii = run(root.path(), "10", false);
    let unicode = run(root.path(), "\u{0661}\u{0660}", false);
    assert!(ascii.status.success() && unicode.status.success());
    assert_eq!(unicode.stdout, ascii.stdout);
}

#[test]
fn huge_width_does_not_turn_into_a_negative_wrap_limit() {
    let text = PythonString::from_scalars("one two three");
    assert_eq!(layout_console_text(&text, usize::MAX), text);
}

const ROOT_VARS: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
    "TMPDIR",
    "TMP",
    "TEMP",
];

fn pair_command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = Command::new(program);
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("SK_CF56_PAIR_ROOT", root)
        .env("SK_CF56_REPO", &repo)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONIOENCODING", "utf-8:strict");
    for key in ROOT_VARS {
        command.env(key, root.join(key));
    }
    command
}

const SOURCE_PAIR: &str = r#"
import os,pathlib,sys
root=pathlib.Path(os.environ["SK_CF56_PAIR_ROOT"])
repo=pathlib.Path(os.environ["SK_CF56_REPO"])
assert pathlib.Path.cwd()==root
assert repo.is_absolute()
for key in ("HOME","XDG_CONFIG_HOME","XDG_DATA_HOME","XDG_CACHE_HOME","XDG_STATE_HOME","XDG_RUNTIME_DIR","XDG_CONFIG_DIRS","XDG_DATA_DIRS","TMPDIR","TMP","TEMP"): assert pathlib.Path(os.environ[key])==root/key
assert os.environ["PYTHONPATH"]==str(repo/"src")
assert os.environ["SPEC_KITTY_PACKS_ROOT"]==str(repo/"packs")
assert os.environ["PYTHONHASHSEED"]=="0"
assert sys.version_info[:3]==(3,11,15)
import io,json
from specify_cli.cli.console import CliConsole
rows=[]
for width,points in json.loads((root/"cases.json").read_text()):
 out=io.StringIO()
 console=CliConsole(width=width,file=out,force_terminal=False,color_system=None)
 console.print("".join(map(chr,points)),markup=False)
 rows.append(list(map(ord,out.getvalue())))
print("CF56_PAIR_RECEIPT "+json.dumps(rows))
"#;

#[test]
fn cf56_actual_native_pair_child() {
    let Some(root) = std::env::var_os("SK_CF56_PAIR_ROOT").map(std::path::PathBuf::from) else {
        return;
    };
    assert_eq!(std::env::current_dir().unwrap(), root);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(repo.is_absolute());
    for key in ROOT_VARS {
        assert_eq!(
            std::env::var_os(key).map(std::path::PathBuf::from),
            Some(root.join(key))
        );
    }
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(std::path::PathBuf::from),
        Some(repo.join("src"))
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(std::path::PathBuf::from),
        Some(repo.join("packs"))
    );
    assert_eq!(std::env::var("PYTHONHASHSEED").as_deref(), Ok("0"));
    let cases: Vec<(usize, Vec<u32>)> =
        serde_json::from_slice(&fs::read(root.join("cases.json")).unwrap()).unwrap();
    let rows: Vec<Vec<u32>> = cases
        .into_iter()
        .map(|(width, points)| {
            match sk_cli::compat::console_text::prepare(
                &PythonString::from_codepoints(&points).unwrap(),
                width,
            ) {
                Some(mut text) => {
                    text.push_scalars("\n");
                    text.codepoints().to_vec()
                }
                None => Vec::new(),
            }
        })
        .collect();
    println!(
        "CF56_PAIR_RECEIPT {}",
        serde_json::to_string(&rows).unwrap()
    );
}

#[test]
fn both_real_children_match_the_source_cli_console_policy() {
    let root = tempfile::tempdir().unwrap();
    for key in ROOT_VARS {
        fs::create_dir(root.path().join(key)).unwrap();
    }
    let inputs = [
        "one two three four five",
        "日本語\tx",
        "e\u{301}e\u{301} x",
        "👨\u{200d}👩 family",
        "a\0b\u{1}c\u{7f}d\u{85}e",
        "a\u{1b}[31mred\u{1b}[0mz",
        "a\u{1b}]title\u{7}z",
        "a\u{9b}\0b",
        "a\n\nb\n",
        "alpha\u{a0}beta gamma",
    ];
    let mut cases: Vec<(usize, Vec<u32>)> = [80, 10, 5, 1, 0, usize::MAX]
        .into_iter()
        .flat_map(|width| {
            inputs
                .iter()
                .map(move |text| (width, text.chars().map(u32::from).collect()))
        })
        .collect();
    for width in [80, 1] {
        cases.push((width, vec![97, 0xdc80, 98, 0xd800, 99]));
    }
    fs::write(
        root.path().join("cases.json"),
        serde_json::to_vec(&cases).unwrap(),
    )
    .unwrap();
    let source = pair_command(root.path(), oracle::python())
        .args(["-c", SOURCE_PAIR])
        .output()
        .unwrap();
    assert!(
        source.status.success(),
        "{}",
        String::from_utf8_lossy(&source.stderr)
    );
    let parse = |output: &[u8]| -> Vec<Vec<u32>> {
        let text = std::str::from_utf8(output).unwrap();
        let receipt = text
            .lines()
            .find_map(|line| line.strip_prefix("CF56_PAIR_RECEIPT "))
            .expect("actual child receipt");
        serde_json::from_str(receipt).unwrap()
    };
    let expected = parse(&source.stdout);
    assert_eq!(expected.len(), cases.len());
    let native = pair_command(root.path(), std::env::current_exe().unwrap())
        .args(["--exact", "cf56_actual_native_pair_child", "--nocapture"])
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    let actual = parse(&native.stdout);
    for (index, (got, wanted)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(got, wanted, "case {index}, {:?}", cases[index]);
    }
    assert_eq!(actual.len(), expected.len());
}
