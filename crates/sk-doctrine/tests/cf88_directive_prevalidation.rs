//! CF88: the Source Directive repository `_pre_validate` abort phase.
//!
//! PARENT QUARANTINE: these producer controls reuse a fixture, rewrite output
//! roots, and lack required expected-Source/hashseed/codec guards. They are
//! preserved as historical unqualified controls, ignored by default, NOT parity
//! evidence. Use `cf88_parent_diagnostic_phase` for the current independent,
//! guarded relative-path comparison with untouched output messages.
//!
//! `BaseArtifactRepository` calls `self._pre_validate(data, yaml_file)` after the
//! non-mapping guard and **before** `model_validate`, and its per-file
//! `except (YAMLError, ValidationError, OSError)` does not catch what the hook
//! raises. `DirectiveRepository._pre_validate` calls
//! `reject_directive_inline_refs`, so a directive document that still carries a
//! retired inline-reference field raises `InlineReferenceRejectedError` (a
//! `ValueError`) and aborts the whole repository construction — in every layer,
//! and before an overlay's id is even read. A Native load that turned that into
//! the ordinary schema-skip would silently answer "no such directive".
//!
//! This suite pins the measured Source behaviour and the Native phase:
//!
//! * **abort text** — the four-line Source message (path, `artifact_kind`,
//!   `forbidden_field`, `migration`) per forbidden field, in Source's tuple order
//!   (`tactic_refs`, `paradigm_refs`, `applies_to`), with the hook running before
//!   the overlay id check (`directive:?` when the id is absent) and with Python
//!   `str()` of a structured id value;
//! * **kept skips** — a non-mapping root, a schema-invalid document and a YAML
//!   syntax error still warn-and-continue, so the abort is not a blanket
//!   fatalisation of malformed input;
//! * **the consumers** — the real `CanonicalOfferingService` and the include
//!   renderer surface the abort instead of a miss, and a repaired fixture loads
//!   again (same-instance retry, warm snapshot, fresh-service fix).
//!
//! Both legs run as **actual re-executed children**: the Source leg is the
//! pinned, lock-exact CPython oracle; the Native leg is this binary re-executed.
//! Each child asserts its CWD, HOME, all seven XDG roots (including
//! `XDG_CONFIG_DIRS` and `XDG_DATA_DIRS`), `TMPDIR`/`TMP`/`TEMP`, the absolute
//! repository `src` on `PYTHONPATH` and the pinned packs root **before**
//! computing. Source runs first and its intended cause is asserted before the
//! Native leg runs.
//!
//! The two mutating tests give each leg its **own** independent fixture (a
//! repair/retry test cannot share one mutated tree), and each leg's receipt is
//! compared with its own fixture root normalized to `<ROOT>` — the business
//! outputs (phase, class, message text, id sets, rendered headings) are compared
//! exactly.
//!
//! Deliberately deferred (reported as debt, not approximated): the reference's
//! skip-warning text, the other six artifact kinds' own `_pre_validate` hooks,
//! and the profile/skill repositories' separate inline-reference policies.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_charter::ArtifactKind;
use sk_doctrine::context_include::{
    IncludeAccess, IncludeError, IncludeInputs, parse_include_selector, render_include,
};
use sk_doctrine::{CanonicalOfferingService, OfferingContext, OfferingRoots};

/// The two XDG roots the shared protocol does not set; this suite asserts them.
const EXTRA_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "configdirs"),
    ("XDG_DATA_DIRS", "datadirs"),
];

const FORBIDDEN: [&str; 3] = ["tactic_refs", "paradigm_refs", "applies_to"];

fn valid(id: &str, title: &str) -> String {
    format!(
        "schema_version: \"1.0\"\nid: {id}\ntitle: {title}\nintent: intent.\nenforcement: required\n"
    )
}

