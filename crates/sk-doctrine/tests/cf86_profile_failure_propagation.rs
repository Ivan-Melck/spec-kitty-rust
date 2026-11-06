//! CF86 — actual profile read/constructor/Native failure propagation.
//!
//! Red-first controls call only the pre-existing public API: `profile_repository`
//! flattens `DoctrineError::ProfileLoadAborted` with `OfferingLoadError::new(...)`, so
//! `abort()` is `None`, `resolve_effective_sets` degrades to an unresolved key with
//! fallback ids (healthy), and a Native `HashSeed` failure surfaces as a message-only
//! error instead of a typed Native cause. The tests below pin the measured Source
//! classes and the real service/strict/fallback consumers. Source-first measurements
//! are recorded in `/tmp/sk-cf86-profile-failure-report.md`; this file is the Native side.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_doctrine::{
    CanonicalOfferingService, EffectiveSetError, EffectiveSetService, NATIVE_OFFERING_SERVICE,
    OfferingContext, ProfileRepository, ServiceKeys, resolve_effective_sets,
};
use sk_kernel::PythonString;

/// The isolated roots every child asserts. `(env var, directory name)`.
const ROOTS: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CACHE_HOME", "xdg-cache"),
    ("XDG_CONFIG_HOME", "xdg-config"),
    ("XDG_DATA_HOME", "xdg-data"),
    ("XDG_STATE_HOME", "xdg-state"),
    ("XDG_RUNTIME_DIR", "xdg-runtime"),
    ("XDG_CONFIG_DIRS", "xdg-config-dirs"),
    ("XDG_DATA_DIRS", "xdg-data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn built_in_root() -> PathBuf {
    repository_root().join("packs/built-in")
}

fn write(path: &Path, text: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture directory");
    }
    fs::write(path, text).expect("fixture file");
}

/// A whole, schema-valid agent profile under `id` (the shipped implementer body).
fn profile_doc(id: &str) -> String {
    fs::read_to_string(built_in_root().join("agent_profiles/implementer-ivan.agent.yaml"))
        .expect("shipped profile")
        .replace("profile-id: implementer-ivan", &format!("profile-id: {id}"))
}

fn profile_with_int_abort(id: &str) -> String {
    profile_doc(id).replace(
        "routing-priority: 50",
        "routing-priority: !!int \"not-an-int\"",
    )
}

fn profile_with_bool_abort(id: &str) -> String {
    let mut text = profile_doc(id);
    text.push_str("sentinel: !!bool \"maybe\"\n");
    text
}

fn fixture() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temp dir");
    for (_, name) in ROOTS {
        fs::create_dir_all(directory.path().join(name)).expect("isolated root");
    }
    directory
}

fn org_pack(root: &Path) -> PathBuf {
    let org = root.join("packs/acme");
    fs::create_dir_all(org.join("agent_profiles")).expect("org profile dir");
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(&org.join("org-charter.yaml"), "{}\n");
    org
}

fn ledger(root: &Path, text: &str) {
    write(&root.join(".kittify/charter/activation.yaml"), text);
}

fn service_for(root: &Path) -> CanonicalOfferingService {
    CanonicalOfferingService::new(OfferingContext::resolve(root).expect("offering context"))
}

fn scalar(ids: &[PythonString]) -> Vec<String> {
    ids.iter()
        .map(|id| id.to_scalar_string().expect("scalar receipt"))
        .collect()
}

fn child_environment(command: &mut Command, root: &Path, seed: &str) {
    let repository = repository_root();
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", seed)
        .env("CF86_BUSINESS_ROOT", root)
        .env("CF86_EXPECTED_SOURCE_ROOT", &repository)
        .env("CF86_ORACLE_CHECKED", "53");
    for (key, name) in ROOTS {
        command.env(key, root.join(name));
    }
}

