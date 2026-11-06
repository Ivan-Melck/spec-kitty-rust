//! CF71 — Source-first paired character-budget and fetch-substitution controls.
//!
//! SOURCE FIRST: the parent runs the pinned oracle Python child against the real
//! `charter.activation.context_renderers.token_budget` / `fetch_stanza` modules,
//! then re-invokes this test as an isolated Native child which evaluates the same
//! observations through `sk_doctrine::text_budget`. Both children assert the
//! temporary CWD/HOME/all-XDG/TMP roots, the independently supplied absolute
//! Source root and packs root, the hash seed and (Source) the exact interpreter
//! BEFORE any business read.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use serde_json::{Value, json};
use sk_doctrine::context_render::{
    FetchScope, fetch_command, fetch_stanza_lines, fetch_stanza_lines_exact,
};
use sk_doctrine::text_budget::{
    BUDGET_DEFAULT, PROFILE_INLINE_BODY_LIMIT_CHARS, RenderedSection, apply_token_budget,
    budget_estimate, budget_without_warning_line, enforce_token_budget, join_sections,
    warning_line,
};
use sk_kernel::PythonString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT_ENV: &str = "CF71_PARITY_ROOT";
const REPO_ENV: &str = "CF71_PARITY_REPO";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

fn text(value: &str) -> PythonString {
    PythonString::from_scalars(value)
}

/// The seven XDG variables, plus the three temporary-directory spellings.
fn env_roots() -> [(&'static str, &'static str); 10] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CONFIG_DIRS", "config_dirs"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_DATA_DIRS", "data_dirs"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
    ]
}

fn child_command(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .env("TEMP", root.join("tmp"));
    for (key, name) in env_roots() {
        command.env(key, root.join(name));
    }
    command
}

/// Assert every isolated root before the child reads any business input.
fn assert_child_environment(root: &Path, repo: &Path) {
    assert!(repo.is_absolute());
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    assert_eq!(
        std::env::current_dir().expect("child cwd"),
        root,
        "child must run in its temporary root"
    );
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_CONFIG_DIRS",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "XDG_DATA_DIRS",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
        "TMP",
        "TEMP",
    ] {
        let expected = match key {
            "HOME" => root.join("home"),
            "XDG_CONFIG_HOME" => root.join("config"),
            "XDG_CONFIG_DIRS" => root.join("config_dirs"),
            "XDG_CACHE_HOME" => root.join("cache"),
            "XDG_DATA_HOME" => root.join("data"),
            "XDG_DATA_DIRS" => root.join("data_dirs"),
            "XDG_STATE_HOME" => root.join("state"),
            "XDG_RUNTIME_DIR" => root.join("runtime"),
            _ => root.join("tmp"),
        };
        assert_eq!(
            PathBuf::from(std::env::var_os(key).unwrap_or_default()),
            expected,
            "{key}"
        );
    }
    assert_eq!(
        PathBuf::from(std::env::var_os("PYTHONPATH").expect("absolute PYTHONPATH")),
        repo.join("src"),
        "Source must be supplied independently, never derived from the parent"
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("SPEC_KITTY_PACKS_ROOT").expect("packs root")),
        repo.join("packs")
    );
    assert_eq!(
        std::env::var_os("PYTHONHASHSEED").as_deref(),
        Some(std::ffi::OsStr::new("0"))
    );
}

fn outcome_json(sections: &[RenderedSection], budget: i128) -> Value {
    let outcome = apply_token_budget(sections, budget, None);
    json!([
        outcome.text.to_scalar_string().expect("scalar fixture"),
        outcome
            .notes
            .iter()
            .map(|note| note.to_scalar_string().expect("scalar note"))
            .collect::<Vec<_>>()
    ])
}