fn with_inline(id: &str, field: &str) -> String {
    format!("{}{field}: [x]\n", valid(id, "Inline"))
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

/// The environment and isolation assertions every child shares.
fn child(root: &Path, program: impl AsRef<OsStr>) -> Command {
    let repository = isolated::repository_root();
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("CF88_ROOT", root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in isolated::ROOTS.iter().chain(EXTRA_ROOTS.iter()) {
        command.env(key, root.join(name));
    }
    command
}

fn assert_child_roots(root: &Path) {
    let cwd = std::env::current_dir().expect("cwd");
    assert_eq!(cwd, root, "child CWD is the fixture");
    for (key, name) in isolated::ROOTS.iter().chain(EXTRA_ROOTS.iter()) {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    let repository = isolated::repository_root();
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(repository.join("src")),
        "PYTHONPATH is the absolute repository src"
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repository.join("packs")),
        "the pack root is the pinned repository packs"
    );
    let _ = support::python();
}

/// A fresh fixture with the built-in, org and project layer directories.
fn fixture(case: &str) -> PathBuf {
    let root = isolated::fixture(case);
    for (_, name) in EXTRA_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra isolated root");
    }
    root
}

/// The three layer directories one directive fixture case reads.
struct Case {
    name: String,
    base: PathBuf,
}

impl Case {
    fn new(root: &Path, name: &str) -> Self {
        let base = root.join(name);
        for leaf in [
            "builtin/directives",
            "org/directives",
            ".kittify/charter-packs/directive",
        ] {
            std::fs::create_dir_all(base.join(leaf)).expect("case layer directory");
        }
        Self {
            name: name.to_owned(),
            base,
        }
    }

    fn builtin(&self) -> PathBuf {
        self.base.join("builtin")
    }

    fn builtin_dir(&self) -> PathBuf {
        self.base.join("builtin/directives")
    }

    fn org_root(&self) -> PathBuf {
        self.base.join("org")
    }

    fn org_dir(&self) -> PathBuf {
        self.base.join("org/directives")
    }

    fn project_root(&self) -> PathBuf {
        self.base.join(".kittify/charter-packs")
    }

    fn project_dir(&self) -> PathBuf {
        self.base.join(".kittify/charter-packs/directive")
    }

    fn to_json(cases: &[Case]) -> String {
        let entries: Vec<serde_json::Value> = cases
            .iter()
            .map(|case| {
                serde_json::json!({
                    "name": case.name,
                    "base": case.base.display().to_string(),
                })
            })
            .collect();
        serde_json::to_string(&entries).expect("cases JSON")
    }

    fn from_env() -> Vec<Case> {
        let raw = std::env::var("CF88_CASES").expect("CF88_CASES");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("cases JSON");
        value
            .as_array()
            .expect("cases array")
            .iter()
            .map(|entry| Case {
                name: entry["name"].as_str().expect("name").to_owned(),
                base: PathBuf::from(entry["base"].as_str().expect("base")),
            })
            .collect()
    }
}

