//! CF2 — agent profiles are validated on the canonical Kernel object graph.
//!
//! These are ordinary authored-YAML documents (no fixtures are copied from
//! Source) that pin the *behaviour* of the graph migration:
//!
//! * an ordinary profile still loads with the reference model's defaults;
//! * a Source-valid cycle in a value the model does not enter does not reject
//!   the profile (a tree projection would have refused it);
//! * a cycle in a *required* field is refused at the model phase with the
//!   model/schema wording, never as a YAML read error;
//! * an anchored shared value is read in both places;
//! * an authored surrogate keeps its exact code point;
//! * the layer overlay deep-merges nested mappings without materialising an
//!   intermediate document.
//!
//! Only the public repository surface is used, so the assertions are about
//! observable behaviour rather than an internal DTO.

use std::fs;
use std::path::Path;

use sk_doctrine::ProfileRepository;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    fs::write(path, text).expect("write profile");
}

struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        for (name, text) in files {
            write(
                &directory.path().join("project/agent_profiles").join(name),
                text,
            );
        }
        fs::create_dir_all(directory.path().join("built-in/agent_profiles")).expect("built-in dir");
        Self { directory }
    }

    fn load(&self) -> ProfileRepository {
        ProfileRepository::load_profile_directories(
            &self.directory.path().join("built-in/agent_profiles"),
            &[],
            Some(&self.directory.path().join("project/agent_profiles")),
            None,
        )
        .expect("repository load")
    }
}

const REQUIRED_FIELDS: &str = "profile-id: {id}\nname: {name}\npurpose: prove the graph path\nroles: [reviewer]\n\
     specialization:\n  primary-focus: review\n";

fn profile(id: &str, name: &str, extra: &str) -> String {
    REQUIRED_FIELDS.replace("{id}", id).replace("{name}", name) + extra
}

#[test]
fn an_ordinary_profile_loads_with_the_reference_model_defaults() {
    let fixture = Fixture::new(&[(
        "smoke.agent.yaml",
        &profile("graph-smoke", "Graph Smoke", ""),
    )]);
    let repository = fixture.load();
    assert!(
        repository.skipped().is_empty(),
        "{:?}",
        repository.skipped()
    );
    let loaded = repository.get_str("graph-smoke").expect("loaded profile");

    assert_eq!(
        loaded
            .roles()
            .iter()
            .filter_map(|r| r.to_scalar_string().ok())
            .collect::<Vec<_>>(),
        vec!["reviewer".to_owned()]
    );
    assert_eq!(
        loaded
            .primary_focus()
            .and_then(|f| f.to_scalar_string().ok())
            .as_deref(),
        Some("review")
    );
    // Declared defaults, from `model_dump(by_alias=True)`.
    assert_eq!(loaded.routing_priority(), 50);
    assert_eq!(loaded.max_concurrent_tasks(), Some(5));
    assert_eq!(
        loaded.schema_version().to_scalar_string().ok().as_deref(),
        Some("1.0")
    );
    assert!(
        loaded.model_dump().string("description").is_some(),
        "the default description is present in the model dump"
    );
}

#[test]
fn an_ignored_cycle_does_not_reject_an_otherwise_valid_profile() {
    // `mode-defaults` items are Pydantic models with `extra="ignore"`, so the
    // cyclic `extra:` value is never read. A whole-document tree projection
    // cannot represent the cycle at all; the graph can, and the profile loads.
    let fixture = Fixture::new(&[(
        "cycle.agent.yaml",
        &profile(
            "cycle-ok",
            "Cycle Ok",
            "mode-defaults:\n  - mode: m\n    description: d\n    use-case: u\n    extra: &c [*c]\n",
        ),
    )]);
    let repository = fixture.load();
    assert!(
        repository.skipped().is_empty(),
        "{:?}",
        repository.skipped()
    );
    let loaded = repository.get_str("cycle-ok").expect("loaded profile");
    assert_eq!(loaded.list("mode-defaults").len(), 1);
    // The model dump is a real (acyclic) document.
    assert_eq!(loaded.model_dump().list("mode-defaults").len(), 1);
}

