//! CF84 — Source same-instance constructor-failure recovery and success-only publication.
//!
//! These controls call the pre-existing public APIs only. Each one is a Native
//! business observation, not a compiler/harness shape: the reference publishes an
//! identity table only after a completed scan and a repository/mapping only after
//! a completed construction, so repairing the failing file and retrying the SAME
//! instance recovers. The Native caches under test memoise the failed
//! construction instead, which is the defect this lane closes.
//!
//! Source-first measurement (pinned 3.11.15 child, independent business tree) is
//! recorded in `/tmp/sk-cf84-source-retry-cache-report.md`; this file is the
//! Native side and its controls.

use std::fs;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_doctrine::{CanonicalOfferingService, IdentityIndex, OfferingContext};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn built_in_root() -> PathBuf {
    repository_root().join("packs/built-in")
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture directory");
    }
    fs::write(path, text).expect("fixture file");
}

fn built_in(relative: &str) -> String {
    fs::read_to_string(built_in_root().join(relative))
        .unwrap_or_else(|error| panic!("read built-in {relative}: {error}"))
}

/// A whole, schema-valid agent profile under `id` (the shipped implementer body).
fn profile_doc(id: &str) -> String {
    built_in("agent_profiles/implementer-ivan.agent.yaml")
        .replace("profile-id: implementer-ivan", &format!("profile-id: {id}"))
}

fn tactic(id: &str) -> String {
    format!("id: {id}\nschema_version: \"1.0\"\nname: {id}\nsteps:\n  - title: Do the thing\n")
}

/// The `id:` constructor the reference's read does not catch, at every layer.
const ABORTING_ID: &str = "id: !!timestamp 2021-13-45\n";

fn aborting_id(path: &Path) {
    write(path, ABORTING_ID);
}

fn project_root() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temp dir");
    let project = directory.path().join("project");
    fs::create_dir_all(project.join(ArtifactKind::Tactic.project_dir())).expect("project dir");
    (directory, project)
}

fn identity(project: &Path) -> IdentityIndex {
    IdentityIndex::from_roots(&built_in_root(), &[], Some(project))
}

fn org_pack(root: &Path, charter: &str) -> PathBuf {
    let org = root.join("packs/acme-org");
    fs::create_dir_all(org.join("tactics")).expect("org tactics dir");
    fs::create_dir_all(org.join("directives")).expect("org directives dir");
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme-org\n        local_path: packs/acme-org\n",
    );
    write(
        &org.join("org-charter.yaml"),
        if charter.is_empty() { "{}\n" } else { charter },
    );
    org
}

fn ledger(root: &Path, text: &str) {
    write(&root.join(".kittify/charter/activation.yaml"), text);
}

fn service_for(root: &Path) -> CanonicalOfferingService {
    let context = OfferingContext::resolve(root).expect("offering context");
    CanonicalOfferingService::new(context)
}

fn scalar(ids: &[sk_kernel::PythonString]) -> Vec<String> {
    ids.iter()
        .map(|id| id.to_scalar_string().expect("scalar receipt"))
        .collect()
}

/// A failed identity construction must not poison the instance: repairing the
/// aborting file and asking again on the SAME index recovers the reference result.
#[test]
fn identity_same_instance_retries_after_the_abort_is_repaired() {
    let (_directory, project) = project_root();
    let tactics = project.join(ArtifactKind::Tactic.project_dir());
    write(&tactics.join("a.tactic.yaml"), &tactic("alpha"));
    aborting_id(&tactics.join("b.tactic.yaml"));
    let index = identity(&project);

    let abort = index
        .try_declared_id(ArtifactKind::Tactic, "b")
        .expect_err("the reference's scan aborts on the queried kind");
    let abort = abort.abort().expect("a Source constructor abort");
    assert_eq!(abort.python, "ValueError");
    assert!(
        abort.message.contains("month must be in 1..12"),
        "the real constructor cause is retained: {abort}"
    );

    write(&tactics.join("b.tactic.yaml"), &tactic("gamma"));
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "b")
            .expect("the same index retries the repaired kind"),
        Some("gamma".to_owned())
    );
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "a")
            .expect("the completed table answers every later lookup"),
        Some("alpha".to_owned())
    );
}

