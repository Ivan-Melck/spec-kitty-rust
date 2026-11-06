//! Live, lock-exact oracle differential tests for the native language-scope
//! service (`sk_doctrine::language_vocabulary` + `sk_doctrine::language_scope`).
//!
//! Every expected value is produced at test time by the pinned Python oracle
//! (`support::python()` verifies the lock first) against the **same** temporary
//! fixture the native side reads: a copy of the checkout's `packs/built-in`, a
//! temporary project and, where the case needs one, a temporary org pack. No
//! expectation is hand-frozen, and nothing here writes into the checkout.
//!
//! The comparisons are exact full values (whole sorted vocabulary sets, whole
//! language lists/`None`, whole placeholder booleans). Each scope test also has
//! a non-vacuous control: the scoped artifact is removed and both sides must
//! change together, so a provider that silently returned "everything" or
//! "nothing" could not pass.
//!
//! The reference keys its vocabulary memo on `repo_root` only, because its packs
//! root is ambient; the native service also keys the explicit packs-root seam.
//! The parent-vs-leaf packs-root parity checked here is therefore a native seam
//! property (already accepted for the org floor/context roots), not reference
//! behaviour.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::{Value, json};
use sk_doctrine::{
    InferRequest, InferredLanguages, InterviewAnswers, LanguageScopeService,
    LanguageVocabularyCache, SHIPPED_DEFAULT_ANSWERS, scoped_language_vocabulary,
};

struct Fixture {
    _temp: tempfile::TempDir,
    /// The pack **parent** (`<temp>/packs`), as `SPEC_KITTY_PACKS_ROOT` names it.
    packs: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn root(&self) -> &Path {
        self._temp.path()
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let source = entry.path();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&source, &target);
        } else if entry.file_type().unwrap().is_file() {
            fs::copy(&source, &target).unwrap();
        }
    }
}

/// A temporary project plus a temporary copy of the shipped built-in pack.
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let packs = temp.path().join("packs");
    let project = temp.path().join("project");
    copy_tree(
        &support::repository_root().join("packs/built-in"),
        &packs.join("built-in"),
    );
    fs::create_dir_all(&project).unwrap();
    Fixture {
        _temp: temp,
        packs,
        project,
    }
}

/// One oracle operation. The reference driver below dispatches on `op`.
fn op_vocabulary(repo: Option<&Path>) -> Value {
    json!({"op": "vocabulary", "repo": repo.map(|path| path.display().to_string())})
}

fn op_guidance(repo: Option<&Path>, languages: Option<&[&str]>) -> Value {
    json!({
        "op": "guidance",
        "repo": repo.map(|path| path.display().to_string()),
        "languages": languages,
    })
}

fn op_declared(text: &str) -> Value {
    json!({"op": "declared", "text": text})
}

fn op_placeholder(value: &str) -> Value {
    json!({"op": "placeholder", "value": value})
}

fn op_infer(
    repo: Option<&Path>,
    answers_yaml: Option<&str>,
    shipped_default: bool,
    prefer: bool,
) -> Value {
    json!({
        "op": "infer",
        "repo": repo.map(|path| path.display().to_string()),
        "answers_yaml": answers_yaml,
        "shipped_default": shipped_default,
        "prefer": prefer,
    })
}

/// The reference driver. It runs in the temporary project with
/// `PYTHONPATH=<repo>/src` and `SPEC_KITTY_PACKS_ROOT=<packs>`, so the reference
/// and the native side read the same packs tree.
const ORACLE_SCRIPT: &str = r#"
import json
import sys
from pathlib import Path

from ruamel.yaml import YAML

from charter.activation.interview import CharterInterview, default_interview
from charter.activation.language_scope import (
    extract_declared_languages,
    infer_repo_languages,
    is_placeholder_language_answer,
    lacks_specialist_guidance,
)
from charter.activation.language_vocabulary import scoped_language_vocabulary

spec = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))


def maybe_path(value):
    return None if value is None else Path(value)


results = []
for op in spec:
    kind = op["op"]
    if kind == "vocabulary":
        scoped_language_vocabulary.cache_clear()
        results.append(sorted(scoped_language_vocabulary(maybe_path(op["repo"]))))
    elif kind == "guidance":
        scoped_language_vocabulary.cache_clear()
        results.append(lacks_specialist_guidance(op["languages"], maybe_path(op["repo"])))
    elif kind == "declared":
        results.append(extract_declared_languages(op["text"]))
    elif kind == "placeholder":
        results.append(is_placeholder_language_answer(op["value"]))
    elif kind == "infer":
        scoped_language_vocabulary.cache_clear()
        interview = None
        if op.get("shipped_default"):
            interview = default_interview(mission="software-dev", profile="minimal")
        elif op.get("answers_yaml") is not None:
            answers = YAML(typ="safe").load(op["answers_yaml"]) or {}
            interview = CharterInterview.from_dict(
                {"answers": {str(key): value for key, value in answers.items()}}
            )
        results.append(
            infer_repo_languages(
                maybe_path(op["repo"]),
                interview=interview,
                prefer_interview=bool(op.get("prefer")),
            )
        )
    else:
        raise SystemExit("unknown op: " + str(kind))

print(json.dumps(results))
"#;

