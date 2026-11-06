//! CF83 — typed Native offering/runtime-output failure propagation.
//!
//! Phase 1 (the red-first tests) calls only the **pre-existing** public API so the
//! healthy-cache/fallback behaviour and the mis-classified `HashSeedError` are red
//! for the right reason: the current production path stores the delivery failure,
//! publishes the resolved ids and returns `Ok`. Phase 2 adds the typed route
//! (`GateFailure`/`NativeOfferingFailure` -> `OfferingLoadError` -> `ServiceKeys` ->
//! `EffectiveSetError`) and the real service/effective-set consumer assertions; those
//! additions are not retroactively claimed as the original red.
//!
//! Source controls here are Source-only measurements (invalid `PYTHONIOENCODING` /
//! `PYTHONHASHSEED` die before business; a Source-valid codec such as `cp1252`
//! emits). The Native side of a Source-valid codec is an explicit **negative gap**,
//! never an equivalent-output pair.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_charter::ArtifactKind;
use sk_doctrine::{
    ActivationGate, ActivationReadError, CanonicalOfferingService, NativeOfferingFailure,
    OfferingContext,
};
use sk_kernel::PythonString;
use sk_kernel::python_stream::StreamError;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn built_in_root() -> PathBuf {
    repository_root().join("packs/built-in")
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn text(value: &str) -> PythonString {
    PythonString::from_scalars(value)
}

/// One independent business tree: pack wiring, one known tactic and one unknown
/// activation token (the warning source the delivery failure aborts on).
fn populate(root: &Path) {
    for name in [
        "home",
        "xdg-cache",
        "xdg-config",
        "xdg-data",
        "xdg-state",
        "xdg-runtime",
        "xdg-config-dirs",
        "xdg-data-dirs",
        "tmp",
    ] {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let case = root.join("case");
    write(
        &case.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(&case.join("packs/acme/org-charter.yaml"), "{}\n");
    let tactic =
        fs::read_to_string(built_in_root().join("tactics/moscow-scoping-lens.tactic.yaml"))
            .expect("built-in tactic");
    write(
        &case.join("packs/acme/tactics/alpha.tactic.yaml"),
        &tactic.replace("id: moscow-scoping-lens", "id: alpha-tactic"),
    );
    write(
        &case.join(".kittify/charter/activation.yaml"),
        "activated_tactics: [alpha, ghost]\n",
    );
}

fn child_environment(command: &mut Command, root: &Path, seed: &str) {
    let repo = repository_root();
    let case = root.join("case");
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(&case)
        .env("HOME", root.join("home"))
        .env("XDG_CACHE_HOME", root.join("xdg-cache"))
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_DATA_HOME", root.join("xdg-data"))
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .env("XDG_RUNTIME_DIR", root.join("xdg-runtime"))
        .env("XDG_CONFIG_DIRS", root.join("xdg-config-dirs"))
        .env("XDG_DATA_DIRS", root.join("xdg-data-dirs"))
        .env("TMPDIR", root.join("tmp"))
        .env("TMP", root.join("tmp"))
        .env("TEMP", root.join("tmp"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONHASHSEED", seed)
        .env("CF83_EXPECTED_SOURCE_ROOT", &repo)
        .env("CF83_SOURCE_PYTHON", support::python())
        .env("CF83_ORACLE_CHECKED", "53")
        .env("CF83_CASE_ROOT", &case)
        .env("CF83_BUSINESS_ROOT", root);
}

/// Both children assert their own absolute roots/environment before the first read.
fn assert_child_environment(business_root: &Path, seed: &str) {
    let repo = repository_root();
    let case = business_root.join("case");
    assert!(business_root.is_absolute() && business_root.is_dir());
    assert_eq!(std::env::current_dir().unwrap(), case);
    assert_eq!(
        Path::new(&std::env::var("HOME").unwrap()),
        business_root.join("home")
    );
    for (key, name) in [
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
    ] {
        assert_eq!(
            Path::new(&std::env::var(key).unwrap()),
            business_root.join(name),
            "{key}"
        );
    }
    assert_eq!(
        std::env::var("PYTHONPATH").unwrap(),
        repo.join("src").display().to_string()
    );
    assert_eq!(
        std::env::var("SPEC_KITTY_PACKS_ROOT").unwrap(),
        repo.join("packs").display().to_string()
    );
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), seed);
    assert!(Path::new(&std::env::var("CF83_SOURCE_PYTHON").unwrap()).is_file());
    assert_eq!(std::env::var("CF83_ORACLE_CHECKED").unwrap(), "53");
    assert_eq!(
        std::env::var_os("CF83_EXPECTED_SOURCE_ROOT").map(PathBuf::from),
        Some(repo)
    );
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
}

fn receipts(stdout: &str) -> BTreeMap<String, String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("CF83_RECEIPT "))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.trim().to_owned()))
        .collect()
}

