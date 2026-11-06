//! CF43 — exact Source include delivery over the merged record: paired proof.
//!
//! CF38 delivered the exact payload (`canonical_artifact_body_exact`) and the exact
//! `Full artifact:` renderer. This file proves the **consumer** wiring: the
//! non-template `charter context --include` path now serves its heading, its inline
//! body and its payload from that one exact payload, over the **merged** record
//! (built-in fields inherited, project-overlay fields replaced), and the result is
//! identical to the actual pinned Source child's rendered text.
//!
//! Coverage:
//!
//! 1. `glossary-pack:spec-kitty-core` — a real built-in pack with a project overlay
//!    that replaces `description` (a lone surrogate) and `terms` (typed `.nan`,
//!    `.inf`, `-.inf` and non-ASCII text) while inheriting `provenance`;
//! 2. `styleguide:common-docs` — a real built-in styleguide with an overlay that
//!    replaces `title` (surrogate) and `structural_lint_config` (an arbitrary-width
//!    integer, constructed `!!pairs`/`!!omap`/`!!binary`, array nulls, a dropped
//!    mapping null, non-finite floats inside `Any`, non-ASCII text) while
//!    inheriting `scope`, `principles` and `applies_to_languages`;
//! 3. `agent-profile:implementer-ivan` — the model-dump payload with no `Field`
//!    schema in this crate (`ProfileBody::model_json_exact`).
//!
//! Both children are actual processes: the Source child is the pinned CPython
//! 3.11.15 oracle and the Native child is this test binary re-executed. Each asserts
//! its own CWD, HOME, all seven XDG roots (including `XDG_CONFIG_DIRS` and
//! `XDG_DATA_DIRS`), TMPDIR/TMP/TEMP, the absolute `PYTHONPATH`, the pinned packs
//! root and `PYTHONHASHSEED=0`. Text travels as code-point arrays, so a lone
//! surrogate survives the transport exactly.
//!
//! Not a whole-command or CLI acceptance: the `--json` envelope and the text-mode
//! encoding phase are proved separately in `crates/sk-cli/tests/cf43_exact_include_cli.rs`.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value as JsonValue, json};
use sk_doctrine::context_include::{
    IncludeAccess, IncludeInputs, parse_include_selector, render_include_exact,
};

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

const SELECTORS: [&str; 3] = [
    "glossary-pack:spec-kitty-core",
    "styleguide:common-docs",
    "agent-profile:implementer-ivan",
];

/// A project overlay for a real built-in glossary pack.
///
/// `description` (overridden) carries a lone surrogate; `terms` (overridden) carry
/// the typed non-finite floats and non-ASCII text. `provenance` is inherited.
const GLOSSARY_OVERLAY: &str = "\
id: spec-kitty-core
description: \"Overlay description \\ud800.\"
terms:
  - surface: nan-term
    definition: \"h\\u00e9llo \\u2603 nan\"
    confidence: .nan
    status: active
  - surface: posinf-term
    definition: positive infinity
    confidence: .inf
    status: active
  - surface: neginf-term
    definition: negative infinity
    confidence: -.inf
    status: active
  - surface: bigfloat-term
    definition: large finite float
    confidence: 1.2345678901234568e+300
    status: active
";

/// A project overlay for a real built-in styleguide.
///
/// `title` (overridden) carries a lone surrogate; `structural_lint_config`
/// (overridden) carries the unstructured domain: an arbitrary-width integer,
/// constructed `!!pairs`/`!!omap`/`!!binary`, array nulls, a dropped mapping null,
/// non-finite floats (which pydantic's JSON projection turns into `null`) and
/// non-ASCII text. `scope`/`principles`/`applies_to_languages` are inherited.
const STYLEGUIDE_OVERLAY: &str = "\
id: common-docs
title: \"Overlay styleguide \\ud800 title\"
structural_lint_config:
  big_integer: 123456789012345678901234567890
  nonfinite: [.nan, .inf, -.inf]
  list_nulls: [1, null, 2]
  map_nulls: {kept: 1, dropped: null}
  pairs_value: !!pairs [{a: b}, {a: c}]
  omap_value: !!omap [{x: 1}, {y: 2}]
  bytes_value: !!binary SGVsbG8=
  unicode: \"h\\u00e9llo \\u2603\"