#[test]
fn a_cycle_in_a_required_field_is_refused_at_the_model_phase() {
    let fixture = Fixture::new(&[(
        "required-cycle.agent.yaml",
        "profile-id: cycle-bad\nname: &n [*n]\npurpose: p\nroles: [reviewer]\n\
         specialization: {primary-focus: x}\n",
    )]);
    let repository = fixture.load();
    assert!(repository.get_str("cycle-bad").is_none());
    let skipped = repository.skipped();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    let summary = &skipped[0].summary;
    assert!(
        summary.contains("is not of type 'string'"),
        "the refusal is the schema/model shape error, not a read failure: {summary}"
    );
    assert!(
        !summary.contains("YAML/read error"),
        "a Source-valid cyclic document must not be a YAML read error: {summary}"
    );
    assert_eq!(
        skipped[0].profile_id_label().as_deref(),
        Some("cycle-bad"),
        "the declared id is still reported"
    );
}

#[test]
fn an_anchored_shared_value_reads_the_same_in_both_fields() {
    let fixture = Fixture::new(&[(
        "shared.agent.yaml",
        "profile-id: shared\nname: &v shared text\npurpose: *v\nroles: [reviewer]\n\
         specialization: {primary-focus: x}\n",
    )]);
    let repository = fixture.load();
    assert!(
        repository.skipped().is_empty(),
        "{:?}",
        repository.skipped()
    );
    let loaded = repository.get_str("shared").expect("loaded profile");
    assert_eq!(
        loaded
            .name()
            .and_then(|n| n.to_scalar_string().ok())
            .as_deref(),
        Some("shared text")
    );
    assert_eq!(
        loaded
            .body
            .string("purpose")
            .and_then(|p| p.to_scalar_string().ok())
            .as_deref(),
        Some("shared text")
    );
}

#[test]
fn an_exact_surrogate_name_keeps_its_code_point() {
    let fixture = Fixture::new(&[(
        "surrogate.agent.yaml",
        "profile-id: exact\nname: \"\\uD800\"\npurpose: p\nroles: [reviewer]\n\
         specialization: {primary-focus: x}\n",
    )]);
    let repository = fixture.load();
    assert!(
        repository.skipped().is_empty(),
        "{:?}",
        repository.skipped()
    );
    let loaded = repository.get_str("exact").expect("loaded profile");
    assert_eq!(
        loaded.name().expect("name").codepoints(),
        [0xd800],
        "the authored escape is one unpaired surrogate, not a replacement char"
    );
    // The model dump round-trips the same exact code point.
    assert_eq!(
        loaded
            .model_dump()
            .string("name")
            .expect("dumped name")
            .codepoints(),
        [0xd800]
    );
}

#[test]
fn the_layer_overlay_deep_merges_nested_mappings_through_the_graph() {
    let directory = tempfile::tempdir().expect("tempdir");
    write(
        &directory
            .path()
            .join("built-in/agent_profiles/ivan.agent.yaml"),
        &profile(
            "ivan",
            "Ivan",
            "description: kept\ncapabilities: [a, b]\nspecialization-context:\n  languages: [python]\n  frameworks: [django]\n",
        ),
    );
    write(
        &directory
            .path()
            .join("project/agent_profiles/ivan.agent.yaml"),
        "profile-id: ivan\ndescription: Org\nspecialization-context:\n  frameworks: [flask]\n",
    );
    let repository = ProfileRepository::load_profile_directories(
        &directory.path().join("built-in/agent_profiles"),
        &[],
        Some(&directory.path().join("project/agent_profiles")),
        None,
    )
    .expect("repository load");
    assert!(
        repository.skipped().is_empty(),
        "{:?}",
        repository.skipped()
    );
    let loaded = repository.get_str("ivan").expect("loaded profile");
    assert_eq!(
        loaded
            .description()
            .and_then(|d| d.to_scalar_string().ok())
            .as_deref(),
        Some("Org"),
        "the higher layer replaces a scalar"
    );
    assert_eq!(
        loaded.domain_keywords(),
        Vec::new(),
        "an unmentioned nested list stays empty, not invented"
    );
    let context = loaded.body.get("specialization-context").expect("context");
    assert_eq!(
        context
            .get("languages")
            .and_then(|v| v.as_sequence())
            .map(|v| v.len()),
        Some(1),
        "a nested mapping key only the lower layer declared is kept"
    );
    assert_eq!(
        context
            .get("frameworks")
            .and_then(|v| v.as_sequence())
            .and_then(|v| v.first())
            .and_then(|v| v.as_str()),
        Some("flask"),
        "the higher layer replaces the nested value it declares"
    );
}
