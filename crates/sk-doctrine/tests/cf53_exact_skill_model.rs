//! CF53 — the exact pack-skill model/repository and the raw org-policy consumer
//! (Doctrine side).
//!
//! Source: `src/charter/offering/pack_skills/{models,validation,repository}.py`,
//! `src/charter/activation/org_pack_discovery.py`.
//!
//! What the pinned reference actually does (observed with the lock-exact oracle,
//! see the CF53 report):
//!
//! * a lone surrogate in a **constrained** `str` field (`title`, `description`,
//!   `SkillParameter.name`, `SkillExpansion.target` — `Field(min_length=1)`)
//!   makes pydantic refuse the record, so it is skipped with a warning;
//! * a lone surrogate in an **unconstrained** `str` field (`version`,
//!   `parameters[].default`, `parameters[].description`, `triggers`, `tools`,
//!   `maintainers`, `invocation.side_effects`, `expands_to.args`) is admitted and
//!   served: the record hashes and renders, and only the physical
//!   `write_text(..., encoding="utf-8")` raises `UnicodeEncodeError`;
//! * `read_org_skill_namespace` validates **every** value it encounters while
//!   folding, so an invalid namespace in an earlier pack is refused even when a
//!   later pack overrides it;
//! * the absent-`activated_skills` default is the **coercing** reader
//!   (`read_org_required_ids`), not the strict per-field floor read.
//!
//! The installer-side phases (preparation, `source_hash`, rendering, the write
//! refusal, and the paired Source/Native run) live in
//! `crates/sk-install/tests/cf53_pack_skill_preparation.rs`, because only that
//! crate may depend on the installer.

use std::path::{Path, PathBuf};

use sk_charter::{ArtifactKind, PackTier};
use sk_doctrine::context_include::{
    IncludeAccess, IncludeInputs, parse_include_selector, render_include_exact,
};
use sk_doctrine::{PackSkillRepository, canonical_artifact_body_exact};

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, text).expect("write");
}

/// An unconstrained-surrogate record plus a plain sibling and a constrained-field
/// record the reference skips.
const SURROGATE_SKILL: &str = "schema_version: '1.0'\nid: exact-demo\ntitle: Exact demo\ndescription: Ships   the  release.\nform: prompt\nbody_path: exact-demo.md\ntriggers: [go]\nparameters:\n  - name: scope\n    description: Which issues\n    default: \"14d\\ud800\"\nversion: \"1.0.\\ud800\"\nmaintainers: [\"m\\ud800\"]\n";

const PLAIN_SKILL: &str = "schema_version: '1.0'\nid: plain-demo\ntitle: \"T\\u00e9\"\ndescription: \"Plain\\u00a0description\"\nform: prompt\nbody_path: plain-demo.md\ntriggers: [go]\n";

const SKIPPED_SKILL: &str = "schema_version: '1.0'\nid: skipped-demo\ntitle: \"T\\ud800\"\ndescription: D\nform: prompt\nbody_path: skipped-demo.md\n";

fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("sk-cf53-{name}-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clean fixture");
    }
    let built_in = root.join("built_in/skills");
    let org = root.join("org/skills");
    std::fs::create_dir_all(&built_in).expect("built-in dir");
    std::fs::create_dir_all(&org).expect("org dir");
    write(
        &root.join("built_in/pack.yaml"),
        "name: built-in\npack_level: built-in\n",
    );
    write(&org.join("exact-demo.skill.yaml"), SURROGATE_SKILL);
    write(&org.join("exact-demo.md"), "Do the exact thing.\n");
    write(&org.join("plain-demo.skill.yaml"), PLAIN_SKILL);
    write(&org.join("plain-demo.md"), "Do the plain thing.\n");
    write(&org.join("skipped-demo.skill.yaml"), SKIPPED_SKILL);
    write(&org.join("skipped-demo.md"), "Never loads.\n");
    (root, built_in, org)
}