";

/// The reference renderer, over the real merged repositories.
const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["PYTHONPATH"]).parent
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
assert pathlib.Path(os.environ["XDG_CONFIG_DIRS"]) == root / "config-dirs"
assert pathlib.Path(os.environ["XDG_DATA_DIRS"]) == root / "data-dirs"
assert os.environ["PYTHONHASHSEED"] == "0", os.environ["PYTHONHASHSEED"]

from charter.activation.context import build_charter_context_include

payload = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "texts": {},
    "errors": {},
}
for selector in json.loads(os.environ["SK_CF43_SELECTORS"]):
    try:
        text = build_charter_context_include(root, selector)
    except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
        payload["errors"][selector] = "%s: %s" % (type(exc).__name__, exc)
    else:
        payload["texts"][selector] = [ord(char) for char in text]
print("CF43_RECEIPT " + json.dumps(payload))
"#;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

/// A project fixture: an authored config plus the two project-layer overlays that
/// shadow real built-in records.
fn fixture() -> PathBuf {
    let root = isolated::fixture("cf43-exact-include-delivery");
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra isolated XDG directory");
    }
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\n",
    );
    write(
        &root.join(".kittify/charter/charter.md"),
        "# Fixture Charter\n",
    );
    write(
        &root.join(".kittify/charter-packs/glossary_packs/spec-kitty-core.glossary-pack.yaml"),
        GLOSSARY_OVERLAY,
    );
    write(
        &root.join(".kittify/charter-packs/styleguide/common-docs.styleguide.yaml"),
        STYLEGUIDE_OVERLAY,
    );
    root
}

/// Every authored byte below `.kittify`, for the no-write assertion.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(directory: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&root.join(".kittify"), &mut out);
    out
}

fn parse_receipt(stdout: &str) -> (String, String, BTreeMap<String, Vec<u32>>) {
    let marker = "CF43_RECEIPT ";
    let index = stdout
        .find(marker)
        .unwrap_or_else(|| panic!("no receipt in {stdout:?}"));
    let line = stdout[index + marker.len()..]
        .lines()
        .next()
        .expect("receipt line");
    let value: JsonValue = serde_json::from_str(line.trim()).expect("receipt JSON");
    let errors = value["errors"].as_object().expect("errors object");
    assert!(errors.is_empty(), "a child refused a selector: {errors:?}");
    let texts = value["texts"]
        .as_object()
        .expect("texts object")
        .iter()
        .map(|(selector, points)| {
            (
                selector.clone(),
                points
                    .as_array()
                    .expect("code-point array")
                    .iter()
                    .map(|point| u32::try_from(point.as_u64().expect("code point")).expect("u32"))
                    .collect(),
            )
        })
        .collect();
    (
        value["runner"].as_str().expect("runner").to_owned(),
        value["cwd"].as_str().expect("cwd").to_owned(),
        texts,
    )
}

/// Split one exact text into its `"\n"`-separated lines (code points).
fn lines_of(text: &[u32]) -> Vec<Vec<u32>> {
    let mut lines = vec![Vec::new()];
    for point in text {
        if *point == u32::from('\n') {
            lines.push(Vec::new());
        } else {
            lines.last_mut().expect("line").push(*point);
        }
    }
    lines
}

