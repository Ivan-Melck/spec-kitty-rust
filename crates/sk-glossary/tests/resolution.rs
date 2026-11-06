use std::path::{Path, PathBuf};

use sk_glossary::{
    ConflictType, Glossary, GlossaryError, GlossaryScope, GlossaryStore, LookupOptions,
    LookupStatus, Severity, TermSense, TermStatus, TermSurface, resolve_term,
};

fn fixture_repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo")
}

fn fixture_glossary() -> Glossary {
    Glossary::load(fixture_repo()).expect("load fixture glossary")
}

fn sense(surface: &str, scope: GlossaryScope, definition: &str, status: TermStatus) -> TermSense {
    TermSense::new(
        TermSurface::parse(surface).expect("normalized surface"),
        scope,
        definition,
        1.0,
        status,
    )
    .expect("valid sense")
}

#[test]
fn resolves_matches_in_scope_precedence_order() {
    let glossary = fixture_glossary();
    let result = glossary
        .resolve("  WorkSpace \n")
        .expect("resolve workspace");

    assert_eq!(result.surface, "workspace");
    assert_eq!(result.status, LookupStatus::Ambiguous);
    let scopes: Vec<_> = result.matches.iter().map(|sense| sense.scope).collect();
    assert_eq!(
        scopes,
        [
            GlossaryScope::MissionLocal,
            GlossaryScope::TeamDomain,
            GlossaryScope::AudienceDomain,
        ]
    );
    assert_eq!(
        result.matches[0].definition,
        "Mission-local Git worktree created for the current work package"
    );
    assert_eq!(result.matches[2].status, TermStatus::Draft);
}

#[test]
fn reports_ambiguity_only_for_multiple_active_senses() {
    let glossary = fixture_glossary();

    let result = glossary.resolve("workspace").expect("resolve workspace");
    assert_eq!(result.status, LookupStatus::Ambiguous);
    let conflict = result.conflict.as_ref().expect("ambiguous conflict");
    assert_eq!(conflict.term, "workspace");
    assert_eq!(conflict.conflict_type, ConflictType::Ambiguous);
    assert_eq!(conflict.severity, Severity::Medium);
    assert!((conflict.confidence - 1.0).abs() < f64::EPSILON);
    assert_eq!(conflict.candidate_senses.len(), 3);
    assert_eq!(conflict.context, "");

    let mission = glossary.resolve("mission").expect("resolve mission");
    assert_eq!(mission.status, LookupStatus::Ambiguous);
    assert_eq!(mission.matches.len(), 3);
    assert_eq!(
        mission
            .conflict
            .as_ref()
            .expect("ambiguous mission")
            .candidate_senses
            .len(),
        3
    );

    let result = glossary.resolve("glossary").expect("resolve glossary");
    assert_eq!(result.status, LookupStatus::Resolved);
    assert_eq!(result.matches.len(), 1);
    assert!(result.conflict.is_none());
}

#[test]
fn draft_and_deprecated_matches_do_not_create_ambiguity() {
    let glossary = Glossary::from_senses([
        sense(
            "alpha",
            GlossaryScope::TeamDomain,
            "Active team meaning",
            TermStatus::Active,
        ),
        sense(
            "alpha",
            GlossaryScope::AudienceDomain,
            "Draft audience meaning",
            TermStatus::Draft,
        ),
        sense(
            "beta",
            GlossaryScope::SpecKittyCore,
            "Active core meaning",
            TermStatus::Active,
        ),
        sense(
            "beta",
            GlossaryScope::TeamDomain,
            "Deprecated team meaning",
            TermStatus::Deprecated,
        ),
    ]);

    let alpha = glossary.resolve("alpha").expect("resolve alpha");
    assert_eq!(alpha.status, LookupStatus::Resolved);
    assert_eq!(alpha.matches.len(), 2);
    assert!(alpha.conflict.is_none());

    let beta = glossary.resolve("beta").expect("resolve beta");
    assert_eq!(beta.status, LookupStatus::Resolved);
    assert_eq!(beta.matches.len(), 2);
    assert!(beta.conflict.is_none());
}

#[test]
fn reports_no_active_sense_when_only_draft_or_deprecated_matches_exist() {
    let glossary = fixture_glossary();
    let result = glossary
        .resolve("feature")
        .expect("resolve deprecated feature");

    assert_eq!(result.status, LookupStatus::NoActiveSense);
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].status, TermStatus::Deprecated);
    assert!(result.conflict.is_none());

    let glossary = Glossary::from_senses([
        sense(
            "alpha",
            GlossaryScope::TeamDomain,
            "Draft meaning",
            TermStatus::Draft,
        ),
        sense(
            "alpha",
            GlossaryScope::AudienceDomain,
            "Deprecated meaning",
            TermStatus::Deprecated,
        ),
    ]);
    let result = glossary.resolve("alpha").expect("resolve inactive alpha");
    assert_eq!(result.status, LookupStatus::NoActiveSense);
    assert_eq!(result.matches.len(), 2);
    assert!(result.conflict.is_none());
}