fn assert_child_environment(root: &Path, seed: &str) {
    let repository = repository_root();
    assert!(root.is_absolute() && root.is_dir());
    assert_eq!(std::env::current_dir().unwrap(), root, "child CWD");
    for (key, name) in ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key}"
        );
    }
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(repository.join("src")),
        "absolute PYTHONPATH"
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(repository.join("packs")),
        "absolute packs root"
    );
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), seed);
    assert_eq!(std::env::var("CF86_ORACLE_CHECKED").unwrap(), "53");
    assert_eq!(
        std::env::var_os("CF86_EXPECTED_SOURCE_ROOT").map(PathBuf::from),
        Some(repository)
    );
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    let stream = sk_kernel::python_stream::StreamPolicy::for_stderr().unwrap();
    assert_eq!(stream.encoding(), "utf-8");
    assert_eq!(stream.errors(), "backslashreplace");
}

/// The real agent-profile repository constructor, with the measured Source class.
#[test]
fn profile_repository_retains_the_source_constructor_abort_and_class() {
    let directory = fixture();
    let root = directory.path();
    let org = org_pack(root);
    write(
        &org.join("agent_profiles/ivan.agent.yaml"),
        profile_doc("ivan"),
    );
    write(
        &org.join("agent_profiles/bad.agent.yaml"),
        profile_with_int_abort("bad"),
    );
    ledger(root, "activated_agent_profiles: [ivan, bad]\n");
    let service = service_for(root);

    let error = service
        .ids_for("activated_agent_profiles")
        .expect_err("the profile repository construction aborts");
    let abort = error
        .abort()
        .expect("the real Source constructor abort is retained");
    assert_eq!(abort.python, "ValueError");
    assert!(
        abort
            .message
            .contains("invalid literal for int() with base 10"),
        "the real constructor message survives: {abort}"
    );
    // The reference's own reason string carries the class and message, not the
    // path (`an artifact layer cannot be scanned: ValueError: ...`); the path stays
    // on the typed `DoctrineError`/`ProfileLoadAbort` seam.
    let rendered = error.to_string();
    assert!(rendered.contains("ValueError"), "{rendered}");
    assert!(rendered.contains("invalid literal for int()"), "{rendered}");
    assert!(
        error.native().is_none(),
        "a Source abort is not a Native cause"
    );

    // A `!!bool` spelling is a real Python `KeyError`, not a generic ValueError.
    write(
        &org.join("agent_profiles/bad.agent.yaml"),
        profile_with_bool_abort("bad"),
    );
    let error = service_for(root)
        .ids_for("activated_agent_profiles")
        .expect_err("the bool constructor aborts");
    let abort = error.abort().expect("typed KeyError abort");
    assert_eq!(abort.python, "KeyError");
    assert!(abort.message.contains("maybe"), "{abort}");

    // A non-mapping document reaches `data.get(...)` and raises `AttributeError`.
    write(&org.join("agent_profiles/bad.agent.yaml"), "- one\n- two\n");
    let error = service_for(root)
        .ids_for("activated_agent_profiles")
        .expect_err("the non-mapping document aborts");
    let abort = error.abort().expect("typed AttributeError abort");
    assert_eq!(abort.python, "AttributeError");
}