fn scalars(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

/// The Native child: render every selector with the exact route and report its
/// exact code points.
#[test]
fn cf43_native_child_and_receipt() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    isolated::assert_isolated(&root);
    for (key, name) in EXTRA_XDG_ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is asserted inside the actual Native child"
        );
    }
    assert_eq!(
        std::env::var("PYTHONHASHSEED").as_deref(),
        Ok("0"),
        "the attribution is pinned in the actual Native child"
    );
    let selectors: Vec<String> =
        serde_json::from_str(&std::env::var("SK_CF43_SELECTORS").expect("selectors env"))
            .expect("selectors JSON");
    let access = IncludeAccess::load(&root).expect("include access");
    let inputs = IncludeInputs {
        project_root: &root,
        access: &access,
    };
    let mut texts = serde_json::Map::new();
    for selector in selectors {
        let parsed = parse_include_selector(&selector).expect("selector parses");
        let text = render_include_exact(&inputs, &parsed, &selector).expect("selector renders");
        texts.insert(selector, json!(text.codepoints()));
    }
    println!(
        "CF43_RECEIPT {}",
        json!({
            "runner": "native",
            "cwd": std::env::current_dir().expect("cwd").display().to_string(),
            "texts": texts,
            "errors": {},
        })
    );
}

/// The paired proof: Source renders first, then the Native child must match it
/// exactly, over inherited **and** overridden fields, with the authored code points
/// intact.
#[test]
fn merged_inherited_and_overridden_fields_match_the_source_child() {
    let root = fixture();
    let before = snapshot(&root);
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let selectors = serde_json::to_string(&SELECTORS).expect("selectors JSON");

    let source_stdout = isolated::source_script(
        &root,
        SOURCE_SCRIPT,
        &[
            ("SK_CF43_SELECTORS", selectors.as_str()),
            ("PYTHONHASHSEED", "0"),
            (
                "XDG_CONFIG_DIRS",
                config_dirs.to_str().expect("config dirs"),
            ),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("data dirs")),
        ],
    );
    let (source_runner, source_cwd, source_texts) = parse_receipt(&source_stdout);
    assert_eq!(source_runner, "source-python-3.11.15");
    assert_eq!(source_cwd, root.display().to_string());
    let mut rendered = source_texts.keys().cloned().collect::<Vec<String>>();
    rendered.sort();
    let mut expected = SELECTORS.map(str::to_owned).to_vec();
    expected.sort();
    assert_eq!(
        rendered, expected,
        "the Source child rendered every selector"
    );

    // The intended Source cause is asserted BEFORE the Native child runs.
    let glossary = lines_of(
        source_texts
            .get("glossary-pack:spec-kitty-core")
            .expect("glossary text"),
    );
    assert!(
        glossary
            .iter()
            .any(|line| line == &scalars("            \"confidence\": NaN,")),
        "the reference keeps the typed NaN: {glossary:?}"
    );
    assert!(
        glossary
            .iter()
            .any(|line| line == &scalars("            \"confidence\": Infinity,")),
        "the reference keeps typed +Infinity"
    );
    assert!(
        glossary
            .iter()
            .any(|line| line == &scalars("            \"confidence\": -Infinity,")),
        "the reference keeps typed -Infinity"
    );
    assert!(
        glossary
            .iter()
            .any(|line| line == &scalars("        \"provenance\": \"built-in\",")),
        "the inherited field is present in the payload"
    );
    // The overridden description's lone surrogate is authored text, not an escape.
    assert!(
        glossary.iter().any(|line| {
            line.starts_with(&scalars("  Overlay description ")) && line.contains(&0xD800)
        }),
        "the reference printed the authored lone surrogate: {glossary:?}"
    );

    let styleguide = lines_of(
        source_texts
            .get("styleguide:common-docs")
            .expect("styleguide text"),
    );
    assert!(
        styleguide.iter().any(|line| {
            line == &scalars("          \"big_integer\": 123456789012345678901234567890,")
        }),
        "the arbitrary-width integer is preserved"
    );
    assert!(
        styleguide
            .iter()
            .any(|line| line == &scalars("          \"list_nulls\": [")),
        "array nulls are kept"
    );
    assert!(
        styleguide
            .iter()
            .any(|line| line == &scalars("          \"map_nulls\": {")),
        "the mapping keeps its non-null member"
    );
    assert!(
        styleguide
            .iter()
            .any(|line| line == &scalars("          \"bytes_value\": \"Hello\",")),
        "`!!binary` decodes to its UTF-8 text"
    );

    let native_stdout = isolated::native_reexec(
        &root,
        "cf43_native_child_and_receipt",
        &[
            ("SK_CF43_SELECTORS", selectors.as_str()),
            ("PYTHONHASHSEED", "0"),
            (
                "XDG_CONFIG_DIRS",
                config_dirs.to_str().expect("config dirs"),
            ),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("data dirs")),
        ],
    );
    let (native_runner, native_cwd, native_texts) = parse_receipt(&native_stdout);
    assert_eq!(native_runner, "native");
    assert_eq!(native_cwd, root.display().to_string());
    assert_eq!(
        native_texts.keys().cloned().collect::<Vec<String>>(),
        source_texts.keys().cloned().collect::<Vec<String>>(),
        "both children rendered the same selectors"
    );
    for (selector, source_text) in &source_texts {
        let native_text = native_texts
            .get(selector)
            .unwrap_or_else(|| panic!("native receipt for {selector}"));
        assert_eq!(
            native_text, source_text,
            "{selector}: the exact rendered text (code points)"
        );
    }

    // The read is a pure projection: no authored byte changed.
    assert_eq!(before, snapshot(&root), "the include path wrote no file");
}