#[test]
fn unknown_terms_get_an_unknown_conflict_and_deterministic_severity() {
    let glossary = fixture_glossary();

    let result = glossary.resolve("not-a-term").expect("resolve unknown");
    assert_eq!(result.status, LookupStatus::Unknown);
    assert!(result.matches.is_empty());
    let conflict = result.conflict.as_ref().expect("unknown conflict");
    assert_eq!(conflict.conflict_type, ConflictType::Unknown);
    assert_eq!(conflict.severity, Severity::Low);
    assert!(conflict.candidate_senses.is_empty());

    let options = LookupOptions {
        confidence: 0.3,
        critical_step: true,
        ..LookupOptions::default()
    };
    let result = glossary
        .resolve_with("not-a-term", &options)
        .expect("resolve critical unknown");
    let conflict = result.conflict.as_ref().expect("critical conflict");
    assert_eq!(conflict.conflict_type, ConflictType::UnresolvedCritical);
    assert_eq!(conflict.severity, Severity::High);
}

#[test]
fn custom_and_limited_scope_orders_are_honored() {
    let glossary = fixture_glossary();

    let options = LookupOptions {
        scopes: vec![GlossaryScope::AudienceDomain, GlossaryScope::TeamDomain],
        ..LookupOptions::default()
    };
    let result = glossary
        .resolve_with("mission", &options)
        .expect("resolve custom order");
    let scopes: Vec<_> = result.matches.iter().map(|sense| sense.scope).collect();
    assert_eq!(
        scopes,
        [GlossaryScope::AudienceDomain, GlossaryScope::TeamDomain,]
    );
    let conflict = result.conflict.as_ref().expect("custom-order ambiguity");
    let candidate_scopes: Vec<_> = conflict
        .candidate_senses
        .iter()
        .map(|sense| sense.scope)
        .collect();
    assert_eq!(candidate_scopes, scopes);

    let options = LookupOptions {
        scopes: vec![GlossaryScope::TeamDomain],
        ..LookupOptions::default()
    };
    let result = glossary
        .resolve_with("workspace", &options)
        .expect("resolve limited scope");
    assert_eq!(result.status, LookupStatus::Resolved);
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].scope, GlossaryScope::TeamDomain);

    let options = LookupOptions {
        scopes: Vec::new(),
        ..LookupOptions::default()
    };
    let result = glossary
        .resolve_with("workspace", &options)
        .expect("resolve no scopes");
    assert_eq!(result.status, LookupStatus::Unknown);
    assert!(result.matches.is_empty());
}

#[test]
fn duplicate_active_senses_in_one_scope_are_ambiguous() {
    let glossary = Glossary::from_senses([
        sense(
            "alpha",
            GlossaryScope::TeamDomain,
            "First team meaning",
            TermStatus::Active,
        ),
        sense(
            "alpha",
            GlossaryScope::TeamDomain,
            "Second team meaning",
            TermStatus::Active,
        ),
    ]);

    let result = glossary.resolve("alpha").expect("resolve duplicate alpha");
    assert_eq!(result.status, LookupStatus::Ambiguous);
    assert_eq!(result.matches.len(), 2);
    assert_eq!(
        result.matches[0].definition, "First team meaning",
        "insertion order is the deterministic tie-break inside a scope"
    );
    assert_eq!(result.matches[1].definition, "Second team meaning");
}

#[test]
fn repeated_lookups_are_byte_for_byte_deterministic() {
    let glossary = fixture_glossary();
    let first = serde_json::to_string(&glossary.resolve("workspace").expect("first lookup"))
        .expect("serialize first lookup");

    for _ in 0..16 {
        let next = serde_json::to_string(&glossary.resolve("workspace").expect("next lookup"))
            .expect("serialize next lookup");
        assert_eq!(next, first);
    }
}