/// The reference catches `(YAMLError, OSError)` per file; those stay ordinary skips.
/// Its Reader also refuses invalid UTF-8 as a `YAMLError`, so that is a skip too — the
/// measured phase is reproduced with the Kernel strict decoder's diagnostic.
#[test]
fn profile_caught_read_failures_stay_skipped() {
    let directory = fixture();
    let root = directory.path();
    let org = org_pack(root);
    let profiles = org.join("agent_profiles");
    write(&profiles.join("good.agent.yaml"), profile_doc("cf86-good"));
    write(
        &profiles.join("syntax.agent.yaml"),
        "profile-id: [unclosed\n",
    );
    write(
        &profiles.join("schema.agent.yaml"),
        "profile-id: cf86-schema\n",
    );
    let unreadable = profiles.join("unreadable.agent.yaml");
    write(&unreadable, profile_doc("cf86-unreadable"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).expect("mode");
    }
    write(
        &profiles.join("invalid.agent.yaml"),
        b"profile-id: cf86-utf8\xff\n",
    );
    let repository = ProfileRepository::load_profile_directories(
        &built_in_root(),
        std::slice::from_ref(&profiles),
        None,
        None,
    )
    .expect("caught read failures are skips, not aborts");

    assert!(
        repository
            .get(&PythonString::from_scalars("cf86-good"))
            .is_some(),
        "the valid sibling still loads"
    );
    let summaries: Vec<String> = repository
        .skipped()
        .iter()
        .map(|entry| entry.summary.clone())
        .collect();
    assert!(
        summaries
            .iter()
            .any(|text| text.contains("YAML/read error") && text.contains("YAML syntax error")),
        "{summaries:?}"
    );
    assert!(
        summaries
            .iter()
            .any(|text| text.contains("required property")),
        "{summaries:?}"
    );
    #[cfg(unix)]
    assert!(
        summaries.iter().any(|text| {
            (text.contains("Permission denied") || text.contains("os error 13"))
                && text.contains("YAML/read error")
        }),
        "the OS read phase is a skip: {summaries:?}"
    );
    assert!(
        summaries
            .iter()
            .any(|text| text.contains("invalid start byte")),
        "the strict decode phase keeps the reference Reader shape: {summaries:?}"
    );

    // The activation-aware service is equally nonfatal for the same tree.
    let ids = scalar(
        &service_for(root)
            .ids_for("activated_agent_profiles")
            .expect("the skipped profile tree is a successful construction"),
    );
    assert!(ids.contains(&"cf86-good".to_owned()), "{ids:?}");
    assert!(!ids.contains(&"cf86-schema".to_owned()), "{ids:?}");
}

/// Strict pass reports the key unresolved (the reference catches the abort); the
/// fallback re-read has no catch and raises the same real class. It must never report
/// fallback health for a real Source-escaping constructor abort.
#[test]
fn profile_strict_pass_is_unresolved_and_the_fallback_escapes() {
    let directory = fixture();
    let root = directory.path();
    let org = org_pack(root);
    write(
        &org.join("agent_profiles/ivan.agent.yaml"),
        profile_doc("ivan"),
    );
    write(
        &org.join("agent_profiles/bad.agent.yaml"),
        profile_with_int_abort("bad"),
    );
    ledger(root, "activated_agent_profiles: [ivan, bad]\n");

    match service_for(root).service_keys("activated_agent_profiles") {
        ServiceKeys::Aborted(abort) => {
            assert_eq!(abort.python, "ValueError", "typed strict-phase cause");
        }
        other => panic!("strict phase must carry the typed abort, got {other:?}"),
    }

    let error = resolve_effective_sets(
        root,
        &["activated_agent_profiles"],
        &NATIVE_OFFERING_SERVICE,
    )
    .expect_err("the fallback re-read escapes the real constructor abort");
    match error {
        EffectiveSetError::Fatal(abort) => {
            assert_eq!(abort.python, "ValueError");
            assert!(
                abort.message.contains("invalid literal for int()"),
                "{abort}"
            );
        }
        other => panic!("expected the Source fallback abort, got {other:?}"),
    }

    // Repair in place: a fresh resolution recovers, exactly like the reference.
    write(
        &org.join("agent_profiles/bad.agent.yaml"),
        profile_doc("bad"),
    );
    let sets = resolve_effective_sets(
        root,
        &["activated_agent_profiles"],
        &NATIVE_OFFERING_SERVICE,
    )
    .expect("the repaired tree resolves");
    assert!(sets[0].resolved, "{sets:?}");
    assert!(scalar(&sets[0].ids).contains(&"bad".to_owned()), "{sets:?}");
}

