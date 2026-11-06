//! CF87 — a Source-valid pack skill is DELIVERED by `charter context --include skill:<id>`.
//!
//! Source policy (`src/charter/activation/context.py::build_charter_context_include`):
//! the include builder wraps only `agent-profile` and `glossary-pack` in the
//! activation-aware service; every other document kind — `skill` included — is
//! rendered from the PLAIN `CharterOfferingService`, whose `skills` property loads
//! the canonical pack-skill repository. The effective *deny* annotation is decided
//! before the dispatch, so a deactivated skill is annotated, never gated by the
//! required-only `activated_skills` floor.
//!
//! The Native dispatch used to pass the activation gate for every non-directive
//! kind. `skill` has the required-only absent-key default (the org
//! `required_skills` floor), so a Source-valid authored skill became the
//! structured miss `No skill found for selector 'skill:<id>'.`. These tests drive
//! the real `sk_doctrine::context_include` production route, not a model struct.
//!
//! The paired plan runs BOTH sides as actual re-executed children on independent
//! fresh fixtures. Each child asserts, BEFORE any business read, its own CWD, HOME,
//! all seven XDG roots, TMPDIR/TMP/TEMP, an INDEPENDENTLY supplied absolute
//! `SK_CF87_EXPECTED_SOURCE` (never derived from `PYTHONPATH`), `PYTHONPATH` and
//! packs under it, the pinned hash seed, the Source/checkout origin (`git
//! rev-parse HEAD` of that root), the lock-exact oracle's 53 requirements, the
//! interpreted filesystem/stdout/stderr codecs and (Source) `charter`'s real
//! module origin. The Source child is the pinned CPython 3.11.15; the Native child
//! is this test binary. The four outcomes are one plan, not four pairs: served
//! (body/model text), true miss, deny annotation and the body-read skip. The
//! fixture write is per side, never shared or reset in place, and both business
//! trees are snapshotted to prove the include wrote nothing.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_doctrine::context_include::{
    IncludeAccess, IncludeInputs, parse_include_selector, render_include, render_include_exact,
};

/// The selector both sides render.
const SELECTOR: &str = "skill:fixture-skill";
/// The body file the record declares and the post-validation reads.
const BODY: &str = "Fixture skill body.\n";
/// The pin for `PYTHONHASHSEED` handed to both children.
const HASH_SEED: &str = "0";
/// The deny annotation the reference produces for the fixture skill.
const DENIED: &str = "skill:fixture-skill is deactivated in this project \
                      (charter activate skill fixture-skill restores it)";
/// The reference's miss message shared by a genuine miss and a skipped record.
const MISS: &str = "No skill found for selector 'skill:fixture-skill'.";
/// The reference's miss message for an unknown id.
const MISS_UNKNOWN: &str = "No skill found for selector 'skill:no-such-skill'.";

/// The two XDG roots outside [`isolated::ROOTS`] (which carries HOME, the five
/// `*_HOME`/`RUNTIME_DIR` roots and the three TMP names).
const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The Source implementation files whose existence pins the expected root's origin.
const SOURCE_OWNERS: [&str; 2] = [
    "src/charter/activation/context.py",
    "src/charter/offering/pack_skills/repository.py",
];

/// One business case in the single paired plan.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct Case {
    name: String,
    root: String,
    selector: String,
}

/// The repository root, derived from the crate manifest (never from `PYTHONPATH`).
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root resolves")
}

/// `git rev-parse HEAD` of one root, parent-supplied to both children.
fn checkout_origin(root: &Path) -> String {
    let output = Command::new("git")
        .args([
            "-C",
            root.to_str().expect("utf-8 root"),
            "rev-parse",
            "HEAD",
        ])
        .output()
        .expect("git rev-parse runs");
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8 git stdout")
        .trim()
        .to_owned()
}