/// The reference's partial pass caches survive the abort: the layer file list and
/// each completed per-file read (including a caught `None`) are retained, so a
/// rewrite/removal/birth is NOT observed by the retried scan — a new index sees it.
#[test]
fn identity_retains_completed_reads_and_the_scan_boundary_across_the_abort() {
    let (_directory, project) = project_root();
    let tactics = project.join(ArtifactKind::Tactic.project_dir());
    write(&tactics.join("a.tactic.yaml"), &tactic("alpha"));
    write(&tactics.join("b.tactic.yaml"), &tactic("beta"));
    aborting_id(&tactics.join("c.tactic.yaml"));
    let index = identity(&project);
    let _ = index
        .try_declared_id(ArtifactKind::Tactic, "c")
        .expect_err("the first scan aborts");

    // Rewrite both completed files, repair the aborting one, birth a new file and
    // remove a completed one -- all before the same-instance retry.
    write(&tactics.join("a.tactic.yaml"), &tactic("alpha-two"));
    write(&tactics.join("b.tactic.yaml"), &tactic("beta-two"));
    write(&tactics.join("c.tactic.yaml"), &tactic("gamma"));
    write(&tactics.join("d.tactic.yaml"), &tactic("delta"));
    fs::remove_file(tactics.join("b.tactic.yaml")).expect("remove a completed file");

    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "c")
            .expect("the aborting file is retried"),
        Some("gamma".to_owned())
    );
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "a")
            .expect("completed read retained"),
        Some("alpha".to_owned()),
        "the reference keeps the per-file read it completed before the abort"
    );
    assert_eq!(
        index.try_declared_id(ArtifactKind::Tactic, "alpha-two"),
        Ok(None),
        "a rewrite of a completed file is not re-read by the same pass"
    );
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "b")
            .expect("the cached scan still names the removed file"),
        Some("beta".to_owned()),
        "the memoised layer scan is the pass boundary, not a fresh directory walk"
    );
    assert_eq!(
        index.try_declared_id(ArtifactKind::Tactic, "d"),
        Ok(None),
        "a file born after the failed scan is outside the memoised scan"
    );
    assert_eq!(
        index.try_declared_id(ArtifactKind::Tactic, "delta"),
        Ok(None)
    );

    // A new index is a new pass: it reads the tree as it is now.
    let fresh = identity(&project);
    assert_eq!(
        fresh
            .try_declared_id(ArtifactKind::Tactic, "a")
            .expect("fresh pass"),
        Some("alpha-two".to_owned())
    );
    assert_eq!(fresh.try_declared_id(ArtifactKind::Tactic, "b"), Ok(None));
    assert_eq!(
        fresh
            .try_declared_id(ArtifactKind::Tactic, "d")
            .expect("fresh pass sees the birth"),
        Some("delta".to_owned())
    );
}

/// A caught per-file failure is a *completed* read (the reference's `_parse_id`
/// returns `None`), so the same pass does not retry it — while the uncaught read
/// IS retried. A new index re-reads both.
#[test]
fn identity_caught_per_file_result_is_completed_and_not_retried() {
    let (_directory, project) = project_root();
    let tactics = project.join(ArtifactKind::Tactic.project_dir());
    write(&tactics.join("a-broken.tactic.yaml"), "id: [unclosed\n");
    aborting_id(&tactics.join("z-fail.tactic.yaml"));
    let index = identity(&project);
    let _ = index
        .try_declared_id(ArtifactKind::Tactic, "z-fail")
        .expect_err("the uncaught read aborts the scan");

    write(&tactics.join("a-broken.tactic.yaml"), &tactic("sigma"));
    write(&tactics.join("z-fail.tactic.yaml"), &tactic("gamma-z"));

    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "z-fail")
            .expect("the uncaught read is retried"),
        Some("gamma-z".to_owned())
    );
    assert_eq!(
        index.try_declared_id(ArtifactKind::Tactic, "a-broken"),
        Ok(None),
        "the caught per-file result stays the completed None"
    );
    assert_eq!(
        index.try_declared_id(ArtifactKind::Tactic, "sigma"),
        Ok(None),
        "the rewritten caught file is not re-read by the same pass"
    );

    let fresh = identity(&project);
    assert_eq!(
        fresh
            .try_declared_id(ArtifactKind::Tactic, "a-broken")
            .expect("fresh pass"),
        Some("sigma".to_owned())
    );
}

/// A completed table is published and stays warm; a new instance reads the change.
#[test]
fn identity_successful_table_stays_warm_and_a_new_index_re_reads() {
    let (_directory, project) = project_root();
    let tactics = project.join(ArtifactKind::Tactic.project_dir());
    write(&tactics.join("a.tactic.yaml"), &tactic("alpha"));
    let index = identity(&project);
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "a")
            .expect("first scan succeeds"),
        Some("alpha".to_owned())
    );
    write(&tactics.join("a.tactic.yaml"), &tactic("beta"));
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "a")
            .expect("the published table is the instance's answer"),
        Some("alpha".to_owned()),
        "a successful construction is never invalidated"
    );
    assert_eq!(
        identity(&project)
            .try_declared_id(ArtifactKind::Tactic, "a")
            .expect("new index"),
        Some("beta".to_owned())
    );
}

