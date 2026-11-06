//! CF32 focused smoke: canonical merged / annotated / lazy non-template include serving.
//!
//! These drive the real `sk_doctrine::context_include` surface over real temporary pack
//! trees, so the canonical authorities are exercised end to end:
//!
//! * `CanonicalOfferingService`'s layered repositories (built-in → org → project field
//!   merge), instead of the identity index's declaring file;
//! * the effective-deny annotation (`load_activation_ledger` + `org_required_stems` +
//!   `effective_deny`/`resolve_deny`), which annotates rather than refuses;
//! * the reference's asymmetry that only `agent-profile`/`glossary-pack` selectors render
//!   on the activation-aware service;
//! * per-kind laziness: a `directive` answer never reads another kind's repository, so an
//!   unrelated kind's load failure cannot change it.
//!
//! Not a deep differential campaign. Deferred proof is enumerated in the lane report
//! (`/tmp/sk-cf32-effective-include-serving-report.md`): exact-Python byte equality of
//! every rendered line against the oracle, `!!`-tag/Unicode bodies, a merged directive
//! repository (Native gap), org-charter floor strictness parity, and a fully paired
//! Source/Native child matrix for these fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_doctrine::context_include::{
    IncludeAccess, IncludeInputs, parse_include_selector, render_include,
};
use sk_doctrine::{OfferingRoots, allow_key, deny_key};

const TACTIC: &str = "schema_version: \"1.0\"\n\
id: scoping-lens\n\
name: Built-in Name\n\
purpose: Built-in purpose line.\n\
steps:\n\
- title: Built-in step title\n\
\x20 description: Built-in step detail.\n";

const TACTIC_OVERLAY: &str = "id: scoping-lens\n\
name: Project Overlay Name\n";

const STYLEGUIDE: &str = "schema_version: \"1.0\"\n\
id: fixture-style\n\
title: Fixture Styleguide\n\
scope: code\n\
principles:\n\
- Fixture principle.\n";

const GLOSSARY: &str = "id: fixture-glossary\n\
provenance: built-in\n\
description: Fixture glossary description.\n\
terms:\n\
- surface: fixture-term\n\
\x20 definition: Fixture term definition.\n\
\x20 confidence: 1.0\n\
\x20 status: active\n";

const PARADIGM: &str = "schema_version: '1.0'\n\
id: fixture-paradigm\n\
name: Fixture Paradigm\n\
summary: Fixture paradigm summary.\n";

const SKILL: &str = "schema_version: '1.0'\n\
id: fixture-skill\n\
title: Fixture Skill\n\
description: Fixture skill description.\n\
form: prompt\n\
triggers: [alpha trigger, beta trigger]\n\
body_path: fixture-skill.skill.md\n";

const STEP_CONTRACT: &str = "schema_version: '1.0'\n\
id: fixture-action\n\
action: fixture-action\n\
mission: software-dev\n\
steps:\n\
- id: bootstrap\n\
\x20 description: Load the fixture context\n";

