//! CF66 — ordered action-context phases over the ONE canonical resolution.
//!
//! The reference CLI builds `charter context` in two real stages:
//! `build_charter_context` renders the text first (the only builder that takes
//! `--profile`, and it has **no** `include_all` parameter), then
//! `build_charter_context_json` builds the structured payload second (that
//! builder has **no** profile parameter at all). This file proves the phase
//! separation those two builders imply, against the canonical merged-record
//! fixture:
//!
//! * the text stage is progressive and never serializes a record body, so a
//!   link whose unrequested `Any` member cannot be JSON-dumped still renders,
//!   and the same record under `include_all` fails only in the structured stage;
//! * the profile channel belongs to the text stage; the structured arrays do not
//!   receive it even when the request still carries the id;
//! * a repository constructor abort (the reference's `KeyError` phase) is not
//!   converted to a healthy miss in either stage — no blanket error-to-`None`.
//!
//! In-process controls over the canonical `sk-doctrine` APIs. The ordered CLI
//! execution and its Source/Native child assertions live in
//! `crates/sk-cli/tests/cf66_context_execution_cli.rs`; this file makes no CLI,
//! whole-action parity, approval or performance claim.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::path::{Path, PathBuf};

use sk_doctrine::{
    ArtifactEntry, CharterContext, ContextInputs, ContextPhase, ContextRequest, Delivery,
    DoctrineError, IdentityIndex, ProfileRepository, ProgressiveDeliveryError,
    build_action_context, build_action_context_structured, build_action_context_text,
    context_roots, load_activation_ledger, load_graph_or_dir, load_org_pack, merge_three_layers,
    required_floor,
};

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

/// A link-delivered styleguide carrying bytes the JSON-mode dump refuses.
const LAZY_STYLEGUIDE: &str = "\
schema_version: \"1.0\"
id: common-docs
title: \"CF66 lazy docs \\u00e9 title\"
scope: docs
principles:
  - overlay principle
structural_lint_config:
  bytes_value: !!binary /w==
";

/// A constructed `KeyError` (invalid `!!bool` spelling) that aborts the layered
/// repository load — the reference's per-file catch does not cover it.
const ABORTING_DIRECTIVE: &str = "\
schema_version: \"1.0\"
id: DIRECTIVE_001
title: \"CF66 aborting title\"
enforcement: !!bool not-a-bool
";

/// The project graph: one eager tactic, one `suggests`-only styleguide and one
/// profile-only tactic reached solely through the profile channel.
const PROJECT_GRAPH: &str = "\
schema_version: '1.0'
generated_at: STATIC
generated_by: cf66-test
nodes:
- urn: tactic:cf66-eager
  kind: tactic
  label: CF66 Eager
- urn: styleguide:common-docs
  kind: styleguide
  label: Common Docs
- urn: agent_profile:cf66-profile
  kind: agent_profile
  label: CF66 Profile
- urn: styleguide:cf66-profile-styleguide
  kind: styleguide
  label: CF66 Profile Styleguide
edges:
- source: action:software-dev/implement
  target: tactic:cf66-eager
  relation: requires
- source: action:software-dev/implement
  target: styleguide:common-docs
  relation: suggests
- source: agent_profile:cf66-profile
  target: styleguide:cf66-profile-styleguide
  relation: requires
";

/// A minimal project-layer profile (the canonical profile schema fields).
const PROFILE: &str = "\
profile-id: cf66-profile
name: CF66 Profile
description: CF66 phase probe profile.
schema-version: \"1.0\"
roles:
- implementer
purpose: Prove the profile channel is a text-stage input.
specialization:
  primary-focus: phase proof
";

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

fn fixture(case: &str, bad_directive: bool) -> PathBuf {
    let root = isolated::fixture(case);
    for (_, name) in EXTRA_XDG_ROOTS {
        std::fs::create_dir_all(root.join(name)).expect("extra isolated XDG directory");
    }
    write(
        &root.join(".kittify/config.yaml"),
        "# authored project config\ncustom_setting: keep-me\n",
    );
    write(
        &root.join(".kittify/charter/charter.yaml"),
        "schema_version: \"1.1.0\"\ncatalog:\n  references: []\ngovernance:\n  authority_paths: [docs/context]\n",
    );
    write(
        &root.join(".kittify/charter/charter.md"),
        "# CF66 Fixture Charter\n\n## Policy Summary\n\n- one rule\n",
    );
    write(
        &root.join(".kittify/charter-packs/styleguide/common-docs.styleguide.yaml"),
        LAZY_STYLEGUIDE,
    );
    write(
        &root.join(".kittify/charter-packs/cf66.graph.yaml"),
        PROJECT_GRAPH,
    );
    write(
        &root.join(".kittify/charter-packs/agent_profiles/cf66-profile.agent.yaml"),
        PROFILE,
    );
    if bad_directive {
        write(
            &root.join(
                ".kittify/charter-packs/directive/001-architectural-integrity-standard.directive.yaml",
            ),
            ABORTING_DIRECTIVE,
        );
    }
    let output = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&root)
        .output()
        .expect("run git init");
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    root
}

