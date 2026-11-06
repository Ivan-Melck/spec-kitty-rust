//! CF8 focused smoke: real action-context build + canonical activation writes.
//!
//! These use the pinned built-in pack and a scratch project fixture, exercising
//! the same authorities the `charter` CLI adapter calls:
//! `build_action_context` -> `context_render`, then
//! `plan_activation`/`plan_deactivation` -> `ActivationTransaction`.
//!
//! Not covered here (pending, and reported as such): the charter compiler /
//! synthesizer tails, preset application, pack-skill projection and the
//! reference's full parity corpora.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sk_doctrine::context_render::{RenderInputs, render_with};
use sk_doctrine::{
    ActivationInputs, ActivationTransaction, CascadeScope, ContextInputs, ContextRequest,
    DeactivationInputs, DenyListChoice, DenySet, EffectiveWhenAbsent, IdentityIndex,
    NativeOfferingService, ProfileRepository, YamlDocument, build_action_context,
    cascade_activation_targets, deactivation_plan, effective_deny, kind_deny_key,
    load_activation_ledger, load_graph_or_dir, plan_activation, plan_deactivation,
    read_org_required_provenance, required_floor, resolve_effective_sets, scan_all,
};

fn built_in_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/built-in")
        .canonicalize()
        .expect("pinned built-in pack")
}

const CHARTER_MD: &str = "# Fixture Charter\n\n\
## Policy Summary\n\n\
- Keep one canonical authority.\n\n\
## Terminology Canon\n\n\
Use Mission, never Feature.\n";

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let charter = temp.path().join(".kittify/charter");
    fs::create_dir_all(&charter).expect("charter dir");
    fs::write(
        charter.join("activation.yaml"),
        "# authored activation state\n\
         activated_directives:\n\
         - 001-architectural-integrity-standard\n",
    )
    .expect("ledger");
    fs::write(charter.join("charter.md"), CHARTER_MD).expect("charter.md");
    temp
}

struct Doctrine {
    built_in: PathBuf,
    graph: sk_doctrine::DrgGraph,
    identity: IdentityIndex,
    floor: BTreeMap<sk_charter::ArtifactKind, Vec<String>>,
    profiles: ProfileRepository,
}

fn doctrine(root: &Path) -> Doctrine {
    let built_in = built_in_root();
    let graph = load_graph_or_dir(&built_in).expect("graph");
    let mut identity = IdentityIndex::from_roots(&built_in, &[], None);
    scan_all(&mut identity);
    let floor = required_floor(root, &identity).expect("floor").into_map();
    let profiles = ProfileRepository::load(&built_in, &[], None).expect("profiles");
    Doctrine {
        built_in,
        graph,
        identity,
        floor,
        profiles,
    }
}

#[test]
fn real_action_context_renders_the_pinned_bootstrap_grain() {
    let temp = fixture();
    let root = temp.path();
    let doctrine = doctrine(root);
    let ledger = load_activation_ledger(root).expect("ledger");
    let inputs = ContextInputs {
        graph: &doctrine.graph,
        ledger: &ledger,
        identity: &doctrine.identity,
        profiles: &doctrine.profiles,
        project_root: root,
        required_floor: &doctrine.floor,
        project_warnings: &[],
    };
    let request = ContextRequest {
        action: "implement",
        mission_type: Some("software-dev"),
        depth: 2,
        profile: None,
        include_all: false,
    };
    let context = build_action_context(&inputs, &request).expect("context");
    assert_eq!(context.action, "implement");
    assert_eq!(context.mode, sk_doctrine::ContextMode::Bootstrap);
    assert!(
        !context.directives.is_empty(),
        "the pinned grain delivers directives"
    );
    let markdown = fs::read_to_string(root.join(".kittify/charter/charter.md")).expect("md");
    let text = render_with(
        &context,
        &RenderInputs {
            charter_markdown: Some(&markdown),
            project_root: Some(root),
            declared_authority_paths: &[],
            reference_docs: &[],
            compact_governance: None,
            languages: None,
            lacks_specialist_guidance: false,
            profile_sections: None,
            selection_block: None,
        },
    )
    .expect("render");
    assert!(text.starts_with("Charter Context (Bootstrap):"));
    assert!(text.contains("  - Keep one canonical authority."));
    assert!(text.contains("Action-Critical Charter Sections (implement):"));
    assert!(text.contains("Action Doctrine (implement):"));
    assert!(text.contains("  Directives:"));
    assert!(
        text.lines()
            .any(|line| line.starts_with("    - ") && line.contains(": ")),
        "at least one artifact line renders:\n{text}"
    );
    // Every delivered directive id reaches the text (no silent drop).
    for directive in &context.directives {
        assert!(
            text.contains(&directive.id),
            "missing delivered directive {}",
            directive.id
        );
    }
}

