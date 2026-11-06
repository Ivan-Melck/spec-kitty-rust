//! CF78 — Source-first controls for the canonical exact activation-ledger/deny
//! read core and its actual consumers.
//!
//! Phase 1 (the first two tests) calls ONLY the pre-existing public API
//! (`load_denied` / `LoadDeniedOptions`) against the Source truth table
//! established before the implementation, so a Source-valid refusal and a
//! Source-invalid success were red for the right reason. Phase 2 pins the exact
//! core, the checked scalar view and the actual consumer seam.
//!
//! Every fixture is a fresh temporary business tree; the built-in identity layer
//! resolves through the packaged `packs/built-in` root, as in production.

use std::fs;
use std::path::{Path, PathBuf};

use sk_doctrine::{
    ActivationReadError, ActivationSource, LoadDeniedOptions, denied_by_kind,
    load_activation_ledger, load_activation_ledger_exact, load_denied,
};
use sk_kernel::PythonString;

/// A lone high surrogate: admissible in the Source ledger grammar (an escaped
/// `\uD800` scalar) and preserved code point for code point.
fn cs() -> PythonString {
    PythonString::from_codepoints(&[0xD800]).unwrap()
}

/// A private-use code point: Source-admissible, scalar-projectable.
fn pua() -> PythonString {
    PythonString::from_codepoints(&[0xE000]).unwrap()
}

/// A noncharacter: Source-admissible, scalar-projectable.
fn noncharacter() -> PythonString {
    PythonString::from_codepoints(&[0xFFFE]).unwrap()
}

fn nul() -> PythonString {
    PythonString::from_codepoints(&[0]).unwrap()
}

fn text(value: &str) -> PythonString {
    PythonString::from_scalars(value)
}

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
    fs::create_dir_all(org.join("directives")).unwrap();
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

/// Spell *codepoints* as the Source-admissible double-quoted YAML escape a
/// ledger scalar needs for a code point Rust cannot push into a plain `str`.
fn escaped(codepoints: &[u32]) -> String {
    codepoints
        .iter()
        .map(|code| format!("\\u{code:04X}"))
        .collect()
}

/// The escaped spelling of the lone-surrogate token.
fn cs_escaped() -> String {
    escaped(cs().codepoints())
}

fn quoted(tokens: &[PythonString]) -> String {
    tokens
        .iter()
        .map(|token| format!("\"{}\"", escaped(token.codepoints())))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Guarded business-child environment (lane harness marker)
// ---------------------------------------------------------------------------

/// Assert this process's own isolated business environment *before* any read,
/// when the lane harness marks it as a guarded child (`CF78_GUARDED_CHILD=1`).
///
/// Without the marker (an ordinary `cargo test` run) the assertion is not made;
/// this is a harness-scoped guard, not a claim that every run is isolated.
#[test]
fn guarded_child_environment_is_independent() {
    if std::env::var("CF78_GUARDED_CHILD").as_deref() != Ok("1") {
        return;
    }
    let path_vars = [
        "HOME",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
        "TMPDIR",
        "TMP",
        "TEMP",
    ];
    for name in path_vars {
        let value = std::env::var(name).unwrap_or_else(|_| panic!("missing {name}"));
        let path = Path::new(&value);
        assert!(path.is_absolute(), "{name} is not absolute: {value}");
    }
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(
        cwd,
        Path::new(&std::env::var("TMPDIR").unwrap()),
        "the child CWD is its own temporary directory"
    );
    let pythonpath = std::env::var("PYTHONPATH").unwrap();
    assert!(
        Path::new(&pythonpath).is_absolute(),
        "PYTHONPATH is absolute"
    );
    let packs = std::env::var("SPEC_KITTY_PACKS_ROOT").unwrap();
    assert!(
        Path::new(&packs).join("built-in/directives").is_dir(),
        "the child's packs root carries the built-in directives"
    );
    assert!(
        std::env::var("PYTHONHASHSEED").is_ok(),
        "the child pins its hash seed"
    );
}

// ---------------------------------------------------------------------------
// Phase 1: pre-existing public API vs the Source truth table
// ---------------------------------------------------------------------------

#[test]
fn source_admits_an_escaped_surrogate_deny_token_but_the_reader_refused_it() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        &format!("deactivated_directives: [\"{}\"]\n", cs_escaped()),
    );
    // Source `load_denied` resolves this ledger to one directives deny set whose
    // stem is the lone surrogate and whose URN is `directive:<surrogate>`.
    let denied = load_denied(root, &LoadDeniedOptions::default()).unwrap();
    assert_eq!(denied.len(), 1, "Source resolves the surrogate deny set");
}