#[test]
fn lookup_result_serializes_with_stable_snake_case_fields() {
    let glossary = fixture_glossary();
    let result = glossary.resolve("glossary").expect("resolve glossary");
    let value = serde_json::to_value(&result).expect("serialize lookup result");

    assert_eq!(value["surface"], "glossary");
    assert_eq!(value["status"], "resolved");
    assert_eq!(value["conflict"], serde_json::Value::Null);
    assert_eq!(value["matches"][0]["surface"], "glossary");
    assert_eq!(value["matches"][0]["scope"], "spec_kitty_core");
    assert_eq!(value["matches"][0]["status"], "active");
    assert_eq!(
        value["matches"][0]["definition"],
        "A scoped collection of canonical term surfaces and their senses"
    );

    let ambiguous = glossary.resolve("workspace").expect("resolve workspace");
    let value = serde_json::to_value(&ambiguous).expect("serialize ambiguous lookup");
    assert_eq!(value["status"], "ambiguous");
    assert_eq!(value["conflict"]["conflict_type"], "ambiguous");
    assert_eq!(value["conflict"]["severity"], "medium");
    assert_eq!(
        value["conflict"]["candidate_senses"][0]["scope"],
        "mission_local"
    );
}

#[test]
fn lookup_accepts_a_prevalidated_surface() {
    let glossary = fixture_glossary();
    let surface = TermSurface::parse("lane").expect("normalized surface");
    let result = glossary.lookup(&surface);

    assert_eq!(result.status, LookupStatus::Resolved);
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].scope, GlossaryScope::MissionLocal);
    assert_eq!(result.matches[0].status, TermStatus::Active);
}

#[test]
fn convenience_scope_apis_match_the_explicit_options_api() {
    let glossary: GlossaryStore = fixture_glossary();
    let scopes = [GlossaryScope::AudienceDomain, GlossaryScope::TeamDomain];

    let options = LookupOptions {
        scopes: scopes.to_vec(),
        ..LookupOptions::default()
    };
    let via_options = glossary
        .resolve_with("Mission", &options)
        .expect("resolve with options");
    let via_scopes = glossary
        .resolve_in("Mission", &scopes)
        .expect("resolve in scopes");
    let via_function = resolve_term("Mission", &scopes, &glossary).expect("resolve term");

    assert_eq!(via_options, via_scopes);
    assert_eq!(via_scopes, via_function);
    assert_eq!(via_scopes.status, LookupStatus::Ambiguous);
}

#[test]
fn invalid_queries_and_options_are_typed_errors() {
    let glossary = fixture_glossary();
    assert!(matches!(
        glossary.resolve("   "),
        Err(GlossaryError::EmptySurface)
    ));

    let options = LookupOptions {
        confidence: 1.01,
        ..LookupOptions::default()
    };
    assert!(matches!(
        glossary.resolve_with("glossary", &options),
        Err(GlossaryError::InvalidConfidence { .. })
    ));
}

#[test]
fn all_terms_sorted_is_scope_precedence_then_surface() {
    let glossary = fixture_glossary();
    let terms = glossary.all_terms_sorted();
    let observed: Vec<_> = terms
        .iter()
        .map(|sense| (sense.scope(), sense.surface().as_str()))
        .collect();

    assert_eq!(
        observed,
        [
            (GlossaryScope::MissionLocal, "lane"),
            (GlossaryScope::MissionLocal, "workspace"),
            (GlossaryScope::TeamDomain, "ceremony commit"),
            (GlossaryScope::TeamDomain, "characterization test"),
            (GlossaryScope::TeamDomain, "mission"),
            (GlossaryScope::TeamDomain, "workspace"),
            (GlossaryScope::AudienceDomain, "mission"),
            (GlossaryScope::AudienceDomain, "workspace"),
            (GlossaryScope::SpecKittyCore, "feature"),
            (GlossaryScope::SpecKittyCore, "glossary"),
            (GlossaryScope::SpecKittyCore, "mission"),
        ]
    );
}

#[test]
fn load_scopes_only_loads_requested_scopes() {
    let glossary = Glossary::load_scopes(
        fixture_repo(),
        &[GlossaryScope::TeamDomain, GlossaryScope::SpecKittyCore],
    )
    .expect("load selected scopes");
    assert_eq!(glossary.len(), 7);
    assert!(glossary.iter().all(|sense| {
        matches!(
            sense.scope(),
            GlossaryScope::TeamDomain | GlossaryScope::SpecKittyCore
        )
    }));
}

#[test]
fn add_sense_uses_scope_precedence_without_a_global_sort() {
    let mut glossary = Glossary::new();
    glossary.add_sense(sense(
        "zeta",
        GlossaryScope::SpecKittyCore,
        "Core zeta",
        TermStatus::Active,
    ));
    glossary.add_sense(sense(
        "alpha",
        GlossaryScope::MissionLocal,
        "Local alpha",
        TermStatus::Active,
    ));

    let scopes: Vec<_> = glossary.iter().map(|sense| sense.scope()).collect();
    assert_eq!(
        scopes,
        [GlossaryScope::MissionLocal, GlossaryScope::SpecKittyCore]
    );
}
