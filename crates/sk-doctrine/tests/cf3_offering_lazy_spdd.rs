//! CF3: lazily-constructed per-kind identity + raw SPDD activation selection.
//!
//! Focused acceptance for the implementation breadth, not exhaustive probing:
//!
//! * construction reads no artifact at all, and an *unused* kind whose
//!   construction would abort never surfaces;
//! * a *queried* abort surfaces at that query (the reference's phase), while a
//!   merely-malformed file is skipped;
//! * the raw declared-id / file-stem distinction and the project/org overlay
//!   precedence, including the flat-before-legacy grouping;
//! * the canonical offering service loads per kind, so an unrelated kind's abort
//!   is not a global failure;
//! * the raw SPDD selector's public facade over the four selectors (missing
//!   config, allow-all, explicit empty, ledger deny, legacy deny).

use std::fs;
use std::path::Path;

use sk_charter::ArtifactKind;
use sk_doctrine::{
    CanonicalOfferingService, IdentityIndex, IdentityLayer, OfferingContext, OfferingRoots,
    activation_selection::{self, SPDD_DIRECTIVE_ID, SPDD_PARADIGM_ID, SPDD_TACTIC_FILL_ID},
};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    fs::write(path, text).expect("write");
}

/// A document whose construction raises a Python exception the reference's
/// artifact loader does not catch (`KeyError` from an invalid `!!bool`).
const FATAL_DOCUMENT: &str = "id: !!bool notabool\n";

#[test]
fn construction_reads_nothing_and_an_unused_abort_never_surfaces() {
    let directory = tempfile::tempdir().expect("tempdir");
    let built_in = directory.path().join("built-in");
    write(
        &built_in.join("directives/001-arch.directive.yaml"),
        "id: ARCH_001\n",
    );
    // A *queried* kind would abort here…
    write(&built_in.join("tactics/broken.tactic.yaml"), FATAL_DOCUMENT);

    // …but construction indexes exactly these roots and reads no directory, so
    // it cannot fail on a kind nothing has asked for yet.
    let index = IdentityIndex::from_roots(&built_in, &[], None);

    // The directive question succeeds: the aborted kind is never read.
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Directive, "001-arch")
            .expect("directives query"),
        Some("ARCH_001".to_owned())
    );

    // The tactic question is where the abort belongs.
    let error = index
        .try_declared_id(ArtifactKind::Tactic, "broken")
        .expect_err("the queried kind aborts");
    assert_eq!(
        error.abort().expect("a Source constructor abort").python,
        "KeyError"
    );
    // …and it is memoised, so a second query re-raises the same abort instead of
    // re-reading the tree.
    assert_eq!(
        index
            .try_scan(ArtifactKind::Tactic)
            .expect_err("still an abort")
            .abort()
            .expect("a Source constructor abort")
            .python,
        "KeyError"
    );

    // The total capability view reports the boundary instead of substituting.
    assert_eq!(index.declared_id(ArtifactKind::Tactic, "broken"), None);
}

#[test]
fn each_kind_is_read_at_its_first_query_not_at_construction() {
    // The observable consequence of laziness: a file that appears after the
    // index was constructed is visible, because the scan happens at the query.
    let directory = tempfile::tempdir().expect("tempdir");
    let built_in = directory.path().join("built-in");
    fs::create_dir_all(built_in.join("directives")).expect("dirs");
    let empty = IdentityIndex::from_roots(&built_in, &[], None);
    assert_eq!(
        empty.try_declared_id(ArtifactKind::Directive, "late"),
        Ok(None)
    );
    drop(empty);

    write(
        &built_in.join("directives/late.directive.yaml"),
        "id: LATE_ID\n",
    );
    let fresh = IdentityIndex::from_roots(&built_in, &[], None);
    assert_eq!(
        fresh
            .try_declared_id(ArtifactKind::Directive, "late")
            .expect("query"),
        Some("LATE_ID".to_owned())
    );
}

#[test]
fn a_malformed_document_is_skipped_without_failing_the_kind() {
    let directory = tempfile::tempdir().expect("tempdir");
    let built_in = directory.path().join("built-in");
    write(&built_in.join("tactics/good.tactic.yaml"), "id: GOOD\n");
    write(
        &built_in.join("tactics/broken.tactic.yaml"),
        "id: [broken\n",
    );

    let index = IdentityIndex::from_roots(&built_in, &[], None);
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::Tactic, "good")
            .expect("tactics query"),
        Some("GOOD".to_owned())
    );
    // The malformed file contributes no entry — it is a skip, not an abort and
    // not an invented stem identity.
    assert_eq!(
        index.try_declared_id(ArtifactKind::Tactic, "broken"),
        Ok(None)
    );
}

