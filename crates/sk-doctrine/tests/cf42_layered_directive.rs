//! CF42: the canonical typed layered **Directive** repository as the include consumer.
//!
//! The pinned Source exposes `CharterOfferingService.directives` (a
//! `BaseArtifactRepository`) and the `--include directive:<id>` renderer reads its
//! *merged* record. This suite pins the Native equivalent end to end:
//!
//! * **admission** is the ONE whole-model authority (`offering::model`): a document
//!   the reference's `Directive` model rejects is *skipped*, so it is a miss rather
//!   than a render, while a document the loader cannot even key (an unhashable raw
//!   `id`) still aborts the load;
//! * **layering** is built-in → org → project with the reference's field merge onto
//!   the *built-in* item, so an overlay that re-declares only `title` still prints the
//!   built-in `intent`;
//! * **identity** is the crate's one `IdentityIndex` (declared id, file stem, numeric
//!   shorthand), and the plain `artifact:` probe uses the directive id normaliser;
//! * **the consumer** prints the merged model dump (CF38's exact payload), never the
//!   declaring file's raw YAML — the deleted fallback's premise.
//!
//! Both legs run as **actual re-executed children**: the Source leg is the pinned,
//! lock-exact CPython oracle; the Native leg is this binary re-executed. Each child
//! asserts its CWD, HOME, all seven XDG roots (including `XDG_CONFIG_DIRS` and
//! `XDG_DATA_DIRS`), `TMPDIR`/`TMP`/`TEMP`, the absolute repository `src` on
//! `PYTHONPATH` and the pinned packs root **before** computing. Source runs first, its
//! intended cause is asserted, and only then does Native run.
//!
//! Deliberately deferred proof (reported as debt, not approximated here): the
//! repository's `_pre_validate` inline-reference rejection (an *aborting* ValueError
//! Native's shared loader has no hook for), the reference's skip-warning text, and the
//! `!!binary`/trailing-newline corner cases of the id pattern are not re-paired here
//! beyond the admission unit tests.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_doctrine::context_include::{
    IncludeAccess, IncludeError, IncludeInputs, parse_include_selector, render_include,
};
use sk_doctrine::{CanonicalOfferingService, OfferingContext, OfferingRoots};

/// The two XDG roots the shared E4 protocol does not set; CF42 requires both.
const EXTRA_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "configdirs"),
    ("XDG_DATA_DIRS", "datadirs"),
];

/// The pre-edit order the Source renderer must produce, selector by selector.
const DEFAULT_SELECTORS: [&str; 18] = [
    // identity: file stem, declared id and numeric shorthand name one merged record
    "directive:024-locality-of-change",
    "directive:DIRECTIVE_024",
    "directive:24",
    // a stem whose declared id differs from its numeric prefix
    "directive:900-acme-override",
    // the project overlay of a different built-in
    "directive:010-specification-fidelity-requirement",
    // an untouched built-in (the real shipped document)
    "directive:025-boy-scout-rule",
    // an org-only artifact and its stem
    "directive:ACME_900",
    "directive:909-lenient-ok",
    // whole-model rejects: a miss, never a render and never a fabricated dump
    "directive:invalid",
    "directive:bad-id",
    "directive:badref",
    "directive:extra",
    "directive:lenient",
    "directive:no-such",
    // an unrelated kind whose file is syntactically broken is a per-file skip, and
    // never changes a directive answer (the fixture writes it)
    "tactic:broken-tactic",
    // the best-effort probe takes the normaliser, not the identity index
    "artifact:24",
    "artifact:900",
    "artifact:909",
];

/// One child's answer for one selector.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Rendered {
    Text(String),
    Error(String),
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

/// Which directives the fixture carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// built-in pack + org overlays/invalids + a project overlay (+ a broken tactic).
    Layered,
    /// an overlay whose raw `id` is a list: the reference's `TypeError` abort.
    UnhashableId,
    /// the same overlays plus an activation-ledger deny.
    Denied,
}