/// Write one independent, Source-valid project-tier pack skill plus its sibling body.
fn write_fixture(root: &Path) {
    write_fixture_inner(root, Some(BODY));
}

/// The same record without the sibling body (the post-validation read control).
fn write_fixture_without_body(root: &Path) {
    write_fixture_inner(root, None);
}

fn write_fixture_inner(root: &Path, body: Option<&str>) {
    let skills = root.join(".kittify/charter-packs/skills");
    std::fs::create_dir_all(&skills).expect("project skill dir");
    std::fs::create_dir_all(root.join(".kittify/charter")).expect("charter dir");
    std::fs::write(
        root.join(".kittify/config.yaml"),
        "# authored project config\n",
    )
    .expect("config");
    std::fs::write(
        skills.join("fixture-skill.skill.yaml"),
        "schema_version: '1.0'\n\
         id: fixture-skill\n\
         title: Fixture Skill\n\
         description: Fixture skill description.\n\
         form: prompt\n\
         triggers: [alpha trigger, beta trigger]\n\
         body_path: fixture-skill.skill.md\n",
    )
    .expect("skill record");
    if let Some(body) = body {
        std::fs::write(skills.join("fixture-skill.skill.md"), body).expect("skill body");
    }
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra XDG root");
    }
}

/// Deactivate the fixture skill through the canonical activation ledger.
fn deny_fixture_skill(root: &Path) {
    std::fs::write(
        root.join(".kittify/charter/activation.yaml"),
        "deactivated_skills:\n  - fixture-skill\n",
    )
    .expect("activation ledger");
}

/// The rendered exact text, asserted to be a scalar code-point sequence (the
/// fixture is ASCII; a non-scalar result would be a real finding, not a test
/// accommodation).
fn rendered(root: &Path) -> String {
    let access = IncludeAccess::load(root).expect("include access");
    let inputs = IncludeInputs {
        project_root: root,
        access: &access,
    };
    let selector = parse_include_selector(SELECTOR).expect("selector parses");
    render_include(&inputs, &selector, SELECTOR)
        .expect("a Source-valid pack skill is delivered, never omitted")
}

#[test]
fn a_source_valid_prompt_skill_is_served_by_the_plain_include_route() {
    let root = isolated::fixture("cf87-skill-include");
    write_fixture(&root);
    // Premise check: the Source post-validation phase requires this sibling body.
    let text = rendered(&root);
    assert!(
        text.starts_with("Skill fixture-skill: Fixture Skill\n"),
        "{text}"
    );
    assert!(
        text.contains("\n    Description: Fixture skill description.\n"),
        "{text}"
    );
    assert!(text.contains("\n    Form: prompt\n"), "{text}");
    assert!(
        text.contains("\n    Triggers: alpha trigger, beta trigger\n"),
        "{text}"
    );
    // The exact model dump (the reference's `Full artifact:` payload) still names
    // the authored body path; the include phase does not invent inline body text.
    assert!(
        text.contains("\"body_path\": \"fixture-skill.skill.md\""),
        "{text}"
    );
    assert!(text.contains("\"id\": \"fixture-skill\""), "{text}");
    assert!(text.contains("    Full artifact:\n      {\n"), "{text}");
}

#[test]
fn a_real_missing_skill_still_refuses_with_the_reference_message() {
    let root = isolated::fixture("cf87-skill-missing");
    write_fixture(&root);
    let access = IncludeAccess::load(&root).expect("include access");
    let inputs = IncludeInputs {
        project_root: &root,
        access: &access,
    };
    let selector = parse_include_selector("skill:no-such-skill").expect("selector parses");
    let error = render_include_exact(&inputs, &selector, "skill:no-such-skill")
        .expect_err("a miss is a miss");
    assert_eq!(
        error.to_string(),
        "No skill found for selector 'skill:no-such-skill'."
    );
    assert_eq!(
        error.code(),
        None,
        "the Source miss carries no machine code"
    );
}

