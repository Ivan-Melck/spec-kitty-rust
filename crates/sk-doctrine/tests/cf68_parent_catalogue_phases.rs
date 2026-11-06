use sk_doctrine::{IdentityIndex, load_reference_catalog};
use sk_kernel::PythonString;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn identity() -> IdentityIndex {
    IdentityIndex::from_roots(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packs/built-in"),
        &[],
        None,
    )
}
fn put(root: &Path, text: &[u8]) -> PathBuf {
    let path = root.join(".kittify/charter/charter.yaml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}
const BASE: &str = "catalog:\n  references:\n    - id: x\n      kind: directive\n";

#[test]
fn exact_catalogue_fields_keep_all_surrogates_nul_and_noncharacters() {
    let root = tempfile::tempdir().unwrap();
    let mut quoted = String::new();
    for code in 0xd800..=0xdfff {
        quoted.push_str(&format!("\\u{code:04x}"));
    }
    quoted.push_str("\\0\\ue000\\ufdd0\\U0010ffff");
    put(root.path(), format!("catalog:\n  references:\n    - id: \"directive:x\\ud800\"\n      kind: directive\n      title: \"{quoted}\"\n").as_bytes());
    let refs = load_reference_catalog(root.path(), &identity()).unwrap();
    assert_eq!(
        refs.len(),
        1,
        "valid exact text must not become an empty catalogue"
    );
    let mut expected: Vec<u32> = (0xd800..=0xdfff).collect();
    expected.extend([0, 0xe000, 0xfdd0, 0x10ffff]);
    assert_eq!(refs[0].title.codepoints(), expected);
    assert_eq!(
        refs[0].id,
        PythonString::from_codepoints(&[
            100, 105, 114, 101, 99, 116, 105, 118, 101, 58, 120, 0xd800
        ])
        .unwrap()
    );
}

#[test]
fn catalogue_uses_python_str_for_nonstring_fields() {
    let root = tempfile::tempdir().unwrap();
    put(root.path(), b"catalog:\n  references:\n    - id: true\n      title: 3.25\n      local_path: null\n      kind: .nan\n      summary: !!binary YQ==\n");
    let refs = load_reference_catalog(root.path(), &identity()).unwrap();
    assert_eq!(refs.len(), 1);
    let actual = [
        &refs[0].id,
        &refs[0].title,
        &refs[0].local_path,
        &refs[0].kind,
        &refs[0].summary,
    ]
    .map(|s| s.to_scalar_string().unwrap());
    assert_eq!(actual, ["True", "3.25", "None", "nan", "b'a'"]);
}

#[test]
fn constructor_runtime_error_is_not_an_empty_or_successful_catalogue() {
    let root = tempfile::tempdir().unwrap();
    put(
        root.path(),
        format!("bad: !!timestamp 2024-02-30\n{BASE}").as_bytes(),
    );
    let result = load_reference_catalog(root.path(), &identity());
    assert!(
        matches!(result, Err(sk_doctrine::ReferenceCatalogError::Construct { source, .. }) if matches!(*source, sk_kernel::python_yaml::YamlError::Runtime { cause: sk_kernel::python_yaml::RuntimeCause::Value, .. })),
        "Source ValueError escapes with its typed cause"
    );
}

#[test]
fn unrelated_cycle_does_not_poison_the_requested_catalogue() {
    let root = tempfile::tempdir().unwrap();
    put(
        root.path(),
        format!("unrelated: &x [*x]\n{BASE}").as_bytes(),
    );
    let refs = load_reference_catalog(root.path(), &identity()).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].id.to_scalar_string().unwrap(), "x");
}

#[test]
fn caught_utf8_utf16_and_duplicate_documents_are_empty() {
    let root = tempfile::tempdir().unwrap();
    for bytes in [
        vec![0xff],
        [
            vec![0xff, 0xfe],
            BASE.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ]
        .concat(),
        format!("catalog: {{}}\n{BASE}").into_bytes(),
    ] {
        put(root.path(), &bytes);
        assert!(
            load_reference_catalog(root.path(), &identity())
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg(unix)]
#[test]
fn unignored_exists_error_is_not_a_healthy_empty_catalogue() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), BASE.as_bytes());
    let directory = path.parent().unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o0)).unwrap();
    let outcome = load_reference_catalog(root.path(), &identity());
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        outcome.is_err(),
        "Path.exists PermissionError is outside the Source read catch"
    );
}

#[cfg(unix)]
#[test]
fn presence_retains_unignored_stat_error() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let path = put(root.path(), BASE.as_bytes());
    let directory = path.parent().unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o0)).unwrap();
    let outcome = sk_doctrine::charter_present(root.path());
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        matches!(outcome, Err(sk_doctrine::DoctrineError::Io { source, .. }) if source.raw_os_error() == Some(13))
    );
}

#[test]
fn presence_is_lazy_on_undeclared_but_checked_on_grain_rails() {
    use sk_doctrine::{
        ContextInputs, ContextRequest, DrgGraph, ProfileRepository, TextRail,
        build_text_stage_with_presence, load_activation_ledger, load_graph_or_dir,
    };
    let root = tempfile::tempdir().unwrap();
    let built_in = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packs/built-in");
    let builtin_graph = load_graph_or_dir(&built_in).unwrap();
    let action = builtin_graph
        .node("action:software-dev/tasks")
        .unwrap()
        .clone();
    let graph = DrgGraph::new("1.0", "STATIC", "cf68-parent", vec![action], Vec::new()).unwrap();
    let identity = identity();
    let profiles = ProfileRepository::load(&built_in, &[], None).unwrap();
    let ledger = load_activation_ledger(root.path()).unwrap();
    let floor = std::collections::BTreeMap::new();
    let inputs = ContextInputs {
        graph: &graph,
        ledger: &ledger,
        identity: &identity,
        profiles: &profiles,
        project_root: root.path(),
        required_floor: &floor,
        project_warnings: &[],
    };
    let mut request = ContextRequest::new("consolidate");
    request.mission_type = Some("software-dev");
    let result = build_text_stage_with_presence(&inputs, &request, || {
        panic!("undeclared rail must not probe presence")
    })
    .unwrap();
    assert_eq!(result.rail, TextRail::NonBootstrap);
    request.action = "tasks";
    request.depth = 2;
    let result = build_text_stage_with_presence(&inputs, &request, || Ok(true)).unwrap();
    assert_eq!(
        result.rail,
        TextRail::Bootstrap,
        "declared action with no artifact edges remains grain-capable"
    );
    let error = build_text_stage_with_presence(&inputs, &request, || {
        Err(sk_doctrine::DoctrineError::InvalidAction {
            action: "presence sentinel".into(),
        })
    })
    .unwrap_err();
    assert!(
        matches!(error, sk_doctrine::DoctrineError::InvalidAction { action } if action == "presence sentinel")
    );
}

#[test]
fn exact_deny_token_never_becomes_the_empty_token() {
    let deny = sk_doctrine::DenySet {
        stems: std::collections::BTreeSet::from([String::new()]),
        ..Default::default()
    };
    assert!(!deny.admits_exact(&PythonString::default()));
    assert!(deny.admits_exact(&PythonString::from_codepoints(&[0xd800]).unwrap()));
}
