use std::str::FromStr;

use sk_glossary::{
    GlossaryError, GlossaryScope, SCOPE_RESOLUTION_ORDER, TermSense, TermStatus, TermSurface,
};

#[test]
fn term_surface_requires_the_upstream_normalized_form() {
    let surface = TermSurface::parse("workspace").expect("normalized surface");
    assert_eq!(surface.as_str(), "workspace");
    assert_eq!(surface.to_string(), "workspace");

    for value in ["Workspace", " workspace", "workspace ", "myTerm"] {
        let error = TermSurface::parse(value).expect_err(value);
        assert!(
            matches!(error, GlossaryError::NonNormalizedSurface { .. }),
            "unexpected error for {value:?}: {error}"
        );
    }

    assert!(matches!(
        TermSurface::parse(""),
        Err(GlossaryError::EmptySurface)
    ));
}

#[test]
fn term_surface_normalizes_for_cli_input_without_accepting_empty_text() {
    assert_eq!(
        TermSurface::normalized("  WorkSpace \n")
            .expect("normalizable surface")
            .as_str(),
        "workspace"
    );
    assert_eq!(
        TermSurface::normalized("MISSION")
            .expect("normalizable surface")
            .as_str(),
        "mission"
    );
    assert!(matches!(
        TermSurface::normalized(" \t "),
        Err(GlossaryError::EmptySurface)
    ));
}

#[test]
fn term_surface_parses_and_serializes_as_a_transparent_string() {
    let parsed = TermSurface::from_str("workspace").expect("normalized surface");
    assert_eq!(parsed, TermSurface::parse("workspace").unwrap());

    let encoded = serde_json::to_string(&parsed).expect("serialize surface");
    assert_eq!(encoded, "\"workspace\"");
    assert_eq!(
        serde_json::from_str::<TermSurface>(&encoded).expect("deserialize surface"),
        parsed
    );
    assert!(serde_json::from_str::<TermSurface>("\"Workspace\"").is_err());
}

#[test]
fn scope_order_is_the_upstream_precedence_order() {
    assert_eq!(
        SCOPE_RESOLUTION_ORDER,
        [
            GlossaryScope::MissionLocal,
            GlossaryScope::TeamDomain,
            GlossaryScope::AudienceDomain,
            GlossaryScope::SpecKittyCore,
        ]
    );
    for (scope, value, filename) in [
        (
            GlossaryScope::MissionLocal,
            "mission_local",
            "mission_local.yaml",
        ),
        (GlossaryScope::TeamDomain, "team_domain", "team_domain.yaml"),
        (
            GlossaryScope::AudienceDomain,
            "audience_domain",
            "audience_domain.yaml",
        ),
        (
            GlossaryScope::SpecKittyCore,
            "spec_kitty_core",
            "spec_kitty_core.yaml",
        ),
    ] {
        assert_eq!(scope.as_str(), value);
        assert_eq!(scope.filename(), filename);
        assert_eq!(GlossaryScope::from_str(value).unwrap(), scope);
        assert_eq!(GlossaryScope::from_filename(filename), Some(scope));
        assert_eq!(
            GlossaryScope::from_path(
                &std::path::Path::new("/repo/.kittify/glossaries").join(filename)
            ),
            Some(scope)
        );
        assert_eq!(
            serde_json::to_string(&scope).unwrap(),
            format!("\"{value}\"")
        );
    }

    assert!(GlossaryScope::from_str("unknown").is_err());
    assert_eq!(GlossaryScope::from_filename("mission_local.yml"), None);
    assert_eq!(GlossaryScope::from_filename("other.yaml"), None);
}

#[test]
fn term_status_uses_the_upstream_vocabulary() {
    assert_eq!(TermStatus::default(), TermStatus::Draft);
    assert_eq!(TermStatus::from_str("active").unwrap(), TermStatus::Active);
    assert_eq!(TermStatus::from_str("draft").unwrap(), TermStatus::Draft);
    assert_eq!(
        TermStatus::from_str("deprecated").unwrap(),
        TermStatus::Deprecated
    );
    assert!(TermStatus::from_str("archived").is_err());
    assert_eq!(
        serde_json::to_string(&TermStatus::Active).unwrap(),
        "\"active\""
    );
}

#[test]
fn term_sense_validates_confidence_and_definition() {
    let surface = TermSurface::parse("workspace").unwrap();
    let sense = TermSense::new(
        surface.clone(),
        GlossaryScope::TeamDomain,
        "A Git worktree",
        0.9,
        TermStatus::Active,
    )
    .expect("valid sense");
    assert_eq!(sense.surface(), &surface);
    assert_eq!(sense.scope(), GlossaryScope::TeamDomain);
    assert_eq!(sense.definition(), "A Git worktree");
    assert!((sense.confidence() - 0.9).abs() < f64::EPSILON);
    assert_eq!(sense.status(), TermStatus::Active);

    for confidence in [-0.01, 1.01, f64::NAN] {
        let error = TermSense::new(
            surface.clone(),
            GlossaryScope::TeamDomain,
            "definition",
            confidence,
            TermStatus::Active,
        )
        .expect_err("invalid confidence");
        assert!(matches!(error, GlossaryError::InvalidConfidence { .. }));
    }

    for definition in ["", " ", "\n\t"] {
        let error = TermSense::new(
            surface.clone(),
            GlossaryScope::TeamDomain,
            definition,
            1.0,
            TermStatus::Active,
        )
        .expect_err("empty definition");
        assert!(matches!(error, GlossaryError::EmptyDefinition));
    }
}