/// Run the reference driver over `ops` against `packs` and return its results.
fn run_oracle(fixture: &Fixture, ops: &Value, packs: &Path) -> Vec<Value> {
    let scratch = tempfile::tempdir_in(fixture.root()).unwrap();
    let spec_path = scratch.path().join("ops.json");
    fs::write(&spec_path, serde_json::to_vec(ops).unwrap()).unwrap();
    let output = Command::new(support::python())
        .arg("-c")
        .arg(ORACLE_SCRIPT)
        .arg(&spec_path)
        .current_dir(&fixture.project)
        .envs(support::oracle_env(packs))
        .output()
        .expect("spawn python oracle");
    assert!(
        output.status.success(),
        "oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("oracle emitted JSON");
    value.as_array().expect("oracle emitted a list").clone()
}

/// The native vocabulary as the oracle reports it (sorted list).
fn native_vocabulary(repo: Option<&Path>, packs: &Path) -> Value {
    let vocabulary = scoped_language_vocabulary(repo, Some(packs)).expect("native vocabulary");
    json!(vocabulary.into_iter().collect::<Vec<String>>())
}

fn native_guidance(repo: Option<&Path>, packs: &Path, languages: Option<&[&str]>) -> Value {
    let owned: Option<Vec<String>> =
        languages.map(|items| items.iter().map(|item| (*item).to_owned()).collect());
    json!(
        sk_doctrine::lacks_specialist_guidance(owned.as_deref(), repo, Some(packs))
            .expect("native guidance")
    )
}

fn native_infer(
    repo: Option<&Path>,
    packs: &Path,
    answers_yaml: Option<&str>,
    shipped_default: bool,
    prefer: bool,
) -> Value {
    let interview = if shipped_default {
        Some(shipped_default_interview())
    } else {
        answers_yaml.map(|yaml| {
            let value: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(yaml).expect("case answers YAML");
            InterviewAnswers::from_yaml_answers(&value)
        })
    };
    let request = InferRequest {
        repo_root: repo,
        interview: interview.as_ref(),
        prefer_interview: prefer,
        packs_root: Some(packs),
    };
    let inferred = LanguageScopeService::new()
        .infer(&request)
        .expect("native infer");
    json!(inferred.into_option())
}

/// `default_interview(profile="minimal")`'s answers: the first seven shipped
/// answers, in `MINIMAL_QUESTION_ORDER`.
fn shipped_default_interview() -> InterviewAnswers {
    const MINIMAL_QUESTION_ORDER: [&str; 7] = [
        "project_intent",
        "languages_frameworks",
        "testing_requirements",
        "quality_gates",
        "review_policy",
        "performance_targets",
        "deployment_constraints",
    ];
    InterviewAnswers::new(MINIMAL_QUESTION_ORDER.iter().filter_map(|key| {
        SHIPPED_DEFAULT_ANSWERS
            .iter()
            .find(|(question, _)| question == key)
            .map(|(question, answer)| ((*question).to_owned(), (*answer).to_owned()))
    }))
}

fn assert_vocabulary_matches(
    native: &Value,
    expected: &Value,
    must_contain: &[&str],
    must_not_contain: &[&str],
) {
    assert_eq!(native, expected, "exact scoped-language vocabulary");
    let tokens: Vec<&str> = expected
        .as_array()
        .expect("vocabulary array")
        .iter()
        .map(|token| token.as_str().expect("token string"))
        .collect();
    for token in must_contain {
        assert!(
            tokens.contains(token),
            "non-vacuity: {token:?} must be in the shipped/overlay vocabulary {tokens:?}"
        );
    }
    for token in must_not_contain {
        assert!(
            !tokens.contains(token),
            "{token:?} must not be vocabulary: {tokens:?}"
        );
    }
}

#[test]
fn bom_artifacts_keep_their_scoped_languages() {
    let fixture = fixture();
    let artifact = fixture
        .project
        .join(".kittify/charter-packs/tactic/bom.tactic.yaml");
    write(
        &artifact,
        "\u{feff}id: bom\napplies_to_languages: [bomlang]\n",
    );
    let expected = run_oracle(
        &fixture,
        &json!([op_vocabulary(Some(&fixture.project))]),
        &fixture.packs,
    );
    assert_vocabulary_matches(
        &native_vocabulary(Some(&fixture.project), &fixture.packs),
        &expected[0],
        &["bomlang"],
        &[],
    );
    fs::remove_file(artifact).unwrap();
    assert!(
        !native_vocabulary(Some(&fixture.project), &fixture.packs)
            .as_array()
            .unwrap()
            .contains(&json!("bomlang"))
    );
}

#[test]
fn bom_config_compiled_and_interview_tiers_match_the_oracle() {
    let fixture = fixture();
    let mut differences = Vec::new();
    for (relative, text, expected_tokens) in [
        (
            ".kittify/config.yaml",
            "\u{feff}other: yes\nlanguages: [python]\n",
            json!(["python"]),
        ),
        (
            ".kittify/charter/charter.yaml",
            "\u{feff}metadata: {}\ncatalog:\n  languages: [zig]\n",
            json!(["zig"]),
        ),
        (
            ".kittify/charter/interview/answers.yaml",
            "\u{feff}mission: software-dev\nprofile: minimal\nanswers:\n  languages_frameworks: zig\n",
            json!(["unknown"]),
        ),
    ] {
        let path = fixture.project.join(relative);
        write(&path, text);
        let expected = run_oracle(
            &fixture,
            &json!([op_infer(Some(&fixture.project), None, false, false)]),
            &fixture.packs,
        );
        assert_eq!(
            expected[0], expected_tokens,
            "oracle positive control: {relative}"
        );
        let native = native_infer(Some(&fixture.project), &fixture.packs, None, false, false);
        if native != expected[0] {
            differences.push(format!(
                "{relative}: native {native}, oracle {}",
                expected[0]
            ));
        }
        fs::remove_file(path).unwrap();
    }
    assert!(
        differences.is_empty(),
        "BOM tier differences: {differences:?}"
    );
}

#[test]
fn an_unknown_yaml_tag_discards_the_whole_artifact() {
    let fixture = fixture();
    let artifact = fixture
        .project
        .join(".kittify/charter-packs/tactic/tagged.tactic.yaml");
    write(
        &artifact,
        "id: tagged\napplies_to_languages: [!custom taggedlang, plainlang]\n",
    );
    let expected = run_oracle(
        &fixture,
        &json!([op_vocabulary(Some(&fixture.project))]),
        &fixture.packs,
    );
    assert_vocabulary_matches(
        &native_vocabulary(Some(&fixture.project), &fixture.packs),
        &expected[0],
        &[],
        &["taggedlang", "plainlang"],
    );
    write(
        &artifact,
        "id: tagged\napplies_to_languages: [!!str taggedlang, plainlang]\n",
    );
    let positive = run_oracle(
        &fixture,
        &json!([op_vocabulary(Some(&fixture.project))]),
        &fixture.packs,
    );
    assert_vocabulary_matches(
        &native_vocabulary(Some(&fixture.project), &fixture.packs),
        &positive[0],
        &["taggedlang", "plainlang"],
        &[],
    );
}

#[test]
fn org_registry_is_not_disabled_by_unrelated_config_fields() {
    let fixture = fixture();
    let org = fixture.root().join("org");
    write(
        &org.join("tactics/org.tactic.yaml"),
        "id: org\napplies_to_languages: [orglang]\n",
    );
    for extra in [
        "charter: 123\n",
        "charter_packs:\n  project:\n    skill_namespace: '  '\n",
    ] {
        let text = if extra.starts_with("charter:") {
            format!(
                "{extra}charter_packs:\n  org:\n    packs:\n      - name: org\n        local_path: {}\n",
                org.display()
            )
        } else {
            format!(
                "{extra}  org:\n    packs:\n      - name: org\n        local_path: {}\n",
                org.display()
            )
        };
        write(&fixture.project.join(".kittify/config.yaml"), &text);
        let expected = run_oracle(
            &fixture,
            &json!([op_vocabulary(Some(&fixture.project))]),
            &fixture.packs,
        );
        assert_vocabulary_matches(
            &native_vocabulary(Some(&fixture.project), &fixture.packs),
            &expected[0],
            &["orglang"],
            &[],
        );
    }
}

#[test]
fn shipped_vocabulary_and_guidance_match_the_oracle() {
    let fixture = fixture();
    let project = fixture.project.as_path();
    let ops = json!([
        op_vocabulary(Some(project)),
        op_vocabulary(None),
        op_guidance(Some(project), None),
        op_guidance(Some(project), Some(&[])),
        op_guidance(Some(project), Some(&["unknown"])),
        op_guidance(Some(project), Some(&["python"])),
        op_guidance(Some(project), Some(&["rust"])),
        op_guidance(Some(project), Some(&["python", "unknown"])),
        op_guidance(Some(project), Some(&["jvm"])),
        op_guidance(Some(project), Some(&["kotlin"])),
        op_guidance(Some(project), Some(&["java"])),
    ]);
    let expected = run_oracle(&fixture, &ops, &fixture.packs);

    assert_vocabulary_matches(
        &native_vocabulary(Some(project), &fixture.packs),
        &expected[0],
        &["python"],
        &["any", "all", "unknown"],
    );
    assert_eq!(
        native_vocabulary(None, &fixture.packs),
        expected[1],
        "repo_root=None reads the built-in packs only"
    );
    assert_eq!(
        native_vocabulary(Some(project), &fixture.packs),
        native_vocabulary(Some(project), &fixture.packs.join("built-in")),
        "the parent and built-in-leaf packs-root spellings must agree"
    );

    let guidance_cases: [Option<&[&str]>; 8] = [
        None,
        Some(&[]),
        Some(&["unknown"]),
        Some(&["python"]),
        Some(&["rust"]),
        Some(&["python", "unknown"]),
        Some(&["jvm"]),
        Some(&["kotlin"]),
    ];
    for (index, languages) in guidance_cases.iter().enumerate() {
        assert_eq!(
            native_guidance(Some(project), &fixture.packs, *languages),
            expected[index + 2],
            "guidance for {languages:?}"
        );
    }
    // Non-vacuous: the matrix really contains both verdicts.
    assert_eq!(
        expected[4],
        json!(true),
        "no shipped rust doctrine → advisory"
    );
    assert_eq!(
        expected[5],
        json!(false),
        "shipped python doctrine → not advisory"
    );
    assert!(!guidance_cases.is_empty());
}

#[test]
fn project_overlay_scope_union_and_removal_match_the_oracle() {
    let fixture = fixture();
    let project = fixture.project.as_path();
    let tactic = ".kittify/charter-packs/tactic/zig-tactic.tactic.yaml";
    write(
        &fixture.project.join(tactic),
        "id: zig-tactic\napplies_to_languages: [zig]\n",
    );

    let zig_answer = "languages_frameworks: zig";
    let combined_answer = "languages_frameworks: Python and zig";
    let present_ops = json!([
        op_vocabulary(Some(project)),
        op_vocabulary(None),
        op_infer(Some(project), Some(zig_answer), false, false),
        op_infer(Some(project), Some(combined_answer), false, false),
        op_infer(Some(project), Some(zig_answer), false, true),
        op_guidance(Some(project), Some(&["zig"])),
    ]);
    let present = run_oracle(&fixture, &present_ops, &fixture.packs);

    assert_vocabulary_matches(
        &native_vocabulary(Some(project), &fixture.packs),
        &present[0],
        &["zig", "python"],
        &["any", "all", "unknown"],
    );
    assert!(
        !present[1]
            .as_array()
            .unwrap()
            .iter()
            .any(|token| token == "zig"),
        "the project overlay must not leak into the built-in-only read"
    );
    assert_eq!(
        present[2],
        json!(["zig"]),
        "vocabulary union recognises zig"
    );
    assert_eq!(
        present[3],
        json!(["python", "zig"]),
        "built-in hits and vocabulary hits are unioned"
    );
    for (index, (answers, prefer)) in [
        (Some(zig_answer), false),
        (Some(combined_answer), false),
        (Some(zig_answer), true),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            native_infer(Some(project), &fixture.packs, answers, false, prefer),
            present[index + 2],
            "inference over the project overlay"
        );
    }
    assert_eq!(
        native_guidance(Some(project), &fixture.packs, Some(&["zig"])),
        present[5],
        "the overlay gives zig specialist guidance"
    );
    assert_eq!(present[5], json!(false));

    // Non-vacuous control: remove the only scoped artifact; both sides change.
    fs::remove_file(fixture.project.join(tactic)).unwrap();
    let removed_ops = json!([
        op_vocabulary(Some(project)),
        op_infer(Some(project), Some(zig_answer), false, false),
        op_guidance(Some(project), Some(&["zig"])),
    ]);
    let removed = run_oracle(&fixture, &removed_ops, &fixture.packs);
    assert_vocabulary_matches(
        &native_vocabulary(Some(project), &fixture.packs),
        &removed[0],
        &["python"],
        &["zig"],
    );
    assert_eq!(
        removed[1],
        json!(["unknown"]),
        "without the scope, a real declaration is unknown"
    );
    assert_eq!(
        native_infer(
            Some(project),
            &fixture.packs,
            Some(zig_answer),
            false,
            false
        ),
        removed[1]
    );
    assert_eq!(removed[2], json!(true));
    assert_eq!(
        native_guidance(Some(project), &fixture.packs, Some(&["zig"])),
        removed[2]
    );
}

