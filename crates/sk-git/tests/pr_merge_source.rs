//! Source-first parity for `accept --merge-commit` landing verification.
//!
//! One real repository is built with plain git (base → mission corpus → landed
//! merge shape / landed squash shape / staged-corpus shape / un-landed branch).
//! The PINNED Source verifier (`verify_pr_merge_evidence`) runs and its evidence
//! or refusal text is asserted FIRST; the Native verifier then runs the same
//! cases against the same repository. Every input is a real commit in that
//! repository — no fabricated ids, no rewritten refs.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sk_git::{GitRepository, verify_pr_merge_evidence};

const SLUG: &str = "demo-mission";

const ORACLE: &str = r#"
import json, sys
from pathlib import Path
from specify_cli.consolidation.baseline import (
    PrMergeEvidenceError,
    verify_pr_merge_evidence,
)

repo = Path(sys.argv[1])
cases = json.loads(sys.argv[2])
out = []
for case in cases:
    try:
        evidence = verify_pr_merge_evidence(
            repo,
            case["slug"],
            case["merge_commit"],
            target_ref=case["target_ref"],
            attest_first_landing=case["attest"],
            owned=None,
        )
    except PrMergeEvidenceError as exc:
        out.append({"name": case["name"], "error": str(exc)})
    else:
        out.append(
            {
                "name": case["name"],
                "ok": {
                    "pr_merge_commit": evidence.pr_merge_commit,
                    "baseline_merge_commit": evidence.baseline_merge_commit,
                    "anchor_evidence": evidence.anchor_evidence,
                },
            }
        )
print(json.dumps(out, sort_keys=True))
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "--all"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

/// Build the fixture repository and return `(root, cases)`.
fn fixture() -> (PathBuf, Value) {
    let root = tempfile::tempdir().expect("sandbox").keep();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("README.md"), "base\n").expect("readme");
    let base = commit_all(&root, "base");

    // The landed squash/rebase shape: a single-parent commit that introduces the
    // whole mission corpus, created on a branch and then fast-forwarded onto main.
    git(&root, &["switch", "-q", "-c", "landing-squash"]);
    std::fs::create_dir_all(root.join(format!("kitty-specs/{SLUG}"))).expect("mission dir");
    std::fs::write(
        root.join(format!("kitty-specs/{SLUG}/meta.json")),
        "{\n  \"mission_id\": \"01KN2371WNZX28ME4MN97B67PJ\",\n  \"mission_slug\": \"demo-mission\",\n  \"target_branch\": \"main\"\n}\n",
    )
    .expect("meta");
    let squash = commit_all(&root, "land mission corpus");
    git(&root, &["switch", "-q", "main"]);
    git(&root, &["merge", "-q", "--ff-only", "landing-squash"]);

    // The landed merge shape: a real two-parent merge commit on main, for a
    // DIFFERENT mission so the two landing shapes never overlap in the tree.
    git(&root, &["switch", "-q", "-c", "landing-merge", &base]);
    std::fs::create_dir_all(root.join("kitty-specs/merge-mission")).expect("mission dir");
    std::fs::write(
        root.join("kitty-specs/merge-mission/meta.json"),
        "{\"mission_id\": \"01KN2371WNZX28ME4MN97B67PJ\", \"mission_slug\": \"merge-mission\"}\n",
    )
    .expect("meta");
    commit_all(&root, "land mission corpus (merge shape)");
    git(&root, &["switch", "-q", "main"]);
    git(
        &root,
        &["merge", "-q", "--no-ff", "-m", "land it", "landing-merge"],
    );
    let merge = git(&root, &["rev-parse", "HEAD"]);
    let merge_parent = git(&root, &["rev-parse", "HEAD^1"]);

    // The staged-corpus shape: the mission directory exists at the parent
    // without meta.json, so a later meta.json commit must be refused.
    git(&root, &["switch", "-q", "-c", "staged", &base]);
    std::fs::create_dir_all(root.join("kitty-specs/staged-mission")).expect("mission dir");
    std::fs::write(root.join("kitty-specs/staged-mission/spec.md"), "# spec\n").expect("spec");
    commit_all(&root, "staged corpus part one");
    std::fs::write(
        root.join("kitty-specs/staged-mission/meta.json"),
        "{\"mission_slug\": \"staged-mission\"}\n",
    )
    .expect("meta");
    let staged_meta = commit_all(&root, "staged corpus meta");
    git(&root, &["switch", "-q", "main"]);
    git(
        &root,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "land the staged corpus",
            "staged",
        ],
    );

    // An un-landed branch commit that does carry the mission corpus.
    git(&root, &["switch", "-q", "-c", "unlanded", &base]);
    std::fs::create_dir_all(root.join("kitty-specs/unlanded")).expect("mission dir");
    std::fs::write(
        root.join("kitty-specs/unlanded/meta.json"),
        "{\"mission_slug\": \"unlanded\"}\n",
    )
    .expect("meta");
    let unlanded = commit_all(&root, "unlanded corpus");
    git(&root, &["switch", "-q", "main"]);

    let cases = serde_json::json!([
        {"name": "squash-attested", "slug": SLUG, "merge_commit": squash, "target_ref": "main", "attest": true},
        {"name": "squash-unattested", "slug": SLUG, "merge_commit": squash, "target_ref": "main", "attest": false},
        {"name": "merge-attested", "slug": "merge-mission", "merge_commit": merge, "target_ref": "main", "attest": true},
        {"name": "merge-unattested", "slug": "merge-mission", "merge_commit": merge, "target_ref": "main", "attest": false},
        {"name": "staged-corpus", "slug": "staged-mission", "merge_commit": staged_meta, "target_ref": "main", "attest": true},
        {"name": "not-carried", "slug": "absent-mission", "merge_commit": squash, "target_ref": "main", "attest": true},
        {"name": "not-landed", "slug": "unlanded", "merge_commit": unlanded, "target_ref": "main", "attest": true},
        {"name": "unknown-sha", "slug": SLUG, "merge_commit": "0123456789abcdef0123456789abcdef01234567", "target_ref": "main", "attest": true},
        {"name": "unborn-target", "slug": SLUG, "merge_commit": squash, "target_ref": "no-such-branch", "attest": true},
        {"name": "abbreviated", "slug": SLUG, "merge_commit": &squash[..10], "target_ref": "main", "attest": true},
        {"name": "not-hex", "slug": SLUG, "merge_commit": "HEAD^1", "target_ref": "main", "attest": true},
        {"name": "empty", "slug": SLUG, "merge_commit": "   ", "target_ref": "main", "attest": true},
        {"name": "merge-with-tracking-ref", "slug": "merge-mission", "merge_commit": merge, "target_ref": "main", "attest": true},
        {"name": "merge-parent-is-baseline", "slug": "merge-mission", "merge_commit": merge_parent, "target_ref": "main", "attest": true}
    ]);

    (root, cases)
}

