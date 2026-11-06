//! CF89 — the remaining Source artifact pre-validation hooks.
//!
//! One bounded plan, run by two genuinely independent children: the pinned
//! Source oracle and this test binary. Each child asserts its own fresh
//! CWD/HOME/seven-XDG/TMP trees, its own absolute expected Source, PYTHONPATH,
//! packs root and hash seed, the real filesystem/stdout/stderr codecs, and the
//! pinned CPython 3.11.15 / exact-53 oracle proof **before** touching business
//! data. Both children receive the same rows (one Rust table, JSON on stdin for
//! the Source child) and write their own fixture files under their own root with
//! identical **relative** paths, so the returned diagnostic paths are compared
//! raw — no root substitution, no output rewriting, no shared directory.
//!
//! The rows cover the five Source hook overrides beyond Directive (Tactic,
//! Paradigm, Styleguide, Toolguide, Procedure), Procedure's nested-step scan and
//! its precedence/non-list/non-dict/`applies_to` behaviour, the eager
//! `str(data.get("id", "?"))` spellings, the overlay phase (hook before
//! `data.get("id")`), a healthy artifact, and Directive itself as the unchanged
//! control. A Native-only cache control sits beside the plan; it is not part of
//! the Source comparison.

mod support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// `<kind token>`, `<plural directory>`, `<filename suffix>`.
const KINDS: [(&str, &str, &str); 6] = [
    ("directive", "directives", "directive"),
    ("tactic", "tactics", "tactic"),
    ("paradigm", "paradigms", "paradigm"),
    ("styleguide", "styleguides", "styleguide"),
    ("toolguide", "toolguides", "toolguide"),
    ("procedure", "procedures", "procedure"),
];

/// The plan shared by both children: `<label>`, `<kind token>`, `<YAML body>`.
///
/// Every row is written to the same relative path for its kind, observed through
/// a freshly constructed repository/service, and then removed, so no row
/// depends on another row's residue.
const ROWS: &[(&str, &str, &str)] = &[
    (
        "directive.toplevel",
        "directive",
        "id: item-1\ntactic_refs: []\n",
    ),
    ("tactic.toplevel", "tactic", "id: item-1\ntactic_refs: []\n"),
    (
        "paradigm.toplevel",
        "paradigm",
        "id: item-1\ntactic_refs: []\n",
    ),
    (
        "styleguide.toplevel",
        "styleguide",
        "id: item-1\ntactic_refs: []\n",
    ),
    (
        "toolguide.toplevel",
        "toolguide",
        "id: item-1\ntactic_refs: []\n",
    ),
    (
        "procedure.toplevel",
        "procedure",
        "id: item-1\ntactic_refs: []\n",
    ),
    (
        "tactic.applies_to",
        "tactic",
        "id: item-1\napplies_to: [x]\n",
    ),
    (
        "tactic.field-order",
        "tactic",
        "id: item-1\napplies_to: [x]\nparadigm_refs: []\ntactic_refs: []\n",
    ),
    ("tactic.no-field", "tactic", "id: item-1\n"),
    (
        "procedure.step",
        "procedure",
        "id: item-1\nsteps:\n  - title: one\n    tactic_refs: [tactic:t]\n",
    ),
    (
        "procedure.step-field-order",
        "procedure",
        "id: item-1\nsteps:\n  - title: one\n    paradigm_refs: [p]\n    tactic_refs: [t]\n",
    ),
    (
        "procedure.step-second",
        "procedure",
        "id: item-1\nsteps:\n  - title: clean\n  - title: dirty\n    paradigm_refs: [p]\n",
    ),
    (
        "procedure.toplevel-precedence",
        "procedure",
        "id: item-1\nparadigm_refs: [p]\nsteps:\n  - title: one\n    tactic_refs: [t]\n",
    ),
    (
        "procedure.steps-non-list",
        "procedure",
        "id: item-1\nsteps: not-a-list\n",
    ),
    ("procedure.steps-null", "procedure", "id: item-1\nsteps:\n"),
    (
        "procedure.steps-false",
        "procedure",
        "id: item-1\nsteps: false\n",
    ),
    (
        "procedure.steps-mapping",
        "procedure",
        "id: item-1\nsteps:\n  tactic_refs: [t]\n",
    ),
    (
        "procedure.step-applies-to",
        "procedure",
        "id: item-1\nsteps:\n  - title: one\n    applies_to: [x]\n",
    ),
    (
        "procedure.step-nondict",
        "procedure",
        "id: item-1\nsteps:\n  - just-a-string\n  - 3\n",
    ),
    ("tactic.missing-id", "tactic", "tactic_refs: []\n"),
    (
        "tactic.structured-id",
        "tactic",
        "id: [a, {b: 1}]\ntactic_refs: []\n",
    ),
    ("tactic.null-id", "tactic", "id:\ntactic_refs: []\n"),
    ("tactic.bool-id", "tactic", "id: true\ntactic_refs: []\n"),
    ("tactic.int-id", "tactic", "id: 7\ntactic_refs: []\n"),
    ("tactic.float-id", "tactic", "id: 1e20\ntactic_refs: []\n"),
    (
        "tactic.date-id",
        "tactic",
        "id: 2020-01-01\ntactic_refs: []\n",
    ),
    (
        "tactic.healthy",
        "tactic",
        "schema_version: '1.0'\nid: healthy-tactic\nname: Healthy\nsteps:\n  - title: one\n",
    ),
    ("overlay.no-id", "tactic", "tactic_refs: []\n"),
    (
        "tactic.root-omap",
        "tactic",
        "!!omap\n- id: item-1\n- tactic_refs: []\n",
    ),
    (
        "procedure.step-omap",
        "procedure",
        "id: item-1\nsteps:\n  - !!omap\n    - title: one\n    - tactic_refs: []\n",
    ),
    (
        "procedure.steps-pairs",
        "procedure",
        "id: item-1\nsteps: !!pairs\n  - tactic_refs: []\n",
    ),
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A private root with the seven XDG trees, the redirected TMP trees and the two
/// layer directories both children write into.
fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (_, name) in ROOTS {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
    }
    for (_, plural, _) in KINDS {
        std::fs::create_dir_all(root.path().join("builtin").join(plural)).unwrap();
        std::fs::create_dir_all(root.path().join("project").join(plural)).unwrap();
    }
    root
}