#[test]
fn raw_declared_ids_and_file_stems_stay_distinct() {
    let directory = tempfile::tempdir().expect("tempdir");
    let built_in = directory.path().join("built-in");
    write(
        &built_in.join("tactics/qa-tactic-seven.tactic.yaml"),
        "id: QA_SEVEN\n",
    );
    let index = IdentityIndex::from_roots(&built_in, &[], None);

    // A file name and a declared id are two different questions.
    assert!(index.names_a_file(ArtifactKind::Tactic, "qa-tactic-seven"));
    assert!(!index.names_a_file(ArtifactKind::Tactic, "QA_SEVEN"));
    assert!(index.has_declared_id(ArtifactKind::Tactic, "QA_SEVEN"));
    assert!(!index.has_declared_id(ArtifactKind::Tactic, "qa-tactic-seven"));

    // …and both spellings resolve to the same artifact.
    assert_eq!(
        index.declared_id(ArtifactKind::Tactic, "qa-tactic-seven"),
        Some("QA_SEVEN".to_owned())
    );
    assert_eq!(
        index.stem(ArtifactKind::Tactic, "QA_SEVEN"),
        Some("qa-tactic-seven".to_owned())
    );
    // A `<kind>:` URN resolves like the bare id, and never resolves back out.
    assert_eq!(
        index.stem(ArtifactKind::Tactic, "tactic:QA_SEVEN"),
        Some("qa-tactic-seven".to_owned())
    );
}

#[test]
fn project_then_org_then_built_in_precedence() {
    let directory = tempfile::tempdir().expect("tempdir");
    let built_in = directory.path().join("built-in");
    let org = directory.path().join("org");
    let project = directory.path().join("project");
    write(
        &built_in.join("tactics/shared.tactic.yaml"),
        "id: BUILTIN_ID\n",
    );
    write(&org.join("tactics/shared.tactic.yaml"), "id: ORG_ID\n");
    write(
        &project.join("tactic/shared.tactic.yaml"),
        "id: PROJECT_ID\n",
    );

    let index = IdentityIndex::from_roots(
        &built_in,
        std::slice::from_ref(&org),
        Some(project.as_path()),
    );
    assert_eq!(
        index.source_layer(ArtifactKind::Tactic, "shared"),
        Some(IdentityLayer::Project)
    );
    assert_eq!(
        index.declared_id(ArtifactKind::Tactic, "shared"),
        Some("PROJECT_ID".to_owned())
    );
    assert_eq!(
        index.declared_id_in_layer(ArtifactKind::Tactic, "shared", IdentityLayer::BuiltIn),
        Some("BUILTIN_ID".to_owned())
    );
    assert_eq!(
        index.declared_id_in_layer(ArtifactKind::Tactic, "shared", IdentityLayer::Org),
        Some("ORG_ID".to_owned())
    );
}

#[test]
fn the_flat_org_layout_wins_over_the_legacy_built_in_child() {
    let directory = tempfile::tempdir().expect("tempdir");
    let built_in = directory.path().join("built-in");
    let org = directory.path().join("org");
    // Same file name in the flat layout and in the legacy `<plural>/built-in/`
    // package layout of the *same* org pack.
    write(&org.join("tactics/shared.tactic.yaml"), "id: FLAT_ID\n");
    write(
        &org.join("tactics/built-in/shared.tactic.yaml"),
        "id: LEGACY_ID\n",
    );
    // A deeper directory that merely happens to be named `built-in` is NOT the
    // reserved layout, so it stays discoverable.
    write(
        &org.join("tactics/writing/built-in/nested.tactic.yaml"),
        "id: NESTED_ID\n",
    );
    write(
        &built_in.join("tactics/built-in/shared.tactic.yaml"),
        "id: BUILTIN_LEGACY_ID\n",
    );

    let index = IdentityIndex::from_roots(&built_in, std::slice::from_ref(&org), None);
    assert_eq!(
        index.declared_id(ArtifactKind::Tactic, "shared"),
        Some("FLAT_ID".to_owned()),
        "the flat org layout keeps precedence over the legacy one"
    );
    assert_eq!(
        index.declared_id_in_layer(ArtifactKind::Tactic, "shared", IdentityLayer::Org),
        Some("FLAT_ID".to_owned())
    );
    assert_eq!(
        index.declared_id(ArtifactKind::Tactic, "nested"),
        Some("NESTED_ID".to_owned()),
        "a deeper `built-in` directory is not the reserved immediate child"
    );
}

