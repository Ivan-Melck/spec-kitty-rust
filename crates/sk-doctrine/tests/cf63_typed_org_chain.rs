//! CF63 — typed org pack-context / `extends:`-chain assembly and the authored
//! per-pack interview-default order, proved against the pinned Source.
//!
//! The reference has two typed doors into `org-charter.yaml`:
//!
//! * `load_org_charter_policies(repo_root)` — the flat, registry-order fold this
//!   crate already had (`fold_org_charter_policies`);
//! * `load_org_charter_policies(repo_root, pack_context=ctx)` — the
//!   pack-context door: build the typed pack set from `ctx.pack_roots`, resolve
//!   each pack's `extends:` chain **base first**, fold every chain under strict
//!   `schema_version` equality, then fold the resolved packs in `pack_roots`
//!   order (per-key interview-default replacement, first-seen key positions).
//!
//! This file pins the second door and the authored key order it must preserve.
//!
//! Both sides are actual processes. The Source child is the pinned CPython
//! 3.11.15 oracle selected and lock-exact-verified by `support::python`; the
//! Native child is this test binary re-executed. Each asserts its own CWD, HOME,
//! all seven XDG roots, TMPDIR/TMP/TEMP, the absolute `PYTHONPATH`, the pinned
//! packs root and `PYTHONHASHSEED=0` before answering, and every string travels
//! as code points so a lone surrogate survives the transport exactly.
//!
//! Not CLI acceptance and not a parity claim beyond the exercised fixtures.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::{Value as JsonValue, json};
use sk_doctrine::PythonString;

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The reference receipt for one typed case.
///
/// `CF63_MODE` selects the door:
/// * `flat` — `load_org_charter_policies(project)`;
/// * `per_pack` — `load_org_charter_policy(project/packs/<CF63_PACKS[0]>)`;
/// * `context` — `load_org_charter_policies(project, pack_context=ctx)` over the
///   ordered `CF63_PACKS` roots.
const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys
root = pathlib.Path(os.environ["SK_E4_ROOT"])
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["SK_CF63_SOURCE_ROOT"])
assert repo.is_absolute()
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

from charter.activation.org_charter import load_org_charter_policies, load_org_charter_policy
from charter.activation.pack_context import PackContext
import charter.activation.org_charter as org_module
assert pathlib.Path(org_module.__file__).is_relative_to(repo / "src")

project = pathlib.Path(os.environ["CF63_PROJECT"])
mode = os.environ["CF63_MODE"]
names = json.loads(os.environ["CF63_PACKS"])

def points(text):
    return [ord(char) for char in text]

def exact(value):
    return None if value is None else points(value)

def scalar(value):
    if isinstance(value, bool):
        return {"type": "bool", "value": value}
    return {"type": "str", "points": points(str(value))}

payload = {
    "runner": "source-python-3.11.15",
    "cwd": str(pathlib.Path.cwd()),
    "ok": None,
    "class": None,
    "message": None,
    "defaults": None,
    "required_directives": None,
    "org_name": None,
    "schema_version": None,
    "contributing": None,
}

def emit(policy):
    payload["ok"] = True
    payload["defaults"] = [[points(key), scalar(value)] for key, value in policy.interview_defaults.items()]
    payload["required_directives"] = [points(token) for token in policy.required_directives]
    payload["org_name"] = exact(policy.org_name)
    payload["schema_version"] = policy.schema_version

try:
    if mode == "flat":
        emit(load_org_charter_policies(project))
    elif mode == "per_pack":
        policy = load_org_charter_policy(project / "packs" / names[0])
        if policy is None:
            payload["ok"] = True
            payload["defaults"] = []
            payload["required_directives"] = []
            payload["schema_version"] = 2
        else:
            emit(policy)
    elif mode == "from_config":
        emit(load_org_charter_policies(project, pack_context=PackContext.from_config(project)))
    else:
        ctx = PackContext(
            activated_kinds=frozenset({"directives", "tactics"}),
            activated_mission_types=frozenset({"software-dev"}),
            pack_roots=tuple(project / "packs" / name for name in names),
            org_pack_names=tuple(names),
            repo_root=project,
        )
        emit(load_org_charter_policies(project, pack_context=ctx))
