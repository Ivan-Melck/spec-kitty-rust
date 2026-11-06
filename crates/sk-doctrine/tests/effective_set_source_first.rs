//! Source-first conformance for the canonical effective-set seam.
//!
//! Every test runs the **actual pinned reference** first
//! (`charter.activation.effective_set.resolve_effective_sets`, CPython 3.11.15 /
//! exact53, fail-closed), asserts the cause it actually produced, and only then
//! runs this crate against the same fixture and asserts **equality** (ids,
//! fallback ids, resolved flag and reason, transported as exact code points).
//!
//! # History (preserved, not rewritten)
//!
//! E3's version of this file asserted the seam's missing service half: a
//! non-directive key failed closed with `SERVICE_BUILD_PREFIX`, the mission-type
//! fallback was empty, and a mixed request poisoned every pending key. Those
//! assertions were *honest gaps*, not parity. Their original bytes are preserved
//! at `/tmp/e4-preimages/crates_sk-doctrine_tests_effective_set_source_first.rs`
//! (sha256 `1bb772da…c70ab`) and the E3 report keeps the mismatch receipts. The
//! canonical offering service now exists, so those explicit expected-mismatch
//! assertions are replaced by Source equality — they are not relabelled parity.
//!
//! # Isolation
//!
//! Source and Native each run as a real re-executed child with asserted CWD,
//! HOME, every XDG/TMP root, `PYTHONPATH` and pack root (see
//! `support/isolated.rs`). Fixtures are fresh, unique and exclusive; none is
//! cleaned up or reused.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::path::Path;

use isolated::{assert_same, entry, has_fallback, has_id, run_pair};