/// A fixture repo over the shipped built-in pack.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(case: &str, shape: Shape) -> Self {
        let root = isolated::fixture(case);
        for (_, name) in EXTRA_ROOTS {
            std::fs::create_dir_all(root.join(name)).expect("extra isolated root");
        }
        let org = root.join("packs/acme-org");
        let project = root.join(".kittify/charter-packs");
        std::fs::create_dir_all(org.join("directives")).expect("org directives");
        std::fs::create_dir_all(project.join("directive")).expect("project directives");
        // A project config and a broken unrelated kind: neither may change a
        // directive answer, and neither may be eagerly parsed.
        write(
            &root.join(".kittify/config.yaml"),
            "# authored project config\n",
        );
        write(
            &root.join("packs/acme-org/tactics/broken.tactic.yaml"),
            "id: broken-tactic\nsteps: [unclosed\n",
        );
        match shape {
            Shape::Layered | Shape::Denied => Self::layered_overlays(&org, &project),
            Shape::UnhashableId => write(
                &org.join("directives/913-list-id.directive.yaml"),
                "id: [not, hashable]\ntitle: List Id\n",
            ),
        }
        if shape == Shape::Denied {
            write(
                &root.join(".kittify/charter/activation.yaml"),
                "deactivated_directives:\n- DIRECTIVE_024\n- never-written\n",
            );
        }
        Self { root }
    }

    /// The overlays and the whole-model-invalid documents of the default fixture.
    fn layered_overlays(org: &Path, project: &Path) {
        // org partial overlay of a real built-in: the field merge must inherit
        // everything it does not repeat.
        write(
            &org.join("directives/900-acme-override.directive.yaml"),
            "id: DIRECTIVE_024\ntitle: Org Locality Title\n",
        );
        // project partial overlay of a different real built-in.
        write(
            &project.join("directive/901-acme-project.directive.yaml"),
            "id: DIRECTIVE_010\ntitle: Project Fidelity Title\nintent: Project-authored intent override.\n",
        );
        // org-only artifacts: one advisory with a typed reference, one lenient.
        write(
            &org.join("directives/902-acme-new.directive.yaml"),
            "schema_version: \"1.0\"\nid: ACME_900\ntitle: Acme New Directive\nintent: Acme authored intent.\n\
             enforcement: advisory\nprocedures:\n- Follow the acme procedure.\nreferences:\n\
             - type: tactic\n  id: acme-tactic\n  when: on acme work\n  reason: acme rationale\n",
        );
        write(
            &org.join("directives/909-lenient-ok.directive.yaml"),
            "schema_version: \"1.0\"\nid: DIRECTIVE_909\ntitle: Lenient With Allowances\n\
             intent: Lenient ok.\nenforcement: lenient-adherence\n\
             explicit_allowances:\n- allowed deviation\n",
        );
        // whole-model invalid: required `enforcement` missing.
        write(
            &org.join("directives/903-invalid.directive.yaml"),
            "schema_version: \"1.0\"\nid: DIRECTIVE_903\ntitle: Invalid Missing Enforcement\n\
             intent: Invalid.\n",
        );
        // whole-model invalid: the id pattern is upper-case only.
        write(
            &org.join("directives/904-bad-id.directive.yaml"),
            "schema_version: \"1.0\"\nid: directive_904\ntitle: Bad Id\nintent: Bad id.\n\
             enforcement: required\n",
        );
        // whole-model invalid: lenient-adherence needs explicit allowances.
        write(
            &org.join("directives/905-lenient.directive.yaml"),
            "schema_version: \"1.0\"\nid: DIRECTIVE_905\ntitle: Lenient Without Allowances\n\
             intent: Lenient.\nenforcement: lenient-adherence\n",
        );
        // whole-model invalid: a reference kind that is not a canonical value.
        write(
            &org.join("directives/906-badref.directive.yaml"),
            "schema_version: \"1.0\"\nid: DIRECTIVE_906\ntitle: Bad Reference Type\n\
             intent: Bad ref.\nenforcement: required\nreferences:\n- type: nope\n  id: x\n",
        );
        // whole-model invalid: `extra="forbid"`.
        write(
            &org.join("directives/908-extra.directive.yaml"),
            "schema_version: \"1.0\"\nid: DIRECTIVE_908\ntitle: Extra Key\nintent: Extra.\n\
             enforcement: required\nextra_field: 1\n",
        );
    }
}

