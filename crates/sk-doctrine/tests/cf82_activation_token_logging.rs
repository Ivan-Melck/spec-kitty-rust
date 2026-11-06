//! CF82 — activation-token warning text/order and the Source-default logger delivery.
//!
//! Phase 1 (this file's first two tests) calls ONLY the pre-existing public API so
//! that the missing delivery and the Rust-`Debug` warning text are red for the right
//! reason. Phase 2 adds the canonical set order, the one warning owner, the Kernel
//! default-logging transport and the real consumer controls.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_charter::ArtifactKind;
use sk_doctrine::{
    ActivationGate, CanonicalOfferingService, IdentityIndex, OfferingContext, WarningLog,
};
use sk_kernel::PythonString;
use sk_kernel::python_logging::{LoggingPolicy, Suppression, WARNING, WarningDelivery};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn built_in_root() -> PathBuf {
    repository_root().join("packs/built-in")
}

fn built_in(relative: &str) -> String {
    fs::read_to_string(built_in_root().join(relative))
        .unwrap_or_else(|error| panic!("read built-in {relative}: {error}"))
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

/// One independent business tree: pack wiring, ledger and the tactic files a case
/// needs. Every case is materialized by this single function, so the Source and
/// Native children read byte-identical relative layouts under different roots.
fn populate(root: &Path, case: &str) {
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
    let case_root = root.join(case);
    write(
        &case_root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(&case_root.join("packs/acme/org-charter.yaml"), "{}\n");
    let tactic = built_in("tactics/moscow-scoping-lens.tactic.yaml");
    let tactic_with = |id: &str| tactic.replace("id: moscow-scoping-lens", id);
    match case {
        "tokens" => {
            write(
                &case_root.join("packs/acme/tactics/alpha.tactic.yaml"),
                &tactic_with("id: alpha-tactic"),
            );
            let mut ledger = String::from("activated_tactics: [alpha");
            for number in 1..=12 {
                ledger.push_str(&format!(", t{number:02}"));
            }
            ledger.push_str("]\n");
            write(&case_root.join(".kittify/charter/activation.yaml"), &ledger);
        }
        "netting" => {
            write(
                &case_root.join(".kittify/charter/activation.yaml"),
                "activated_tactics: [t01, t02, t03, t04, t05, t06, t07, t08, t09, t10]\n\
                 deactivated_tactics: [t03, t07]\n",
            );
        }
        "known" => {
            write(
                &case_root.join("packs/acme/tactics/alpha.tactic.yaml"),
                &tactic_with("id: alpha-tactic"),
            );
            write(
                &case_root.join(".kittify/charter/activation.yaml"),
                "activated_tactics: [alpha]\n",
            );
        }
        "duplicates" => {
            write(
                &case_root.join(".kittify/charter/activation.yaml"),
                "activated_tactics: [t01, t02, t01, t03, t02]\n",
            );
        }
        "service_only" => {
            write(
                &case_root.join(".kittify/charter/activation.yaml"),
                "activated_tactics: [ghost]\n",
            );
        }
        "abort" => {
            write(
                &case_root.join("packs/acme/tactics/alpha.tactic.yaml"),
                &tactic_with("id: alpha-tactic"),
            );
            write(
                &case_root.join("packs/acme/tactics/bad.tactic.yaml"),
                &tactic_with("id: !!timestamp 2021-13-45"),
            );
            write(
                &case_root.join(".kittify/charter/activation.yaml"),
                "activated_tactics: [alpha]\n",
            );
        }
        other => panic!("unknown case {other}"),
    }
}

fn child_environment(command: &mut Command, root: &Path, case: &str, seed: &str) {
    let repo = repository_root();
    let case_root = root.join(case);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(&case_root)
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
        .env("CF82_EXPECTED_SOURCE_ROOT", &repo)
        .env("CF82_SOURCE_PYTHON", support::python())
        .env("CF82_ORACLE_CHECKED", "53")
        .env("CF82_CASE_ROOT", &case_root)
        .env("CF82_BUSINESS_ROOT", root)
        .env("CF82_CASE", case);
}

/// Both children assert their own absolute roots and independently supplied
/// expectations before the first business read.
fn assert_child_environment(case_root: &Path, business_root: &Path) {
    let repo = repository_root();
    assert!(case_root.is_absolute() && case_root.is_dir());
    assert!(business_root.is_absolute() && business_root.is_dir());
    assert_eq!(std::env::current_dir().unwrap(), case_root);
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
    assert!(
        matches!(
            std::env::var("PYTHONHASHSEED").unwrap().as_str(),
            "0" | "42"
        ),
        "the probe declares its seed"
    );
    assert!(Path::new(&std::env::var("CF82_SOURCE_PYTHON").unwrap()).is_file());
    assert_eq!(std::env::var("CF82_ORACLE_CHECKED").unwrap(), "53");
    assert_eq!(
        std::env::var_os("CF82_EXPECTED_SOURCE_ROOT").map(PathBuf::from),
        Some(repo)
    );
    // The actual filesystem codec this process will use for repository paths.
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
}

fn receipt_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("CF82_RECEIPT "))
        .map(|line| line.trim().to_owned())
        .collect()
}