/// The twelve directive cases: eight aborts and four kept-skip/healthy controls.
fn write_cases(root: &Path) -> Vec<Case> {
    let mut cases = Vec::new();
    for field in FORBIDDEN {
        let case = Case::new(root, &format!("builtin-{field}"));
        write(
            &case.builtin_dir().join("001-ok.directive.yaml"),
            &valid("DIRECTIVE_001", "Ok"),
        );
        write(
            &case.builtin_dir().join("002-inline.directive.yaml"),
            &with_inline("DIRECTIVE_002", field),
        );
        cases.push(case);
    }

    // Two forbidden fields present, `applies_to` authored first: Source's tuple
    // order (`tactic_refs` first) decides which field the abort names.
    let case = Case::new(root, "builtin-two-fields");
    write(
        &case.builtin_dir().join("001-ok.directive.yaml"),
        &valid("DIRECTIVE_001", "Ok"),
    );
    write(
        &case.builtin_dir().join("002-inline.directive.yaml"),
        &format!(
            "{}applies_to: [x]\ntactic_refs: [y]\n",
            valid("DIRECTIVE_002", "Inline")
        ),
    );
    cases.push(case);

    // An overlay with an inline reference and no id: the hook runs before the
    // overlay's id check, so this aborts instead of the "no id" skip.
    let case = Case::new(root, "org-inline-no-id");
    write(
        &case.org_dir().join("900-inline.directive.yaml"),
        "tactic_refs: [x]\n",
    );
    cases.push(case);

    // An overlay that shadows a built-in id aborts before the field merge, and the
    // project layer aborts too.
    let case = Case::new(root, "org-inline-shadow");
    write(
        &case.org_dir().join("900-inline.directive.yaml"),
        "id: DIRECTIVE_001\ntitle: Shadow\ntactic_refs: [x]\n",
    );
    cases.push(case);

    let case = Case::new(root, "project-inline");
    write(
        &case.project_dir().join("900-inline.directive.yaml"),
        "id: DIRECTIVE_001\ntitle: P\ntactic_refs: [x]\n",
    );
    cases.push(case);

    // The message carries Python `str()` of the raw id value.
    let case = Case::new(root, "builtin-structured-id");
    write(
        &case.builtin_dir().join("002-inline.directive.yaml"),
        "id: {nested: [1, 2]}\ntitle: T\ntactic_refs: [x]\n",
    );
    cases.push(case);

    // Controls: the non-mapping guard, model admission and the YAML reader keep
    // their per-file skip, so the load still succeeds around them.
    let case = Case::new(root, "nonmapping");
    write(
        &case.builtin_dir().join("001-ok.directive.yaml"),
        &valid("DIRECTIVE_001", "Ok"),
    );
    write(
        &case.builtin_dir().join("002-list.directive.yaml"),
        "- just\n- a list\n",
    );
    cases.push(case);

    let case = Case::new(root, "schema-invalid");
    write(
        &case.builtin_dir().join("001-ok.directive.yaml"),
        &valid("DIRECTIVE_001", "Ok"),
    );
    write(
        &case.builtin_dir().join("002-missing.directive.yaml"),
        "schema_version: \"1.0\"\nid: DIRECTIVE_002\ntitle: T\nintent: I.\n",
    );
    cases.push(case);

    let case = Case::new(root, "syntax-error");
    write(
        &case.builtin_dir().join("001-ok.directive.yaml"),
        &valid("DIRECTIVE_001", "Ok"),
    );
    write(
        &case.builtin_dir().join("002-broken.directive.yaml"),
        "id: [unclosed\n",
    );
    cases.push(case);

    let case = Case::new(root, "healthy");
    write(
        &case.builtin_dir().join("001-ok.directive.yaml"),
        &valid("DIRECTIVE_001", "Ok"),
    );
    write(
        &case.org_dir().join("900-org.directive.yaml"),
        "id: DIRECTIVE_001\ntitle: Org Title\n",
    );
    write(
        &case.project_dir().join("901-new.directive.yaml"),
        &valid("DIRECTIVE_003", "Project New"),
    );
    cases.push(case);

    cases
}

/// The Source probe's shared isolation prologue.
const ISOLATION: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["CF88_ROOT"])
assert pathlib.Path.cwd() == root, (str(pathlib.Path.cwd()), str(root))
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["PYTHONPATH"]).parent
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CONFIG_DIRS", "configdirs"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_DATA_DIRS", "datadirs"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
"#;

fn source_script(body: &str) -> String {
    format!("{ISOLATION}{body}")
}

const CASES_BODY: &str = r#"
from charter.offering.directives.repository import DirectiveRepository
cases = {}
for case in json.loads(os.environ["CF88_CASES"]):
    base = pathlib.Path(case["base"])
    try:
        repository = DirectiveRepository(
            built_in_dir=base / "builtin" / "directives",
            org_dirs=[base / "org" / "directives"],
            project_dir=base / ".kittify" / "charter-packs" / "directive",
        )
    except Exception as exc:  # noqa: BLE001 - the reference's own escape is the answer
        cases[case["name"]] = {"phase": "abort", "class": type(exc).__name__, "message": str(exc)}
    else:
        cases[case["name"]] = {"phase": "loaded", "keys": sorted(item.id for item in repository.list_all())}