/// One real built-in file of `kind`, read from the repository's pinned packs.
fn built_in(relative: &str) -> String {
    let path = isolated::repository_root()
        .join("packs/built-in")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// Write `body` at `relative` inside `fixture`, creating parents.
fn write(fixture: &Path, relative: &str, body: &str) {
    let path = fixture.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture directory");
    std::fs::write(&path, body).expect("fixture file");
}

/// Declare one local org pack rooted at `packs/acme-org`.
fn declare_org_pack(fixture: &Path) {
    write(
        fixture,
        ".kittify/config.yaml",
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme-org\n",
    );
}

/// A real built-in tactic with its declared id swapped (schema stays valid).
fn org_tactic(declared_id: &str) -> String {
    let template = built_in("tactics/moscow-scoping-lens.tactic.yaml");
    let swapped = template.replace("id: moscow-scoping-lens", &format!("id: {declared_id}"));
    assert_ne!(swapped, template, "the fixture swaps the declared id");
    swapped
}

/// A real built-in step contract with its declared id swapped.
fn step_contract(declared_id: &str) -> String {
    let template =
        built_in("missions/built_in_step_contracts/documentation-audit.step-contract.yaml");
    let swapped = template.replace("id: documentation-audit", &format!("id: {declared_id}"));
    assert_ne!(swapped, template, "the fixture swaps the declared id");
    swapped
}

/// The full activation-key universe: every real kind, the required-only kind and
/// the ledger outlier.
const ALL_KEYS: [&str; 11] = [
    "activated_directives",
    "activated_tactics",
    "activated_styleguides",
    "activated_toolguides",
    "activated_paradigms",
    "activated_procedures",
    "activated_agent_profiles",
    "activated_mission_step_contracts",
    "activated_glossary_packs",
    "activated_skills",
    "mission_type_activations",
];

/// Every real kind resolves, the required-only kind does not, and the ledger's
/// fallback is the real flat roster — identical on both sides.
#[test]
fn bare_project_resolves_every_supported_kind() {
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &ALL_KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("bare-all-kinds");
    let (source, native) = run_pair(
        &fixture,
        &ALL_KEYS,
        "bare_project_resolves_every_supported_kind",
    );
    assert!(
        source.error.is_none(),
        "reference raised: {:?}",
        source.error
    );

    const RESOLVED: [&str; 9] = [
        "activated_directives",
        "activated_tactics",
        "activated_styleguides",
        "activated_toolguides",
        "activated_paradigms",
        "activated_procedures",
        "activated_agent_profiles",
        "activated_mission_step_contracts",
        "activated_glossary_packs",
    ];
    for key in RESOLVED {
        let reference = entry(&source, key);
        assert!(
            reference.resolved,
            "reference resolves {key}: {reference:?}"
        );
        assert!(!reference.ids.is_empty(), "reference lists {key} ids");
        assert!(
            reference.fallback.is_empty(),
            "a resolved entry carries no fallback"
        );
        assert!(reference.reason.is_none());
    }
    let skill = entry(&source, "activated_skills");
    assert!(!skill.resolved, "skill is required-only");
    assert_eq!(
        skill.reason.as_deref(),
        Some(sk_doctrine::REQUIRED_ONLY_REASON)
    );
    assert!(
        skill.fallback.is_empty(),
        "required-only carries no fallback"
    );
    let ledger = entry(&source, "mission_type_activations");
    assert!(!ledger.resolved);
    assert_eq!(ledger.reason.as_deref(), Some(sk_doctrine::LEDGER_REASON));
    assert!(
        !ledger.fallback.is_empty(),
        "the reference scans the shipped mission types into the fallback"
    );

    assert!(native.error.is_none(), "native raised: {:?}", native.error);
    for key in ALL_KEYS {
        assert_same(entry(&source, key), entry(&native, key), key);
    }
}

/// Duplicates resolve once, in request order, on both sides.
#[test]
fn duplicate_keys_resolve_once_in_request_order() {
    const KEYS: [&str; 3] = [
        "activated_directives",
        "activated_directives",
        "activated_tactics",
    ];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("dedup-order");
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "duplicate_keys_resolve_once_in_request_order",
    );
    assert!(
        source.error.is_none(),
        "reference raised: {:?}",
        source.error
    );
    let order: Vec<&str> = source.sets.iter().map(|set| set.key.as_str()).collect();
    assert_eq!(order, vec!["activated_directives", "activated_tactics"]);
    assert!(
        source.sets.iter().all(|set| set.resolved),
        "the reference resolves both keys: {:?}",
        source.sets
    );
    assert_eq!(native.sets.len(), source.sets.len());
    for (expected, actual) in source.sets.iter().zip(native.sets.iter()) {
        assert_same(expected, actual, "request order");
    }
}

/// An unknown key refuses with the reference's exact text and reads nothing.
#[test]
fn unknown_key_refuses_with_the_reference_text() {
    const KEYS: [&str; 1] = ["activated_ntended"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("unknown-key");
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "unknown_key_refuses_with_the_reference_text",
    );
    let error = source
        .error
        .expect("the reference raises for an unknown key");
    assert!(
        error.starts_with("ValueError: Unknown activation key 'activated_ntended'. Valid keys: ["),
        "reference error: {error}"
    );
    assert!(
        error.contains("'mission_type_activations']"),
        "reference lists every valid key: {error}"
    );
    assert_eq!(
        native.error.as_deref(),
        error.strip_prefix("ValueError: "),
        "native refusal text is the reference's"
    );
    assert!(
        !fixture.join(".kittify").exists(),
        "no repository read happened"
    );
}

