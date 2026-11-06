//! CF57 — the authored typed org scalar algebra and the exact governance payload,
//! proved against the pinned Source.
//!
//! CF55 delivered the canonical typed cross-pack fold but disclosed that the
//! typed model stored the reference's `str | bool` policy values as the *text*
//! `"True"`/`"False"` and that `schema_version` was a bounded `i64`. Both are
//! real data losses: `(field, true)` and `(field, "True")` collapsed to one
//! dedupe identity, and Source-valid versions (`'1_0'`, `'١٢'`, a 40-digit int)
//! were refused or truncated. This file proves the corrected model against the
//! actual reference:
//!
//! * the `str | bool` union's *admission* (`true`/`1`/`1.0` -> boolean,
//!   `'True'`/`'yes'` -> text, `2`/`2.0`/`1.5`/`null` -> invalid);
//! * the fold's identity and merge semantics (`(f, true)` and `(f, 'True')` are
//!   two entries; a repeated identical entry replaces in place; per-key
//!   replacement keeps the last pack's authored type; the lenient
//!   `schema_version` keeps the last truthy value, exactly, at any width);
//! * Python `int(str)` text and arbitrary width, including Unicode decimal
//!   digits and the interpreter's conversion ceiling (a text spelling above it is
//!   a schema refusal, an integer *literal* above it makes the reference's YAML
//!   load raise, i.e. the pack is unreadable);
//! * the live `org_charter` JSON block: an authored `true` stays a JSON boolean,
//!   `'True'` stays a JSON string, and a `required_directives` id carrying a lone
//!   surrogate is emitted instead of dropped.
//!
//! Both sides are actual processes. The Source child is the pinned CPython
//! 3.11.15 oracle selected and lock-exact-verified by `support::python`; the
//! Native child is this test binary re-executed. Each asserts its own CWD, HOME,
//! all seven XDG roots (including `XDG_CONFIG_DIRS`/`XDG_DATA_DIRS`),
//! TMPDIR/TMP/TEMP, the absolute `PYTHONPATH`, the pinned packs root and
//! `PYTHONHASHSEED=0` before answering, and every string travels as code points so
//! a lone surrogate survives the transport exactly.
//!
//! Not CLI acceptance, not a parity claim beyond the exercised fixtures, and not
//! approval of the reference's `pack_context`/`extends:` typed path (this lane
//! does not implement it — see `org_charter::fold`'s module docs).

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as JsonValue, json};
use sk_doctrine::{
    AuthoredScalar, DoctrineError, OrgCharterPolicy, fold_org_charter_policies,
    load_org_charter_policy, org_charter_block,
};

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The `str | bool` written-scalar matrix: `(case, YAML value token)`.
///
/// The token is spliced into both an `interview_defaults` value and a
/// `governance_policies[*].value`; the two fields share the reference's union.
const UNION_CASES: [(&str, &str); 20] = [
    ("bool_true", "true"),
    ("bool_false", "false"),
    ("int_one", "1"),
    ("int_zero", "0"),
    ("int_two", "2"),
    ("float_one", "1.0"),
    ("float_zero", "0.0"),
    ("float_fraction", "1.5"),
    ("float_two", "2.0"),
    ("text_true", "'True'"),
    ("text_lower", "'true'"),
    ("text_one", "'1'"),
    ("text_empty", "''"),
    ("null", "null"),
    ("text_yes", "yes"),
    ("big_int", "10000000000000000000000000000000000000000"),
    // pydantic's lax `str` accepts `bytes` (strict UTF-8), and the union tries
    // `str` before `bool`, so UTF-8 bytes are text.
    ("bytes_utf8", "!!binary aGk="),
    ("bytes_text_true", "!!binary VHJ1ZQ=="),
    ("bytes_empty", "!!binary ''"),
    ("bytes_non_utf8", "!!binary /w=="),
];

/// The 50-digit version used by the fold and the `schema_version` matrix.
const WIDE: &str = "12345678901234567890123456789012345678901234567890";

