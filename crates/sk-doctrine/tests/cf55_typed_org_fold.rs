//! CF55 — the canonical **typed** cross-pack org-policy fold and its
//! writer-facing floor (`org_required_stems`), proved against the pinned Source.
//!
//! CF48 delivered the canonical **raw** `org-charter.yaml` reader and disclosed
//! that the reference's `org_required_stems` does not read the raw per-field walk
//! at all: it reads the typed cross-pack fold `load_org_charter_policies`. This
//! file proves the new typed fold exists, that its verdicts match the reference,
//! and that the production writer floor consumes it — so a pack the typed model
//! refuses no longer contributes its already-valid `required_<kind>` lists.
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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::{Value as JsonValue, json};
use sk_charter::ArtifactKind;
use sk_doctrine::{
    IdentityIndex, PythonString, fold_org_charter_policies, org_required_stems,
    read_org_required_provenance_exact,
};

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The reference receipt: the typed fold, the writer floor and the raw reader.
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

from charter.activation.org_charter import load_org_charter_policies, org_required_stems
from charter.activation.org_pack_discovery import read_org_required_tokens
from charter.offering.artifact_kinds import ArtifactKind

project = pathlib.Path(os.environ["CF55_PROJECT"])
kind = os.environ["CF55_KIND"]
kind_enum = next(candidate for candidate in ArtifactKind if candidate.plural == kind)

def points(text):
    return [ord(char) for char in text]

def exact(value):
    return None if value is None else points(value)

def text(value):
    return points(str(value))

payload = {
    "runner": "source-python-3.11.15",
    "cwd": str(pathlib.Path.cwd()),
    "fold": None,
    "fold_error": None,
    "stems": None,
    "stems_error": None,
    "warnings": [],
    "raw": None,
}

try:
    policy = load_org_charter_policies(project)
except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
    payload["fold_error"] = type(exc).__name__
    policy = None

if policy is not None:
    payload["fold"] = {
        "schema_version": policy.schema_version,
        "org_name": exact(policy.org_name),
        "skill_namespace": exact(policy.skill_namespace),
        "required": [points(token) for token in getattr(policy, "required_" + kind)],
        "interview_defaults": sorted(
            [[points(key), text(value)] for key, value in policy.interview_defaults.items()]
        ),
        "governance": [
            [points(gp.field), text(gp.value), points(gp.enforcement)]
            for gp in policy.governance_policies
        ],
        "activations": [
            [
                sorted([[points(key), points(value)] for key, value in entry.activation_context.items()]),
                points(entry.charter_pack_id),
                points(entry.artifact_id),
                exact(entry.artifact_kind),
            ]
            for entry in policy.activations
        ],
    }

warnings = []
try:
    payload["stems"] = [points(stem) for stem in org_required_stems(project, kind, warnings)]
except Exception as exc:  # noqa: BLE001 - the reference's own refusal is the result
    payload["stems_error"] = type(exc).__name__
payload["warnings"] = [points(warning) for warning in warnings]
payload["raw"] = [
    points(token) for token in read_org_required_tokens(project, kind_enum, strict=False)
]
print("CF55_RECEIPT " + json.dumps(payload))
"#;

/// A fixture root with a project and the ordered `(name, org-charter body)` packs.
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
        write(
            &pack.join("drg/fragment.yaml"),
            &format!("pack_name: {name}\nnodes: []\nedges: []\n"),
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

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
    fs::write(path, text).expect("write fixture");
}

fn identity(project: &Path) -> IdentityIndex {
    let org_roots: Vec<PathBuf> = sk_doctrine::configured_org_roots(project)
        .expect("org roots")
        .into_iter()
        .map(|(_, root)| root)
        .collect();
    IdentityIndex::from_roots(
        &isolated::repository_root().join("packs/built-in"),
        &org_roots,
        None,
    )
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

/// The reference's phase name for a Native refusal the Source child also raises.
fn refusal_phase(error: &sk_doctrine::DoctrineError) -> &'static str {
    match error {
        sk_doctrine::DoctrineError::ActivationIdCollision { .. } => "ActivationIdCollisionError",
        sk_doctrine::DoctrineError::OrgCharterRetiredField { .. } => "RetiredPackFieldError",
        _ => "DoctrineError",
    }
}

