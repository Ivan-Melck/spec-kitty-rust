//! Source-first differential tests for the provenance-healing migrations.
//!
//! The fixture `fixtures/charter_synthesis_provenance_oracle.json` records what
//! the PINNED Source produced (`m_3_2_7_heal_provenance_paths`,
//! `m_4_0_0rc5_heal_template_set_provenance` run under CPython 3.11.15 with the
//! repository's `.venv-rust-oracle`): the exact input tree, the exact post-state
//! bytes, `detect`/`can_apply`, the reported changes and a second-run
//! convergence. This test replays that tree against the native migrations and
//! compares every byte — it never adapts the golden to the native output.
//!
//! Two branches need a real `git` checkout (the former-checkout proof); the test
//! builds it the same way the recording probe did and requires git to be
//! available.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sk_install::migrations::completion_modern::charter_synthesis::provenance;

const ORACLE: &str = include_str!("fixtures/charter_synthesis_provenance_oracle.json");

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

    fn substitute(&self, text: &str) -> String {
        text.replace("$PACKS", &self.path("packs").display().to_string())
            .replace("$PROJECT", &self.path("project").display().to_string())
            .replace("$CURRENT", &self.path("current").display().to_string())
            .replace(
                "$UNPROVABLE",
                &self.path("unprovable").display().to_string(),
            )
            .replace(
                "$FORMER",
                &self.path("former-checkout").display().to_string(),
            )
            .replace(
                "$FORMERPROJECT",
                &self.path("former-project").display().to_string(),
            )
            .replace("$BUNDLE", &self.path("bundle").display().to_string())
            .replace("$COMPOSED", &self.path("composed").display().to_string())
            .replace(
                "$DOCTRINEKEY",
                &self.path("doctrine-key").display().to_string(),
            )
            .replace("$PROJECT", &self.path("a").display().to_string())
            .replace("$COLLISION", &self.path("b").display().to_string())
            .replace("$GAIN", &self.path("a").display().to_string())
            .replace("$SEEDED", &self.path("b").display().to_string())
            .replace("$SETTLED", &self.path("c").display().to_string())
            .replace("$NONE", &self.path("d").display().to_string())
            // Branch-matrix fixtures (each branch owns its own directory).
            .replace(
                "$SINGLE_PACK_LEGACY_FORM",
                &self.path("single").display().to_string(),
            )
            .replace(
                "$ORGANISATION_PACKS",
                &self.path("org").display().to_string(),
            )
            .replace("$MIXEDORG", &self.path("mixed").display().to_string())
            .replace("$NOTMAPPING", &self.path("notmap").display().to_string())
            .replace("$SYMLINK", &self.path("symlink").display().to_string())
            .replace("$MOVEBLOCKED", &self.path("blocked").display().to_string())
            .replace(
                "$MINIMAL_EQUAL",
                &self.path("minimal").display().to_string(),
            )
            .replace(
                "$INSTALLED_REMOVED_SKILLS",
                &self.path("skills").display().to_string(),
            )
            .replace(
                "$LOCK_CONFIG_STORE",
                &self.path("config").display().to_string(),
            )
            .replace(
                "$LOCK_LEDGER_STORE",
                &self.path("ledger").display().to_string(),
            )
            .replace("$GI_SYMLINK", &self.path("gisym").display().to_string())
            .replace("$ACCEPTED", &self.path("acc").display().to_string())
            .replace("$REFUSED", &self.path("ref").display().to_string())
            .replace("$ABSENT", &self.path("c").display().to_string())
            .replace("$COLLISION", &self.path("b").display().to_string())
            .replace("$GAIN", &self.path("a").display().to_string())
            .replace("$SEEDED", &self.path("b").display().to_string())
            .replace("$SETTLED", &self.path("c").display().to_string())
            .replace("$NONE", &self.path("d").display().to_string())
            // Branch-matrix fixtures (each branch owns its own directory).
            .replace(
                "$SINGLE_PACK_LEGACY_FORM",
                &self.path("single").display().to_string(),
            )
            .replace(
                "$ORGANISATION_PACKS",
                &self.path("org").display().to_string(),
            )
            .replace("$MIXEDORG", &self.path("mixed").display().to_string())
            .replace("$NOTMAPPING", &self.path("notmap").display().to_string())
            .replace("$SYMLINK", &self.path("symlink").display().to_string())
            .replace("$MOVEBLOCKED", &self.path("blocked").display().to_string())
            .replace(
                "$MINIMAL_EQUAL",
                &self.path("minimal").display().to_string(),
            )
            .replace(
                "$INSTALLED_REMOVED_SKILLS",
                &self.path("skills").display().to_string(),
            )
            .replace(
                "$LOCK_CONFIG_STORE",
                &self.path("config").display().to_string(),
            )
            .replace(
                "$LOCK_LEDGER_STORE",
                &self.path("ledger").display().to_string(),
            )
            .replace("$GI_SYMLINK", &self.path("gisym").display().to_string())
            .replace("$ACCEPTED", &self.path("acc").display().to_string())
            .replace("$REFUSED", &self.path("ref").display().to_string())
            .replace("$WORKSPACE", &self.root.display().to_string())
    }

    /// Write `files` (paths relative to `root_relative`) beneath the workspace.
    fn write(&self, root_relative: &str, files: &BTreeMap<String, String>) {
        for (relative, text) in files {
            let path = self.root.join(root_relative).join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, self.substitute(text)).expect("write fixture file");
        }
    }

    fn write_packs(&self, packs: &Value) {
        let mut files = BTreeMap::new();
        for (relative, text) in packs.as_object().expect("packs object") {
            files.insert(
                relative.clone(),
                text.as_str().expect("pack text").to_string(),
            );
        }
        self.write("packs", &files);
    }

    fn snapshot(&self, root: &Path) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        collect(root, root, &mut out);
        out
    }
}