/// The `schema_version` spelling matrix: `(case, YAML value token)`.
fn schema_cases() -> Vec<(String, String)> {
    let huge_text = format!("'{}'", "1".repeat(4301));
    let huge_literal = "1".repeat(4301);
    vec![
        ("int_two".to_owned(), "2".to_owned()),
        ("text_two".to_owned(), "'2'".to_owned()),
        ("underscore".to_owned(), "' 1_0 '".to_owned()),
        ("plus".to_owned(), "'+2'".to_owned()),
        ("arabic".to_owned(), format!("'{}'", "\u{0661}\u{0662}")),
        ("fullwidth".to_owned(), format!("'{}'", "\u{ff11}\u{ff12}")),
        ("wide_literal".to_owned(), WIDE.to_owned()),
        ("wide_text".to_owned(), format!("'{WIDE}'")),
        ("zero".to_owned(), "0".to_owned()),
        ("text_zero".to_owned(), "'0'".to_owned()),
        ("neg_zero".to_owned(), "-0".to_owned()),
        ("bool".to_owned(), "true".to_owned()),
        ("float".to_owned(), "2.0".to_owned()),
        ("empty_text".to_owned(), "''".to_owned()),
        ("double_underscore".to_owned(), "'1__0'".to_owned()),
        ("leading_underscore".to_owned(), "'_1'".to_owned()),
        ("trailing_underscore".to_owned(), "'1_'".to_owned()),
        ("hex_text".to_owned(), "'0x10'".to_owned()),
        ("space_plus_underscore".to_owned(), "'  +1_0  '".to_owned()),
        ("huge_text_4301".to_owned(), huge_text),
        ("huge_literal_4301".to_owned(), huge_literal),
    ]
}

/// The reference receipt: every loader case, the typed fold and the JSON block.
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

from charter.activation.org_charter import load_org_charter_policy, load_org_charter_policies
from charter.activation.org_charter_loader import load_org_charter_json_block

cases = pathlib.Path(os.environ["CF57_CASES"])
project = pathlib.Path(os.environ["CF57_PROJECT"])
block_pack = pathlib.Path(os.environ["CF57_BLOCKPACK"])
union_names = json.loads(os.environ["CF57_UNION"])
schema_names = json.loads(os.environ["CF57_SCHEMA"])

def points(value):
    return [ord(char) for char in value]

def scalar(value):
    if isinstance(value, bool):
        return {"type": "bool", "value": value}
    return {"type": "str", "points": points(value)}

def loader_receipt(entry):
    try:
        policy = load_org_charter_policy(entry)
    except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
        return {"ok": False, "phase": "invalid", "error": type(exc).__name__}
    if policy is None:
        return {"ok": False, "phase": "none"}
    return {"ok": True,
            "interview": sorted([[points(k), scalar(v)] for k, v in policy.interview_defaults.items()]),
            "governance": [[points(gp.field), scalar(gp.value), points(gp.enforcement)]
                           for gp in policy.governance_policies],
            "schema_version": str(policy.schema_version)}

payload = {
    "runner": "source-python-3.11.15",
    "cwd": str(pathlib.Path.cwd()),
    "union": {},
    "schema": {},
    "fold": None,
    "fold_error": None,
    "block": None,
    "block_error": None,
}

for name in union_names:
    payload["union"][name] = {
        "interview": loader_receipt(cases / ("iv_" + name)),
        "governance": loader_receipt(cases / ("gp_" + name)),
    }
for name in schema_names:
    payload["schema"][name] = loader_receipt(cases / ("sv_" + name))

try:
    folded = load_org_charter_policies(project)
except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
    payload["fold_error"] = type(exc).__name__
else:
    payload["fold"] = {
        "schema_version": str(folded.schema_version),
        "interview_defaults": [[points(k), scalar(v)] for k, v in folded.interview_defaults.items()],
        "governance": [[points(gp.field), scalar(gp.value), points(gp.enforcement)]
                       for gp in folded.governance_policies],
        "required": [points(token) for token in folded.required_directives],
    }