/// The real production consumer: a failed tactic repository construction must not
/// poison the service; repairing it and retrying the same property recovers, and
/// the rebuilt repository sees a file born between the failure and the retry.
#[test]
fn offering_service_same_instance_retries_after_repository_abort() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    let org = org_pack(root, "");
    write(&org.join("tactics/kale.tactic.yaml"), &tactic("kale"));
    aborting_id(&org.join("tactics/b-fail.tactic.yaml"));
    ledger(root, "activated_tactics: [kale, lemon, mango]\n");
    let service = service_for(root);

    let error = service
        .ids_for("activated_tactics")
        .expect_err("the tactic repository construction aborts");
    let abort = error
        .abort()
        .expect("the real constructor abort is retained");
    assert_eq!(abort.python, "ValueError");
    assert!(
        abort.message.contains("month must be in 1..12"),
        "the real cause survives the carrier: {abort}"
    );

    write(&org.join("tactics/b-fail.tactic.yaml"), &tactic("lemon"));
    write(&org.join("tactics/c-born.tactic.yaml"), &tactic("mango"));
    assert_eq!(
        scalar(
            &service
                .ids_for("activated_tactics")
                .expect("the same service retries the repaired repository")
        ),
        vec!["kale".to_owned(), "lemon".to_owned(), "mango".to_owned()],
        "a repository rebuilt after a failed construction is a fresh Source load"
    );
}

/// A repository that successfully *skipped* a caught-invalid file is published and
/// stays warm; repairing that file does not refresh the existing service, and a
/// new service sees it.
#[test]
fn offering_service_successful_skipped_repository_stays_warm() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    let org = org_pack(root, "");
    write(&org.join("tactics/olive.tactic.yaml"), &tactic("olive"));
    // `steps: []` is a Source-caught validation failure: skipped, not fatal.
    write(
        &org.join("tactics/papaya.tactic.yaml"),
        "id: papaya\nschema_version: \"1.0\"\nname: papaya\nsteps: []\n",
    );
    ledger(root, "activated_tactics: [olive, papaya]\n");
    let service = service_for(root);
    assert_eq!(
        scalar(
            &service
                .ids_for("activated_tactics")
                .expect("the skipped repository is a successful construction")
        ),
        vec!["olive".to_owned()]
    );

    write(&org.join("tactics/papaya.tactic.yaml"), &tactic("papaya"));
    assert_eq!(
        scalar(
            &service
                .ids_for("activated_tactics")
                .expect("the published repository is the instance's answer")
        ),
        vec!["olive".to_owned()],
        "repairing a skipped file does not refresh an existing service"
    );
    assert_eq!(
        scalar(
            &service_for(root)
                .ids_for("activated_tactics")
                .expect("a new service re-reads")
        ),
        vec!["olive".to_owned(), "papaya".to_owned()]
    );
}

/// The agent-profile repository is a distinct Source repository with its own
/// catches; its lazy property still publishes only a successful construction.
#[test]
fn offering_profile_repository_retries_after_abort() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    let org = org_pack(root, "");
    write(
        &org.join("agent_profiles/ivan.agent.yaml"),
        &profile_doc("ivan"),
    );
    write(
        &org.join("agent_profiles/z-fail.agent.yaml"),
        "profile-id: !!timestamp 2021-13-45\n",
    );
    ledger(root, "activated_agent_profiles: [ivan, zed]\n");
    let service = service_for(root);
    let error = service
        .ids_for("activated_agent_profiles")
        .expect_err("the profile repository construction aborts");
    // The profile repository's own loader renders the cause into the message; a
    // typed profile abort carrier is a separate (recorded) gap, not this lane.
    assert!(
        error.to_string().contains("month must be in 1..12"),
        "the profile read's real cause is retained: {error}"
    );

    write(
        &org.join("agent_profiles/z-fail.agent.yaml"),
        &profile_doc("zed"),
    );
    assert_eq!(
        scalar(
            &service
                .ids_for("activated_agent_profiles")
                .expect("the same service retries the repaired profile repository")
        ),
        vec!["ivan".to_owned(), "zed".to_owned()]
    );
}

/// The pack-skill repository owns a different `_load()` (tier post-validation and
/// `overrides`/`enhances` folding); its construction publishes on success only.
#[test]
fn offering_skill_repository_retries_after_abort() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    let org = org_pack(root, "");
    write(
        &org.join("skills/kale.skill.yaml"),
        "id: kale\nname: Kale\nprompt: Do the thing\n",
    );
    aborting_id(&org.join("skills/z-fail.skill.yaml"));
    ledger(root, "{}\n");
    let service = service_for(root);
    let error = match service.skill_repository() {
        Ok(_) => panic!("the pack-skill construction aborts"),
        Err(error) => error,
    };
    assert_eq!(
        error.abort().map(|abort| abort.python),
        Some("ValueError"),
        "the skill read's real cause: {error}"
    );

    write(
        &org.join("skills/z-fail.skill.yaml"),
        "id: lemon\nname: Lemon\nprompt: Do the thing\n",
    );
    // The control is the construction retry: before the fix the memoised abort made
    // every later call fail. The repaired document's own admission is the pack-skill
    // repository's existing rule (CF44), unchanged here.
    assert!(
        service.skill_repository().is_ok(),
        "the same service retries the repaired skill repository"
    );
}
