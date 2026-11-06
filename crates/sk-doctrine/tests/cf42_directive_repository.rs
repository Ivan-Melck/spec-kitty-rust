//! CF42: the Directive repository's **layer order** and **collision** contract.
//!
//! `cf42_layered_directive.rs` proves the served include text; this suite pins the
//! layering rule itself, which the reference states in `BaseArtifactRepository`:
//!
//! * built-in → each declared org pack (in declaration order, later wins) → project;
//! * a same-id overlay merges **field by field onto the built-in item**, so a project
//!   overlay that re-declares only `title` discards an org `intent` it does not repeat
//!   (`_merge(built_in[item_id], data)`), and never composes onto the org result;
//! * a new id declared by two org packs resolves to the later pack;
//! * the repository key stays the authored id, so the served record is addressable by
//!   the declared id, the file stem and the numeric shorthand.
//!
//! Both legs are actual re-executed children; each asserts CWD, HOME, all seven XDG
//! roots (including `XDG_CONFIG_DIRS` and `XDG_DATA_DIRS`), `TMPDIR`/`TMP`/`TEMP`, the
//! absolute repository `src` on `PYTHONPATH` and the pinned packs root before
//! computing, and the Source leg is the lock-exact pinned oracle.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_doctrine::OfferingRoots;
use sk_doctrine::context_include::{
    IncludeAccess, IncludeInputs, parse_include_selector, render_include,
};

const EXTRA_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "configdirs"),
    ("XDG_DATA_DIRS", "datadirs"),
];

const SELECTORS: [&str; 6] = [
    // title from each layer; the built-in scope/intent inheritance is asserted below
    "directive:024-locality-of-change",
    // an id declared by BOTH org packs, the later one only partially
    "directive:ACME_910",
    "directive:910-shared",
    // the mirror case: an earlier partial document is skipped
    "directive:ACME_911",
    // a project-only artifact
    "directive:010-specification-fidelity-requirement",
    // a numeric shorthand for the doubly-overlaid built-in
    "directive:24",
];

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

/// Two org packs in declaration order plus the project pack root.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = isolated::fixture("cf42-layer-order");
        for (_, name) in EXTRA_ROOTS {
            std::fs::create_dir_all(root.join(name)).expect("extra isolated root");
        }
        let acme = root.join("packs/acme-org");
        let beta = root.join("packs/beta-org");
        let project = root.join(".kittify/charter-packs");
        // The earlier org pack declares the shared new id …
        write(
            &acme.join("directives/910-shared.directive.yaml"),
            "schema_version: \"1.0\"\nid: ACME_910\ntitle: Acme Shared Title\n\
             intent: Acme shared intent.\nenforcement: advisory\n",
        );
        // … and the later org pack re-declares it with a PARTIAL document. A NEW id is
        // validated exactly as authored (the reference's `_insert_overlay_item`), so this
        // partial document is skipped rather than merged, and the earlier complete
        // record survives — the asymmetry against a shadowing overlay, which merges.
        write(
            &beta.join("directives/910-shared.directive.yaml"),
            "id: ACME_910\ntitle: Beta Shared Title\n",
        );
        // The mirror case: the earlier pack's partial document is skipped and the later
        // pack's complete one is inserted.
        write(
            &acme.join("directives/911-mirror.directive.yaml"),
            "id: ACME_911\ntitle: Acme Partial Title\n",
        );
        write(
            &beta.join("directives/911-mirror.directive.yaml"),
            "schema_version: \"1.0\"\nid: ACME_911\ntitle: Beta Complete Title\n\
             intent: Beta complete intent.\nenforcement: advisory\n",
        );
        // Both org packs touch the same built-in id; the later pack's title wins, but
        // the intent it does not repeat still comes from the built-in, not from acme.
        write(
            &acme.join("directives/900-acme-override.directive.yaml"),
            "id: DIRECTIVE_024\ntitle: Acme Locality Title\nintent: Acme locality intent.\n",
        );
        write(
            &beta.join("directives/900-beta-override.directive.yaml"),
            "id: DIRECTIVE_024\ntitle: Beta Locality Title\n",
        );
        // A project overlay of a different built-in.
        write(
            &project.join("directive/901-project.directive.yaml"),
            "id: DIRECTIVE_010\ntitle: Project Fidelity Title\n\
             intent: Project-authored intent override.\n",
        );
        write(
            &root.join(".kittify/config.yaml"),
            "# authored project config\n",
        );
        Self { root }
    }
}

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
        Some(repository.join("src"))
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repository.join("packs"))
    );
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
org_roots = [root / "packs" / "acme-org", root / "packs" / "beta-org"]
repository = _build_offering_service(root, org_roots=org_roots).directives
payload = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "keys": [
        {"key": item.id, "provenance": repository.get_provenance(item.id)}
        for item in repository.list_all()
    ],
    "selectors": {},
}
for selector in json.loads(os.environ["CF42_SELECTORS"]):
    try:
        text = build_charter_context_include(root, selector, org_roots=org_roots)
    except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the answer
        payload["selectors"][selector] = {
            "error": "%s: %s" % (type(exc).__name__, str(exc).splitlines()[0])
        }
    else:
        payload["selectors"][selector] = {"text": text}