try:
    block = load_org_charter_json_block([block_pack])
except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
    payload["block_error"] = type(exc).__name__
else:
    def encode(value):
        if isinstance(value, bool):
            return {"type": "bool", "value": value}
        if isinstance(value, str):
            return {"type": "str", "points": points(value)}
        if isinstance(value, list):
            return [encode(item) for item in value]
        if isinstance(value, dict):
            return [[points(key), encode(item)] for key, item in value.items()]
        return {"type": type(value).__name__, "repr": repr(value)}
    payload["block"] = encode(block)

print("CF57_RECEIPT " + json.dumps(payload))
"#;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
    fs::write(path, text).expect("write fixture");
}

/// A fixture root with the per-case packs, the fold project and the block project.
fn fixture(case: &str) -> PathBuf {
    let root = isolated::fixture(case);
    for (_, name) in EXTRA_XDG_ROOTS {
        fs::create_dir_all(root.join(name)).expect("extra isolated XDG directory");
    }
    let cases = root.join("cases");
    for (name, token) in UNION_CASES {
        write(
            &cases.join(format!("iv_{name}")).join("org-charter.yaml"),
            &format!("interview_defaults:\n  a: {token}\n"),
        );
        write(
            &cases.join(format!("gp_{name}")).join("org-charter.yaml"),
            &format!("governance_policies:\n  - {{field: f, value: {token}}}\n"),
        );
    }
    for (name, token) in schema_cases() {
        write(
            &cases.join(format!("sv_{name}")).join("org-charter.yaml"),
            &format!("schema_version: {token}\n"),
        );
    }

    // The fold project: two configured packs whose authored values disagree in
    // *type* (bool vs text) as well as in spelling.
    let project = root.join("project");
    fs::create_dir_all(&project).expect("project");
    let base = project.join("packs/base");
    write(&base.join("pack.yaml"), "pack_level: org\nname: base\n");
    write(
        &base.join("org-charter.yaml"),
        "schema_version: 0\n\
         interview_defaults: {k: true, keep: base}\n\
         governance_policies:\n  - {field: f, value: true}\n  - {field: g, value: 'True'}\n\
         required_directives: [D1, '  D2  ']\n",
    );
    let overlay = project.join("packs/overlay");
    write(
        &overlay.join("pack.yaml"),
        "pack_level: org\nname: overlay\n",
    );
    write(
        &overlay.join("org-charter.yaml"),
        &format!(
            "schema_version: {WIDE}\n\
             interview_defaults: {{k: 'True'}}\n\
             governance_policies:\n  - {{field: f, value: 'True'}}\n  - {{field: g, value: true}}\n\
             required_directives: [D2]\n"
        ),
    );
    write(
        &project.join(".kittify/config.yaml"),
        &format!(
            "charter_packs:\n  org:\n    packs:\n      - name: base\n        local_path: {}\n      - name: overlay\n        local_path: {}\n",
            base.display(),
            overlay.display()
        ),
    );

    // The block project: one pack with an authored boolean value, an authored
    // text `'True'` and a `required_directives` id holding a lone surrogate.
    let blocks = root.join("blocks");
    fs::create_dir_all(&blocks).expect("block project");
    let block_pack = blocks.join("packs/acme");
    write(
        &block_pack.join("pack.yaml"),
        "pack_level: org\nname: acme\n",
    );
    write(
        &block_pack.join("org-charter.yaml"),
        "org_name: Acme\n\
         governance_policies:\n  - {field: f, value: true}\n  - {field: g, value: 'True'}\n  - {field: h, value: false}\n\
         required_directives: [\"author\\ud800\", ok-id]\n",
    );
    write(
        &blocks.join(".kittify/config.yaml"),
        &format!(
            "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
            block_pack.display()
        ),
    );
    root
}

/// The case-name lists both children use.
fn union_names() -> Vec<&'static str> {
    UNION_CASES.iter().map(|(name, _)| *name).collect()
}

fn schema_names() -> Vec<String> {
    schema_cases().into_iter().map(|(name, _)| name).collect()
}