fn source_output(root: &Path, cases: &Value) -> Value {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = repo_root.join("target/pr-merge-oracle.py");
    std::fs::write(&script, ORACLE).expect("write the oracle script");
    let output = Command::new(oracle::python())
        .arg(&script)
        .arg(root)
        .arg(cases.to_string())
        .current_dir(&repo_root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", repo_root.join("target/pr-merge-home"))
        .env(
            "PYTHONPATH",
            format!(
                "{}:{}",
                repo_root.display(),
                repo_root.join("src").display()
            ),
        )
        .env("SPEC_KITTY_PACKS_ROOT", repo_root.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("run the pinned Source verifier");
    assert!(
        output.status.success(),
        "the pinned Source verifier failed (harness/premise failure, not a business red):\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle json")
}

#[test]
fn pr_merge_verification_matches_the_pinned_source() {
    let (root, cases) = fixture();
    let source = source_output(&root, &cases);
    let rows = source.as_array().expect("rows");
    assert_eq!(rows.len(), cases.as_array().unwrap().len());

    let repository = GitRepository::open(&root).expect("open the fixture repository");
    for (row, case) in rows.iter().zip(cases.as_array().unwrap()) {
        let name = case["name"].as_str().expect("name");
        let attest = case["attest"].as_bool().expect("attest");
        let native = verify_pr_merge_evidence(
            &repository,
            case["slug"].as_str().expect("slug"),
            case["merge_commit"].as_str().expect("merge commit"),
            case["target_ref"].as_str().expect("target ref"),
            attest,
        );
        match (&row["ok"], native) {
            (Value::Object(evidence), Ok(native)) => {
                assert_eq!(
                    native.pr_merge_commit.as_str(),
                    evidence["pr_merge_commit"].as_str().unwrap(),
                    "{name} landing commit"
                );
                assert_eq!(
                    native.baseline_merge_commit.as_str(),
                    evidence["baseline_merge_commit"].as_str().unwrap(),
                    "{name} baseline anchor"
                );
                assert_eq!(
                    native.anchor_evidence,
                    evidence["anchor_evidence"].as_str().unwrap(),
                    "{name} anchor evidence class"
                );
            }
            (Value::Null, Err(native)) => {
                assert_eq!(
                    native.to_string(),
                    row["error"].as_str().expect("error text"),
                    "{name} refusal text"
                );
            }
            (Value::Null, Ok(native)) => panic!(
                "{name}: Source refused but Native accepted {} -> {}",
                native.pr_merge_commit, native.baseline_merge_commit
            ),
            (Value::Object(_), Err(native)) => {
                panic!("{name}: Source accepted but Native refused: {native}")
            }
            (other, _) => panic!("{name}: unexpected oracle row {other:?}"),
        }
    }
}
