//! Scoped-safe-commit conformance against the committed oracle corpus.
//!
//! Evidence lives in `tests/fixtures/safe_commit/` and is produced by
//! `generate_oracle.py` from the pinned Python baseline. This test executes
//! every committed case against the native `sk-git` implementation in a child
//! process (so an inherited `GIT_INDEX_FILE` and a post-CAS competitor cannot
//! leak between cases) and compares the complete before/after snapshots.
//!
//! There is deliberately no `#[ignore]`/skip path here: a missing, drifted or
//! empty corpus fails. The single ignored test is the subprocess helper the
//! parent spawns. The custom-`GIT_INDEX_FILE` case is an exact match now that the
//! operator index is captured at repository open; the corpus no longer records
//! an open index mismatch.

#[path = "common/safe_commit_oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use oracle::{Case, Json, NativeRun};
use sk_kernel::content_digest::sha256_hex;

const FIXTURE_ROOT: &str = "tests/fixtures/safe_commit";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_ROOT)
}

fn read_json(path: &Path) -> Json {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("required fixture {} is missing: {error}", path.display()));
    Json::parse(&text)
        .unwrap_or_else(|error| panic!("{} is not valid JSON: {error}", path.display()))
}

fn load_case(path: &Path) -> Case {
    Case::from_json(&read_json(path))
}

fn manifest() -> Json {
    read_json(&fixture_root().join("manifest.json"))
}

#[test]
fn corpus_manifest_is_complete_and_undrifted() {
    let fixture_root = fixture_root();
    assert!(
        fixture_root.is_dir(),
        "the committed oracle corpus is missing at {}",
        fixture_root.display()
    );
    let manifest = manifest();
    assert_eq!(
        manifest.at("baseline").as_str(),
        "9aed3a8012cb5eb22c0128b14d525039f95fea7a",
        "the corpus must bind the pinned Python baseline"
    );
    let generator = fixture_root.join("generate_oracle.py");
    assert_eq!(
        manifest.at("generator_sha256").as_str(),
        sha256_hex(&std::fs::read(&generator).expect("generator")),
        "generate_oracle.py changed without regenerating the corpus"
    );
    let verification = manifest.at("verify_oracle");
    assert!(
        verification.at("ok").as_bool(),
        "the generator recorded a failed lock-exact oracle verification"
    );
    assert!(
        verification.at("drift").as_array().is_empty(),
        "the generator recorded oracle drift"
    );
    assert!(
        verification.at("missing").as_array().is_empty(),
        "the generator recorded missing oracle requirements"
    );
    assert!(
        manifest.at("normalizations").as_array().len() >= 3,
        "normalisations must be documented in the manifest"
    );

    let cases = manifest.at("cases").as_array();
    let required = manifest.at("required_case_names").strings();
    assert_eq!(
        cases.len(),
        required.len(),
        "the manifest must list every required case exactly once"
    );
    let mut seen = BTreeMap::new();
    let mut expectations = BTreeMap::new();
    let mut divergences: Vec<String> = Vec::new();
    for entry in cases {
        let name = entry.at("name").as_str().to_owned();
        assert!(
            seen.insert(name.clone(), ()).is_none(),
            "duplicate case {name}"
        );
        let relative = entry.at("file").as_str();
        let path = fixture_root.join(relative);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("case {name} file {relative} is missing: {error}"));
        assert_eq!(
            sha256_hex(&bytes),
            entry.at("sha256").as_str(),
            "case {name} drifted from its recorded digest; regenerate the corpus"
        );
        let case = load_case(&path);
        assert_eq!(case.name, name, "case name mismatch in {relative}");
        let expectation = entry.at("expectation").as_str().to_owned();
        assert!(
            matches!(
                expectation.as_str(),
                "match" | "documented_divergence" | "open_mismatch"
            ),
            "case {name} has unknown expectation {expectation}"
        );
        if expectation == "documented_divergence" {
            divergences.push(name.clone());
        }
        *expectations.entry(expectation).or_insert(0usize) += 1;
        assert_eq!(case.expectation, entry.at("expectation").as_str());
    }
    for name in &required {
        assert!(seen.contains_key(name), "required case {name} is absent");
    }
    assert!(
        expectations.get("match").copied().unwrap_or(0) >= 31,
        "the corpus needs a substantive set of exact oracle matches: {expectations:?}"
    );
    assert_eq!(
        expectations
            .get("documented_divergence")
            .copied()
            .unwrap_or(0),
        1,
        "the remaining literal-path correction must stay explicit: {expectations:?}"
    );
    assert_eq!(
        expectations.get("open_mismatch").copied().unwrap_or(0),
        0,
        "the captured operator index closed the last open mismatch; a new one must be recorded explicitly: {expectations:?}"
    );
    assert_eq!(
        divergences,
        vec!["pathspec_magic_literal_name".to_string()],
        "the one documented divergence must stay the literal-path case; a re-labelled \
         divergence would otherwise satisfy the counts"
    );
    assert_eq!(
        cases.len(),
        32,
        "the scoped-safe-commit corpus is a 32-case closure: {expectations:?}"
    );

    // Explicit coverage pins, so deleting a hard case cannot quietly reduce scope.
    let required_behaviour = [
        "prepare_and_commit_msg_rewrite",
        "commit_msg_rejection_refused",
        "pre_commit_rejection_usage_phrase_not_environment",
        "message_cleanup_whitespace",
        "message_cleanup_strip_config",
        "message_cleanup_verbatim_config",
        "whitespace_only_message_refused",
        "noop_runs_hooks_before_classification",
        "sparse_in_cone_preserves_skip_worktree",
        "sparse_out_of_cone_refused",
        "intent_to_add_and_staged_deletion_preserved",
        "unmerged_entries_preserved",
        "worktree_deletion_committed",
        "index_only_deletion_keeps_file",
        "literal_unusual_filenames",
        "nested_hook_advance_conflict",
        "post_cas_competitor_reconciled_to_landed_commit",
        "git_index_file_selector_honoured",
        "pinned_signing_failure_typed",
        "hook_stages_extra_paths_landed",
    ];
    for name in required_behaviour {
        assert!(
            seen.contains_key(name),
            "required behaviour case {name} is absent from the corpus"
        );
    }
}

