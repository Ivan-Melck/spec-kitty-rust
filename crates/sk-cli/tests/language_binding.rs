//! Live, required-oracle checks for the CLI-bound language signal and the
//! canonical profile matcher.
//!
//! The Python oracle is selected lock-exactly (`scripts/rust/verify_oracle.py`)
//! and absence/drift fails; there is no skip path. The native side reads the
//! same temporary project and the same `packs/built-in` tree the reference
//! reads. Expectations are not hand-frozen: they are produced by the reference
//! for every case, with explicit literal assertions on the load-bearing states
//! (`None` vs `[]`, configured override, stringification, the Python-literal
//! interview shape and default-vs-disk masking) so a vacuous comparison cannot
//! pass.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sk_doctrine::{InferRequest, InterviewAnswers, LanguageScopeService, SHIPPED_DEFAULT_ANSWERS};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn packs_root() -> PathBuf {
    repo_root().join("packs")
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, text).expect("write fixture");
}

fn reset(root: &Path) {
    let kittify = root.join(".kittify");
    if kittify.exists() {
        fs::remove_dir_all(&kittify).expect("reset .kittify");
    }
    fs::create_dir_all(&kittify).expect("create .kittify");
}

fn zig_tactic(root: &Path) {
    write(
        root,
        ".kittify/charter-packs/tactic/zig-practice.tactic.yaml",
        "schema_version: '1.0'\nid: zig-practice\nname: Zig practice\napplies_to_languages: [zig]\n",
    );
}

fn config(root: &Path, languages: &str) {
    write(
        root,
        ".kittify/config.yaml",
        &format!("agents:\n  available: [codex]\n{languages}"),
    );
}

fn compiled(root: &Path, languages: &str) {
    write(
        root,
        ".kittify/charter/charter.yaml",
        &format!("catalog:\n  languages: {languages}\n"),
    );
}

fn answers(root: &Path, answer: &str) {
    write(
        root,
        ".kittify/charter/interview/answers.yaml",
        &format!("answers:\n  languages_frameworks: {answer}\n"),
    );
}

const ORACLE_SCRIPT: &str = r#"
import json
import sys
from pathlib import Path

from ruamel.yaml import YAML

from charter.activation.interview import CharterInterview, default_interview
from charter.activation.language_scope import infer_repo_languages
from charter.offering.shared.scoping import applies_to_languages_match

spec = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
results = []
for op in spec:
    kind = op["op"]
    if kind == "infer":
        interview = None
        if op.get("shipped_default"):
            interview = default_interview(mission="software-dev", profile="minimal")
        elif op.get("answers_yaml") is not None:
            answers = YAML(typ="safe").load(op["answers_yaml"]) or {}
            interview = CharterInterview.from_dict(
                {"answers": {str(key): value for key, value in answers.items()}}
            )
        results.append(
            infer_repo_languages(Path(op["repo"]), interview=interview)
        )
    elif kind == "applies":
        results.append(
            applies_to_languages_match(op["artifact"], op["active"])
        )
    else:
        raise SystemExit("unknown op: " + str(kind))
print(json.dumps(results))
"#;