/// The Native `PYTHONHASHSEED` refusal is a typed Native cause with the real phase and
/// original Kernel error, never a fabricated Source `Fatal Python error` class.
#[test]
fn native_hash_seed_profile_failure_is_a_typed_native_cause() {
    if std::env::var_os("CF86_NATIVE_HASHSEED").is_some() {
        let root = PathBuf::from(std::env::var("CF86_BUSINESS_ROOT").unwrap());
        assert_child_environment(&root, "bogus");
        let profiles = root.join("packs/acme/agent_profiles");
        let loaded =
            ProfileRepository::load_profile_directories(&built_in_root(), &[profiles], None, None);
        println!("CF86_HASH repository_ok={}", loaded.is_ok());
        let context = OfferingContext::new(
            root.clone(),
            sk_doctrine::OfferingRoots {
                built_in: Some(built_in_root()),
                org_roots: vec![root.join("packs/acme")],
                project: None,
            },
            None,
        );
        let service = CanonicalOfferingService::new(context);
        match service.ids_for("activated_agent_profiles") {
            Ok(ids) => println!("CF86_HASH service_cause=ok ids={}", ids.len()),
            Err(error) => {
                let cause = if error.abort().is_some() {
                    "abort"
                } else if error.native().is_some() {
                    "native"
                } else {
                    "plain"
                };
                let rendered = error.to_string();
                println!("CF86_HASH service_cause={cause}");
                println!(
                    "CF86_HASH service_has_phase={}",
                    rendered.contains("agent profile read")
                );
                println!(
                    "CF86_HASH service_has_fatal_label={}",
                    rendered.contains("Fatal Python error: config_init_hash_seed")
                );
            }
        }
        return;
    }

    let directory = fixture();
    let root = directory.path();
    let org = org_pack(root);
    write(
        &org.join("agent_profiles/hash.agent.yaml"),
        "profile-id: cf86-hash\ncapabilities: !!set\n  ? alpha\n  ? beta\n",
    );
    ledger(root, "activated_agent_profiles: [cf86-hash]\n");
    let mut command = Command::new(std::env::current_exe().unwrap());
    child_environment(&mut command, root, "bogus");
    command
        .args([
            "--exact",
            "native_hash_seed_profile_failure_is_a_typed_native_cause",
            "--nocapture",
        ])
        .env("CF86_NATIVE_HASHSEED", "1");
    let output = command.output().expect("hashseed native child");
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let receipts = hash_receipts(&stdout);
    assert_eq!(
        receipts.get("service_cause").map(String::as_str),
        Some("native"),
        "a Native start-up cause must not be flattened or labelled a Source class: {stdout}"
    );
    assert_eq!(
        receipts.get("service_has_phase").map(String::as_str),
        Some("true"),
        "the profile read phase is named: {stdout}"
    );
}

fn hash_receipts(stdout: &str) -> BTreeMap<String, String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("CF86_HASH "))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.trim().to_owned()))
        .collect()
}