fn build_phase(
    root: &Path,
    phase: ContextPhase,
    profile: Option<&str>,
    include_all: bool,
) -> Result<CharterContext, DoctrineError> {
    let (built_in_root, org_roots, project_root) = context_roots(root).expect("context roots");
    let built_in = load_graph_or_dir(&built_in_root).expect("built-in graph");
    let mut fragments = Vec::new();
    for (index, org) in org_roots.iter().enumerate() {
        let name = org
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("org")
            .to_owned();
        fragments.push(load_org_pack(&name, org, index + 1).expect("org pack"));
    }
    let project_graph = project_root
        .as_deref()
        .filter(|layer| sk_doctrine::has_graph_files(layer))
        .map(|layer| load_graph_or_dir(layer).expect("project graph"));
    let report =
        merge_three_layers(&built_in, &fragments, project_graph.as_ref()).expect("merge layers");
    let identity = IdentityIndex::from_roots(&built_in_root, &org_roots, project_root.as_deref());
    let profiles = ProfileRepository::load(&built_in_root, &org_roots, project_root.as_deref())
        .expect("profiles");
    let ledger = load_activation_ledger(root).expect("ledger");
    let floor = required_floor(root, &identity)
        .expect("required floor")
        .into_map();
    let inputs = ContextInputs {
        graph: &report.graph,
        ledger: &ledger,
        identity: &identity,
        profiles: &profiles,
        project_root: root,
        required_floor: &floor,
        project_warnings: &[],
    };
    let mut request = ContextRequest::new("implement");
    request.mission_type = Some("software-dev");
    request.depth = 2;
    request.profile = profile;
    request.include_all = include_all;
    match phase {
        ContextPhase::Single => build_action_context(&inputs, &request),
        ContextPhase::Text => build_action_context_text(&inputs, &request),
        ContextPhase::Structured => build_action_context_structured(&inputs, &request),
    }
}

fn entry<'a>(entries: &'a [ArtifactEntry], id: &str) -> Option<&'a ArtifactEntry> {
    entries.iter().find(|entry| entry.id == id)
}

#[test]
fn text_stage_is_progressive_and_never_dumps_a_body() {
    let root = fixture("cf66-phase-bodies", false);

    // `include_all` is NOT a text-stage input: the text builder ignores it even
    // when the caller's request carries it.
    let text = build_phase(&root, ContextPhase::Text, None, true).expect("text stage");
    let lazy = entry(&text.styleguides, "common-docs").expect("link entry");
    assert_eq!(lazy.delivery, Delivery::Link);
    assert!(
        lazy.body.is_none(),
        "the text stage must not serialize a record body"
    );
    assert!(
        text.all_directives.is_empty(),
        "the reference text build never assembles all_directives"
    );

    // The structured stage without include_all is the same progressive link and
    // still dumps no body.
    let plain = build_phase(&root, ContextPhase::Structured, None, false).expect("structured");
    let plain_lazy = entry(&plain.styleguides, "common-docs").expect("link entry");
    assert_eq!(plain_lazy.delivery, Delivery::Link);
    assert!(plain_lazy.body.is_none());

    // Under include_all the structured stage inlines the record and must dump
    // its body: the reference's real UnicodeDecodeError phase.
    match build_phase(&root, ContextPhase::Structured, None, true) {
        Err(DoctrineError::ProgressiveDelivery { source }) => match *source {
            ProgressiveDeliveryError::Payload {
                cause: sk_doctrine::PayloadFailure::BytesNotUtf8(_),
                ..
            } => {}
            other => panic!("unexpected structured phase: {other:?}"),
        },
        Ok(_) => panic!("include_all must serialize the invalid body"),
        Err(other) => panic!("unexpected error: {other:?}"),
    }

    // The legacy single-build helper serializes bodies too, which is exactly the
    // ordering the ordered execution removes from the text stage.
    assert!(
        build_phase(&root, ContextPhase::Single, None, true).is_err(),
        "the single-build helper still combines both stages"
    );
}

#[test]
fn profile_channel_is_a_text_stage_input_only() {
    let root = fixture("cf66-phase-profile", false);

    let text = build_phase(&root, ContextPhase::Text, Some("cf66-profile"), false)
        .expect("text stage with profile");
    assert!(
        entry(&text.styleguides, "cf66-profile-styleguide").is_some(),
        "the profile channel delivers into the text stage"
    );
    assert!(
        !text.profile_links.is_empty(),
        "the profile links are part of the text-stage resolution"
    );

    // The structured builder takes no profile argument: a request that still
    // carries the id must NOT deliver the profile-only tactic.
    let structured = build_phase(&root, ContextPhase::Structured, Some("cf66-profile"), false)
        .expect("structured stage ignores profile");
    assert!(
        entry(&structured.styleguides, "cf66-profile-styleguide").is_none(),
        "the structured arrays must not receive the text profile channel"
    );
    assert!(structured.profile_links.is_empty());

    // The legacy combined helper keeps its old combined behaviour.
    let combined = build_phase(&root, ContextPhase::Single, Some("cf66-profile"), false)
        .expect("single build");
    assert!(entry(&combined.styleguides, "cf66-profile-styleguide").is_some());
}

#[test]
fn repository_constructor_abort_escalates_in_both_stages() {
    let root = fixture("cf66-phase-abort", true);

    for phase in [ContextPhase::Text, ContextPhase::Structured] {
        match build_phase(&root, phase, None, false) {
            Err(DoctrineError::ProgressiveDelivery { source }) => match *source {
                ProgressiveDeliveryError::Repository {
                    aborted: Some((python, message)),
                    ..
                } => {
                    assert_eq!(python, "KeyError", "{phase:?} preserves the abort class");
                    assert!(
                        message.contains("not-a-bool"),
                        "{phase:?} preserves the cause: {message}"
                    );
                }
                other => panic!("{phase:?}: unexpected phase {other:?}"),
            },
            Ok(_) => panic!("{phase:?}: a constructor abort is not a healthy miss"),
            Err(other) => panic!("{phase:?}: unexpected error {other:?}"),
        }
    }
}
