//! Pack acquisition acceptance tests: pin validation, digest parity with the
//! Python oracle, staged bounded archives, cache/verify/place, and recovery.

mod support;

use std::io::Write;
use std::path::Path;

use sk_install::archive::{ArchiveLimits, extract_archive};
use sk_install::pack::{
    FileTransport, PackAcquirer, PackTreeStatus, recover_interrupted_swap, remove_tree_bounded,
};
use sk_install::pin::PackPin;

const VERSION: &str = "4.0.0rc7";

fn pin_path() -> &'static Path {
    Path::new("<test-pin>")
}

fn acquirer(cache: &Path) -> PackAcquirer {
    PackAcquirer::new(cache, Box::new(FileTransport))
}

#[test]
fn digests_match_the_python_reference_oracle() {
    // Byte-identical fixture to the one used with the Python oracle
    // (`charter.offering.packs.pack_clean_digest.clean_tree_digest` and
    // `pack_manifest.snapshot_sha256(include_manifest=True, skip_bytecode=True)`).
    let directory = tempfile::tempdir().expect("temp dir");
    let pack = directory.path().join("packs/built-in");
    support::write(&pack.join("pack.yaml"), support::default_pack_yaml());
    support::write(
        &pack.join("directives/DIRECTIVE_001.yaml"),
        "id: DIRECTIVE_001\ntitle: Fixture\r\nbody: CRLF line\r\n",
    );
    support::write(&pack.join("root.graph.yaml"), "x: 1\n");

    let clean = sk_install::clean_tree_digest(&pack, sk_charter::PackTier::BuiltIn).expect("clean");
    let full = sk_install::full_tree_digest(&pack).expect("full");
    assert_eq!(clean, support::ORACLE_CLEAN, "clean-tree digest parity");
    assert_eq!(full, support::ORACLE_FULL, "full-tree digest parity");

    // Bytecode is exempt from the full digest, so it cannot drift the pin.
    support::write(&pack.join("__pycache__/noise.pyc"), "ignored");
    assert_eq!(
        sk_install::full_tree_digest(&pack).expect("full"),
        support::ORACLE_FULL
    );
}

#[test]
fn cache_fetch_verify_and_place_round_trip() {
    let repository_dir = tempfile::tempdir().expect("repository");
    let cache = tempfile::tempdir().expect("cache");
    let destination_dir = tempfile::tempdir().expect("destination");
    let repository =
        support::PackRepository::create(repository_dir.path(), support::FIXTURE_COMMIT, |_| {});
    let pin = repository.pin();
    let acquirer = acquirer(cache.path());
    let destination = destination_dir.path().join("built-in");

    let report = acquirer
        .acquire(&pin, &destination, false)
        .expect("acquisition succeeds");
    assert!(!report.recovered_swap);
    assert!(report.cache.fetched, "the first run fetches");
    assert!(report.files >= 3);
    assert_eq!(
        acquirer.check(&pin, &destination).expect("check"),
        PackTreeStatus::Ok
    );
    assert_eq!(
        sk_install::full_tree_digest(&destination).expect("digest"),
        pin.snapshot_sha256.clone().expect("snapshot")
    );

    // A second run is a verified cache hit, and works offline.
    let second = acquirer
        .acquire(&pin, &destination, true)
        .expect("offline cache hit");
    assert!(!second.cache.fetched);
    assert_eq!(second.files, report.files);

    // A cache-only acquirer (no transport) places from the cache and refuses to
    // fetch when the cache holds nothing.
    let cache_only = PackAcquirer::cache_only(cache.path());
    assert_eq!(
        cache_only.check(&pin, &destination).expect("check"),
        PackTreeStatus::Ok
    );
    remove_tree_bounded(&second.cache.path).expect("clear cache");
    let error = cache_only
        .fetch_to_cache(&pin, false)
        .expect_err("no transport");
    assert_eq!(error.code(), "PACK_SYNC_UNAVAILABLE");
}

#[test]
fn a_tampered_cache_entry_is_refused_and_refetched() {
    let repository_dir = tempfile::tempdir().expect("repository");
    let cache = tempfile::tempdir().expect("cache");
    let repository =
        support::PackRepository::create(repository_dir.path(), support::FIXTURE_COMMIT, |_| {});
    let pin = repository.pin();
    let acquirer = acquirer(cache.path());
    let entry = acquirer.fetch_to_cache(&pin, false).expect("cache fetch");

    // Poison the cache.
    support::write(
        &entry.path.join("directives/DIRECTIVE_001.yaml"),
        "id: EVIL\n",
    );

    // Offline: the poisoned entry is removed and the run fails closed.
    let error = acquirer
        .fetch_to_cache(&pin, true)
        .expect_err("poisoned cache refused");
    assert_eq!(error.code(), "PACK_SYNC_UNAVAILABLE");
    assert!(
        !entry.path.exists(),
        "nothing that fails its digest stays cached"
    );

    // Online: it is refetched and verified again.
    let recovered = acquirer.fetch_to_cache(&pin, false).expect("refetch");
    assert!(recovered.fetched);
    pin.verify(pin_path(), &recovered.path)
        .expect("verified again");
}

