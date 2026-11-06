//! CF70 — the Source reference-pointer pipeline and its live bootstrap render.
//!
//! The reference's `Reference Docs:` block is not a catalogue echo: it is
//! `_filter_references_for_action` → `_distribute_references_across_kinds` →
//! `_reference_source_index` / `_resolve_reference_source` /
//! `_select_reference_pointers` (default cap 12, checked after each append),
//! rendered as `  - {id}: {title} ({path})` with no empty-title special case.
//! `references_count` remains the deny-admitted authored catalogue length.
//!
//! The pair drives the **actual Source CLI handler** and the **same facade the
//! Native CLI calls** as two children, each asserting its own temporary CWD,
//! HOME, all seven XDG roots, TMPDIR/TMP/TEMP, the absolute Source/packs roots,
//! the hash seed, the exact 3.11.15 interpreter and the actual filesystem codec
//! **before** any business read. The Source root is supplied explicitly
//! (`SK_CF70_SOURCE_ROOT`), never derived from `PYTHONPATH.parent`.
//!
//! The Native-only selector controls at the bottom are prospective API checks
//! (they did not exist before this delivery), not extra paired features.

#[path = "cf65_support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};

const CHARTER_BODY: &str = "# CF70 pointer charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n\n## Terminology Canon\n\nUse Mission, never Feature.\n";

/// Real built-in directive files, so the catalogue ids resolve through the
/// `DIRECTIVE_<digits>` filename alias.
const DIRECTIVES: [(&str, &str); 13] = [
    ("001", "architectural-integrity-standard"),
    ("003", "decision-documentation-requirement"),
    ("010", "specification-fidelity-requirement"),
    ("018", "doctrine-versioning-requirement"),
    ("024", "locality-of-change"),
    ("025", "boy-scout-rule"),
    ("028", "search-tool-discipline"),
    ("029", "agent-commit-signing-policy"),
    ("030", "test-and-typecheck-quality-gate"),
    ("031", "context-aware-design"),
    ("032", "conceptual-alignment"),
    ("033", "targeted-staging-policy"),
    ("034", "test-first-development"),
];

const CATALOGUE_ENTRIES: usize = DIRECTIVES.len() + 5;

/// The fixture catalogue: 13 directives (one with an empty title, so the Source
/// shape's double space is compared), a paradigm resolved only by the
/// `Path.stem` fallback (`_LIBRARY/paradigm-<slug>.md`), a tactic whose
/// `.tactic.yaml` local path does NOT match the first-dot index stem, one global
/// and one action-scoped `local_support` entry, and one project-generated kind.
fn catalogue() -> String {
    let mut yaml = String::from("catalog:\n  references:\n");
    for (index, (number, slug)) in DIRECTIVES.iter().enumerate() {
        let title = if index == 0 { "" } else { "Title" };
        yaml.push_str(&format!(
            "    - id: \"directive:DIRECTIVE_{number}\"\n      title: \"{title}\"\n      local_path: \"directives/{number}-{slug}.directive.yaml\"\n      kind: \"directive\"\n      summary: \"directive {number}\"\n"
        ));
    }
    yaml.push_str(
        "    - id: \"paradigm:library/domain-driven-design\"\n      title: \"Domain Driven Design\"\n      local_path: \"_LIBRARY/paradigm-domain-driven-design.md\"\n      kind: \"paradigm\"\n      summary: \"fallback slug\"\n",
    );
    yaml.push_str(
        "    - id: \"tactic:architecture/dependency-hygiene\"\n      title: \"Dependency Hygiene\"\n      local_path: \"tactics/architecture/dependency-hygiene.tactic.yaml\"\n      kind: \"tactic\"\n      summary: \"stem keeps .tactic\"\n",
    );
    yaml.push_str(
        "    - id: \"local_support:global\"\n      title: \"Global\"\n      local_path: \"\"\n      kind: \"local_support\"\n      summary: \"no action scope\"\n",
    );
    yaml.push_str(
        "    - id: \"local_support:review-only\"\n      title: \"Review only\"\n      local_path: \"\"\n      kind: \"local_support\"\n      summary: \"(action: review)\"\n",
    );
    yaml.push_str(
        "    - id: \"user_profile:analyst\"\n      title: \"Analyst\"\n      local_path: \"\"\n      kind: \"user_profile\"\n      summary: \"project-generated\"\n",
    );
    yaml
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("write fixture");
}