fn points(token: &str) -> Vec<u32> {
    token.chars().map(|character| character as u32).collect()
}

fn exact(token: &sk_kernel::PythonString) -> JsonValue {
    json!(token.codepoints())
}

fn scalar_json(value: &AuthoredScalar) -> JsonValue {
    match value {
        AuthoredScalar::Bool(flag) => json!({"type": "bool", "value": flag}),
        AuthoredScalar::Text(text) => json!({"type": "str", "points": text.codepoints()}),
    }
}

/// One `load_org_charter_policy` case, in the Source receipt's shape.
fn loader_receipt(entry: &Path) -> JsonValue {
    match load_org_charter_policy(entry) {
        Ok(Some(policy)) => {
            let interview: Vec<JsonValue> = policy
                .interview_defaults
                .iter()
                .map(|(key, value)| json!([exact(key), scalar_json(value)]))
                .collect();
            json!({
                "ok": true,
                "interview": interview,
                "governance": policy
                    .governance_policies
                    .iter()
                    .map(|policy| {
                        json!([
                            exact(&policy.field),
                            scalar_json(&policy.value),
                            exact(&policy.enforcement)
                        ])
                    })
                    .collect::<Vec<_>>(),
                "schema_version": policy.schema_version.as_decimal(),
            })
        }
        Ok(None) => json!({"ok": false, "phase": "none"}),
        Err(DoctrineError::InvalidOrgCharter { .. }) => {
            json!({"ok": false, "phase": "invalid", "error": "ValidationError"})
        }
        Err(error) => panic!("unexpected typed-loader error: {error}"),
    }
}

/// The Native side of the receipt, in an isolated re-exec of this binary.
fn native_receipt_json(root: &Path) -> String {
    let cases = root.join("cases");
    let project = root.join("project");
    let blocks = root.join("blocks");

    let mut union = serde_json::Map::new();
    for name in union_names() {
        union.insert(
            name.to_owned(),
            json!({
                "interview": loader_receipt(&cases.join(format!("iv_{name}"))),
                "governance": loader_receipt(&cases.join(format!("gp_{name}"))),
            }),
        );
    }
    let mut schema = serde_json::Map::new();
    for name in schema_names() {
        schema.insert(
            name.clone(),
            loader_receipt(&cases.join(format!("sv_{name}"))),
        );
    }

    let fold = match fold_org_charter_policies(&project) {
        Ok(fold) => json!({
            "schema_version": fold.schema_version().as_decimal(),
            "interview_defaults": fold
                .interview_defaults()
                .iter()
                .map(|(key, value)| json!([exact(key), scalar_json(value)]))
                .collect::<Vec<_>>(),
            "governance": fold
                .governance_policies()
                .iter()
                .map(|policy| {
                    json!([
                        exact(&policy.field),
                        scalar_json(&policy.value),
                        exact(&policy.enforcement)
                    ])
                })
                .collect::<Vec<_>>(),
            "required": fold
                .required_for(sk_charter::ArtifactKind::Directive)
                .iter()
                .map(exact)
                .collect::<Vec<_>>(),
        }),
        Err(error) => panic!("unexpected fold error: {error}"),
    };

    let block = org_charter_block(&blocks);
    let block = encode_block(&block.to_json_value());

    json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "union": union,
        "schema": schema,
        "fold": fold,
        "fold_error": JsonValue::Null,
        "block": block,
        "block_error": JsonValue::Null,
    })
    .to_string()
}