const DIRECTIVE: &str = "schema_version: \"1.0\"\n\
id: DIRECTIVE_902\n\
title: Fixture Directive\n\
intent: Fixture directive intent.\n\
enforcement: required\n";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A repo fixture with a built-in pack root, one org pack and a project pack root.
struct Fixture {
    root: tempfile::TempDir,
    built_in: PathBuf,
    org: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::TempDir::new().expect("temp");
        let base = root.path();
        let built_in = base.join("packs-built-in");
        let org = base.join("packs-acme");
        let project = base.join("project/.kittify/charter-packs");
        write(&built_in.join("tactics/scoping-lens.tactic.yaml"), TACTIC);
        write(
            &built_in.join("styleguides/fixture-style.styleguide.yaml"),
            STYLEGUIDE,
        );
        write(
            &built_in.join("glossary_packs/fixture-glossary.glossary-pack.yaml"),
            GLOSSARY,
        );
        write(
            &built_in.join("paradigms/fixture-paradigm.paradigm.yaml"),
            PARADIGM,
        );
        write(&built_in.join("skills/fixture-skill.skill.yaml"), SKILL);
        // Source `PackSkillRepository._post_validate` requires a prompt body
        // beside the record; the fixture must be Source-valid for the include
        // consumer to see the skill at all.
        write(
            &built_in.join("skills/fixture-skill.skill.md"),
            "Fixture skill body.\n",
        );
        write(
            &built_in.join("missions/built_in_step_contracts/fixture-action.step-contract.yaml"),
            STEP_CONTRACT,
        );
        write(
            &built_in.join("directives/902-fixture.directive.yaml"),
            DIRECTIVE,
        );
        fs::create_dir_all(&org).expect("org root");
        fs::create_dir_all(&project).expect("project root");
        write(
            &base.join(".kittify/config.yaml"),
            "# authored project config\n",
        );
        Self {
            root,
            built_in,
            org,
            project,
        }
    }

    fn root(&self) -> &Path {
        self.root.path()
    }

    fn access(&self) -> IncludeAccess {
        IncludeAccess::with_roots(
            self.root(),
            OfferingRoots {
                built_in: Some(self.built_in.clone()),
                org_roots: vec![self.org.clone()],
                project: Some(self.project.clone()),
            },
        )
        .expect("fixture access")
    }

    /// Write the activation ledger with the given extra keys.
    fn ledger(&self, extra: &str) {
        write(&self.root().join(".kittify/charter/activation.yaml"), extra);
    }

    fn render(&self, selector: &str) -> Result<String, sk_doctrine::context_include::IncludeError> {
        let parsed = parse_include_selector(selector).expect("selector parses");
        let access = self.access();
        let inputs = IncludeInputs {
            project_root: self.root(),
            access: &access,
        };
        render_include(&inputs, &parsed, selector)
    }

    fn deny(&self, kind: ArtifactKind, id: &str) -> String {
        format!("{}:\n  - {id}\n", deny_key(kind).expect("deny key"))
    }

    fn allow(&self, kind: ArtifactKind, ids: &[&str]) -> String {
        let mut text = format!("{}:\n", allow_key(kind).expect("allow key"));
        for id in ids {
            text.push_str(&format!("  - {id}\n"));
        }
        text
    }
}

#[test]
fn a_project_overlay_merges_onto_the_built_in_record() {
    // The overlay re-declares only `id` + `name`; the reference's field merge keeps the
    // built-in purpose and steps, and the rendered body/dump are of the MERGED model.
    let fixture = Fixture::new();
    write(
        &fixture.project.join("tactic/scoping-lens.tactic.yaml"),
        TACTIC_OVERLAY,
    );
    let text = fixture
        .render("tactic:scoping-lens")
        .expect("merged tactic");
    assert!(
        text.starts_with("Tactic scoping-lens: Project Overlay Name\n"),
        "{text}"
    );
    assert!(text.contains("    Name: Project Overlay Name"), "{text}");
    assert!(
        text.contains("    Purpose: Built-in purpose line."),
        "the built-in field is inherited, not dropped: {text}"
    );
    assert!(
        text.contains("      - Built-in step title"),
        "the built-in steps are inherited: {text}"
    );
    assert!(text.contains("        Built-in step detail."), "{text}");
    assert!(
        text.contains("\"name\": \"Project Overlay Name\""),
        "the model dump is the merged record: {text}"
    );
    assert!(
        text.contains("\"purpose\": \"Built-in purpose line.\""),
        "{text}"
    );
}

#[test]
fn an_org_overlay_replaces_the_built_in_field_it_declares() {
    let fixture = Fixture::new();
    write(
        &fixture.org.join("tactics/scoping-lens.tactic.yaml"),
        "id: scoping-lens\npurpose: Org purpose line.\n",
    );
    let text = fixture.render("tactic:scoping-lens").expect("org tactic");
    assert!(text.contains("    Name: Built-in Name"), "{text}");
    assert!(text.contains("    Purpose: Org purpose line."), "{text}");
    assert!(text.contains("      - Built-in step title"), "{text}");
}