fn collect(root: &Path, current: &Path, out: &mut BTreeMap<String, String>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .expect("within root")
            .to_string_lossy()
            .replace('\\', "/");
        let kind = entry.file_type().expect("file type");
        if kind.is_symlink() {
            // Symlinks are compared as literal targets by `actual_symlinks`, so
            // this walk never dereferences (or reads through) one.
            continue;
        }
        if kind.is_dir() {
            // Directories carry no content; only files are compared.
            collect(root, &path, out);
        } else {
            let text = std::fs::read(&path).expect("read file");
            match String::from_utf8(text) {
                Ok(text) => {
                    out.insert(relative, text);
                }
                Err(error) => {
                    out.insert(relative, format!("<binary:{}>", error.into_bytes().len()));
                }
            }
        }
    }
}

fn expected_files(workspace: &Workspace, case: &Value, field: &str) -> BTreeMap<String, String> {
    case[field]
        .as_object()
        .expect("snapshot object")
        .iter()
        .map(|(path, entry)| {
            (
                path.clone(),
                workspace.substitute(entry["text"].as_str().unwrap_or_default()),
            )
        })
        .collect()
}

/// The recorded built-in pack root of this fixture (the explicit tier input the
/// production migrations resolve from `SPEC_KITTY_PACKS_ROOT`).
fn packs_root(workspace: &Workspace) -> std::path::PathBuf {
    workspace.path("packs/built-in")
}

fn compare(actual: &BTreeMap<String, String>, expected: &BTreeMap<String, String>, context: &str) {
    for (path, text) in expected {
        match actual.get(path) {
            Some(found) if found == text => {}
            Some(found) => {
                panic!("{context}: {path} differs\n--- source ---\n{text}\n--- native ---\n{found}")
            }
            None => panic!("{context}: {path} missing from the native tree"),
        }
    }
    for path in actual.keys() {
        assert!(
            expected.contains_key(path),
            "{context}: native wrote {path}, which the Source did not"
        );
    }
}

fn oracle() -> Value {
    serde_json::from_str(ORACLE).expect("fixture parses")
}

fn provenance_paths_heal_exactly_as_the_source_did() {
    let oracle = oracle();
    let case = &oracle["provenance"];
    let workspace = Workspace::new();
    workspace.write_packs(&oracle["packs"]);
    workspace.write("project", &expected_files(&workspace, case, "before"));
    let project = workspace.path("project");
    let root = packs_root(&workspace);

    assert_eq!(
        provenance::detect_with(&project, Some(&root)).expect("detect"),
        case["detect"].as_bool().unwrap(),
        "detect"
    );
    assert_eq!(
        provenance::can_apply_with(&project, Some(&root)).expect("can_apply"),
        expected_can_apply(case),
        "can_apply"
    );

    let outcome = provenance::apply_with(&project, Some(&root)).expect("apply");
    assert_eq!(
        outcome.changes,
        expected(&workspace, &case["result"]["changes_made"])
    );
    assert!(outcome.changed);
    compare(
        &workspace.snapshot(&project),
        &expected_files(&workspace, case, "after"),
        "provenance heal",
    );

    let second = provenance::apply_with(&project, Some(&root)).expect("second apply");
    assert_eq!(
        second.changes,
        expected(&workspace, &case["second_run"]["changes_made"])
    );
    assert!(!second.changed);
    assert!(!provenance::detect_with(&project, Some(&root)).expect("detect after"));
}

/// `(True, "")` in the Source record means "applicable" (`None` natively).
fn expected(workspace: &Workspace, value: &Value) -> Vec<String> {
    strings(value)
        .into_iter()
        .map(|line| workspace.substitute(&line))
        .collect()
}

fn expected_can_apply(case: &Value) -> Option<String> {
    let pair = case["can_apply"].as_array().expect("can_apply pair");
    if pair[0].as_bool().unwrap_or(false) {
        None
    } else {
        Some(pair[1].as_str().unwrap_or_default().to_string())
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("string list")
        .iter()
        .map(|item| item.as_str().expect("string").to_string())
        .collect()
}

fn build_former_checkout(workspace: &Workspace) {
    let checkout = workspace.path("former-checkout");
    std::fs::create_dir_all(checkout.join("packs/built-in/missions/software-dev"))
        .expect("create checkout");
    std::fs::create_dir_all(checkout.join("src/specify_cli")).expect("create checkout src");
    std::fs::write(
        checkout.join("pyproject.toml"),
        "[project]\nname = \"spec-kitty-cli\"\n\n[project.urls]\nRepository = \"https://github.com/spec-kitty/spec-kitty\"\n",
    )
    .expect("write pyproject");
    std::fs::write(
        checkout.join("packs/built-in/missions/software-dev/mission.yaml"),
        "name: software-dev\n",
    )
    .expect("write mission");
    std::fs::write(checkout.join("src/specify_cli/__init__.py"), "").expect("write module");
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "probe@example.com"],
        vec!["config", "user.name", "Probe"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/spec-kitty/spec-kitty.git",
        ],
        vec!["add", "-A"],
        vec!["commit", "-q", "-m", "seed"],
    ] {
        let status = Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(&args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }
}

fn template_set_rows_heal_only_with_proven_checkout_identity() {
    let oracle = oracle();
    let workspace = Workspace::new();
    workspace.write_packs(&oracle["packs"]);
    build_former_checkout(&workspace);

    for (name, directory) in [
        ("current_root", "current"),
        ("unprovable", "unprovable"),
        ("former_checkout", "former-project"),
    ] {
        let case = &oracle["template_set"][name];
        workspace.write(directory, &expected_files(&workspace, case, "before"));
        let project = workspace.path(directory);
        let root = packs_root(&workspace);
        assert_eq!(
            provenance::detect_template_set_with(&project, Some(&root)).expect("detect"),
            case["detect"].as_bool().unwrap(),
            "{name}: detect"
        );
        assert_eq!(
            provenance::describe_template_set_leaks_with(&project, Some(&root)).expect("leaks"),
            expected(&workspace, &case["leaks"]),
            "{name}: leaks"
        );
        let ambiguities = provenance::describe_template_set_ambiguities_with(&project, Some(&root))
            .expect("ambiguities");
        assert_eq!(
            ambiguities.len(),
            case["ambiguities"].as_array().unwrap().len(),
            "{name}: ambiguity count"
        );
        let outcome = provenance::apply_template_set_with(&project, Some(&root)).expect("apply");
        // The archive directory is stamped with the run's own UTC instant, so
        // both sides are bound through the same placeholder; nothing else in a
        // change line is volatile.
        let actual: Vec<String> = outcome
            .changes
            .iter()
            .map(|line| normalise_volatile(line))
            .collect();
        let wanted: Vec<String> = expected(&workspace, &case["result"]["changes_made"])
            .iter()
            .map(|line| normalise_volatile(line))
            .collect();
        assert_eq!(actual, wanted, "{name}: changes");
        compare(
            &workspace.snapshot(&project),
            &expected_files(&workspace, case, "after"),
            name,
        );
        let second =
            provenance::apply_template_set_with(&project, Some(&root)).expect("second apply");
        assert_eq!(
            second.changes,
            expected(&workspace, &case["second_run"]["changes_made"])
        );
        assert!(
            !provenance::detect_template_set_with(&project, Some(&root)).expect("detect after")
        );
    }
}