/// Encode the block payload in the Source receipt's `{type, points|value}` shape.
fn encode_block(value: &sk_kernel::python_json::PythonJsonValue) -> JsonValue {
    use sk_kernel::python_json::PythonJsonValue;
    match value {
        PythonJsonValue::Null => json!({"type": "NoneType", "repr": "None"}),
        PythonJsonValue::Bool(flag) => json!({"type": "bool", "value": flag}),
        PythonJsonValue::Integer(token) => json!({"type": "int", "points": points(token)}),
        PythonJsonValue::Number(token) => json!({"type": "int", "points": points(token)}),
        PythonJsonValue::Float(number) => json!({"type": "float", "repr": number}),
        PythonJsonValue::String(text) => json!({"type": "str", "points": text.codepoints()}),
        PythonJsonValue::Array(items) => JsonValue::Array(items.iter().map(encode_block).collect()),
        PythonJsonValue::Object(fields) => JsonValue::Array(
            fields
                .iter()
                .map(|(key, value)| json!([points(key), encode_block(value)]))
                .collect(),
        ),
        PythonJsonValue::ExactObject(object) => JsonValue::Array(
            object
                .entries()
                .iter()
                .map(|(key, value)| json!([exact(key), encode_block(value)]))
                .collect(),
        ),
    }
}

/// The Native child: assert isolation, then answer from the canonical APIs.
#[test]
fn cf57_native_child_and_receipt() {
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
    println!("CF57_RECEIPT {}", native_receipt_json(&root));
}

/// Run the Source child, assert the intended Source cause, then the Native child.
fn run_pair(root: &Path) -> (JsonValue, JsonValue) {
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let union = serde_json::to_string(&union_names()).expect("union JSON");
    let schema = serde_json::to_string(&schema_names()).expect("schema JSON");
    let env: Vec<(&str, String)> = vec![
        ("PYTHONHASHSEED", "0".to_owned()),
        (
            "XDG_CONFIG_DIRS",
            config_dirs.to_str().expect("config-dirs").to_owned(),
        ),
        (
            "XDG_DATA_DIRS",
            data_dirs.to_str().expect("data-dirs").to_owned(),
        ),
        (
            "CF57_CASES",
            root.join("cases").to_str().expect("cases").to_owned(),
        ),
        (
            "CF57_PROJECT",
            root.join("project").to_str().expect("project").to_owned(),
        ),
        (
            "CF57_BLOCKPACK",
            root.join("blocks/packs/acme")
                .to_str()
                .expect("block pack")
                .to_owned(),
        ),
        ("CF57_UNION", union),
        ("CF57_SCHEMA", schema),
    ];
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let source_stdout = isolated::source_script(root, SOURCE_SCRIPT, &env);
    let source = isolated::receipt_json(&source_stdout, "CF57_RECEIPT ");
    assert_eq!(
        source["runner"], "source-python-3.11.15",
        "the Source child is the pinned interpreter"
    );
    assert_eq!(source["cwd"], root.display().to_string());
    let native_stdout = isolated::native_reexec(root, "cf57_native_child_and_receipt", &env);
    let native = isolated::receipt_json(&native_stdout, "CF57_RECEIPT ");
    assert_eq!(native["runner"], "native");
    assert_eq!(
        native["cwd"], source["cwd"],
        "both children ran in the same fixture"
    );
    (source, native)
}

/// One `[key, value]` pair in the block receipt's encoded shape.
fn pair(key: &str, value: JsonValue) -> JsonValue {
    json!([points(key), value])
}

fn pstr(value: &str) -> JsonValue {
    json!({"type": "str", "points": points(value)})
}