#[test]
fn the_plain_skill_route_still_annotates_a_deactivated_record() {
    let root = isolated::fixture("cf87-skill-denied");
    write_fixture(&root);
    deny_fixture_skill(&root);
    assert_eq!(rendered(&root), DENIED);
}

// ---------------------------------------------------------------------------
// Paired Source/Native proof (both actual isolated children, independent fixtures)
// ---------------------------------------------------------------------------

const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, subprocess, sys

ROOT = pathlib.Path(os.environ["SK_E4_ROOT"])
EXPECTED = pathlib.Path(os.environ["SK_CF87_EXPECTED_SOURCE"])


def check(condition, message):
    if not condition:
        raise SystemExit(f"CF87-SOURCE-ENV-ASSERTION-FAILED: {message}")


check(ROOT.is_absolute() and ROOT.is_dir(), "business root")
check(EXPECTED.is_absolute() and EXPECTED.is_dir(), "expected source root")
for owner in [
    "src/charter/activation/context.py",
    "src/charter/offering/pack_skills/repository.py",
]:
    check((EXPECTED / owner).is_file(), "source owner " + owner)
check(pathlib.Path.cwd() == ROOT, "cwd")
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    check(pathlib.Path(os.environ[key]) == ROOT / name, key)
check(pathlib.Path(os.environ["PYTHONPATH"]) == EXPECTED / "src", "PYTHONPATH")
check(pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == EXPECTED / "packs", "packs")
check(os.environ["PYTHONHASHSEED"] == "0", "hash seed")
check(sys.version_info[:3] == (3, 11, 15), "interpreter")
check(
    (sys.getfilesystemencoding(), sys.getfilesystemencodeerrors())
    == ("utf-8", "surrogateescape"),
    "filesystem codec",
)
check((sys.stderr.encoding, sys.stderr.errors) == ("utf-8", "backslashreplace"), "stderr codec")
check(sys.stdout.encoding.lower().replace("_", "-") == "utf-8", "stdout codec")
check(sys.stdout.errors == "surrogateescape", "stdout errors")

import charter
check(pathlib.Path(charter.__file__).resolve().is_relative_to(EXPECTED / "src"), "charter origin")

head = subprocess.check_output(
    ["git", "-C", str(EXPECTED), "rev-parse", "HEAD"], text=True
).strip()
check(head == os.environ["SK_CF87_EXPECTED_ORIGIN"], "checkout origin")

verified = json.loads(
    subprocess.check_output(
        [
            os.environ["SK_CF87_ORACLE_PYTHON"],
            os.environ["SK_CF87_ORACLE_SCRIPT"],
            "--root",
            str(EXPECTED),
        ],
        text=True,
        cwd=str(EXPECTED),
    )
)
check(verified.get("ok") is True, "oracle ok")
check(verified.get("checked_runtime_requirements") == 53, "oracle exact53")
check(verified.get("python") == "3.11.15", "oracle interpreter")

from charter.activation.context import build_charter_context_include

results = {}
for case in json.loads(os.environ["SK_CF87_CASES"]):
    root = pathlib.Path(case["root"])
    try:
        text = build_charter_context_include(root, case["selector"])
    except BaseException as error:  # noqa: BLE001 - the reference's own answer is the result
        results[case["name"]] = {
            "kind": "error",
            "type": type(error).__name__,
            "message": str(error),
        }
    else:
        results[case["name"]] = {"kind": "text", "text": [ord(character) for character in text]}

print(
    "CF87_SOURCE "
    + json.dumps(
        {
            "runner": "source-python-3.11.15",
            "cwd": str(pathlib.Path.cwd()),
            "results": results,
        },
        sort_keys=True,
    )
)
"#;