/// The central classification: a Source-valid codec/handler this composition does
/// not carry is a **Native capability gap**, not a Source startup fatal.
#[test]
fn stream_failure_classification_separates_startup_from_native_gaps() {
    let unsupported_codec = StreamError::UnsupportedEncoding {
        name: "cp1252".to_owned(),
        detail: "native composition lacks the codec".to_owned(),
    };
    assert!(
        !unsupported_codec.is_stream_startup_failure(),
        "a Source-valid codec the native stream cannot carry is not a Source startup fatal"
    );
    assert!(
        !StreamError::UnsupportedHandler {
            name: "namereplace"
        }
        .is_stream_startup_failure(),
        "a Source-valid error handler the native encoder lacks is not a Source startup fatal"
    );
    assert!(
        StreamError::InvalidStartup {
            detail: "PYTHONUTF8".to_owned()
        }
        .is_stream_startup_failure()
    );
    assert!(
        !StreamError::Io {
            class: sk_kernel::python_stream::WriteClass::Stderr,
            detail: "No space left on device".to_owned()
        }
        .is_stream_startup_failure(),
        "a physical write failure is caught by Handler.emit"
    );
    assert!(
        !StreamError::BrokenPipe {
            class: sk_kernel::python_stream::WriteClass::Stderr
        }
        .is_stream_startup_failure()
    );
}

/// The pre-existing production path: a delivery escape must never be published as
/// healthy membership, and a second call must reattempt, not hit a cache.
#[test]
fn native_delivery_escape_is_a_typed_failure_for_gate_service_and_effective_set() {
    if let Some(mode) = std::env::var_os("CF83_NATIVE_MODE") {
        let business_root = PathBuf::from(std::env::var("CF83_BUSINESS_ROOT").unwrap());
        let seed = std::env::var("CF83_CHILD_SEED").unwrap();
        assert_child_environment(&business_root, &seed);
        let case = business_root.join("case");
        match mode.to_string_lossy().as_ref() {
            "gate" | "cp1252" => gate_child(&case),
            "service" => service_child(&case),
            "effective" => effective_child(&case),
            other => panic!("unknown native mode {other}"),
        }
        return;
    }

    let business_root = tempfile::tempdir().unwrap();
    populate(business_root.path());

    let run = |mode: &str, codec: &str| {
        let mut command = Command::new(std::env::current_exe().unwrap());
        child_environment(&mut command, business_root.path(), "0");
        command
            .args([
                "--exact",
                "native_delivery_escape_is_a_typed_failure_for_gate_service_and_effective_set",
                "--nocapture",
            ])
            .env("CF83_NATIVE_MODE", mode)
            .env("CF83_CHILD_SEED", "0")
            .env("PYTHONIOENCODING", codec);
        let output = command.output().expect("run the native child");
        assert!(
            output.status.success(),
            "{mode} child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (
            receipts(&String::from_utf8(output.stdout).unwrap()),
            String::from_utf8(output.stderr).unwrap(),
        )
    };

    // A Source-valid codec this native composition does not carry: the reference
    // runs and warns, native cannot deliver. This is a negative control, not a pair.
    let (cp1252, cp1252_stderr) = run("cp1252", "cp1252");
    assert_eq!(cp1252_stderr, "", "no bytes are written on the native gap");
    assert_eq!(
        cp1252.get("result").map(String::as_str),
        Some("err"),
        "a Source-valid codec the native stream lacks is an explicit typed gap: {cp1252:?}"
    );
    assert_eq!(
        cp1252.get("cause").map(String::as_str),
        Some("native_warning_delivery"),
        "the gap is a typed Native delivery failure, not a Source-abort label: {cp1252:?}"
    );

    for mode in ["gate", "service", "effective"] {
        let (receipt, stderr) = run(mode, "not-a-real-codec");
        assert_eq!(stderr, "", "{mode}: the escaped phase writes nothing");
        assert_eq!(
            receipt.get("result").map(String::as_str),
            Some("err"),
            "{mode}: a delivery escape must return a typed failure, not healthy membership: {receipt:?}"
        );
        assert_eq!(
            receipt.get("retry").map(String::as_str),
            Some("err"),
            "{mode}: a retry reattempts delivery instead of hitting a published cache: {receipt:?}"
        );
        assert_eq!(
            receipt.get("cause").map(String::as_str),
            Some("native_warning_delivery"),
            "{mode}: the typed Native delivery cause is preserved: {receipt:?}"
        );
        assert_eq!(
            receipt.get("evidence").map(String::as_str),
            Some("some"),
            "{mode}: the gate's delivery/escape evidence is retained: {receipt:?}"
        );
        assert!(
            !receipt.contains_key("membership"),
            "{mode}: no membership is exposed with the failure: {receipt:?}"
        );
    }
}