#[test]
fn a_project_overlay_merges_onto_the_built_in_base_not_the_org_result() {
    // The reference's `_merge` always starts from the built-in item, so a project
    // overlay that re-declares an id discards an org field it does not repeat. This is
    // the pinned precedence; Native must not "improve" it into later-wins composition.
    let fixture = Fixture::new();
    write(
        &fixture.org.join("tactics/scoping-lens.tactic.yaml"),
        "id: scoping-lens\npurpose: Org purpose line.\n",
    );
    write(
        &fixture.project.join("tactic/scoping-lens.tactic.yaml"),
        TACTIC_OVERLAY,
    );
    let text = fixture
        .render("tactic:scoping-lens")
        .expect("layered tactic");
    assert!(text.contains("    Name: Project Overlay Name"), "{text}");
    assert!(
        text.contains("    Purpose: Built-in purpose line."),
        "the project overlay merges onto the built-in base: {text}"
    );
    assert!(text.contains("      - Built-in step title"), "{text}");
}

#[test]
fn a_denied_selector_renders_the_reference_note_instead_of_the_body() {
    let fixture = Fixture::new();
    fixture.ledger(&fixture.deny(ArtifactKind::Tactic, "scoping-lens"));
    assert_eq!(
        fixture
            .render("tactic:scoping-lens")
            .expect("denied tactic"),
        "tactic:scoping-lens is deactivated in this project \
         (charter activate tactic scoping-lens restores it)"
    );
    // The generic probe is gated against every kind and names the denied kind's
    // hyphenated canonical token as the restore argument.
    assert_eq!(
        fixture
            .render("artifact:scoping-lens")
            .expect("denied probe"),
        "artifact:scoping-lens is deactivated in this project \
         (charter activate tactic scoping-lens restores it)"
    );
    // A deny of a multi-word kind hyphenates its canonical token.
    let hyphenated = Fixture::new();
    hyphenated.ledger(&hyphenated.deny(ArtifactKind::GlossaryPack, "fixture-glossary"));
    assert_eq!(
        hyphenated
            .render("artifact:fixture-glossary")
            .expect("denied glossary probe"),
        "artifact:fixture-glossary is deactivated in this project \
         (charter activate glossary-pack fixture-glossary restores it)"
    );
}

#[test]
fn the_deny_annotation_is_decided_before_any_artifact_tree_is_read() {
    // No artifact file exists for this id at all: the effective-deny decision (ledger +
    // org floor + identity spellings) is enough to annotate, so the note — not a
    // `No tactic found` miss — is the reference's answer.
    let fixture = Fixture::new();
    fixture.ledger(&fixture.deny(ArtifactKind::Tactic, "never-written"));
    assert_eq!(
        fixture.render("tactic:never-written").expect("denied note"),
        "tactic:never-written is deactivated in this project \
         (charter activate tactic never-written restores it)"
    );
}

#[test]
fn an_org_required_id_defeats_a_stale_deny() {
    // The org required floor nets a stale deny: a denied id an org pack requires is NOT
    // annotated (the reference's `D* = resolve_deny(D) − R`). The org pack is declared
    // through the canonical project config registry, which is what the floor read uses.
    let fixture = Fixture::new();
    write(
        &fixture.org.join("pack.yaml"),
        "pack_id: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: acme\npack_level: org\n",
    );
    write(
        &fixture.org.join("org-charter.yaml"),
        "schema_version: 2\norg_name: Acme\nrequired_tactics: [scoping-lens]\n",
    );
    write(
        &fixture.root().join(".kittify/config.yaml"),
        &format!(
            "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
            fixture.org.display()
        ),
    );
    fixture.ledger(&fixture.deny(ArtifactKind::Tactic, "scoping-lens"));
    let text = fixture
        .render("tactic:scoping-lens")
        .expect("required tactic still renders");
    assert!(
        text.starts_with("Tactic scoping-lens: Built-in Name\n"),
        "{text}"
    );
}

#[test]
fn the_allow_list_gates_only_the_activation_aware_kinds() {
    let fixture = Fixture::new();
    // An explicit allow list that does not name the id is the reference's structured
    // miss for a gated kind (glossary-pack renders on the activation-aware service).
    fixture.ledger(&fixture.allow(ArtifactKind::GlossaryPack, &["some-other-pack"]));
    assert_eq!(
        fixture
            .render("glossary-pack:fixture-glossary")
            .expect_err("gated miss")
            .to_string(),
        "No glossary_pack found for selector 'glossary_pack:fixture-glossary'."
    );
    // Every other kind renders on the plain service, so the same empty allow list does
    // not remove it — the reference's documented asymmetry.
    let plain = Fixture::new();
    plain.ledger(&plain.allow(ArtifactKind::Tactic, &[]));
    let text = plain.render("tactic:scoping-lens").expect("plain tactic");
    assert!(
        text.starts_with("Tactic scoping-lens: Built-in Name\n"),
        "{text}"
    );
}

