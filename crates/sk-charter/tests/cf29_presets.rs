//! CF29 — the activation-preset offering module: strict loader, discovery,
//! manifest hashing and the project's offering packs.
//!
//! Fixtures are fresh `tempfile` directories. Nothing here writes to a project.

use std::fs;
use std::path::Path;

use sk_charter::ArtifactKind;
use sk_charter::presets::{
    ACTIVATED_KINDS_KEY, ActivationPreset, DEFAULT_PRESET_NAME, MISSION_TYPE_ACTIVATIONS_KEY,
    NAME_MAX_LENGTH, OfferingPack, PresetFormatError, PresetLoadError, discover_presets,
    enumerate_presets, governed_kinds, kind_gate_omissions_is_empty, list_offering_packs,
    load_preset, load_preset_file, pack_presets_dir, preset_activation_key, preset_activation_keys,
    preset_files,
};

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, text).expect("fixture file");
}

fn pack_with_preset(name: &str, body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("fixture root");
    let pack = directory.path().join("pack");
    write(&pack.join("presets").join(format!("{name}.yaml")), body);
    (directory, pack)
}

fn format_error(path: &Path) -> PresetFormatError {
    load_preset_file(path).expect_err("the loader refuses this file")
}

#[test]
fn governed_kinds_are_derived_from_the_taxonomy() {
    let kinds = governed_kinds();
    assert!(kinds.contains(&ArtifactKind::Directive));
    assert!(kinds.contains(&ArtifactKind::Tactic));
    assert!(kinds.contains(&ArtifactKind::AgentProfile));
    assert!(
        kinds.contains(&ArtifactKind::AntiPattern),
        "an anti-pattern is charter-activatable"
    );
    assert!(
        !kinds.contains(&ArtifactKind::Skill),
        "a skill keeps its own absence contract"
    );
    assert!(
        !kinds.contains(&ArtifactKind::GlossaryPack),
        "a glossary pack keeps its own absence contract"
    );
    assert!(!kinds.contains(&ArtifactKind::Template));
    assert!(!kinds.contains(&ArtifactKind::Asset));
    assert_eq!(
        preset_activation_key(ArtifactKind::AgentProfile),
        "activated_agent_profiles"
    );
    assert_eq!(
        preset_activation_keys().len(),
        governed_kinds().len(),
        "one key per governed kind"
    );
    assert_eq!(preset_activation_keys()[0], "activated_directives");
}

#[test]
fn a_full_preset_loads_with_present_keys_only() {
    let (_directory, pack) = pack_with_preset(
        "curated",
        "name: curated\ndescription: A curated baseline.\nactivated_directives:\n  - 001-arch\n  - DIRECTIVE_002\nactivated_tactics: []\nactivated_kinds:\n  - directives\n  - tactics\nmission_type_activations:\n  - software-dev\n",
    );
    let preset = load_preset(&pack, "curated").expect("the preset loads");
    assert_eq!(preset.name, "curated");
    assert_eq!(preset.description, "A curated baseline.");
    assert_eq!(
        preset.activations.get("activated_directives"),
        Some(&vec!["001-arch".to_owned(), "DIRECTIVE_002".to_owned()])
    );
    assert_eq!(
        preset.activations.get("activated_tactics"),
        Some(&Vec::<String>::new()),
        "an empty list is present and activates nothing"
    );
    assert!(
        !preset.activations.contains_key("activated_styleguides"),
        "an absent key is absent, not empty"
    );
    assert_eq!(
        preset.listed_kinds(),
        vec![ArtifactKind::Directive, ArtifactKind::Tactic]
    );
    assert_eq!(
        preset.activated_kinds,
        Some(vec!["directives".to_owned(), "tactics".to_owned()])
    );
    assert!(preset.kind_gate_omissions().is_empty());
    assert!(kind_gate_omissions_is_empty(&preset));
    assert_eq!(
        preset.mission_type_activations,
        Some(vec!["software-dev".to_owned()])
    );
    assert_eq!(preset.source, pack_presets_dir(&pack).join("curated.yaml"));
}

#[test]
fn a_kind_gate_that_omits_a_listed_kind_is_reported() {
    let (_directory, pack) = pack_with_preset(
        "narrow",
        "name: narrow\ndescription: Narrow.\nactivated_directives:\n  - 001-arch\nactivated_kinds:\n  - tactics\n",
    );
    let preset = load_preset(&pack, "narrow").expect("the preset loads");
    assert_eq!(preset.kind_gate_omissions(), vec!["directives".to_owned()]);
    assert!(!kind_gate_omissions_is_empty(&preset));
}