#[test]
fn org_chain_scopes_both_spellings_and_removal_match_the_oracle() {
    let fixture = fixture();
    let project = fixture.project.as_path();
    let org = fixture.root().join("org-pack");
    write(
        &org.join("tactics/zig-org.tactic.yaml"),
        "id: zig-org\napplies_to_languages: [zig]\n",
    );
    write(
        &org.join("tactic/zig-singular.tactic.yaml"),
        "id: zig-singular\napplies_to_languages: [zigzag]\n",
    );
    write(
        &org.join("agent_profiles/zig-zoe.agent.yaml"),
        "profile-id: zig-zoe\nname: Zig Zoe\npurpose: review\nroles: [reviewer]\nspecialization:\n  primary-focus: zig\napplies_to_languages: [zigpro]\n",
    );
    write(
        &fixture.project.join(".kittify/config.yaml"),
        &format!(
            "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
            org.display()
        ),
    );

    let present_ops = json!([
        op_vocabulary(Some(project)),
        op_infer(
            Some(project),
            Some("languages_frameworks: zigzag"),
            false,
            false
        ),
    ]);
    let present = run_oracle(&fixture, &present_ops, &fixture.packs);
    assert_vocabulary_matches(
        &native_vocabulary(Some(project), &fixture.packs),
        &present[0],
        &["zig", "zigzag", "zigpro", "python"],
        &["any", "all", "unknown"],
    );
    assert_eq!(
        native_infer(
            Some(project),
            &fixture.packs,
            Some("languages_frameworks: zigzag"),
            false,
            false
        ),
        present[1]
    );

    // Removing the plural-spelling tactic drops `zig` but keeps `zigzag`.
    fs::remove_file(org.join("tactics/zig-org.tactic.yaml")).unwrap();
    let partial_ops = json!([op_vocabulary(Some(project))]);
    let partial = run_oracle(&fixture, &partial_ops, &fixture.packs);
    assert_vocabulary_matches(
        &native_vocabulary(Some(project), &fixture.packs),
        &partial[0],
        &["zigzag", "zigpro"],
        &["zig"],
    );

    // Removing the org pack root entirely drops the org contribution.
    fs::remove_dir_all(&org).unwrap();
    let absent_ops = json!([op_vocabulary(Some(project))]);
    let absent = run_oracle(&fixture, &absent_ops, &fixture.packs);
    assert_vocabulary_matches(
        &native_vocabulary(Some(project), &fixture.packs),
        &absent[0],
        &["python"],
        &["zig", "zigzag", "zigpro"],
    );
}