#[test]
fn the_repository_admits_unconstrained_surrogates_and_skips_constrained_ones() {
    let (_root, built_in, org) = fixture("admit");
    let repository =
        PackSkillRepository::load(&built_in, std::slice::from_ref(&org), None, Some("acme"))
            .expect("loads");
    let mut ids = repository.ids();
    ids.sort();
    assert_eq!(ids, vec!["exact-demo", "plain-demo"]);
    // The constrained-field record is the reference's skip (pydantic
    // `string_unicode`), never a Native-only refusal.
    assert!(repository.skill("skipped-demo").is_none());
    assert!(
        repository
            .warnings()
            .join("\n")
            .contains("skipped-demo.skill.yaml"),
        "{:?}",
        repository.warnings()
    );

    let exact = repository.skill("exact-demo").expect("served");
    // Exact code points, not a scalar projection or a marker spelling.
    assert_eq!(scalar(&exact.id), "exact-demo");
    assert_eq!(
        exact.version.codepoints(),
        &[0x31, 0x2e, 0x30, 0x2e, 0xd800]
    );
    assert_eq!(
        exact.parameters[0]
            .default
            .as_ref()
            .expect("default")
            .codepoints(),
        &[0x31, 0x34, 0x64, 0xd800]
    );
    assert_eq!(exact.maintainers[0].codepoints(), &[0x6d, 0xd800]);
    assert_eq!(repository.provenance_of("exact-demo"), Some("org"));
    assert_eq!(repository.tier_of("exact-demo"), Some(PackTier::Org));
    assert_eq!(
        repository.body_text("exact-demo").as_deref(),
        Some("Do the exact thing.\n")
    );

    // The one serializer escapes the exact code unit the way Python
    // `json.dumps(ensure_ascii=True)` does (`_source_hash`'s text phase).
    let dump = exact.model_dump_json();
    let rendered = dump
        .render_python(None, true)
        .to_scalar_string()
        .expect("ascii");
    assert!(
        rendered.contains("\"version\": \"1.0.\\ud800\""),
        "{rendered}"
    );
    // ... and the mapping carrier round-trips through the canonical graph: the
    // record rebuilt from the carrier's markers is the same record, so a public
    // field cannot diverge from the mapping the include payload is rendered from.
    let (mapping, markers) = exact.exact_mapping().expect("mapping");
    let graph = sk_doctrine::exact_mapping_graph(&mapping, &markers).expect("graph");
    let again =
        canonical_artifact_body_exact(ArtifactKind::Skill, graph.root_view()).expect("payload");
    let rebuilt = sk_doctrine::PackSkill::from_exact_dump(&again).expect("record");
    assert_eq!(rebuilt.model_dump_json(), dump);
    assert_eq!(rebuilt.version, exact.version);
    assert_eq!(rebuilt.parameters[0].default, exact.parameters[0].default);
}

#[test]
fn an_overlay_shadow_keeps_the_built_in_markers_and_refuses_a_constrained_surrogate() {
    let (root, built_in, org) = fixture("overlay");
    // A project overlay shadows the built-in `base` skill; the built-in record
    // itself carries an unconstrained surrogate, so the *union* of the two layers'
    // markers is what makes the merged record exact.
    write(
        &built_in.join("base.skill.yaml"),
        "schema_version: '1.0'\nid: base\ntitle: Base\ndescription: Base.\nform: prompt\nbody_path: base.md\nversion: \"2.0.\\ud800\"\n",
    );
    write(&built_in.join("base.md"), "built-in body\n");
    let project = root.join("project/skills");
    write(&project.join("base.md"), "built-in body\n");
    write(
        &project.join("base.skill.yaml"),
        "schema_version: '1.0'\nid: base\ntitle: Overlaid\ndescription: Overlaid.\nform: prompt\nbody_path: base.md\n",
    );
    let repository = PackSkillRepository::load(
        &built_in,
        std::slice::from_ref(&org),
        Some(&project),
        Some("acme"),
    )
    .expect("loads");
    let folded = repository.skill("base").expect("folded overlay");
    assert_eq!(scalar(&folded.title), "Overlaid");
    // The inherited (built-in) unconstrained field keeps its exact code unit.
    assert_eq!(
        folded.version.codepoints(),
        &[0x32, 0x2e, 0x30, 0x2e, 0xd800]
    );
    // The include payload resolves it through the union of both layers' markers.
    let (mapping, markers) = folded.exact_mapping().expect("mapping");
    let graph = sk_doctrine::exact_mapping_graph(&mapping, &markers).expect("graph");
    let payload =
        canonical_artifact_body_exact(ArtifactKind::Skill, graph.root_view()).expect("payload");
    let rebuilt = sk_doctrine::PackSkill::from_exact_dump(&payload).expect("record");
    assert_eq!(rebuilt.version, folded.version);
    assert_eq!(rebuilt.title, folded.title);

    // A constrained-field surrogate in the *overlay* is the reference's skip.
    write(
        &project.join("base.skill.yaml"),
        "schema_version: '1.0'\nid: base\ntitle: \"Over\\ud800\"\ndescription: Overlaid.\nform: prompt\nbody_path: base.md\n",
    );
    let repository = PackSkillRepository::load(
        &built_in,
        std::slice::from_ref(&org),
        Some(&project),
        Some("acme"),
    )
    .expect("loads");
    let folded = repository.skill("base").expect("built-in record survives");
    assert_eq!(scalar(&folded.title), "Base");
    assert!(
        repository.warnings().join("\n").contains("base.skill.yaml"),
        "{:?}",
        repository.warnings()
    );
}