except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
    payload["ok"] = False
    payload["class"] = type(exc).__name__
    payload["message"] = str(exc)

print("CF63_RECEIPT " + json.dumps(payload))
"#;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
    fs::write(path, text).expect("write fixture");
}

/// A fixture root with a project and the ordered `(pack name, org-charter body)`
/// packs, plus a registry entry for each pack.
fn fixture(case: &str, packs: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let root = isolated::fixture(case);
    for (_, name) in EXTRA_XDG_ROOTS {
        fs::create_dir_all(root.join(name)).expect("extra isolated XDG directory");
    }
    let project = root.join("project");
    fs::create_dir_all(&project).expect("project");
    let mut registry = String::from("charter_packs:\n  org:\n    packs:\n");
    for (name, body) in packs {
        let pack = project.join("packs").join(name);
        write(
            &pack.join("pack.yaml"),
            &format!("pack_level: org\nname: {name}\n"),
        );
        write(&pack.join("org-charter.yaml"), body);
        registry.push_str(&format!(
            "      - name: {name}\n        local_path: {}\n",
            pack.display()
        ));
    }
    write(&project.join(".kittify/config.yaml"), &registry);
    (root, project)
}

/// The reference receipt carries `schema_version` as a Python `int`; render the
/// exact decimal as a JSON number when it fits, else as its exact decimal text
/// (`serde_json` is not an arbitrary-width integer transport).
fn schema_json(decimal: &str) -> JsonValue {
    match serde_json::Number::from_str(decimal) {
        Ok(number) => JsonValue::Number(number),
        Err(_) => JsonValue::from(decimal),
    }
}

fn points(token: &PythonString) -> Vec<u32> {
    token.codepoints().to_vec()
}

fn value_of(token: Option<&PythonString>) -> JsonValue {
    match token {
        Some(token) => json!(points(token)),
        None => JsonValue::Null,
    }
}

fn authored(value: &sk_doctrine::AuthoredScalar) -> JsonValue {
    match value {
        sk_doctrine::AuthoredScalar::Bool(flag) => json!({"type": "bool", "value": flag}),
        sk_doctrine::AuthoredScalar::Text(text) => json!({"type": "str", "points": points(text)}),
    }
}

/// One Native receipt: the door named by `CF63_MODE`, in the `OrgCharterPolicyFold`
/// shape the reference receipt uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Door {
    Flat,
    PerPack,
    Context,
    FromRepo,
}

fn door() -> Door {
    match std::env::var("CF63_MODE").expect("CF63_MODE").as_str() {
        "flat" => Door::Flat,
        "per_pack" => Door::PerPack,
        "context" => Door::Context,
        "from_config" => Door::FromRepo,
        other => panic!("unknown CF63_MODE {other:?}"),
    }
}