fn native_observations() -> Value {
    let sections = [
        RenderedSection::new("s1", "Header One", &"x".repeat(10)),
        RenderedSection::new("s2", "", &"y".repeat(20)),
    ];
    let noop = [RenderedSection::new("a", "H", &"z".repeat(50))];
    let tie = [
        RenderedSection::new("b", "B:", &"1".repeat(300)).with_selector("tactic:B"),
        RenderedSection::new("a", "A:", &"2".repeat(300)).with_selector("tactic:A"),
    ];
    let tie_budget = i128::try_from(join_sections(&tie).codepoints().len()).unwrap() - 10;
    let exhaust = [
        RenderedSection::new("c", "C:", &"3".repeat(10)).with_selector("tactic:C"),
        RenderedSection::new("a", "A:", &"1".repeat(10)).with_selector("tactic:A"),
        RenderedSection::new("b", "B:", &"2".repeat(10)).with_selector("tactic:B"),
    ];
    let empty_replacement =
        [RenderedSection::new("g", "Grp:", "long body text").with_replacement_body("")];
    let longer =
        [RenderedSection::new("L", "L:", "tiny").with_selector("directive:LONG-SELECTOR-NAME")];

    let critical = text(
        "Action-Critical Charter Sections (implement):\n### Terminology Canon\ncanon body one\n\n### \
         Code Review Checklist\nreview body two\n### Regression Vigilance\nregression body three",
    );
    let critical_text = text(&format!(
        "Bootstrap header\n\n{}\n\nTail",
        critical.to_scalar_string().unwrap()
    ));
    let repeated_profile = text("Profile-Cited Directives (p):\nSHARED-BODY");
    let crlf =
        text("### Terminology Canon\r\nbody one\r\n\r\n### Regression Vigilance\r\nbody two\r\n");
    let nbsp = text("###\u{a0}Terminology Canon\nbody one");
    let selection = text(
        "Selected directives:\n  - DIRECTIVE_001\n  - DIRECTIVE_002\n\nSelected tactics:\n  - \
         tactic-one",
    );

    let owned = text("/tmp/x y/owned");
    let handle = text("mission-1");
    let scope = FetchScope {
        owned_root: &owned,
        mission_handle: &handle,
    };
    let scoped = fetch_stanza_lines_exact(
        &text("directive:DIRECTIVE_001"),
        &text("are about to apply a code change"),
        &PythonString::default(),
        Some(&scope),
    )
    .iter()
    .map(|line| line.to_scalar_string().expect("scalar stanza"))
    .collect::<Vec<_>>();
    let dotted = fetch_stanza_lines_exact(
        &text("directive:X"),
        &text("\u{130} need to x"),
        &PythonString::default(),
        None,
    );
    let when = |clause: &str| {
        fetch_stanza_lines("directive:X", clause, "")
            .into_iter()
            .nth(1)
            .expect("two-line stanza")
    };

    json!({
        "pins": {
            "budget": BUDGET_DEFAULT,
            "profile_limit": PROFILE_INLINE_BODY_LIMIT_CHARS,
            "estimate": budget_estimate(&[text("abc"), text("de"), text("")]),
            "warning": warning_line(2, BUDGET_DEFAULT).to_scalar_string().unwrap(),
            "reserve": budget_without_warning_line(2, BUDGET_DEFAULT).expect("representable remainder"),
        },
        "apply_join": outcome_json(&sections, 1_000_000),
        "apply_nonpositive": outcome_json(&noop, 0),
        "apply_tie": outcome_json(&tie, tie_budget),
        "apply_exhaust": outcome_json(&exhaust, 1),
        "apply_empty_replacement": outcome_json(&empty_replacement, 5),
        "apply_longer": outcome_json(&longer, 6),
        "enforce_identity": critical_text
            == enforce_token_budget(
                &critical_text,
                &text("implement"),
                &PythonString::default(),
                &critical,
                &PythonString::default(),
                1_000_000,
                None,
            ),
        "enforce_nonpositive": enforce_token_budget(
            &critical_text,
            &text("implement"),
            &PythonString::default(),
            &critical,
            &PythonString::default(),
            0,
            None,
        ).to_scalar_string().unwrap(),
        "enforce_no_candidates": enforce_token_budget(
            &text("plain text with no candidates at all"),
            &text("implement"),
            &PythonString::default(),
            &PythonString::default(),
            &PythonString::default(),
            5,
            None,
        ).to_scalar_string().unwrap(),
        "enforce_repeated": enforce_token_budget(
            &text("Head\n\nSHARED-BODY\n\nmid\n\nSHARED-BODY\n\nTail"),
            &text("implement"),
            &repeated_profile,
            &PythonString::default(),
            &PythonString::default(),
            1,
            None,
        ).to_scalar_string().unwrap(),
        "enforce_crlf": enforce_token_budget(
            &text(&format!("H\r\n\r\n{}Tail", crlf.to_scalar_string().unwrap())),
            &text("implement"),
            &PythonString::default(),
            &crlf,
            &PythonString::default(),
            1,
            None,
        ).to_scalar_string().unwrap(),
        "enforce_nbsp": enforce_token_budget(
            &text(&format!("H\n\n{}\n\nTail", nbsp.to_scalar_string().unwrap())),
            &text("implement"),
            &PythonString::default(),
            &nbsp,
            &PythonString::default(),
            1,
            None,
        ).to_scalar_string().unwrap(),
        "enforce_selection": enforce_token_budget(
            &text(&format!("Head\n\n{}\n\nTail", selection.to_scalar_string().unwrap())),
            &text("implement"),
            &PythonString::default(),
            &PythonString::default(),
            &selection,
            5,
            None,
        ).to_scalar_string().unwrap(),
        "fetch": {
            "plain": fetch_stanza_lines("section:terminology-canon", "", "  "),
            "spaced": fetch_command(&text("section:My Section"), None)
                .to_scalar_string().unwrap(),
            "scoped": scoped,
            "capital": when("Are About To ship"),
            "introduces": when("introduces a term"),
            "gerund_short": when("aing"),
            "ing_only": when("ing"),
            "dotted": dotted[1].to_scalar_string().unwrap(),
        }
    })
}

