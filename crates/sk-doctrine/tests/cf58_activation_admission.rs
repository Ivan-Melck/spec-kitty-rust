//! CF58 — the canonical activation selector/trigger/mission registry and the
//! typed org-charter admission gate it drives, proved against the pinned Source.
//!
//! CF55's typed cross-pack fold is live, but a typed pack whose
//! `activation_context` carried an unknown `mission_type`/`action` was accepted,
//! so a typed-invalid pack contributed its `required_<kind>` lists to the writer
//! floor. The reference validates both selectors in the `ActivationEntry` model
//! validator (`charter.activation.activations`), raising `ValidationError`, which
//! its fold skips. This file proves the new registry exists, that its vocabularies
//! are the reference's (derived mission roster + the two action vocabularies), and
//! that the typed consumer now skips such a pack while the raw reader stays
//! unchanged.
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
//! approval of the reference's `pack_context`/`extends:` typed path.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as JsonValue, json};
use sk_charter::ArtifactKind;
use sk_doctrine::{
    ALLOWED_ACTIONS, IdentityIndex, PythonString, REGISTERED_TRIGGERS, admits_action,
    fold_org_charter_policies, org_required_stems, read_org_required_tokens_exact,
};

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// The reference receipt: canonical vocabularies, the typed fold, the writer
/// floor and the raw per-field reader.
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

from charter.activation.activations import (
    ALLOWED_ACTIONS,
    ALLOWED_MISSION_TYPES,
    REGISTERED_TRIGGERS,
)
from charter.activation.org_charter import load_org_charter_policies, org_required_stems
from charter.activation.org_pack_discovery import read_org_required_tokens
from charter.offering.artifact_kinds import ArtifactKind

project = pathlib.Path(os.environ["CF58_PROJECT"])
kind = os.environ["CF58_KIND"]
kind_enum = next(candidate for candidate in ArtifactKind if candidate.plural == kind)

def points(text):
    return [ord(char) for char in text]

def exact(value):
    return None if value is None else points(value)