#[test]
fn a_pin_that_disagrees_with_the_tree_is_refused_and_the_destination_survives() {
    let repository_dir = tempfile::tempdir().expect("repository");
    let cache = tempfile::tempdir().expect("cache");
    let destination_dir = tempfile::tempdir().expect("destination");
    let repository =
        support::PackRepository::create(repository_dir.path(), support::FIXTURE_COMMIT, |_| {});
    let mut pin = repository.pin();
    pin.tree_digest.value = "0".repeat(64);

    // A pre-existing tree the operator has in place.
    let destination = destination_dir.path().join("built-in");
    support::write(&destination.join("pack.yaml"), "operator copy\n");
    let before = support::file_bytes(&destination.join("pack.yaml"));

    let error = acquirer(cache.path())
        .acquire(&pin, &destination, false)
        .expect_err("digest mismatch");
    assert_eq!(error.code(), "PACK_DIGEST_MISMATCH");
    assert_eq!(
        support::file_bytes(&destination.join("pack.yaml")),
        before,
        "a refused acquisition never touches the placed tree"
    );
    assert!(
        acquirer(cache.path())
            .check(&pin, &destination)
            .expect("check")
            == PackTreeStatus::Stale
    );
}

#[test]
fn a_pinned_tree_containing_a_symlink_is_refused() {
    let repository_dir = tempfile::tempdir().expect("repository");
    let cache = tempfile::tempdir().expect("cache");
    let repository =
        support::PackRepository::create(repository_dir.path(), support::FIXTURE_COMMIT, |_| {});
    let pin = repository.pin();
    // The published tree gains a symlink after the pin was computed: the
    // transport must refuse it before the digest can even be evaluated.
    std::os::unix::fs::symlink(
        "/etc/passwd",
        repository.pack_root.join("directives/ESCAPE.yaml"),
    )
    .expect("fixture symlink");
    let error = acquirer(cache.path())
        .fetch_to_cache(&pin, false)
        .expect_err("symlink refused");
    assert_eq!(error.code(), "PACK_DIGEST_MISMATCH");
    assert!(error.to_string().contains("symlink"));
}

#[test]
fn a_repository_serving_the_wrong_commit_is_refused() {
    let repository_dir = tempfile::tempdir().expect("repository");
    let repository = support::PackRepository::create(
        repository_dir.path(),
        "1111111111111111111111111111111111111111",
        |_| {},
    );
    // The pin names a different commit than the repository advertises.
    let clean = sk_install::clean_tree_digest(&repository.pack_root, sk_charter::PackTier::BuiltIn)
        .expect("clean");
    let full = sk_install::full_tree_digest(&repository.pack_root).expect("full");
    let pin = PackPin::from_parts(
        pin_path(),
        repository.url(),
        support::FIXTURE_COMMIT.to_string(),
        "packs/built-in".to_string(),
        sk_install::TreeDigest {
            algorithm: sk_install::TREE_DIGEST_ALGORITHM.to_string(),
            value: clean,
            level: "built-in".to_string(),
        },
        None,
        Some(full),
    )
    .expect("pin parses");
    let cache = tempfile::tempdir().expect("cache");
    let error = acquirer(cache.path())
        .fetch_to_cache(&pin, false)
        .expect_err("wrong commit");
    assert_eq!(error.code(), "PACK_COMMIT_UNRESOLVABLE");
}