// ---------------------------------------------------------------------------
// `consolidate_charter_bundle_fold`
// ---------------------------------------------------------------------------

#[test]
fn the_charter_synthesis_effects_match_the_source() {
    provenance_paths_heal_exactly_as_the_source_did();
    template_set_rows_heal_only_with_proven_checkout_identity();
    bundles_fold_exactly_as_the_source_did();
    the_legacy_project_intent_rename_matches_the_source();
    the_charter_pack_cutover_matches_the_source();
    the_promote_interview_selections_matches_the_source();
    the_charter_pack_cutover_matches_the_source();
    the_charter_pack_cutover_branches_match_the_source();
    the_fold_coercions_match_the_source();
}

const FOLD_ORACLE: &str = include_str!("fixtures/charter_synthesis_fold_oracle.json");

/// The composed `metadata.generated_at` is genuinely "now"; the backup-directory
/// stamp is the archive's own timestamp. Both are bound by asserting their shape
/// and then replacing them with the recorded placeholder, never by pretending
/// they are stable.
fn normalise_volatile(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find(".backup-") {
        out.push_str(&rest[..position + ".backup-".len()]);
        let tail = &rest[position + ".backup-".len()..];
        // An already-substituted placeholder is consumed as itself, so the
        // normaliser is idempotent.
        let consumed = if tail.starts_with("$STAMP") {
            "$STAMP".len()
        } else {
            backup_token_length(tail)
        };
        out.push_str("$STAMP");
        rest = &tail[consumed..];
    }
    out.push_str(rest);
    normalise_stamp_suffix(&out)
}

/// `<YYYYMMDD>T<HHMMSS>Z`; a `-<n>` uniquifier stays visible so two archives of
/// the same second are still distinguishable.
fn backup_token_length(tail: &str) -> usize {
    let characters: Vec<char> = tail.chars().collect();
    let mut index = 0usize;
    while index < 16 {
        match characters.get(index) {
            Some(character)
                if character.is_ascii_digit() || *character == 'T' || *character == 'Z' =>
            {
                index += 1;
            }
            _ => return 0,
        }
    }
    // The `-<n>` uniquifier counts archives already made in that same second, so
    // it depends on the run's wall clock (the Source record happened inside one
    // second and therefore numbers 1,2,3; a slower run may start a new stamp
    // instead). Both forms collapse to the same placeholder.
    index
}

/// Drop a `-<n>` uniquifier after an already-substituted `$STAMP`.
fn normalise_stamp_suffix(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find("$STAMP-") {
        out.push_str(&rest[..position + "$STAMP".len()]);
        let tail = &rest[position + "$STAMP-".len()..];
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            out.push_str(&tail[..0]);
            out.push('-');
            rest = &tail[0..];
        } else {
            rest = &tail[digits..];
        }
    }
    out.push_str(rest);
    out
}

fn normalise_metadata_stamp(text: &str) -> String {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("generated_at:") {
            let value = line
                .trim_start()
                .trim_start_matches("generated_at:")
                .trim()
                .trim_matches(['\'', '"']);
            assert!(
                value.len() == 20 && value.ends_with('Z') && value.contains('T'),
                "generated_at must be the Source's second-precision UTC stamp, got {value:?}"
            );
            let indent = &line[..line.len() - line.trim_start().len()];
            out.push_str(&format!("{indent}generated_at: <NOW>\n"));
        } else {
            out.push_str(line);
        }
    }
    out
}

fn normalise(map: BTreeMap<String, String>, composed_now: bool) -> BTreeMap<String, String> {
    map.into_iter()
        // The snapshot records directories with a trailing `/`; the recorded
        // Source fixtures list files only, and a directory is never content.
        .filter(|(path, _)| !path.ends_with('/'))
        .map(|(path, text)| {
            let path = normalise_volatile(&path);
            let text = normalise_volatile(&text);
            // Only a freshly composed `charter.yaml` carries the run's own
            // `generated_at`; an already-authored bundle keeps its recorded
            // value byte for byte.
            let text = if composed_now && path.ends_with("charter/charter.yaml") {
                normalise_metadata_stamp(&text)
            } else {
                text
            };
            (path, text)
        })
        .collect()
}