fn native_fold(
    project: &Path,
    names: &[String],
    door: Door,
) -> Result<JsonValue, sk_doctrine::DoctrineError> {
    match door {
        Door::Flat => sk_doctrine::fold_org_charter_policies(project).map(|fold| {
            json!({
                "defaults": fold
                    .interview_defaults()
                    .iter()
                    .map(|(key, value)| json!([points(key), authored(value)]))
                    .collect::<Vec<_>>(),
                "required_directives": fold
                    .required_for(sk_charter::ArtifactKind::Directive)
                    .iter()
                    .map(points)
                    .collect::<Vec<_>>(),
                "org_name": value_of(fold.org_name()),
                "schema_version": schema_json(fold.schema_version().as_decimal()),
            })
        }),
        Door::PerPack => {
            let policy =
                sk_doctrine::load_org_charter_policy(&project.join("packs").join(&names[0]));
            policy.map(|policy| match policy {
                None => json!({
                    "defaults": [],
                    "required_directives": [],
                    "org_name": JsonValue::Null,
                    "schema_version": 2,
                }),
                Some(policy) => json!({
                    "defaults": policy
                        .interview_defaults
                        .iter()
                        .map(|(key, value)| json!([points(key), authored(value)]))
                        .collect::<Vec<_>>(),
                    "required_directives": policy
                        .required_for(sk_charter::ArtifactKind::Directive)
                        .iter()
                        .map(points)
                        .collect::<Vec<_>>(),
                    "org_name": value_of(policy.org_name.as_ref()),
                    "schema_version": schema_json(policy.schema_version.as_decimal()),
                }),
            })
        }
        Door::FromRepo => {
            let context = sk_doctrine::OrgPackContext::from_repo(project);
            context
                .and_then(|context| {
                    sk_doctrine::fold_org_charter_policies_with_context(project, Some(&context))
                })
                .map(|fold| {
                    json!({
                        "defaults": fold
                            .interview_defaults()
                            .iter()
                            .map(|(key, value)| json!([points(key), authored(value)]))
                            .collect::<Vec<_>>(),
                        "required_directives": fold
                            .required_for(sk_charter::ArtifactKind::Directive)
                            .iter()
                            .map(points)
                            .collect::<Vec<_>>(),
                        "org_name": value_of(fold.org_name()),
                        "schema_version": schema_json(fold.schema_version().as_decimal()),
                    })
                })
        }
        Door::Context => {
            let roots = names
                .iter()
                .map(|name| project.join("packs").join(name))
                .collect::<Vec<_>>();
            let context = sk_doctrine::OrgPackContext::from_roots(roots);
            sk_doctrine::fold_org_charter_policies_with_context(project, Some(&context)).map(
                |fold| {
                    json!({
                        "defaults": fold
                            .interview_defaults()
                            .iter()
                            .map(|(key, value)| json!([points(key), authored(value)]))
                            .collect::<Vec<_>>(),
                        "required_directives": fold
                            .required_for(sk_charter::ArtifactKind::Directive)
                            .iter()
                            .map(points)
                            .collect::<Vec<_>>(),
                        "org_name": value_of(fold.org_name()),
                        "schema_version": schema_json(fold.schema_version().as_decimal()),
                    })
                },
            )
        }
    }
}

/// The reference exception class a Native refusal stands for.
///
/// The reference raises two distinct `extends:` classes and both arrive here as
/// the crate's one `OrgCharterExtends` variant, so the class is read off the exact
/// message text the variant carries.
fn native_error_phase(error: &sk_doctrine::DoctrineError) -> &'static str {
    match error {
        sk_doctrine::DoctrineError::OrgCharterRetiredField { .. } => "RetiredPackFieldError",
        sk_doctrine::DoctrineError::OrgCharterExtends { message, .. } => {
            if message.starts_with("Cycle detected in extends: chain:") {
                "OrgCharterCycleError"
            } else {
                "OrgCharterExtensionError"
            }
        }
        sk_doctrine::DoctrineError::InvalidOrgCharter { .. } => "ValueError",
        _ => "DoctrineError",
    }
}

/// The reference exception TEXT a Native refusal stands for.
///
/// The crate's shared error enum renders its own diagnostic wrapper
/// (`org charter <path>: invalid extends chain: ...`), and the retired-field
/// variant carries the same facts the reference's exception names; this projects
/// the facts the reference prints instead of asserting the wrapper as parity.
fn native_message(error: &sk_doctrine::DoctrineError) -> String {
    match error {
        sk_doctrine::DoctrineError::OrgCharterExtends { message, .. } => message.clone(),
        sk_doctrine::DoctrineError::InvalidOrgCharter { message, .. } => message.clone(),
        sk_doctrine::DoctrineError::OrgCharterRetiredField {
            path,
            field,
            replacement,
            runbook,
        } => format!(
            "{}: field '{}' was removed. Replacement: {}. See {runbook}.",
            path.display(),
            field,
            replacement
        ),
        other => other.to_string(),
    }
}

/// The Native side of the receipt, in an isolated re-exec of this binary.
fn native_receipt_json(project: &Path, names: &[String], door: Door) -> String {
    let fold = native_fold(project, names, door);
    let mut receipt = json!({
        "runner": "native",
        "cwd": std::env::current_dir().expect("cwd").display().to_string(),
        "ok": false,
        "class": JsonValue::Null,
        "message": JsonValue::Null,
        "defaults": JsonValue::Null,
        "required_directives": JsonValue::Null,
        "org_name": JsonValue::Null,
        "schema_version": JsonValue::Null,
        "contributing": JsonValue::Null,
    });
    match fold {
        Ok(payload) => {
            receipt["ok"] = json!(true);
            for (key, value) in payload.as_object().expect("payload object") {
                receipt[key] = value.clone();
            }
        }
        Err(error) => {
            receipt["class"] = json!(native_error_phase(&error));
            receipt["message"] = json!(native_message(&error));
        }
    }
    receipt.to_string()
}

