//! CF48: the canonical **raw** org-charter reader against the pinned reference.
//!
//! Every case runs the reference FIRST, in a child process whose CWD/HOME/XDG/
//! TMP/PYTHONPATH/packs root are all asserted from inside the child, and compares
//! the observed value with the Native raw owner. The point of the lane is that the
//! raw readers are NOT the typed whole-policy loader:
//!
//! * a malformed *unrelated* field (an ignored `schema_version`, a non-list
//!   `required_<other>`) must not erase a requested kind;
//! * the strict floor reader validates the requested field only, while
//!   `read_org_required_ids` / the selection overlay coerce members with Python
//!   `str()` — two different APIs, never one;
//! * an invalid `extends:` chain falls back to the declared pack ALONE (lenient),
//!   never a partially visited prefix;
//! * an empty/non-mapping document is a lenient skip and a strict refusal;
//! * an uncaught constructor exception escapes both postures;
//! * a missing sibling pack (and a missing built-in pack) must not erase a
//!   readable floor.
//!
//! These are behavioral equivalence checks for the exercised cases, not a
//! whole-language or workflow claim.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use sk_charter::ArtifactKind;

use sk_doctrine::{
    DoctrineError, IdentityIndex, PythonString, org_charter_raw_documents, read_org_required_ids,
    read_org_required_ids_exact, read_org_required_provenance_exact, read_org_required_selections,
    read_org_required_selections_exact, read_org_required_tokens, read_org_required_tokens_exact,
    read_org_skill_namespace, read_org_skill_namespace_exact,
};

mod support;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
    fs::write(path, text).expect("write fixture");
}

