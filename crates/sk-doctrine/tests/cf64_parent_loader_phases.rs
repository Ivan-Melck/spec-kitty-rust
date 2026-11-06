use sk_kernel::encoding_provenance::EncodingProvenanceJournal;

#[test]
fn existing_directory_charter_is_a_read_error_not_healthy_absence() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".kittify/charter/charter.md")).unwrap();
    let journal = EncodingProvenanceJournal::new(root.path());
    assert!(sk_doctrine::load_charter_document(root.path(), &journal).is_err());
    assert!(
        !root
            .path()
            .join(".kittify/encoding-provenance/global.jsonl")
            .exists()
    );
}
