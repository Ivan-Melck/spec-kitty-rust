//! CF29 — `charter activate --preset` execution: the pure plan and the one
//! activation write, against fresh temporary projects.
//!
//! The built-in pack is the repository's real fetched tree (the same one the
//! CLI reads), so `default` and `minimal` are the shipped presets, and the
//! fixture org pack supplies the layered ids, floors and test presets.
//!
//! These are bounded functional smoke tests: they prove the essential
//! plan/write behaviour, refusals and non-destruction. Differential (Source vs
//! Native) proof, Unicode ids, cycle/depth behaviour and concurrency under a
//! real second process are recorded as outstanding work in the lane report.

use std::fs;
use std::path::{Path, PathBuf};

use sk_doctrine::preset_application::{
    PresetApplicationError, apply_preset_plan, apply_preset_plan_observed, plan_preset_application,
};
use sk_doctrine::{DoctrineError, YamlDocument};

const ACTIVATION_REL: &str = ".kittify/charter/activation.yaml";

struct Fixture {
    directory: tempfile::TempDir,
    project: PathBuf,
}

impl Fixture {
    fn new(activation: &str) -> Self {
        let directory = tempfile::tempdir().expect("fixture root");
        let project = directory.path().to_path_buf();
        write(
            &project.join(".kittify/config.yaml"),
            "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: acme-pack\n",
        );
        write(&project.join(ACTIVATION_REL), activation);
        let fixture = Self { directory, project };
        // Two org directives: one the presets list, one the org charter requires.
        fixture.artifact(
            "directives/900-fixture.directive.yaml",
            "id: DIRECTIVE_900\ntitle: Fixture directive\n",
        );
        fixture.artifact(
            "directives/901-org-required.directive.yaml",
            "id: DIRECTIVE_901\ntitle: Org required directive\n",
        );
        fixture
    }

    fn path(&self) -> &Path {
        &self.project
    }

    fn artifact(&self, relative: &str, text: &str) {
        write(&self.project.join("acme-pack").join(relative), text);
    }

    fn preset(&self, name: &str, body: &str) {
        write(
            &self
                .project
                .join("acme-pack/presets")
                .join(format!("{name}.yaml")),
            body,
        );
    }

    fn org_charter(&self, text: &str) {
        self.artifact("org-charter.yaml", text);
    }

    fn activation_text(&self) -> String {
        fs::read_to_string(self.project.join(ACTIVATION_REL)).expect("the activation file")
    }

    fn activation(&self) -> YamlDocument {
        YamlDocument::open(&self.project.join(ACTIVATION_REL)).expect("the activation document")
    }

    fn write_activation(&self, text: &str) {
        write(&self.project.join(ACTIVATION_REL), text);
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, text).expect("fixture file");
}

fn ids(document: &YamlDocument, key: &str) -> Option<Vec<String>> {
    document.get_string_list(key).expect("a string list")
}

#[test]
fn the_default_preset_leaves_the_keys_it_does_not_govern_alone() {
    let fixture = Fixture::new(
        "# hand-written header stays\n\
         activated_directives:\n  - 024-locality-of-change\n\
         activated_skills:\n  - land-pr\n\
         activated_glossary_packs:\n  - my-glossary\n\
         deactivated_mystery:\n  - keep-me\n\
         mission_type_activations:\n  - software-dev\n  - documentation\n  - research\n  - plan\n",
    );
    let plan = plan_preset_application(fixture.path(), "built-in", "default").expect("the plan");
    assert!(
        plan.target_file.ends_with(ACTIVATION_REL),
        "the ledger is the write target: {}",
        plan.target_file.display()
    );
    assert_eq!(
        plan.removed,
        vec!["activated_directives".to_owned()],
        "a key the preset leaves out is removed"
    );
    assert!(
        plan.written.is_empty(),
        "the default preset writes no allow list: {:?}",
        plan.written
    );
    assert_eq!(
        plan.customised_changes,
        vec!["activated_directives".to_owned()],
        "a hand-made list is customised"
    );
    let before = fixture.activation_text();
    let refusal = apply_preset_plan(fixture.path(), &plan, false).expect_err("customised");
    match &refusal {
        PresetApplicationError::WouldOverwrite(plan) => {
            assert_eq!(
                plan.customised_changes,
                vec!["activated_directives".to_owned()]
            );
            assert_eq!(refusal.code(), "PRESET_WOULD_OVERWRITE");
        }
        other => panic!("expected a forced-overwrite refusal, got {other:?}"),
    }
    assert_eq!(
        fixture.activation_text(),
        before,
        "a refusal writes nothing"
    );

    let outcome = apply_preset_plan(fixture.path(), &plan, true).expect("the forced write");
    assert!(outcome.written.is_empty());

    let document = fixture.activation();
    assert_eq!(ids(&document, "activated_directives"), None, "removed");
    assert_eq!(
        ids(&document, "activated_skills"),
        Some(vec!["land-pr".to_owned()]),
        "a skill keeps its own absence contract: never read or written"
    );
    assert_eq!(
        ids(&document, "activated_glossary_packs"),
        Some(vec!["my-glossary".to_owned()]),
        "a glossary pack keeps its own absence contract"
    );
    assert_eq!(
        ids(&document, "deactivated_mystery"),
        Some(vec!["keep-me".to_owned()]),
        "an unknown exclusion key survives the write"
    );
    let text = fixture.activation_text();
    assert!(
        text.contains("# hand-written header stays"),
        "the authored comment survives: {text}"
    );
}