const SOURCE_PROBE: &str = r####"
import json, os, sys
from pathlib import Path

root = Path(os.environ["CF71_PARITY_ROOT"])
repo = Path(os.environ["CF71_PARITY_REPO"])
assert Path.cwd() == root
assert Path(os.environ["HOME"]) == root / "home"
for key, name in [
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CONFIG_DIRS", "config_dirs"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_DATA_DIRS", "data_dirs"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
]:
    assert Path(os.environ[key]) == root / name, key
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
assert os.environ["PYTHONHASHSEED"] == "0"
assert sys.version_info[:3] == (3, 11, 15), sys.version
assert repo.is_absolute(), repo
assert sys.getfilesystemencoding() == "utf-8"
assert sys.getfilesystemencodeerrors() == "surrogateescape"
import charter.activation.context_renderers.token_budget as tb
assert Path(tb.__file__).resolve() == repo / "src/charter/activation/context_renderers/token_budget.py"

from charter.activation.context_renderers.fetch_stanza import (
    fetch_command,
    fetch_stanza_lines,
)
from charter.activation.context_renderers.token_budget import (
    BUDGET_DEFAULT,
    _PROFILE_INLINE_BODY_LIMIT_CHARS,
    RenderedSection,
    _budget_estimate,
    _enforce_token_budget,
    apply_token_budget,
    budget_without_warning_line,
    warning_line,
)

def outcome(sections, budget):
    text, notes = apply_token_budget(sections, budget=budget)
    return [text, notes]

sections = [
    RenderedSection("s1", "Header One", "x" * 10),
    RenderedSection("s2", "", "y" * 20),
]
noop = [RenderedSection("a", "H", "z" * 50)]
tie = [
    RenderedSection("b", "B:", "1" * 300, selector="tactic:B"),
    RenderedSection("a", "A:", "2" * 300, selector="tactic:A"),
]
tie_text = "A:\n" + "2" * 300 + "\n\n" + "B:\n" + "1" * 300
tie_budget = len(tie_text) - 10
exhaust = [
    RenderedSection("c", "C:", "3" * 10, selector="tactic:C"),
    RenderedSection("a", "A:", "1" * 10, selector="tactic:A"),
    RenderedSection("b", "B:", "2" * 10, selector="tactic:B"),
]
empty_replacement = [RenderedSection("g", "Grp:", "long body text", replacement_body="")]
longer = [RenderedSection("L", "L:", "tiny", selector="directive:LONG-SELECTOR-NAME")]

critical = (
    "Action-Critical Charter Sections (implement):\n### Terminology Canon\ncanon body one\n\n### "
    "Code Review Checklist\nreview body two\n### Regression Vigilance\nregression body three"
)
critical_text = "Bootstrap header\n\n" + critical + "\n\nTail"
repeated_profile = "Profile-Cited Directives (p):\nSHARED-BODY"
crlf = "### Terminology Canon\r\nbody one\r\n\r\n### Regression Vigilance\r\nbody two\r\n"
nbsp = "###\u00a0Terminology Canon\nbody one"
selection = (
    "Selected directives:\n  - DIRECTIVE_001\n  - DIRECTIVE_002\n\nSelected tactics:\n  - tactic-one"
)

class Invocation:
    owned_root = Path("/tmp/x y/owned")
    mission_handle = "mission-1"
class Scope:
    invocation = Invocation()

def when(clause):
    return fetch_stanza_lines("directive:X", clause, indent="")[1]