fn plural(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(token, _, _)| *token == kind)
        .map(|(_, plural, _)| *plural)
        .unwrap()
}

fn suffix(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(token, _, _)| *token == kind)
        .map(|(_, _, suffix)| *suffix)
        .unwrap()
}

/// The one row path, relative to the child's own CWD.
/// The one row path for its label, relative to the child's own CWD.
///
/// Each row gets its own filename because the reference memoises shipped
/// documents on `(path, size, mtime_ns, variant)`: rewriting one path with a
/// same-size body inside one timestamp tick is a different (real) behaviour
/// than parsing fresh inputs, and would hide the hook under test here.
fn row_path(label: &str, kind: &str, project: bool) -> PathBuf {
    Path::new(if project { "project" } else { "builtin" })
        .join(plural(kind))
        .join(format!(
            "row-{}.{}.yaml",
            label.replace('.', "-"),
            suffix(kind)
        ))
}

fn environment(command: &mut Command, root: &Path) {
    let expected = repository_root();
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CF89_ROOT", root)
        .env("CF89_SOURCE", &expected)
        .env("PYTHONPATH", expected.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", expected.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in ROOTS {
        command.env(key, root.join(name));
    }
}

/// Assert this child's own environment before any business read.
fn native_guards(root: &Path) {
    assert!(root.is_absolute());
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key}"
        );
    }
    let expected = repository_root();
    assert_eq!(
        std::env::var_os("CF89_SOURCE").map(PathBuf::from),
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

fn service(project: bool) -> CanonicalOfferingService {
    CanonicalOfferingService::new(OfferingContext::new(
        PathBuf::from("."),
        OfferingRoots {
            built_in: Some(PathBuf::from("builtin")),
            org_roots: vec![],
            project: project.then(|| PathBuf::from("project")),
        },
        None,
    ))
}

fn native_record(kind: &str, project: bool) -> serde_json::Value {
    let kind: ArtifactKind = kind.parse().expect("canonical kind token");
    match service(project).artifact_ids(kind) {
        Ok(ids) => serde_json::json!({
            "class": serde_json::Value::Null,
            "ids": ids
                .iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>(),
        }),
        Err(error) => match error.abort() {
            Some(abort) => serde_json::json!({
                "class": abort.python,
                "message": abort.message,
            }),
            None => serde_json::json!({
                "class": "<not-an-abort>",
                "debug": format!("{error:?}"),
            }),
        },
    }
}

fn native_plan(root: &Path) -> serde_json::Value {
    let mut plan = serde_json::Map::new();
    for (label, kind, body) in ROWS {
        let project = label.starts_with("overlay.");
        let path = row_path(label, kind, project);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        plan.insert((*label).to_owned(), native_record(kind, project));
        std::fs::remove_file(root.join(&path)).unwrap();
    }
    serde_json::Value::Object(plan)
}

/// Native-only cache control: a failed construction is retried, a successful
/// (empty) construction stays warm. Not part of the Source comparison.
fn native_cache(root: &Path) -> serde_json::Value {
    let path = row_path("tactic.cache", "tactic", false);
    std::fs::write(&path, "id: item-1\ntactic_refs: []\n").unwrap();
    let cached = service(false);
    let first = cached
        .artifact_ids(ArtifactKind::Tactic)
        .is_err_and(|error| error.abort().is_some());
    std::fs::write(&path, "id: item-1\n").unwrap();
    let retried = cached
        .artifact_ids(ArtifactKind::Tactic)
        .map(|ids| ids.is_empty())
        .unwrap_or(false);
    std::fs::write(&path, "id: item-1\ntactic_refs: []\n").unwrap();
    let warm = cached
        .artifact_ids(ArtifactKind::Tactic)
        .map(|ids| ids.is_empty())
        .unwrap_or(false);
    std::fs::remove_file(root.join(&path)).unwrap();
    serde_json::json!({"aborted": first, "retried_empty": retried, "warm_empty": warm})
}

const SOURCE: &str = r#"
import json, os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['CF89_ROOT'])
expected = pathlib.Path(os.environ['CF89_SOURCE'])
assert root.is_absolute() and pathlib.Path.cwd() == root
assert expected.is_absolute() and expected.is_dir()
for key, name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','configdirs'),('XDG_DATA_DIRS','datadirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert pathlib.Path(os.environ[key]) == root / name, key
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
from charter.offering.paradigms.repository import ParadigmRepository
from charter.offering.procedures.repository import ProcedureRepository
from charter.offering.styleguides.repository import StyleguideRepository
from charter.offering.tactics.repository import TacticRepository
from charter.offering.toolguides.repository import ToolguideRepository

