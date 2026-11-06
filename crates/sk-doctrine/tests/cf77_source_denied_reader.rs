//! CF77 — Source-first controls for the canonical effective-deny reader and the
//! actual `governance_charter_selection` consumer.
//!
//! The reader cases mirror the pinned Python reference's truth table
//! (`charter.activation.pack_context.load_denied` /
//! `activation_ledger.ledger_may_hold_deny`), which was established before the
//! Native implementation existed. The consumer cases call the real public
//! `governance_charter_selection`, not a private helper.
//!
//! Every fixture is a fresh temporary business tree; the built-in identity layer
//! resolves through the packaged `packs/built-in` root, as in production.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_doctrine::{
    ExactActivationLedger, ExactDenySet, GovernanceCharterSelection, IdentityIndex,
    LoadDeniedOptions, governance_charter_selection, ledger_may_hold_deny, load_denied,
};
use sk_kernel::PythonString;

const DIRECTIVE_STEM: &str = "010-specification-fidelity-requirement";
const DIRECTIVE_ID: &str = "DIRECTIVE_010";
const OTHER_ID: &str = "DIRECTIVE_018";
const TACTIC_STEM: &str = "anti-corruption-layer";

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn ledger(root: &Path, text: &str) {
    write(&root.join(".kittify/charter/activation.yaml"), text);
}

fn org_pack(root: &Path, required_directives: Option<&str>) -> PathBuf {
    let org = root.join("org-pack");
    fs::create_dir_all(&org).unwrap();
    write(
        &root.join(".kittify/config.yaml"),
        &format!(
            "charter_packs:\n  org:\n    packs:\n      - name: fixture-org\n        local_path: {}\n",
            org.display()
        ),
    );
    if let Some(required) = required_directives {
        write(
            &org.join("org-charter.yaml"),
            &format!("required_directives: {required}\n"),
        );
    }
    org
}

type SpellingRow = (String, Vec<String>, Vec<String>, Vec<String>);

fn spellings(denied: &[(String, ExactDenySet)]) -> Vec<SpellingRow> {
    let scalar = |items: &std::collections::BTreeSet<PythonString>| {
        items
            .iter()
            .map(|item| item.to_scalar_string().unwrap())
            .collect::<Vec<String>>()
    };
    denied
        .iter()
        .map(|(plural, deny)| {
            (
                plural.clone(),
                scalar(&deny.stems),
                scalar(&deny.declared_ids),
                scalar(&deny.urns),
            )
        })
        .collect()
}

fn text(value: &str) -> PythonString {
    PythonString::from_scalars(value)
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

#[test]
fn fresh_and_modern_empty_projects_read_no_deny() {
    let fresh = root();
    assert!(ledger_may_hold_deny(fresh.path()).unwrap());
    assert_eq!(
        load_denied(fresh.path(), &LoadDeniedOptions::default()).unwrap(),
        Vec::new()
    );

    let modern = root();
    ledger(modern.path(), "{}\n");
    assert!(ledger_may_hold_deny(modern.path()).unwrap());
    assert_eq!(
        load_denied(modern.path(), &LoadDeniedOptions::default()).unwrap(),
        Vec::new()
    );
}

#[test]
fn modern_deny_is_spelled_through_identity_and_sorted_by_plural() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        &format!(
            "deactivated_agent_profiles: [analyst-annie]\n\
             deactivated_anti_patterns: [some-antipattern]\n\
             deactivated_directives: [{DIRECTIVE_STEM}]\n"
        ),
    );
    assert!(ledger_may_hold_deny(root).unwrap());
    let denied = load_denied(root, &LoadDeniedOptions::default()).unwrap();
    assert_eq!(
        denied
            .iter()
            .map(|(plural, _)| plural.as_str())
            .collect::<Vec<_>>(),
        ["agent_profiles", "anti_patterns", "directives"]
    );
    assert_eq!(
        spellings(&denied)[2],
        (
            "directives".to_owned(),
            vec![DIRECTIVE_STEM.to_owned()],
            vec![DIRECTIVE_ID.to_owned()],
            vec![format!("directive:{DIRECTIVE_ID}")],
        )
    );
}

#[test]
fn legacy_allow_only_surface_is_answered_by_the_key_probe() {
    let directory = root();
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        &format!("activated_directives: [{DIRECTIVE_STEM}]\n"),
    );
    assert!(!ledger_may_hold_deny(root).unwrap());
    assert_eq!(
        load_denied(root, &LoadDeniedOptions::default()).unwrap(),
        Vec::new()
    );
}

#[test]
fn legacy_deny_key_on_the_pointed_charter_is_probed_but_never_applied() {
    let directory = root();
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "charter: charter.yaml\n",
    );
    write(
        &root.join("charter.yaml"),
        &format!("activated_directives: []\ndeactivated_directives: [{TACTIC_STEM}]\n"),
    );
    assert!(ledger_may_hold_deny(root).unwrap());
    assert_eq!(
        load_denied(root, &LoadDeniedOptions::default()).unwrap(),
        Vec::new()
    );
}