#[test]
fn unsafe_and_unverifiable_pins_fail_closed() {
    let directory = tempfile::tempdir().expect("temp dir");
    let pin_file = directory.path().join("built-in.source.yaml");
    let digest = "3b4cbd97a28efcce3b07f9751ccdde4a976bd206b5049056a9c347b9fb73f839";

    for (content, expected) in [
        // Tag instead of a commit SHA.
        (
            format!(
                "repository: https://example.invalid/r.git\ncommit: v1.2.3\nsubtree: packs/built-in\ntree_digest:\n  algorithm: clean-tree-sha256\n  value: {digest}\n  level: built-in\nsnapshot_sha256: {digest}\n"
            ),
            "PACK_PIN_INVALID",
        ),
        // Subtree traversal.
        (
            format!(
                "repository: https://example.invalid/r.git\ncommit: {}\nsubtree: ../etc\ntree_digest:\n  algorithm: clean-tree-sha256\n  value: {digest}\n  level: built-in\nsnapshot_sha256: {digest}\n",
                support::FIXTURE_COMMIT
            ),
            "PACK_PIN_INVALID",
        ),
        // Unsupported repository scheme.
        (
            format!(
                "repository: ftp://example.invalid/r.git\ncommit: {}\nsubtree: packs/built-in\ntree_digest:\n  algorithm: clean-tree-sha256\n  value: {digest}\n  level: built-in\nsnapshot_sha256: {digest}\n",
                support::FIXTURE_COMMIT
            ),
            "PACK_PIN_INVALID",
        ),
        // Unknown digest algorithm.
        (
            format!(
                "repository: https://example.invalid/r.git\ncommit: {}\nsubtree: packs/built-in\ntree_digest:\n  algorithm: md5\n  value: {digest}\n  level: built-in\nsnapshot_sha256: {digest}\n",
                support::FIXTURE_COMMIT
            ),
            "PACK_PIN_INVALID",
        ),
    ] {
        support::write(&pin_file, content);
        let error = PackPin::load(&pin_file).expect_err("pin refused");
        assert_eq!(error.code(), expected);
    }

    // A valid pin shape loads, and a pin without the full-tree digest is unverifiable.
    let valid = format!(
        "repository: https://example.invalid/r.git\ncommit: {}\nsubtree: packs/built-in\ntree_digest:\n  algorithm: clean-tree-sha256\n  value: {digest}\n  level: built-in\nsnapshot_sha256: {digest}\n",
        support::FIXTURE_COMMIT
    );
    support::write(&pin_file, valid);
    PackPin::load(&pin_file).expect("valid pin");

    let partial = format!(
        "repository: https://example.invalid/r.git\ncommit: {}\nsubtree: packs/built-in\ntree_digest:\n  algorithm: clean-tree-sha256\n  value: {digest}\n  level: built-in\n",
        support::FIXTURE_COMMIT
    );
    support::write(&pin_file, partial);
    let pin = PackPin::load(&pin_file).expect("partial pin parses");
    let error = pin
        .verify(&pin_file, directory.path())
        .expect_err("unverifiable");
    assert_eq!(error.code(), "PACK_PIN_INVALID");
}

// ---------------------------------------------------------------------------
// Archive guards
// ---------------------------------------------------------------------------

fn tar_gz_raw_name(header: &mut tar::Header, name: &str) {
    // `Header::set_path` refuses `..`; the guard under test must see the raw
    // member name, so the GNU name field is written directly.
    let gnu = header.as_gnu_mut().expect("gnu header");
    let bytes = name.as_bytes();
    gnu.name[..bytes.len()].copy_from_slice(bytes);
}

fn tar_gz(path: &Path, entries: &[(&str, Option<&[u8]>, bool)]) {
    let file = std::fs::File::create(path).expect("create archive");
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, data, is_symlink) in entries {
        let mut header = tar::Header::new_gnu();
        if *is_symlink {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder
                .append_link(
                    &mut header,
                    name,
                    data.map(|bytes| std::str::from_utf8(bytes).unwrap())
                        .unwrap_or("/etc/passwd"),
                )
                .expect("append symlink");
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(data.map(|bytes| bytes.len() as u64).unwrap_or(0));
            header.set_mode(0o644);
            tar_gz_raw_name(&mut header, name);
            header.set_cksum();
            builder
                .append(&header, data.unwrap_or(b""))
                .expect("append file");
        }
    }
    let encoder = builder.into_inner().expect("finish tar");
    encoder.finish().expect("finish gzip");
}

#[test]
fn archive_traversal_members_are_refused() {
    let directory = tempfile::tempdir().expect("temp dir");
    let archive = directory.path().join("evil.tar.gz");
    tar_gz(&archive, &[("../escape.txt", Some(b"owned"), false)]);
    let target = directory.path().join("extract");
    let error = extract_archive(
        &archive,
        &target,
        sk_install::ArchiveKind::TarGz,
        &ArchiveLimits::default(),
    )
    .expect_err("traversal refused");
    assert_eq!(error.code(), "archive_traversal");
    assert!(!directory.path().join("escape.txt").exists());
}

#[test]
fn archive_symlink_members_are_refused() {
    let directory = tempfile::tempdir().expect("temp dir");
    let archive = directory.path().join("link.tar.gz");
    tar_gz(&archive, &[("link", Some(b"/etc/passwd"), true)]);
    let target = directory.path().join("extract");
    let error = extract_archive(
        &archive,
        &target,
        sk_install::ArchiveKind::TarGz,
        &ArchiveLimits::default(),
    )
    .expect_err("symlink refused");
    assert_eq!(error.code(), "archive_link");
    assert!(!target.join("link").exists());
}