/// The Native child: assert isolation, then answer from the canonical APIs.
#[test]
fn cf63_native_child_and_receipt() {
    let Some(root) = isolated::native_root() else {
        return;
    };
    isolated::assert_isolated(&root);
    assert_eq!(
        std::env::var_os("SK_CF63_SOURCE_ROOT").map(PathBuf::from),
        Some(isolated::repository_root())
    );
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
    let project = PathBuf::from(std::env::var("CF63_PROJECT").expect("CF63_PROJECT"));
    let names: Vec<String> =
        serde_json::from_str(&std::env::var("CF63_PACKS").expect("CF63_PACKS")).expect("pack JSON");
    println!(
        "CF63_RECEIPT {}",
        native_receipt_json(&project, &names, door())
    );
}

/// Run the Source child, assert it ran isolated, then the Native child.
fn run_pair(root: &Path, project: &Path, names: &[&str], door: Door) -> (JsonValue, JsonValue) {
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let names_json = serde_json::to_string(names).expect("pack list JSON");
    let mode = match door {
        Door::Flat => "flat",
        Door::PerPack => "per_pack",
        Door::Context => "context",
        Door::FromRepo => "from_config",
    };
    let source_root = isolated::repository_root();
    let env: Vec<(&str, &str)> = vec![
        (
            "SK_CF63_SOURCE_ROOT",
            source_root.to_str().expect("Source root"),
        ),
        ("PYTHONHASHSEED", "0"),
        ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
        ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
        ("CF63_PROJECT", project.to_str().expect("project")),
        ("CF63_PACKS", &names_json),
        ("CF63_MODE", mode),
    ];
    let source_stdout = isolated::source_script(root, SOURCE_SCRIPT, &env);
    let source = isolated::receipt_json(&source_stdout, "CF63_RECEIPT ");
    assert_eq!(
        source["runner"], "source-python-3.11.15",
        "the Source child is the pinned interpreter"
    );
    assert_eq!(source["cwd"], root.display().to_string());
    let native_stdout = isolated::native_reexec(root, "cf63_native_child_and_receipt", &env);
    let native = isolated::receipt_json(&native_stdout, "CF63_RECEIPT ");
    assert_eq!(native["runner"], "native");
    assert_eq!(
        native["cwd"], source["cwd"],
        "both children ran in the fixture"
    );
    (source, native)
}

fn point_list(value: &JsonValue) -> Vec<u32> {
    value
        .as_array()
        .expect("code-point array")
        .iter()
        .map(|point| u32::try_from(point.as_u64().expect("code point")).expect("u32"))
        .collect()
}

fn default_keys(receipt: &JsonValue) -> Vec<Vec<u32>> {
    receipt["defaults"]
        .as_array()
        .unwrap_or_else(|| panic!("no defaults in receipt: {receipt}"))
        .iter()
        .map(|entry| point_list(&entry[0]))
        .collect()
}

fn text(value: &str) -> Vec<u32> {
    value.chars().map(|character| character as u32).collect()
}

#[test]
fn authored_interview_default_order_is_the_reference_dict_order() {
    // The reference's model field is a Python `dict`: `interview_defaults`
    // iterates in AUTHORED order (`zulu` before `alpha` here), never sorted by
    // key, and a later pack's override keeps the FIRST declaration's slot.
    let (root, project) = fixture(
        "cf63-authored-order",
        &[
            (
                "base",
                "schema_version: 1\n\
                 interview_defaults:\n\
                 \x20 zulu: from-base\n\
                 \x20 alpha: keep-base\n",
            ),
            (
                "overlay",
                "schema_version: 1\n\
                 extends: base\n\
                 interview_defaults:\n\
                 \x20 mike: from-overlay\n\
                 \x20 zulu: from-overlay\n\
                 \x20 alpha: from-overlay\n",
            ),
        ],
    );
    let (source, native) = run_pair(&root, &project, &["base", "overlay"], Door::PerPack);
    assert_eq!(source["ok"], true, "Source per-pack load: {source}");
    assert_eq!(
        default_keys(&source),
        vec![text("zulu"), text("alpha")],
        "the reference keeps the authored per-pack order: {source}"
    );
    assert_eq!(
        default_keys(&native),
        default_keys(&source),
        "the typed loader must carry the authored dict order, not a sorted key order"
    );
    assert_eq!(
        native["defaults"], source["defaults"],
        "values follow the keys"
    );
}