/// `skill` is required-only: never resolved, and no fallback is offered.
#[test]
fn skill_is_never_resolved() {
    const KEYS: [&str; 1] = ["activated_skills"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("skill-required-only");
    let (source, native) = run_pair(&fixture, &KEYS, "skill_is_never_resolved");
    assert!(source.error.is_none());
    let reference = entry(&source, "activated_skills");
    assert!(!reference.resolved);
    assert_eq!(
        reference.reason.as_deref(),
        Some(sk_doctrine::REQUIRED_ONLY_REASON)
    );
    assert!(reference.ids.is_empty() && reference.fallback.is_empty());
    assert_same(reference, entry(&native, "activated_skills"), "skill");
}

/// The ledger key's fallback is the real flat mission-type roster (built-in),
/// not an empty placeholder.
#[test]
fn mission_type_ledger_fallback_is_the_real_flat_roster() {
    const KEYS: [&str; 1] = ["mission_type_activations"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("mission-type-roster");
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "mission_type_ledger_fallback_is_the_real_flat_roster",
    );
    let reference = entry(&source, "mission_type_activations");
    assert!(!reference.resolved);
    assert!(
        reference.fallback.len() > 1,
        "the shipped roster has several mission types: {reference:?}"
    );
    assert!(has_fallback(reference, "software-dev"), "{reference:?}");
    let actual = entry(&native, "mission_type_activations");
    assert!(has_fallback(actual, "software-dev"), "{actual:?}");
    assert_same(reference, actual, "mission-type fallback");
}

/// An org pack contributes language-scoped and unscoped artifacts per declared
/// root; the scoped one is excluded from the service half but its file stem is
/// still available, exactly as in the reference.
#[test]
fn declared_org_pack_layers_follow_the_reference() {
    const KEYS: [&str; 2] = ["activated_tactics", "activated_styleguides"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("org-layers");
    declare_org_pack(&fixture);
    write(
        &fixture,
        "packs/acme-org/tactics/900-acme-scoping.tactic.yaml",
        &org_tactic("acme-scoping-lens"),
    );
    // A real scoped styleguide: python-scoped, so a `languages: [rust]` project
    // excludes it from the service while the scan half still lists its stem.
    let styleguide = built_in("styleguides/python-conventions.styleguide.yaml")
        .replace("id: python-conventions", "id: acme-python-conventions");
    write(
        &fixture,
        "packs/acme-org/styleguides/900-acme-python.styleguide.yaml",
        &styleguide,
    );
    write(
        &fixture,
        ".kittify/config.yaml",
        "languages:\n  - rust\ncharter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme-org\n",
    );

    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "declared_org_pack_layers_follow_the_reference",
    );
    assert!(
        source.error.is_none(),
        "reference raised: {:?}",
        source.error
    );
    let tactics = entry(&source, "activated_tactics");
    assert!(tactics.resolved);
    assert!(has_id(tactics, "900-acme-scoping"), "{tactics:?}");
    let styleguides = entry(&source, "activated_styleguides");
    assert!(styleguides.resolved);
    assert!(
        has_id(styleguides, "900-acme-python"),
        "the scan half keeps the language-excluded file's stem: {styleguides:?}"
    );
    assert_same(tactics, entry(&native, "activated_tactics"), "org tactics");
    assert_same(
        styleguides,
        entry(&native, "activated_styleguides"),
        "org styleguides",
    );
}

/// Mission step contracts are the service-only layer: a declared org/project
/// contract is not in the reference's availability scan, but its loaded id is in
/// the service mapping, so both sides list it.
#[test]
fn service_only_step_contract_overlays_are_listed() {
    const KEYS: [&str; 1] = ["activated_mission_step_contracts"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("step-contract-overlays");
    declare_org_pack(&fixture);
    write(
        &fixture,
        "packs/acme-org/mission_step_contracts/900-acme-audit.step-contract.yaml",
        &step_contract("acme-documentation-audit"),
    );
    write(
        &fixture,
        ".kittify/charter-packs/mission_step_contracts/901-project-audit.step-contract.yaml",
        &step_contract("project-documentation-audit"),
    );
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "service_only_step_contract_overlays_are_listed",
    );
    assert!(
        source.error.is_none(),
        "reference raised: {:?}",
        source.error
    );
    let reference = entry(&source, "activated_mission_step_contracts");
    assert!(reference.resolved);
    assert!(has_id(reference, "900-acme-audit"), "{reference:?}");
    assert!(has_id(reference, "901-project-audit"), "{reference:?}");
    assert_same(
        reference,
        entry(&native, "activated_mission_step_contracts"),
        "step contracts",
    );
}

/// The declared-name construction domain: a present-but-odd `id` (`null`, a
/// list, whitespace) is `str(value).strip()`; an absent or empty one is skipped.
#[test]
fn declared_name_construction_domain_is_python_str_strip() {
    const KEYS: [&str; 1] = ["activated_directives"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("id-domain");
    write(
        &fixture,
        ".kittify/charter-packs/directive/901-null.directive.yaml",
        "id: null\n",
    );
    write(
        &fixture,
        ".kittify/charter-packs/directive/902-list.directive.yaml",
        "id: [alpha, beta]\n",
    );
    write(
        &fixture,
        ".kittify/charter-packs/directive/903-empty.directive.yaml",
        "id: \"\"\n",
    );
    write(
        &fixture,
        ".kittify/charter-packs/directive/904-absent.directive.yaml",
        "title: x\n",
    );
    write(
        &fixture,
        ".kittify/charter-packs/directive/905-spaced.directive.yaml",
        "id: \"  ACME_POLICY  \"\n",
    );
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "declared_name_construction_domain_is_python_str_strip",
    );
    assert!(
        source.error.is_none(),
        "reference raised: {:?}",
        source.error
    );
    let reference = entry(&source, "activated_directives");
    assert!(reference.resolved);
    for listed in ["901-null", "902-list", "905-spaced"] {
        assert!(
            has_id(reference, listed),
            "{listed} declares a name: {reference:?}"
        );
    }
    for skipped in ["903-empty", "904-absent"] {
        assert!(
            !has_id(reference, skipped),
            "{skipped} declares no name: {reference:?}"
        );
    }
    assert_same(
        reference,
        entry(&native, "activated_directives"),
        "declared-name domain",
    );
}

/// Two files declaring the same id keep both stems (the reference emits each
/// file's config stem, not the declared id).
#[test]
fn same_declared_id_two_stems_keeps_both_stems() {
    const KEYS: [&str; 1] = ["activated_tactics"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("same-id-two-stems");
    declare_org_pack(&fixture);
    write(
        &fixture,
        "packs/acme-org/tactics/901-dup-a.tactic.yaml",
        &org_tactic("acme-duplicate-lens"),
    );
    write(
        &fixture,
        "packs/acme-org/tactics/902-dup-b.tactic.yaml",
        &org_tactic("acme-duplicate-lens"),
    );
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "same_declared_id_two_stems_keeps_both_stems",
    );
    assert!(
        source.error.is_none(),
        "reference raised: {:?}",
        source.error
    );
    let reference = entry(&source, "activated_tactics");
    assert!(has_id(reference, "901-dup-a"), "{reference:?}");
    assert!(has_id(reference, "902-dup-b"), "{reference:?}");
    assert_same(
        reference,
        entry(&native, "activated_tactics"),
        "same-id stems",
    );
}

/// A broken org registry poisons every pending key in the pass (the service is
/// built once), and the readable project layer still answers both fallbacks.
#[test]
fn broken_org_registry_poisons_pending_keys_in_one_pass() {
    const KEYS: [&str; 2] = ["activated_directives", "activated_tactics"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("broken-registry");
    write(
        &fixture,
        ".kittify/config.yaml",
        "charter_packs: [not, a, mapping\n",
    );
    write(
        &fixture,
        ".kittify/charter-packs/directive/900-local.directive.yaml",
        "id: LOCAL_POLICY\n",
    );
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "broken_org_registry_poisons_pending_keys_in_one_pass",
    );
    assert!(
        source.error.is_none(),
        "reference raised: {:?}",
        source.error
    );
    for key in KEYS {
        let reference = entry(&source, key);
        assert!(!reference.resolved, "{key} fails closed");
        let reason = reference.reason.as_deref().expect("a refusal reason");
        assert!(
            reason.starts_with("the org pack registry cannot be read: "),
            "{key}: {reason}"
        );
        assert!(
            !reference.fallback.is_empty(),
            "{key}: the readable built-in layer still answers"
        );
        if key == "activated_directives" {
            assert!(
                has_fallback(reference, "900-local"),
                "{key}: the authored project directive is kept"
            );
        }
        let actual = entry(&native, key);
        assert_eq!(actual.resolved, reference.resolved, "{key}: resolved");
        assert_eq!(actual.ids, reference.ids, "{key}: ids");
        assert_eq!(actual.fallback, reference.fallback, "{key}: fallback ids");
        // The shared prefix is the reference's own refusal sentence; the detail
        // after it is each YAML reader's parse diagnostic, which is a disclosed
        // boundary (the same reason class, not a byte-equal parser message).
        assert!(
            actual
                .reason
                .as_deref()
                .is_some_and(|native_reason| native_reason
                    .starts_with("the org pack registry cannot be read: ")),
            "{key}: {:?}",
            actual.reason
        );
    }
}

/// A declared-but-unfetched org pack renders the reference's fetch remedy, and
/// a broken mission-type layer drops only its own map from the roster fallback.
#[test]
fn missing_org_pack_and_malformed_mission_type_layer_degrade_like_the_reference() {
    const KEYS: [&str; 3] = [
        "activated_directives",
        "activated_tactics",
        "mission_type_activations",
    ];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("missing-org-and-bad-roster");
    declare_org_pack(&fixture);
    // The declared org root exists, so the chain resolves; the mission-type file
    // inside it is schema-invalid, so that root map contributes nothing.
    write(
        &fixture,
        "packs/acme-org/mission_types/broken.yaml",
        "id: broken\ndisplay_name: Broken\nunknown_key: 1\n",
    );
    write(
        &fixture,
        "packs/acme-org/tactics/900-acme-scoping.tactic.yaml",
        &org_tactic("acme-scoping-lens"),
    );
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "missing_org_pack_and_malformed_mission_type_layer_degrade_like_the_reference",
    );
    assert!(
        source.error.is_none(),
        "reference raised: {:?}",
        source.error
    );
    let ledger = entry(&source, "mission_type_activations");
    assert!(!ledger.resolved);
    assert!(
        has_fallback(ledger, "software-dev"),
        "the base map still answers the roster: {ledger:?}"
    );
    assert!(
        !has_fallback(ledger, "broken"),
        "the malformed layer contributes nothing: {ledger:?}"
    );
    for key in KEYS {
        assert_same(entry(&source, key), entry(&native, key), key);
    }
}

/// An org-declared mission type adds a roster id; the flat per-root scan sees it.
#[test]
fn org_mission_type_joins_the_roster_fallback() {
    const KEYS: [&str; 1] = ["mission_type_activations"];
    if let Some(root) = isolated::native_root() {
        isolated::assert_isolated(&root);
        println!(
            "NATIVE_RECEIPT {}",
            isolated::native_receipt_json(&root, &KEYS)
        );
        return;
    }
    let fixture = isolated::fixture("org-mission-type");
    declare_org_pack(&fixture);
    write(
        &fixture,
        "packs/acme-org/mission_types/acme-flow.yaml",
        "schema_version: 1\nid: acme-flow\ndisplay_name: Acme Flow\n",
    );
    let (source, native) = run_pair(
        &fixture,
        &KEYS,
        "org_mission_type_joins_the_roster_fallback",
    );
    let reference = entry(&source, "mission_type_activations");
    assert!(has_fallback(reference, "acme-flow"), "{reference:?}");
    assert_same(
        reference,
        entry(&native, "mission_type_activations"),
        "org roster",
    );
}