fn bundles_fold_exactly_as_the_source_did() {
    let oracle: Value = serde_json::from_str(FOLD_ORACLE).expect("fold fixture parses");
    for (name, directory) in [
        ("bundle", "bundle"),
        ("composed", "composed"),
        ("legacy_doctrine_key", "doctrine-key"),
    ] {
        let case = &oracle[name];
        let workspace = Workspace::new();
        let mut files = BTreeMap::new();
        for (path, text) in case["before"].as_object().expect("before") {
            files.insert(
                path.replace("$BUNDLE", "")
                    .replace("$COMPOSED", "")
                    .replace("$DOCTRINEKEY", "")
                    .trim_start_matches('/')
                    .to_string(),
                text.as_str().unwrap_or_default().to_string(),
            );
        }
        workspace.write(directory, &files);
        let project = workspace.path(directory);
        // The recorded `detect` is the POST-apply value (the probe recorded it
        // after running the migration), so it is compared after the apply
        // below. The `bundle` case additionally records a pre-apply `detect`
        // expectation in its `before`/`result` pair.
        if name == "bundle" {
            assert_eq!(
                sk_install::migrations::completion_modern::charter_synthesis::fold::detect(
                    &project
                )
                .expect("detect"),
                case["detect"].as_bool().unwrap(),
                "bundle: detect before apply"
            );
        }
        let expected_tree: BTreeMap<String, String> = case["after"]
            .as_object()
            .expect("after")
            .iter()
            .map(|(path, text)| {
                (
                    path.replace("$BUNDLE", "")
                        .replace("$COMPOSED", "")
                        .replace("$DOCTRINEKEY", "")
                        .trim_start_matches('/')
                        .to_string(),
                    text.as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let applied =
            sk_install::migrations::completion_modern::charter_synthesis::fold::apply(&project);
        if name == "legacy_doctrine_key" {
            // The Source CATCHES the refusal and returns a failed
            // `MigrationResult` (nothing is written), so the recorded payload is
            // `success = false` with the refusal text as its error — not a raised
            // error, which would drop the report.
            let outcome = applied.expect("apply");
            assert!(
                !outcome.success,
                "{name}: the recorded Source result failed"
            );
            assert!(outcome.changes.is_empty(), "{name}: no change lines");
            assert_eq!(
                outcome.errors,
                expected(&workspace, &case["result"]["errors"]),
                "{name}: errors"
            );
            assert!(!outcome.changed, "{name}: nothing was written");
            compare(
                &normalise(workspace.snapshot(&project), false),
                &normalise(expected_tree, false),
                name,
            );
            continue;
        }
        let detect_after =
            sk_install::migrations::completion_modern::charter_synthesis::fold::detect(&project)
                .expect("detect after");
        if name == "bundle" {
            // The recorded value for this case is the PRE-apply verdict; a
            // converged fold reports a clean no-op afterwards.
            assert!(!detect_after, "bundle: detect after apply");
        } else {
            assert_eq!(
                detect_after,
                case["detect"].as_bool().unwrap(),
                "{name}: detect after apply"
            );
        }
        let outcome = applied.expect("apply");
        // The archive directory is stamped with the run's own UTC instant, so
        // both sides are bound through the same placeholder; nothing else in a
        // change line is volatile.
        let actual: Vec<String> = outcome
            .changes
            .iter()
            .map(|line| normalise_volatile(line))
            .collect();
        let wanted: Vec<String> = expected(&workspace, &case["result"]["changes_made"])
            .iter()
            .map(|line| normalise_volatile(line))
            .collect();
        assert_eq!(actual, wanted, "{name}: changes");
        let composed_now = name == "bundle";
        compare(
            &normalise(workspace.snapshot(&project), composed_now),
            &normalise(expected_tree, composed_now),
            name,
        );
    }
}

// ---------------------------------------------------------------------------
// `4.0.0rc8_project_intent_directive_rename`
// ---------------------------------------------------------------------------

const RENAME_ORACLE: &str = include_str!("fixtures/charter_synthesis_rename_oracle.json");

/// The `kind: file` members of a recorded snapshot, as substituted text.
///
/// Directory members are compared separately ([`expected_dirs`]) because the
/// native snapshot collects file content only; folding the two would write an
/// empty file where the Source has a directory.
fn expected_file_members(
    workspace: &Workspace,
    case: &Value,
    field: &str,
) -> BTreeMap<String, String> {
    recorded_members(case, field)
        .filter(|(_, entry)| is_file(entry))
        .map(|(path, entry)| {
            let text = entry["text"].as_str().unwrap_or_default();
            (path, workspace.substitute(text))
        })
        .collect()
}

fn recorded_members<'a>(
    case: &'a Value,
    field: &str,
) -> impl Iterator<Item = (String, &'a Value)> + 'a {
    case[field]
        .as_object()
        .expect("snapshot object")
        .iter()
        .map(|(path, entry)| (path.clone(), entry))
}

fn is_file(entry: &Value) -> bool {
    entry["kind"].as_str() == Some("file")
}

/// Directory members of a recorded snapshot (POSIX, project-relative).
fn expected_dirs(case: &Value, field: &str) -> std::collections::BTreeSet<String> {
    recorded_members(case, field)
        .filter(|(_, entry)| entry["kind"].as_str() == Some("dir"))
        .map(|(path, _)| path)
        .collect()
}

/// Directory members of a real tree, so a directory the Source kept cannot be
/// silently dropped by a native step that only moves files.
fn actual_dirs(root: &Path) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    fn walk(root: &Path, current: &Path, out: &mut std::collections::BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            out.insert(
                path.strip_prefix(root)
                    .expect("within root")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            walk(root, &path, out);
        }
    }
    walk(root, root, &mut out);
    out
}

/// Write the `kind: file` members of a recorded snapshot beneath `root_relative`.
fn write_recorded(workspace: &Workspace, root_relative: &str, case: &Value, field: &str) {
    workspace.write(
        root_relative,
        &expected_file_members(workspace, case, field),
    );
    // A recorded symlink is part of the Source tree; materialise the literal
    // target so a branch that moves symlinks is exercised for real.
    for (path, target) in expected_symlinks(workspace, case, field) {
        let link = workspace.root.join(root_relative).join(&path);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).expect("create symlink parent");
        }
        std::os::unix::fs::symlink(&target, &link).expect("create recorded symlink");
    }
}

/// The `kind: symlink` members of a recorded snapshot, as substituted targets.
fn expected_symlinks(workspace: &Workspace, case: &Value, field: &str) -> BTreeMap<String, String> {
    recorded_members(case, field)
        .filter(|(_, entry)| entry["kind"].as_str() == Some("symlink"))
        .map(|(path, entry)| {
            let target = entry["target"].as_str().unwrap_or_default();
            (path, workspace.substitute(target))
        })
        .collect()
}