#[test]
fn the_include_payload_of_a_served_skill_keeps_the_exact_code_points() {
    let (root, _built_in, _org) = fixture("include");
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: org\n        local_path: org\n",
    );
    let access = IncludeAccess::load(&root).expect("include authorities");
    let inputs = IncludeInputs {
        project_root: &root,
        access: &access,
    };
    let selector = parse_include_selector("skill:exact-demo").expect("selector");
    let rendered =
        render_include_exact(&inputs, &selector, "skill:exact-demo").expect("selector renders");
    let text = rendered.to_scalar_string().expect("scalar text");
    // The exact payload prints the surrogate as an escaped code unit — the text
    // Python's `json.dumps(ensure_ascii=True)` produces — never a transport
    // marker and never a replacement character.
    assert!(text.contains("1.0.\\ud800"), "{text}");
    assert!(!text.contains("transport-marker"), "{text}");
    assert!(!text.contains('\u{fffd}'), "{text}");
    assert!(text.contains("Full artifact:"), "{text}");
}

#[test]
fn the_namespace_fold_validates_every_encountered_value_before_a_later_override() {
    let root = std::env::temp_dir().join(format!("sk-cf53-namespace-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clean");
    }
    write(
        &root.join("packs/built-in/pack.yaml"),
        "name: built-in\npack_level: built-in\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: bad\n        local_path: packs/bad\n      - name: good\n        local_path: packs/good\n",
    );
    write(
        &root.join("packs/bad/org-charter.yaml"),
        "schema_version: 1\nskill_namespace: Bad_Namespace\n",
    );
    write(
        &root.join("packs/good/org-charter.yaml"),
        "schema_version: 1\nskill_namespace: good\n",
    );
    // The reference refuses the *first* invalid value even though `good` would
    // override it, and names the pack it was reading.
    let error = sk_doctrine::read_org_skill_namespace(&root).expect_err("invalid namespace");
    let message = error.to_string();
    assert!(message.contains("Bad_Namespace"), "{message}");
    assert!(message.contains("org pack 'bad'"), "{message}");
    assert!(message.contains("fix `skill_namespace`"), "{message}");
    assert!(matches!(
        error,
        sk_doctrine::DoctrineError::OrgSkillNamespaceInvalid { .. }
    ));

    // An invalid *later* value names its own pack.
    write(
        &root.join("packs/bad/org-charter.yaml"),
        "schema_version: 1\nskill_namespace: bad\n",
    );
    write(
        &root.join("packs/good/org-charter.yaml"),
        "schema_version: 1\nskill_namespace: good_2\n",
    );
    let error = sk_doctrine::read_org_skill_namespace(&root).expect_err("invalid namespace");
    assert!(error.to_string().contains("org pack 'good'"), "{error}");

    // ... and a valid fold still answers the last non-empty value.
    write(
        &root.join("packs/good/org-charter.yaml"),
        "schema_version: 1\nskill_namespace: good\n",
    );
    assert_eq!(
        sk_doctrine::read_org_skill_namespace(&root).expect("namespace"),
        Some("good".to_owned())
    );
}

#[test]
fn the_consumer_reads_the_coercing_org_required_ids_as_the_absent_key_default() {
    let root = std::env::temp_dir().join(format!("sk-cf53-floor-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clean");
    }
    write(
        &root.join("packs/built-in/pack.yaml"),
        "name: built-in\npack_level: built-in\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: org\n        local_path: packs/org\n",
    );
    write(
        &root.join("packs/org/org-charter.yaml"),
        "schema_version: 1\nskill_namespace: acme\nrequired_skills: [1, '  plain-demo  ', true, null]\n",
    );
    // `_absent_key_default` is the **coercing** reader: `[1, ' x ', true, null]`
    // yields `1`, `x`, `True`, `None`. The strict per-field floor read answers
    // `[]` for exactly this list, which would retire every installed skill.
    let coerced = sk_doctrine::read_org_required_ids(&root, ArtifactKind::Skill).expect("ids");
    assert_eq!(coerced, vec!["1", "plain-demo", "True", "None"]);
    let strict =
        sk_doctrine::read_org_required_tokens(&root, ArtifactKind::Skill, false).expect("floor");
    assert!(strict.is_empty(), "{strict:?}");
    assert_eq!(
        sk_doctrine::read_org_skill_namespace(&root).expect("namespace"),
        Some("acme".to_owned())
    );
}

fn scalar(value: &sk_kernel::PythonString) -> String {
    value.to_scalar_string().expect("scalar")
}