/// The Native side of the receipt, in an isolated re-exec of this binary.
fn native_receipt_json(project: &Path, kind: ArtifactKind) -> String {
    let identity = identity(project);
    let mut warnings = Vec::new();
    let stems = org_required_stems(project, kind, &identity, &mut warnings);
    let stems_error = stems
        .as_ref()
        .err()
        .map(|error| JsonValue::from(refusal_phase(error)))
        .unwrap_or(JsonValue::Null);
    let stems = stems.unwrap_or_default();
    let raw = sk_doctrine::read_org_required_tokens_exact(project, kind, false)
        .expect("native raw reader");
    let fold = fold_org_charter_policies(project);
    let fold_error = fold
        .as_ref()
        .err()
        .map(|error| JsonValue::from(refusal_phase(error)))
        .unwrap_or(JsonValue::Null);
    let fold = fold.ok();

    let fold_json = match &fold {
        None => JsonValue::Null,
        Some(fold) => {
            let mut interview_defaults: Vec<JsonValue> = fold
                .interview_defaults()
                .iter()
                .map(|(key, value)| json!([points(key), points(&value.python_str())]))
                .collect();
            interview_defaults.sort_by_key(|entry| entry[0].to_string());
            // The reference receipt carries the exact Python `int`. `serde_json`
            // holds an i64/u64 as a number and anything wider as its exact decimal
            // text; either way the value is never truncated.
            let schema_version =
                match serde_json::Number::from_str(fold.schema_version().as_decimal()) {
                    Ok(number) => JsonValue::Number(number),
                    Err(_) => JsonValue::String(fold.schema_version().as_decimal().to_owned()),
                };
            json!({
                "schema_version": schema_version,
                "org_name": value_of(fold.org_name()),
                "skill_namespace": value_of(fold.skill_namespace()),
                "required": fold.required_for(kind).iter().map(points).collect::<Vec<_>>(),
                "interview_defaults": interview_defaults,
                "governance": fold
                    .governance_policies()
                    .iter()
                    .map(|policy| {
                        json!([
                            points(&policy.field),
                            points(&policy.value.python_str()),
                            points(&policy.enforcement)
                        ])
                    })
                    .collect::<Vec<_>>(),
                "activations": fold
                    .activations()
                    .iter()
                    .map(|entry| {
                        let mut context: Vec<(Vec<u32>, Vec<u32>)> = entry
                            .context
                            .iter()
                            .map(|(key, value)| (points(key), points(value)))
                            .collect();
                        context.sort();
                        json!([
                            context,
                            points(&entry.charter_pack_id),
                            points(&entry.artifact_id),
                            value_of(entry.artifact_kind.as_ref()),
                        ])
                    })
                    .collect::<Vec<_>>(),
            })
        }
    };

    json!({
        "runner": "native",
        "cwd": std::env::current_dir()
            .expect("cwd")
            .display()
            .to_string(),
        "fold": fold_json,
        "fold_error": fold_error,
        "stems": stems.iter().map(points).collect::<Vec<_>>(),
        "stems_error": stems_error,
        "warnings": warnings
            .iter()
            .map(|warning| warning.chars().map(|c| c as u32).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        "raw": raw.iter().map(points).collect::<Vec<_>>(),
    })
    .to_string()
}

/// The Native child: assert isolation, then answer from the canonical APIs.
#[test]
fn cf55_native_child_and_receipt() {
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
    let project = PathBuf::from(std::env::var("CF55_PROJECT").expect("CF55_PROJECT"));
    let plural = std::env::var("CF55_KIND").expect("CF55_KIND");
    let kind = ArtifactKind::from_plural(&plural).expect("kind plural");
    println!("CF55_RECEIPT {}", native_receipt_json(&project, kind));
}

/// Run the Source child, assert it ran isolated, then the Native child.
fn run_pair(root: &Path, project: &Path, kind: &str, test_name: &str) -> (JsonValue, JsonValue) {
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let source_stdout = isolated::source_script(
        root,
        SOURCE_SCRIPT,
        &[
            ("PYTHONHASHSEED", "0"),
            ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
            ("CF55_PROJECT", project.to_str().expect("project")),
            ("CF55_KIND", kind),
        ],
    );
    let source = isolated::receipt_json(&source_stdout, "CF55_RECEIPT ");
    assert_eq!(
        source["runner"], "source-python-3.11.15",
        "the Source child is the pinned interpreter"
    );
    assert_eq!(source["cwd"], root.display().to_string());
    let native_stdout = isolated::native_reexec(
        root,
        test_name,
        &[
            ("PYTHONHASHSEED", "0"),
            ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
            ("CF55_PROJECT", project.to_str().expect("project")),
            ("CF55_KIND", kind),
        ],
    );
    let native = isolated::receipt_json(&native_stdout, "CF55_RECEIPT ");
    assert_eq!(native["runner"], "native");
    assert_eq!(
        native["cwd"], source["cwd"],
        "both children ran in the fixture"
    );
    (source, native)
}

fn tokens(names: &[&str]) -> JsonValue {
    json!(
        names
            .iter()
            .map(|name| name.chars().map(|c| c as u32).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    )
}

/// One string as its exact code-point array.
fn t(text: &str) -> JsonValue {
    json!(text.chars().map(|c| c as u32).collect::<Vec<_>>())
}

#[test]
fn typed_invalid_pack_is_excluded_while_the_raw_reader_still_sees_it() {
    // The CF48 defect this capability closes: `schema_version: not-an-int` makes
    // the typed model refuse the whole pack, so `org_required_stems` must NOT see
    // its `required_directives`; the raw per-field reader ignores the unrelated
    // field and still reads them (its own, different role).
    let (root, project) = fixture(
        "cf55-typed-invalid",
        &[
            ("good", "required_directives: [DIRECTIVE_001]\n"),
            (
                "bad",
                "schema_version: not-an-int\nrequired_directives: [BAD1]\n",
            ),
        ],
    );
    let (source, native) = run_pair(
        &root,
        &project,
        "directives",
        "cf55_native_child_and_receipt",
    );
    let stem = "001-architectural-integrity-standard";
    // The Source cause: the typed fold keeps the good pack only; the raw reader
    // keeps both; the writer floor maps the surviving id to its stem.
    assert_eq!(source["fold"]["required"], tokens(&["DIRECTIVE_001"]));
    assert_eq!(source["stems"], tokens(&[stem]));
    assert_eq!(source["raw"], tokens(&["DIRECTIVE_001", "BAD1"]));
    assert_eq!(native["raw"], source["raw"], "the raw reader is unchanged");
    assert_eq!(native["fold"], source["fold"], "the typed fold verdict");
    assert_eq!(native["stems"], source["stems"], "the writer floor");
    assert_eq!(native["warnings"], source["warnings"]);
}

#[test]
fn fold_keeps_declaration_order_not_extends_chain_order() {
    // The reference's writer floor calls `load_org_charter_policies(repo_root)`
    // with no `pack_context`, so the typed fold never resolves `extends:`: it
    // unions each configured pack's OWN document in declaration order. The raw
    // walk does resolve the chain, which re-orders the same tokens — a real,
    // observable difference between the two roles.
    let (root, project) = fixture(
        "cf55-declared-order",
        &[
            ("alpha", "extends: zeta\nrequired_directives: [A1]\n"),
            ("middle", "required_directives: [M1]\n"),
            ("zeta", "required_directives: [Z1]\n"),
        ],
    );
    let (source, native) = run_pair(
        &root,
        &project,
        "directives",
        "cf55_native_child_and_receipt",
    );
    assert_eq!(
        source["fold"]["required"],
        tokens(&["A1", "M1", "Z1"]),
        "the typed fold is declaration order, not chain order"
    );
    // The raw walk emits alpha, then alpha's base zeta, then middle, then zeta.
    assert_eq!(source["raw"], tokens(&["A1", "Z1", "M1"]));
    assert_eq!(native["fold"], source["fold"]);
    assert_eq!(native["raw"], source["raw"]);
    assert_eq!(native["stems"], source["stems"]);
}

#[test]
fn typed_projection_and_coercions_match_the_source_model() {
    // `schema_version: '1_0'` (Python `int()` text), an unstripped `org_name`, a
    // blank later `skill_namespace`, `str | bool` interview/governance values and
    // a singular `artifact_kind` all go through the reference model's own rules.
    let (root, project) = fixture(
        "cf55-typed-projection",
        &[
            (
                "acme",
                "schema_version: '1_0'\n\
                 org_name: '  Acme  '\n\
                 skill_namespace: '   '\n\
                 required_directives: ['  DIRECTIVE_001  ', DIRECTIVE_001]\n\
                 interview_defaults: {a: 1, b: '  x  '}\n\
                 governance_policies:\n  - {field: commits, value: 1}\n\
                 activations:\n  - activation_context: {mission_type: software-dev, action: implement}\n    charter_pack_id: acme\n    artifact_id: a1\n    artifact_kind: directive\n",
            ),
            ("beta", "skill_namespace: 'beta-ns'\n"),
        ],
    );
    let (source, native) = run_pair(
        &root,
        &project,
        "directives",
        "cf55_native_child_and_receipt",
    );
    assert_eq!(
        source["fold"]["schema_version"], 2,
        "the lenient fold keeps the LAST truthy version, and the second pack's \
         model default (2) is last — `schema_version: '1_0'` only proves the first \
         pack parsed (its ids and org_name survive below)"
    );
    assert_eq!(source["fold"]["org_name"], t("  Acme  "));
    assert_eq!(
        source["fold"]["skill_namespace"],
        t("beta-ns"),
        "the fold keeps the last non-empty namespace"
    );
    assert_eq!(
        source["fold"]["required"],
        tokens(&["DIRECTIVE_001"]),
        "the fold strips and dedupes the authored list"
    );
    assert_eq!(
        source["fold"]["interview_defaults"],
        json!([[t("a"), t("True")], [t("b"), t("  x  ")],]),
        "an int 0/1 is the reference model's bool; a string is kept verbatim"
    );
    assert_eq!(
        source["fold"]["governance"],
        json!([[t("commits"), t("True"), t("advisory")]])
    );
    assert_eq!(
        source["fold"]["activations"],
        json!([[
            [
                [t("action"), t("implement")],
                [t("mission_type"), t("software-dev")],
            ],
            t("acme"),
            t("a1"),
            t("directives"),
        ]]),
        "the singular kind is normalised to the canonical plural"
    );
    assert_eq!(native["fold"], source["fold"], "the typed projection");
    assert_eq!(native["stems"], source["stems"]);
    assert_eq!(native["raw"], source["raw"]);
}

#[test]
fn retired_activation_field_refuses_the_writer_floor() {
    // `doctrine_pack_id` was replaced by `charter_pack_id` (#3732). The reference
    // RAISES `RetiredPackFieldError` from the typed fold instead of skipping the
    // pack — the floor refuses rather than silently losing the pack's ids.
    let (root, project) = fixture(
        "cf55-retired-field",
        &[(
            "acme",
            "required_directives: [DIRECTIVE_001]\n\
             activations:\n  - activation_context: {}\n    doctrine_pack_id: acme\n    artifact_id: a1\n",
        )],
    );
    let (source, native) = run_pair(
        &root,
        &project,
        "directives",
        "cf55_native_child_and_receipt",
    );
    assert_eq!(
        source["fold_error"], "RetiredPackFieldError",
        "the reference re-raises instead of skipping the pack"
    );
    assert_eq!(source["stems_error"], "RetiredPackFieldError");
    assert_eq!(
        native["fold_error"], source["fold_error"],
        "the Native fold refuses with the same phase"
    );
    assert_eq!(native["stems_error"], source["stems_error"]);
    let error = org_required_stems(
        &project,
        ArtifactKind::Directive,
        &identity(&project),
        &mut Vec::new(),
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("doctrine_pack_id")
            && message.contains("charter_pack_id")
            && message.contains("docs/migrations/charter-pack-cutover.md"),
        "the refusal names the field, its replacement and the runbook: {message}"
    );
}

#[test]
fn writer_floor_and_fold_write_nothing() {
    let (root, project) = fixture(
        "cf55-no-writes",
        &[
            ("good", "required_directives: [DIRECTIVE_001]\n"),
            (
                "bad",
                "schema_version: not-an-int\nrequired_directives: [BAD1]\n",
            ),
        ],
    );
    let before = footprint(&root);
    let fold = fold_org_charter_policies(&project).expect("fold");
    assert_eq!(fold.contributing_packs(), ["good"]);
    let _ = org_required_stems(
        &project,
        ArtifactKind::Directive,
        &identity(&project),
        &mut Vec::new(),
    )
    .expect("writer floor");
    let _ = sk_doctrine::read_org_required_tokens_exact(&project, ArtifactKind::Directive, false)
        .expect("raw reader");
    assert_eq!(before, footprint(&root), "no file was created or rewritten");
}

#[test]
fn writer_refuses_a_collision_that_the_reader_keeps_as_written() {
    // `DIRECTIVE_001` is declared by the built-in `001-...` file; the org pack
    // declares a DIFFERENT id in a file with that same name, so the file name can
    // no longer stand for `DIRECTIVE_001`. The reference's READERS catch that
    // collision and keep the token as written (a deny can never drop it); its
    // WRITERS refuse it. Both behaviors exist here on purpose.
    let (_root, project) = fixture(
        "cf55-collision",
        &[("acme", "required_directives: [DIRECTIVE_001]\n")],
    );
    write(
        &project.join("packs/acme/directives/001-architectural-integrity-standard.directive.yaml"),
        "schema_version: \"1.0\"\nid: DIRECTIVE_OTHER\ntitle: Other\nintent: other\n",
    );
    let identity = identity(&project);
    let reader =
        read_org_required_provenance_exact(&project, ArtifactKind::Directive, &identity, false)
            .expect("the reader keeps the colliding token as written");
    let key = PythonString::from_scalars("DIRECTIVE_001");
    assert!(
        reader.contains_key(&key),
        "the reader keeps the authored token, not a stem: {reader:?}"
    );
    let error = org_required_stems(
        &project,
        ArtifactKind::Directive,
        &identity,
        &mut Vec::new(),
    )
    .expect_err("the writer refuses the ambiguous stem");
    assert!(
        matches!(
            error,
            sk_doctrine::DoctrineError::ActivationIdCollision { .. }
        ),
        "{error:?}"
    );
}

/// Every file under `root` with its length, so a created/rewritten file is caught.
fn footprint(root: &Path) -> BTreeMap<PathBuf, u64> {
    let mut entries = BTreeMap::new();
    collect(root, root, &mut entries);
    entries
}

fn collect(root: &Path, directory: &Path, entries: &mut BTreeMap<PathBuf, u64>) {
    let Ok(read) = fs::read_dir(directory) else {
        return;
    };
    for entry in read {
        let entry = entry.expect("entry");
        let path = entry.path();
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            collect(root, &path, entries);
        } else {
            let relative = path.strip_prefix(root).expect("relative").to_path_buf();
            entries.insert(relative, entry.metadata().expect("metadata").len());
        }
    }
}