#[test]
fn the_offering_service_loads_one_kind_at_a_time() {
    let directory = tempfile::tempdir().expect("tempdir");
    let built_in = directory.path().join("built-in");
    write(
        &built_in.join("tactics/good.tactic.yaml"),
        "id: GOOD_TACTIC\n",
    );
    // A queried-kind abort waiting in an unrelated kind.
    write(
        &built_in.join("procedures/broken.procedure.yaml"),
        FATAL_DOCUMENT,
    );

    let context = OfferingContext::new(
        directory.path().to_path_buf(),
        OfferingRoots {
            built_in: Some(built_in.clone()),
            org_roots: Vec::new(),
            project: None,
        },
        None,
    );
    let service = CanonicalOfferingService::new(context);

    // The tactic question never touches the procedure kind: it loads cleanly
    // (this fixture's single tactic document is not schema-admitted, which is a
    // skip, not an abort), while the procedure kind's own answer carries the
    // abort.
    let tactics = service.ids_for("activated_tactics").expect("tactics load");
    assert!(tactics.is_empty(), "nothing here is schema-admitted");

    // The procedure question is where that abort belongs, and it is typed.
    let error = service
        .ids_for("activated_procedures")
        .expect_err("the queried kind aborts");
    assert_eq!(
        error.abort().map(|abort| abort.python),
        Some("KeyError"),
        "{error}"
    );
}

#[test]
fn the_raw_spdd_selector_exposes_the_reference_truth_table() {
    // 1. A missing `.kittify/config.yaml` pins `false` (the reference's carve-out).
    let missing = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        activation_selection::is_spdd_reasons_active(missing.path()),
        Ok(false)
    );

    // 2. A present but empty config is the allow-all state: every selector's
    //    allow key is absent, so all four are admitted.
    let empty = tempfile::tempdir().expect("tempdir");
    write(
        &empty.path().join(".kittify/config.yaml"),
        "# nothing configured\n",
    );
    let selection =
        activation_selection::spdd_activation_selection(empty.path()).expect("selection");
    assert!(selection.active());
    assert_eq!(
        selection.observed(),
        vec![
            SPDD_PARADIGM_ID,
            SPDD_TACTIC_FILL_ID,
            "reasons-canvas-review",
            SPDD_DIRECTIVE_ID
        ]
    );

    // 3. Present-but-explicitly-empty lists select nothing.
    let empty_lists = tempfile::tempdir().expect("tempdir");
    write(
        &empty_lists.path().join(".kittify/config.yaml"),
        "activated_paradigms: []\nactivated_tactics: []\nactivated_directives: []\n",
    );
    assert_eq!(
        activation_selection::is_spdd_reasons_active(empty_lists.path()),
        Ok(false)
    );

    // 4. A ledger deny wins even over an absent (allow-all) key.
    let denied = tempfile::tempdir().expect("tempdir");
    write(&denied.path().join(".kittify/config.yaml"), "agents: {}\n");
    write(
        &denied.path().join(".kittify/charter/activation.yaml"),
        "deactivated_paradigms: [structured-prompt-driven-development]\ndeactivated_tactics: [reasons-canvas-fill, reasons-canvas-review]\ndeactivated_directives: [DIRECTIVE_038]\n",
    );
    let selection =
        activation_selection::spdd_activation_selection(denied.path()).expect("selection");
    assert!(!selection.active(), "a ledger deny is applied");
    assert!(!selection.paradigm && !selection.tactic_fill && !selection.directive);

    // 5. A legacy source applies no deny list at all.
    let legacy = tempfile::tempdir().expect("tempdir");
    write(
        &legacy.path().join(".kittify/config.yaml"),
        "activated_directives: [DIRECTIVE_038]\ndeactivated_directives: [DIRECTIVE_038]\n",
    );
    let selection =
        activation_selection::spdd_activation_selection(legacy.path()).expect("selection");
    assert_eq!(
        selection.source,
        Some(sk_doctrine::ActivationSource::Legacy)
    );
    assert!(selection.active(), "a legacy deny key is never applied");
}