#[test]
fn kind_globs_and_unusual_scope_literals_match_the_oracle() {
    let fixture = fixture();
    let project = fixture.project.as_path();
    let overlay = ".kittify/charter-packs";
    write(
        &fixture
            .project
            .join(format!("{overlay}/tactic/mixed.tactic.yaml")),
        "id: mixed\napplies_to_languages: [zig, \"Python \", 12, null, true, \"a b\", \"\", \"c#\", \"c++\", \"objective-c\", \"MIXED\", \"any\", \"ALL\", \"unknown\"]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/tactic/nested/deep.tactic.yaml")),
        "id: deep\napplies_to_languages: [elm]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/tactic/wrong.styleguide.yaml")),
        "id: wrong\napplies_to_languages: [wrongkind]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/tactic/malformed.tactic.yaml")),
        "applies_to_languages: [unterminated\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/tactic/scalar.tactic.yaml")),
        "id: scalar\napplies_to_languages: rust\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/tactic/sequence.tactic.yaml")),
        "- applies_to_languages\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/tactic/comment.tactic.yaml")),
        "# applies_to_languages: [commentonly]\nid: comment\n",
    );
    write_bytes(
        &fixture
            .project
            .join(format!("{overlay}/tactic/nonutf8.tactic.yaml")),
        b"\xff\xfeapplies_to_languages: [brokenbytes]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/styleguide/s.styleguide.yaml")),
        "id: s\napplies_to_languages: [svelte]\n",
    );
    // The project overlay reads one directory spelling per kind (singular for
    // styleguide/tactic/procedure, plural for toolguides/agent_profiles); the
    // other spelling is not scanned there (only on the org chain).
    write(
        &fixture
            .project
            .join(format!("{overlay}/styleguides/plural.styleguide.yaml")),
        "id: plural\napplies_to_languages: [pluralonly]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/toolguides/t.toolguide.yaml")),
        "id: t\napplies_to_languages: [toolz]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/procedure/p.procedure.yaml")),
        "id: p\napplies_to_languages: [procz]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/agent_profiles/a.agent.yaml")),
        "profile-id: a\nname: A\npurpose: review\nroles: [reviewer]\nspecialization:\n  primary-focus: a\napplies_to_languages: [agentz]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/directive/d.directive.yaml")),
        "id: d\napplies_to_languages: [directiveonly]\n",
    );
    write(
        &fixture
            .project
            .join(format!("{overlay}/tactic/.hidden.tactic.yaml")),
        "id: hidden\napplies_to_languages: [dotlang]\n",
    );
    // A directory whose name looks like an artifact is not a readable file.
    fs::create_dir_all(
        fixture
            .project
            .join(format!("{overlay}/tactic/directory.tactic.yaml")),
    )
    .unwrap();

    let ops = json!([
        op_vocabulary(Some(project)),
        op_infer(
            Some(project),
            Some("languages_frameworks: Mixed and dotlang"),
            false,
            false
        )
    ]);
    let expected = run_oracle(&fixture, &ops, &fixture.packs);

    assert_vocabulary_matches(
        &native_vocabulary(Some(project), &fixture.packs),
        &expected[0],
        &[
            "zig", "python", "mixed", "elm", "svelte", "toolz", "procz", "agentz", "dotlang",
        ],
        &[
            "wrongkind",
            "pluralonly",
            "commentonly",
            "directiveonly",
            "brokenbytes",
            "any",
            "all",
            "unknown",
            "c",
            "a b",
        ],
    );
    assert_eq!(
        native_infer(
            Some(project),
            &fixture.packs,
            Some("languages_frameworks: Mixed and dotlang"),
            false,
            false
        ),
        expected[1],
        "only whole-word vocabulary hits are recognised"
    );
    assert_eq!(expected[1], json!(["dotlang", "mixed"]));
}

