//! Acceptance tests for the typed artifact-kind taxonomy.
//!
//! Contract mirror: `src/charter/offering/artifact_kinds.py`. The Rust type is
//! the single kind enumeration; operator tokens are hyphenated, canonical YAML
//! values are snake_case, and `mission-type` is deliberately *not* an
//! `ArtifactKind`.

use proptest::prelude::*;
use sk_charter::{
    ArtifactKind, CHARTER_KIND_TOKENS, CharterError, CharterKind, MISSION_TYPE_TOKEN,
};

const EXPECTED_OPERATOR_TOKENS: [&str; 13] = [
    "directive",
    "tactic",
    "styleguide",
    "toolguide",
    "paradigm",
    "procedure",
    "agent-profile",
    "mission-step-contract",
    "template",
    "asset",
    "glossary-pack",
    "anti-pattern",
    "skill",
];

#[test]
fn all_members_expose_canonical_plural_glob_and_operator_forms() {
    assert_eq!(ArtifactKind::ALL.len(), 13);
    for (kind, expected_token) in ArtifactKind::ALL.iter().zip(EXPECTED_OPERATOR_TOKENS) {
        assert_eq!(kind.operator_token(), expected_token);
        assert_eq!(kind.canonical(), expected_token.replace('-', "_"));
        assert_eq!(kind.to_string(), expected_token.replace('-', "_"));
        assert_eq!(
            ArtifactKind::from_operator_token(kind.operator_token()).unwrap(),
            *kind
        );
        assert_eq!(
            ArtifactKind::from_canonical(kind.canonical()).unwrap(),
            *kind
        );
        assert_eq!(ArtifactKind::from_plural(kind.plural()).unwrap(), *kind);
        if !kind.glob_pattern().is_empty() {
            assert_eq!(
                ArtifactKind::from_glob_pattern(kind.glob_pattern()),
                Some(*kind),
                "glob patterns are unique per kind"
            );
        }
    }
}

#[test]
fn plurals_and_project_dirs_match_the_python_authority() {
    let expected: [(&str, &str, &str); 13] = [
        ("directive", "directives", "directive"),
        ("tactic", "tactics", "tactic"),
        ("styleguide", "styleguides", "styleguide"),
        ("toolguide", "toolguides", "toolguides"),
        ("paradigm", "paradigms", "paradigms"),
        ("procedure", "procedures", "procedure"),
        ("agent_profile", "agent_profiles", "agent_profiles"),
        (
            "mission_step_contract",
            "mission_step_contracts",
            "mission_step_contracts",
        ),
        ("template", "templates", "templates"),
        ("asset", "assets", "assets"),
        ("glossary_pack", "glossary_packs", "glossary_packs"),
        ("anti_pattern", "anti_patterns", "anti_patterns"),
        ("skill", "skills", "skills"),
    ];
    for (kind, (canonical, plural, project_dir)) in ArtifactKind::ALL.iter().zip(expected) {
        assert_eq!(kind.canonical(), canonical);
        assert_eq!(kind.plural(), plural);
        assert_eq!(kind.project_dir(), project_dir);
    }
}

#[test]
fn glob_patterns_are_the_canonical_yaml_suffixes() {
    let expected = [
        "*.directive.yaml",
        "*.tactic.yaml",
        "*.styleguide.yaml",
        "*.toolguide.yaml",
        "*.paradigm.yaml",
        "*.procedure.yaml",
        "*.agent.yaml",
        "*.step-contract.yaml",
        "",
        "*.asset.yaml",
        "*.glossary-pack.yaml",
        "*.anti_pattern.yaml",
        "*.skill.yaml",
    ];
    for (kind, pattern) in ArtifactKind::ALL.iter().zip(expected) {
        assert_eq!(kind.glob_pattern(), pattern);
    }
}

#[test]
fn activatable_membership_matches_the_enum_fact() {
    // TEMPLATE and ASSET resolve specially and are never live rules;
    // ANTI_PATTERN is activatable (issue #5409) even though it is not a
    // hand-authorable operator token.
    assert!(!ArtifactKind::Template.activatable());
    assert!(!ArtifactKind::Asset.activatable());
    assert!(ArtifactKind::AntiPattern.activatable());
    assert!(ArtifactKind::MissionStepContract.activatable());
    assert_eq!(
        ArtifactKind::ALL.iter().filter(|k| k.activatable()).count(),
        11
    );
}