#[test]
fn source_raised_on_an_identity_constructor_abort_but_the_reader_answered_ok() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, Some("[]"));
    write(
        &org.join("directives/bad-timestamp.directive.yaml"),
        "id: !!timestamp 2021-13-45\n",
    );
    ledger(root, "deactivated_directives: [bad-timestamp]\n");
    // Source's identity scan raises `ValueError: month must be in 1..12` here;
    // a healthy resolution is the defective answer.
    assert!(
        load_denied(root, &LoadDeniedOptions::default()).is_err(),
        "the Source identity scan abort must escape the reader"
    );
}

// ---------------------------------------------------------------------------
// Phase 2: the exact core
// ---------------------------------------------------------------------------

#[test]
fn exact_deny_keeps_the_escaped_surrogate_token_and_urn() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        &format!("deactivated_directives: [\"{}\"]\n", cs_escaped()),
    );

    let exact = load_activation_ledger_exact(root).unwrap();
    assert_eq!(exact.source, ActivationSource::Ledger);
    assert_eq!(exact.deny_for(sk_charter::ArtifactKind::Directive), [cs()]);

    let denied = load_denied(root, &LoadDeniedOptions::default()).unwrap();
    assert_eq!(denied.len(), 1);
    let (plural, deny) = &denied[0];
    assert_eq!(plural, "directives");
    assert_eq!(deny.stems.iter().cloned().collect::<Vec<_>>(), [cs()]);
    assert!(deny.declared_ids.is_empty());
    let mut urn_codepoints: Vec<u32> = "directive:".chars().map(u32::from).collect();
    urn_codepoints.push(0xD800);
    let urn = PythonString::from_codepoints(&urn_codepoints).unwrap();
    assert_eq!(deny.urns.len(), 1);
    assert!(deny.urns.contains(&urn));
    // Real code-point membership, not an empty-token substitution.
    assert!(!deny.admits_exact(&cs()));
    assert!(!deny.admits_exact(&urn));
    assert!(deny.admits_exact(&text("010-specification-fidelity-requirement")));

    // The pre-existing scalar view still refuses this Source-valid ledger; the
    // exact scalar projection states *why* (a capability boundary, not Source
    // invalidity).
    assert!(load_activation_ledger(root).is_err());
    assert!(matches!(
        exact.to_scalar_view(),
        Err(ActivationReadError::ScalarProjection(_))
    ));
}

#[test]
fn exact_unknown_members_keys_and_nul_are_retained() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        &format!(
            "deactivated_directives: [{}]\ndeactivated_mystery: [\"{}\"]\n",
            quoted(&[cs(), pua(), noncharacter(), nul()]),
            cs_escaped()
        ),
    );

    let exact = load_activation_ledger_exact(root).unwrap();
    assert_eq!(
        exact.deny_for(sk_charter::ArtifactKind::Directive),
        [cs(), pua(), noncharacter(), nul()]
    );
    assert_eq!(
        exact.unknown_deny_keys,
        [PythonString::from_scalars("deactivated_mystery")]
    );
    assert_eq!(
        exact.unknown_deny_values,
        [(
            PythonString::from_scalars("deactivated_mystery"),
            Some(vec![cs()])
        )]
    );

    let denied = load_denied(root, &LoadDeniedOptions::default()).unwrap();
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0].1.stems.len(), 4);
    assert!(!denied[0].1.admits_exact(&cs()));
    assert!(!denied[0].1.admits_exact(&pua()));
    assert!(!denied[0].1.admits_exact(&noncharacter()));
    assert!(!denied[0].1.admits_exact(&nul()));
}

#[test]
fn exact_none_vs_empty_stays_three_state_and_malformed_ids_refuse() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        "deactivated_directives: null\ndeactivated_tactics: []\n",
    );
    let exact = load_activation_ledger_exact(root).unwrap();
    assert!(
        exact
            .deny_for(sk_charter::ArtifactKind::Directive)
            .is_empty()
    );
    assert!(exact.deny_for(sk_charter::ArtifactKind::Tactic).is_empty());
    assert!(
        load_denied(root, &LoadDeniedOptions::default())
            .unwrap()
            .is_empty()
    );

    for malformed in ["[1]", "[true]", "[null]"] {
        ledger(root, &format!("deactivated_directives: {malformed}\n"));
        match load_denied(root, &LoadDeniedOptions::default()) {
            Err(ActivationReadError::Doctrine(error)) => {
                assert!(
                    error.to_string().to_lowercase().contains("activation"),
                    "the malformed id refusal is the ledger's own typed error: {error}"
                );
            }
            other => panic!("Source refuses a non-string id list, got {other:?}"),
        }
    }
}

