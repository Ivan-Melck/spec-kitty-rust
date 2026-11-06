//! CF11: Source-recorded closure of the remaining real gaps in the six charter
//! migration effects.
//!
//! Every assertion here replays a tree the PINNED Source actually produced (see
//! each fixture's `_provenance`), so a green is a comparison against Source
//! output, not against a native preference:
//!
//! * `cf11_charter_synthesis_promote_seeded_hint_oracle.json` — the
//!   `4.0.0rc8_promote_interview_selections` seeded-list HINT. The A5 fixture
//!   never reached `_seeded_list_hints` (its present list held only a subset of
//!   the available directives, and its offering did not resolve), so the warning
//!   path was unverified. Here the present `activated_directives` list is exactly
//!   what the Source's own `resolve_effective_sets` calls available (38 at the
//!   pin), the interview selected two, and the recorded run warns, narrows nothing
//!   and writes nothing — on the real run AND on the dry run.
//! * `cf11_charter_synthesis_carrier_oracle.json` — the two carrier branches the
//!   A5 pass left on the wrong side of an `Err`: the promote DRY RUN of a settled
//!   project (the preview line, not the real run's already-promoted line) and the
//!   legacy-rename REFUSAL, which the Source CATCHES into `success = false` with
//!   the refusal text as its error instead of raising.
//!
//! One further branch — `SelectionResolver.build` failing — is a NATIVE-ONLY smoke
//! below: the reference's upgrade wrapper reads `.kittify/config.yaml` before the
//! migration body, so the same broken config aborts a real Source run outside
//! `apply()` and the branch cannot be paired honestly. The assertion is therefore
//! labelled as such and claims no Source equality.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

const SEEDED_HINT: &str =
    include_str!("fixtures/cf11_charter_synthesis_promote_seeded_hint_oracle.json");
const CARRIER: &str = include_str!("fixtures/cf11_charter_synthesis_carrier_oracle.json");
const LEAKS: &str = include_str!("fixtures/cf11_charter_synthesis_leaks_oracle.json");

struct Workspace {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        let root = directory.path().to_path_buf();
        Self {
            _directory: directory,
            root,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

fn oracle(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture parses")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("a recorded list")
        .iter()
        .map(|item| item.as_str().expect("a recorded string").to_string())
        .collect()
}

/// Write the `kind: file` / `kind: dir` members of a recorded snapshot.
fn write_recorded(project: &Path, snapshot: &Value) {
    let members = snapshot.as_object().expect("a recorded snapshot");
    for (path, entry) in members {
        let target = project.join(path);
        match entry["kind"].as_str().expect("a recorded kind") {
            "dir" => std::fs::create_dir_all(&target).expect("create dir"),
            "file" => {
                std::fs::create_dir_all(target.parent().expect("a parent")).expect("create parent");
                std::fs::write(&target, entry["text"].as_str().expect("recorded text"))
                    .expect("write file");
            }
            other => panic!("the fixture records an unsupported member kind {other:?}"),
        }
    }
}

/// The whole tree: every directory and every file's exact bytes.
fn snapshot(project: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![project.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(project)
                .expect("under the project")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                out.insert(format!("{relative}/"), String::new());
                stack.push(path);
            } else {
                out.insert(
                    relative,
                    String::from_utf8(std::fs::read(&path).expect("read")).expect("utf-8"),
                );
            }
        }
    }
    out
}

fn recorded_tree(snapshot: &Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (path, entry) in snapshot.as_object().expect("a recorded snapshot") {
        match entry["kind"].as_str().expect("a recorded kind") {
            "dir" => {
                out.insert(format!("{path}/"), String::new());
            }
            "file" => {
                out.insert(
                    path.clone(),
                    entry["text"].as_str().expect("recorded text").to_string(),
                );
            }
            other => panic!("the fixture records an unsupported member kind {other:?}"),
        }
    }
    out
}

/// `[True, ""]` in the Source record means "applicable" (`None` natively).
fn expected_can_apply(case: &Value) -> Option<String> {
    let recorded = case["can_apply"].as_array().expect("a recorded can_apply");
    let applicable = recorded[0].as_bool().expect("a recorded flag");
    if applicable {
        None
    } else {
        Some(recorded[1].as_str().expect("a recorded reason").to_string())
    }
}

/// Substitute the fixture's `$PACKS` placeholder with this run's pack root.
fn substitute_packs(text: &str, packs: &Path) -> String {
    text.replace("$PACKS", &packs.to_string_lossy())
}

