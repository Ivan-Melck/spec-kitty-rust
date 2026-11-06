//! CF26 focused smoke: the NEW `charter context --include` non-section selectors.
//!
//! These exercise the real canonical authorities on real fixtures: the
//! `sk-charter` kind vocabulary, the crate's lazy `IdentityIndex` (which resolves an
//! authored declared id that differs from the file stem), the constructed-YAML document
//! read, the canonical model dump, the profile repository and the pure body/payload
//! projections in `context_render`.
//!
//! They are **not** a deep differential campaign. Deliberately deferred proof: the
//! reference's six-tier `template:` resolver, the canonical model dumps for
//! `glossary_pack`/`paradigm`/`skill`/`mission_step_contract` (this crate has none yet),
//! the activation deny gate on a selector (`_deactivated_note`), overlay field-merge for
//! a partially re-declared id, exact-Python byte equality of every rendered line, the
//! `artifact:` probe over a large ambiguous pack, and every Unicode/`!!`-tag shape of the
//! lossless transport inside an artifact body.

use std::fs;
use std::path::Path;

use sk_doctrine::OfferingRoots;
use sk_doctrine::context_include::{
    IncludeAccess, IncludeInputs, parse_include_selector, render_include,
};

const DIRECTIVE: &str = "schema_version: \"1.0\"\n\
id: DIRECTIVE_900\n\
title: Fixture Directive\n\
intent: >\n\
\x20 Keep the fixture intent verbatim.\n\
enforcement: required\n\
scope: Fixture scope line.\n\
procedures:\n\
- First fixture procedure.\n\
- Second fixture procedure.\n\
integrity_rules:\n\
- Fixture integrity rule.\n\
validation_criteria:\n\
- Fixture validation criterion.\n";

const TACTIC: &str = "schema_version: \"1.0\"\n\
id: fixture-tactic\n\
name: Fixture Tactic\n\
purpose: Fixture tactic purpose.\n\
steps:\n\
- title: First step title\n\
\x20 description: First step detail.\n\
- title: Second step title\n";

const STYLEGUIDE: &str = "schema_version: \"1.0\"\n\
id: fixture-styleguide\n\
title: Fixture Styleguide\n\
scope: code\n\
principles:\n\
- Fixture principle one.\n\
- Fixture principle two.\n";

const PROCEDURE: &str = "schema_version: \"1.0\"\n\
id: fixture-procedure\n\
name: Fixture Procedure\n\
purpose: Fixture procedure purpose.\n\
entry_condition: Fixture entry condition.\n\
exit_condition: Fixture exit condition.\n\
steps:\n\
- title: Procedure step title\n\
\x20 description: Procedure step detail.\n";

const PROFILE: &str = "profile-id: fixture-agent\n\
name: Fixture Agent\n\
description: Fixture agent profile.\n\
schema-version: \"1.0\"\n\
roles:\n\
- implementer\n\
purpose: Fixture agent purpose.\n\
specialization:\n\
\x20 primary-focus: Fixture focus\n";

const PARADIGM: &str = "schema_version: \"1.0\"\n\
id: fixture-paradigm\n\
name: Fixture Paradigm\n\
summary: Fixture paradigm summary.\n";

// CF32: the fixtures below are the Source models' *admitted* shapes (the reference's
// repository `.get()` only returns a validated model, so a document it would skip is a
// miss, not a render). The assertions that use them are unchanged.
const GLOSSARY: &str = "id: fixture-glossary\n\
provenance: built-in\n\
description: Fixture glossary description.\n\
terms:\n\
- surface: fixture-term\n\
\x20 definition: Fixture term definition.\n\
\x20 confidence: 1.0\n\
\x20 status: active\n";

const SHARED_DIRECTIVE: &str = "schema_version: \"1.0\"\n\
id: DIRECTIVE_901\n\
title: Shared Directive\n\
intent: Shared directive intent.\n\
enforcement: required\n";

const SHARED_STYLEGUIDE: &str = "schema_version: \"1.0\"\n\
id: shared\n\
title: Shared Styleguide\n\
scope: code\n\
principles:\n\
- Shared principle.\n";