#[test]
fn oracle_corpus_matches_native_transactions() {
    let fixture_root = fixture_root();
    let manifest = manifest();
    // Order by the committed manifest so failures are reproducible.
    let mut failures: Vec<String> = Vec::new();
    for entry in manifest.at("cases").as_array() {
        let name = entry.at("name").as_str().to_owned();
        let path = fixture_root.join(entry.at("file").as_str());
        let case = load_case(&path);
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_and_check(&case)));
        if let Err(payload) = result {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                })
                .unwrap_or_else(|| "<non-string panic>".to_string());
            failures.push(format!("{name}: {message}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of the committed oracle cases failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

fn run_and_check(case: &Case) {
    let temp = tempfile::tempdir().expect("create case root");
    let root = temp.path();
    let out = root.join("native.json");
    let case_path = fixture_root()
        .join("cases")
        .join(format!("{}.json", case.name));
    let exe = std::env::current_exe().expect("current test executable");
    let mut command = Command::new(exe);
    command
        .args([
            "--ignored",
            "--exact",
            "native_case_helper",
            "--",
            case_path.to_str().expect("UTF-8 fixture path"),
            out.to_str().expect("UTF-8 output path"),
            root.to_str().expect("UTF-8 root path"),
        ])
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("HOME", root.join("home"))
        .env("TMPDIR", root.join("tmp"))
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_CACHE_HOME", root.join("xdg-cache"))
        .env("XDG_DATA_HOME", root.join("xdg-data"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000")
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("RUST_BACKTRACE", "1")
        .env("SK_RUN_ROOT", root)
        .env("SK_CUSTOM_INDEX", root.join("custom-index"))
        .env("SK_SAFE_COMMIT_HOOK_LOG", root.join("hooks.log"))
        .env("SK_POST_CAS_INDEX", root.join("competitor-index"))
        .env("SK_POST_CAS_SENTINEL", root.join("competitor.done"))
        .env("SK_POST_CAS_FAILED", root.join("competitor.failed"));
    if case.request.custom_index {
        command.env("GIT_INDEX_FILE", root.join("custom-index"));
    }
    let output = command.output().expect("spawn the native case helper");
    if !output.status.success() {
        panic!(
            "native case helper failed ({}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let run = NativeRun::from_json(&read_json(&out));
    assert_eq!(run.case, case.name, "helper ran the wrong case");
    oracle::assert_no_transient_git_state(&root.join("repo"), &case.name);
    assert_eq!(
        run.before, case.before,
        "{}: fixture setup drift; the native before-state no longer equals the oracle's",
        case.name
    );

    assert_eq!(
        run.native.variant, case.native.variant,
        "{}: typed native variant (detail: {:?})",
        case.name, run.native.error_detail
    );
    assert_eq!(
        run.native.landed_kind, case.native.landed_kind,
        "{}: typed landed kind",
        case.name
    );

    match case.expectation.as_str() {
        "match" => oracle::assert_full_match(case, &run),
        "documented_divergence" => {
            oracle::assert_hook_evidence(&case.hooks, &run.hooks, &case.name);
            assert_documented_divergence(case, &run, root);
        }
        other => panic!("{}: unknown expectation {other}", case.name),
    }

    match case.name.as_str() {
        "nested_hook_advance_conflict" => {
            let candidate = run.native.candidate.as_deref().expect("candidate evidence");
            assert_eq!(
                run.native.observed_tip.as_deref(),
                Some(run.after.head.as_str()),
                "the observed tip must equal the nested commit now on the branch"
            );
            assert_ne!(
                candidate, run.after.head,
                "candidate is not the nested commit"
            );
            assert!(
                !oracle::is_ancestor(&root.join("repo"), candidate, &run.after.head),
                "the losing candidate must not be reachable from the nested hook commit"
            );
        }
        "post_cas_competitor_reconciled_to_landed_commit" => {
            let landed = run.native.commit.as_deref().expect("landed commit");
            assert_ne!(
                run.after.head, landed,
                "the competitor must be the new HEAD"
            );
            assert_eq!(
                run.after.parents,
                vec![landed.to_string()],
                "the competitor must sit directly on the landed commit"
            );
            assert_eq!(
                run.after.message.as_deref(),
                Some("competitor\n"),
                "the competitor commit is the settled HEAD"
            );
            let index_p = run
                .after
                .index
                .iter()
                .find(|entry| entry.ends_with(" p.txt"))
                .expect("p.txt index entry");
            let landed_blob = oracle::git_stdout(
                &root.join("repo"),
                &["rev-parse", &format!("{landed}:p.txt")],
            );
            assert!(
                index_p.contains(landed_blob.trim()),
                "the real index must be reconciled to the landed commit, not the moving HEAD: {index_p} vs {}",
                landed_blob.trim()
            );
        }
        _ => {}
    }
}

fn assert_documented_divergence(case: &Case, run: &NativeRun, root: &Path) {
    match case.name.as_str() {
        "hook_formatter_makes_candidate_nonempty" => {
            assert_eq!(case.oracle.outcome, "error");
            assert_eq!(case.oracle.refusal_class.as_deref(), Some("no_change"));
            assert_eq!(case.after.head, case.before.head);
            assert_eq!(run.native.outcome, "ok");
            let landed = run.native.commit.as_deref().expect("native commit");
            assert_eq!(run.after.head, landed);
            assert_eq!(run.after.committed_paths, vec!["a.txt".to_string()]);
            assert_eq!(run.after.message.as_deref(), Some("diff: formatted\n"));
            assert!(
                run.after.status.is_empty(),
                "the reformatted requested path is fully committed: {:?}",
                run.after.status
            );
            assert!(
                run.after
                    .files
                    .iter()
                    .any(|file| file.path == "a.txt" && file.text.as_deref() == Some("formatted\n")),
                "the hook's reformat must reach the native commit"
            );
        }
        "pathspec_magic_literal_name" => {
            assert_eq!(case.oracle.outcome, "error");
            assert_eq!(
                case.oracle.refusal_class.as_deref(),
                Some("landed_with_followup")
            );
            assert_eq!(run.native.outcome, "ok");
            assert_eq!(
                run.after.committed_paths,
                vec![":(glob)*.txt".to_string()],
                "native must commit only the literal path"
            );
            assert_eq!(
                run.after.status,
                vec![
                    " M a.txt".to_string(),
                    " M b.txt".to_string(),
                    " M other.txt".to_string(),
                ],
                "the unrelated modifications stay unstaged and the literal path is committed"
            );
        }
        "worktree_signing_enforced_divergence" => {
            assert_eq!(case.oracle.outcome, "ok");
            assert_eq!(run.native.outcome, "error");
            assert_eq!(run.native.refusal_class.as_deref(), Some("signing_failure"));
            assert_eq!(run.after, run.before, "the refusal must land no commit");
            assert_eq!(run.native.commit, None);
        }
        "pinned_commit_msg_rewrite_reflog_source" => {
            assert_eq!(case.oracle.outcome, "ok");
            assert_eq!(run.native.outcome, "ok");
            assert_eq!(run.native.commit, case.oracle.commit);
            assert_eq!(run.after.head, case.after.head);
            assert_eq!(run.after.message, case.after.message);
            assert_eq!(run.after.tree, case.after.tree);
            let mut native = run.after.clone();
            let mut expected = case.after.clone();
            native.reflog.clear();
            expected.reflog.clear();
            assert_eq!(
                native, expected,
                "only the reflog source may differ on this case"
            );
            let native_subject = run.after.reflog[0]
                .split_once('|')
                .expect("native reflog entry")
                .1;
            let oracle_subject = case.after.reflog[0]
                .split_once('|')
                .expect("oracle reflog entry")
                .1;
            assert_eq!(
                run.after.reflog[0].split_once('|').unwrap().0,
                case.after.reflog[0].split_once('|').unwrap().0,
                "both reflog entries describe the same commit"
            );
            assert!(oracle_subject.contains("subject body"), "{oracle_subject}");
            assert!(
                native_subject.contains("Signed-off-by: hook"),
                "native reflog source is the final rewritten message: {native_subject}"
            );
            assert_ne!(native_subject, oracle_subject);
            let _ = root;
        }
        "ordinary_soft_wrapped_reflog_source" => {
            assert_eq!(case.oracle.outcome, "ok");
            assert_eq!(run.native.outcome, "ok");
            assert_eq!(run.native.commit, case.oracle.commit);
            assert_eq!(run.after.message, case.after.message);
            let mut native = run.after.clone();
            let mut expected = case.after.clone();
            native.reflog.clear();
            expected.reflog.clear();
            assert_eq!(native, expected, "only the reflog subject may differ");
            let native_subject = run.after.reflog[0].split_once('|').unwrap().1;
            let oracle_subject = case.after.reflog[0].split_once('|').unwrap().1;
            assert_eq!(oracle_subject, "commit: subject");
            assert_eq!(native_subject, "commit: subject body");
        }
        other => panic!("{other}: unhandled documented divergence"),
    }
}

/// Subprocess helper: execute one committed case and write its observation.
#[test]
#[ignore]
fn native_case_helper() {
    let args: Vec<String> = std::env::args().collect();
    let separator = args
        .iter()
        .position(|arg| arg == "--")
        .expect("helper needs `-- <case> <out> <root>`");
    let case_path = PathBuf::from(&args[separator + 1]);
    let out_path = PathBuf::from(&args[separator + 2]);
    let root = PathBuf::from(&args[separator + 3]);
    let case = load_case(&case_path);
    let run = oracle::execute_case(&case, &root);
    assert_eq!(
        run.before, case.before,
        "native setup drift for {}",
        case.name
    );
    std::fs::write(&out_path, run.to_json().to_text()).expect("write helper observation");
}