/// Copy the pinned built-in pack tree (the reference imports read its `missions/`
/// leaf at import time, so the child needs a complete built-in tier).
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("destination");
    for entry in fs::read_dir(source).expect("read source") {
        let entry = entry.expect("entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn scalars(tokens: &[PythonString]) -> Vec<String> {
    tokens
        .iter()
        .map(|token| token.to_scalar_string().expect("scalar token"))
        .collect()
}

/// The child-process environment preamble: every root is asserted in the child.
const ISOLATION: &str = r#"
import os, pathlib
from charter.offering.artifact_kinds import ArtifactKind
cwd = pathlib.Path.cwd()
assert cwd.as_posix() == os.environ["SK_EXPECT_CWD"], cwd
home = pathlib.Path(os.environ["HOME"])
assert home.as_posix() == os.environ["SK_EXPECT_HOME"], home
for key in ("XDG_CONFIG_HOME","XDG_CACHE_HOME","XDG_DATA_HOME","XDG_STATE_HOME",
            "XDG_RUNTIME_DIR","XDG_CONFIG_DIRS","XDG_DATA_DIRS","TMPDIR","TMP","TEMP"):
    value = os.environ[key]
    assert os.path.isabs(value), (key, value)
    assert value == os.environ["SK_EXPECT_SANDBOX"], (key, value)
pythonpath = os.environ["PYTHONPATH"]
assert os.path.isabs(pythonpath) and (pathlib.Path(pythonpath) / "charter").is_dir(), pythonpath
packs = pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"])
assert os.path.isabs(str(packs)), packs
assert (packs / "built-in").is_dir() == (os.environ["SK_EXPECT_BUILT_IN"] == "1"), packs
"#;

struct Fixture {
    temp: tempfile::TempDir,
    project: PathBuf,
    packs: PathBuf,
}

impl Fixture {
    /// A project whose org packs are declared in config order; `docs` maps pack
    /// name to its `org-charter.yaml` body (`None` = no file).
    fn new(docs: &[(&str, Option<&str>)], order: &[&str]) -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let project = temp.path().join("project");
        let packs = project.join("packs");
        fs::create_dir_all(&packs).expect("packs");
        copy_tree(
            &support::repository_root().join("packs/built-in"),
            &packs.join("built-in"),
        );
        let mut declarations = String::from("charter_packs:\n  org:\n    packs:\n");
        for name in order {
            let root = packs.join(name);
            fs::create_dir_all(&root).expect("pack root");
            write(
                &root.join("pack.yaml"),
                &format!("name: {name}\npack_level: org\n"),
            );
            if let Some((_, Some(body))) = docs.iter().find(|(key, _)| key == name) {
                write(&root.join("org-charter.yaml"), body);
            }
            declarations.push_str(&format!(
                "      - name: {name}\n        local_path: {}\n",
                root.display()
            ));
        }
        write(&project.join(".kittify/config.yaml"), &declarations);
        Self {
            temp,
            project,
            packs,
        }
    }

    fn sandbox(&self) -> PathBuf {
        let sandbox = self.temp.path().join("sandbox");
        fs::create_dir_all(&sandbox).expect("sandbox");
        sandbox
    }

    /// Run one reference script with a fully asserted isolated environment.
    fn source(
        &self,
        packs_root: &Path,
        require_built_in: bool,
        body: &str,
    ) -> std::process::Output {
        let sandbox = self.sandbox();
        let cwd = sandbox.join("cwd");
        fs::create_dir_all(&cwd).expect("cwd");
        let script = format!("{ISOLATION}\n{body}\n");
        let mut env: Vec<(String, String)> = support::oracle_env(packs_root);
        env.push(("HOME".to_owned(), sandbox.display().to_string()));
        for key in [
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_RUNTIME_DIR",
            "XDG_CONFIG_DIRS",
            "XDG_DATA_DIRS",
            "TMPDIR",
            "TMP",
            "TEMP",
        ] {
            env.push((key.to_owned(), sandbox.display().to_string()));
        }
        env.push(("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()));
        env.push(("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()));
        env.push(("SK_EXPECT_CWD".to_owned(), cwd.display().to_string()));
        env.push(("SK_EXPECT_HOME".to_owned(), sandbox.display().to_string()));
        env.push((
            "SK_EXPECT_SANDBOX".to_owned(),
            sandbox.display().to_string(),
        ));
        env.push((
            "SK_EXPECT_BUILT_IN".to_owned(),
            if require_built_in { "1" } else { "0" }.to_owned(),
        ));
        env.push(("SK_PROJECT".to_owned(), self.project.display().to_string()));
        Command::new(support::python())
            .arg("-c")
            .arg(script)
            .current_dir(&cwd)
            .env_clear()
            .envs(env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run reference")
    }

    fn source_json(&self, packs_root: &Path, require_built_in: bool, body: &str) -> Value {
        let output = self.source(packs_root, require_built_in, body);
        assert!(
            output.status.success(),
            "reference failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("reference JSON")
    }
}

/// The reference walk, as `member`/`declared` pairs.
const WALK: &str = r#"
import json, os
from pathlib import Path
from charter.activation.org_pack_discovery import _iter_org_charter_docs
root = Path(os.environ["SK_PROJECT"])
print(json.dumps([[declared, member] for declared, member, _raw in _iter_org_charter_docs(root)]))
"#;

#[test]
fn coercing_readers_and_the_strict_per_field_floor_are_different_apis() {
    let fixture = Fixture::new(
        &[(
            "acme",
            Some("required_directives: [1, '  D1  ', true, null]\nrequired_tactics: [T1]\n"),
        )],
        &["acme"],
    );
    let body = r#"
import json, os
from pathlib import Path
from charter.activation.org_pack_discovery import (
    _read_org_required_selections, _required_values, read_org_required_ids, read_org_required_tokens,
)
root = Path(os.environ["SK_PROJECT"])
field = {"required_directives": [1, "  D1  ", True, None]}
print(json.dumps({
    "ids": read_org_required_ids(root, ArtifactKind("directive")),
    "tokens": read_org_required_tokens(root, ArtifactKind("directive"), strict=False),
    "per_field": _required_values(field, "required_directives", "acme", strict=False),
    "selections": _read_org_required_selections(root),
}))
"#;
    let source = fixture.source_json(&fixture.packs, true, body);
    assert_eq!(
        source["ids"],
        serde_json::json!(["1", "D1", "True", "None"]),
        "the coercing reader stringifies every member"
    );
    assert_eq!(
        source["tokens"],
        serde_json::json!([]),
        "the strict per-field floor refuses a non-string member by skipping the field"
    );

    let project = &fixture.project;
    assert_eq!(
        scalars(&read_org_required_ids_exact(project, ArtifactKind::Directive).unwrap()),
        source["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        "native coercing reader matches the reference"
    );
    assert_eq!(
        read_org_required_tokens_exact(project, ArtifactKind::Directive, false).unwrap(),
        Vec::<PythonString>::new(),
        "native per-field floor matches the reference (field dropped)"
    );
    assert!(
        read_org_required_tokens(project, ArtifactKind::Directive, true).is_err(),
        "the strict per-field floor refuses the requested kind"
    );
    assert_eq!(
        read_org_required_tokens_exact(project, ArtifactKind::Tactic, false).unwrap(),
        vec![PythonString::from_scalars("T1")],
        "an unrelated malformed field cannot poison another kind"
    );
    let selections = read_org_required_selections(project).unwrap();
    assert_eq!(
        selections[&ArtifactKind::Directive],
        vec![
            "1".to_owned(),
            "D1".to_owned(),
            "True".to_owned(),
            "None".to_owned()
        ],
        "the selection overlay uses the coercing rule for directives"
    );
    assert_eq!(
        selections.len(),
        8,
        "only the overlayable kinds are overlaid"
    );
    assert!(!selections.contains_key(&ArtifactKind::Skill));
}

#[test]
fn ignored_schema_version_and_unrelated_malformed_fields_do_not_poison_a_kind() {
    let fixture = Fixture::new(
        &[(
            "acme",
            Some(
                "schema_version: not-an-int\ninterview_defaults: [not, a, mapping]\n\
                 required_directives: 7\nrequired_tactics: [T1]\nrequired_skills: [s1]\n",
            ),
        )],
        &["acme"],
    );
    let body = r#"
import json, os
from pathlib import Path
from charter.activation.org_pack_discovery import read_org_required_ids
root = Path(os.environ["SK_PROJECT"])
print(json.dumps({
    "directives": read_org_required_ids(root, ArtifactKind("directive")),
    "tactics": read_org_required_ids(root, ArtifactKind("tactic")),
    "skills": read_org_required_ids(root, ArtifactKind("skill")),
}))
"#;
    let source = fixture.source_json(&fixture.packs, true, body);
    let project = &fixture.project;
    for (kind, key) in [
        (ArtifactKind::Directive, "directives"),
        (ArtifactKind::Tactic, "tactics"),
        (ArtifactKind::Skill, "skills"),
    ] {
        assert_eq!(
            scalars(&read_org_required_ids_exact(project, kind).unwrap()),
            source[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect::<Vec<_>>(),
            "kind {key} matches the reference"
        );
    }
    assert_eq!(
        read_org_required_ids(project, ArtifactKind::Directive).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn an_invalid_extends_chain_falls_back_to_the_declared_pack_alone() {
    for (label, overlay_extends, base_extends) in
        [("dangling-middle", "b", "missing"), ("cycle", "b", "a")]
    {
        let fixture = Fixture::new(
            &[
                (
                    "a",
                    Some(&format!(
                        "extends: {overlay_extends}\nrequired_directives: [A1]\n"
                    )),
                ),
                (
                    "b",
                    Some(&format!(
                        "extends: {base_extends}\nrequired_directives: [B1]\n"
                    )),
                ),
            ],
            &["a", "b"],
        );
        let source = fixture.source_json(&fixture.packs, true, WALK);
        assert_eq!(
            source,
            serde_json::json!([["a", "a"], ["b", "b"]]),
            "{label}: the reference falls back to the declared pack alone"
        );
        let native: Vec<(String, String)> = org_charter_raw_documents(&fixture.project, false)
            .unwrap()
            .into_iter()
            .map(|document| (document.declared().to_owned(), document.member().to_owned()))
            .collect();
        assert_eq!(
            serde_json::to_value(&native).unwrap(),
            source,
            "{label}: native must not walk a partially visited prefix"
        );
        assert!(
            org_charter_raw_documents(&fixture.project, true).is_err(),
            "{label}: the strict posture refuses the chain"
        );
    }
}

#[test]
fn empty_and_non_mapping_documents_are_lenient_skips_and_strict_refusals() {
    for (label, body) in [("empty", ""), ("list", "- a list\n"), ("null", "null\n")] {
        let fixture = Fixture::new(&[("acme", Some(body))], &["acme"]);
        let source = fixture.source_json(&fixture.packs, true, WALK);
        assert_eq!(source, serde_json::json!([]), "{label}: nothing is walked");
        assert!(
            org_charter_raw_documents(&fixture.project, false)
                .unwrap()
                .is_empty(),
            "{label}: lenient skips the pack"
        );
        let error = org_charter_raw_documents(&fixture.project, true).unwrap_err();
        assert!(
            matches!(error, DoctrineError::InvalidOrgCharter { .. }),
            "{label}: strict refuses with the caught phase, got {error}"
        );
        assert!(
            error.to_string().contains("is not a YAML mapping"),
            "{label}: the refusal names the non-mapping document, got {error}"
        );
    }
}

#[test]
fn an_uncaught_constructor_failure_escapes_every_posture() {
    let fixture = Fixture::new(
        &[("acme", Some("required_directives: [!!bool notabool]\n"))],
        &["acme"],
    );
    let body = r#"
import json, os
from pathlib import Path
from charter.activation.org_pack_discovery import _iter_org_charter_docs
root = Path(os.environ["SK_PROJECT"])
try:
    list(_iter_org_charter_docs(root))
except KeyError as exc:
    print(json.dumps({"escaped": type(exc).__name__, "message": str(exc)}))
else:
    print(json.dumps({"escaped": None}))
"#;
    let source = fixture.source_json(&fixture.packs, true, body);
    assert_eq!(
        source["escaped"],
        serde_json::json!("KeyError"),
        "the reference does not catch this constructor failure"
    );
    for strict in [false, true] {
        let error = org_charter_raw_documents(&fixture.project, strict).unwrap_err();
        match error {
            DoctrineError::OrgCharterReadAborted {
                exception, message, ..
            } => {
                assert_eq!(exception, "KeyError", "strict={strict}");
                assert!(message.contains("notabool"), "strict={strict}: {message}");
            }
            other => panic!("strict={strict}: expected an aborted read, got {other}"),
        }
    }
}

#[test]
fn a_missing_sibling_pack_does_not_erase_the_readable_floor() {
    // The reference walks the CONFIGURED registry, so a declared pack that is not
    // on disk contributes nothing and never shadows a readable sibling. (The
    // built-in tier's availability is likewise irrelevant to this read; the
    // module-local regression pins that Native no longer routes it through the
    // built-in search.)
    let fixture = Fixture::new(
        &[
            ("ghost", Some("required_directives: [G1]\n")),
            ("real", Some("required_directives: [R1]\n")),
        ],
        &["ghost", "real"],
    );
    fs::remove_dir_all(fixture.packs.join("ghost")).expect("remove ghost");
    let body = r#"
import json, os
from pathlib import Path
from charter.activation.org_pack_discovery import _iter_org_charter_docs, read_org_required_ids
root = Path(os.environ["SK_PROJECT"])
print(json.dumps({
    "walk": [[d, m] for d, m, _raw in _iter_org_charter_docs(root)],
    "directives": read_org_required_ids(root, ArtifactKind("directive")),
}))
"#;
    let source = fixture.source_json(&fixture.packs, true, body);
    assert_eq!(source["walk"], serde_json::json!([["real", "real"]]));
    assert_eq!(source["directives"], serde_json::json!(["R1"]));

    let project = &fixture.project;
    let native_walk: Vec<(String, String)> = org_charter_raw_documents(project, false)
        .unwrap()
        .into_iter()
        .map(|document| (document.declared().to_owned(), document.member().to_owned()))
        .collect();
    assert_eq!(serde_json::to_value(&native_walk).unwrap(), source["walk"]);
    assert_eq!(
        scalars(&read_org_required_ids_exact(project, ArtifactKind::Directive).unwrap()),
        vec!["R1".to_owned()]
    );
    // The strict posture still refuses a declared pack that is not a directory,
    // in declaration order (the first declared pack is `ghost`).
    let error = org_charter_raw_documents(project, true).unwrap_err();
    assert!(error.to_string().contains("ghost"), "{error}");
}

#[test]
fn the_raw_readers_are_side_effect_free() {
    let fixture = Fixture::new(
        &[(
            "acme",
            Some("required_directives: [D1]\nskill_namespace: acme\n"),
        )],
        &["acme"],
    );
    let before = snapshot(&fixture.project);
    let project = &fixture.project;
    org_charter_raw_documents(project, true).unwrap();
    org_charter_raw_documents(project, false).unwrap();
    read_org_required_tokens_exact(project, ArtifactKind::Directive, true).unwrap();
    read_org_required_ids_exact(project, ArtifactKind::Directive).unwrap();
    read_org_required_selections_exact(project).unwrap();
    read_org_skill_namespace_exact(project).unwrap();
    let identity = IdentityIndex::from_roots(
        &support::repository_root().join("packs/built-in"),
        std::slice::from_ref(&fixture.packs.join("acme")),
        None,
    );
    read_org_required_provenance_exact(project, ArtifactKind::Directive, &identity, true).unwrap();
    assert_eq!(
        snapshot(project),
        before,
        "the readers must not write a byte"
    );
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, path: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(path)
            .map(|entries| {
                entries
                    .map(|entry| entry.expect("entry").path())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        entries.sort();
        for entry in entries {
            if entry.is_dir() {
                out.push((
                    entry
                        .strip_prefix(base)
                        .expect("inside root")
                        .display()
                        .to_string(),
                    Vec::new(),
                ));
                walk(base, &entry, out);
            } else {
                out.push((
                    entry
                        .strip_prefix(base)
                        .expect("inside root")
                        .display()
                        .to_string(),
                    fs::read(&entry).expect("read file"),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn inherited_requirements_are_walked_declared_first_then_nearest_base() {
    let fixture = Fixture::new(
        &[
            ("base", Some("required_directives: [BASE1]\n")),
            (
                "overlay",
                Some("extends: base\nrequired_directives: [OVER1, BASE1]\n"),
            ),
        ],
        &["overlay", "base"],
    );
    let source = fixture.source_json(&fixture.packs, true, WALK);
    assert_eq!(
        source,
        serde_json::json!([
            ["overlay", "overlay"],
            ["overlay", "base"],
            ["base", "base"]
        ])
    );
    let native: Vec<(String, String)> = org_charter_raw_documents(&fixture.project, true)
        .unwrap()
        .into_iter()
        .map(|document| (document.declared().to_owned(), document.member().to_owned()))
        .collect();
    assert_eq!(serde_json::to_value(&native).unwrap(), source);
    let body = r#"
import json, os
from pathlib import Path
from charter.activation.org_pack_discovery import read_org_required_ids
print(json.dumps(read_org_required_ids(Path(os.environ["SK_PROJECT"]), ArtifactKind("directive"))))
"#;
    let source_ids = fixture.source_json(&fixture.packs, true, body);
    assert_eq!(source_ids, serde_json::json!(["OVER1", "BASE1"]));
    assert_eq!(
        scalars(&read_org_required_ids_exact(&fixture.project, ArtifactKind::Directive).unwrap()),
        vec!["OVER1".to_owned(), "BASE1".to_owned()],
        "first-seen order across the declared-first chain"
    );
}

#[test]
fn exact_tokens_and_exact_provenance_keep_code_points_without_inventing_spellings() {
    let fixture = Fixture::new(
        &[(
            "acme",
            Some("required_agent_profiles: [\"ghost\\ud800\", ok-id]\n"),
        )],
        &["acme"],
    );
    let body = r#"
import json, os
from pathlib import Path
from charter.activation.org_pack_discovery import read_org_required_ids
tokens = read_org_required_ids(Path(os.environ["SK_PROJECT"]), ArtifactKind("agent_profile"))
print(json.dumps([[ord(ch) for ch in token] for token in tokens]))
"#;
    let source = fixture.source_json(&fixture.packs, true, body);
    assert_eq!(
        source,
        serde_json::json!([
            [0x67, 0x68, 0x6f, 0x73, 0x74, 0xd800],
            [0x6f, 0x6b, 0x2d, 0x69, 0x64]
        ])
    );
    let project = &fixture.project;
    let exact = read_org_required_ids_exact(project, ArtifactKind::AgentProfile).unwrap();
    assert_eq!(exact.len(), 2);
    assert_eq!(
        exact[0].codepoints(),
        &[0x67, 0x68, 0x6f, 0x73, 0x74, 0xd800]
    );
    assert_eq!(
        read_org_required_ids(project, ArtifactKind::AgentProfile).unwrap(),
        vec!["ok-id".to_owned()],
        "the scalar view names its capability boundary instead of inventing a spelling"
    );

    let identity = IdentityIndex::from_roots(
        &support::repository_root().join("packs/built-in"),
        std::slice::from_ref(&fixture.packs.join("acme")),
        None,
    );
    let provenance =
        read_org_required_provenance_exact(project, ArtifactKind::AgentProfile, &identity, false)
            .unwrap();
    let keys: Vec<Vec<u32>> = provenance
        .keys()
        .map(|token| token.codepoints().to_vec())
        .collect();
    let expected = vec![0x67u32, 0x68, 0x6f, 0x73, 0x74, 0xd800];
    assert!(
        keys.contains(&expected),
        "an unresolved non-scalar token stays in the exact provenance map: {keys:?}"
    );
}

#[test]
fn documents_and_namespace_are_read_from_the_declared_packs_only() {
    let fixture = Fixture::new(
        &[
            (
                "base",
                Some("skill_namespace: from-base\nrequired_directives: [BASE1]\n"),
            ),
            (
                "overlay",
                Some("extends: base\nskill_namespace: from-overlay\n"),
            ),
        ],
        &["base", "overlay"],
    );
    let source = fixture.source_json(
        &fixture.packs,
        true,
        r#"
import json, os
from pathlib import Path
from charter.activation.org_pack_discovery import read_org_skill_namespace
print(json.dumps(read_org_skill_namespace(Path(os.environ["SK_PROJECT"]))))
"#,
    );
    // `overlay` is declared last and wins; `from-base` is inherited only as a
    // chain member, and the namespace fold reads DECLARED documents only.
    assert_eq!(source, serde_json::json!("from-overlay"));
    let project = &fixture.project;
    assert_eq!(
        read_org_skill_namespace(project).unwrap(),
        Some("from-overlay".to_owned())
    );
    assert_eq!(
        read_org_skill_namespace_exact(project).unwrap(),
        Some(PythonString::from_scalars("from-overlay"))
    );
    // The projected document view keeps each member's own requirements.
    let documents = org_charter_raw_documents(project, false).unwrap();
    let members: BTreeMap<String, Vec<String>> = documents
        .iter()
        .map(|document| {
            (
                format!("{}:{}", document.declared(), document.member()),
                scalars(
                    &document
                        .requirement_tokens(ArtifactKind::Directive, false)
                        .unwrap(),
                ),
            )
        })
        .collect();
    assert_eq!(members["overlay:base"], vec!["BASE1".to_owned()]);
    assert_eq!(members["base:base"], vec!["BASE1".to_owned()]);
    assert!(members["overlay:overlay"].is_empty());

    let selections = read_org_required_selections_exact(project).unwrap();
    assert_eq!(
        scalars(&selections[&ArtifactKind::Directive]),
        vec!["BASE1".to_owned()]
    );
    assert!(selections.contains_key(&ArtifactKind::Tactic));
    assert!(!selections.contains_key(&ArtifactKind::GlossaryPack));
    assert!(!selections.contains_key(&ArtifactKind::Asset));
}