/// Assert the Native child's full environment before it touches business bytes.
fn assert_native_guards(root: &Path) {
    isolated::assert_isolated(root);
    for (key, name) in EXTRA_XDG_ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is asserted inside the actual Native child"
        );
    }
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok(HASH_SEED),
        "the Native child runs under the pinned hash seed"
    );
    let expected = PathBuf::from(
        std::env::var_os("SK_CF87_EXPECTED_SOURCE").expect("expected source from the parent"),
    );
    let repository = repository_root();
    assert_eq!(
        expected, repository,
        "the expected Source root is this checkout, supplied independently of PYTHONPATH"
    );
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(expected.join("src")),
        "PYTHONPATH is under the expected Source root"
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(expected.join("packs")),
        "packs root is under the expected Source root"
    );
    for owner in SOURCE_OWNERS {
        assert!(
            expected.join(owner).is_file(),
            "expected Source root carries {owner}"
        );
    }
    assert_eq!(
        checkout_origin(&expected),
        std::env::var("SK_CF87_EXPECTED_ORIGIN").expect("origin from the parent"),
        "the expected Source root is the checkout the parent measured"
    );
    let python = PathBuf::from(
        std::env::var_os("SK_CF87_ORACLE_PYTHON").expect("oracle interpreter from the parent"),
    );
    let script = PathBuf::from(
        std::env::var_os("SK_CF87_ORACLE_SCRIPT").expect("oracle script from the parent"),
    );
    let output = Command::new(&python)
        .arg(&script)
        .arg("--root")
        .arg(&expected)
        .current_dir(&expected)
        .output()
        .expect("the pinned oracle verifier runs");
    assert!(
        output.status.success(),
        "oracle verify failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let verified: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("oracle verify JSON");
    assert_eq!(verified["ok"], serde_json::json!(true), "{verified}");
    assert_eq!(
        verified["checked_runtime_requirements"],
        serde_json::json!(53),
        "{verified}"
    );
    assert_eq!(
        verified["python"],
        serde_json::json!("3.11.15"),
        "{verified}"
    );
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    let stdout =
        sk_kernel::python_stream::StreamPolicy::from_environment().expect("stdout stream policy");
    assert_eq!(stdout.encoding(), "utf-8");
    assert_eq!(stdout.errors(), "surrogateescape");
    let stderr =
        sk_kernel::python_stream::StreamPolicy::for_stderr().expect("stderr stream policy");
    assert_eq!(stderr.encoding(), "utf-8");
    assert_eq!(stderr.errors(), "backslashreplace");
}

/// The Native child: assert the full environment, then render every planned case.
#[test]
fn cf87_native_child_receipt() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    assert_native_guards(&root);
    let cases: Vec<Case> =
        serde_json::from_str(&std::env::var("SK_CF87_CASES").expect("cases from the parent"))
            .expect("cases JSON");
    let mut results = serde_json::Map::new();
    for case in cases {
        let selector = parse_include_selector(&case.selector).expect("selector parses");
        let business = Path::new(&case.root);
        let access = IncludeAccess::load(business).expect("include access");
        let inputs = IncludeInputs {
            project_root: business,
            access: &access,
        };
        let value = match render_include_exact(&inputs, &selector, &case.selector) {
            Ok(text) => serde_json::json!({
                "kind": "text",
                "text": text.codepoints().to_vec(),
            }),
            Err(error) => serde_json::json!({
                "kind": "error",
                "type": "include",
                "message": error.to_string(),
                "code": error.code(),
            }),
        };
        results.insert(case.name.clone(), value);
    }
    let payload = serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "results": results,
    });
    println!("CF87_NATIVE {payload}");
}