#[test]
fn exact_legacy_deny_key_is_reported_not_applied() {
    let directory = root();
    let root = directory.path();
    write(
        &root.join(".kittify/config.yaml"),
        "activated_directives: []\ndeactivated_directives: [900-unknown]\n",
    );
    let exact = load_activation_ledger_exact(root).unwrap();
    assert_eq!(exact.source, ActivationSource::Legacy);
    assert_eq!(
        exact.ignored_legacy_deny_keys,
        [PythonString::from_scalars("deactivated_directives")]
    );
    assert!(
        exact
            .deny_for(sk_charter::ArtifactKind::Directive)
            .is_empty()
    );
    assert!(
        load_denied(root, &LoadDeniedOptions::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn exact_floor_subtracts_a_surrogate_and_an_alias_in_identity_space() {
    let directory = root();
    let root = directory.path();
    // Exact surrogate floor of the same id.
    org_pack(root, Some(&format!("[\"{}\"]", cs_escaped())));
    ledger(
        root,
        &format!("deactivated_directives: [\"{}\"]\n", cs_escaped()),
    );
    assert!(
        load_denied(root, &LoadDeniedOptions::default())
            .unwrap()
            .is_empty(),
        "the required surrogate is never denied"
    );

    // Declared-id deny of an org-required stem.
    let org = org_pack(root, Some("[001-arch]"));
    write(
        &org.join("directives/001-arch.directive.yaml"),
        "id: ARCH_001\n",
    );
    ledger(root, "deactivated_directives: [ARCH_001]\n");
    assert!(
        load_denied(root, &LoadDeniedOptions::default())
            .unwrap()
            .is_empty(),
        "a denied required alias is inert"
    );
    // The same deny without the floor survives, exactly (the org artifact stays
    // indexed, so its declared id and stem are both spelled).
    write(&org.join("org-charter.yaml"), "required_directives: []\n");
    let denied = load_denied(root, &LoadDeniedOptions::default()).unwrap();
    assert_eq!(denied.len(), 1);
    assert_eq!(
        denied[0].1.declared_ids.iter().cloned().collect::<Vec<_>>(),
        [text("ARCH_001")]
    );
    assert_eq!(
        denied[0].1.stems.iter().cloned().collect::<Vec<_>>(),
        [text("001-arch")]
    );
}

#[test]
fn exact_identity_abort_is_a_typed_abort_at_the_source_phase() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, Some("[]"));
    write(
        &org.join("directives/bad-timestamp.directive.yaml"),
        "id: !!timestamp 2021-13-45\n",
    );
    ledger(root, "deactivated_directives: [bad-timestamp]\n");

    // The ledger itself is readable; the abort belongs to the identity phase.
    let exact = load_activation_ledger_exact(root).unwrap();
    assert_eq!(
        exact.deny_for(sk_charter::ArtifactKind::Directive),
        [text("bad-timestamp")]
    );

    match load_denied(root, &LoadDeniedOptions::default()) {
        Err(ActivationReadError::Aborted {
            phase,
            python,
            message,
            ..
        }) => {
            assert_eq!(python, "ValueError");
            assert!(
                message.contains("month must be in 1..12"),
                "the real cause is retained: {message}"
            );
            assert!(
                phase.contains("directives"),
                "phase names the kind: {phase}"
            );
        }
        other => panic!("Source aborts the identity scan, got {other:?}"),
    }
}

#[test]
fn scalar_projection_of_a_scalar_ledger_is_the_existing_read() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        "activated_directives: [010-specification-fidelity-requirement]\n\
         deactivated_directives: [900-unknown]\n\
         deactivated_mystery: [kept]\n\
         activated_tactics: null\n",
    );
    let exact = load_activation_ledger_exact(root).unwrap();
    let scalar = load_activation_ledger(root).unwrap();
    assert_eq!(
        exact.to_scalar_view().unwrap(),
        scalar,
        "the exact read and the existing scalar read agree on a scalar ledger"
    );
    assert_eq!(
        scalar.deny_for(sk_charter::ArtifactKind::Directive),
        ["900-unknown"]
    );
}

#[test]
fn reference_catalog_deny_is_the_one_exact_reader() {
    let directory = root();
    let root = directory.path();
    ledger(
        root,
        &format!("deactivated_directives: [\"{}\"]\n", cs_escaped()),
    );
    let denied = denied_by_kind(root, None).unwrap();
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0].0, sk_charter::ArtifactKind::Directive);
    assert_eq!(
        denied[0].1.stems.iter().cloned().collect::<Vec<_>>(),
        [cs()]
    );

    let org = org_pack(root, Some("[]"));
    write(
        &org.join("directives/bad-timestamp.directive.yaml"),
        "id: !!timestamp 2021-13-45\n",
    );
    match denied_by_kind(root, None) {
        Err(ActivationReadError::Aborted { python, .. }) => assert_eq!(python, "ValueError"),
        other => panic!("the consumer carries the reader's typed abort, got {other:?}"),
    }
}