/// Every symlink of a real tree (POSIX path -> literal target).
fn actual_symlinks(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                out.insert(
                    path.strip_prefix(root)
                        .expect("within root")
                        .to_string_lossy()
                        .replace('\\', "/"),
                    std::fs::read_link(&path)
                        .expect("read link")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            } else if kind.is_dir() {
                stack.push(path);
            }
        }
    }
    out
}

/// The activation ledger (a bundle-content-hash member).
const ACTIVATION_LEDGER_REL: &str = ".kittify/charter/activation.yaml";

/// The canonical bundle stamp over two members, through the ONE recipe
/// (`sk_charter::hash_content`, exactly what [`compute_bundle_content_hash`]
/// combines: the required member then each present optional member).
fn bundle_stamp(charter: &str, ledger: &str) -> String {
    let digests = format!(
        "{}\n{}",
        sk_charter::hash_content(charter),
        sk_charter::hash_content(ledger)
    );
    sk_charter::hash_content(&digests)
}

/// One digest value from a synthesis manifest, in either the inline or ruamel's
/// folded form.
fn manifest_digest(text: &str, key: &str) -> String {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim_start_matches(':').trim();
            if !rest.is_empty() {
                return rest.to_string();
            }
            return lines
                .find(|next| !next.trim().is_empty())
                .map(|next| next.trim().to_string())
                .unwrap_or_default();
        }
    }
    panic!("{key} absent from the manifest");
}

fn the_legacy_project_intent_rename_matches_the_source() {
    use sk_install::migrations::completion_modern::charter_synthesis::legacy_rename;

    let oracle: Value = serde_json::from_str(RENAME_ORACLE).expect("rename fixture parses");
    let workspace = Workspace::new();

    // -- present legacy state: dry run, apply, convergence --------------------
    let case = &oracle["rename"];
    let project = workspace.path("a");
    write_recorded(&workspace, "a", case, "before");
    assert_eq!(
        legacy_rename::detect(&project).expect("detect"),
        case["detect"].as_bool().unwrap(),
        "rename: detect"
    );
    assert_eq!(
        legacy_rename::can_apply(&project).expect("can_apply"),
        expected_can_apply(case),
        "rename: can_apply"
    );
    // The reference's dry run is the read-only plan: it returns the action lines
    // and writes nothing, so the tree must still equal the recorded before.
    assert_eq!(
        legacy_rename::preflight(&project).expect("dry run"),
        expected(&workspace, &case["dry_actions"]),
        "rename: dry actions"
    );
    compare_recorded(
        &workspace,
        &project,
        case,
        "before",
        "rename: dry run wrote nothing",
    );
    let outcome = legacy_rename::rename(&project).expect("apply");
    assert!(outcome.changed, "rename: changed");
    assert_eq!(
        outcome.changes,
        expected(&workspace, &case["result"]["changes_made"]),
        "rename: changes"
    );
    assert_eq!(
        outcome.warnings,
        expected(&workspace, &case["result"]["warnings"]),
        "rename: warnings"
    );
    compare_recorded(&workspace, &project, case, "after", "rename: after");

    // The ledger and its whole digest chain are byte-identical: the canonical
    // recipe reproduces the recorded Source bundle stamp from the recorded
    // Source bytes AND from the bytes actually written (no mask, no offset,
    // no hash normalisation).
    let written = workspace.snapshot(&project);
    let source = expected_file_members(&workspace, case, "after");
    assert_eq!(
        written[ACTIVATION_LEDGER_REL], source[ACTIVATION_LEDGER_REL],
        "rename: the ledger is byte-identical to the Source"
    );
    let source_bundle = manifest_digest(
        &source[".kittify/charter/synthesis-manifest.yaml"],
        "bundle_content_hash",
    );
    let native_bundle = manifest_digest(
        &written[".kittify/charter/synthesis-manifest.yaml"],
        "bundle_content_hash",
    );
    assert_eq!(
        bundle_stamp(
            &source[".kittify/charter/charter.yaml"],
            &source[ACTIVATION_LEDGER_REL]
        ),
        source_bundle,
        "rename: the canonical recipe reproduces the recorded Source bundle stamp"
    );
    assert_eq!(
        bundle_stamp(
            &written[".kittify/charter/charter.yaml"],
            &written[ACTIVATION_LEDGER_REL]
        ),
        native_bundle,
        "rename: the canonical recipe reproduces the stamp over the written bytes"
    );
    assert_eq!(
        native_bundle, source_bundle,
        "rename: the written bundle stamp equals the Source's"
    );
    assert_eq!(
        manifest_digest(
            &written[".kittify/charter/synthesis-manifest.yaml"],
            "manifest_hash"
        ),
        manifest_digest(
            &source[".kittify/charter/synthesis-manifest.yaml"],
            "manifest_hash"
        ),
        "rename: the written manifest_hash equals the Source's"
    );
}

// ---------------------------------------------------------------------------
// `4.0.0rc8_promote_interview_selections`
// ---------------------------------------------------------------------------

const PROMOTE_ORACLE: &str = include_str!("fixtures/charter_synthesis_promote_oracle.json");