#[test]
fn the_provenance_leak_report_matches_the_source() {
    use sk_install::migrations::completion_modern::charter_synthesis::provenance;

    let oracle = oracle(LEAKS);
    let workspace = Workspace::new();
    let project = workspace.path("leaks");
    let packs = workspace.path("packs");
    // The `built_in` input is the BUILT-IN TIER root (`<packs>/built-in`), the same
    // root the portable token strips to `${SPEC_KITTY_PACKS_ROOT}/built-in/…`.
    let built_in = packs.join("built-in");
    std::fs::create_dir_all(&built_in).expect("packs root");
    for (path, entry) in oracle["before"].as_object().expect("before") {
        let target = project.join(path);
        std::fs::create_dir_all(target.parent().expect("a parent")).expect("parents");
        std::fs::write(
            &target,
            substitute_packs(entry["text"].as_str().expect("text"), &packs),
        )
        .expect("write");
    }

    // The read-only leak report is the classification `detect` uses: the two
    // healable rows, `repr`-quoted, and NOT the template_set row, the out-of-tree
    // absolute or the repo-relative row.
    assert_eq!(
        provenance::describe_leaks_with(&project, Some(&built_in)).expect("describe_leaks"),
        strings(&oracle["leaks"])
            .into_iter()
            .map(|line| substitute_packs(&line, &packs))
            .collect::<Vec<String>>(),
        "leaks: the Source's own report"
    );
    assert_eq!(
        provenance::detect_with(&project, Some(&built_in)).expect("detect"),
        oracle["detect"].as_bool().expect("recorded detect"),
        "leaks: detect agrees with the report"
    );

    let outcome = provenance::apply_with(&project, Some(&built_in)).expect("apply");
    assert!(outcome.success, "leaks: the heal succeeds");
    assert_eq!(
        outcome
            .changes
            .iter()
            .map(|line| substitute_packs(line, &packs))
            .collect::<Vec<String>>(),
        strings(&oracle["result"]["changes_made"])
            .into_iter()
            .map(|line| substitute_packs(&line, &packs))
            .collect::<Vec<String>>(),
        "leaks: the heal's change lines"
    );
    let wanted: BTreeMap<String, String> = oracle["after"]
        .as_object()
        .expect("after")
        .iter()
        .filter(|(_, entry)| entry["kind"] == "file")
        .map(|(path, entry)| {
            (
                path.clone(),
                substitute_packs(entry["text"].as_str().expect("text"), &packs),
            )
        })
        .collect();
    let written: BTreeMap<String, String> = snapshot(&project)
        .into_iter()
        .filter(|(path, _)| !path.ends_with('/'))
        .collect();
    assert_eq!(written, wanted, "leaks: the healed bytes are the Source's");

    // The recorded `can_apply` is the POST-heal answer (the probe asked after
    // applying), so the healed project reports the Source's refusal reason.
    assert_eq!(
        provenance::can_apply_with(&project, Some(&built_in)).expect("can_apply"),
        Some(
            oracle["can_apply_after"][1]
                .as_str()
                .expect("recorded reason")
                .to_string()
        ),
        "leaks: the post-heal can_apply reason"
    );
    assert!(
        provenance::describe_leaks_with(&project, Some(&built_in))
            .expect("describe_leaks")
            .is_empty(),
        "leaks: a healed project reports no leak"
    );
}

#[test]
fn the_seeded_list_hint_matches_the_source() {
    use sk_install::migrations::completion_modern::charter_synthesis::promote;

    let oracle = oracle(SEEDED_HINT);
    let case = &oracle["seeded_hint"];
    let workspace = Workspace::new();
    let project = workspace.path("seeded-hint");
    write_recorded(&project, &case["before"]);

    // The hint's cardinality is the Source's OWN availability answer, recorded
    // alongside the run: 38 directives at the pin.
    assert_eq!(
        oracle["resolved"]["activated_directives"]["count"].as_u64(),
        Some(38),
        "seeded hint: the recorded availability count"
    );

    assert_eq!(
        promote::detect(&project).expect("detect"),
        case["detect"].as_bool().expect("recorded detect"),
        "seeded hint: detect is always true"
    );
    assert_eq!(
        promote::can_apply(&project).expect("can_apply"),
        expected_can_apply(case),
        "seeded hint: can_apply"
    );

    let outcome = promote::apply(&project).expect("apply");
    assert!(outcome.success, "seeded hint: the run succeeds");
    assert_eq!(
        outcome.changes,
        strings(&case["result"]["changes_made"]),
        "seeded hint: changes"
    );
    assert_eq!(
        outcome.warnings,
        strings(&case["result"]["warnings"]),
        "seeded hint: warnings are the Source's hint, verbatim"
    );
    assert_eq!(
        outcome.errors,
        strings(&case["result"]["errors"]),
        "seeded hint: no errors"
    );
    assert!(!outcome.changed, "seeded hint: a hint narrows nothing");
    assert_eq!(
        snapshot(&project),
        recorded_tree(&case["after"]),
        "seeded hint: the whole tree is byte-identical to the Source's"
    );

    // The same hint on the dry run, whose change line is the preview.
    let dry = promote::apply_dry_run(&project).expect("dry run");
    assert_eq!(
        dry.changes,
        strings(&case["dry_result"]["changes_made"]),
        "seeded hint: dry-run change"
    );
    assert_eq!(
        dry.warnings,
        strings(&case["dry_result"]["warnings"]),
        "seeded hint: dry-run warnings"
    );
    assert_eq!(
        snapshot(&project),
        recorded_tree(&case["before"]),
        "seeded hint: the dry run wrote nothing"
    );
}