#[test]
fn built_in_content_dir_carve_out_is_three_kinds() {
    let with_dir: Vec<&str> = ArtifactKind::ALL
        .iter()
        .filter(|kind| kind.has_built_in_content_dir())
        .map(|kind| kind.canonical())
        .collect();
    assert_eq!(with_dir.len(), 10);
    for kind in [
        ArtifactKind::MissionStepContract,
        ArtifactKind::Template,
        ArtifactKind::AntiPattern,
    ] {
        assert!(!kind.has_built_in_content_dir(), "{kind:?}");
    }
}

#[test]
fn operator_tokens_normalize_case_hyphens_and_whitespace() {
    for token in [
        "agent-profile",
        "AGENT-PROFILE",
        " Agent_Profile ",
        "agent_profile",
    ] {
        assert_eq!(
            ArtifactKind::from_operator_token(token).unwrap(),
            ArtifactKind::AgentProfile
        );
    }
    for token in ["glossary-pack", "Glossary_Pack"] {
        assert_eq!(
            ArtifactKind::from_operator_token(token).unwrap(),
            ArtifactKind::GlossaryPack
        );
    }
}

#[test]
fn mission_type_is_distinguished_from_artifact_kinds() {
    let error = ArtifactKind::from_operator_token(MISSION_TYPE_TOKEN).unwrap_err();
    assert!(matches!(error, CharterError::MissionTypeNotAnArtifactKind));

    // The canonical underscore spelling must not sneak through as a fallback.
    let error = ArtifactKind::from_operator_token("mission_type").unwrap_err();
    assert!(matches!(error, CharterError::MissionTypeNotAnArtifactKind));

    let error = ArtifactKind::from_canonical(MISSION_TYPE_TOKEN).unwrap_err();
    assert!(matches!(error, CharterError::MissionTypeNotAnArtifactKind));

    // The charter-kind universe routes it explicitly.
    assert_eq!(
        CharterKind::from_token(MISSION_TYPE_TOKEN).unwrap(),
        CharterKind::MissionType
    );
    assert_eq!(CharterKind::MissionType.operator_token(), "mission-type");
    assert_eq!(
        CharterKind::from_token("anti-pattern").unwrap(),
        CharterKind::Artifact(ArtifactKind::AntiPattern)
    );
}

#[test]
fn unknown_kind_errors_name_the_token_and_list_valid_operator_tokens() {
    let error = ArtifactKind::from_operator_token("directivez").unwrap_err();
    match error {
        CharterError::UnknownArtifactKind { token, valid } => {
            assert_eq!(token, "directivez");
            assert!(valid.contains("directive"));
            assert!(valid.contains("agent-profile"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn charter_kind_universe_excludes_non_hand_authorable_tokens() {
    assert_eq!(CHARTER_KIND_TOKENS.len(), 11);
    assert!(CHARTER_KIND_TOKENS.contains(&"directive"));
    assert!(CHARTER_KIND_TOKENS.contains(&"skill"));
    assert!(CHARTER_KIND_TOKENS.contains(&"mission-type"));
    assert!(!CHARTER_KIND_TOKENS.contains(&"template"));
    assert!(!CHARTER_KIND_TOKENS.contains(&"asset"));
    assert!(!CHARTER_KIND_TOKENS.contains(&"anti-pattern"));
}

#[test]
fn unknown_plural_and_canonical_forms_fail_closed() {
    assert!(matches!(
        ArtifactKind::from_plural("directivez").unwrap_err(),
        CharterError::UnknownPlural { .. }
    ));
    assert!(matches!(
        ArtifactKind::from_canonical("anti pattern").unwrap_err(),
        CharterError::UnknownArtifactKind { .. }
    ));
    assert_eq!(ArtifactKind::from_glob_pattern("*.unknown.yaml"), None);
    assert_eq!(ArtifactKind::from_glob_pattern(""), None);
}

proptest! {
    #[test]
    fn arbitrary_tokens_never_panic(token in ".{0,80}") {
        let _ = ArtifactKind::from_operator_token(&token);
        let _ = ArtifactKind::from_canonical(&token);
        let _ = ArtifactKind::from_plural(&token);
        let _ = CharterKind::from_token(&token);
    }
}