fn git_init(root: &Path) {
    let status = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("-b")
        .arg("main")
        .arg(root)
        .status()
        .expect("git init");
    assert!(status.success(), "git init failed for {}", root.display());
}

fn build_case(project: &Path) {
    write(
        &project.join(".kittify/config.yaml"),
        "custom_setting: keep-me\n",
    );
    write(
        &project.join(".kittify/charter/charter.yaml"),
        &format!("schema_version: \"1.1.0\"\n{}", catalogue()),
    );
    write(&project.join(".kittify/charter/charter.md"), CHARTER_BODY);
    git_init(project);
}

/// Assert the isolated Source child environment before any business read.
const SOURCE_ASSERTIONS: &str = r#"
root = pathlib.Path(os.environ["SK_CF70_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
source_root = pathlib.Path(os.environ["SK_CF70_SOURCE_ROOT"])
assert source_root.is_absolute(), source_root
assert source_root == pathlib.Path(os.environ["SK_CF65_SOURCE_ROOT"])
assert pathlib.Path(os.environ["PYTHONPATH"]) == source_root / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == source_root / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
assert os.environ["PYTHONHASHSEED"] == "0", os.environ["PYTHONHASHSEED"]
assert sys.getfilesystemencoding() == "utf-8", sys.getfilesystemencoding()
assert sys.getfilesystemencodeerrors() == "surrogateescape"
import charter.activation.context_renderers.reference_pointers as rp
assert pathlib.Path(rp.__file__).resolve().is_relative_to((source_root / "src").resolve())
"#;

const SOURCE_PROBE: &str = r#"
import contextlib, io, json, os, pathlib, sys

root = pathlib.Path(os.environ["SK_CF70_ROOT"])
ROOT_ASSERTIONS
from specify_cli.cli.commands.charter.context import context as context_command

project = root / "project"
os.chdir(project)
out, err = io.StringIO(), io.StringIO()
code, error, envelope = 0, None, None
try:
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        context_command(
            owned_checkout=None, mission=None, action="implement", include=None,
            mark_loaded=True, profile=None, mission_type="software-dev",
            json_output=True, include_all=False,
        )
except SystemExit as exc:
    code = exc.code if isinstance(exc.code, int) else 1
except BaseException as exc:
    code, error = 1, type(exc).__name__ + ": " + str(exc)
text = out.getvalue()
block = []
if code == 0:
    envelope = json.loads(text)
    body = envelope.get("context") or ""
    lines = body.splitlines()
    if "Reference Docs:" in lines:
        start = lines.index("Reference Docs:")
        end = start + 1
        while end < len(lines) and lines[end].startswith("  - "):
            end += 1
        block = lines[start:end]
receipt = {
    "runner": "source-python-" + ".".join(str(p) for p in sys.version_info[:3]),
    "codec": [sys.getfilesystemencoding(), sys.getfilesystemencodeerrors()],
    "exit": code,
    "error": error,
    "mode": envelope.get("mode") if envelope else None,
    "first_load": envelope.get("first_load") if envelope else None,
    "references_count": envelope.get("references_count") if envelope else None,
    "block": block,
    "stderr": err.getvalue(),
}
print("CF70_SOURCE_RECEIPT " + json.dumps(receipt))
"#;

fn run_source(root: &Path) -> JsonValue {
    let source_root = support::repository_root();
    let script = SOURCE_PROBE.replace("ROOT_ASSERTIONS", SOURCE_ASSERTIONS);
    let output = support::isolated_command(root, support::python())
        .arg("-c")
        .arg(script)
        .env("SK_CF70_ROOT", root)
        .env("SK_CF70_SOURCE_ROOT", &source_root)
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .output()
        .expect("run the pinned Source child");
    std::fs::write(root.join("source.stdout"), &output.stdout).expect("source stdout");
    std::fs::write(root.join("source.stderr"), &output.stderr).expect("source stderr");
    assert!(
        output.status.success(),
        "Source child failed ({:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("Source stdout is UTF-8");
    let line = stdout
        .lines()
        .find(|line| line.starts_with("CF70_SOURCE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Source receipt in {stdout:?}"));
    serde_json::from_str(line["CF70_SOURCE_RECEIPT ".len()..].trim()).expect("receipt JSON")
}

/// The Native child: assert isolation + the captured codec, then call the SAME
/// facade the real CLI command calls.
#[test]
fn cf70_native_pointer_child() {
    let Some(root) = std::env::var_os("SK_CF70_NATIVE_ROOT").map(PathBuf::from) else {
        return; // ordinary run: the parent test drives this child
    };
    support::assert_isolated(&root);
    let codec = sk_kernel::python_os_encoding::OsEncodingContext::from_environment();
    assert_eq!(codec.label(), "utf-8, surrogateescape");
    let project = root.join("project");
    std::env::set_current_dir(&project).expect("child cwd");

    let result = sk_cli::compat::charter::execute_context(
        &project,
        "implement",
        Some("software-dev"),
        None,
        false,
        true,
        true,
    );
    let mut receipt = json!({"runner": "native-child", "codec": codec.label()});
    match result {
        Ok(execution) => {
            let text = execution
                .text
                .to_scalar_string()
                .expect("scalar bootstrap text");
            let lines: Vec<&str> = text.lines().collect();
            let mut block = Vec::new();
            if let Some(start) = lines.iter().position(|line| *line == "Reference Docs:") {
                let mut end = start + 1;
                while end < lines.len() && lines[end].starts_with("  - ") {
                    end += 1;
                }
                block = lines[start..end]
                    .iter()
                    .map(|line| (*line).to_owned())
                    .collect();
            }
            receipt["exit"] = json!(0);
            receipt["mode"] = json!(match execution.mode {
                sk_doctrine::ContextMode::Bootstrap => "bootstrap",
                sk_doctrine::ContextMode::Compact => "compact",
                sk_doctrine::ContextMode::Missing => "missing",
            });
            receipt["first_load"] = json!(execution.first_load);
            receipt["references_count"] = json!(execution.references_count);
            receipt["block"] = json!(block);
        }
        Err(error) => {
            receipt["exit"] = json!(1);
            receipt["error"] = json!(format!("{error:?}"));
            receipt["mode"] = JsonValue::Null;
            receipt["first_load"] = JsonValue::Null;
            receipt["references_count"] = JsonValue::Null;
            receipt["block"] = json!([]);
        }
    }
    std::fs::write(root.join("native.receipt.json"), receipt.to_string()).expect("native receipt");
}

fn run_native_child(root: &Path) -> JsonValue {
    let executable = std::env::current_exe().expect("test binary");
    let output = support::isolated_command(root, executable)
        .arg("cf70_native_pointer_child")
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env("SK_CF70_NATIVE_ROOT", root)
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .output()
        .expect("run the Native child");
    std::fs::write(root.join("native.stdout"), &output.stdout).expect("native stdout");
    std::fs::write(root.join("native.stderr"), &output.stderr).expect("native stderr");
    assert!(
        output.status.success(),
        "Native child failed ({:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = std::fs::read_to_string(root.join("native.receipt.json"))
        .expect("the Native child wrote its receipt");
    serde_json::from_str(&receipt).expect("native receipt JSON")
}

/// The live pointer block, compared exactly in two actual children.
#[test]
fn pointer_block_matches_the_reference_in_two_actual_children() {
    let source_base = support::fixture("cf70-pointers-source");
    let native_base = support::fixture("cf70-pointers-native");
    build_case(&source_base.join("project"));
    build_case(&native_base.join("project"));

    let expected = run_source(&source_base);
    let actual = run_native_child(&native_base);

    assert_eq!(actual["exit"], expected["exit"], "{:?}", expected["error"]);
    assert_eq!(actual["mode"], expected["mode"]);
    assert_eq!(actual["first_load"], expected["first_load"]);
    assert_eq!(actual["references_count"], expected["references_count"]);
    assert_eq!(
        expected["references_count"],
        json!(CATALOGUE_ENTRIES),
        "count is the authored catalogue, not the selected pointers"
    );
    let expected_block = expected["block"].as_array().expect("source block");
    assert_eq!(
        expected_block.len(),
        13,
        "header plus the selector's default 12 pointers: {expected_block:?}"
    );
    assert_eq!(
        actual["block"], expected["block"],
        "Native pointer block differs from Source"
    );

    // Named Source facts inside the live block.
    let block: Vec<&str> = expected_block
        .iter()
        .map(|line| line.as_str().expect("line"))
        .collect();
    assert_eq!(block[0], "Reference Docs:");
    assert!(
        block
            .iter()
            .any(|line| line.starts_with("  - directive:DIRECTIVE_001:  (")),
        "empty-title line keeps Source's double space: {block:?}"
    );
    assert!(
        block.iter().any(|line| line
            .starts_with("  - paradigm:library/domain-driven-design: Domain Driven Design (")
            && line.ends_with("/packs/built-in/paradigms/domain-driven-design.paradigm.yaml)")),
        "the Path.stem fallback resolves the library-style local path: {block:?}"
    );
    assert!(
        !block.iter().any(|line| line.contains("dependency-hygiene")),
        "a `.tactic.yaml` local path does not match the first-dot stem: {block:?}"
    );
}

// ---------------------------------------------------------------------------
// Native-only selector controls (prospective API checks, not paired features)
// ---------------------------------------------------------------------------

fn offering(root: &Path) -> PathBuf {
    let offering = root.join("offering");
    for kind in sk_doctrine::REFERENCE_KINDS {
        std::fs::create_dir_all(offering.join(kind.plural())).expect("kind dir");
    }
    offering
}

fn reference(kind: &str, id: &str, local_path: &str) -> sk_doctrine::ReferenceEntry {
    sk_doctrine::ReferenceEntry {
        id: sk_kernel::PythonString::from_scalars(id),
        title: sk_kernel::PythonString::default(),
        local_path: sk_kernel::PythonString::from_scalars(local_path),
        kind: sk_kernel::PythonString::from_scalars(kind),
        summary: sk_kernel::PythonString::default(),
    }
}

fn selector_codec() -> sk_kernel::python_os_encoding::OsEncodingContext {
    sk_kernel::python_os_encoding::OsEncodingContext::from_lookup(|key| {
        (key == "PYTHONUTF8").then(|| std::ffi::OsString::from("1"))
    })
}

#[test]
fn index_and_selection_follow_the_reference_policies() {
    let temp = support::fixture("cf70-selector");
    let root = offering(&temp);
    let directives = root.join("directives");
    write(&directives.join("001-alpha.directive.yaml"), "x: 1\n");
    write(&directives.join("x.y.z.yaml"), "x: 1\n");
    write(&directives.join("dup.yaml"), "x: 1\n");
    write(&directives.join("dup.yaml.yaml"), "x: 1\n");
    write(&directives.join("a.yaml"), "x: 1\n");
    write(&directives.join("a/x.yaml"), "x: 1\n");
    write(&directives.join("a-b.yaml"), "x: 1\n");
    std::fs::create_dir_all(directives.join("zdir.yaml")).expect("yaml directory");
    std::os::unix::fs::symlink("missing", directives.join("dangling.yaml")).expect("dangling");
    let outside = temp.join("outside");
    write(&outside.join("outside.yaml"), "x: 1\n");
    std::os::unix::fs::symlink(&outside, root.join("tactics/childlink")).expect("child link");
    write(&root.join("tactics/real.tactic.yaml"), "x: 1\n");
    let real_root = temp.join("real-root");
    write(&real_root.join("directives/alias.directive.yaml"), "x: 1\n");
    std::os::unix::fs::symlink(&real_root, root.join("rootlink")).expect("root link");

    let codec = selector_codec();
    let index = sk_doctrine::reference_source_index(&root, &codec).expect("index");
    let keys = |kind: &str| -> Vec<String> {
        let mut keys: Vec<String> = index
            .kind_index(&sk_kernel::PythonString::from_scalars(kind))
            .expect("kind")
            .keys()
            .map(|key| {
                sk_kernel::PythonString::from_codepoints(key)
                    .expect("key")
                    .to_scalar_string()
                    .expect("scalar key")
            })
            .collect();
        keys.sort();
        keys
    };
    // `a/x.yaml` sorts before `a.yaml` (component tuples), so the first-dot stem
    // `x` binds the nested file; `dup.yaml` wins the `dup` setdefault; the yaml
    // directory and the dangling link are candidates; the child symlink is not
    // descended.
    assert_eq!(
        keys("directive"),
        vec![
            "001-alpha",
            "DIRECTIVE_001",
            "a",
            "a-b",
            "dangling",
            "dup",
            "x",
            "zdir",
        ]
    );
    assert_eq!(keys("tactic"), vec!["real"]);
    assert!(
        index
            .kind_index(&sk_kernel::PythonString::from_scalars("nope"))
            .is_none()
    );

    // An explicitly passed symlink root is scanned (the index cache is keyed by
    // the supplied Path, so this is an independent root).
    let linked =
        sk_doctrine::reference_source_index(&root.join("rootlink"), &codec).expect("symlink root");
    assert!(
        linked
            .kind_index(&sk_kernel::PythonString::from_scalars("directive"))
            .expect("kind")
            .contains_key(
                &sk_kernel::PythonString::from_scalars("alias")
                    .codepoints()
                    .to_vec()
            )
    );

    // Cache freeze: the shared repo fixture's later file does not appear.
    write(&directives.join("later.directive.yaml"), "x: 1\n");
    let frozen = sk_doctrine::reference_source_index(&root, &codec).expect("cached");
    assert_eq!(
        frozen
            .kind_index(&sk_kernel::PythonString::from_scalars("directive"))
            .expect("kind")
            .len(),
        8
    );

    // Selection skips only unresolved candidates, caps AFTER appending, and
    // keeps a `limit = 0` result at one resolved pointer.
    let references = vec![
        reference("directive", "directive:missing-1", "directives/none.yaml"),
        reference("directive", "directive:001-alpha", ""),
        reference("tactic", "tactic:real", ""),
        reference("directive", "directive:zdir", ""),
    ];
    let action = sk_kernel::PythonString::from_scalars("implement");
    let selected = sk_doctrine::select_reference_pointers(&references, &action, &root, &codec)
        .expect("select");
    assert_eq!(selected.len(), 3);
    let limited =
        sk_doctrine::select_reference_pointers_with_limit(&references, &action, &root, &codec, 0)
            .expect("limit 0");
    assert_eq!(limited.len(), 1);
    assert_eq!(
        limited[0].entry.id.to_scalar_string().unwrap(),
        "tactic:real"
    );
    assert!(
        limited[0]
            .source_text
            .to_scalar_string()
            .unwrap()
            .ends_with("/tactics/real.tactic.yaml")
    );
}