#[test]
fn malformed_legacy_surface_refuses_instead_of_answering_no_deny() {
    let directory = root();
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "activated_directives: [\n",
    );
    assert!(ledger_may_hold_deny(root).is_err());
    assert!(load_denied(root, &LoadDeniedOptions::default()).is_err());
}

#[test]
fn unknown_deny_key_is_probed_reported_and_not_applied() {
    let directory = root();
    let root = directory.path();
    ledger(root, "deactivated_mystery: [x]\n");
    assert!(ledger_may_hold_deny(root).unwrap());
    assert_eq!(
        load_denied(root, &LoadDeniedOptions::default()).unwrap(),
        Vec::new()
    );
}

#[test]
fn explicit_kinds_filter_is_distinct_from_every_kind() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        &format!(
            "deactivated_directives: [{DIRECTIVE_STEM}]\ndeactivated_tactics: [{TACTIC_STEM}]\n"
        ),
    );
    assert_eq!(
        load_denied(root, &LoadDeniedOptions::default())
            .unwrap()
            .iter()
            .map(|(plural, _)| plural.as_str())
            .collect::<Vec<_>>(),
        ["directives", "tactics"]
    );
    assert_eq!(
        load_denied(
            root,
            &LoadDeniedOptions {
                kinds: Some(&[]),
                ..LoadDeniedOptions::default()
            }
        )
        .unwrap(),
        Vec::new()
    );
    assert_eq!(
        load_denied(
            root,
            &LoadDeniedOptions {
                kinds: Some(&[ArtifactKind::Tactic]),
                ..LoadDeniedOptions::default()
            }
        )
        .unwrap()
        .iter()
        .map(|(plural, _)| plural.as_str())
        .collect::<Vec<_>>(),
        ["tactics"]
    );
}

#[test]
fn supplied_ledger_bypasses_the_source_read_and_is_used_as_given() {
    let directory = root();
    let root = directory.path();
    ledger(root, "{}");
    let supplied = ExactActivationLedger {
        deny: BTreeMap::from([(
            ArtifactKind::Directive,
            vec![PythonString::from_scalars(DIRECTIVE_STEM)],
        )]),
        ..ExactActivationLedger::default()
    };
    let denied = load_denied(
        root,
        &LoadDeniedOptions {
            ledger: Some(&supplied),
            ..LoadDeniedOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        spellings(&denied),
        [(
            "directives".to_owned(),
            vec![DIRECTIVE_STEM.to_owned()],
            vec![DIRECTIVE_ID.to_owned()],
            vec![format!("directive:{DIRECTIVE_ID}")],
        )]
    );
}

#[test]
fn required_floor_subtracts_a_denied_org_required_id_in_any_spelling() {
    let directory = root();
    let root = directory.path();
    org_pack(root, Some(&format!("[{DIRECTIVE_ID}]")));
    ledger(
        root,
        &format!("deactivated_directives: [{DIRECTIVE_STEM}]\n"),
    );
    assert_eq!(
        load_denied(root, &LoadDeniedOptions::default()).unwrap(),
        Vec::new()
    );

    ledger(root, &format!("deactivated_directives: [{DIRECTIVE_ID}]\n"));
    write(
        &root.join("org-pack/org-charter.yaml"),
        &format!("required_directives: [{DIRECTIVE_STEM}]\n"),
    );
    assert_eq!(
        load_denied(root, &LoadDeniedOptions::default()).unwrap(),
        Vec::new()
    );
}

#[test]
fn supplied_empty_org_roots_change_identity_spelling_only() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, None);
    write(
        &org.join("directives/900-org-only.directive.yaml"),
        "id: ORG_ONLY_900\nschema_version: '1.0'\ntitle: Org\n",
    );
    ledger(root, "deactivated_directives: [900-org-only]\n");

    assert_eq!(
        spellings(&load_denied(root, &LoadDeniedOptions::default()).unwrap()),
        [(
            "directives".to_owned(),
            vec!["900-org-only".to_owned()],
            vec!["ORG_ONLY_900".to_owned()],
            vec!["directive:ORG_ONLY_900".to_owned()],
        )]
    );
    // The supplied (empty) chain is indexed as given, so the org-only id has no
    // declared id; the stem and the canonical URN spelling stay exact.
    assert_eq!(
        spellings(
            &load_denied(
                root,
                &LoadDeniedOptions {
                    org_roots: Some(&[]),
                    ..LoadDeniedOptions::default()
                }
            )
            .unwrap()
        ),
        [(
            "directives".to_owned(),
            vec!["900-org-only".to_owned()],
            Vec::new(),
            vec!["directive:DIRECTIVE_900".to_owned()],
        )]
    );
}