#[test]
fn archive_member_and_byte_bombs_are_refused() {
    let directory = tempfile::tempdir().expect("temp dir");
    let archive = directory.path().join("many.tar.gz");
    tar_gz(
        &archive,
        &[
            ("a.txt", Some(b"a"), false),
            ("b.txt", Some(b"b"), false),
            ("c.txt", Some(b"c"), false),
        ],
    );
    let target = directory.path().join("extract");
    let limits = ArchiveLimits {
        max_members: 2,
        ..ArchiveLimits::default()
    };
    let error = extract_archive(&archive, &target, sk_install::ArchiveKind::TarGz, &limits)
        .expect_err("member bomb");
    assert_eq!(error.code(), "archive_too_many_members");

    let target = directory.path().join("extract2");
    let limits = ArchiveLimits {
        max_extracted_bytes: 1,
        ..ArchiveLimits::default()
    };
    let error = extract_archive(&archive, &target, sk_install::ArchiveKind::TarGz, &limits)
        .expect_err("byte bomb");
    assert_eq!(error.code(), "archive_too_large");
}

#[test]
fn zip_traversal_members_are_refused() {
    let directory = tempfile::tempdir().expect("temp dir");
    let archive = directory.path().join("evil.zip");
    {
        let file = std::fs::File::create(&archive).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("../escape.txt", zip::write::SimpleFileOptions::default())
            .expect("start entry");
        writer.write_all(b"owned").expect("write entry");
        writer.finish().expect("finish zip");
    }
    let target = directory.path().join("extract");
    let error = extract_archive(
        &archive,
        &target,
        sk_install::ArchiveKind::Zip,
        &ArchiveLimits::default(),
    )
    .expect_err("traversal refused");
    assert_eq!(error.code(), "archive_traversal");
    assert!(!directory.path().join("escape.txt").exists());
}

#[test]
fn a_valid_archive_extracts_deterministically() {
    let directory = tempfile::tempdir().expect("temp dir");
    let archive = directory.path().join("pack.tar.gz");
    tar_gz(
        &archive,
        &[
            (
                "pack.yaml",
                Some(support::default_pack_yaml().as_bytes()),
                false,
            ),
            (
                "directives/DIRECTIVE_001.yaml",
                Some(b"id: DIRECTIVE_001\n"),
                false,
            ),
        ],
    );
    let target = directory.path().join("extract");
    let files = extract_archive(
        &archive,
        &target,
        sk_install::ArchiveKind::TarGz,
        &ArchiveLimits::default(),
    )
    .expect("extract");
    assert_eq!(files, 2);
    assert_eq!(
        support::read(&target.join("pack.yaml")),
        support::default_pack_yaml()
    );

    // A staged single top-level directory is hoisted.
    let nested = directory.path().join("nested.tar.gz");
    tar_gz(
        &nested,
        &[(
            "repo-sha/pack.yaml",
            Some(support::default_pack_yaml().as_bytes()),
            false,
        )],
    );
    let target = directory.path().join("extract-nested");
    extract_archive(
        &nested,
        &target,
        sk_install::ArchiveKind::TarGz,
        &ArchiveLimits::default(),
    )
    .expect("extract");
    assert!(sk_install::archive::flatten_single_top_dir(&target).expect("flatten"));
    assert!(target.join("pack.yaml").is_file());
}

#[test]
fn an_interrupted_swap_is_repaired() {
    let directory = tempfile::tempdir().expect("temp dir");
    let destination = directory.path().join("built-in");
    let aside = directory.path().join(".built-in.aside");
    support::write(&aside.join("pack.yaml"), "previous tree\n");

    assert!(recover_interrupted_swap(&destination).expect("recover"));
    assert!(destination.join("pack.yaml").is_file());
    assert!(!aside.exists());
    assert_eq!(
        support::read(&destination.join("pack.yaml")),
        "previous tree\n"
    );

    // When the swap finished, only the aside cleanup is left.
    support::write(&destination.join("pack.yaml"), "new tree\n");
    support::write(&aside.join("pack.yaml"), "old tree\n");
    assert!(recover_interrupted_swap(&destination).expect("recover"));
    assert!(!aside.exists());
    assert_eq!(support::read(&destination.join("pack.yaml")), "new tree\n");
    assert!(!recover_interrupted_swap(&destination).expect("idempotent"));
}

#[test]
fn bounded_removal_never_follows_a_link() {
    let directory = tempfile::tempdir().expect("temp dir");
    let outside = directory.path().join("outside");
    support::write(&outside.join("keep.txt"), "keep me\n");
    let tree = directory.path().join("tree");
    support::write(&tree.join("file.txt"), "delete me\n");
    std::os::unix::fs::symlink(&outside, tree.join("link")).expect("symlink");
    remove_tree_bounded(&tree).expect("remove tree");
    assert!(!tree.exists());
    assert_eq!(support::read(&outside.join("keep.txt")), "keep me\n");
    let _ = VERSION;
}