const SHARED_PROCEDURE: &str = "schema_version: \"1.0\"\n\
id: shared\n\
name: Shared Procedure\n\
purpose: Shared procedure purpose.\n\
entry_condition: Shared entry condition.\n\
exit_condition: Shared exit condition.\n\
steps:\n\
- title: Shared step\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// One built-in pack root plus one project pack root.
struct Fixture {
    root: tempfile::TempDir,
    // CF32: the include now reads the canonical lazy access authority instead of a bare
    // identity index + profile repository; the assertions below are unchanged.
    access: IncludeAccess,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path();
        let packs = root.join("packs");
        // The authored id deliberately differs from the file stem: the selector resolves
        // through the identity index, never by guessing `<id>.directive.yaml`.
        write(
            &packs.join("directives/900-fixture-file.directive.yaml"),
            DIRECTIVE,
        );
        write(&packs.join("tactics/fixture-tactic.tactic.yaml"), TACTIC);
        write(
            &packs.join("styleguides/fixture-styleguide.styleguide.yaml"),
            STYLEGUIDE,
        );
        write(
            &packs.join("procedures/fixture-procedure.procedure.yaml"),
            PROCEDURE,
        );
        write(
            &packs.join("agent_profiles/fixture-agent.agent.yaml"),
            PROFILE,
        );
        write(
            &packs.join("paradigms/fixture-paradigm.paradigm.yaml"),
            PARADIGM,
        );
        write(
            &packs.join("glossary_packs/fixture-glossary.glossary-pack.yaml"),
            GLOSSARY,
        );
        // The project overlay is a real project pack root (Source's `_project_dir` uses
        // the per-kind project directory only for `.kittify/charter-packs`).
        let project = root.join("project/.kittify/charter-packs");
        // The project overlay uses the reference's project-tier directory spellings
        // (`project_dir`): singular for directive/tactic/styleguide/procedure.
        write(
            &project.join("directive/shared-directive.directive.yaml"),
            SHARED_DIRECTIVE,
        );
        write(
            &project.join("styleguide/shared-styleguide.styleguide.yaml"),
            SHARED_STYLEGUIDE,
        );
        write(
            &project.join("procedure/shared-procedure.procedure.yaml"),
            SHARED_PROCEDURE,
        );
        let access = IncludeAccess::with_roots(
            root,
            OfferingRoots {
                built_in: Some(packs),
                org_roots: Vec::new(),
                project: Some(project),
            },
        )
        .expect("fixture access");
        Self { root: temp, access }
    }

    fn render(&self, selector: &str) -> Result<String, sk_doctrine::context_include::IncludeError> {
        let parsed = parse_include_selector(selector).expect("selector parses");
        let inputs = IncludeInputs {
            project_root: self.root.path(),
            access: &self.access,
        };
        render_include(&inputs, &parsed, selector)
    }
}

#[test]
fn directive_selector_resolves_the_authored_identity_not_the_file_stem() {
    let fixture = Fixture::new();
    let by_stem = fixture
        .render("directive:900-fixture-file")
        .expect("directive");
    // Heading, inline body and the canonical model dump, in the reference's order.
    assert!(
        by_stem.starts_with("Directive DIRECTIVE_900: Fixture Directive\n"),
        "{by_stem}"
    );
    assert!(by_stem.contains("    Intent: Keep the fixture intent verbatim."));
    assert!(by_stem.contains("    Scope: Fixture scope line."));
    assert!(by_stem.contains("    Procedures:\n      - First fixture procedure."));
    assert!(by_stem.contains("    Integrity rules:\n      - Fixture integrity rule."));
    assert!(by_stem.contains("    Validation criteria:\n      - Fixture validation criterion."));
    assert!(by_stem.contains("    Full artifact:\n      {\n"));
    assert!(by_stem.contains("\"id\": \"DIRECTIVE_900\""));
    assert!(by_stem.contains("\"title\": \"Fixture Directive\""));

    // Declared id, numeric form and file stem all name one artifact.
    for selector in [
        "directive:DIRECTIVE_900",
        "directive:directive_900",
        "directive:900-fixture-file",
    ] {
        assert_eq!(
            fixture.render(selector).expect(selector),
            by_stem,
            "{selector}"
        );
    }
    // No raw YAML is printed as the payload.
    assert!(!by_stem.contains("schema_version:"));
    assert!(!by_stem.contains("enforcement: required"));
}

#[test]
fn tactic_selector_renders_steps_with_their_descriptions() {
    let text = Fixture::new()
        .render("tactic:fixture-tactic")
        .expect("tactic");
    assert!(
        text.starts_with("Tactic fixture-tactic: Fixture Tactic\n"),
        "{text}"
    );
    assert!(text.contains("    Name: Fixture Tactic"));
    assert!(text.contains("    Purpose: Fixture tactic purpose."));
    assert!(text.contains("    Steps:\n      - First step title\n        First step detail."));
    assert!(text.contains("      - Second step title\n"));
    assert!(text.contains("\"id\": \"fixture-tactic\""));
}