#[test]
fn the_built_in_presets_load_through_the_real_pack() {
    let built_in = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/built-in")
        .canonicalize()
        .expect("the fetched built-in pack");
    let default = load_preset(&built_in, DEFAULT_PRESET_NAME).expect("the default preset");
    assert!(default.activations.is_empty(), "the default lists no ids");
    assert_eq!(default.activated_kinds, None);
    assert!(default.mission_type_activations.is_some());
    let minimal = load_preset(&built_in, "minimal").expect("the minimal preset");
    assert!(
        minimal.activations.contains_key("activated_directives"),
        "minimal curates directives"
    );
    let discovered: Vec<String> = discover_presets(&built_in)
        .expect("both presets load")
        .into_iter()
        .map(|preset| preset.name)
        .collect();
    assert_eq!(discovered, vec!["default".to_owned(), "minimal".to_owned()]);
}

#[test]
fn the_loader_refuses_every_malformed_shape_by_field() {
    let (_directory, pack) = pack_with_preset(
        "bad-key",
        "name: bad-key\ndescription: Bad.\nactivated_skill: []\n",
    );
    let error = format_error(&pack.join("presets/bad-key.yaml"));
    assert_eq!(error.field, "activated_skill");

    let (_directory, pack) = pack_with_preset(
        "skills",
        "name: skills\ndescription: Bad.\nactivated_skills: []\n",
    );
    let error = format_error(&pack.join("presets/skills.yaml"));
    assert_eq!(error.field, "activated_skills");
    assert!(
        error.detail.contains("keeps its own activation contract"),
        "{error}"
    );

    let (_directory, pack) = pack_with_preset(
        "scoped",
        "name: scoped\ndescription: Bad.\nactivations:\n  - kind: directive\n",
    );
    let error = format_error(&pack.join("presets/scoped.yaml"));
    assert_eq!(error.field, "activations");
    assert!(error.detail.contains("context-scoped"), "{error}");

    let (_directory, pack) = pack_with_preset("mismatch", "name: other\ndescription: Fine.\n");
    let error = format_error(&pack.join("presets/mismatch.yaml"));
    assert_eq!(error.field, "name");
    assert!(error.detail.contains("file stem"), "{error}");

    let (_directory, pack) = pack_with_preset("Upper", "name: Upper\ndescription: Fine.\n");
    let error = format_error(&pack.join("presets/Upper.yaml"));
    assert_eq!(error.field, "name");

    let long = "a".repeat(NAME_MAX_LENGTH + 1);
    let (directory, pack) = pack_with_preset(&long, &format!("name: {long}\ndescription: Fine.\n"));
    let error = format_error(&pack.join("presets").join(format!("{long}.yaml")));
    assert_eq!(error.field, "name");
    assert!(error.detail.contains("longer than"), "{error}");
    drop(directory);

    let (_directory, pack) = pack_with_preset("destitute", "name: destitute\ndescription: '   '\n");
    let error = format_error(&pack.join("presets/destitute.yaml"));
    assert_eq!(error.field, "description");

    let (_directory, pack) = pack_with_preset("nameless", "description: Fine.\n");
    let error = format_error(&pack.join("presets/nameless.yaml"));
    assert_eq!(error.field, "name");
    assert!(error.detail.contains("required"), "{error}");

    let (_directory, pack) = pack_with_preset(
        "duplicated",
        "name: duplicated\ndescription: Fine.\nactivated_directives:\n  - 001-arch\n  - 001-arch\n",
    );
    let error = format_error(&pack.join("presets/duplicated.yaml"));
    assert_eq!(error.field, "activated_directives");
    assert!(error.detail.contains("duplicate"), "{error}");

    let (_directory, pack) = pack_with_preset(
        "scalar",
        "name: scalar\ndescription: Fine.\nactivated_directives: 001-arch\n",
    );
    let error = format_error(&pack.join("presets/scalar.yaml"));
    assert_eq!(error.field, "activated_directives");
    assert!(error.detail.contains("list of strings"), "{error}");

    let (_directory, pack) = pack_with_preset(
        "blank-entry",
        "name: blank-entry\ndescription: Fine.\nmission_type_activations:\n  - '  '\n",
    );
    let error = format_error(&pack.join("presets/blank-entry.yaml"));
    assert_eq!(error.field, MISSION_TYPE_ACTIVATIONS_KEY);

    let (_directory, pack) = pack_with_preset(
        "unknown-gate",
        "name: unknown-gate\ndescription: Fine\nactivated_kinds:\n  - wizards\n",
    );
    let error = format_error(&pack.join("presets/unknown-gate.yaml"));
    assert_eq!(error.field, ACTIVATED_KINDS_KEY);
    assert!(error.detail.contains("unknown kind"), "{error}");

    let (_directory, pack) = pack_with_preset("empty", "# only a comment\n");
    let error = format_error(&pack.join("presets/empty.yaml"));
    assert_eq!(error.field, "<file>");
    assert!(error.detail.contains("expected a YAML mapping"), "{error}");

    let (_directory, pack) = pack_with_preset("listy", "- one\n- two\n");
    let error = format_error(&pack.join("presets/listy.yaml"));
    assert_eq!(error.field, "<file>");
}