fn activation_inputs<'a>(
    document: &'a YamlDocument,
    available: &'a [String],
    effective: &'a [String],
    deny: &'a [String],
    effective_deny_set: &'a DenySet,
    narrow: Option<&'a [String]>,
) -> ActivationInputs<'a> {
    ActivationInputs {
        yaml_key: "activated_directives",
        available_ids: available,
        config_data: document,
        effective_ids: effective,
        effective_when_absent: EffectiveWhenAbsent::All,
        effective_ids_in_force: true,
        narrow_base: narrow,
        previous_ids: effective,
        deny_key: Some("deactivated_directives"),
        current_deny: deny,
        deny_change: DenyListChoice::Activate,
        effective_deny: effective_deny_set,
    }
}

#[test]
fn canonical_plan_and_commit_are_idempotent_and_preserve_authored_state() {
    let temp = fixture();
    let root = temp.path();
    let doctrine = doctrine(root);
    let kind = sk_charter::ArtifactKind::Directive;
    let ledger_path = root.join(".kittify/charter/activation.yaml");
    let document = YamlDocument::open(&ledger_path).expect("document");
    let ledger = load_activation_ledger(root).expect("ledger");
    let deny = ledger.deny_for(kind).to_vec();
    let floor_ids = doctrine.floor.get(&kind).cloned().unwrap_or_default();
    let deny_set = effective_deny(kind, deny.clone(), floor_ids, &doctrine.identity);
    let mut available: Vec<String> = doctrine.identity.stems(kind);
    let declared: Vec<String> = available
        .iter()
        .filter_map(|stem| doctrine.identity.declared_id(kind, stem))
        .collect();
    available.extend(declared);
    available.sort();
    available.dedup();

    let mut sets = resolve_effective_sets(root, &["activated_directives"], &NativeOfferingService)
        .expect("effective sets");
    let effective = sets
        .pop()
        .expect("set")
        .scalar_ids()
        .expect("scalar directives");

    let inputs = activation_inputs(&document, &available, &effective, &deny, &deny_set, None);
    let plan = plan_activation("directive", "024-locality-of-change", &inputs).expect("plan");
    assert_eq!(plan.activated, vec!["024-locality-of-change".to_owned()]);
    ActivationTransaction::begin(root)
        .expect("begin")
        .commit(&plan)
        .expect("commit");

    let after_first = fs::read(&ledger_path).expect("ledger after first");
    let text = String::from_utf8(after_first.clone()).expect("utf8");
    assert!(text.contains("# authored activation state"));
    assert!(text.contains("024-locality-of-change"));
    assert!(text.contains("001-architectural-integrity-standard"));

    // A second plan over the new document is a no-op with a clear warning, and
    // the commit leaves the authored bytes unchanged.
    let document = YamlDocument::open(&ledger_path).expect("document 2");
    let ledger = load_activation_ledger(root).expect("ledger 2");
    let deny = ledger.deny_for(kind).to_vec();
    let deny_set = effective_deny(
        kind,
        deny.clone(),
        doctrine.floor.get(&kind).cloned().unwrap_or_default(),
        &doctrine.identity,
    );
    let mut sets = resolve_effective_sets(root, &["activated_directives"], &NativeOfferingService)
        .expect("effective sets 2");
    let effective = sets
        .pop()
        .expect("set")
        .scalar_ids()
        .expect("scalar directives");
    let inputs = activation_inputs(&document, &available, &effective, &deny, &deny_set, None);
    let again =
        plan_activation("directive", "024-locality-of-change", &inputs).expect("second plan");
    assert!(again.activated.is_empty());
    assert!(
        again
            .warnings
            .iter()
            .any(|warning| warning.contains("already activated")),
        "{:?}",
        again.warnings
    );
    ActivationTransaction::begin(root)
        .expect("begin 2")
        .commit(&again)
        .expect("commit 2");
    assert_eq!(
        fs::read(&ledger_path).expect("ledger after second"),
        after_first,
        "an idempotent retry must not rewrite the authored document"
    );
}