#[test]
fn catalog_kinds_render_their_own_inline_bodies() {
    let styleguide = Fixture::new()
        .render("styleguide:fixture-styleguide")
        .expect("styleguide");
    assert!(
        styleguide.starts_with("Styleguide fixture-styleguide: Fixture Styleguide\n"),
        "{styleguide}"
    );
    assert!(styleguide.contains("    Scope: code"));
    assert!(styleguide.contains("    Principles:\n      - Fixture principle one."));

    let procedure = Fixture::new()
        .render("procedure:fixture-procedure")
        .expect("procedure");
    assert!(
        procedure.starts_with("Procedure fixture-procedure: Fixture Procedure\n"),
        "{procedure}"
    );
    assert!(procedure.contains("    Entry condition: Fixture entry condition."));
    assert!(procedure.contains("    Exit condition: Fixture exit condition."));
    assert!(procedure.contains("      - Procedure step title\n        Procedure step detail."));
}

#[test]
fn agent_profile_selector_renders_the_repository_record_and_its_full_dump() {
    let text = Fixture::new()
        .render("agent-profile:fixture-agent")
        .expect("agent profile");
    assert!(
        text.starts_with("Agent profile fixture-agent: Fixture Agent\n"),
        "{text}"
    );
    assert!(text.contains("    Name: Fixture Agent"));
    assert!(text.contains("    Purpose: Fixture agent purpose."));
    assert!(text.contains("    Roles: implementer"));
    assert!(text.contains("    Full artifact:"));
    assert!(text.contains("\"profile-id\": \"fixture-agent\""));
    // The hyphenated alias form of the model dump, not the authored snake case.
    assert!(text.contains("\"schema-version\": \"1.0\""));
}

#[test]
fn missing_members_refuse_with_the_kind_specific_reference_message() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .render("directive:no-such-directive")
            .unwrap_err()
            .to_string(),
        "No directive found for selector 'directive:no-such-directive'."
    );
    assert_eq!(
        fixture
            .render("tactic:no-such-tactic")
            .unwrap_err()
            .to_string(),
        "No tactic found for selector 'tactic:no-such-tactic'."
    );
    assert_eq!(
        fixture
            .render("agent-profile:no-such-profile")
            .unwrap_err()
            .to_string(),
        "No agent_profile found for selector 'agent_profile:no-such-profile'."
    );
    assert_eq!(
        fixture
            .render("styleguide:no-such-style")
            .unwrap_err()
            .to_string(),
        "No styleguide found for selector 'styleguide:no-such-style'."
    );
}

#[test]
fn selector_grammar_and_kind_vocabulary_refuse_exactly_like_the_reference() {
    let fixture = Fixture::new();
    assert_eq!(
        parse_include_selector("directive").unwrap_err().to_string(),
        "Expected --include selector in '<kind>:<id>' form."
    );
    assert_eq!(
        parse_include_selector("directive:   ")
            .unwrap_err()
            .to_string(),
        "Expected --include selector in '<kind>:<id>' form."
    );
    // The kind is lower-cased and trimmed, so a padded upper-case token still resolves.
    let parsed = parse_include_selector("  Directive : fixture ").expect("parsed");
    assert_eq!(parsed.kind, "directive");
    assert_eq!(parsed.identifier, "fixture");
    assert_eq!(
        fixture.render("nonsense:thing").unwrap_err().to_string(),
        "Unsupported --include selector kind 'nonsense'."
    );
    // `asset` is a canonical kind but not addressable; the reference reports the token.
    assert_eq!(
        fixture.render("asset:thing").unwrap_err().to_string(),
        "Unsupported --include selector kind 'asset'."
    );
    // `anti_pattern` is recognised and resolves to the standard not-found.
    assert_eq!(
        fixture
            .render("anti-pattern:thing")
            .unwrap_err()
            .to_string(),
        "No anti_pattern found for selector 'anti_pattern:thing'."
    );
    assert_eq!(
        fixture
            .render("mission-type:research")
            .unwrap_err()
            .to_string(),
        "--include does not support the 'mission-type' selector (selector 'mission-type:research'); \
         mission types are not addressable governance artifacts."
    );
}

#[test]
fn artifact_probe_resolves_one_match_and_refuses_an_ambiguous_one() {
    let fixture = Fixture::new();
    let text = fixture
        .render("artifact:fixture-tactic")
        .expect("single match");
    assert!(
        text.starts_with("Tactic fixture-tactic: Fixture Tactic\n"),
        "{text}"
    );
    assert_eq!(
        fixture.render("artifact:shared").unwrap_err().to_string(),
        "Ambiguous artifact selector 'artifact:shared' matched kinds: styleguide, procedure."
    );
    // A directive identifier is normalized by the reference's directive id normaliser
    // before the probe looks it up (numeric ids only, by design).
    let directive = fixture
        .render("artifact:901")
        .expect("normalized directive match");
    assert!(
        directive.starts_with("Directive DIRECTIVE_901: Shared Directive\n"),
        "{directive}"
    );
    assert_eq!(
        fixture.render("artifact:nowhere").unwrap_err().to_string(),
        "No artifact found for selector 'artifact:nowhere'."
    );
}