#[test]
fn a_load_failure_of_an_unrelated_kind_never_changes_a_directive_answer() {
    let fixture = Fixture::new();
    // A project overlay whose raw `id` is a list raises the reference's
    // `TypeError: unhashable type: 'list'` while the tactic repository loads.
    write(
        &fixture.project.join("tactic/broken.tactic.yaml"),
        "id: [not, hashable]\nname: Broken\n",
    );
    let text = fixture
        .render("directive:902-fixture")
        .expect("directive is independent");
    assert!(
        text.starts_with("Directive DIRECTIVE_902: Fixture Directive\n"),
        "{text}"
    );
    // The tactic repository that really does load that file surfaces the abort.
    let error = fixture
        .render("tactic:scoping-lens")
        .expect_err("constructor abort");
    assert_eq!(error.code(), None, "a Python exception is not a gap code");
    assert!(
        error
            .to_string()
            .contains("TypeError: unhashable type: 'list'"),
        "{error}"
    );
}

#[test]
fn the_four_cf31_kinds_render_their_source_inline_bodies() {
    let fixture = Fixture::new();

    let glossary = fixture
        .render("glossary-pack:fixture-glossary")
        .expect("glossary pack");
    assert!(
        glossary.starts_with("Glossary pack fixture-glossary: fixture-glossary\n"),
        "{glossary}"
    );
    // `_format_inline_glossary_pack_body` is the two-space shape (not 4/6).
    assert!(
        glossary.contains("\n  Fixture glossary description.\n"),
        "{glossary}"
    );
    assert!(
        glossary.contains("\n  - fixture-term: Fixture term definition.\n"),
        "{glossary}"
    );

    let paradigm = fixture
        .render("paradigm:fixture-paradigm")
        .expect("paradigm");
    assert!(
        paradigm.contains("    Name: Fixture Paradigm"),
        "{paradigm}"
    );
    assert!(
        paradigm.contains("    Summary: Fixture paradigm summary."),
        "{paradigm}"
    );

    let skill = fixture.render("skill:fixture-skill").expect("pack skill");
    assert!(
        skill.starts_with("Skill fixture-skill: Fixture Skill\n"),
        "{skill}"
    );
    assert!(
        skill.contains("    Description: Fixture skill description."),
        "{skill}"
    );
    assert!(skill.contains("    Form: prompt"), "{skill}");
    assert!(
        skill.contains("    Triggers: alpha trigger, beta trigger"),
        "{skill}"
    );

    let contract = fixture
        .render("mission-step-contract:fixture-action")
        .expect("step contract");
    assert!(
        contract.starts_with("Mission step contract fixture-action: fixture-action\n"),
        "{contract}"
    );
    assert!(
        contract.contains("    Action: fixture-action"),
        "{contract}"
    );
    assert!(contract.contains("    Mission: software-dev"), "{contract}");
    assert!(
        contract.contains("    Steps:\n      - bootstrap: Load the fixture context"),
        "{contract}"
    );
}

#[test]
fn an_unadmitted_document_is_a_miss_not_a_fabricated_dump() {
    // The repository admits whole documents; a document the reference's model refuses is
    // skipped, so the include must miss instead of projecting a partial body.
    let fixture = Fixture::new();
    write(
        &fixture.built_in.join("tactics/broken-shape.tactic.yaml"),
        "schema_version: '1.0'\nid: broken-shape\nname: Broken\nsteps:\n- description: no title\n",
    );
    assert_eq!(
        fixture
            .render("tactic:broken-shape")
            .expect_err("skipped document")
            .to_string(),
        "No tactic found for selector 'tactic:broken-shape'."
    );
}

#[test]
fn a_missing_artifact_still_refuses_with_the_reference_message() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .render("styleguide:no-such-style")
            .expect_err("miss")
            .to_string(),
        "No styleguide found for selector 'styleguide:no-such-style'."
    );
    assert_eq!(
        fixture
            .render("agent-profile:no-such-profile")
            .expect_err("miss")
            .to_string(),
        "No agent_profile found for selector 'agent_profile:no-such-profile'."
    );
}