print("CF88_SOURCE_RECEIPT " + json.dumps({
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "cases": cases,
}, sort_keys=True))
"#;

const CONSUMER_BODY: &str = r#"
from charter.activation.active_charter_service_builder import _build_offering_service
from charter.activation.context import build_charter_context_include
root_path = pathlib.Path(os.environ["CF88_ROOT"])
org = root_path / "packs" / "acme-org"
selectors = json.loads(os.environ["CF88_SELECTORS"])
payload = {"runner": "source-python-3.11.15", "cwd": str(pathlib.Path.cwd())}
service = _build_offering_service(root_path, org_roots=[org])
try:
    service.directives
except Exception as exc:  # noqa: BLE001 - an aborting load is the answer
    payload["first"] = {"phase": "abort", "class": type(exc).__name__, "message": str(exc)}
else:
    payload["first"] = {"phase": "loaded"}
for selector in selectors:
    try:
        build_charter_context_include(root_path, selector, org_roots=[org])
    except Exception as exc:  # noqa: BLE001 - the include consumer surfaces the abort
        payload["selectors_first"] = payload.get("selectors_first") or {}
        payload["selectors_first"][selector] = {"phase": "abort", "class": type(exc).__name__, "message": str(exc)}
    else:
        payload["selectors_first"] = payload.get("selectors_first") or {}
        payload["selectors_first"][selector] = {"phase": "rendered"}
(pathlib.Path(os.environ["CF88_OFFENDER"])).unlink()
try:
    repository = service.directives
except Exception as exc:  # noqa: BLE001 - a retry that still aborts is the answer
    payload["retry"] = {"phase": "abort", "class": type(exc).__name__, "message": str(exc)}
else:
    payload["retry"] = {"phase": "loaded", "count": len(repository.list_all())}
heads = {}
for selector in selectors:
    heads[selector] = build_charter_context_include(root_path, selector, org_roots=[org]).splitlines()[0]
payload["heads"] = heads
print("CF88_SOURCE_RECEIPT " + json.dumps(payload, sort_keys=True))
"#;

const WARM_BODY: &str = r#"
from charter.activation.active_charter_service_builder import _build_offering_service
from charter.activation.context import build_charter_context_include
root_path = pathlib.Path(os.environ["CF88_ROOT"])
org = root_path / "packs" / "acme-org"
project = root_path / ".kittify" / "charter-packs" / "directive"
selector = "directive:DIRECTIVE_950"
payload = {"runner": "source-python-3.11.15", "cwd": str(pathlib.Path.cwd())}
warm = _build_offering_service(root_path, org_roots=[org])
payload["warm_first"] = warm.directives.get("DIRECTIVE_950") is not None
(project / "950-new.directive.yaml").write_text(
    'schema_version: "1.0"\nid: DIRECTIVE_950\ntitle: Repaired\nintent: I.\nenforcement: required\n',
    encoding="utf-8",
)
payload["warm_second"] = warm.directives.get("DIRECTIVE_950") is not None
fresh = _build_offering_service(root_path, org_roots=[org])
payload["fresh"] = fresh.directives.get("DIRECTIVE_950") is not None
payload["fresh_title"] = build_charter_context_include(root_path, selector, org_roots=[org]).splitlines()[0]
print("CF88_SOURCE_RECEIPT " + json.dumps(payload, sort_keys=True))
"#;

fn run_source(root: &Path, body: &str, env: &[(&str, &str)]) -> serde_json::Value {
    let mut command = child(root, support::python());
    command.arg("-c").arg(source_script(body));
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("run the pinned Source");
    std::fs::write(root.join("source.stdout"), &output.stdout).expect("source stdout");
    std::fs::write(root.join("source.stderr"), &output.stderr).expect("source stderr");
    assert!(
        output.status.success(),
        "Source child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    isolated::receipt_json(
        &String::from_utf8(output.stdout).expect("source stdout is UTF-8"),
        "CF88_SOURCE_RECEIPT ",
    )
}

fn selectors_json(selectors: &[&str]) -> String {
    serde_json::to_string(selectors).expect("selectors JSON")
}