#[test]
fn strict_floor_read_is_scoped_to_the_pending_kind() {
    let directory = root();
    let root = directory.path();
    org_pack(root, Some("[true]"));
    ledger(
        root,
        &format!("deactivated_directives: [{DIRECTIVE_STEM}]\n"),
    );
    assert!(
        load_denied(root, &LoadDeniedOptions::default()).is_err(),
        "a malformed required field of the denied kind refuses"
    );

    ledger(root, &format!("deactivated_tactics: [{TACTIC_STEM}]\n"));
    assert_eq!(
        load_denied(root, &LoadDeniedOptions::default())
            .unwrap()
            .iter()
            .map(|(plural, _)| plural.as_str())
            .collect::<Vec<_>>(),
        ["tactics"],
        "an unrelated malformed required field is never read"
    );
}

#[test]
fn exact_codepoints_are_kept_in_the_stem_and_only_the_urn_is_normalized() {
    let directory = root();
    let root = directory.path();
    let token = "unknown\u{200b}\u{e000}id\u{1f600}";
    ledger(root, &format!("deactivated_directives: [\"{token}\"]\n"));
    assert_eq!(
        spellings(&load_denied(root, &LoadDeniedOptions::default()).unwrap()),
        [(
            "directives".to_owned(),
            vec![token.to_owned()],
            Vec::new(),
            vec!["directive:UNKNOWN\u{200b}\u{e000}ID\u{1f600}".to_owned()],
        )]
    );
}

#[test]
fn existing_resolve_deny_spells_an_unresolved_directive_urn_like_source() {
    // Source: `_spell` mints `artifact_to_urn("directive", token)`, so an
    // unresolved directive token's URN is `directive:DIRECTIVE_900`, never the
    // raw `directive:900-org-only`.
    let identity = IdentityIndex::from_roots(Path::new("/nonexistent-built-in"), &[], None);
    let deny = sk_doctrine::resolve_deny(
        ArtifactKind::Directive,
        vec!["900-org-only".to_owned()],
        &identity,
    );
    assert_eq!(deny.stems, ["900-org-only".to_owned()].into());
    assert!(deny.declared_ids.is_empty());
    assert_eq!(deny.urns, ["directive:DIRECTIVE_900".to_owned()].into());
}

// ---------------------------------------------------------------------------
// Actual consumer: `governance_charter_selection`
// ---------------------------------------------------------------------------

fn charter_selection(root: &Path, selected: &str) -> GovernanceCharterSelection {
    write(
        &root.join(".kittify/charter/charter.yaml"),
        &format!("governance:\n  charter:\n    selected_directives: [{selected}]\n"),
    );
    governance_charter_selection(root, None).unwrap()
}

#[test]
fn consumer_subtracts_the_effective_deny_from_the_authored_selection() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        &format!("deactivated_directives: [{DIRECTIVE_STEM}]\n"),
    );
    let selection = charter_selection(root, &format!("{DIRECTIVE_ID}, {OTHER_ID}"));
    assert_eq!(selection.selected_directives, vec![text(OTHER_ID)]);
}

#[test]
fn consumer_keeps_a_denied_id_the_org_packs_require() {
    let directory = root();
    let root = directory.path();
    org_pack(root, Some(&format!("[{DIRECTIVE_ID}]")));
    ledger(
        root,
        &format!("deactivated_directives: [{DIRECTIVE_STEM}]\n"),
    );
    let selection = charter_selection(root, &format!("{DIRECTIVE_ID}, {OTHER_ID}"));
    assert_eq!(
        selection.selected_directives,
        vec![text(DIRECTIVE_ID), text(OTHER_ID)]
    );
}

#[test]
fn consumer_without_a_deny_never_reads_the_strict_floor() {
    let directory = root();
    let root = directory.path();
    org_pack(root, Some("[true]"));
    // Keep the org registry and add a legacy allow key: the project is on the
    // legacy surface with no `deactivated_<kind>` key, so the reader's key probe
    // answers "no deny" and the strict floor read never runs.
    let config = root.join(".kittify/config.yaml");
    let mut body = fs::read_to_string(&config).unwrap();
    body.push_str("activated_directives: []\n");
    fs::write(&config, body).unwrap();
    let selection = charter_selection(root, DIRECTIVE_ID);
    assert_eq!(
        selection.selected_directives,
        vec![text(DIRECTIVE_ID), text("True")],
        "the coercing selection overlay applies; the strict deny floor is not read"
    );
}

#[test]
fn consumer_refuses_when_a_denied_kinds_required_field_is_malformed() {
    let directory = root();
    let root = directory.path();
    org_pack(root, Some("[true]"));
    ledger(
        root,
        &format!("deactivated_directives: [{DIRECTIVE_STEM}]\n"),
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "governance: {}\n",
    );
    assert!(governance_charter_selection(root, None).is_err());
}