/// The isolated child environment both legs share.
fn child(root: &Path, program: impl AsRef<OsStr>) -> Command {
    let repository = isolated::repository_root();
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("CF42_ROOT", root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in isolated::ROOTS.iter().chain(EXTRA_ROOTS.iter()) {
        command.env(key, root.join(name));
    }
    command
}

/// Assert every isolated root the child was handed, and the lock-exact oracle.
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
    // Verified before any computation on either leg.
    let _ = support::python();
}

const SOURCE_PROBE: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["CF42_ROOT"])
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
from charter.activation.context import build_charter_context_include
from charter.activation.active_charter_service_builder import _build_offering_service
org = root / "packs" / "acme-org"
payload = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "keys": [],
    "selectors": {},
}
try:
    repository = _build_offering_service(root, org_roots=[org]).directives
except Exception as exc:  # noqa: BLE001 - an aborting load is the answer
    payload["load_error"] = "%s: %s" % (type(exc).__name__, str(exc).splitlines()[0])
else:
    payload["keys"] = [
        {"key": item.id, "provenance": repository.get_provenance(item.id)}
        for item in repository.list_all()
    ]
for selector in json.loads(os.environ["CF42_SELECTORS"]):
    try:
        text = build_charter_context_include(root, selector, org_roots=[org])
    except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the answer
        payload["selectors"][selector] = {
            "error": "%s: %s" % (type(exc).__name__, str(exc).splitlines()[0])
        }
    else:
        payload["selectors"][selector] = {"text": text}
print("CF42_SOURCE_RECEIPT " + json.dumps(payload))
"#;

fn selectors_json(selectors: &[&str]) -> String {
    serde_json::to_string(selectors).expect("selectors JSON")
}

/// Run the pinned Source child for `selectors`.
fn run_source(root: &Path, selectors: &[&str]) -> serde_json::Value {
    let output = child(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("CF42_SELECTORS", selectors_json(selectors))
        .output()
        .expect("run the pinned Source");
    std::fs::write(root.join("source.stdout"), &output.stdout).expect("source stdout");
    std::fs::write(root.join("source.stderr"), &output.stderr).expect("source stderr");
    assert!(
        output.status.success(),
        "Source child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("source stdout is UTF-8");
    let receipt = isolated::receipt_json(&stdout, "CF42_SOURCE_RECEIPT ");
    assert_eq!(
        receipt["runner"].as_str(),
        Some("source-python-3.11.15"),
        "the Source child is the pinned interpreter"
    );
    assert_eq!(
        receipt["cwd"].as_str(),
        Some(root.display().to_string().as_str())
    );
    receipt
}

/// The Native answer for `selectors`, as the receipt both legs compare.
fn native_receipt_json(root: &Path, selectors: &[&str]) -> serde_json::Value {
    let access = IncludeAccess::with_roots(
        root,
        OfferingRoots {
            built_in: Some(isolated::repository_root().join("packs/built-in")),
            org_roots: vec![root.join("packs/acme-org")],
            project: Some(root.join(".kittify/charter-packs")),
        },
    )
    .expect("native access");
    let inputs = IncludeInputs {
        project_root: root,
        access: &access,
    };
    let mut rendered = serde_json::Map::new();
    for selector in selectors {
        let parsed = parse_include_selector(selector).expect("selector parses");
        let value = match render_include(&inputs, &parsed, selector) {
            Ok(text) => serde_json::json!({ "text": text }),
            Err(error) => serde_json::json!({ "error": native_error(&error) }),
        };
        rendered.insert((*selector).to_owned(), value);
    }
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "selectors": rendered,
    })
}

/// The Source exception spelling of a Native include failure (no second taxonomy).
///
/// The reference raises `ValueError` for a miss and lets the loader's own exception
/// (`TypeError`, …) escape an abort; both reach the CLI as `<class>: <message>`.
fn native_error(error: &IncludeError) -> String {
    match error {
        IncludeError::Missing { message } => format!("ValueError: {message}"),
        IncludeError::Aborted { message } => message.clone(),
        other => other.to_string(),
    }
}