/// The Native answer for the same cases through the public service API.
fn native_cases_receipt(root: &Path, cases: &[Case]) -> serde_json::Value {
    let mut rendered = serde_json::Map::new();
    for case in cases {
        let context = OfferingContext::new(
            root.to_path_buf(),
            OfferingRoots {
                built_in: Some(case.builtin()),
                org_roots: vec![case.org_root()],
                project: Some(case.project_root()),
            },
            None,
        );
        let service = CanonicalOfferingService::new(context);
        let entry = match service.artifact_ids(ArtifactKind::Directive) {
            Ok(ids) => serde_json::json!({
                "phase": "loaded",
                "keys": ids
                    .iter()
                    .map(|id| id.to_scalar_string().expect("fixture ids are scalars"))
                    .collect::<Vec<_>>(),
            }),
            Err(error) => match error.abort() {
                Some(abort) => serde_json::json!({
                    "phase": "abort",
                    "class": abort.python,
                    "message": abort.message,
                }),
                None => serde_json::json!({
                    "phase": "native",
                    "message": error.message(),
                }),
            },
        };
        rendered.insert(case.name.clone(), entry);
    }
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "cases": rendered,
    })
}

fn native_access(root: &Path) -> IncludeAccess {
    IncludeAccess::with_roots(
        root,
        OfferingRoots {
            built_in: Some(isolated::repository_root().join("packs/built-in")),
            org_roots: vec![root.join("packs/acme-org")],
            project: Some(root.join(".kittify/charter-packs")),
        },
    )
    .expect("native access")
}

fn render(access: &IncludeAccess, root: &Path, selectors: &[&str]) -> serde_json::Value {
    let inputs = IncludeInputs {
        project_root: root,
        access,
    };
    let mut rendered = serde_json::Map::new();
    for selector in selectors {
        let parsed = parse_include_selector(selector).expect("selector parses");
        let entry = match render_include(&inputs, &parsed, selector) {
            Ok(text) => serde_json::json!({
                "phase": "rendered",
                "head": text.lines().next().unwrap_or_default(),
            }),
            Err(IncludeError::Aborted { message }) => {
                serde_json::json!({ "phase": "abort", "message": message })
            }
            Err(other) => serde_json::json!({ "phase": "error", "message": other.to_string() }),
        };
        rendered.insert((*selector).to_owned(), entry);
    }
    serde_json::Value::Object(rendered)
}

fn native_consumer_receipt(root: &Path, selectors: &[&str], offender: &Path) -> serde_json::Value {
    let access = native_access(root);
    let service = access.offering().expect("native offering");
    let first = match service.artifact_ids(ArtifactKind::Directive) {
        Ok(_) => serde_json::json!({ "phase": "loaded" }),
        Err(error) => match error.abort() {
            Some(abort) => serde_json::json!({
                "phase": "abort",
                "class": abort.python,
                "message": abort.message,
            }),
            None => serde_json::json!({ "phase": "native", "message": error.message() }),
        },
    };
    let selectors_first = render(&access, root, selectors);
    std::fs::remove_file(offender).expect("remove the offending document");
    let retry = || match service.artifact_ids(ArtifactKind::Directive) {
        Ok(ids) => serde_json::json!({ "phase": "loaded", "count": ids.len() }),
        Err(error) => serde_json::json!({ "phase": "abort", "message": error.to_string() }),
    };
    let retried = retry();
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "first": first,
        "selectors_first": selectors_first,
        "retry": retried,
        "heads": render(&access, root, selectors),
    })
}