#[test]
fn a_settled_dry_run_previews_instead_of_claiming_the_real_run_line() {
    use sk_install::migrations::completion_modern::charter_synthesis::promote;

    let oracle = oracle(CARRIER);
    let case = &oracle["settled_dry"];
    let workspace = Workspace::new();
    let project = workspace.path("settled-dry");
    write_recorded(&project, &case["before"]);

    let dry = promote::apply_dry_run(&project).expect("dry run");
    assert_eq!(
        dry.changes,
        strings(&case["dry_result"]["changes_made"]),
        "settled dry run: the Source's preview line"
    );
    assert_eq!(
        dry.warnings,
        strings(&case["dry_result"]["warnings"]),
        "settled dry run: warnings"
    );
    assert_eq!(
        dry.errors,
        strings(&case["dry_result"]["errors"]),
        "settled dry run: errors"
    );
    assert_eq!(
        snapshot(&project),
        recorded_tree(&case["before"]),
        "settled dry run: nothing was written"
    );

    // The real run on the same tree still reports the already-promoted line.
    let real = promote::apply(&project).expect("apply");
    assert!(real.success);
    assert_eq!(
        real.changes,
        vec![
            "Interview selections were promoted by an earlier upgrade; nothing to promote"
                .to_string()
        ],
        "settled real run: the already-promoted line"
    );
}

#[test]
fn a_rename_refusal_is_a_failed_result_carrying_its_reason() {
    use sk_install::migrations::completion_modern::charter_synthesis::legacy_rename;

    let oracle = oracle(CARRIER);
    let case = &oracle["rename_refused"];
    let workspace = Workspace::new();
    let project = workspace.path("rename-refused");
    write_recorded(&project, &case["before"]);

    assert_eq!(
        legacy_rename::detect(&project).expect("detect"),
        case["detect"].as_bool().expect("recorded detect"),
        "rename refusal: a refusal is not 'nothing to do'"
    );
    assert_eq!(
        legacy_rename::can_apply(&project).expect("can_apply"),
        expected_can_apply(case),
        "rename refusal: can_apply"
    );

    let outcome = legacy_rename::apply(&project).expect("apply");
    assert!(
        !outcome.success,
        "rename refusal: the Source returns a FAILED result, not a raised error"
    );
    assert_eq!(
        outcome.errors,
        strings(&case["result"]["errors"]),
        "rename refusal: the refusal text is the Source's, verbatim"
    );
    assert_eq!(
        outcome.changes,
        strings(&case["result"]["changes_made"]),
        "rename refusal: no change lines"
    );
    assert_eq!(
        outcome.warnings,
        strings(&case["result"]["warnings"]),
        "rename refusal: warnings"
    );
    assert!(!outcome.changed, "rename refusal: nothing was written");
    assert_eq!(
        snapshot(&project),
        recorded_tree(&case["before"]),
        "rename refusal: the whole tree is untouched"
    );

    // The helper itself still refuses out loud (its own contract, the shape
    // `charter synthesize` calls), so the two doors stay distinguishable.
    assert!(
        legacy_rename::rename(&project).is_err(),
        "rename refusal: the charter-layer helper keeps raising"
    );
}

#[test]
fn an_unresolvable_doctrine_root_is_a_failed_result() {
    // NATIVE-ONLY SMOKE (no Source pairing, and no Source-equality claim): the
    // reference's runner reads `.kittify/config.yaml` in its upgrade wrapper
    // before `apply()` runs, so the same broken config aborts a real Source run
    // outside this branch and the recorded text cannot be compared honestly.
    // What is asserted is the CARRIER contract the reference implements in this
    // method: a resolver build failure returns `success = false` with the
    // `Could not resolve doctrine root: …` error instead of raising.
    use sk_install::migrations::completion_modern::charter_synthesis::promote;

    let workspace = Workspace::new();
    let project = workspace.path("resolver-failure");
    std::fs::create_dir_all(project.join(".kittify/charter/interview")).expect("dirs");
    std::fs::write(
        project.join(".kittify/config.yaml"),
        "agents: [unclosed\ncharter: .kittify/charter/charter.yaml\n",
    )
    .expect("config");
    std::fs::write(
        project.join(".kittify/charter/charter.yaml"),
        "governance:\n  charter:\n    selected_directives: []\ncatalog:\n  references: []\n\
         metadata:\n  bundle_schema_version: 2\n",
    )
    .expect("charter");
    std::fs::write(
        project.join(".kittify/charter/interview/answers.yaml"),
        "selected_directives:\n- DIRECTIVE_001\n",
    )
    .expect("answers");
    let before = snapshot(&project);

    let outcome = promote::apply(&project).expect("apply returns an outcome");
    assert!(
        !outcome.success,
        "resolver failure: the reference returns success = false"
    );
    assert_eq!(outcome.errors.len(), 1, "resolver failure: one error");
    assert!(
        outcome.errors[0].starts_with("Could not resolve doctrine root: "),
        "resolver failure: the reference's prefix, got {:?}",
        outcome.errors[0]
    );
    assert!(!outcome.changed, "resolver failure: nothing was written");
    assert_eq!(
        snapshot(&project),
        before,
        "resolver failure: the tree is untouched"
    );
}