#[test]
fn project_layer_candidates_match_the_oracle() {
    let fixture = fixture();
    // No `.kittify/charter-packs`: the code-local layer is the fallback
    // candidate, and the retired repo-root `doctrine/` directory is not one.
    let legacy_project = fixture.root().join("legacy-project");
    write(
        &legacy_project.join("src/charter/offering/tactic/legacy.tactic.yaml"),
        "id: legacy\napplies_to_languages: [legacylang]\n",
    );
    write(
        &legacy_project.join("doctrine/tactic/retired.tactic.yaml"),
        "id: retired\napplies_to_languages: [retiredlang]\n",
    );
    // Both candidates present: the project pack root wins.
    let pack_project = fixture.root().join("pack-project");
    write(
        &pack_project.join(".kittify/charter-packs/tactic/pack.tactic.yaml"),
        "id: pack\napplies_to_languages: [packlang]\n",
    );
    write(
        &pack_project.join("src/charter/offering/tactic/legacy.tactic.yaml"),
        "id: legacy\napplies_to_languages: [legacylang]\n",
    );

    let ops = json!([
        op_vocabulary(Some(legacy_project.as_path())),
        op_vocabulary(Some(pack_project.as_path())),
    ]);
    let expected = run_oracle(&fixture, &ops, &fixture.packs);

    assert_vocabulary_matches(
        &native_vocabulary(Some(legacy_project.as_path()), &fixture.packs),
        &expected[0],
        &["legacylang", "python"],
        &["retiredlang"],
    );
    assert_vocabulary_matches(
        &native_vocabulary(Some(pack_project.as_path()), &fixture.packs),
        &expected[1],
        &["packlang", "python"],
        &["legacylang"],
    );
}