print(json.dumps({
    "pins": {
        "budget": BUDGET_DEFAULT,
        "profile_limit": _PROFILE_INLINE_BODY_LIMIT_CHARS,
        "estimate": _budget_estimate(["abc", "de", ""]),
        "warning": warning_line(2, BUDGET_DEFAULT),
        "reserve": budget_without_warning_line(2, BUDGET_DEFAULT),
    },
    "apply_join": outcome(sections, 1000000),
    "apply_nonpositive": outcome(noop, 0),
    "apply_tie": outcome(tie, tie_budget),
    "apply_exhaust": outcome(exhaust, 1),
    "apply_empty_replacement": outcome(empty_replacement, 5),
    "apply_longer": outcome(longer, 6),
    "enforce_identity": critical_text == _enforce_token_budget(
        critical_text,
        action="implement",
        profile_block="",
        section_block=critical,
        budget=1000000,
    ),
    "enforce_nonpositive": _enforce_token_budget(
        critical_text, action="implement", profile_block="", section_block=critical, budget=0
    ),
    "enforce_no_candidates": _enforce_token_budget(
        "plain text with no candidates at all",
        action="implement",
        profile_block="",
        section_block="",
        budget=5,
    ),
    "enforce_repeated": _enforce_token_budget(
        "Head\n\nSHARED-BODY\n\nmid\n\nSHARED-BODY\n\nTail",
        action="implement",
        profile_block=repeated_profile,
        section_block="",
        budget=1,
    ),
    "enforce_crlf": _enforce_token_budget(
        "H\r\n\r\n" + crlf + "Tail",
        action="implement",
        profile_block="",
        section_block=crlf,
        budget=1,
    ),
    "enforce_nbsp": _enforce_token_budget(
        "H\n\n" + nbsp + "\n\nTail",
        action="implement",
        profile_block="",
        section_block=nbsp,
        budget=1,
    ),
    "enforce_selection": _enforce_token_budget(
        "Head\n\n" + selection + "\n\nTail",
        action="implement",
        profile_block="",
        section_block="",
        selection_block=selection,
        budget=5,
    ),
    "fetch": {
        "plain": fetch_stanza_lines("section:terminology-canon", "", indent="  "),
        "spaced": fetch_command("section:My Section"),
        "scoped": fetch_stanza_lines(
            "directive:DIRECTIVE_001",
            "are about to apply a code change",
            indent="",
            scope=Scope(),
        ),
        "capital": when("Are About To ship"),
        "introduces": when("introduces a term"),
        "gerund_short": when("aing"),
        "ing_only": when("ing"),
        "dotted": fetch_stanza_lines("directive:X", "\u0130 need to x", indent="")[1],
    },
}, ensure_ascii=True))
"####;

#[test]
fn budget_semantics_match_the_source_child() {
    let repo = repo();
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        let root = PathBuf::from(root);
        assert_child_environment(&root, &repo);
        let native = native_observations();
        fs::write(
            root.join("native.json"),
            serde_json::to_vec_pretty(&native).expect("native observations"),
        )
        .expect("write native observations");
        return;
    }

    let root = tempfile::tempdir().expect("fixture").keep();
    for (_, name) in env_roots() {
        fs::create_dir_all(root.join(name)).expect("isolated root");
    }
    fs::create_dir_all(root.join("config_dirs")).expect("config dirs");
    fs::create_dir_all(root.join("data_dirs")).expect("data dirs");
    let probe = root.join("source_probe.py");
    fs::write(&probe, SOURCE_PROBE).expect("write Source probe");

    let source = child_command(&root, &repo, oracle::python())
        .env(ROOT_ENV, &root)
        .env(REPO_ENV, &repo)
        .arg(&probe)
        .output()
        .expect("run the Source child");
    fs::write(root.join("source.stderr"), &source.stderr).expect("retain Source stderr");
    assert!(
        source.status.success(),
        "Source FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    fs::write(root.join("source.json"), &source.stdout).expect("retain Source observations");
    let expected: Value = serde_json::from_slice(&source.stdout).expect("one Source JSON document");

    let native = child_command(&root, &repo, std::env::current_exe().expect("test binary"))
        .env(ROOT_ENV, &root)
        .env(REPO_ENV, &repo)
        .args([
            "--exact",
            "budget_semantics_match_the_source_child",
            "--nocapture",
        ])
        .output()
        .expect("run the Native child");
    fs::write(root.join("native.stderr"), &native.stderr).expect("retain Native stderr");
    assert!(
        native.status.success(),
        "Native child: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
    let actual: Value = serde_json::from_slice(
        &fs::read(root.join("native.json")).expect("native observations file"),
    )
    .expect("one Native JSON document");
    assert_eq!(
        actual, expected,
        "Source and Native budget observations must agree byte-for-byte in value"
    );
}