fn the_promote_interview_selections_matches_the_source() {
    use sk_install::migrations::completion_modern::charter_synthesis::promote;

    let oracle: Value = serde_json::from_str(PROMOTE_ORACLE).expect("promote fixture parses");
    let workspace = Workspace::new();

    // -- a present list gains the resolved ids; an absent key is never written --
    let case = &oracle["gain"];
    let project = workspace.path("a");
    write_recorded(&workspace, "a", case, "before");
    assert_eq!(
        promote::detect(&project).expect("detect"),
        case["detect"].as_bool().unwrap(),
        "promote: detect is always true"
    );
    assert_eq!(
        promote::can_apply(&project).expect("can_apply"),
        expected_can_apply(case),
        "promote: can_apply"
    );
    let dry = promote::apply_dry_run(&project).expect("dry run");
    assert_eq!(
        dry.changes,
        expected(&workspace, &case["dry_result"]["changes_made"]),
        "promote: dry-run change"
    );
    assert_eq!(
        dry.warnings,
        expected(&workspace, &case["dry_result"]["warnings"]),
        "promote: dry-run warnings"
    );
    compare_recorded(
        &workspace,
        &project,
        case,
        "after_dry",
        "promote: the dry run wrote only its lock file",
    );
    let outcome = promote::apply(&project).expect("apply");
    assert_eq!(
        outcome.changes,
        expected(&workspace, &case["result"]["changes_made"]),
        "promote: changes"
    );
    assert_eq!(
        outcome.warnings,
        expected(&workspace, &case["result"]["warnings"]),
        "promote: warnings"
    );
    compare_recorded(&workspace, &project, case, "after", "promote: after");
    let second = promote::apply(&project).expect("second apply");
    assert_eq!(
        second.changes,
        expected(&workspace, &case["second_result"]["changes_made"]),
        "promote: second-run changes"
    );
    assert_eq!(
        second.warnings,
        expected(&workspace, &case["second_result"]["warnings"]),
        "promote: second-run warnings"
    );

    // -- the remaining Source cases: seeded, settled ledger, no selections -----
    //
    // Each of these changes nothing, so the recorded post-state is also its
    // input; replaying it must reproduce the recorded result AND the same tree.
    for (directory, field, context) in [
        ("b", "seeded", "promote: seeded list"),
        ("c", "settled", "promote: settled ledger"),
        ("d", "no_selections", "promote: no selections"),
    ] {
        let case = &oracle[field];
        let project = workspace.path(directory);
        write_recorded(&workspace, directory, case, "after");
        let outcome = promote::apply(&project).expect("apply");
        assert_eq!(
            outcome.changes,
            expected(&workspace, &case["result"]["changes_made"]),
            "{context}: changes"
        );
        assert_eq!(
            outcome.warnings,
            expected(&workspace, &case["result"]["warnings"]),
            "{context}: warnings"
        );
        compare_recorded(&workspace, &project, case, "after", context);
    }
}

// ---------------------------------------------------------------------------
// `charter_pack_cutover`
// ---------------------------------------------------------------------------

const CUTOVER_ORACLE: &str = include_str!("fixtures/charter_synthesis_cutover_oracle.json");
const CUTOVER_BRANCHES: &str =
    include_str!("fixtures/charter_synthesis_cutover_branches_oracle.json");

/// Compare a real tree with a recorded snapshot: file bytes, directory set, and
/// nothing extra on either side.
fn compare_recorded(
    workspace: &Workspace,
    project: &Path,
    case: &Value,
    field: &str,
    context: &str,
) {
    compare(
        &workspace.snapshot(project),
        &expected_file_members(workspace, case, field),
        context,
    );
    assert_eq!(
        actual_dirs(project),
        expected_dirs(case, field),
        "{context}: directory set"
    );
}

/// The skills-manifest timestamps are the run's own UTC instant and the library
/// version is the running CLI's: both are volatile, so they are bound on BOTH
/// sides. Nothing else in the manifest is normalised — entry order, retained
/// fields and the dropped `source_ref`/`source_hash` stay load-bearing.
fn bind_cutover_volatile(text: &str) -> String {
    let mut out = text.to_string();
    for key in ["created_at", "updated_at", "spec_kitty_version"] {
        out = replace_scalar_value(&out, key);
    }
    out
}