#[test]
fn declared_languages_and_placeholders_match_the_oracle() {
    let fixture = fixture();
    const TEXTS: [&str; 20] = [
        "Python services with pytest and ruff. TypeScript frontend built with tsc.",
        "Java-based backend",
        "Python-based services",
        "Java 21 with Gradle",
        "Kotlin with Gradle",
        "Gradle",
        "maven",
        "librespot-java",
        "my-python-thing",
        "React-TypeScript",
        "node.js",
        "node.jsx",
        "C#",
        "C++",
        "PHPUnit and Swift XCTest",
        "Rust with cargo and rustc",
        "Ruby on Rails with RSpec",
        "go and dart",
        "JUnit",
        "  ",
    ];
    const PLACEHOLDERS: [&str; 18] = [
        "",
        "  ",
        "[]",
        "-",
        "n/a",
        "N/A",
        "na",
        "none",
        "null",
        "TBD",
        "any",
        "ALL",
        "unknown",
        "language-agnostic",
        "language agnostic",
        "[NEEDS CLARIFICATION: which?]",
        "Use only the languages, frameworks, and tools explicitly declared by this project.",
        "zig",
    ];

    let mut ops = Vec::new();
    for text in TEXTS {
        ops.push(op_declared(text));
    }
    for value in PLACEHOLDERS {
        ops.push(op_placeholder(value));
    }
    let expected = run_oracle(&fixture, &json!(ops), &fixture.packs);

    for (index, text) in TEXTS.iter().enumerate() {
        assert_eq!(
            json!(sk_doctrine::extract_declared_languages(text)),
            expected[index],
            "declared languages for {text:?}"
        );
    }
    for (index, value) in PLACEHOLDERS.iter().enumerate() {
        assert_eq!(
            json!(sk_doctrine::is_placeholder_language_answer(value)),
            expected[TEXTS.len() + index],
            "placeholder verdict for {value:?}"
        );
    }
    // Non-vacuous anchors for the compound guard and the build-token rule.
    assert_eq!(expected[0], json!(["python", "typescript"]));
    assert_eq!(expected[1], json!(["java"]));
    assert_eq!(expected[5], json!(["jvm"]));
    assert_eq!(expected[7], json!([]));
    assert_eq!(expected[8], json!([]));
    assert_eq!(expected[9], json!([]));
    assert_eq!(expected[TEXTS.len() + 5], json!(true));
    assert_eq!(expected[TEXTS.len() + 17], json!(false));
}

struct InferCase {
    name: &'static str,
    files: Vec<(&'static str, &'static str)>,
    answers_yaml: Option<&'static str>,
    shipped_default: bool,
    prefer: bool,
}