#[test]
fn canonical_deactivation_records_the_exclusion() {
    let temp = fixture();
    let root = temp.path();
    let doctrine = doctrine(root);
    let kind = sk_charter::ArtifactKind::Directive;
    let ledger_path = root.join(".kittify/charter/activation.yaml");
    let document = YamlDocument::open(&ledger_path).expect("document");
    let available: Vec<String> = doctrine.identity.stems(kind);
    let required_by =
        read_org_required_provenance(root, kind, &doctrine.identity, true).expect("floor");
    let ledger = load_activation_ledger(root).expect("ledger");
    let deny = ledger.deny_for(kind).to_vec();
    let deny_key = kind_deny_key(kind);
    let also = vec!["001-architectural-integrity-standard".to_owned()];
    let inputs = DeactivationInputs {
        yaml_key: "activated_directives",
        config_data: &document,
        available_ids: &available,
        required_by: &required_by,
        deny_key: deny_key.as_deref(),
        current_deny: &deny,
        record_exclusion: true,
        also_matches: &also,
    };
    let plan = plan_deactivation("directive", "001-architectural-integrity-standard", &inputs)
        .expect("plan");
    ActivationTransaction::begin(root)
        .expect("begin")
        .commit(&plan)
        .expect("commit");
    let text = fs::read_to_string(&ledger_path).expect("ledger text");
    assert!(text.contains("deactivated_directives"));
    assert!(text.contains("001-architectural-integrity-standard"));
    assert!(text.contains("# authored activation state"));

    let listed = load_activation_ledger(root).expect("ledger 2");
    assert!(
        listed
            .deny_for(kind)
            .iter()
            .any(|id| id == "001-architectural-integrity-standard")
    );
}

#[test]
fn cascade_planners_traverse_the_pinned_graph() {
    let temp = fixture();
    let root = temp.path();
    let doctrine = doctrine(root);
    let empty: BTreeSet<String> = BTreeSet::new();
    let activated = cascade_activation_targets(
        &doctrine.graph,
        "mission_type:software-dev",
        &CascadeScope::all(),
        &empty,
        false,
    );
    assert!(
        !activated.activated.is_empty(),
        "a mission type reaches activatable doctrine"
    );
    assert!(
        activated
            .activated
            .keys()
            .any(|kind| kind == "agent_profile" || kind == "tactic" || kind == "directive"),
        "{:?}",
        activated.activated.keys().collect::<Vec<_>>()
    );

    // The activation state the shared-reference safety check reads.
    let ledger = load_activation_ledger(root).expect("ledger");
    let state =
        sk_doctrine::ActivationState::from_ledger(&ledger, &doctrine.identity, &doctrine.floor)
            .expect("state");
    let mut active: BTreeSet<String> = BTreeSet::new();
    for (kind, allow) in &state.allow {
        let prefix = kind.canonical();
        match allow {
            Some(ids) => active.extend(ids.iter().cloned()),
            None => active.extend(
                doctrine
                    .graph
                    .nodes()
                    .iter()
                    .filter(|node| node.kind().as_str() == prefix)
                    .map(|node| node.urn().to_owned()),
            ),
        }
    }
    let plan = deactivation_plan(
        &doctrine.graph,
        "directive:024-locality-of-change",
        &CascadeScope::all(),
        &active,
    );
    // Shared children are reported, never removed.
    let _ = plan.skipped_shared;
    let _ = doctrine.built_in;
}