fn native_warm_receipt(root: &Path) -> serde_json::Value {
    const SELECTOR: &str = "directive:DIRECTIVE_950";
    let project = root.join(".kittify/charter-packs/directive");
    let access = native_access(root);
    let service = access.offering().expect("native offering");
    let present = |service: &CanonicalOfferingService| {
        service.artifact_ids(ArtifactKind::Directive).map(|ids| {
            ids.iter()
                .any(|id| id.to_scalar_string().ok().as_deref() == Some("DIRECTIVE_950"))
        })
    };
    let warm_first = present(service).expect("warm load");
    write(
        &project.join("950-new.directive.yaml"),
        "schema_version: \"1.0\"\nid: DIRECTIVE_950\ntitle: Repaired\nintent: I.\nenforcement: required\n",
    );
    let warm_second = present(service).expect("warm load");
    let fresh_access = native_access(root);
    let fresh_service = fresh_access.offering().expect("fresh offering");
    let fresh = present(fresh_service).expect("fresh load");
    let fresh_title = render(&fresh_access, root, &[SELECTOR]);
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "warm_first": warm_first,
        "warm_second": warm_second,
        "fresh": fresh,
        "fresh_title": fresh_title[SELECTOR]["head"],
    })
}

fn native_reexec_receipt(root: &Path, test_name: &str, env: &[(&str, &str)]) -> serde_json::Value {
    let executable = std::env::current_exe().expect("test binary");
    let mut command = child(root, executable);
    command
        .args(["--exact", test_name, "--nocapture"])
        .env("SK_E4_NATIVE_ROOT", root);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("run the Native child");
    std::fs::write(root.join("native.stdout"), &output.stdout).expect("native stdout");
    std::fs::write(root.join("native.stderr"), &output.stderr).expect("native stderr");
    assert!(
        output.status.success(),
        "Native child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = isolated::receipt_json(
        &String::from_utf8(output.stdout).expect("native stdout is UTF-8"),
        "CF88_NATIVE_RECEIPT ",
    );
    assert_eq!(receipt["runner"].as_str(), Some("native"));
    receipt
}

/// Replace the fixture root in every string so two independent fixtures compare.
fn normalize(root: &Path, value: &mut serde_json::Value) {
    let prefix = root.display().to_string();
    fn walk(prefix: &str, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => *text = text.replace(prefix, "<ROOT>"),
            serde_json::Value::Array(items) => items.iter_mut().for_each(|item| walk(prefix, item)),
            serde_json::Value::Object(map) => map.values_mut().for_each(|item| walk(prefix, item)),
            _ => {}
        }
    }
    walk(&prefix, value);
}

fn case_entry<'a>(receipt: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    receipt["cases"]
        .get(name)
        .unwrap_or_else(|| panic!("no case {name} in receipt: {receipt}"))
}