/// Every path under `root` (directories and file bytes), for the no-write assertion.
/// The shared isolation helper's own receipt files are excluded: they are harness
/// artifacts written by `source_script`/`native_reexec`, not project bytes.
const HARNESS_RECEIPTS: [&str; 4] = [
    "source-script.stdout",
    "source-script.stderr",
    "native.stdout",
    "native.stderr",
];

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(base: &Path, directory: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                out.insert(relative, None);
                walk(base, &path, out);
            } else {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                if HARNESS_RECEIPTS.contains(&name) {
                    continue;
                }
                out.insert(relative, Some(std::fs::read(&path).unwrap_or_default()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn cases_json(cases: &[Case]) -> String {
    serde_json::to_string(cases).expect("cases JSON")
}

fn business_case(name: &str, root: &Path, selector: &str) -> Case {
    Case {
        name: name.to_owned(),
        root: root.display().to_string(),
        selector: selector.to_owned(),
    }
}

fn run_source_plan(root: &Path, cases: &[Case]) -> serde_json::Value {
    let expected = repository_root();
    let origin = checkout_origin(&expected);
    let oracle_python = expected.join(".venv-rust-oracle/bin/python");
    let oracle_script = expected.join("scripts/rust/verify_oracle.py");
    assert!(
        oracle_python.is_file(),
        "the pinned oracle interpreter exists"
    );
    assert!(oracle_script.is_file(), "the oracle verifier exists");
    let cases = cases_json(cases);
    let stdout = isolated::source_script(
        root,
        SOURCE_SCRIPT,
        &[
            ("PYTHONHASHSEED", HASH_SEED),
            ("SK_CF87_EXPECTED_SOURCE", expected.to_str().expect("path")),
            ("SK_CF87_EXPECTED_ORIGIN", &origin),
            (
                "SK_CF87_ORACLE_PYTHON",
                oracle_python.to_str().expect("path"),
            ),
            (
                "SK_CF87_ORACLE_SCRIPT",
                oracle_script.to_str().expect("path"),
            ),
            ("SK_CF87_CASES", &cases),
            (
                "XDG_CONFIG_DIRS",
                root.join("config-dirs").to_str().expect("path"),
            ),
            (
                "XDG_DATA_DIRS",
                root.join("data-dirs").to_str().expect("path"),
            ),
        ],
    );
    isolated::receipt_json(&stdout, "CF87_SOURCE ")
}

fn run_native_plan(root: &Path, cases: &[Case]) -> serde_json::Value {
    let expected = repository_root();
    let origin = checkout_origin(&expected);
    let oracle_python = expected.join(".venv-rust-oracle/bin/python");
    let oracle_script = expected.join("scripts/rust/verify_oracle.py");
    let cases = cases_json(cases);
    let stdout = isolated::native_reexec(
        root,
        "cf87_native_child_receipt",
        &[
            ("PYTHONHASHSEED", HASH_SEED),
            ("SK_CF87_EXPECTED_SOURCE", expected.to_str().expect("path")),
            ("SK_CF87_EXPECTED_ORIGIN", &origin),
            (
                "SK_CF87_ORACLE_PYTHON",
                oracle_python.to_str().expect("path"),
            ),
            (
                "SK_CF87_ORACLE_SCRIPT",
                oracle_script.to_str().expect("path"),
            ),
            ("SK_CF87_CASES", &cases),
            (
                "XDG_CONFIG_DIRS",
                root.join("config-dirs").to_str().expect("path"),
            ),
            (
                "XDG_DATA_DIRS",
                root.join("data-dirs").to_str().expect("path"),
            ),
        ],
    );
    isolated::receipt_json(&stdout, "CF87_NATIVE ")
}

#[test]
fn source_and_native_deliver_the_same_skill_include_from_independent_fixtures() {
    // Two independent fixture sets: neither child reads the other side's tree, and
    // neither is reset/shared to make the inputs look equal.
    let source_primary = isolated::fixture("cf87-source-served");
    let source_denied = isolated::fixture("cf87-source-denied");
    let source_no_body = isolated::fixture("cf87-source-no-body");
    write_fixture(&source_primary);
    write_fixture(&source_denied);
    deny_fixture_skill(&source_denied);
    write_fixture_without_body(&source_no_body);

    let source_cases = vec![
        business_case("served", &source_primary, SELECTOR),
        business_case("miss", &source_primary, "skill:no-such-skill"),
        business_case("denied", &source_denied, SELECTOR),
        business_case("no_body", &source_no_body, SELECTOR),
    ];
    let source_trees = [&source_primary, &source_denied, &source_no_body];
    let source_before: Vec<_> = source_trees.iter().map(|root| snapshot(root)).collect();
    let source = run_source_plan(&source_primary, &source_cases);
    let source_after: Vec<_> = source_trees.iter().map(|root| snapshot(root)).collect();
    assert_eq!(
        source_before, source_after,
        "the Source include wrote nothing"
    );
    assert_eq!(source["runner"], "source-python-3.11.15");
    assert_eq!(source["cwd"], source_primary.display().to_string());

    let native_primary = isolated::fixture("cf87-native-served");
    let native_denied = isolated::fixture("cf87-native-denied");
    let native_no_body = isolated::fixture("cf87-native-no-body");
    write_fixture(&native_primary);
    write_fixture(&native_denied);
    deny_fixture_skill(&native_denied);
    write_fixture_without_body(&native_no_body);

    let native_cases = vec![
        business_case("served", &native_primary, SELECTOR),
        business_case("miss", &native_primary, "skill:no-such-skill"),
        business_case("denied", &native_denied, SELECTOR),
        business_case("no_body", &native_no_body, SELECTOR),
    ];
    let native_trees = [&native_primary, &native_denied, &native_no_body];
    let native_before: Vec<_> = native_trees.iter().map(|root| snapshot(root)).collect();
    let native = run_native_plan(&native_primary, &native_cases);
    let native_after: Vec<_> = native_trees.iter().map(|root| snapshot(root)).collect();
    assert_eq!(
        native_before, native_after,
        "the Native include wrote nothing"
    );
    assert_eq!(native["runner"], "native");
    assert_eq!(native["cwd"], native_primary.display().to_string());

    for name in ["served", "miss", "denied", "no_body"] {
        let reference = &source["results"][name];
        let native = &native["results"][name];
        assert_eq!(native["kind"], reference["kind"], "{name}: outcome kind");
        match reference["kind"].as_str() {
            Some("text") => assert_eq!(native["text"], reference["text"], "{name}: text"),
            Some("error") => {
                assert_eq!(
                    native["message"], reference["message"],
                    "{name}: error message"
                );
                assert_eq!(
                    native["code"],
                    serde_json::Value::Null,
                    "{name}: no machine code"
                );
            }
            other => panic!("unexpected outcome kind for {name}: {other:?}"),
        }
    }

    // The compared text really is the delivered skill body/model phase, not an
    // identical failure or an empty line.
    let served = &source["results"]["served"]["text"];
    let text: String = served
        .as_array()
        .expect("code-point array")
        .iter()
        .map(|point| {
            char::from_u32(u32::try_from(point.as_u64().expect("code point")).expect("u32"))
                .expect("scalar fixture codepoint")
        })
        .collect();
    assert!(
        text.starts_with("Skill fixture-skill: Fixture Skill\n"),
        "{text}"
    );
    assert!(
        text.contains("\n    Description: Fixture skill description.\n"),
        "{text}"
    );
    assert!(
        text.contains("\"body_path\": \"fixture-skill.skill.md\""),
        "{text}"
    );
    assert_eq!(
        source["results"]["denied"]["text"]
            .as_array()
            .map(|points| {
                points
                    .iter()
                    .map(|point| {
                        char::from_u32(
                            u32::try_from(point.as_u64().expect("codepoint")).expect("u32"),
                        )
                        .expect("scalar")
                    })
                    .collect::<String>()
            }),
        Some(DENIED.to_owned())
    );
    assert_eq!(source["results"]["miss"]["message"], MISS_UNKNOWN);
    assert_eq!(source["results"]["no_body"]["message"], MISS);
}