fn replace_scalar_value(text: &str, key: &str) -> String {
    let needle = format!("\"{key}\": \"");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find(&needle) {
        out.push_str(&rest[..position + needle.len()]);
        let tail = &rest[position + needle.len()..];
        match tail.find('"') {
            Some(end) => {
                out.push_str("$VOLATILE");
                rest = &tail[end..];
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn bind_cutover_tree(
    tree: &mut BTreeMap<String, String>,
    case: &Value,
    field: &str,
) -> BTreeMap<String, String> {
    let mut wanted = expected_file_members_from(case, field);
    for (path, text) in tree.iter_mut() {
        if path.ends_with("skills-manifest.json") {
            *text = bind_cutover_volatile(text);
        }
    }
    for (path, text) in wanted.iter_mut() {
        if path.ends_with("skills-manifest.json") {
            *text = bind_cutover_volatile(text);
        }
    }
    wanted
}

fn without_git(tree: BTreeMap<String, String>) -> BTreeMap<String, String> {
    tree.into_iter()
        .filter(|(path, _)| !(path == ".git" || path.starts_with(".git/")))
        .collect()
}

fn dirs_without_git(
    dirs: std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    dirs.into_iter()
        .filter(|path| !(path == ".git" || path.starts_with(".git/")))
        .collect()
}

fn expected_file_members_from(case: &Value, field: &str) -> BTreeMap<String, String> {
    recorded_members(case, field)
        .filter(|(_, entry)| is_file(entry))
        .map(|(path, entry)| (path, entry["text"].as_str().unwrap_or_default().to_string()))
        .collect()
}

/// A `success=false` Source result must arrive as an `Ok` outcome carrying the
/// whole payload (report JSON, errors, review flag, preserved paths), never as a
/// generic refusal.
fn assert_failed_outcome(
    outcome: &sk_install::upgrade::MigrationOutcome,
    case: &Value,
    workspace: &Workspace,
    context: &str,
) {
    assert!(!outcome.success, "{context}: success must be false");
    assert!(!outcome.errors.is_empty(), "{context}: errors carried");
    assert_eq!(
        outcome.errors,
        expected(workspace, &case["result"]["errors"]),
        "{context}: errors"
    );
    assert_eq!(
        outcome.changes[0],
        expected(workspace, &case["result"]["changes_made"])[0],
        "{context}: report JSON carried on the failure path"
    );
}

fn the_charter_pack_cutover_matches_the_source() {
    use sk_install::migrations::completion_modern::charter_synthesis::cutover;

    let oracle: Value = serde_json::from_str(CUTOVER_ORACLE).expect("cutover fixture parses");
    let workspace = Workspace::new();
    let case = &oracle["cutover"];
    let project = workspace.path("a");
    write_recorded(&workspace, "a", case, "before");

    assert_eq!(
        cutover::detect(&project).expect("detect"),
        case["detect"].as_bool().unwrap(),
        "cutover: detect"
    );
    assert_eq!(
        cutover::structural_detect(&project).expect("structural_detect"),
        case["structural_detect"].as_bool().unwrap(),
        "cutover: structural_detect"
    );
    assert_eq!(
        cutover::is_first_application(&project),
        case["is_first_application"].as_bool().unwrap(),
        "cutover: is_first_application"
    );
    assert_eq!(
        cutover::can_apply(&project).expect("can_apply"),
        expected_can_apply(case),
        "cutover: can_apply"
    );

    let dry = cutover::dry_run(&project).expect("dry run");
    assert_eq!(
        dry.changes,
        expected(&workspace, &case["dry_result"]["changes_made"]),
        "cutover: dry-run changes"
    );
    assert_eq!(
        dry.warnings,
        expected(&workspace, &case["dry_result"]["warnings"]),
        "cutover: dry-run warnings"
    );
    compare_recorded(
        &workspace,
        &project,
        case,
        "after_dry",
        "cutover: the dry run wrote nothing",
    );

    let outcome = cutover::apply(&project).expect("apply");
    assert!(outcome.success, "cutover: the source run succeeded");
    assert_eq!(
        outcome.changes,
        expected(&workspace, &case["result"]["changes_made"]),
        "cutover: changes"
    );
    assert_eq!(
        outcome.warnings,
        expected(&workspace, &case["result"]["warnings"]),
        "cutover: warnings"
    );
    assert_eq!(
        outcome.errors,
        expected(&workspace, &case["result"]["errors"]),
        "cutover: errors"
    );
    assert_eq!(
        outcome.manual_review_required,
        case["result"]["manual_review_required"].as_bool().unwrap(),
        "cutover: manual_review_required"
    );
    assert_eq!(
        outcome.preserved_paths,
        expected(&workspace, &case["result"]["preserved_paths"]),
        "cutover: preserved_paths"
    );

    let mut actual = workspace.snapshot(&project);
    let wanted = bind_cutover_tree(&mut actual, case, "after");
    compare(&actual, &wanted, "cutover: after");
    assert_eq!(
        actual_dirs(&project),
        expected_dirs(case, "after"),
        "cutover: directory set"
    );

    assert_eq!(
        cutover::detect(&project).expect("detect after"),
        case["detect_after"].as_bool().unwrap(),
        "cutover: detect after"
    );
    assert_eq!(
        cutover::structural_detect(&project).expect("structural detect after"),
        case["structural_detect_after"].as_bool().unwrap(),
        "cutover: structural_detect after"
    );

    let second = cutover::apply(&project).expect("second apply");
    assert_eq!(
        second.changes,
        expected(&workspace, &case["second_result"]["changes_made"]),
        "cutover: second-run changes"
    );

    // -- a differing-content collision refuses with the FULL payload ----------
    let case = &oracle["collision"];
    let project = workspace.path("b");
    write_recorded(&workspace, "b", case, "before");
    assert_eq!(
        cutover::detect(&project).expect("collision detect"),
        case["detect"].as_bool().unwrap(),
        "collision: detect"
    );
    let refused = cutover::apply(&project).expect("collision apply is a business result");
    assert_failed_outcome(&refused, case, &workspace, "collision");
    assert_eq!(
        refused.changes.len(),
        1,
        "collision: only the report JSON, no human lines"
    );
    assert!(
        refused.partial_writes.is_empty(),
        "collision: nothing was persisted"
    );
    compare_recorded(
        &workspace,
        &project,
        case,
        "before",
        "collision: nothing written",
    );
}

/// The branch matrix: every continuation branch now runs over its own recorded
/// Source tree (before, dry, applied, after, second run), so the eight formerly
/// unprobed branches are byte-verified rather than read-code claims.
fn the_charter_pack_cutover_branches_match_the_source() {
    use sk_install::migrations::completion_modern::charter_synthesis::cutover;

    let oracle: Value = serde_json::from_str(CUTOVER_BRANCHES).expect("branch fixture parses");
    let workspace = Workspace::new();

    /// One standard branch: before → dry (nothing written) → apply → second run.
    fn standard(
        workspace: &Workspace,
        oracle: &Value,
        key: &str,
        directory: &str,
    ) -> std::collections::BTreeMap<String, String> {
        let case = &oracle[key];
        let project = workspace.path(directory);
        write_recorded(workspace, directory, case, "before");
        assert_eq!(
            cutover::detect(&project).expect("detect"),
            case["detect"].as_bool().unwrap(),
            "{key}: detect"
        );
        let dry = cutover::dry_run(&project).expect("dry run");
        assert_eq!(
            dry.changes,
            expected(workspace, &case["dry_result"]["changes_made"]),
            "{key}: dry changes"
        );
        // The dry run must leave the tree exactly as it was (the Source
        // template's `.git` is excluded: the cutover never touches it and the
        // fixture materialises files only).
        let mut dry_actual = workspace.snapshot(&project);
        let dry_wanted = bind_cutover_tree(&mut dry_actual, case, "after_dry");
        compare(&without_git(dry_actual), &without_git(dry_wanted), key);
        assert_eq!(
            dirs_without_git(actual_dirs(&project)),
            dirs_without_git(expected_dirs(case, "after_dry")),
            "{key}: dry-run directory set"
        );
        let outcome = cutover::apply(&project).expect("apply");
        assert!(outcome.success, "{key}: success");
        assert_eq!(
            outcome.changes,
            expected(workspace, &case["result"]["changes_made"]),
            "{key}: changes"
        );
        assert_eq!(
            outcome.warnings,
            expected(workspace, &case["result"]["warnings"]),
            "{key}: warnings"
        );
        assert_eq!(
            outcome.errors,
            expected(workspace, &case["result"]["errors"]),
            "{key}: errors"
        );
        let mut actual = workspace.snapshot(&project);
        let wanted = bind_cutover_tree(&mut actual, case, "after");
        compare(&without_git(actual), &without_git(wanted), key);
        assert_eq!(
            dirs_without_git(actual_dirs(&project)),
            dirs_without_git(expected_dirs(case, "after")),
            "{key}: directory set (the Source template's .git excluded)"
        );
        assert_eq!(
            actual_symlinks(&project),
            expected_symlinks(workspace, case, "after"),
            "{key}: symlink set and literal targets"
        );
        let mut actual = workspace.snapshot(&project);
        let _ = &mut actual;
        let second = cutover::apply(&project).expect("second apply");
        assert_eq!(
            second.changes,
            expected(workspace, &case["second_result"]["changes_made"]),
            "{key}: second-run changes"
        );
        actual
    }

    for (key, directory) in [
        ("single_pack", "single"),
        ("organisation_packs", "org"),
        ("mixed_organisation_packs", "mixed"),
        ("not_a_mapping", "notmap"),
        ("symlink_in_move", "symlink"),
        ("minimal_equal", "minimal"),
        ("installed_removed_skills", "skills"),
        ("gitignore_symlink", "gisym"),
    ] {
        standard(&workspace, &oracle, key, directory);
    }

    // -- the failure branch: real footprint, no fabricated success, retry ------
    let case = &oracle["move_blocked"];
    let project = workspace.path("blocked");
    write_recorded(&workspace, "blocked", case, "before");
    let blocker = project.join(".kittify/charter-packs");
    std::fs::write(&blocker, "not a directory\n").expect("occlude the pack root");
    let dry = cutover::dry_run(&project).expect("dry run");
    assert!(
        dry.success,
        "move_blocked: the dry run cannot see the IO obstruction (Source parity)"
    );
    assert_eq!(
        dry.changes,
        expected(&workspace, &case["dry_result"]["changes_made"]),
        "move_blocked: dry changes"
    );
    let failed = cutover::apply(&project).expect("a business failure is an Ok outcome");
    assert_failed_outcome(&failed, case, &workspace, "move_blocked");
    assert!(
        failed.partial_writes.is_empty(),
        "move_blocked: nothing was persisted, so no partial write is claimed"
    );
    compare_recorded(
        &workspace,
        &project,
        case,
        "failed_phase_footprint",
        "move_blocked: failed-phase footprint",
    );
    std::fs::remove_file(&blocker).expect("release the obstruction");
    let retry = cutover::apply(&project).expect("retry");
    assert!(retry.success, "move_blocked: the retry succeeds");
    assert_eq!(
        retry.changes,
        expected(&workspace, &case["retry_result"]["changes_made"]),
        "move_blocked: retry changes"
    );
    let mut actual = workspace.snapshot(&project);
    let wanted = bind_cutover_tree(&mut actual, case, "after_retry");
    compare(&actual, &wanted, "move_blocked: after retry");
    assert_eq!(
        cutover::detect(&project).expect("detect after retry"),
        case["detect_after_retry"].as_bool().unwrap(),
        "move_blocked: detect after retry"
    );

    // -- the dry-run lock phase, in both activation-store shapes ---------------
    for (label, key) in [("config_store", "config"), ("ledger_store", "ledger")] {
        let case = &oracle["dry_run_lock"][label];
        let project = workspace.path(key);
        write_recorded(&workspace, key, case, "before");
        let before = workspace.snapshot(&project);
        let dry = cutover::dry_run(&project).expect("dry run");
        assert_eq!(
            dry.changes,
            expected(&workspace, &case["dry_result"]["changes_made"]),
            "{label}: dry changes"
        );
        assert_eq!(
            workspace.snapshot(&project),
            before,
            "{label}: the dry run adds no file at all (no lock footprint)"
        );
        let applied = cutover::apply(&project).expect("apply");
        assert!(applied.success, "{label}: success");
        let mut actual = workspace.snapshot(&project);
        let wanted = bind_cutover_tree(&mut actual, case, "after");
        compare(&without_git(actual), &without_git(wanted), label);
    }
}

// ---------------------------------------------------------------------------
// `consolidate_charter_bundle_fold` — the pydantic coercion table
// ---------------------------------------------------------------------------

const FOLD_COERCIONS: &str = include_str!("fixtures/charter_synthesis_fold_coercions_oracle.json");

/// The coercions pydantic v2 accepts compose into the recorded bytes, and the one
/// it refuses writes nothing (the Source lets the `ValidationError` propagate, so
/// the native door is an `Err` with the same "nothing persisted" footprint).
fn the_fold_coercions_match_the_source() {
    use sk_install::migrations::completion_modern::charter_synthesis::fold;

    let oracle: Value = serde_json::from_str(FOLD_COERCIONS).expect("coercion fixture parses");
    let workspace = Workspace::new();

    // -- accepted: integral float, "yes"/1 booleans, numeric strings, int float --
    let case = &oracle["accepted"];
    let project = workspace.path("acc");
    write_recorded(&workspace, "acc", case, "before");
    let outcome = fold::apply(&project).expect("apply");
    // The archive directory is stamped with the run's own UTC instant, so both
    // sides are bound through the same placeholder (nothing else is volatile).
    let actual: Vec<String> = outcome
        .changes
        .iter()
        .map(|line| normalise_volatile(line))
        .collect();
    let wanted: Vec<String> = expected(&workspace, &case["result"]["changes_made"])
        .iter()
        .map(|line| normalise_volatile(line))
        .collect();
    assert_eq!(actual, wanted, "coercions: changes");
    assert_eq!(
        outcome.errors,
        expected(&workspace, &case["result"]["errors"]),
        "coercions: errors"
    );
    let expected_tree: BTreeMap<String, String> = expected_file_members_from(case, "after");
    compare(
        &normalise(workspace.snapshot(&project), true),
        &normalise(expected_tree, true),
        "coercions: composed bytes",
    );

    // -- refused: a fractional float in an int field -------------------------
    let case = &oracle["refused"];
    let project = workspace.path("ref");
    write_recorded(&workspace, "ref", case, "before");
    let refused = fold::apply(&project);
    let detail = match refused {
        Err(sk_install::error::InstallError::UpgradeRefused { detail }) => detail,
        other => panic!("refused: expected a refusal, got {other:?}"),
    };
    assert!(
        detail.contains("pr_approvals"),
        "refused: the refusal names the field: {detail}"
    );
    assert_eq!(
        workspace.snapshot(&project),
        expected_file_members_from(case, "before"),
        "refused: nothing was written"
    );
}