#[test]
#[ignore = "protocol quarantine: shared fixture, root rewriting and incomplete child guards"]
fn directive_inline_references_abort_like_the_source() {
    if let Some(root) = isolated::native_root() {
        assert_child_roots(&root);
        let cases = Case::from_env();
        println!(
            "CF88_NATIVE_RECEIPT {}",
            native_cases_receipt(&root, &cases)
        );
        return;
    }
    let root = fixture("cf88-directive-prevalidation");
    let cases = write_cases(&root);
    let mut source = run_source(&root, CASES_BODY, &[("CF88_CASES", &Case::to_json(&cases))]);
    assert_eq!(source["runner"].as_str(), Some("source-python-3.11.15"));

    // The pinned Source cause, asserted before the Native leg runs.
    for (case, field) in [
        ("builtin-tactic_refs", "tactic_refs"),
        ("builtin-paradigm_refs", "paradigm_refs"),
        ("builtin-applies_to", "applies_to"),
        ("builtin-two-fields", "tactic_refs"),
        ("org-inline-no-id", "tactic_refs"),
        ("org-inline-shadow", "tactic_refs"),
        ("project-inline", "tactic_refs"),
        ("builtin-structured-id", "tactic_refs"),
    ] {
        let entry = case_entry(&source, case);
        assert_eq!(entry["phase"].as_str(), Some("abort"), "{case}: {entry}");
        assert_eq!(
            entry["class"].as_str(),
            Some("InlineReferenceRejectedError"),
            "{case}: the source raises its own ValueError subclass"
        );
        let message = entry["message"].as_str().expect("message");
        assert!(
            message.starts_with("Inline reference rejected in "),
            "{case}: {message}"
        );
        assert!(
            message.contains(&format!(
                "\n  artifact_kind: directive\n  forbidden_field: {field}\n  migration: Remove {field} from YAML; add edge {{source: directive:"
            )),
            "{case}: {message}"
        );
        assert!(
            message.ends_with("to packs/built-in/directive.graph.yaml"),
            "{case}: {message}"
        );
        assert!(message.contains(".directive.yaml:\n"), "{case}: {message}");
    }
    let no_id = case_entry(&source, "org-inline-no-id")["message"]
        .as_str()
        .expect("message");
    assert!(
        no_id.contains("{source: directive:?,"),
        "the hook runs before the overlay id check: {no_id}"
    );
    let structured = case_entry(&source, "builtin-structured-id")["message"]
        .as_str()
        .expect("message");
    assert!(
        structured.contains("{source: directive:{'nested': [1, 2]},"),
        "the id label is Python str() of the raw value: {structured}"
    );
    for case in ["nonmapping", "schema-invalid", "syntax-error"] {
        let entry = case_entry(&source, case);
        assert_eq!(entry["phase"].as_str(), Some("loaded"), "{case}: {entry}");
        assert_eq!(
            entry["keys"],
            serde_json::json!(["DIRECTIVE_001"]),
            "{case}: the per-file skip is kept, so the other document still loads"
        );
    }
    let healthy = case_entry(&source, "healthy");
    assert_eq!(healthy["phase"].as_str(), Some("loaded"));
    assert_eq!(
        healthy["keys"],
        serde_json::json!(["DIRECTIVE_001", "DIRECTIVE_003"]),
        "the healthy fixture loads both layers: {healthy}"
    );

    // The Native leg must produce the same phase, class, message and key sets.
    let mut native = native_reexec_receipt(
        &root,
        "directive_inline_references_abort_like_the_source",
        &[("CF88_CASES", &Case::to_json(&cases))],
    );
    normalize(&root, &mut source);
    normalize(&root, &mut native);
    for case in &cases {
        assert_eq!(
            case_entry(&native, &case.name),
            case_entry(&source, &case.name),
            "{}: the Native load must match the pinned Source",
            case.name
        );
    }
}

#[test]
#[ignore = "protocol quarantine: root rewriting and incomplete child guards"]
fn the_service_and_include_consumer_abort_and_retry_like_the_source() {
    if let Some(root) = isolated::native_root() {
        assert_child_roots(&root);
        let selectors = std::env::var("CF88_SELECTORS").expect("CF88_SELECTORS");
        let selectors: Vec<String> = serde_json::from_str(&selectors).expect("selectors JSON");
        let selectors: Vec<&str> = selectors.iter().map(String::as_str).collect();
        let offender = root.join(".kittify/charter-packs/directive/900-inline.directive.yaml");
        println!(
            "CF88_NATIVE_RECEIPT {}",
            native_consumer_receipt(&root, &selectors, &offender)
        );
        return;
    }
    let selectors = [
        "directive:024-locality-of-change",
        "directive:025-boy-scout-rule",
    ];

    let source_root = fixture("cf88-directive-consumer-source");
    let source_offender = write_consumer_fixture(&source_root);
    let mut source = run_source(
        &source_root,
        CONSUMER_BODY,
        &[
            ("CF88_SELECTORS", &selectors_json(&selectors)),
            ("CF88_OFFENDER", &source_offender.display().to_string()),
        ],
    );
    let first = &source["first"];
    assert_eq!(first["phase"].as_str(), Some("abort"), "{first}");
    assert_eq!(
        first["class"].as_str(),
        Some("InlineReferenceRejectedError"),
        "the service property raises the source exception: {first}"
    );
    for selector in selectors {
        assert_eq!(
            source["selectors_first"][selector]["class"].as_str(),
            Some("InlineReferenceRejectedError"),
            "{selector}: the include consumer surfaces the abort, not a miss: {source}"
        );
    }
    assert_eq!(
        source["retry"]["phase"].as_str(),
        Some("loaded"),
        "the same service retries after the offending file is gone: {source}"
    );
    assert_eq!(
        source["retry"]["count"].as_u64(),
        Some(38),
        "the repaired fixture loads the shipped built-in pack: {source}"
    );

    let native_root = fixture("cf88-directive-consumer-native");
    let native_offender = write_consumer_fixture(&native_root);
    let mut native = native_reexec_receipt(
        &native_root,
        "the_service_and_include_consumer_abort_and_retry_like_the_source",
        &[
            ("CF88_SELECTORS", &selectors_json(&selectors)),
            ("CF88_OFFENDER", &native_offender.display().to_string()),
        ],
    );
    normalize(&source_root, &mut source);
    normalize(&native_root, &mut native);
    assert_eq!(
        native["first"], source["first"],
        "the Native service carries the Source class and full abort message"
    );
    for selector in selectors {
        assert_eq!(
            native["selectors_first"][selector]["phase"].as_str(),
            Some("abort"),
            "{selector}: {native}"
        );
        assert_eq!(
            native["selectors_first"][selector]["message"],
            format!(
                "{}: {}",
                source["first"]["class"].as_str().expect("class"),
                source["first"]["message"].as_str().expect("message")
            ),
            "{selector}: the include abort carries the Source class and message"
        );
    }
    assert_eq!(
        native["retry"], source["retry"],
        "the same Native service retries to the same loaded id count"
    );
    for selector in selectors {
        assert_eq!(
            native["heads"][selector]["head"], source["heads"][selector],
            "{selector}: the repaired fixture renders the same heading"
        );
    }
}