/// Run the Native leg as an isolated re-exec and return its receipt.
fn run_native(root: &Path, selectors: &[&str], test_name: &str) -> serde_json::Value {
    let executable = std::env::current_exe().expect("test binary");
    let output = child(root, executable)
        .args(["--exact", test_name, "--nocapture"])
        .env("SK_E4_NATIVE_ROOT", root)
        .env("CF42_SELECTORS", selectors_json(selectors))
        .output()
        .expect("run the Native child");
    std::fs::write(root.join("native.stdout"), &output.stdout).expect("native stdout");
    std::fs::write(root.join("native.stderr"), &output.stderr).expect("native stderr");
    assert!(
        output.status.success(),
        "Native child failed (status {:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("native stdout is UTF-8");
    let receipt = isolated::receipt_json(&stdout, "CF42_NATIVE_RECEIPT ");
    assert_eq!(receipt["runner"].as_str(), Some("native"));
    assert_eq!(
        receipt["cwd"].as_str(),
        Some(root.display().to_string().as_str())
    );
    receipt
}

/// The selectors the Native child was asked to render.
fn child_selectors() -> Vec<String> {
    let raw = std::env::var("CF42_SELECTORS").expect("CF42_SELECTORS");
    serde_json::from_str(&raw).expect("selectors JSON")
}

fn selector_value(receipt: &serde_json::Value, selector: &str) -> Rendered {
    let entry = receipt["selectors"]
        .get(selector)
        .unwrap_or_else(|| panic!("no {selector} in receipt: {receipt}"));
    match (entry.get("text"), entry.get("error")) {
        (Some(text), None) => Rendered::Text(text.as_str().expect("text").to_owned()),
        (None, Some(error)) => Rendered::Error(error.as_str().expect("error").to_owned()),
        _ => panic!("malformed receipt entry for {selector}: {entry}"),
    }
}

/// Compare one selector's two legs, naming which side deviated.
fn assert_same_selector(source: &serde_json::Value, native: &serde_json::Value, selector: &str) {
    assert_eq!(
        selector_value(native, selector),
        selector_value(source, selector),
        "{selector}: Native must render what the pinned Source renders"
    );
}

/// Source-only assertions on the promoted include text, per selector.
fn assert_source_cause(source: &serde_json::Value) {
    let text = |selector: &str| match selector_value(source, selector) {
        Rendered::Text(text) => text,
        Rendered::Error(error) => panic!("{selector} must render, got {error}"),
    };
    let error = |selector: &str| match selector_value(source, selector) {
        Rendered::Text(text) => panic!("{selector} must refuse, got {text}"),
        Rendered::Error(error) => error,
    };

    // The org overlay replaces only `title`; the built-in fields are inherited and the
    // payload is the merged model dump (never the declaring file's raw YAML).
    for selector in [
        "directive:024-locality-of-change",
        "directive:DIRECTIVE_024",
        "directive:24",
        "directive:900-acme-override",
        "artifact:24",
    ] {
        let rendered = text(selector);
        assert!(
            rendered.starts_with("Directive DIRECTIVE_024: Org Locality Title\n"),
            "{selector}: {rendered}"
        );
        assert!(
            rendered.contains("\"title\": \"Org Locality Title\""),
            "{selector}: the overlay field is the merged record: {rendered}"
        );
        assert!(
            rendered.contains("\"scope\": \"Applies to code, tests, and documentation"),
            "{selector}: the built-in scope is inherited: {rendered}"
        );
        assert!(
            !rendered.contains("schema_version:"),
            "{selector}: {rendered}"
        );
    }
    // The project overlay merges onto the built-in base too.
    let project = text("directive:010-specification-fidelity-requirement");
    assert!(
        project.starts_with("Directive DIRECTIVE_010: Project Fidelity Title\n"),
        "{project}"
    );
    assert!(
        project.contains("\"intent\": \"Project-authored intent override.\""),
        "{project}"
    );
    // An untouched built-in directive still renders its shipped record.
    let built_in = text("directive:025-boy-scout-rule");
    assert!(
        built_in.starts_with("Directive DIRECTIVE_025: Boy Scout Rule\n"),
        "{built_in}"
    );
    assert!(
        built_in.contains("    Full artifact:\n      {\n"),
        "{built_in}"
    );
    // An org-only artifact and its stem resolve through the identity index.
    let org_only = text("directive:ACME_900");
    assert!(
        org_only.starts_with("Directive ACME_900: Acme New Directive\n"),
        "{org_only}"
    );
    assert!(
        org_only.contains("\"type\": \"tactic\""),
        "the typed reference is dumped: {org_only}"
    );
    let lenient = text("directive:909-lenient-ok");
    assert!(
        lenient.starts_with("Directive DIRECTIVE_909: Lenient With Allowances\n"),
        "{lenient}"
    );
    assert!(
        lenient.contains("\"enforcement\": \"lenient-adherence\"")
            && lenient.contains("\"allowed deviation\""),
        "the lenient directive's allowances are dumped: {lenient}"
    );
    // Whole-model rejects: the reference skips the document, so the selector misses.
    for (selector, id) in [
        ("directive:invalid", "DIRECTIVE_903"),
        ("directive:bad-id", "DIRECTIVE_904"),
        ("directive:lenient", "DIRECTIVE_905"),
        ("directive:badref", "DIRECTIVE_906"),
        ("directive:extra", "DIRECTIVE_908"),
    ] {
        assert_eq!(
            error(selector),
            format!("ValueError: No directive found for selector '{selector}'."),
            "{selector} ({id}) must be a whole-model miss"
        );
    }
    assert_eq!(
        error("directive:no-such"),
        "ValueError: No directive found for selector 'directive:no-such'.".to_owned()
    );
    // The probe normalises the token (`900` -> `DIRECTIVE_900`) and then misses,
    // because the oracle only declares `DIRECTIVE_024` in that file.
    assert_eq!(
        error("artifact:900"),
        "ValueError: No artifact found for selector 'artifact:900'.".to_owned()
    );
    assert_eq!(
        error("tactic:broken-tactic"),
        "ValueError: No tactic found for selector 'tactic:broken-tactic'.".to_owned(),
        "an unrelated broken file is skipped, not promoted into a directive failure"
    );
}

/// The Source repository key/provenance half (Source truth; the pair covers the rest).
fn assert_source_keys(source: &serde_json::Value) {
    let keys: Vec<(String, String)> = source["keys"]
        .as_array()
        .expect("keys array")
        .iter()
        .map(|entry| {
            (
                entry["key"].as_str().expect("key").to_owned(),
                entry["provenance"].as_str().expect("provenance").to_owned(),
            )
        })
        .collect();
    assert!(
        keys.iter()
            .any(|(key, layer)| key == "DIRECTIVE_024" && layer == "org"),
        "the org overlay is the served layer: {keys:?}"
    );
    assert!(
        keys.iter()
            .any(|(key, layer)| key == "DIRECTIVE_010" && layer == "project"),
        "the project overlay is the served layer: {keys:?}"
    );
    assert!(
        keys.iter()
            .any(|(key, layer)| key == "ACME_900" && layer == "org"),
        "an org-only artifact is inserted under its declared id: {keys:?}"
    );
    assert!(
        keys.iter()
            .any(|(key, layer)| key == "DIRECTIVE_909" && layer == "org"),
        "the lenient artifact is admitted once its allowances are declared: {keys:?}"
    );
    for skipped in [
        "DIRECTIVE_903",
        "directive_904",
        "DIRECTIVE_905",
        "DIRECTIVE_906",
        "DIRECTIVE_908",
        "DIRECTIVE_900",
    ] {
        assert!(
            !keys.iter().any(|(key, _)| key == skipped),
            "{skipped} is not an admitted directive: {keys:?}"
        );
    }
    assert!(
        keys.iter()
            .any(|(key, layer)| key == "DIRECTIVE_025" && layer == "built-in"),
        "the shipped built-in layer contributes its directives: {keys:?}"
    );
}

#[test]
fn layered_directive_records_render_like_the_source() {
    if let Some(root) = isolated::native_root() {
        assert_child_roots(&root);
        let selectors: Vec<String> = child_selectors();
        let refs: Vec<&str> = selectors.iter().map(String::as_str).collect();
        println!("CF42_NATIVE_RECEIPT {}", native_receipt_json(&root, &refs));
        return;
    }
    let fixture = Fixture::new("cf42-layered", Shape::Layered);
    let selectors: Vec<&str> = DEFAULT_SELECTORS.to_vec();
    let source = run_source(&fixture.root, &selectors);
    assert!(
        source.get("load_error").is_none(),
        "the pinned Source repository loads: {source}"
    );
    assert_source_keys(&source);
    assert_source_cause(&source);
    let native = run_native(
        &fixture.root,
        &selectors,
        "layered_directive_records_render_like_the_source",
    );
    for selector in selectors {
        assert_same_selector(&source, &native, selector);
    }
}

#[test]
fn an_unhashable_overlay_id_aborts_like_the_source() {
    if let Some(root) = isolated::native_root() {
        assert_child_roots(&root);
        let selectors: Vec<String> = child_selectors();
        let refs: Vec<&str> = selectors.iter().map(String::as_str).collect();
        println!("CF42_NATIVE_RECEIPT {}", native_receipt_json(&root, &refs));
        return;
    }
    let fixture = Fixture::new("cf42-abort", Shape::UnhashableId);
    let selectors = ["directive:025-boy-scout-rule"];
    let source = run_source(&fixture.root, &selectors);
    // The reference's loader does not catch `TypeError`, so the whole load aborts
    // instead of skipping the file (a constructor abort, not a valid-input refusal).
    assert_eq!(
        source["load_error"].as_str(),
        Some("TypeError: unhashable type: 'list'"),
        "{source}"
    );
    assert_eq!(
        selector_value(&source, selectors[0]),
        Rendered::Error("TypeError: unhashable type: 'list'".to_owned())
    );
    let native = run_native(
        &fixture.root,
        &selectors,
        "an_unhashable_overlay_id_aborts_like_the_source",
    );
    for selector in selectors {
        assert_same_selector(&source, &native, selector);
    }
}

#[test]
fn a_denied_directive_is_annotated_before_any_model_load() {
    if let Some(root) = isolated::native_root() {
        assert_child_roots(&root);
        let selectors: Vec<String> = child_selectors();
        let refs: Vec<&str> = selectors.iter().map(String::as_str).collect();
        println!("CF42_NATIVE_RECEIPT {}", native_receipt_json(&root, &refs));
        return;
    }
    let fixture = Fixture::new("cf42-denied", Shape::Denied);
    let selectors = [
        "directive:024-locality-of-change",
        "directive:never-written",
        "artifact:24",
    ];
    let source = run_source(&fixture.root, &selectors);
    assert_eq!(
        selector_value(&source, "directive:024-locality-of-change"),
        Rendered::Text(
            "directive:024-locality-of-change is deactivated in this project \
             (charter activate directive 024-locality-of-change restores it)"
                .to_owned()
        ),
        "the deny note is an annotation, not a refusal: {source}"
    );
    assert_eq!(
        selector_value(&source, "directive:never-written"),
        Rendered::Text(
            "directive:never-written is deactivated in this project \
             (charter activate directive never-written restores it)"
                .to_owned()
        ),
        "the annotation is decided before any artifact tree is read: {source}"
    );
    let native = run_native(
        &fixture.root,
        &selectors,
        "a_denied_directive_is_annotated_before_any_model_load",
    );
    for selector in selectors {
        assert_same_selector(&source, &native, selector);
    }
}

/// API-shape test: the service now serves directives without widening the
/// activation-key mapping (no second, activated-id view of the same kind).
#[test]
fn serving_directives_does_not_change_the_activation_mapping() {
    let context = OfferingContext::resolve(&isolated::repository_root()).expect("offering context");
    let service = CanonicalOfferingService::new(context);
    assert!(CanonicalOfferingService::serves_documents(
        sk_charter::ArtifactKind::Directive
    ));
    assert_eq!(
        service
            .ids_for("activated_directives")
            .expect("directive key"),
        Vec::new(),
        "the reference unions no directive service keys; the scan owns them"
    );
}
