use sk_doctrine::{ReferenceEntry, filter_references_for_action, reference_source_index};
use sk_kernel::{PythonString, python_os_encoding::OsEncodingContext};

#[cfg(unix)]
#[test]
fn source_directory_permission_error_escapes_and_does_not_cache_empty() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o0)).unwrap();
    let probe = sk_kernel::paths::source_path_is_dir(&root.path().join("tactics"));
    let index = reference_source_index(root.path(), &utf8());
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(probe, Err(error) if error.raw_os_error() == Some(13)));
    assert!(
        index.is_err(),
        "Source errors must not become a permanently cached empty index"
    );
    std::fs::create_dir(root.path().join("directives")).unwrap();
    std::fs::write(root.path().join("directives/retry.yaml"), "x: 1\n").unwrap();
    assert!(
        reference_source_index(root.path(), &utf8())
            .unwrap()
            .kind_index(&text("directive"))
            .unwrap()
            .contains_key(text("retry").codepoints())
    );
}

#[cfg(unix)]
#[test]
fn source_directory_nul_valueerror_is_false_not_an_escape() {
    assert!(!sk_kernel::paths::source_path_is_dir(std::path::Path::new("\0")).unwrap());
}

fn text(s: &str) -> PythonString {
    PythonString::from_scalars(s)
}
fn utf8() -> OsEncodingContext {
    OsEncodingContext::from_lookup(|key| (key == "PYTHONUTF8").then(|| "1".into()))
}

#[test]
fn scope_word_at_end_is_a_global_nonmatch_not_a_native_panic() {
    let cases = [
        "(action:x",
        "(action:implement",
        "(action:\u{2003}IMPLEMENT",
        "(action:review) (action:implement)",
        "(action:implement)",
        "(action:implement )",
        "(action:",
        "prefix(action:x)tail",
    ];
    let expected = [1, 1, 1, 0, 1, 1, 1, 0];
    for (summary, kept) in cases.into_iter().zip(expected) {
        let refs = [ReferenceEntry {
            id: text("x"),
            kind: text("local_support"),
            title: text(""),
            summary: text(summary),
            local_path: text(""),
        }];
        assert_eq!(
            filter_references_for_action(&refs, &text("implement")).len(),
            kept,
            "{summary:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn double_root_and_single_root_are_distinct_source_cache_keys() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("directives");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("first.yaml"), "x: 1\n").unwrap();
    let first = reference_source_index(root.path(), &utf8()).unwrap();
    std::fs::write(directory.join("second.yaml"), "x: 2\n").unwrap();
    let doubled = std::path::PathBuf::from(format!(
        "//{}",
        root.path().display().to_string().trim_start_matches('/')
    ));
    let second = reference_source_index(&doubled, &utf8()).unwrap();
    let first_map = first.kind_index(&text("directive")).unwrap();
    let second_map = second.kind_index(&text("directive")).unwrap();
    assert!(!first_map.contains_key(text("second").codepoints()));
    assert!(
        second_map.contains_key(text("second").codepoints()),
        "Source Path('//...') cannot reuse Path('/...')'s frozen scan"
    );
    assert!(
        second_map
            .get(text("second").codepoints())
            .unwrap()
            .as_os_str()
            .to_str()
            .unwrap()
            .starts_with("//")
    );
    assert!(!std::sync::Arc::ptr_eq(&first, &second));
    let dot_alias = reference_source_index(&root.path().join("."), &utf8()).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &dot_alias));
}