/// Compare the full typed-door receipt (defaults, floor, metadata) both ways.
fn assert_same_receipt(source: &JsonValue, native: &JsonValue, what: &str) {
    assert_eq!(
        source["ok"], true,
        "{what}: the reference accepted the fixture: {source}"
    );
    assert_eq!(
        native["ok"], true,
        "{what}: Native accepted the fixture: {native}"
    );
    assert_eq!(
        native["defaults"], source["defaults"],
        "{what}: interview defaults (order + values)"
    );
    assert_eq!(
        native["required_directives"], source["required_directives"],
        "{what}: required_directives"
    );
    assert_eq!(native["org_name"], source["org_name"], "{what}: org_name");
    assert_eq!(
        native["schema_version"], source["schema_version"],
        "{what}: schema_version"
    );
}

fn assert_same_refusal(source: &JsonValue, native: &JsonValue, what: &str) {
    assert_eq!(
        source["ok"], false,
        "{what}: the reference refused the fixture: {source}"
    );
    assert_eq!(
        native["ok"], false,
        "{what}: Native refused the fixture: {native}"
    );
    assert_eq!(native["class"], source["class"], "{what}: refusal class");
    assert_eq!(
        native["message"], source["message"],
        "{what}: refusal message (Source's typed wording)"
    );
}

const BASE_OVERLAY_PACKS: [(&str, &str); 3] = [
    (
        "base",
        "schema_version: 1\n\
         org_name: Base Corp\n\
         required_directives: [base-1, shared-1]\n\
         interview_defaults:\n\
         \x20 zulu: from-base\n\
         \x20 alpha: keep-base\n",
    ),
    (
        "overlay",
        "schema_version: 1\n\
         extends: base\n\
         org_name: Overlay Corp\n\
         required_directives: [overlay-1, shared-1]\n\
         interview_defaults:\n\
         \x20 mike: from-overlay\n\
         \x20 zulu: from-overlay\n\
         \x20 alpha: from-overlay\n",
    ),
    (
        "standalone",
        "schema_version: 1\n\
         required_directives: [solo-1]\n\
         interview_defaults:\n\
         \x20 yankee: solo\n",
    ),
];

#[test]
fn pack_context_folds_base_first_with_authored_order_and_first_seen_union() {
    // Base + derived + declared pack, `pack_roots` in declaration order. The
    // derived pack inherits its base's directives (union, first-seen) and the
    // base's first-declared key slots survive the override.
    let (root, project) = fixture("cf63-context-declaration-order", &BASE_OVERLAY_PACKS);
    let (source, native) = run_pair(
        &root,
        &project,
        &["base", "overlay", "standalone"],
        Door::Context,
    );
    assert_same_receipt(&source, &native, "base+overlay+standalone");
    assert_eq!(
        default_keys(&native),
        vec![text("zulu"), text("alpha"), text("mike"), text("yankee")],
        "authored per-pack order, override in the FIRST slot, later pack's new keys appended"
    );
    assert_eq!(
        point_list(&native["required_directives"][0]),
        text("base-1")
    );
    assert_eq!(
        point_list(&native["required_directives"][3]),
        text("solo-1")
    );
    assert_eq!(native["org_name"], source["org_name"]);
}

#[test]
fn pack_context_order_decides_the_winner_on_a_collision() {
    // Reversing `pack_roots` reverses which pack's override survives, exactly like
    // the reference's `pack_roots`-order fold.
    let (root, project) = fixture("cf63-context-reversed", &BASE_OVERLAY_PACKS);
    let (source, native) = run_pair(&root, &project, &["overlay", "base"], Door::Context);
    assert_same_receipt(&source, &native, "overlay before base");
    assert_eq!(
        default_keys(&native),
        vec![text("zulu"), text("alpha"), text("mike")],
        "the overlay's own chain still declares mike; base then re-points zulu/alpha in place"
    );
}