#[test]
#[ignore = "protocol quarantine: root rewriting and incomplete child guards"]
fn warm_snapshots_and_fresh_reads_match_the_source() {
    if let Some(root) = isolated::native_root() {
        assert_child_roots(&root);
        println!("CF88_NATIVE_RECEIPT {}", native_warm_receipt(&root));
        return;
    }
    let source_root = fixture("cf88-directive-warm-source");
    write_warm_fixture(&source_root);
    let mut source = run_source(&source_root, WARM_BODY, &[]);
    assert_eq!(
        source["warm_first"].as_bool(),
        Some(false),
        "the schema-invalid new document is skipped: {source}"
    );
    assert_eq!(
        source["warm_second"].as_bool(),
        Some(false),
        "the warm snapshot keeps the skip after the repair: {source}"
    );
    assert_eq!(source["fresh"].as_bool(), Some(true), "{source}");
    assert_eq!(
        source["fresh_title"].as_str(),
        Some("Directive DIRECTIVE_950: Repaired"),
        "a fresh service sees the repair: {source}"
    );

    let native_root = fixture("cf88-directive-warm-native");
    write_warm_fixture(&native_root);
    let mut native = native_reexec_receipt(
        &native_root,
        "warm_snapshots_and_fresh_reads_match_the_source",
        &[],
    );
    normalize(&source_root, &mut source);
    normalize(&native_root, &mut native);
    for key in ["warm_first", "warm_second", "fresh", "fresh_title"] {
        assert_eq!(
            native[key], source[key],
            "{key}: the Native read must match the pinned Source"
        );
    }
}

fn write_consumer_fixture(root: &Path) -> PathBuf {
    let org = root.join("packs/acme-org");
    let project = root.join(".kittify/charter-packs/directive");
    std::fs::create_dir_all(&project).expect("project layer");
    std::fs::create_dir_all(org.join("directives")).expect("org layer");
    let offender = project.join("900-inline.directive.yaml");
    write(
        &offender,
        "id: DIRECTIVE_024\ntitle: Inline\ntactic_refs: [x]\n",
    );
    offender
}

fn write_warm_fixture(root: &Path) {
    let org = root.join("packs/acme-org");
    let project = root.join(".kittify/charter-packs/directive");
    std::fs::create_dir_all(&project).expect("project layer");
    std::fs::create_dir_all(org.join("directives")).expect("org layer");
    write(
        &project.join("950-new.directive.yaml"),
        "schema_version: \"1.0\"\nid: DIRECTIVE_950\ntitle: Missing Enforcement\nintent: I.\n",
    );
}