const SOURCE_PROBE: &str = r#"
import json, os, pathlib, sys

bus = pathlib.Path(os.environ["CF82_BUSINESS_ROOT"])
case = pathlib.Path(os.environ["CF82_CASE_ROOT"])
case_name = os.environ["CF82_CASE"]
assert bus.is_absolute() and bus.is_dir(), bus
assert case.is_absolute() and case.is_dir(), case
assert pathlib.Path.cwd() == case, pathlib.Path.cwd()
assert pathlib.Path(os.environ["HOME"]) == bus / "home"
for key, name in [
    ("XDG_CACHE_HOME", "xdg-cache"), ("XDG_CONFIG_HOME", "xdg-config"),
    ("XDG_DATA_HOME", "xdg-data"), ("XDG_STATE_HOME", "xdg-state"),
    ("XDG_RUNTIME_DIR", "xdg-runtime"), ("XDG_CONFIG_DIRS", "xdg-config-dirs"),
    ("XDG_DATA_DIRS", "xdg-data-dirs"), ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == bus / name, (key, os.environ[key])
assert sys.version_info[:3] == (3, 11, 15), sys.version
expected_source = pathlib.Path(os.environ["CF82_EXPECTED_SOURCE_ROOT"])
assert expected_source.is_absolute()
assert pathlib.Path(os.environ["PYTHONPATH"]) == expected_source / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == expected_source / "packs"
assert os.environ["PYTHONHASHSEED"] in {"0", "42"}
assert pathlib.Path(os.environ["CF82_SOURCE_PYTHON"]).is_file()
assert str(pathlib.Path(sys.executable)) == os.environ["CF82_SOURCE_PYTHON"]
assert os.environ["CF82_ORACLE_CHECKED"] == "53"
assert sys.getfilesystemencoding() == "utf-8"
assert sys.getfilesystemencodeerrors() == "surrogateescape"
import charter
assert pathlib.Path(charter.__file__).resolve().is_relative_to(expected_source / "src")

from charter.activation.active_charter_service_builder import build_active_charter_service

def receipt(label, value):
    print("CF82_RECEIPT " + label + "=" + value)

service = build_active_charter_service(case)
receipt("tactics", ",".join(sorted(service.tactics)))
receipt("tactics_again", ",".join(sorted(service.tactics)))
receipt("tactics_fresh", ",".join(sorted(build_active_charter_service(case).tactics)))
"#;

/// The scalar warning is the reference's own `!r` form, not Rust `Debug`.
///
/// Every expected line below was measured from the pinned Source
/// (`identity_index.activation_id`) before this native change.
#[test]
fn scalar_activation_warning_uses_python_repr() {
    let index = IdentityIndex::from_roots(&built_in_root(), &[], None);
    let cases: [(&str, &str); 9] = [
        (
            "plain",
            "unresolved directive activation token 'plain'; best-effort resolved to 'PLAIN'",
        ),
        (
            "a'b",
            "unresolved directive activation token \"a'b\"; best-effort resolved to \"A'B\"",
        ),
        (
            "a\\b",
            "unresolved directive activation token 'a\\\\b'; best-effort resolved to 'A\\\\B'",
        ),
        (
            "a\nb",
            "unresolved directive activation token 'a\\nb'; best-effort resolved to 'A\\nB'",
        ),
        (
            "a\u{0}b",
            "unresolved directive activation token 'a\\x00b'; best-effort resolved to 'A\\x00B'",
        ),
        (
            "\u{e9}",
            "unresolved directive activation token '\u{e9}'; best-effort resolved to '\u{c9}'",
        ),
        (
            "\u{4dc0}",
            "unresolved directive activation token '\u{4dc0}'; best-effort resolved to '\u{4dc0}'",
        ),
        (
            "\u{e000}",
            "unresolved directive activation token '\\ue000'; best-effort resolved to '\\ue000'",
        ),
        (
            "a\"b",
            "unresolved directive activation token 'a\"b'; best-effort resolved to 'A\"B'",
        ),
    ];
    for (token, expected) in cases {
        let mut warnings = Vec::new();
        let resolved = index.activation_id(ArtifactKind::Directive, token, &mut warnings);
        assert_eq!(warnings.len(), 1, "one warning for {token:?}");
        assert_eq!(
            warnings[0], expected,
            "the reference's repr for {token:?} (resolved {resolved:?})"
        );
    }
}

/// The Source and Native business children are independently materialized, both
/// assert their own environment before their first read, and their exact warning
/// bytes, returned membership and cache behaviour agree.
#[test]
fn guarded_source_native_activation_warning_pair() {
    if let Some(case_root) = std::env::var_os("CF82_NATIVE_CASE_ROOT").map(PathBuf::from) {
        let business_root = PathBuf::from(std::env::var("CF82_BUSINESS_ROOT").unwrap());
        assert_child_environment(&case_root, &business_root);
        let context = OfferingContext::resolve(&case_root).expect("offering context");
        let service = CanonicalOfferingService::new(context);
        let ids = service
            .ids_for("activated_tactics")
            .expect("tactic mapping");
        println!(
            "CF82_RECEIPT tactics={}",
            ids.iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        );
        let again = service
            .ids_for("activated_tactics")
            .expect("cached mapping");
        println!(
            "CF82_RECEIPT tactics_again={}",
            again
                .iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        );
        let fresh = CanonicalOfferingService::new(
            OfferingContext::resolve(&case_root).expect("fresh offering context"),
        );
        let fresh_ids = fresh.ids_for("activated_tactics").expect("fresh mapping");
        println!(
            "CF82_RECEIPT tactics_fresh={}",
            fresh_ids
                .iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        );
        return;
    }

    let mut failures = Vec::new();
    for (case, seed) in [
        ("tokens", "0"),
        ("tokens", "42"),
        ("netting", "0"),
        ("duplicates", "0"),
    ] {
        let source_root = tempfile::tempdir().unwrap();
        populate(source_root.path(), case);
        let mut source_command = Command::new(support::python());
        child_environment(&mut source_command, source_root.path(), case, seed);
        source_command.arg("-c").arg(SOURCE_PROBE);
        let source_output = source_command.output().expect("run the Source child");
        assert!(
            source_output.status.success(),
            "Source child failed for {case}/{seed}: {}",
            String::from_utf8_lossy(&source_output.stderr)
        );
        let source_receipts = receipt_lines(&String::from_utf8(source_output.stdout).unwrap());

        let native_root = tempfile::tempdir().unwrap();
        populate(native_root.path(), case);
        let native_case_root = native_root.path().join(case);
        let mut native_command = Command::new(std::env::current_exe().unwrap());
        child_environment(&mut native_command, native_root.path(), case, seed);
        native_command
            .args([
                "--exact",
                "guarded_source_native_activation_warning_pair",
                "--nocapture",
            ])
            .env("CF82_NATIVE_CASE_ROOT", &native_case_root);
        let native_output = native_command.output().expect("run the Native child");
        assert!(
            native_output.status.success(),
            "Native child failed for {case}/{seed}: {}",
            String::from_utf8_lossy(&native_output.stderr)
        );
        let native_receipts = receipt_lines(&String::from_utf8(native_output.stdout).unwrap());

        assert_eq!(
            source_receipts, native_receipts,
            "returned membership/cache receipts for {case}/{seed}"
        );
        if source_output.stderr != native_output.stderr {
            failures.push(format!(
                "{case}/{seed}: Source stderr {:?} vs Native stderr {:?}",
                String::from_utf8_lossy(&source_output.stderr),
                String::from_utf8_lossy(&native_output.stderr)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "the reference delivers its activation warnings on the default logger; native did not match:\n{}",
        failures.join("\n")
    );
}

/// The delivery controls: cache-hit silence, a fresh service's repeat warning, the
/// two measured `logging` suppressions, the service-only bypass, a constructor
/// abort, and the start-up-class escape.
///
/// One child body runs three times on its own materialized tree: the measured
/// default, the two policy suppressions, and an unusable `PYTHONIOENCODING` (the
/// start-up class the reference can never reach). The parent compares the receipts
/// and the exact stderr bytes of each run.
#[test]
fn native_delivery_controls() {
    let Some(mode) = std::env::var_os("CF82_NATIVE_CONTROLS") else {
        let business_root = tempfile::tempdir().unwrap();
        for case in ["netting", "service_only", "abort"] {
            populate(business_root.path(), case);
        }

        let run = |label: &str, codec: Option<&str>, full_stderr: bool| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            child_environment(&mut command, business_root.path(), "netting", "0");
            command
                .args(["--exact", "native_delivery_controls", "--nocapture"])
                .env("CF82_NATIVE_CONTROLS", label);
            if let Some(codec) = codec {
                command.env("PYTHONIOENCODING", codec);
            }
            if full_stderr {
                // The measured handler-error surface: a real full device behind fd 2.
                let device = std::fs::File::options()
                    .write(true)
                    .open("/dev/full")
                    .expect("/dev/full");
                command.stderr(std::process::Stdio::from(device));
            }
            let output = command.output().expect("run the controls child");
            assert!(
                output.status.success(),
                "{label} child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            (
                receipt_lines(&String::from_utf8(output.stdout).unwrap()),
                String::from_utf8(output.stderr).unwrap(),
            )
        };
        let receipt = |lines: &[String], key: &str| {
            lines
                .iter()
                .find_map(|line| line.strip_prefix(&format!("{key}=")))
                .unwrap_or_else(|| panic!("missing receipt {key} in {lines:?}"))
                .to_owned()
        };

        let (default, default_stderr) = run("default", None, false);
        let lines: Vec<&str> = default_stderr.lines().collect();
        assert_eq!(
            lines.len(),
            16,
            "8 delivered warnings for the first resolution and 8 again for a fresh gate:\n{default_stderr}"
        );
        for line in &lines {
            assert!(
                line.starts_with("unresolved tactic activation token 't")
                    && line.contains("; best-effort resolved to 't"),
                "unexpected stderr line {line:?}"
            );
        }
        assert_eq!(
            lines[..8],
            lines[8..],
            "a fresh service re-delivers the same canonical order"
        );
        assert_eq!(receipt(&default, "delivered"), "8");
        assert_eq!(receipt(&default, "handled"), "0");
        assert_eq!(receipt(&default, "suppressed_disabled"), "0");
        assert_eq!(receipt(&default, "cached_outcomes"), "0");
        assert_eq!(receipt(&default, "fresh_delivered"), "8");
        assert_eq!(receipt(&default, "escaped"), "none");
        assert_eq!(receipt(&default, "service_only_membership"), "ghost");
        assert_eq!(receipt(&default, "service_only_outcomes"), "0");
        assert_eq!(receipt(&default, "abort_type"), "ValueError");
        assert_eq!(receipt(&default, "abort_outcomes"), "0");

        let (suppressed, suppressed_stderr) = run("suppressed", None, false);
        assert_eq!(suppressed_stderr, "", "policy suppression writes nothing");
        assert_eq!(receipt(&suppressed, "delivered"), "0");
        assert_eq!(receipt(&suppressed, "fresh_delivered"), "0");
        assert_eq!(receipt(&suppressed, "cached_outcomes"), "0");
        assert_eq!(receipt(&suppressed, "suppressed_disabled"), "8");
        assert_eq!(receipt(&suppressed, "suppressed_last_resort"), "8");
        assert_eq!(
            receipt(&suppressed, "membership"),
            receipt(&default, "membership"),
            "suppression never changes what the gate returns"
        );

        let (escaped, escaped_stderr) = run("escaped", Some("not-a-real-codec"), false);
        assert_eq!(escaped_stderr, "", "the escaped phase writes nothing");
        assert_ne!(receipt(&escaped, "escaped"), "none");
        assert_eq!(receipt(&escaped, "delivered"), "0");
        assert_eq!(receipt(&escaped, "handled"), "0");
        assert_eq!(
            receipt(&escaped, "result"),
            "err",
            "a delivery that cannot happen is a typed failure (CF83), not healthy membership"
        );
        assert_eq!(receipt(&escaped, "native"), "true");
        assert_eq!(
            receipt(&escaped, "membership"),
            "err",
            "no resolved membership is published on the escape"
        );
        assert_eq!(
            receipt(&escaped, "retry_is_err"),
            "true",
            "a retry reattempts delivery instead of hitting a published cache"
        );

        // A real full stderr device: `Handler.emit` fails, `handleError` swallows
        // it, and the reference's caller continues — so the gate still publishes.
        let (handled, _) = run("default", None, true);
        assert_eq!(
            receipt(&handled, "delivered"),
            "0",
            "a full device delivers nothing"
        );
        assert_eq!(receipt(&handled, "handled"), "8");
        assert_eq!(
            receipt(&handled, "membership"),
            receipt(&default, "membership"),
            "a handler-handled failure still publishes the resolution"
        );
        return;
    };

    // Child: one business tree; the run mode only selects the process logging
    // policy the reference would already have configured.
    let mode = mode.to_string_lossy().into_owned();
    let business_root = PathBuf::from(std::env::var("CF82_BUSINESS_ROOT").unwrap());
    let case_root = PathBuf::from(std::env::var("CF82_CASE_ROOT").unwrap());
    assert_child_environment(&case_root, &business_root);

    let policy = match mode.as_str() {
        "suppressed" => LoggingPolicy::source_default().with_disabled_level(WARNING),
        _ => LoggingPolicy::source_default(),
    };
    let netting = business_root.join("netting");
    let gate = || {
        ActivationGate::try_from_project(
            &netting,
            &built_in_root(),
            &[netting.join("packs/acme")],
            None,
        )
        .expect("gate builds")
        .with_logging_policy(policy)
    };
    let count = |log: &WarningLog, wanted: fn(&WarningDelivery) -> bool| {
        log.outcomes()
            .iter()
            .filter(|outcome| wanted(outcome))
            .count()
    };
    let empty: BTreeMap<PythonString, ()> = BTreeMap::new();

    let first = gate();
    let first_result = first.filter(ArtifactKind::Tactic, &empty);
    let kept = match first_result {
        Ok(kept) => kept,
        Err(failure) => {
            // CF83: a delivery that cannot happen is a typed Err and publishes no
            // cache; the delivery/escape evidence is retained on the gate.
            println!("CF82_RECEIPT result=err");
            println!("CF82_RECEIPT native={}", failure.native().is_some());
            println!(
                "CF82_RECEIPT delivered={}",
                count(&first.warning_log(), |outcome| matches!(
                    outcome,
                    WarningDelivery::Delivered
                ))
            );
            println!(
                "CF82_RECEIPT handled={}",
                count(&first.warning_log(), |outcome| matches!(
                    outcome,
                    WarningDelivery::Handled { .. }
                ))
            );
            println!(
                "CF82_RECEIPT escaped={}",
                match first.warning_log().escaped() {
                    Some(error) => error
                        .to_string()
                        .lines()
                        .next()
                        .unwrap_or("escaped")
                        .to_owned(),
                    None => "none".to_owned(),
                }
            );
            println!("CF82_RECEIPT membership=err");
            let retry = first.filter(ArtifactKind::Tactic, &empty);
            println!("CF82_RECEIPT retry_is_err={}", retry.is_err());
            return;
        }
    };
    println!(
        "CF82_RECEIPT delivered={}",
        count(&first.warning_log(), |outcome| matches!(
            outcome,
            WarningDelivery::Delivered
        ))
    );
    println!(
        "CF82_RECEIPT handled={}",
        count(&first.warning_log(), |outcome| matches!(
            outcome,
            WarningDelivery::Handled { .. }
        ))
    );
    println!(
        "CF82_RECEIPT suppressed_disabled={}",
        count(&first.warning_log(), |outcome| matches!(
            outcome,
            WarningDelivery::Suppressed(Suppression::Disabled)
        ))
    );
    println!("CF82_RECEIPT membership={}", kept.len());
    println!(
        "CF82_RECEIPT escaped={}",
        match first.warning_log().escaped() {
            Some(error) => error
                .to_string()
                .lines()
                .next()
                .unwrap_or("escaped")
                .to_owned(),
            None => "none".to_owned(),
        }
    );
    let before = first.warning_log().outcomes().len();
    let again = first
        .filter(ArtifactKind::Tactic, &empty)
        .expect("cached resolution");
    assert_eq!(again, kept, "the cache returns the same membership");
    println!(
        "CF82_RECEIPT cached_outcomes={}",
        first.warning_log().outcomes().len() - before
    );
    let fresh = gate();
    fresh
        .filter(ArtifactKind::Tactic, &empty)
        .expect("fresh resolution");
    println!(
        "CF82_RECEIPT fresh_delivered={}",
        count(&fresh.warning_log(), |outcome| matches!(
            outcome,
            WarningDelivery::Delivered
        ))
    );

    let last_resort = gate().with_logging_policy(
        LoggingPolicy::source_default().with_last_resort_level(sk_kernel::python_logging::ERROR),
    );
    let last_resort_kept = last_resort
        .filter(ArtifactKind::Tactic, &empty)
        .expect("lastResort policy");
    assert_eq!(
        last_resort_kept, kept,
        "a lastResort level never changes the returned membership"
    );
    println!(
        "CF82_RECEIPT suppressed_last_resort={}",
        count(&last_resort.warning_log(), |outcome| matches!(
            outcome,
            WarningDelivery::Suppressed(Suppression::LastResortLevel)
        ))
    );

    let service_only = business_root.join("service_only");
    let service_gate = ActivationGate::try_from_project(
        &service_only,
        &built_in_root(),
        &[service_only.join("packs/acme")],
        None,
    )
    .expect("service-only gate");
    let ghost: BTreeMap<PythonString, ()> = [(text("ghost"), ())].into_iter().collect();
    let ghost_kept = service_gate
        .filter(ArtifactKind::Tactic, &ghost)
        .expect("service-only bypass");
    println!(
        "CF82_RECEIPT service_only_membership={}",
        ghost_kept
            .iter()
            .map(|key| key.to_scalar_string().unwrap())
            .collect::<Vec<_>>()
            .join(",")
    );
    println!(
        "CF82_RECEIPT service_only_outcomes={}",
        service_gate.warning_log().outcomes().len()
    );

    let aborting = business_root.join("abort");
    let abort_gate = ActivationGate::try_from_project(
        &aborting,
        &built_in_root(),
        &[aborting.join("packs/acme")],
        None,
    )
    .expect("abort gate builds");
    let alpha: BTreeMap<PythonString, ()> = [(text("alpha"), ())].into_iter().collect();
    match abort_gate.filter(ArtifactKind::Tactic, &alpha) {
        Err(sk_doctrine::GateFailure::Constructor(abort)) => {
            println!("CF82_RECEIPT abort_type={}", abort.python)
        }
        Err(other) => panic!("a non-constructor failure hides the identity abort: {other}"),
        Ok(aborted) => panic!("a healthy {aborted:?} hides the identity abort"),
    }
    println!(
        "CF82_RECEIPT abort_outcomes={}",
        abort_gate.warning_log().outcomes().len()
    );
}