#[test]
fn a_pending_projection_does_not_hide_a_real_cross_kind_ambiguity() {
    let fixture = Fixture::new();
    // This is a Source-valid Paradigm (all four required fields). The same
    // token already names a tactic; missing Native projection is not absence.
    write(
        &fixture
            .root
            .path()
            .join("packs/paradigms/overlap.paradigm.yaml"),
        &PARADIGM.replace("fixture-paradigm", "fixture-tactic"),
    );
    let error = fixture
        .render("artifact:fixture-tactic")
        .expect_err("two present kinds");
    assert_eq!(
        error.to_string(),
        "Ambiguous artifact selector 'artifact:fixture-tactic' matched kinds: tactic, paradigm."
    );
    assert!(
        !error.is_pending(),
        "ambiguity is a Source refusal, not a projection gap"
    );
}

#[test]
fn kinds_with_a_canonical_model_dump_render_it_instead_of_raw_yaml() {
    // CF31 landed the `glossary_pack`/`paradigm` model dumps in the canonical body
    // authority, so these two kinds no longer take the `CONTEXT_INCLUDE_PENDING` gap:
    // the merged record is admitted by the repository and dumped by the one model-dump
    // authority. The assertion is the Source-faithful one this file always intended —
    // a canonical dump, never raw authored YAML.
    let fixture = Fixture::new();
    for (selector, heading) in [
        (
            "glossary-pack:fixture-glossary",
            "Glossary pack fixture-glossary: fixture-glossary\n",
        ),
        (
            "paradigm:fixture-paradigm",
            "Paradigm fixture-paradigm: Fixture Paradigm\n",
        ),
    ] {
        let text = fixture.render(selector).expect(selector);
        assert!(text.starts_with(heading), "{selector}: {text}");
        assert!(text.contains("    Full artifact:\n"), "{selector}: {text}");
        assert!(!text.contains("schema_version:"), "{selector}: {text}");
        assert!(!text.contains("terms:\n"), "{selector}: {text}");
    }
}

#[test]
fn template_selector_resolves_through_the_canonical_resolver() {
    let fixture = Fixture::new();
    // A project override is a real tier hit in this fixture tree; the template is read
    // from disk and rendered with the reference's heading, never printed as raw YAML.
    write(
        &fixture
            .root
            .path()
            .join(".kittify/charter-packs/.kittify/overrides/missions/software-dev/templates/spec-template.md"),
        "# Fixture spec\n\nBody line.\n\n",
    );
    let text = fixture
        .render("template:software-dev/spec-template.md")
        .expect("template include");
    assert_eq!(
        text,
        "Template software-dev/spec-template.md (tier: override):\n# Fixture spec\n\nBody line."
    );

    // The packaged built-in default resolves through the same chain.
    let packaged = fixture
        .render("template:software-dev/tasks-template.md")
        .expect("packaged template");
    assert!(
        packaged.starts_with("Template software-dev/tasks-template.md (tier: package_default):\n"),
        "{packaged}"
    );
    assert!(!packaged.ends_with('\n'));
    assert!(packaged.len() > 100, "{packaged}");

    // The reference's refusal family carries no Native gap code.
    let missing = fixture
        .render("template:software-dev/no-such-template.md")
        .unwrap_err();
    assert_eq!(
        missing.code(),
        None,
        "a reference ValueError carries no code"
    );
    assert_eq!(
        missing.to_string(),
        "No template found for selector 'template:software-dev/no-such-template.md'."
    );
    let malformed = fixture.render("template:no-slash").unwrap_err();
    assert_eq!(malformed.code(), None);
    assert_eq!(
        malformed.to_string(),
        "Malformed template selector 'template:no-slash': template_id 'no-slash' is not of the \
         form '<mission>/<name>'"
    );
}

#[test]
fn a_malformed_document_of_an_unrelated_kind_is_never_consulted() {
    let fixture = Fixture::new();
    // A syntax-broken tactic cannot change a directive answer: the identity index scans
    // only the queried kind, so the tactic table is never built here.
    write(
        &fixture.root.path().join("packs/tactics/broken.tactic.yaml"),
        "id: broken-tactic\nsteps: [unclosed\n",
    );
    let text = fixture
        .render("directive:900-fixture-file")
        .expect("directive still renders");
    assert!(text.starts_with("Directive DIRECTIVE_900: Fixture Directive\n"));
    // The same file, queried, is a per-file skip: the artifact is simply not loaded.
    assert_eq!(
        fixture
            .render("tactic:broken-tactic")
            .unwrap_err()
            .to_string(),
        "No tactic found for selector 'tactic:broken-tactic'."
    );
}