fn run_oracle(project: &Path, ops: &Value) -> Vec<Value> {
    let spec = tempfile::NamedTempFile::new_in(project).expect("ops file");
    fs::write(spec.path(), serde_json::to_vec(ops).expect("ops json")).expect("write ops");
    let output = Command::new(oracle_support::python())
        .arg("-c")
        .arg(ORACLE_SCRIPT)
        .arg(spec.path())
        .current_dir(project)
        .env("PYTHONPATH", repo_root().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", packs_root())
        .output()
        .expect("spawn required Python oracle");
    assert!(
        output.status.success(),
        "oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("oracle emitted JSON");
    value.as_array().expect("oracle emitted a list").clone()
}

fn native_infer(project: &Path, interview: Option<&InterviewAnswers>) -> Value {
    let request = InferRequest {
        repo_root: Some(project),
        interview,
        prefer_interview: false,
        packs_root: Some(&packs_root()),
    };
    let inferred = LanguageScopeService::new()
        .infer(&request)
        .expect("native infer");
    json!(inferred.into_option())
}

fn shipped_default_interview() -> InterviewAnswers {
    InterviewAnswers::new(
        SHIPPED_DEFAULT_ANSWERS
            .iter()
            .map(|(key, answer)| ((*key).to_owned(), (*answer).to_owned())),
    )
}

#[test]
fn the_inference_tiers_match_the_required_oracle() {
    let temp = tempfile::tempdir().expect("temp");
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project");

    struct Case {
        name: &'static str,
        setup: fn(&Path),
        expected: Option<Value>,
    }

    let cases = [
        Case {
            name: "no signal",
            setup: |_| {},
            expected: Some(Value::Null),
        },
        Case {
            name: "compiled explicit empty",
            setup: |root| compiled(root, "[]"),
            expected: Some(json!([])),
        },
        Case {
            name: "compiled python",
            setup: |root| compiled(root, "[python]"),
            expected: Some(json!(["python"])),
        },
        Case {
            name: "compiled stringification keeps non-strings",
            setup: |root| compiled(root, "[12, null, ' ZIG ']"),
            expected: Some(json!(["12", "none", "zig"])),
        },
        Case {
            name: "configured override beats compiled",
            setup: |root| {
                config(root, "languages: [python]\n");
                compiled(root, "[rust]");
            },
            expected: Some(json!(["python"])),
        },
        Case {
            name: "invalid configured list falls through",
            setup: |root| {
                config(root, "languages: [zig]\n");
                compiled(root, "[rust]");
            },
            expected: Some(json!(["rust"])),
        },
        Case {
            name: "bracketed Python literal answer unions vocabulary",
            setup: |root| {
                zig_tactic(root);
                answers(root, "\"['Zig', 'C++', None, 12]\"");
            },
            expected: Some(json!(["zig"])),
        },
        Case {
            name: "plain custom declaration unions vocabulary",
            setup: |root| {
                zig_tactic(root);
                answers(root, "Zig");
            },
            expected: Some(json!(["zig"])),
        },
        Case {
            name: "built-in declaration",
            setup: |root| answers(root, "Python and pytest"),
            expected: Some(json!(["python"])),
        },
        Case {
            name: "placeholder answer scans the other answers",
            setup: |root| {
                write(
                    root,
                    ".kittify/charter/interview/answers.yaml",
                    "answers:\n  languages_frameworks: n/a\n  testing_requirements: we use Kotlin and Gradle\n",
                )
            },
            expected: Some(json!(["kotlin"])),
        },
        Case {
            name: "compiled empty remains authoritative over the interview",
            setup: |root| {
                zig_tactic(root);
                compiled(root, "[]");
                answers(root, "Zig");
            },
            expected: Some(json!([])),
        },
        Case {
            name: "non-list compiled value falls through",
            setup: |root| {
                zig_tactic(root);
                compiled(root, "null");
                answers(root, "Zig");
            },
            expected: Some(json!(["zig"])),
        },
        Case {
            name: "generic JVM declaration",
            setup: |root| answers(root, "Gradle"),
            expected: Some(json!(["jvm"])),
        },
        Case {
            name: "JVM language suppresses the build-tool family",
            setup: |root| answers(root, "Kotlin and Gradle"),
            expected: Some(json!(["kotlin"])),
        },
        Case {
            name: "bracketed Java and Gradle",
            setup: |root| answers(root, "\"['Java', 'Gradle']\""),
            expected: Some(json!(["java"])),
        },
    ];

    for case in cases {
        reset(&project);
        (case.setup)(&project);
        let oracle = run_oracle(&project, &json!([{"op": "infer", "repo": project}]));
        assert_eq!(oracle.len(), 1);
        assert_eq!(
            native_infer(&project, None),
            oracle[0],
            "case {}",
            case.name
        );
        if let Some(expected) = case.expected {
            assert_eq!(oracle[0], expected, "oracle literal for {}", case.name);
        }
    }
}

#[test]
fn shipped_default_interview_does_not_mask_the_disk_transcript() {
    let temp = tempfile::tempdir().expect("temp");
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project");
    zig_tactic(&project);
    answers(&project, "Zig");

    let interview = shipped_default_interview();
    let oracle = run_oracle(
        &project,
        &json!([{"op": "infer", "repo": project, "shipped_default": true}]),
    );
    assert_eq!(oracle[0], json!(["zig"]), "disk transcript wins");
    assert_eq!(
        native_infer(&project, Some(&interview)),
        oracle[0],
        "the shipped-default interview must not mask the disk transcript"
    );
}

#[test]
fn the_profile_language_matcher_matches_the_required_oracle() {
    let temp = tempfile::tempdir().expect("temp");
    let project = temp.path().join("project");
    fs::create_dir_all(&project).expect("project");

    type MatchCase<'a> = (&'a [&'a str], Option<&'a [&'a str]>, bool);

    let cases: [MatchCase; 14] = [
        (&["python"], None, true),
        (&[], Some(&["unknown"]), true),
        (&["python"], Some(&["python"]), true),
        (&["python"], Some(&["kotlin"]), false),
        (&["jvm"], Some(&["kotlin"]), true),
        (&["java"], Some(&["kotlin"]), false),
        (&["kotlin"], Some(&["jvm"]), false),
        (&["any"], Some(&["rust"]), true),
        (&["all"], Some(&[]), true),
        (&["unknown"], Some(&["python"]), false),
        (&["unknown"], None, false),
        (&["python", "unknown"], Some(&["unknown", "python"]), true),
        (&["python"], Some(&[]), false),
        (&["JVM"], Some(&["KOTLIN"]), true),
    ];

    for (artifact, active, expected) in cases {
        let artifact_owned: Vec<String> =
            artifact.iter().map(|token| (*token).to_owned()).collect();
        let active_owned: Option<Vec<String>> =
            active.map(|tokens| tokens.iter().map(|token| (*token).to_owned()).collect());
        let oracle = run_oracle(
            &project,
            &json!([{
                "op": "applies",
                "artifact": artifact,
                "active": active,
            }]),
        );
        assert_eq!(oracle[0], json!(expected), "oracle literal");
        assert_eq!(
            sk_doctrine::applies_to_languages_match(&artifact_owned, active_owned.as_deref()),
            expected,
            "native matcher for {artifact:?} against {active:?}"
        );
        assert_eq!(
            json!(sk_doctrine::applies_to_languages_match(
                &artifact_owned,
                active_owned.as_deref()
            )),
            oracle[0],
            "native matcher equals the oracle for {artifact:?} against {active:?}"
        );
    }
}