#[test]
fn inference_tiers_match_the_oracle() {
    let fixture = fixture();
    let compiled = ".kittify/charter/charter.yaml";
    let disk_answers = ".kittify/charter/interview/answers.yaml";
    let cases = [
        InferCase {
            name: "configured-wins-over-everything",
            files: vec![
                (".kittify/config.yaml", "languages: [typescript]\n"),
                (compiled, "catalog:\n  languages: [python]\n"),
            ],
            answers_yaml: Some("languages_frameworks: zig"),
            shipped_default: false,
            prefer: true,
        },
        InferCase {
            name: "configured-entries-are-validated",
            files: vec![
                (
                    ".kittify/config.yaml",
                    "languages: [typescript, brainfuck, 12, jvm, \" PYTHON \"]\n",
                ),
                (compiled, "catalog:\n  languages: [rust]\n"),
            ],
            answers_yaml: None,
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "configured-empty-falls-through",
            files: vec![
                (".kittify/config.yaml", "languages: []\n"),
                (compiled, "catalog:\n  languages: [python]\n"),
            ],
            answers_yaml: None,
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "configured-invalid-only-falls-through",
            files: vec![
                (".kittify/config.yaml", "languages: [brainfuck, 7]\n"),
                (compiled, "catalog:\n  languages: [python]\n"),
            ],
            answers_yaml: None,
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "compiled-beats-supplied-interview",
            files: vec![(compiled, "catalog:\n  languages: [python]\n")],
            answers_yaml: Some("languages_frameworks: Zig 0.16.0"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "compiled-empty-is-authoritative",
            files: vec![
                (compiled, "catalog:\n  languages: []\n"),
                (disk_answers, "answers:\n  languages_frameworks: zig\n"),
            ],
            answers_yaml: Some("languages_frameworks: zig"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "compiled-null-falls-back",
            files: vec![(compiled, "catalog:\n  languages:\n")],
            answers_yaml: Some("languages_frameworks: zig"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "compiled-non-list-falls-back",
            files: vec![(compiled, "catalog:\n  languages: python\n")],
            answers_yaml: Some("languages_frameworks: zig"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "compiled-items-are-str-converted",
            files: vec![(compiled, "catalog:\n  languages: [12, null, \" Zig \"]\n")],
            answers_yaml: None,
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "prefer-interview-in-memory",
            files: vec![(compiled, "catalog:\n  languages: [python]\n")],
            answers_yaml: Some("languages_frameworks: Zig 0.16.0"),
            shipped_default: false,
            prefer: true,
        },
        InferCase {
            name: "prefer-interview-on-disk",
            files: vec![
                (compiled, "catalog:\n  languages: [python]\n"),
                (disk_answers, "answers:\n  languages_frameworks: zig\n"),
            ],
            answers_yaml: None,
            shipped_default: false,
            prefer: true,
        },
        InferCase {
            name: "prefer-interview-without-interview-uses-compiled",
            files: vec![(compiled, "catalog:\n  languages: [python]\n")],
            answers_yaml: None,
            shipped_default: false,
            prefer: true,
        },
        InferCase {
            name: "shipped-default-does-not-mask-disk",
            files: vec![(disk_answers, "answers:\n  languages_frameworks: zig\n")],
            answers_yaml: None,
            shipped_default: true,
            prefer: false,
        },
        InferCase {
            name: "no-inputs-is-no-signal",
            files: vec![(".kittify/charter/charter.md", "Java, Maven, and JUnit\n")],
            answers_yaml: None,
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "shipped-default-only-is-no-signal",
            files: vec![],
            answers_yaml: None,
            shipped_default: true,
            prefer: false,
        },
        InferCase {
            name: "empty-supplied-interview-defers-to-disk",
            files: vec![(disk_answers, "answers:\n  languages_frameworks: zig\n")],
            answers_yaml: Some("{}"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "compiled-beats-non-default-interview",
            files: vec![(compiled, "catalog:\n  languages: [python]\n")],
            answers_yaml: Some("languages_frameworks: Rust with cargo"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "prefer-placeholder-interview-is-no-signal",
            files: vec![(compiled, "catalog:\n  languages: [python]\n")],
            answers_yaml: Some("languages_frameworks: N/A"),
            shipped_default: false,
            prefer: true,
        },
        InferCase {
            name: "list-repr-answer-is-split-then-unknown",
            files: vec![],
            answers_yaml: Some("languages_frameworks: [Zig, C]"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "list-repr-answer-keeps-known-token",
            files: vec![],
            answers_yaml: Some("languages_frameworks: [Python, Zig]"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "numeric-answer-is-str-converted",
            files: vec![],
            answers_yaml: Some("languages_frameworks: 3"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "null-answer-is-a-placeholder-and-scans-others",
            files: vec![],
            answers_yaml: Some("languages_frameworks: null\ntesting_requirements: pytest"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "placeholder-answer-scans-other-answers",
            files: vec![],
            answers_yaml: Some(
                "languages_frameworks: n/a\nproject_intent: Port the tool to Zig\ntesting_requirements: pytest",
            ),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "compiled-malformed-falls-back",
            files: vec![(compiled, "catalog:\n  languages: [unterminated\n")],
            answers_yaml: Some("languages_frameworks: zig"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "list-repr-unknown-placeholder",
            files: vec![],
            answers_yaml: Some("languages_frameworks: ['unknown']"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "list-repr-na-placeholder",
            files: vec![],
            answers_yaml: Some("languages_frameworks: [N/A]"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "separator-joined-placeholders",
            files: vec![],
            answers_yaml: Some("languages_frameworks: 'n/a, none; tbd'"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "separator-joined-placeholder-next-to-real",
            files: vec![],
            answers_yaml: Some("languages_frameworks: 'n/a, Zig'"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "configured-malformed-falls-through",
            files: vec![
                (".kittify/config.yaml", "languages: [unterminated\n"),
                (compiled, "catalog:\n  languages: [python]\n"),
            ],
            answers_yaml: Some("languages_frameworks: zig"),
            shipped_default: false,
            prefer: false,
        },
        InferCase {
            name: "configured-non-list-falls-through",
            files: vec![
                (".kittify/config.yaml", "languages: typescript\n"),
                (compiled, "catalog:\n  languages: [python]\n"),
            ],
            answers_yaml: None,
            shipped_default: false,
            prefer: false,
        },
    ];

    let mut ops = Vec::new();
    let mut projects = Vec::new();
    for case in &cases {
        let project = fixture.root().join(case.name);
        fs::create_dir_all(&project).unwrap();
        for (path, text) in &case.files {
            write(&project.join(path), text);
        }
        projects.push(project);
        ops.push(op_infer(
            Some(projects.last().unwrap().as_path()),
            case.answers_yaml,
            case.shipped_default,
            case.prefer,
        ));
    }
    let expected = run_oracle(&fixture, &json!(ops), &fixture.packs);
    assert_eq!(expected.len(), cases.len());

    for (index, case) in cases.iter().enumerate() {
        let project = projects[index].as_path();
        assert_eq!(
            native_infer(
                Some(project),
                &fixture.packs,
                case.answers_yaml,
                case.shipped_default,
                case.prefer
            ),
            expected[index],
            "case {}",
            case.name
        );
    }

    // Non-vacuous anchors: the three result states and the declared-answer rule.
    assert_eq!(expected[0], json!(["typescript"]));
    assert_eq!(expected[1], json!(["typescript", "jvm", "python"]));
    assert_eq!(expected[5], json!([]));
    assert_eq!(expected[8], json!(["12", "none", "zig"]));
    assert_eq!(expected[9], json!(["unknown"]));
    assert_eq!(expected[13], Value::Null);
    assert_eq!(expected[14], Value::Null);
    assert_eq!(expected[18], json!(["unknown"]));
    assert_eq!(expected[19], json!(["python"]));
    assert_eq!(expected[21], json!(["python"]));
    assert_eq!(expected[22], json!(["python"]));
    assert_eq!(expected[23], json!(["unknown"]));
    assert_eq!(expected[24], Value::Null);
    assert_eq!(expected[25], Value::Null);
    assert_eq!(expected[26], Value::Null);
    assert_eq!(expected[27], json!(["unknown"]));
    assert_eq!(expected[28], json!(["python"]));
    assert_eq!(expected[29], json!(["python"]));
    // Every case is genuinely exercised: no two native results were compared
    // against the same oracle slot by accident.
    assert_eq!(projects.len(), 30);
}

#[test]
fn vocabulary_cache_is_bounded_memoised_and_invalidatable() {
    let fixture = fixture();
    let mut roots = Vec::new();
    for index in 0..9 {
        let project = fixture.root().join(format!("cache-root-{index}"));
        write(
            &project.join(".kittify/charter-packs/tactic/t.tactic.yaml"),
            &format!("id: t\napplies_to_languages: [lang{index}]\n"),
        );
        roots.push(project);
    }

    let mut service = LanguageScopeService::with_vocabulary_cache(LanguageVocabularyCache::new());
    assert_eq!(service.vocabulary_cache().capacity(), 8);
    let mut second: Option<Arc<std::collections::BTreeSet<String>>> = None;
    for (index, root) in roots.iter().enumerate() {
        let vocabulary = service
            .scoped_language_vocabulary(Some(root), Some(&fixture.packs))
            .expect("vocabulary");
        assert!(vocabulary.contains(&format!("lang{index}")));
        if index == 1 {
            second = Some(vocabulary);
        }
    }
    assert_eq!(
        service.vocabulary_cache().len(),
        8,
        "bounded to eight roots"
    );
    assert!(
        !service
            .vocabulary_cache()
            .contains(Some(&roots[0]), Some(&fixture.packs)),
        "the ninth root evicts the first"
    );
    assert!(
        service
            .vocabulary_cache()
            .contains(Some(&roots[8]), Some(&fixture.packs)),
        "the newest root is cached"
    );

    // A hit returns the same memoised value (no rescan, same allocation).
    let repeated = service
        .scoped_language_vocabulary(Some(&roots[1]), Some(&fixture.packs))
        .expect("vocabulary");
    assert!(
        Arc::ptr_eq(&repeated, second.as_ref().expect("remembered vocabulary")),
        "a cache hit must reuse the memoised vocabulary"
    );

    // A changed artifact is invisible until the cache is cleared.
    write(
        &roots[1].join(".kittify/charter-packs/tactic/t.tactic.yaml"),
        "id: t\napplies_to_languages: [lang1, langx]\n",
    );
    let stale = service
        .scoped_language_vocabulary(Some(&roots[1]), Some(&fixture.packs))
        .expect("vocabulary");
    assert!(!stale.contains("langx"), "memoised until cleared");

    service.clear_vocabulary_cache();
    assert!(service.vocabulary_cache().is_empty());
    let refreshed = service
        .scoped_language_vocabulary(Some(&roots[1]), Some(&fixture.packs))
        .expect("vocabulary");
    assert!(refreshed.contains("langx"), "cleared cache rescans");

    // ... and the rescanned value still equals the reference's exact set.
    let ops = json!([op_vocabulary(Some(roots[1].as_path()))]);
    let expected = run_oracle(&fixture, &ops, &fixture.packs);
    assert_vocabulary_matches(
        &json!(refreshed.iter().cloned().collect::<Vec<String>>()),
        &expected[0],
        &["lang1", "langx", "python"],
        &["lang0"],
    );

    // A failed scan is not cached: an org `local_path` with an unset
    // environment variable is a typed failure on both sides (the reference
    // raises `OrgPackEnvVarUnsetError`), so nothing is memoised.
    let broken = fixture.root().join("broken-org-project");
    write(
        &broken.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: broken\n        local_path: ${SK_LANGUAGE_SCOPE_UNSET_VAR_9F3A}/pack\n",
    );
    let mut failing = LanguageScopeService::new();
    let error = failing
        .scoped_language_vocabulary(Some(&broken), Some(&fixture.packs))
        .expect_err("an unset org local_path variable must fail");
    assert!(
        error
            .to_string()
            .contains("SK_LANGUAGE_SCOPE_UNSET_VAR_9F3A"),
        "the typed failure names the unresolved token: {error}"
    );
    assert!(failing.vocabulary_cache().is_empty());
}

#[test]
fn inferred_languages_preserve_the_three_states() {
    // The typed contract itself, independent of the oracle: the three states do
    // not collapse into one another.
    assert_eq!(InferredLanguages::NoSignal.into_option(), None);
    assert_eq!(InferredLanguages::Empty.into_option(), Some(Vec::new()));
    assert_eq!(
        InferredLanguages::Declared(vec!["unknown".to_owned()]).into_option(),
        Some(vec!["unknown".to_owned()])
    );
    assert!(InferredLanguages::NoSignal.is_no_signal());
    assert!(!InferredLanguages::Empty.is_no_signal());
    assert_eq!(InferredLanguages::NoSignal.as_slice(), &[] as &[String]);
}