payload = {
    "runner": "source-python-3.11.15",
    "cwd": str(pathlib.Path.cwd()),
    "allowed_mission_types": [points(token) for token in sorted(ALLOWED_MISSION_TYPES)],
    "allowed_actions": [points(token) for token in sorted(ALLOWED_ACTIONS)],
    "registered_triggers": [points(token) for token in sorted(REGISTERED_TRIGGERS)],
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
        "required": [points(token) for token in getattr(policy, "required_" + kind)],
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
print("CF58_RECEIPT " + json.dumps(payload))
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

/// One activation entry carrying `context` (raw inline YAML) and `extra` lines.
fn activation_entry(id: &str, context: &str, extra: &str) -> String {
    format!(
        "required_directives: [{id}]\n\
         activations:\n  - activation_context: {context}\n    charter_pack_id: pack-{id}\n    \
         artifact_id: {id}\n{extra}"
    )
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

fn words(tokens: &[&str]) -> JsonValue {
    json!(
        tokens
            .iter()
            .map(|text| text.chars().map(|c| c as u32).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    )
}

/// One string as its exact code-point array.
fn t(text: &str) -> JsonValue {
    json!(text.chars().map(|c| c as u32).collect::<Vec<_>>())
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
    let registry =
        sk_doctrine::ActivationSelectorRegistry::canonical().expect("canonical registry");
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
        Some(fold) => json!({
            "required": fold.required_for(kind).iter().map(points).collect::<Vec<_>>(),
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
        }),
    };

    json!({
        "runner": "native",
        "cwd": std::env::current_dir()
            .expect("cwd")
            .display()
            .to_string(),
        "allowed_mission_types": registry
            .allowed_mission_types()
            .iter()
            .map(points)
            .collect::<Vec<_>>(),
        "allowed_actions": words(&ALLOWED_ACTIONS),
        "registered_triggers": words(&REGISTERED_TRIGGERS),
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
fn cf58_native_child_and_receipt() {
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
    let project = PathBuf::from(std::env::var("CF58_PROJECT").expect("CF58_PROJECT"));
    let plural = std::env::var("CF58_KIND").expect("CF58_KIND");
    let kind = ArtifactKind::from_plural(&plural).expect("kind plural");
    println!("CF58_RECEIPT {}", native_receipt_json(&project, kind));
}

/// Run the Source child, assert it ran isolated, then the Native child.
fn run_pair(root: &Path, project: &Path, kind: &str) -> (JsonValue, JsonValue) {
    let config_dirs = root.join("config-dirs");
    let data_dirs = root.join("data-dirs");
    let source_stdout = isolated::source_script(
        root,
        SOURCE_SCRIPT,
        &[
            ("PYTHONHASHSEED", "0"),
            ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
            ("CF58_PROJECT", project.to_str().expect("project")),
            ("CF58_KIND", kind),
        ],
    );
    let source = isolated::receipt_json(&source_stdout, "CF58_RECEIPT ");
    assert_eq!(
        source["runner"], "source-python-3.11.15",
        "the Source child is the pinned interpreter"
    );
    assert_eq!(source["cwd"], root.display().to_string());
    let native_stdout = isolated::native_reexec(
        root,
        "cf58_native_child_and_receipt",
        &[
            ("PYTHONHASHSEED", "0"),
            ("XDG_CONFIG_DIRS", config_dirs.to_str().expect("dirs")),
            ("XDG_DATA_DIRS", data_dirs.to_str().expect("dirs")),
            ("CF58_PROJECT", project.to_str().expect("project")),
            ("CF58_KIND", kind),
        ],
    );
    let native = isolated::receipt_json(&native_stdout, "CF58_RECEIPT ");
    assert_eq!(native["runner"], "native");
    assert_eq!(
        native["cwd"], source["cwd"],
        "both children ran in the fixture"
    );
    (source, native)
}

#[test]
fn canonical_vocabularies_are_the_reference_sets() {
    // A pack with one ordinary activation entry, so the fixture is otherwise
    // valid: this test is about the vocabulary facts, not admission.
    let (root, project) = fixture(
        "cf58-vocab",
        &[(
            "acme",
            &activation_entry(
                "DIRECTIVE_001",
                "{mission_type: software-dev, action: implement}",
                "",
            ),
        )],
    );
    let (source, native) = run_pair(&root, &project, "directives");
    assert_eq!(
        source["allowed_mission_types"],
        json!([
            t("any"),
            t("documentation"),
            t("generic"),
            t("plan"),
            t("research"),
            t("software-dev"),
        ]),
        "the reference derives the roster from the built-in mission_types documents"
    );
    assert_eq!(
        source["allowed_actions"],
        json!([
            t("accept"),
            t("charter.context"),
            t("charter.generate"),
            t("charter.interview"),
            t("implement"),
            t("merge"),
            t("plan"),
            t("review"),
            t("specify"),
            t("tasks"),
        ])
    );
    assert_eq!(
        source["registered_triggers"],
        json!([
            t("accept"),
            t("add_dependency"),
            t("charter.context"),
            t("charter.generate"),
            t("charter.interview"),
            t("implement"),
            t("merge"),
            t("plan"),
            t("rename_identifier"),
            t("review"),
            t("specify"),
            t("tasks"),
            t("write_comment"),
            t("write_docstring"),
        ]),
        "the operator vocabulary carries the four fine-grained triggers"
    );
    for key in [
        "allowed_mission_types",
        "allowed_actions",
        "registered_triggers",
        "fold",
        "raw",
        "stems",
    ] {
        assert_eq!(native[key], source[key], "native {key} is the reference's");
    }
    // The registry's own membership predicate agrees with the reference sets.
    assert!(admits_action(&PythonString::from_scalars("write_comment")));
    assert!(!admits_action(&PythonString::from_scalars("write-comment")));
}

#[test]
fn valid_mission_and_bootstrap_action_keep_the_writer_floor() {
    let (root, project) = fixture(
        "cf58-valid",
        &[(
            "acme",
            &activation_entry(
                "DIRECTIVE_001",
                "{mission_type: software-dev, action: implement}",
                "",
            ),
        )],
    );
    let (source, native) = run_pair(&root, &project, "directives");
    assert_eq!(
        source["fold"]["required"],
        json!([t("DIRECTIVE_001")]),
        "the valid pack survives the typed fold"
    );
    assert_eq!(
        source["fold"]["activations"],
        json!([[
            json!([
                json!([t("action"), t("implement")]),
                json!([t("mission_type"), t("software-dev")]),
            ]),
            t("pack-DIRECTIVE_001"),
            t("DIRECTIVE_001"),
            JsonValue::Null,
        ]]),
        "the admitted context is carried verbatim"
    );
    assert_eq!(
        source["stems"],
        json!([t("001-architectural-integrity-standard")])
    );
    assert_eq!(native["fold"], source["fold"]);
    assert_eq!(native["stems"], source["stems"]);
    assert_eq!(native["raw"], source["raw"]);
    assert_eq!(native["warnings"], source["warnings"]);
}

#[test]
fn fine_grained_triggers_and_wildcards_are_admitted() {
    // The values the reference accepts on `activation_context.action` beyond the
    // ten mission verbs: the four fine-grained triggers (absent from the agent
    // bootstrap list) and the two wildcards. Each pack contributes its own id, so
    // an over-strict vocabulary would silently drop one.
    let cases = [
        ("write_comment", "{action: write_comment}"),
        (
            "write_docstring",
            "{mission_type: research, action: write_docstring}",
        ),
        ("rename_identifier", "{action: rename_identifier}"),
        (
            "add_dependency",
            "{mission_type: generic, action: add_dependency}",
        ),
        ("wildcard_any", "{mission_type: any, action: any}"),
        (
            "wildcard_generic",
            "{mission_type: documentation, action: generic}",
        ),
        ("mission_plan", "{mission_type: plan, action: plan}"),
        (
            "charter_loop",
            "{mission_type: software-dev, action: charter.interview}",
        ),
    ];
    let packs: Vec<(String, String)> = cases
        .iter()
        .enumerate()
        .map(|(position, (name, context))| {
            (
                (*name).to_owned(),
                activation_entry(&format!("D{position}"), context, ""),
            )
        })
        .collect();
    let packs: Vec<(&str, &str)> = packs
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect();
    let (root, project) = fixture("cf58-admitted", &packs);
    let (source, native) = run_pair(&root, &project, "directives");
    let expected: Vec<JsonValue> = cases
        .iter()
        .enumerate()
        .map(|(position, _)| t(&format!("D{position}")))
        .collect();
    assert_eq!(
        source["fold"]["required"],
        json!(expected),
        "every admitted selector keeps its pack's required id"
    );
    assert_eq!(
        source["raw"],
        json!(expected),
        "the raw reader is independent of the typed gate"
    );
    assert_eq!(native["fold"], source["fold"]);
    assert_eq!(native["stems"], source["stems"]);
    assert_eq!(native["raw"], source["raw"]);
}

#[test]
fn typed_invalid_context_is_skipped_while_the_raw_reader_keeps_the_ids() {
    // The CF58 defect this capability closes: an unknown mission type (or action)
    // makes the reference's typed model refuse the whole pack, so its
    // `required_<kind>` lists must NOT reach the writer floor. The raw per-field
    // reader has a different role and still sees them.
    let (root, project) = fixture(
        "cf58-typed-invalid",
        &[
            (
                "good",
                &activation_entry(
                    "DIRECTIVE_001",
                    "{mission_type: software-dev, action: implement}",
                    "",
                ),
            ),
            (
                "bad-mission",
                &activation_entry("BADM", "{mission_type: bogus, action: implement}", ""),
            ),
            (
                "bad-action",
                &activation_entry("BADA", "{mission_type: research, action: bogus}", ""),
            ),
        ],
    );
    let (source, native) = run_pair(&root, &project, "directives");
    assert_eq!(
        source["fold"]["required"],
        json!([t("DIRECTIVE_001")]),
        "only the typed-valid pack contributes"
    );
    assert_eq!(
        source["stems"],
        json!([t("001-architectural-integrity-standard")])
    );
    assert_eq!(
        source["raw"],
        json!([t("DIRECTIVE_001"), t("BADM"), t("BADA")]),
        "the raw reader keeps every authored id"
    );
    assert_eq!(native["fold"], source["fold"], "the typed fold verdict");
    assert_eq!(native["stems"], source["stems"], "the writer floor");
    assert_eq!(native["raw"], source["raw"], "the raw reader is unchanged");
    assert_eq!(native["warnings"], source["warnings"]);

    // Phase check outside the child, on the pack the fold skipped: the reference
    // raises `ValidationError` from the model validator, so the typed loader
    // reports an INVALID DOCUMENT (skip), not a refusal (re-raise).
    let pack = project.join("packs/bad-mission");
    let error = sk_doctrine::load_org_charter_policy(&pack)
        .expect_err("an unknown mission type is an invalid pack document");
    assert!(
        matches!(error, sk_doctrine::DoctrineError::InvalidOrgCharter { .. }),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("activation_context.mission_type='bogus'")
            && message.contains("ALLOWED_MISSION_TYPES"),
        "the refusal names the slot and the closed vocabulary: {message}"
    );
    // The raw per-field reader still reads the same id from the project's packs.
    let raw = sk_doctrine::read_org_required_tokens_exact(&project, ArtifactKind::Directive, false)
        .expect("raw reader");
    assert!(
        raw.contains(&PythonString::from_scalars("BADM")),
        "the raw reader keeps the typed-invalid pack's id: {raw:?}"
    );
}

#[test]
fn exact_codepoints_reject_whitespace_case_control_and_surrogates() {
    // The reference matches the authored value against a frozenset, so there is no
    // strip, case-fold or alias step. Each pack below is typed-invalid; the one
    // valid pack proves the floor is not simply empty.
    let (root, project) = fixture(
        "cf58-exact-codepoints",
        &[
            (
                "good",
                &activation_entry(
                    "DIRECTIVE_001",
                    "{mission_type: research, action: review}",
                    "",
                ),
            ),
            (
                "space",
                &activation_entry("SP", "{action: ' implement '}", ""),
            ),
            (
                "case",
                &activation_entry("CASE", "{mission_type: SOFTWARE-DEV}", ""),
            ),
            (
                "empty",
                &activation_entry("EMPTY", "{mission_type: '', action: ''}", ""),
            ),
            (
                "control",
                &activation_entry("CTRL", "{\"action\": \"implement\\u0000\"}", ""),
            ),
            (
                "surrogate",
                &activation_entry("SURR", "{\"mission_type\": \"software-dev\\ud800\"}", ""),
            ),
        ],
    );
    let (source, native) = run_pair(&root, &project, "directives");
    assert_eq!(
        source["fold"]["required"],
        json!([t("DIRECTIVE_001")]),
        "no non-exact token is admitted"
    );
    assert_eq!(
        source["raw"],
        json!([
            t("DIRECTIVE_001"),
            t("SP"),
            t("CASE"),
            t("EMPTY"),
            t("CTRL"),
            t("SURR"),
        ]),
        "the raw reader keeps every authored id"
    );
    assert_eq!(native["fold"], source["fold"]);
    assert_eq!(native["stems"], source["stems"]);
    assert_eq!(native["raw"], source["raw"]);

    // A lone-surrogate mission selector is a real, exactly transported token that
    // cannot equal any roster id, so the vocabulary check (not a construction
    // abort) refuses the pack — the reference reaches the same `ValidationError`.
    let surrogate = sk_doctrine::load_org_charter_policy(&project.join("packs/surrogate"));
    assert!(
        matches!(
            surrogate,
            Err(sk_doctrine::DoctrineError::InvalidOrgCharter { .. })
        ),
        "the surrogate pack is an invalid-document skip: {surrogate:?}"
    );

    // Each ASCII refusal is an invalid-pack-document phase, not a refusal.
    for (pack, body) in [
        ("space", "activation_context.action=' implement '"),
        ("case", "activation_context.mission_type='SOFTWARE-DEV'"),
        ("empty", "activation_context.mission_type=''"),
        ("control", "activation_context.action='implement\\x00'"),
    ] {
        let error = sk_doctrine::load_org_charter_policy(&project.join("packs").join(pack))
            .expect_err("typed-invalid context");
        assert!(
            matches!(error, sk_doctrine::DoctrineError::InvalidOrgCharter { .. }),
            "{pack}: {error:?}"
        );
        assert!(error.to_string().contains(body), "{pack}: {error}");
    }
}

#[test]
fn writer_floor_and_fold_write_nothing() {
    let (root, project) = fixture(
        "cf58-no-writes",
        &[
            (
                "good",
                &activation_entry(
                    "DIRECTIVE_001",
                    "{mission_type: software-dev, action: implement}",
                    "",
                ),
            ),
            ("bad", &activation_entry("BAD", "{mission_type: bogus}", "")),
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
    let _ = read_org_required_tokens_exact(&project, ArtifactKind::Directive, false)
        .expect("raw reader");
    let _ = sk_doctrine::ActivationSelectorRegistry::canonical().expect("registry");
    assert_eq!(before, footprint(&root), "no file was created or rewritten");
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