#[test]
fn source_admission_of_the_str_or_bool_union_is_reproduced() {
    let root = fixture("cf57-union");
    let (source, native) = run_pair(&root);

    // The intended Source cause, asserted BEFORE trusting the Native answer.
    assert_eq!(
        source["union"]["bool_true"]["interview"],
        json!({
            "ok": true,
            "interview": [[points("a"), {"type": "bool", "value": true}]],
            "governance": [],
            "schema_version": "2",
        }),
        "an authored bool is a Python bool"
    );
    assert_eq!(
        source["union"]["bool_false"]["governance"]["governance"],
        json!([[points("f"), {"type": "bool", "value": false}, points("advisory")]])
    );
    for name in [
        "bool_true",
        "bool_false",
        "int_one",
        "int_zero",
        "float_one",
        "float_zero",
    ] {
        let entry = &source["union"][name]["interview"];
        assert_eq!(entry["ok"], true, "{name} is admitted");
        assert_eq!(
            entry["interview"][0][1]["type"], "bool",
            "{name} is admitted as a Python bool"
        );
    }
    for name in [
        "int_two",
        "float_fraction",
        "float_two",
        "null",
        "big_int",
        "bytes_non_utf8",
    ] {
        assert_eq!(
            source["union"][name]["interview"],
            json!({"ok": false, "phase": "invalid", "error": "ValidationError"}),
            "{name} is not a member of the reference union"
        );
    }
    for name in [
        "text_true",
        "text_lower",
        "text_one",
        "text_empty",
        "text_yes",
    ] {
        let entry = &source["union"][name]["interview"];
        assert_eq!(entry["ok"], true, "{name} is admitted as text");
        assert_eq!(
            entry["interview"][0][1]["type"], "str",
            "{name} stays a Python str"
        );
    }
    assert_eq!(
        source["union"]["text_true"]["governance"]["governance"][0][1],
        pstr("True"),
        "the text 'True' is not the boolean True"
    );
    assert_eq!(
        source["union"]["bytes_utf8"]["interview"]["interview"][0][1],
        pstr("hi"),
        "UTF-8 bytes are admitted as the exact text"
    );
    assert_eq!(native["union"], source["union"], "the Native union matrix");
}

#[test]
fn source_python_int_text_and_arbitrary_width_are_reproduced() {
    let root = fixture("cf57-schema");
    let (source, native) = run_pair(&root);

    // Spellings and widths the reference accepts, exactly.
    for (name, expected) in [
        ("int_two", "2"),
        ("text_two", "2"),
        ("underscore", "10"),
        ("plus", "2"),
        ("arabic", "12"),
        ("fullwidth", "12"),
        ("wide_literal", WIDE),
        ("wide_text", WIDE),
        ("zero", "0"),
        ("text_zero", "0"),
        ("neg_zero", "0"),
        ("space_plus_underscore", "10"),
    ] {
        assert_eq!(
            source["schema"][name]["schema_version"], expected,
            "the reference reads {name} as {expected}"
        );
    }
    // The interpreter's digit ceiling: a text spelling is a schema refusal, a
    // literal is an unreadable document (the YAML load raises).
    assert_eq!(
        source["schema"]["huge_text_4301"]["phase"], "invalid",
        "int() refuses a 4301-digit text argument"
    );
    assert_eq!(
        source["schema"]["huge_literal_4301"]["phase"], "none",
        "a 4301-digit literal makes the reference's YAML load fail"
    );
    // Non-integers stay refusals.
    for name in [
        "bool",
        "float",
        "empty_text",
        "double_underscore",
        "leading_underscore",
        "trailing_underscore",
        "hex_text",
    ] {
        assert_eq!(
            source["schema"][name]["phase"], "invalid",
            "{name} is not a Python int"
        );
    }
    assert_eq!(
        native["schema"], source["schema"],
        "the Native version matrix"
    );
}

#[test]
fn fold_keeps_authored_types_and_the_exact_wide_version() {
    let root = fixture("cf57-fold");
    let (source, native) = run_pair(&root);

    assert_eq!(
        source["fold"]["schema_version"], WIDE,
        "the lenient fold keeps the last truthy version on a 50-digit int"
    );
    assert_eq!(
        source["fold"]["interview_defaults"],
        json!([[points("k"), pstr("True")], [points("keep"), pstr("base")],]),
        "per-key replacement keeps the last pack's authored type and the surviving key"
    );
    assert_eq!(
        source["fold"]["governance"],
        json!([
            [points("f"), {"type": "bool", "value": true}, points("advisory")],
            [points("g"), pstr("True"), points("advisory")],
            [points("f"), pstr("True"), points("advisory")],
            [points("g"), {"type": "bool", "value": true}, points("advisory")],
        ]),
        "(f, true) and (f, 'True') are two identities; so are (g, 'True') and (g, true)"
    );
    assert_eq!(
        source["fold"]["required"],
        json!([points("D1"), points("D2")]),
        "the required union still strips and dedupes"
    );
    assert_eq!(native["fold"], source["fold"], "the Native typed fold");
}