/// The old finite entry points are explicit adapters over the ONE exact formatter:
/// for every finite document they must produce the same text as before.
#[test]
fn the_finite_inline_and_payload_adapters_reproduce_the_exact_formatter() {
    use sk_charter::ArtifactKind;
    use sk_doctrine::canonical_artifact_body;
    use sk_doctrine::canonical_artifact_body_exact_path;
    use sk_doctrine::context_render::{
        include_body_lines, include_body_lines_exact, include_full_payload_lines,
        include_full_payload_lines_exact,
    };

    let repository = isolated::repository_root();
    let documents = [
        (
            ArtifactKind::Styleguide,
            "packs/built-in/styleguides/common-docs.styleguide.yaml",
        ),
        (
            ArtifactKind::GlossaryPack,
            "packs/built-in/glossary_packs/spec-kitty-core.glossary-pack.yaml",
        ),
        (
            ArtifactKind::Directive,
            "packs/built-in/directives/010-specification-fidelity-requirement.directive.yaml",
        ),
        (
            ArtifactKind::Tactic,
            "packs/built-in/tactics/architecture/dependency-hygiene.tactic.yaml",
        ),
    ];
    let mut compared = 0;
    for (kind, relative) in documents {
        let path = repository.join(relative);
        let payload = canonical_artifact_body_exact_path(kind, &path).expect("exact payload");
        let exact_inline = include_body_lines_exact(kind.canonical(), &payload)
            .into_iter()
            .map(|line| {
                line.to_scalar_string()
                    .expect("the finite document is scalar")
            })
            .collect::<Vec<String>>();
        assert!(!exact_inline.is_empty(), "{relative}: inline body");
        // The finite adapter reads the same declared document tree.
        let text = std::fs::read_to_string(&path).expect("document read");
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).expect("document parses");
        if let Some(adapter) = canonical_artifact_body(kind, &value) {
            assert_eq!(
                include_body_lines(kind.canonical(), &adapter),
                exact_inline,
                "{relative}: the finite inline adapter matches the exact formatter"
            );
            assert_eq!(
                include_full_payload_lines(&adapter),
                include_full_payload_lines_exact(&payload),
                "{relative}: the finite payload adapter matches the exact renderer"
            );
            compared += 1;
        }
    }
    assert!(compared >= 4, "compared {compared} finite documents");
}