/// The start-up set-policy cause is typed Native, not the fabricated Source class
/// `HashSeedError` (CPython refuses to start, so no such exception exists).
#[test]
fn invalid_hash_seed_is_a_typed_native_startup_cause() {
    if std::env::var_os("CF83_NATIVE_MODE").is_some_and(|mode| mode == "hashseed") {
        let business_root = PathBuf::from(std::env::var("CF83_BUSINESS_ROOT").unwrap());
        let seed = std::env::var("CF83_CHILD_SEED").unwrap();
        assert_child_environment(&business_root, &seed);
        let case = business_root.join("case");
        match ActivationGate::try_from_project(
            &case,
            &built_in_root(),
            &[case.join("packs/acme")],
            None,
        ) {
            Ok(_) => println!("CF83_RECEIPT gate=ok"),
            Err(ActivationReadError::Native(NativeOfferingFailure::ActivationSetOrder {
                phase,
                cause,
                ..
            })) => println!(
                "CF83_RECEIPT gate=native_set_order phase={phase} cause={}",
                format!("{cause}").lines().next().unwrap_or("HashSeedError")
            ),
            Err(ActivationReadError::Aborted { python, .. }) => {
                println!("CF83_RECEIPT gate=aborted:{python}")
            }
            Err(other) => println!("CF83_RECEIPT gate=other:{other}"),
        }
        match sk_doctrine::resolve_effective_sets(
            &case,
            &["activated_tactics"],
            &sk_doctrine::NATIVE_OFFERING_SERVICE,
        ) {
            Ok(_) => println!("CF83_RECEIPT effective=ok"),
            Err(error) if error.is_native() => println!("CF83_RECEIPT effective=native"),
            Err(error) => println!("CF83_RECEIPT effective=other:{error}"),
        }
        return;
    }

    let business_root = tempfile::tempdir().unwrap();
    populate(business_root.path());
    let mut command = Command::new(std::env::current_exe().unwrap());
    child_environment(&mut command, business_root.path(), "bogus");
    command
        .args([
            "--exact",
            "invalid_hash_seed_is_a_typed_native_startup_cause",
            "--nocapture",
        ])
        .env("CF83_NATIVE_MODE", "hashseed")
        .env("CF83_CHILD_SEED", "bogus");
    let output = command.output().expect("run the hashseed child");
    assert!(
        output.status.success(),
        "hashseed child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = receipts(&String::from_utf8(output.stdout).unwrap());
    let gate = receipt.get("gate").map(String::as_str).unwrap_or_default();
    assert!(
        gate.starts_with("native_set_order "),
        "the gate keeps the Kernel HashSeedError as a typed Native start-up cause: {receipt:?}"
    );
    assert!(
        gate.contains("config_init_hash_seed"),
        "the original Kernel fatal text is retained: {receipt:?}"
    );
    assert_eq!(
        receipt.get("effective").map(String::as_str),
        Some("native"),
        "the effective-set pass propagates the Native cause instead of unresolved/fallback health: {receipt:?}"
    );
}

/// Child body: the gate alone. Receipts are the observable production behaviour.
fn gate_child(case: &Path) {
    let gate =
        ActivationGate::try_from_project(case, &built_in_root(), &[case.join("packs/acme")], None)
            .expect("gate builds");
    let items: BTreeMap<PythonString, ()> = [(text("alpha-tactic"), ())].into_iter().collect();
    let first = gate.filter(ArtifactKind::Tactic, &items);
    let retry = gate.filter(ArtifactKind::Tactic, &items);
    println!(
        "CF83_RECEIPT result={}",
        if first.is_ok() { "ok" } else { "err" }
    );
    if let Ok(ids) = &first {
        println!(
            "CF83_RECEIPT membership={}",
            ids.iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    println!(
        "CF83_RECEIPT retry={}",
        if retry.is_ok() { "ok" } else { "err" }
    );
    println!(
        "CF83_RECEIPT cause={}",
        match &first {
            Err(failure) if failure.native().is_some() => "native_warning_delivery",
            Err(_) => "constructor",
            Ok(_) => "none",
        }
    );
    println!(
        "CF83_RECEIPT evidence={}",
        if gate.warning_log().escaped().is_some() {
            "some"
        } else {
            "none"
        }
    );
}

/// Child body: the real canonical offering service (`ids_for`), the production
/// consumer of the gate's filtering route.
fn service_child(case: &Path) {
    let context = OfferingContext::resolve(case).expect("offering context");
    let service = CanonicalOfferingService::new(context);
    let first = service.ids_for("activated_tactics");
    let retry = service.ids_for("activated_tactics");
    println!(
        "CF83_RECEIPT result={}",
        if first.is_ok() { "ok" } else { "err" }
    );
    if let Ok(ids) = &first {
        println!(
            "CF83_RECEIPT membership={}",
            ids.iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    println!(
        "CF83_RECEIPT retry={}",
        if retry.is_ok() { "ok" } else { "err" }
    );
    let (cause, evidence) = match &first {
        Err(error) => match error.native() {
            Some(NativeOfferingFailure::ActivationWarningDelivery { log, .. }) => (
                "native_warning_delivery",
                if log.escaped().is_some() {
                    "some"
                } else {
                    "none"
                },
            ),
            Some(_) => ("native_other", "none"),
            None if error.abort().is_some() => ("constructor", "none"),
            None => ("failed", "none"),
        },
        Ok(_) => ("none", "none"),
    };
    println!("CF83_RECEIPT cause={cause}");
    println!("CF83_RECEIPT evidence={evidence}");
}

/// Child body: the strict/fallback effective-set pass, the other real consumer.
fn effective_child(case: &Path) {
    let first = sk_doctrine::resolve_effective_sets(
        case,
        &["activated_tactics"],
        &sk_doctrine::NATIVE_OFFERING_SERVICE,
    );
    let retry = sk_doctrine::resolve_effective_sets(
        case,
        &["activated_tactics"],
        &sk_doctrine::NATIVE_OFFERING_SERVICE,
    );
    println!(
        "CF83_RECEIPT result={}",
        if first.is_ok() { "ok" } else { "err" }
    );
    if let Ok(sets) = &first {
        println!(
            "CF83_RECEIPT membership={}",
            sets.iter()
                .map(|set| {
                    format!(
                        "{}:{}:{}",
                        set.yaml_key,
                        set.resolved,
                        set.ids
                            .iter()
                            .map(|id| id.to_scalar_string().unwrap())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .collect::<Vec<_>>()
                .join("|")
        );
    }
    println!(
        "CF83_RECEIPT retry={}",
        if retry.is_ok() { "ok" } else { "err" }
    );
    let cause = match &first {
        Err(error) if error.is_native() => "native_warning_delivery",
        Err(_) => "other",
        Ok(_) => "none",
    };
    println!("CF83_RECEIPT cause={cause}");
    let evidence = match &first {
        Err(error) => match error.native() {
            Some(NativeOfferingFailure::ActivationWarningDelivery { log, .. })
                if log.escaped().is_some() =>
            {
                "some"
            }
            _ => "none",
        },
        Ok(_) => "none",
    };
    println!("CF83_RECEIPT evidence={evidence}");
}

/// Source-only control: an invalid start-up stream/hash seed dies before business,
/// while a Source-valid codec (`cp1252`) runs and emits. This is **not** a
/// Source/Native paired green: native carries neither codec, which is an explicit
/// unaccepted capability gap measured separately.
#[test]
fn guarded_source_stream_controls_are_source_only() {
    const PROBE: &str = r#"
import logging, os, pathlib, sys
bus = pathlib.Path(os.environ["CF83_BUSINESS_ROOT"])
assert bus.is_absolute() and bus.is_dir()
for key, name in [("HOME", "home"), ("XDG_CACHE_HOME", "xdg-cache"),
    ("XDG_CONFIG_HOME", "xdg-config"), ("XDG_DATA_HOME", "xdg-data"),
    ("XDG_STATE_HOME", "xdg-state"), ("XDG_RUNTIME_DIR", "xdg-runtime"),
    ("XDG_CONFIG_DIRS", "xdg-config-dirs"), ("XDG_DATA_DIRS", "xdg-data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp")]:
    assert pathlib.Path(os.environ[key]) == bus / name, (key, os.environ[key])
assert sys.version_info[:3] == (3, 11, 15), sys.version
root = pathlib.Path(os.environ["CF83_EXPECTED_SOURCE_ROOT"])
assert root.is_absolute()
assert pathlib.Path(os.environ["PYTHONPATH"]) == root / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == root / "packs"
assert os.environ["CF83_ORACLE_CHECKED"] == "53"
assert str(pathlib.Path(sys.executable)) == os.environ["CF83_SOURCE_PYTHON"]
assert pathlib.Path.cwd() == bus
print("CF83_SOURCE_RECEIPT environment=guarded", flush=True)
print("CF83_SOURCE_RECEIPT business=reached", flush=True)
plan = os.environ["CF83_PLAN"]
if plan == "default":
    logging.warning("warning-line")
elif plan == "cp1252":
    logging.warning("caf\u00e9-line")
else:
    raise SystemExit(1)
print("CF83_SOURCE_RECEIPT done=1", flush=True)
"#;

    let python = support::python();
    let repo = repository_root();
    let root = tempfile::tempdir().unwrap();
    for name in [
        "home",
        "xdg-cache",
        "xdg-config",
        "xdg-data",
        "xdg-state",
        "xdg-runtime",
        "xdg-config-dirs",
        "xdg-data-dirs",
        "tmp",
    ] {
        fs::create_dir_all(root.path().join(name)).unwrap();
    }
    let run = |plan: &str, codec: Option<&str>, seed: &str| {
        let mut command = Command::new(&python);
        command
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .current_dir(root.path())
            .env("HOME", root.path().join("home"))
            .env("XDG_CACHE_HOME", root.path().join("xdg-cache"))
            .env("XDG_CONFIG_HOME", root.path().join("xdg-config"))
            .env("XDG_DATA_HOME", root.path().join("xdg-data"))
            .env("XDG_STATE_HOME", root.path().join("xdg-state"))
            .env("XDG_RUNTIME_DIR", root.path().join("xdg-runtime"))
            .env("XDG_CONFIG_DIRS", root.path().join("xdg-config-dirs"))
            .env("XDG_DATA_DIRS", root.path().join("xdg-data-dirs"))
            .env("TMPDIR", root.path().join("tmp"))
            .env("TMP", root.path().join("tmp"))
            .env("TEMP", root.path().join("tmp"))
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONPATH", repo.join("src"))
            .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
            .env("PYTHONHASHSEED", seed)
            .env("CF83_PLAN", plan)
            .env("CF83_BUSINESS_ROOT", root.path())
            .env("CF83_EXPECTED_SOURCE_ROOT", &repo)
            .env("CF83_SOURCE_PYTHON", &python)
            .env("CF83_ORACLE_CHECKED", "53")
            .arg("-c")
            .arg(PROBE);
        if let Some(codec) = codec {
            command.env("PYTHONIOENCODING", codec);
        }
        command.output().expect("run the Source child")
    };

    let default = run("default", None, "0");
    assert!(default.status.success());
    assert_eq!(default.stdout, b"CF83_SOURCE_RECEIPT environment=guarded\nCF83_SOURCE_RECEIPT business=reached\nCF83_SOURCE_RECEIPT done=1\n");
    assert_eq!(default.stderr, b"WARNING:root:warning-line\n");

    let cp1252 = run("cp1252", Some("cp1252"), "0");
    assert!(cp1252.status.success());
    assert_eq!(
        cp1252.stdout, default.stdout,
        "the Source-valid codec run reaches business exactly like the default"
    );
    assert_eq!(
        cp1252.stderr, b"WARNING:root:caf\xe9-line\n",
        "the pinned interpreter really emits through cp1252"
    );

    let bogus_codec = run("default", Some("not-a-real-codec"), "0");
    assert_eq!(bogus_codec.status.code(), Some(1));
    assert!(bogus_codec.stdout.is_empty(), "no business ran");
    assert!(
        String::from_utf8_lossy(&bogus_codec.stderr)
            .contains("LookupError: unknown encoding: not-a-real-codec")
    );

    let bogus_seed = run("default", None, "bogus");
    assert_eq!(bogus_seed.status.code(), Some(1));
    assert!(bogus_seed.stdout.is_empty(), "no business ran");
    assert!(String::from_utf8_lossy(&bogus_seed.stderr).contains("config_init_hash_seed"));
}