#[test]
fn pack_context_adds_the_built_in_anchor_like_the_reference_context() {
    // `OrgPackContext::from_repo` is the reference's `PackContext.from_config`
    // pack-root slice: the built-in anchor first, then the declared org packs.
    let (root, project) = fixture(
        "cf63-context-from-repo",
        &[("only", "schema_version: 1\nrequired_directives: [only-1]\n")],
    );
    let context = sk_doctrine::OrgPackContext::from_repo(&project).expect("context");
    let roots = context.pack_roots();
    assert_eq!(
        roots.len(),
        2,
        "built-in anchor + one declared org pack: {roots:?}"
    );
    assert_eq!(
        roots[1],
        project.join("packs/only"),
        "declared org roots keep config order"
    );
    let fold = sk_doctrine::fold_org_charter_policies_with_context(&project, Some(&context))
        .expect("fold");
    assert_eq!(
        fold.required_scalar(sk_charter::ArtifactKind::Directive),
        vec!["only-1".to_owned()]
    );
    // The built-in anchor ships no org-charter.yaml, so it contributes nothing.
    assert_eq!(fold.contributing_packs(), ["only"]);
    let _ = root;
}

#[test]
fn from_repo_context_matches_the_reference_from_config() {
    // `OrgPackContext::from_repo` mirrors `PackContext.from_config`'s pack-root
    // slice. The two built-in anchors are different physical directories (the
    // reference anchors on the `charter/offering` package, Native on the packaged
    // built-in pack) and neither ships an `org-charter.yaml`, so the typed fold
    // must agree; a built-in document would be a real divergence to report.
    let (root, project) = fixture(
        "cf63-from-config",
        &[
            (
                "acme",
                "schema_version: 1\n\
                 required_directives: [acme-1, acme-2]\n\
                 interview_defaults:\n\
                 \x20 zz: one\n\
                 \x20 aa: two\n",
            ),
            (
                "team",
                "schema_version: 1\n\
                 extends: acme\n\
                 required_directives: [team-1]\n\
                 interview_defaults:\n\
                 \x20 zz: team-zz\n",
            ),
        ],
    );
    let (source, native) = run_pair(&root, &project, &[], Door::FromRepo);
    assert_same_receipt(&source, &native, "from_config context");
    assert_eq!(
        default_keys(&native),
        vec![text("zz"), text("aa")],
        "the declared packs' authored order survives the from_config door"
    );
    assert_eq!(
        native["required_directives"],
        json!([text("acme-1"), text("acme-2"), text("team-1")]),
        "chain union across the declared packs"
    );
}

#[test]
fn pack_context_chain_refusals_match_the_reference() {
    // Dangling `extends:` -> the reference's `OrgCharterExtensionError` wording.
    let (root, project) = fixture(
        "cf63-context-dangling",
        &[("child", "schema_version: 1\nextends: ghost-pack\n")],
    );
    let (source, native) = run_pair(&root, &project, &["child"], Door::Context);
    assert_same_refusal(&source, &native, "dangling base");

    // Cycle -> `OrgCharterCycleError`, with the loop closed in the path.
    let (root, project) = fixture(
        "cf63-context-cycle",
        &[
            ("alpha", "schema_version: 1\nextends: beta\n"),
            ("beta", "schema_version: 1\nextends: alpha\n"),
        ],
    );
    let (source, native) = run_pair(&root, &project, &["alpha", "beta"], Door::Context);
    assert_same_refusal(&source, &native, "cycle");

    // Strict per-chain `schema_version` equality -> the reference's ValueError.
    let (root, project) = fixture(
        "cf63-context-mismatch",
        &[
            ("mbase", "schema_version: 2\n"),
            ("moverlay", "schema_version: 3\nextends: mbase\n"),
        ],
    );
    let (source, native) = run_pair(&root, &project, &["mbase", "moverlay"], Door::Context);
    assert_same_refusal(&source, &native, "schema_version mismatch");
}