#[test]
fn an_empty_list_activates_nothing_and_an_omitted_mission_type_key_is_untouched() {
    let fixture = Fixture::new("mission_type_activations:\n  - software-dev\n");
    fixture.preset(
        "empty",
        "name: empty\ndescription: Activates no tactics.\nactivated_tactics: []\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "empty").expect("the plan");
    assert_eq!(
        plan.written.get("activated_tactics"),
        Some(&Vec::<String>::new()),
        "an empty list is written as an empty list"
    );
    assert!(
        !plan.written.contains_key("mission_type_activations"),
        "a preset that omits mission types leaves that key untouched"
    );
    assert!(
        !plan
            .removed
            .contains(&"mission_type_activations".to_owned())
    );
    apply_preset_plan(fixture.path(), &plan, false).expect("the write");

    let document = fixture.activation();
    assert_eq!(ids(&document, "activated_tactics"), Some(Vec::new()));
    assert_eq!(
        ids(&document, "mission_type_activations"),
        Some(vec!["software-dev".to_owned()]),
        "the omitted key keeps its authored value"
    );
    assert_eq!(
        ids(&document, "activated_directives"),
        None,
        "an absent per-kind key stays absent"
    );
}

#[test]
fn a_listed_but_empty_mission_type_list_is_written() {
    let fixture = Fixture::new("mission_type_activations:\n  - software-dev\n");
    fixture.preset(
        "no-types",
        "name: no-types\ndescription: No mission types.\nmission_type_activations: []\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "no-types").expect("the plan");
    assert_eq!(
        plan.written.get("mission_type_activations"),
        Some(&Vec::<String>::new())
    );
    // The authored list is hand-made, so emptying it needs --force (OD-6).
    assert_eq!(
        plan.customised_changes,
        vec!["mission_type_activations".to_owned()]
    );
    apply_preset_plan(fixture.path(), &plan, true).expect("the forced write");
    let document = fixture.activation();
    assert_eq!(ids(&document, "mission_type_activations"), Some(Vec::new()));
}

#[test]
fn a_customised_key_is_refused_then_forced_and_the_diff_names_both_values() {
    let fixture = Fixture::new(
        "activated_directives:\n  - 024-locality-of-change\nactivated_tactics:\n  - acceptance-test-first\n",
    );
    let plan = plan_preset_application(fixture.path(), "built-in", "minimal").expect("the plan");
    assert_eq!(
        plan.customised_changes,
        vec!["activated_directives".to_owned()]
    );
    let refusal = apply_preset_plan(fixture.path(), &plan, false).expect_err("refused");
    let details = refusal.detail_lines();
    assert!(
        details
            .iter()
            .any(|line| line.contains("activated_directives: [024-locality-of-change] -> [")),
        "the diff names both values: {details:?}"
    );
    assert_eq!(
        fixture.activation_text(),
        "activated_directives:\n  - 024-locality-of-change\nactivated_tactics:\n  - acceptance-test-first\n"
    );

    let outcome = apply_preset_plan(fixture.path(), &plan, true).expect("forced");
    let written = outcome
        .written
        .get("activated_directives")
        .expect("written");
    assert!(
        written.contains(&"024-locality-of-change".to_owned()),
        "minimal lists that directive: {written:?}"
    );
    let document = fixture.activation();
    assert_eq!(
        ids(&document, "activated_directives"),
        Some(written.clone())
    );
    assert!(
        plan.preset == "minimal" && plan.pack == "built-in",
        "the preset name is never persisted to the document"
    );
    assert!(
        !fixture.activation_text().contains("minimal"),
        "the preset name is not written: {}",
        fixture.activation_text()
    );
}

#[test]
fn a_listed_declared_id_is_stored_as_its_stem_and_the_org_floor_unions_in() {
    let fixture = Fixture::new("");
    fixture.org_charter("org_name: acme\nrequired_directives:\n  - DIRECTIVE_901\n");
    fixture.preset(
        "org-floor",
        "name: org-floor\ndescription: One directive plus the org floor.\nactivated_directives:\n  - DIRECTIVE_900\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "org-floor").expect("the plan");
    assert_eq!(
        plan.written.get("activated_directives"),
        Some(&vec![
            "900-fixture".to_owned(),
            "901-org-required".to_owned()
        ]),
        "the declared id resolves to its file stem and the required id is appended"
    );
    apply_preset_plan(fixture.path(), &plan, false).expect("the write");
    assert_eq!(
        ids(&fixture.activation(), "activated_directives"),
        Some(vec![
            "900-fixture".to_owned(),
            "901-org-required".to_owned()
        ])
    );
}

#[test]
fn the_kind_gate_gains_the_plural_of_every_org_required_kind() {
    let fixture = Fixture::new("");
    fixture.org_charter("org_name: acme\nrequired_directives:\n  - 901-org-required\n");
    fixture.preset(
        "gate",
        "name: gate\ndescription: A gate that lists no directives.\nactivated_kinds: []\nactivated_tactics:\n  - acceptance-test-first\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "gate").expect("the plan");
    assert_eq!(
        plan.written.get("activated_kinds"),
        Some(&vec!["directives".to_owned()]),
        "the kind gate never switches off an org requirement"
    );
}

#[test]
fn an_unknown_pack_an_unknown_preset_and_a_malformed_file_are_coded_refusals() {
    let fixture = Fixture::new("");
    let error = plan_preset_application(fixture.path(), "nope", "default").expect_err("no pack");
    assert_eq!(error.code(), "PACK_NOT_FOUND");
    assert_eq!(error.pack(), Some("nope"));
    assert_eq!(
        error.available(),
        [
            "built-in".to_owned(),
            "acme".to_owned(),
            "project".to_owned()
        ]
    );

    let error = plan_preset_application(fixture.path(), "built-in", "nope").expect_err("no preset");
    assert_eq!(error.code(), "PRESET_NOT_FOUND");
    assert_eq!(error.preset(), Some("nope"));
    assert_eq!(
        error.available(),
        ["default".to_owned(), "minimal".to_owned()]
    );

    let error = plan_preset_application(fixture.path(), "project", "default")
        .expect_err("no project presets");
    assert_eq!(error.code(), "PRESET_NOT_FOUND");
    assert!(error.available().is_empty());

    fixture.preset(
        "broken",
        "name: broken\ndescription: Fine.\nactivated_skills: []\n",
    );
    let error = plan_preset_application(fixture.path(), "acme", "broken").expect_err("malformed");
    assert_eq!(error.code(), "PRESET_FORMAT_INVALID");
    assert!(
        error
            .to_string()
            .contains("keeps its own activation contract"),
        "{error}"
    );
    assert_eq!(fixture.activation_text(), "", "no refusal writes anything");
}

#[test]
fn an_id_that_resolves_nowhere_is_refused_by_kind_key() {
    let fixture = Fixture::new("");
    fixture.preset(
        "ghost",
        "name: ghost\ndescription: Lists a missing directive.\nactivated_directives:\n  - 999-ghost\n",
    );
    let error = plan_preset_application(fixture.path(), "acme", "ghost").expect_err("unresolved");
    assert_eq!(error.code(), "PRESET_ID_UNRESOLVED");
    assert_eq!(
        error
            .unresolved()
            .and_then(|unresolved| unresolved.get("activated_directives")),
        Some(&vec!["999-ghost".to_owned()])
    );
    assert!(
        error.to_string().contains("999-ghost"),
        "the refusal names the id: {error}"
    );

    fixture.preset(
        "ghost-type",
        "name: ghost-type\ndescription: Lists a missing mission type.\nmission_type_activations:\n  - made-up-mission\n",
    );
    let error =
        plan_preset_application(fixture.path(), "acme", "ghost-type").expect_err("unresolved");
    assert_eq!(
        error
            .unresolved()
            .and_then(|unresolved| unresolved.get("mission_type_activations")),
        Some(&vec!["made-up-mission".to_owned()])
    );
    assert_eq!(fixture.activation_text(), "");
}

#[test]
fn a_plan_that_changes_nothing_writes_nothing() {
    let fixture = Fixture::new("activated_directives:\n  - 024-locality-of-change\n");
    fixture.preset(
        "same",
        "name: same\ndescription: Already in force.\nactivated_directives:\n  - 024-locality-of-change\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "same").expect("the plan");
    assert!(plan.is_noop(), "no key changes: {:?}", plan.changes);
    assert!(plan.removed.is_empty());
    let before = fixture.activation_text();
    let outcome = apply_preset_plan(fixture.path(), &plan, false).expect("the no-op");
    assert!(outcome.written.is_empty());
    assert_eq!(fixture.activation_text(), before);
}

#[test]
fn without_force_a_deactivated_listed_id_is_left_out_and_reported() {
    let fixture = Fixture::new("deactivated_directives:\n  - 900-fixture\n");
    fixture.preset(
        "include",
        "name: include\ndescription: Lists the deactivated directive.\nactivated_directives:\n  - 900-fixture\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "include").expect("the plan");
    assert_eq!(
        plan.denied_listed.get("activated_directives"),
        Some(&vec!["900-fixture".to_owned()]),
        "the listed id is deactivated"
    );
    let outcome = apply_preset_plan(fixture.path(), &plan, false).expect("the write");
    assert_eq!(
        outcome.kept_deactivated.get("activated_directives"),
        Some(&vec!["900-fixture".to_owned()])
    );
    assert!(outcome.cleared.is_empty());
    let document = fixture.activation();
    assert_eq!(
        ids(&document, "activated_directives"),
        Some(Vec::new()),
        "the deactivated id is left out of the written list"
    );
    assert_eq!(
        ids(&document, "deactivated_directives"),
        Some(vec!["900-fixture".to_owned()]),
        "the exclusion stays recorded"
    );
}

#[test]
fn force_writes_the_listed_id_back_and_clears_its_exclusion() {
    let fixture = Fixture::new("deactivated_directives:\n  - 900-fixture\n");
    fixture.preset(
        "include",
        "name: include\ndescription: Lists the deactivated directive.\nactivated_directives:\n  - 900-fixture\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "include").expect("the plan");
    let outcome = apply_preset_plan(fixture.path(), &plan, true).expect("the forced write");
    assert_eq!(
        outcome.cleared.get("deactivated_directives"),
        Some(&vec!["900-fixture".to_owned()]),
        "the cleared exclusion is reported by deny key"
    );
    assert_eq!(
        outcome.written.get("activated_directives"),
        Some(&vec!["900-fixture".to_owned()])
    );
    let document = fixture.activation();
    assert_eq!(
        ids(&document, "activated_directives"),
        Some(vec!["900-fixture".to_owned()])
    );
    assert_eq!(
        ids(&document, "deactivated_directives"),
        None,
        "an emptied exclusion list is removed"
    );
}

#[test]
fn a_concurrent_edit_is_re_read_and_kept_while_the_strict_capability_still_refuses() {
    // CF34 correction, measured against the pinned Source: `apply_preset_plan`
    // prepares its write from the authority's CURRENT bytes at apply time, so a
    // concurrent edit between plan and apply is re-read, applied and kept — it is
    // NOT refused with `precondition_changed`. That refusal was a Native-only
    // narrowing; it stays available as the separately named
    // `apply_preset_plan_observed`. Both halves below use the same real
    // pre-image (the plan made against the empty file) and the same concurrent
    // bytes (a comment another writer added).
    let fixture = Fixture::new("");
    fixture.preset(
        "one",
        "name: one\ndescription: One directive.\nactivated_directives:\n  - 900-fixture\n",
    );
    let plan = plan_preset_application(fixture.path(), "acme", "one").expect("the plan");
    let concurrent = "# a concurrent writer got here first\n";
    fixture.write_activation(concurrent);

    // 1. The strict Native capability refuses and mutates nothing.
    let error = apply_preset_plan_observed(fixture.path(), &plan, false).expect_err("stale plan");
    match &error {
        PresetApplicationError::Doctrine(DoctrineError::PreconditionChanged { .. }) => {}
        other => panic!("expected precondition_changed, got {other:?}"),
    }
    assert_eq!(
        fixture.activation_text(),
        concurrent,
        "the strict refusal mutates nothing"
    );

    // 2. The Source composition re-reads, applies, and keeps the other writer's line.
    let outcome = apply_preset_plan(fixture.path(), &plan, false).expect("the re-read apply");
    let document = fixture.activation();
    assert_eq!(
        ids(&document, "activated_directives"),
        Some(vec!["900-fixture".to_owned()])
    );
    assert_eq!(
        outcome.written.get("activated_directives"),
        ids(&document, "activated_directives").as_ref()
    );
    assert!(
        fixture
            .activation_text()
            .contains("a concurrent writer got here first"),
        "the other writer's comment survives our write"
    );
    drop(fixture.directory);
}