/// Same-instance recovery and the success-only publication are Source behaviour:
/// a failed repository construction is retried on the SAME service; a successfully
/// skipped profile is published and stays warm until a new service reads it.
#[test]
fn profile_same_instance_repair_and_warm_skip_are_preserved() {
    let directory = fixture();
    let root = directory.path();
    let org = org_pack(root);
    write(
        &org.join("agent_profiles/bad.agent.yaml"),
        profile_with_int_abort("bad"),
    );
    ledger(root, "activated_agent_profiles: [bad]\n");
    let service = service_for(root);
    assert!(
        service.ids_for("activated_agent_profiles").is_err(),
        "the first construction aborts"
    );
    write(
        &org.join("agent_profiles/bad.agent.yaml"),
        profile_doc("bad"),
    );
    assert_eq!(
        scalar(
            &service
                .ids_for("activated_agent_profiles")
                .expect("the same service retries the repaired profile repository")
        ),
        vec!["bad".to_owned()],
        "a repository rebuilt after a failed construction is a fresh Source load"
    );

    let warm = fixture();
    let warm_root = warm.path();
    let warm_org = org_pack(warm_root);
    write(
        &warm_org.join("agent_profiles/good.agent.yaml"),
        profile_doc("cf86-warm"),
    );
    write(
        &warm_org.join("agent_profiles/skipped.agent.yaml"),
        "profile-id: cf86-skipped\n",
    );
    let warm_service = service_for(warm_root);
    let first = scalar(
        &warm_service
            .ids_for("activated_agent_profiles")
            .expect("a skipped profile is a successful construction"),
    );
    assert!(first.contains(&"cf86-warm".to_owned()), "{first:?}");
    assert!(!first.contains(&"cf86-skipped".to_owned()), "{first:?}");
    write(
        &warm_org.join("agent_profiles/skipped.agent.yaml"),
        profile_doc("cf86-late"),
    );
    let warmed = scalar(
        &warm_service
            .ids_for("activated_agent_profiles")
            .expect("the published snapshot is the instance's answer"),
    );
    assert!(warmed.contains(&"cf86-warm".to_owned()), "{warmed:?}");
    assert!(
        !warmed.contains(&"cf86-late".to_owned()),
        "repairing a skipped profile does not refresh an existing service: {warmed:?}"
    );
    let fresh = scalar(
        &service_for(warm_root)
            .ids_for("activated_agent_profiles")
            .expect("a new service re-reads"),
    );
    assert!(fresh.contains(&"cf86-warm".to_owned()), "{fresh:?}");
    assert!(fresh.contains(&"cf86-late".to_owned()), "{fresh:?}");
}

/// One guarded independent Source/Native pair over selected actual failure classes.
#[test]
fn guarded_source_native_profile_failure_pair() {
    if std::env::var_os("CF86_NATIVE_PAIR").is_some() {
        let root = PathBuf::from(std::env::var("CF86_BUSINESS_ROOT").unwrap());
        assert_child_environment(&root, "0");
        for (key, value) in native_pair_receipts(&root) {
            println!("CF86_NATIVE {key}={value}");
        }
        return;
    }

    let directory = fixture();
    let root = directory.path();
    populate_pair(root);
    write(&root.join("source-probe.py"), SOURCE_PAIR_PROBE.as_bytes());

    let mut source = Command::new(support::python());
    child_environment(&mut source, root, "0");
    source.arg(root.join("source-probe.py"));
    let output = source.output().expect("Source child");
    assert!(
        output.status.success(),
        "Source child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source_receipts = pair_receipts(&String::from_utf8(output.stdout).unwrap(), "CF86_SOURCE ");

    assert_eq!(source_receipts["repo_utf16le"], "ok:bad-present");
    assert_eq!(source_receipts["repo_utf16be"], "ok:bad-present");
    assert_eq!(source_receipts["eff_nonmapping"], "ok:resolved");
    let native_directory = fixture();
    let native_root = native_directory.path();
    assert_ne!(root, native_root);
    populate_pair(native_root);
    let mut native = Command::new(std::env::current_exe().unwrap());
    child_environment(&mut native, native_root, "0");
    native
        .args([
            "--exact",
            "guarded_source_native_profile_failure_pair",
            "--nocapture",
        ])
        .env("CF86_NATIVE_PAIR", "1");
    let output = native.output().expect("Native child");
    assert!(
        output.status.success(),
        "Native child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let native_receipts = pair_receipts(&String::from_utf8(output.stdout).unwrap(), "CF86_NATIVE ");

    println!("CF86_PAIR source={source_receipts:?}");
    println!("CF86_PAIR native={native_receipts:?}");
    assert_eq!(
        source_receipts.len(),
        native_receipts.len(),
        "both children answer the selected plan: source={source_receipts:?} native={native_receipts:?}"
    );
    for (key, source_value) in &source_receipts {
        assert_eq!(
            Some(source_value),
            native_receipts.get(key),
            "the selected failure class must match for {key}"
        );
    }
    assert_eq!(
        source_receipts.get("repo_int").map(String::as_str),
        Some("abort:ValueError")
    );
    assert_eq!(
        source_receipts.get("repo_bool").map(String::as_str),
        Some("abort:KeyError")
    );
    assert_eq!(
        source_receipts.get("repo_nonmapping").map(String::as_str),
        Some("abort:AttributeError")
    );
    assert_eq!(
        source_receipts.get("repo_syntax").map(String::as_str),
        Some("ok")
    );
    assert_eq!(
        source_receipts.get("repo_invalid_utf8").map(String::as_str),
        Some("ok"),
        "the reference catches its Reader's invalid-UTF-8 failure per file"
    );
    assert_eq!(
        source_receipts.get("eff_int").map(String::as_str),
        Some("raise:ValueError")
    );
    assert_eq!(
        source_receipts.get("eff_bool").map(String::as_str),
        Some("raise:KeyError")
    );
    assert_eq!(
        source_receipts.get("eff_invalid_utf8").map(String::as_str),
        Some("raise:UnicodeDecodeError")
    );
    assert_eq!(
        source_receipts.get("eff_syntax").map(String::as_str),
        Some("ok:resolved")
    );
}

fn pair_receipts(stdout: &str, prefix: &str) -> BTreeMap<String, String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.trim().to_owned()))
        .collect()
}