#[test]
fn pack_context_invalid_and_retired_packs_keep_the_reference_split() {
    // A schema-invalid pack is SKIPPED, so a pack that extends it has a dangling
    // base and the chain refuses (the reference's own ordering: skip, then dangle).
    let (root, project) = fixture(
        "cf63-context-invalid-base",
        &[
            ("ibase", "schema_version: 1\nrequired_directives: 7\n"),
            (
                "ioverlay",
                "schema_version: 1\nextends: ibase\nrequired_directives: [i1]\n",
            ),
        ],
    );
    let (source, native) = run_pair(&root, &project, &["ibase", "ioverlay"], Door::Context);
    assert_same_refusal(&source, &native, "invalid base");
    // Alone, the same invalid pack simply contributes nothing.
    let (source, native) = run_pair(&root, &project, &["ibase"], Door::Context);
    assert_same_receipt(&source, &native, "invalid pack alone");

    // A retired field is REFUSED, never skipped.
    let (root, project) = fixture(
        "cf63-context-retired",
        &[(
            "rbase",
            "schema_version: 2\n\
             activations:\n\
             \x20 - doctrine_pack_id: rbase\n\
             \x20   artifact_id: d1\n\
             \x20   artifact_kind: directive\n",
        )],
    );
    let (source, native) = run_pair(&root, &project, &["rbase"], Door::Context);
    assert_eq!(source["class"], "RetiredPackFieldError", "{source}");
    assert_eq!(native["class"], "RetiredPackFieldError", "{native}");
}

#[test]
fn an_empty_context_is_the_model_default_on_both_sides() {
    let (root, project) = fixture(
        "cf63-context-empty",
        &[("ignored", "schema_version: 1\nrequired_directives: [x]\n")],
    );
    let (source, native) = run_pair(&root, &project, &[], Door::Context);
    assert_same_receipt(&source, &native, "empty context");
    assert_eq!(native["schema_version"], 2, "the model default");
    assert_eq!(default_keys(&native), Vec::<Vec<u32>>::new());
    assert!(sk_doctrine::OrgPackContext::from_roots(Vec::new()).is_empty());
}

#[test]
fn the_flat_door_still_ignores_extends_and_stays_the_live_writer_floor() {
    // The reference's no-context call reads the registry and each pack's OWN
    // document: no `extends:` inheritance. `org_required_stems` (the live writer
    // floor) must keep reading this fold, and the two entries must be one body.
    let (root, project) = fixture("cf63-flat-door", &BASE_OVERLAY_PACKS);
    let (source, native) = run_pair(&root, &project, &[], Door::Flat);
    assert_same_receipt(&source, &native, "flat fold");
    assert_eq!(
        default_keys(&native),
        vec![text("zulu"), text("alpha"), text("mike"), text("yankee")],
        "the flat fold still carries every declared pack's own authored order"
    );

    // One body: the no-context entry and the explicit `None` entry agree.
    let flat = sk_doctrine::fold_org_charter_policies(&project).expect("flat fold");
    let explicit_none =
        sk_doctrine::fold_org_charter_policies_with_context(&project, None).expect("explicit None");
    assert_eq!(flat, explicit_none);

    // The live consumer routes through that same typed fold.
    let org_roots = vec![
        project.join("packs/base"),
        project.join("packs/overlay"),
        project.join("packs/standalone"),
    ];
    let identity = sk_doctrine::IdentityIndex::from_roots(
        &isolated::repository_root().join("packs/built-in"),
        &org_roots,
        None,
    );
    let mut warnings = Vec::new();
    let stems = sk_doctrine::org_required_stems(
        &project,
        sk_charter::ArtifactKind::Directive,
        &identity,
        &mut warnings,
    )
    .expect("writer floor");
    let stems: Vec<String> = stems
        .iter()
        .filter_map(|token| token.to_scalar_string().ok())
        .collect();
    assert_eq!(
        stems,
        vec![
            "base-1".to_owned(),
            "shared-1".to_owned(),
            "overlay-1".to_owned(),
            "solo-1".to_owned()
        ],
        "the writer floor reads the typed fold (first-seen union across the declared packs)"
    );
    assert!(
        warnings.iter().any(|warning| warning.contains("base-1")),
        "unresolved ids are still named: {warnings:?}"
    );
    let _ = root;
}