#[test]
fn live_org_charter_block_carries_the_exact_authored_payload() {
    let root = fixture("cf57-block");
    let (source, native) = run_pair(&root);

    let expected = json!([
        pair("present", json!({"type": "bool", "value": true})),
        pair(
            "packs",
            json!([[
                pair("pack_name", pstr("Acme")),
                pair(
                    "governance_policies",
                    json!([
                        [
                            pair("field", pstr("f")),
                            pair("value", json!({"type": "bool", "value": true})),
                            pair("enforcement", pstr("advisory")),
                            pair("source", pstr("org")),
                        ],
                        [
                            pair("field", pstr("g")),
                            pair("value", pstr("True")),
                            pair("enforcement", pstr("advisory")),
                            pair("source", pstr("org")),
                        ],
                        [
                            pair("field", pstr("h")),
                            pair("value", json!({"type": "bool", "value": false})),
                            pair("enforcement", pstr("advisory")),
                            pair("source", pstr("org")),
                        ],
                    ])
                ),
                pair(
                    "required_directives",
                    json!([
                        {"type": "str", "points": [0x61, 0x75, 0x74, 0x68, 0x6f, 0x72, 0xd800]},
                        pstr("ok-id"),
                    ])
                ),
            ]])
        ),
    ]);
    assert_eq!(
        source["block"], expected,
        "the reference block carries the authored bool, the text 'True' and the \
         surrogate-bearing id"
    );
    assert_eq!(native["block"], source["block"], "the Native block payload");
}

/// Every file under `root`, as `(relative path, exact bytes)`.
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, directory: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(directory)
            .expect("read directory")
            .map(|entry| entry.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let relative = path
                .strip_prefix(root)
                .expect("path under root")
                .display()
                .to_string();
            if path.is_dir() {
                out.push((format!("{relative}/"), Vec::new()));
                walk(root, &path, out);
            } else {
                out.push((relative, fs::read(&path).expect("read file")));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn the_typed_fold_and_the_block_write_nothing() {
    // A structural control (no Source needed): the typed fold and the context
    // block are read-only, so the fixture tree is byte-identical afterwards.
    let root = fixture("cf57-read-only");
    let project = root.join("project");
    let blocks = root.join("blocks");
    let before = snapshot(&root);
    let fold = fold_org_charter_policies(&project).expect("fold");
    assert_eq!(fold.contributing_packs(), ["base", "overlay"]);
    let block = org_charter_block(&blocks);
    assert!(block.present);
    assert!(
        block
            .to_json_value()
            .render_python(None, true)
            .to_scalar_string()
            .is_ok(),
        "the exact payload renders through the canonical JSON encoder"
    );
    assert_eq!(
        snapshot(&root),
        before,
        "the fold and the block must not write, lock or rewrite any file"
    );
}

#[test]
fn an_authored_boolean_is_not_the_string_true_in_the_model() {
    // A model-level control that does not need the Source: the two values have
    // different types and are not equal, which is what makes the fold identity
    // and the JSON payload right.
    let root = fixture("cf57-types");
    let policy: OrgCharterPolicy = load_org_charter_policy(&root.join("cases/iv_bool_true"))
        .expect("load")
        .expect("policy");
    let value = policy
        .interview_defaults
        .get(&sk_kernel::PythonString::from_scalars("a"))
        .expect("value");
    assert_eq!(value, &AuthoredScalar::Bool(true));
    assert_ne!(
        value,
        &AuthoredScalar::Text(sk_kernel::PythonString::from_scalars("True"))
    );
    assert_eq!(
        value.python_str().to_scalar_string().unwrap(),
        "True",
        "Python str(True) is 'True', but the value is not text"
    );
    assert_eq!(value.as_bool(), Some(true));
    assert_eq!(value.as_text(), None);
    assert_eq!(
        value.to_json_value(),
        sk_kernel::python_json::PythonJsonValue::Bool(true)
    );
}