fn pair_case(root: &Path, name: &str, profile: &[u8]) -> PathBuf {
    let case = root.join("cases").join(name);
    write(
        &case.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(&case.join("packs/acme/org-charter.yaml"), "{}\n");
    write(
        &case.join(".kittify/charter/activation.yaml"),
        "activated_agent_profiles: [ivan, bad]\n",
    );
    write(
        &case.join("packs/acme/agent_profiles/ivan.agent.yaml"),
        profile_doc("ivan"),
    );
    write(
        &case.join("packs/acme/agent_profiles/bad.agent.yaml"),
        profile,
    );
    case
}

fn populate_pair(root: &Path) {
    pair_case(root, "int", profile_with_int_abort("bad").as_bytes());
    pair_case(root, "bool", profile_with_bool_abort("bad").as_bytes());
    pair_case(root, "nonmapping", b"- one\n- two\n");
    pair_case(root, "syntax", b"profile-id: [unclosed\n");
    pair_case(root, "invalid_utf8", b"profile-id: bad\xff\n");
    let text = profile_doc("bad");
    for (name, little) in [("utf16le", true), ("utf16be", false)] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in text.encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        pair_case(root, name, &bytes);
    }
}

fn native_pair_receipts(root: &Path) -> BTreeMap<String, String> {
    let mut receipts = BTreeMap::new();
    for name in [
        "int",
        "bool",
        "nonmapping",
        "syntax",
        "invalid_utf8",
        "utf16le",
        "utf16be",
    ] {
        let case = root.join("cases").join(name);
        let outcome = match service_for(&case).profile_repository() {
            Ok(repo) if name.starts_with("utf16") => {
                let state = if repo.get_str("bad").is_some() {
                    "present"
                } else {
                    "missing"
                };
                format!("ok:bad-{state}")
            }
            Ok(_) => "ok".to_owned(),
            Err(error) => match error.abort() {
                Some(abort) => format!("abort:{}", abort.python),
                None => "abort:plain".to_owned(),
            },
        };
        receipts.insert(format!("repo_{name}"), outcome);
    }
    for name in ["int", "bool", "invalid_utf8", "syntax", "nonmapping"] {
        let case = root.join("cases").join(name);
        let outcome = match resolve_effective_sets(
            &case,
            &["activated_agent_profiles"],
            &NATIVE_OFFERING_SERVICE,
        ) {
            Ok(sets) => format!(
                "ok:{}",
                if sets[0].resolved {
                    "resolved"
                } else {
                    "unresolved"
                }
            ),
            Err(EffectiveSetError::Fatal(abort)) => format!("raise:{}", abort.python),
            Err(EffectiveSetError::Native(_)) => "raise:Native".to_owned(),
            Err(EffectiveSetError::UnknownKey(_)) => "raise:UnknownKey".to_owned(),
        };
        receipts.insert(format!("eff_{name}"), outcome);
    }
    receipts
}

