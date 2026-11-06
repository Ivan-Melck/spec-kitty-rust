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
//! The paired test runs BOTH sides as actual re-executed children on independent
//! fresh fixtures. Each child asserts its own CWD, HOME, all seven XDG roots,
//! TMPDIR/TMP/TEMP, the absolute repository `src` on `PYTHONPATH`, the absolute
//! `packs` root and the pinned hash seed BEFORE any business read; the Source child
//! is the lock-exact pinned CPython 3.11.15. The fixture write is per side, never
//! shared and never reset in place.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::path::Path;

use sk_doctrine::context_include::{
    IncludeAccess, IncludeInputs, parse_include_selector, render_include, render_include_exact,
};

/// The selector both sides render.
const SELECTOR: &str = "skill:fixture-skill";
/// The body file the record declares and the post-validation reads.
const BODY: &str = "Fixture skill body.\n";
/// The pin for `PYTHONHASHSEED` handed to both children.
const HASH_SEED: &str = "0";

/// The two XDG roots outside [`isolated::ROOTS`] (which carries HOME, the five
/// `*_HOME`/`RUNTIME_DIR` roots and the three TMP names).
const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// Write one independent, Source-valid project-tier pack skill plus its sibling body.
fn write_fixture(root: &Path) {
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
    std::fs::write(skills.join("fixture-skill.skill.md"), BODY).expect("skill body");
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra XDG root");
    }
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
    assert_eq!(error.code(), None, "the Source miss carries no machine code");
}

// ---------------------------------------------------------------------------
// Paired Source/Native proof (both actual isolated children, independent fixtures)
// ---------------------------------------------------------------------------

const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root, (str(pathlib.Path.cwd()), str(root))
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["PYTHONPATH"]).parent
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
assert os.environ["PYTHONHASHSEED"] == "0", os.environ["PYTHONHASHSEED"]
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
from charter.activation.context import build_charter_context_include
payload = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "selector": os.environ["SK_CF87_SELECTOR"],
    "ok": True,
    "text": None,
    "error": None,
}
try:
    text = build_charter_context_include(root, os.environ["SK_CF87_SELECTOR"])
except BaseException as error:  # noqa: BLE001 - the reference's own answer is the result
    payload["ok"] = False
    payload["error"] = "%s: %s" % (type(error).__name__, error)
else:
    payload["text"] = [ord(character) for character in text]
print("CF87_SOURCE " + json.dumps(payload, sort_keys=True))
"#;

/// The Native child: assert the same isolation, then render the real production route.
#[test]
fn cf87_native_child_receipt() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    isolated::assert_isolated(&root);
    for (key, name) in EXTRA_XDG_ROOTS {
        assert_eq!(
            std::env::var_os(key).map(std::path::PathBuf::from),
            Some(root.join(name)),
            "{key} is asserted inside the actual Native child"
        );
    }
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok(HASH_SEED),
        "the Native child runs under the pinned hash seed"
    );
    let raw = std::env::var("SK_CF87_SELECTOR").expect("selector from the parent");
    let selector = parse_include_selector(&raw).expect("selector parses");
    let access = IncludeAccess::load(&root).expect("include access");
    let inputs = IncludeInputs {
        project_root: &root,
        access: &access,
    };
    let mut payload = serde_json::json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "selector": raw,
        "ok": true,
        "text": null,
        "error": null,
    });
    match render_include_exact(&inputs, &selector, &raw) {
        Ok(text) => {
            payload["text"] =
                serde_json::json!(text.codepoints().iter().copied().collect::<Vec<u32>>());
        }
        Err(error) => {
            payload["ok"] = serde_json::json!(false);
            payload["error"] = serde_json::json!(error.to_string());
        }
    }
    println!("CF87_NATIVE {payload}");
}

#[test]
fn source_and_native_deliver_the_same_skill_include_from_independent_fixtures() {
    // Two independent fixtures: neither child reads the other side's tree, and
    // neither is reset/shared to make the inputs look equal.
    let source_root = isolated::fixture("cf87-source-child");
    write_fixture(&source_root);
    let source_stdout = isolated::source_script(
        &source_root,
        SOURCE_SCRIPT,
        &[
            ("SK_CF87_SELECTOR", SELECTOR),
            ("PYTHONHASHSEED", HASH_SEED),
            (
                "XDG_CONFIG_DIRS",
                source_root.join("config-dirs").to_str().expect("path"),
            ),
            (
                "XDG_DATA_DIRS",
                source_root.join("data-dirs").to_str().expect("path"),
            ),
        ],
    );
    let source = isolated::receipt_json(&source_stdout, "CF87_SOURCE ");
    assert_eq!(source["runner"], "source-python-3.11.15");
    assert_eq!(source["cwd"], source_root.display().to_string());
    assert_eq!(
        source["ok"],
        serde_json::json!(true),
        "the Source reference delivers the skill: {source}"
    );

    let native_root = isolated::fixture("cf87-native-child");
    write_fixture(&native_root);
    let native_stdout = isolated::native_reexec(
        &native_root,
        "cf87_native_child_receipt",
        &[
            ("SK_CF87_SELECTOR", SELECTOR),
            ("PYTHONHASHSEED", HASH_SEED),
            (
                "XDG_CONFIG_DIRS",
                native_root.join("config-dirs").to_str().expect("path"),
            ),
            (
                "XDG_DATA_DIRS",
                native_root.join("data-dirs").to_str().expect("path"),
            ),
        ],
    );
    let native = isolated::receipt_json(&native_stdout, "CF87_NATIVE ");
    assert_eq!(native["runner"], "native");
    assert_eq!(native["cwd"], native_root.display().to_string());
    assert_eq!(
        native["ok"],
        serde_json::json!(true),
        "the Native include delivers the skill: {native}"
    );
    assert_eq!(
        native["text"], source["text"],
        "the Native include text is the Source text codepoint for codepoint"
    );

    // The compared text really is the delivered skill body/model phase, not an
    // identical failure or an empty line.
    let points = source["text"].as_array().expect("code-point array");
    let text: String = points
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
}