#[test]
fn an_unknown_preset_names_the_available_ones() {
    let (_directory, pack) = pack_with_preset("alpha", "name: alpha\ndescription: First.\n");
    write(
        &pack_presets_dir(&pack).join("beta.yaml"),
        "name: beta\ndescription: Second.\n",
    );
    write(&pack_presets_dir(&pack).join("README.md"), "not a preset\n");
    fs::create_dir_all(pack_presets_dir(&pack).join("nested.yaml"))
        .expect("a directory, not a file");

    let files = preset_files(&pack).expect("the preset files");
    assert_eq!(
        files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["alpha.yaml".to_owned(), "beta.yaml".to_owned()],
        "only regular *.yaml files, sorted by stem"
    );

    let error = load_preset(&pack, "gamma").expect_err("no such preset");
    match error {
        PresetLoadError::NotFound(error) => {
            assert_eq!(error.name, "gamma");
            assert_eq!(error.available, vec!["alpha".to_owned(), "beta".to_owned()]);
        }
        other => panic!("expected a not-found error, got {other:?}"),
    }

    let error = load_preset(&pack, "Not A Name").expect_err("outside the grammar");
    assert!(matches!(error, PresetLoadError::NotFound(_)));

    let preset = load_preset(&pack, "beta").expect("the sibling loads");
    assert_eq!(preset.name, "beta");
}

#[test]
fn a_pack_without_a_presets_directory_has_none() {
    let directory = tempfile::tempdir().expect("fixture root");
    let pack = directory.path().join("bare");
    fs::create_dir_all(&pack).expect("pack root");
    assert!(preset_files(&pack).expect("no presets").is_empty());
    assert!(discover_presets(&pack).expect("no presets").is_empty());
    assert!(enumerate_presets(&pack).expect("no entries").is_empty());
    assert!(matches!(
        load_preset(&pack, "default").expect_err("none available"),
        PresetLoadError::NotFound(_)
    ));
}

#[test]
fn enumerate_presets_hashes_lf_normalised_bytes_pack_relative() {
    let directory = tempfile::tempdir().expect("fixture root");
    let pack = directory.path().join("pack");
    write(
        &pack.join("presets/lf.yaml"),
        "name: lf\ndescription: LF.\n",
    );
    write(
        &pack.join("presets/crlf.yaml"),
        "name: crlf\r\ndescription: CRLF.\r\n",
    );
    let entries = enumerate_presets(&pack).expect("entries");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>(),
        vec!["crlf".to_owned(), "lf".to_owned()],
        "sorted by name"
    );
    assert_eq!(entries[0].path, "presets/crlf.yaml");
    assert_eq!(entries[1].path, "presets/lf.yaml");
    for entry in &entries {
        assert_eq!(entry.content_hash.len(), 64, "sha256 hex");
        assert!(entry.content_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
    assert_ne!(
        entries[0].content_hash, entries[1].content_hash,
        "different content, different digest"
    );
    // The same bytes with CRLF and with LF must hash identically.
    let crlf = again_value(&pack, "crlf");
    write(
        &pack.join("presets/crlf.yaml"),
        "name: crlf\ndescription: CRLF.\n",
    );
    let lf = again_value(&pack, "crlf");
    assert_eq!(crlf, lf, "line endings are normalised before hashing");
}

fn again_value(pack: &Path, name: &str) -> String {
    enumerate_presets(pack)
        .expect("entries")
        .into_iter()
        .find(|entry| entry.name == name)
        .expect("the entry")
        .content_hash
}

#[test]
fn the_offering_lists_built_in_org_then_project() {
    let directory = tempfile::tempdir().expect("fixture root");
    let project = directory.path();
    write(
        &project.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: acme-pack\n",
    );
    fs::create_dir_all(project.join("acme-pack")).expect("org pack root");
    write(
        &project.join("acme-pack/presets/starter.yaml"),
        "name: starter\ndescription: Org starter.\n",
    );
    let packs: Vec<OfferingPack> =
        list_offering_packs(project, &sk_charter::Limits::default()).expect("the offering");
    let names: Vec<&str> = packs.iter().map(|pack| pack.name.as_str()).collect();
    assert_eq!(names, vec!["built-in", "acme", "project"]);
    assert!(packs[0].ships_presets());
    assert_eq!(
        packs[1].root,
        project.join("acme-pack").canonicalize().expect("canonical")
    );
    assert!(packs[1].ships_presets());
    assert!(
        !packs[2].ships_presets(),
        "the project pack ships no presets"
    );
    assert_eq!(
        packs[2].root,
        project.join(".kittify/charter-packs"),
        "the project pack root is spelled even when absent"
    );
    let org: Vec<ActivationPreset> = discover_presets(&packs[1].root).expect("org presets");
    assert_eq!(org.len(), 1);
    assert_eq!(org[0].name, "starter");
}