REPOS = {
    'directive': (DirectiveRepository, 'directives', 'directive'),
    'tactic': (TacticRepository, 'tactics', 'tactic'),
    'paradigm': (ParadigmRepository, 'paradigms', 'paradigm'),
    'styleguide': (StyleguideRepository, 'styleguides', 'styleguide'),
    'toolguide': (ToolguideRepository, 'toolguides', 'toolguide'),
    'procedure': (ProcedureRepository, 'procedures', 'procedure'),
}

rows = json.load(sys.stdin)
plan = {}
for label, kind, body, relative in rows:
    project = label.startswith('overlay.')
    cls, plural, suffix = REPOS[kind]
    path = pathlib.Path(relative)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding='utf-8')
    kwargs = {'built_in_dir': pathlib.Path('builtin') / plural}
    if project:
        kwargs['project_dir'] = pathlib.Path('project')
    try:
        repository = cls(**kwargs)
    except Exception as exc:
        plan[label] = {'class': type(exc).__name__, 'message': str(exc)}
    else:
        plan[label] = {'class': None, 'ids': sorted(item.id for item in repository.list_all())}
    path.unlink()
print('CF89_PLAN ' + json.dumps(plan, sort_keys=True))
"#;

fn receipt(bytes: &[u8], prefix: &str) -> serde_json::Value {
    let text = std::str::from_utf8(bytes).unwrap();
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("missing {prefix} in:\n{text}"));
    serde_json::from_str(line).unwrap()
}

#[test]
fn shared_prevalidation_hooks_match_the_pinned_source() {
    if let Some(value) = std::env::var_os("CF89_NATIVE") {
        let root = PathBuf::from(value);
        native_guards(&root);
        let plan = native_plan(&root);
        let cache = native_cache(&root);
        println!("CF89_PLAN {}", serde_json::to_string(&plan).unwrap());
        println!("CF89_CACHE {}", serde_json::to_string(&cache).unwrap());
        return;
    }

    let rows: Vec<[String; 4]> = ROWS
        .iter()
        .map(|(label, kind, body)| {
            let path = row_path(label, kind, label.starts_with("overlay."));
            [
                (*label).to_owned(),
                (*kind).to_owned(),
                (*body).to_owned(),
                path.to_str().unwrap().to_owned(),
            ]
        })
        .collect();
    let payload = serde_json::to_vec(&rows).unwrap();

    let source_root = fixture();
    let mut source = Command::new(support::python());
    environment(&mut source, source_root.path());
    let mut child = source
        .arg("-c")
        .arg(SOURCE)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(&payload).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Source: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reference = receipt(&output.stdout, "CF89_PLAN ");
    for label in ["tactic.root-omap", "procedure.step-omap"] {
        assert_eq!(
            reference[label]["class"], "InlineReferenceRejectedError",
            "Source ordered mapping reaches the hook: {label}"
        );
    }
    assert!(
        reference["procedure.steps-pairs"]["class"].is_null(),
        "Source pairs entries are tuples, not dict steps"
    );

    let native_root = fixture();
    assert_ne!(source_root.path(), native_root.path());
    let mut native = Command::new(std::env::current_exe().unwrap());
    environment(&mut native, native_root.path());
    let output = native
        .env("CF89_NATIVE", native_root.path())
        .args([
            "--exact",
            "shared_prevalidation_hooks_match_the_pinned_source",
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
    let observed = receipt(&output.stdout, "CF89_PLAN ");

    assert_eq!(
        observed, reference,
        "identical relative business arguments, raw messages, no rewriting"
    );
    let cache = receipt(&output.stdout, "CF89_CACHE ");
    assert_eq!(cache["aborted"], true);
    assert_eq!(cache["retried_empty"], true);
    assert_eq!(cache["warm_empty"], true);
}