print("CF42_SOURCE_RECEIPT " + json.dumps(payload))
"#;

fn run_source(root: &Path, selectors: &[&str]) -> serde_json::Value {
    let output = child(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env(
            "CF42_SELECTORS",
            serde_json::to_string(selectors).expect("selectors JSON"),
        )
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
    assert_eq!(receipt["runner"].as_str(), Some("source-python-3.11.15"));
    assert_eq!(
        receipt["cwd"].as_str(),
        Some(root.display().to_string().as_str())
    );
    receipt
}

fn native_receipt_json(root: &Path, selectors: &[&str]) -> serde_json::Value {
    let access = IncludeAccess::with_roots(
        root,
        OfferingRoots {
            built_in: Some(isolated::repository_root().join("packs/built-in")),
            org_roots: vec![root.join("packs/acme-org"), root.join("packs/beta-org")],
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
            Err(error) => serde_json::json!({ "error": error.to_string() }),
        };
        rendered.insert((*selector).to_owned(), value);
    }
    serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "selectors": rendered,
    })
}

fn run_native(root: &Path, selectors: &[&str], test_name: &str) -> serde_json::Value {
    let executable = std::env::current_exe().expect("test binary");
    let output = child(root, executable)
        .args(["--exact", test_name, "--nocapture"])
        .env("SK_E4_NATIVE_ROOT", root)
        .env(
            "CF42_SELECTORS",
            serde_json::to_string(selectors).expect("selectors JSON"),
        )
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

fn text_of(receipt: &serde_json::Value, selector: &str) -> String {
    receipt["selectors"]
        .get(selector)
        .and_then(|entry| entry.get("text"))
        .and_then(|text| text.as_str())
        .unwrap_or_else(|| panic!("{selector} must render: {receipt}"))
        .to_owned()
}

#[test]
fn the_layer_order_and_collision_contract_matches_the_source() {
    if let Some(root) = isolated::native_root() {
        assert_child_roots(&root);
        let raw = std::env::var("CF42_SELECTORS").expect("CF42_SELECTORS");
        let selectors: Vec<String> = serde_json::from_str(&raw).expect("selectors JSON");
        let refs: Vec<&str> = selectors.iter().map(String::as_str).collect();
        println!("CF42_NATIVE_RECEIPT {}", native_receipt_json(&root, &refs));
        return;
    }
    let fixture = Fixture::new();
    let selectors: Vec<&str> = SELECTORS.to_vec();
    let source = run_source(&fixture.root, &selectors);

    // Source truth: the later org pack owns the twice-declared new id, and the
    // same-id built-in override resolves to the later org pack's title while the
    // built-in intent survives (the org `intent` it does not repeat is discarded).
    let keys: Vec<(String, String)> = source["keys"]
        .as_array()
        .expect("keys")
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
            .any(|(key, layer)| key == "ACME_910" && layer == "org")
            && keys
                .iter()
                .any(|(key, layer)| key == "ACME_911" && layer == "org"),
        "each twice-declared id is one org record: {keys:?}"
    );
    assert!(
        keys.iter()
            .any(|(key, layer)| key == "DIRECTIVE_024" && layer == "org"),
        "{keys:?}"
    );
    // A NEW id is validated as authored: the later pack's partial document does not
    // participate in a field merge, so the earlier complete record is what is served.
    for selector in ["directive:ACME_910", "directive:910-shared"] {
        let shared = text_of(&source, selector);
        assert!(
            shared.starts_with("Directive ACME_910: Acme Shared Title\n")
                && shared.contains("\"intent\": \"Acme shared intent.\""),
            "{selector}: an inserted id is validated as authored, not merged: {shared}"
        );
        assert!(
            !shared.contains("Beta Shared Title"),
            "{selector}: {shared}"
        );
    }
    let mirror = text_of(&source, "directive:ACME_911");
    assert!(
        mirror.starts_with("Directive ACME_911: Beta Complete Title\n")
            && !mirror.contains("Acme Partial Title"),
        "the earlier partial insert is skipped and the later complete one is served: {mirror}"
    );
    let doubled = text_of(&source, "directive:024-locality-of-change");
    assert!(
        doubled.starts_with("Directive DIRECTIVE_024: Beta Locality Title\n"),
        "{doubled}"
    );
    assert!(
        doubled.contains("\"intent\": \"Changes should stay close to the problem being solved"),
        "the project/org merge starts from the built-in base, so the built-in intent \
         survives and the acme intent does not: {doubled}"
    );
    assert!(
        !doubled.contains("Acme locality intent."),
        "the org override is not composed into the later org layer: {doubled}"
    );
    let project = text_of(&source, "directive:010-specification-fidelity-requirement");
    assert!(
        project.starts_with("Directive DIRECTIVE_010: Project Fidelity Title\n")
            && project.contains("\"intent\": \"Project-authored intent override.\"")
            && project.contains("\"scope\": \"Applies to planning outputs"),
        "{project}"
    );

    let native = run_native(
        &fixture.root,
        &selectors,
        "the_layer_order_and_collision_contract_matches_the_source",
    );
    for selector in selectors {
        let source_text = text_of(&source, selector);
        assert_eq!(
            text_of(&native, selector),
            source_text,
            "{selector}: the layered record must be byte-identical"
        );
    }
}