/// The Source child: same fixture, same selected keys, its own environment assertions.
const SOURCE_PAIR_PROBE: &str = r##"
import os
import pathlib
import sys

ROOT = pathlib.Path(os.environ["CF86_BUSINESS_ROOT"]).resolve()
REPO = pathlib.Path(os.environ["CF86_EXPECTED_SOURCE_ROOT"]).resolve()


def check(condition, message):
    if not condition:
        raise SystemExit(f"CHILD-ENV-ASSERTION-FAILED: {message}")


check(ROOT.is_absolute() and ROOT.is_dir(), "business root")
check(pathlib.Path.cwd() == ROOT, "cwd")
check(pathlib.Path(os.environ["HOME"]).resolve() == ROOT / "home", "HOME")
for key, name in [
    ("XDG_CACHE_HOME", "xdg-cache"), ("XDG_CONFIG_HOME", "xdg-config"),
    ("XDG_DATA_HOME", "xdg-data"), ("XDG_STATE_HOME", "xdg-state"),
    ("XDG_RUNTIME_DIR", "xdg-runtime"), ("XDG_CONFIG_DIRS", "xdg-config-dirs"),
    ("XDG_DATA_DIRS", "xdg-data-dirs"), ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    check(pathlib.Path(os.environ[key]).resolve() == ROOT / name, key)
check(pathlib.Path(os.environ["PYTHONPATH"]).resolve() == REPO / "src", "PYTHONPATH")
check(pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]).resolve() == REPO / "packs", "packs")
check(os.environ["PYTHONHASHSEED"] == "0", "hash seed")
check(sys.version.split()[0] == "3.11.15", "interpreter")
check((sys.getfilesystemencoding(), sys.getfilesystemencodeerrors()) == ("utf-8", "surrogateescape"), "filesystem codec")
check((sys.stderr.encoding, sys.stderr.errors) == ("utf-8", "backslashreplace"), "stderr codec")
import charter
check(pathlib.Path(charter.__file__).resolve().is_relative_to(REPO / "src"), "charter origin")
check(os.environ.get("CF86_ORACLE_CHECKED") == "53", "oracle marker")

from charter.offering.agent_profiles.repository import AgentProfileRepository
from charter.activation.effective_set import resolve_effective_sets


def repository(case):
    try:
        repo = AgentProfileRepository(
            built_in_dir=REPO / "packs/built-in/agent_profiles",
            org_dirs=[case / "packs/acme/agent_profiles"],
        )
        if case.name.startswith("utf16"):
            return "ok:bad-present" if repo.get("bad") is not None else "ok:bad-missing"
        return "ok"
    except BaseException as exc:
        return f"abort:{type(exc).__name__}"


def effective(case):
    try:
        result = resolve_effective_sets(case, ["activated_agent_profiles"])
    except BaseException as exc:
        return f"raise:{type(exc).__name__}"
    entry = result["activated_agent_profiles"]
    return "ok:resolved" if entry.resolved else "ok:unresolved"


for name in ["int", "bool", "nonmapping", "syntax", "invalid_utf8", "utf16le", "utf16be"]:
    print(f"CF86_SOURCE repo_{name}={repository(ROOT / 'cases' / name)}")
for name in ["int", "bool", "invalid_utf8", "syntax", "nonmapping"]:
    print(f"CF86_SOURCE eff_{name}={effective(ROOT / 'cases' / name)}")
"##;
