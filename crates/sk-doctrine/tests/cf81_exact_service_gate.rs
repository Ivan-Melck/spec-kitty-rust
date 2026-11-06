//! CF81 — Source-first controls for the exact activation-aware offering gate.
//!
//! Phase 1 (this file's first tests) calls ONLY the pre-existing public API so a
//! Source-valid refusal and a Source-invalid healthy answer are red for the
//! right reason. Phase 2 adds the exact gate, its fallible identity seam, cache
//! publication and the actual CanonicalOfferingService consumer.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;
use sk_doctrine::{ActivationGate, CanonicalOfferingService, OfferingContext};
use sk_kernel::PythonString;

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

fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn ledger(root: &Path, text: &str) {
    write(&root.join(".kittify/charter/activation.yaml"), text);
}

fn org_pack(root: &Path, charter: &str) -> PathBuf {
    let org = root.join("packs/acme-org");
    fs::create_dir_all(org.join("directives")).unwrap();
    fs::create_dir_all(org.join("tactics")).unwrap();
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme-org\n        local_path: packs/acme-org\n",
    );
    write(
        &org.join("org-charter.yaml"),
        if charter.is_empty() { "{}\n" } else { charter },
    );
    org
}

fn gate(root: &Path, org: &Path) -> Result<ActivationGate, String> {
    ActivationGate::from_project(root, &built_in_root(), &[org.to_path_buf()], None)
}

/// A lone high surrogate is Source-admissible in the ledger grammar; the current
/// scalar gate read refuses the whole construction.
#[test]
fn source_admits_a_surrogate_deny_but_the_gate_refused_to_build() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, "");
    ledger(root, "deactivated_directives: [\"\\uD800\"]\n");
    assert!(
        gate(root, &org).is_ok(),
        "Source resolves the surrogate deny; the gate must not refuse construction"
    );
}

/// An absent `activated_skills` key is not "everything" for a required-when-absent
/// kind: Source materialises only the org-required skills.
#[test]
fn absent_skills_materialise_the_org_required_floor_not_everything() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, "required_skills:\n  - req-skill\n");
    ledger(root, "{}\n");
    let gate = gate(root, &org).expect("no-deny gate builds");
    assert!(
        gate.allow_configured(ArtifactKind::Skill),
        "an absent skill key is a materialised org-required set, not None/everything"
    );
}

/// A malformed required field of an unrelated kind must not poison a build with
/// no pending deny. Source only reads a pending kind's strict floor.
#[test]
fn malformed_unrelated_required_field_does_not_poison_a_no_deny_gate() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, "required_glossary_packs: notalist\n");
    ledger(root, "{}\n");
    assert!(
        gate(root, &org).is_ok(),
        "no deny is pending, so no strict floor is read"
    );
}

/// A pending deny whose identity scan raises a Python constructor abort must not
/// become a healthy resolution in the actual service construction path.
#[test]
fn pending_deny_identity_abort_does_not_become_a_healthy_service() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, "");
    write(
        &org.join("directives/bad.directive.yaml"),
        "id: !!timestamp 2021-13-45\n",
    );
    ledger(root, "deactivated_directives: [bad]\n");

    let outcome = OfferingContext::resolve(root)
        .map_err(|reason| reason.to_string())
        .and_then(|context| {
            CanonicalOfferingService::new(context)
                .ids_for("activated_tactics")
                .map_err(|error| error.to_string())
        });
    match outcome {
        Ok(ids) => panic!("Source aborts the pending deny scan, got healthy ids {ids:?}"),
        Err(message) => assert!(
            message.contains("month must be in 1..12"),
            "the real constructor cause is retained: {message}"
        ),
    }
}

fn text(value: &str) -> PythonString {
    PythonString::from_scalars(value)
}

fn cs() -> PythonString {
    PythonString::from_codepoints(&[0xD800]).unwrap()
}

fn items(names: &[PythonString]) -> std::collections::BTreeMap<PythonString, ()> {
    names.iter().map(|name| (name.clone(), ())).collect()
}

const SOURCE_PROBE: &str = r#"
import json, pathlib, sys, os
root = pathlib.Path(os.environ["CF81_SOURCE_ROOT"])
assert root.is_absolute() and root.is_dir(), root
assert pathlib.Path.cwd() == root, pathlib.Path.cwd()
assert pathlib.Path(os.environ["HOME"]) == root / "home"
for key, name in [
    ("XDG_CACHE_HOME", "xdg-cache"), ("XDG_CONFIG_HOME", "xdg-config"),
    ("XDG_DATA_HOME", "xdg-data"), ("XDG_STATE_HOME", "xdg-state"),
    ("XDG_RUNTIME_DIR", "xdg-runtime"), ("XDG_CONFIG_DIRS", "xdg-config-dirs"),
    ("XDG_DATA_DIRS", "xdg-data-dirs"), ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
assert sys.version_info[:3] == (3, 11, 15), sys.version
assert sys.getfilesystemencoding() == os.environ["CF81_EXPECTED_FS_ENCODING"]
assert sys.getfilesystemencodeerrors() == os.environ["CF81_EXPECTED_FS_ERRORS"]
expected_source = pathlib.Path(os.environ["CF81_EXPECTED_SOURCE_ROOT"])
assert expected_source.is_absolute()
assert pathlib.Path(os.environ["PYTHONPATH"]) == expected_source / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == expected_source / "packs"
assert os.environ["PYTHONHASHSEED"] == "0"
assert pathlib.Path(os.environ["CF81_SOURCE_PYTHON"]).is_file()
assert str(pathlib.Path(sys.executable)) == os.environ["CF81_SOURCE_PYTHON"]
assert os.environ["CF81_ORACLE_CHECKED"] == "53"
import charter
assert pathlib.Path(charter.__file__).resolve().is_relative_to(expected_source / "src")
from charter.activation.pack_context import PackContext
from charter.activation.active_charter_service_builder import build_active_charter_service
from charter.offering.artifact_kinds import ArtifactKind

def service(case):
    return build_active_charter_service(root / case)

def receipt(label, value):
    print("CF81_RECEIPT " + label + "=" + value)

case = root / "absent"
ctx = PackContext.from_config(case)
kept = service("absent")._gated(ArtifactKind.SKILL, {"req-skill": 1, "other": 1}, ctx.activated_skills)
receipt("absent_skills", ",".join(sorted(kept)))

case = root / "empty"
ctx = PackContext.from_config(case)
receipt("present_empty_skills", str(len(ctx.activated_skills or ())))

case = root / "no-deny"
PackContext.from_config(case)
receipt("no_deny_malformed", "ok")

case = root / "pending"
try:
    PackContext.from_config(case)
except Exception as exc:
    receipt("pending_abort", type(exc).__name__)
else:
    receipt("pending_abort", "healthy")

case = root / "cache"
svc = service("cache")
activated = frozenset({"999"})
first = svc._gated(ArtifactKind.DIRECTIVE, {"999": 1}, activated)
second = svc._gated(ArtifactKind.DIRECTIVE, {"DIRECTIVE_999": 1}, activated)
receipt("cache", ",".join(sorted(first)) + "|" + ",".join(sorted(second)))
# The cache belongs to one SERVICE, not its reusable PackContext. Use tactics:
# Native ids_for deliberately does not expose a directive activation mapping.
case = root / "fresh"
old_service = service("fresh")
activated = frozenset({"ghost"})
assert list(old_service._gated(ArtifactKind.TACTIC, {"ghost": 1}, activated)) == ["ghost"]
assert not old_service._gated(ArtifactKind.TACTIC, {"fresh-cache-tactic": 1}, activated)
template = (expected_source / "packs/built-in/tactics/moscow-scoping-lens.tactic.yaml").read_text(encoding="utf-8")
new_file = case / "packs/acme/tactics/ghost.tactic.yaml"
new_file.parent.mkdir(parents=True, exist_ok=True)
new_file.write_text(template.replace("id: moscow-scoping-lens", "id: fresh-cache-tactic"), encoding="utf-8")
fresh_ids = sorted(service("fresh").tactics)
assert fresh_ids == ["fresh-cache-tactic"], fresh_ids
receipt("new_service_cache", ",".join(fresh_ids))

case = root / "deny"
svc = service("deny")
kept = svc._gated(ArtifactKind.TACTIC, {"\ud800": 1, "alpha": 1}, None)
receipt("exact_deny", ",".join(sorted(kept)))

case = root / "service"
svc = service("service")
kept = svc._gated(ArtifactKind.TACTIC, {"alpha-tactic": 1, "beta-tactic": 1}, frozenset({"alpha"}))
receipt("service_mapping", ",".join(sorted(kept)))
"#;

fn assert_child_environment(root: &Path) {
    let repo = repository_root();
    assert_eq!(std::env::current_dir().unwrap(), root);
    assert_eq!(
        Path::new(&std::env::var("HOME").unwrap()),
        root.join("home")
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
            root.join(name),
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
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
    assert!(Path::new(&std::env::var("CF81_SOURCE_PYTHON").unwrap()).is_file());
    assert_eq!(std::env::var("CF81_ORACLE_CHECKED").unwrap(), "53");
    assert_eq!(
        std::env::var_os("CF81_EXPECTED_SOURCE_ROOT").map(PathBuf::from),
        Some(repo)
    );
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
}

fn populate_case(
    root: &Path,
    case: &str,
    ledger_text: &str,
    org_charter: &str,
    files: &[(&str, &str)],
) {
    let case_root = root.join(case);
    write(
        &case_root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(&case_root.join("packs/acme/org-charter.yaml"), org_charter);
    write(
        &case_root.join(".kittify/charter/activation.yaml"),
        ledger_text,
    );
    for (relative, body) in files {
        write(&case_root.join(relative), body);
    }
}

fn populate_child_fixture(root: &Path) {
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
    populate_case(
        root,
        "absent",
        "{}\n",
        "required_skills:\n  - req-skill\n",
        &[],
    );
    populate_case(
        root,
        "empty",
        "activated_skills: []\n",
        "required_skills:\n  - req-skill\n",
        &[],
    );
    populate_case(
        root,
        "no-deny",
        "{}\n",
        "required_glossary_packs: notalist\n",
        &[],
    );
    populate_case(
        root,
        "pending",
        "deactivated_directives: [bad]\n",
        "{}\n",
        &[(
            "packs/acme/directives/bad.directive.yaml",
            "id: !!timestamp 2021-13-45\n",
        )],
    );
    populate_case(
        root,
        "cache",
        "activated_directives: ['999']\n",
        "{}\n",
        &[],
    );
    populate_case(
        root,
        "deny",
        "deactivated_tactics: [\"\\uD800\"]\n",
        "{}\n",
        &[],
    );
    populate_case(root, "fresh", "activated_tactics: [ghost]\n", "{}\n", &[]);
    let tactic = built_in("tactics/moscow-scoping-lens.tactic.yaml");
    populate_case(
        root,
        "service",
        "activated_tactics: [alpha]\n",
        "{}\n",
        &[
            (
                "packs/acme/tactics/alpha.tactic.yaml",
                &tactic.replace("id: moscow-scoping-lens", "id: alpha-tactic"),
            ),
            (
                "packs/acme/tactics/beta.tactic.yaml",
                &tactic.replace("id: moscow-scoping-lens", "id: beta-tactic"),
            ),
        ],
    );
}

fn child_environment(command: &mut std::process::Command, root: &Path) {
    let repo = repository_root();
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
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
        .env("PYTHONHASHSEED", "0")
        .env("CF81_EXPECTED_SOURCE_ROOT", &repo)
        .env("CF81_SOURCE_PYTHON", support::python())
        .env("CF81_ORACLE_CHECKED", "53");
}

fn receipt_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("CF81_RECEIPT "))
        .map(|line| line.trim().to_owned())
        .collect()
}

/// The Source and Native business children are independently materialized, both
/// assert their own environment before their first read, and their exact gate
/// receipts agree.
#[test]
fn guarded_source_native_exact_gate_pair() {
    if let Some(root) = std::env::var_os("CF81_NATIVE_ROOT").map(PathBuf::from) {
        assert_child_environment(&root);
        let mut lines = Vec::new();
        let absent = gate(&root.join("absent"), &root.join("absent/packs/acme")).unwrap();
        let kept = absent
            .filter(
                ArtifactKind::Skill,
                &items(&[text("req-skill"), text("other")]),
            )
            .unwrap();
        lines.push(format!(
            "absent_skills={}",
            kept.iter()
                .map(|key| key.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        ));
        let empty = gate(&root.join("empty"), &root.join("empty/packs/acme")).unwrap();
        lines.push(format!(
            "present_empty_skills={}",
            empty
                .filter(
                    ArtifactKind::Skill,
                    &items(&[text("req-skill"), text("other")])
                )
                .unwrap()
                .len()
        ));
        lines.push(format!(
            "no_deny_malformed={}",
            if gate(&root.join("no-deny"), &root.join("no-deny/packs/acme")).is_ok() {
                "ok"
            } else {
                "refused"
            }
        ));
        let pending = ActivationGate::try_from_project(
            &root.join("pending"),
            &built_in_root(),
            &[root.join("pending/packs/acme")],
            None,
        );
        lines.push(format!(
            "pending_abort={}",
            match pending {
                Err(sk_doctrine::ActivationReadError::Aborted { python, .. }) => python,
                _ => "healthy".to_owned(),
            }
        ));
        let cached_context = OfferingContext::resolve(&root.join("cache")).unwrap();
        let cache = cached_context
            .activation()
            .expect("activation-aware context");
        let first = cache
            .filter(ArtifactKind::Directive, &items(&[text("999")]))
            .unwrap();
        let second = cache
            .filter(ArtifactKind::Directive, &items(&[text("DIRECTIVE_999")]))
            .unwrap();
        lines.push(format!(
            "cache={}|{}",
            first
                .iter()
                .map(|key| key.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(","),
            second
                .iter()
                .map(|key| key.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        ));
        let fresh_context = OfferingContext::resolve(&root.join("fresh")).unwrap();
        let old_gate = fresh_context.activation().unwrap();
        assert_eq!(
            old_gate
                .filter(ArtifactKind::Tactic, &items(&[text("ghost")]))
                .unwrap(),
            vec![text("ghost")]
        );
        assert!(
            old_gate
                .filter(ArtifactKind::Tactic, &items(&[text("fresh-cache-tactic")]))
                .unwrap()
                .is_empty()
        );
        write(
            &root.join("fresh/packs/acme/tactics/ghost.tactic.yaml"),
            &built_in("tactics/moscow-scoping-lens.tactic.yaml")
                .replace("id: moscow-scoping-lens", "id: fresh-cache-tactic"),
        );
        let fresh_service = CanonicalOfferingService::new(fresh_context);
        let fresh_ids = fresh_service.ids_for("activated_tactics").unwrap();
        lines.push(format!(
            "new_service_cache={}",
            fresh_ids
                .iter()
                .map(|key| key.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        ));
        let deny = gate(&root.join("deny"), &root.join("deny/packs/acme")).unwrap();
        let kept = deny
            .filter(ArtifactKind::Tactic, &items(&[cs(), text("alpha")]))
            .unwrap();
        lines.push(format!(
            "exact_deny={}",
            kept.iter()
                .map(|key| key.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        ));
        let context = OfferingContext::resolve(&root.join("service")).unwrap();
        let service = CanonicalOfferingService::new(context);
        let kept = service.ids_for("activated_tactics").unwrap();
        lines.push(format!(
            "service_mapping={}",
            kept.iter()
                .map(|key| key.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
                .join(",")
        ));
        for line in lines {
            println!("CF81_RECEIPT {line}");
        }
        return;
    }

    let source_root = tempfile::tempdir().unwrap();
    populate_child_fixture(source_root.path());
    let mut source_command = std::process::Command::new(support::python());
    child_environment(&mut source_command, source_root.path());
    source_command
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("CF81_SOURCE_ROOT", source_root.path())
        .env("CF81_EXPECTED_FS_ENCODING", "utf-8")
        .env("CF81_EXPECTED_FS_ERRORS", "surrogateescape");
    let source_output = source_command.output().expect("run the Source child");
    assert!(
        source_output.status.success(),
        "Source child failed: {} {}",
        String::from_utf8_lossy(&source_output.stdout),
        String::from_utf8_lossy(&source_output.stderr)
    );
    let source_receipt = receipt_lines(&String::from_utf8(source_output.stdout).unwrap());
    assert_eq!(
        source_receipt.len(),
        8,
        "Source child emitted every receipt"
    );

    let native_root = tempfile::tempdir().unwrap();
    populate_child_fixture(native_root.path());
    let mut native_command = std::process::Command::new(std::env::current_exe().unwrap());
    child_environment(&mut native_command, native_root.path());
    native_command
        .args([
            "--exact",
            "guarded_source_native_exact_gate_pair",
            "--nocapture",
        ])
        .env("CF81_NATIVE_ROOT", native_root.path());
    let native_output = native_command.output().expect("run the Native child");
    assert!(
        native_output.status.success(),
        "Native child failed: {} {}",
        String::from_utf8_lossy(&native_output.stdout),
        String::from_utf8_lossy(&native_output.stderr)
    );
    let native_receipt = receipt_lines(&String::from_utf8(native_output.stdout).unwrap());
    assert_eq!(
        native_receipt.len(),
        8,
        "Native child emitted every receipt"
    );

    assert_eq!(
        source_receipt, native_receipt,
        "Source/Native exact gate receipts"
    );
}

/// Deny membership is exact code-point membership, not a scalar projection that
/// turns a non-scalar key into an automatic healthy admit.
#[test]
fn surrogate_deny_membership_is_exact() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, "");
    ledger(root, "deactivated_tactics: [\"\\uD800\"]\n");
    let gate = gate(root, &org).expect("exact gate builds");
    let kept = gate
        .filter(ArtifactKind::Tactic, &items(&[cs(), text("alpha")]))
        .expect("no identity scan is needed for a deny-only filter");
    assert_eq!(kept, vec![text("alpha")]);
}

/// The exact allow set is deny-netted, a present empty set keeps nothing, and a
/// service-only token is resolved once and published for the rest of the gate's
/// life (the reference's `_activated_id_cache`).
#[test]
fn service_only_token_is_cached_after_the_first_resolution() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, "");
    ledger(root, "activated_directives: ['999']\n");
    let gate = gate(root, &org).expect("gate builds");
    let first = gate
        .filter(ArtifactKind::Directive, &items(&[text("999")]))
        .expect("service-only resolution");
    assert_eq!(first, vec![text("999")]);
    let second = gate
        .filter(ArtifactKind::Directive, &items(&[text("DIRECTIVE_999")]))
        .expect("cached resolution");
    assert!(
        second.is_empty(),
        "the first service-only token is not silently re-spelled from a changed mapping"
    );
}

/// The actual CanonicalOfferingService consumes the exact gate for its tactic
/// mapping; deny wins over allow, and an empty present allow keeps nothing.
#[test]
fn actual_service_consumes_the_exact_gate() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, "");
    let tactic = built_in("tactics/moscow-scoping-lens.tactic.yaml");
    write(
        &org.join("tactics/alpha.tactic.yaml"),
        &tactic.replace("id: moscow-scoping-lens", "id: alpha-tactic"),
    );
    write(
        &org.join("tactics/beta.tactic.yaml"),
        &tactic.replace("id: moscow-scoping-lens", "id: beta-tactic"),
    );
    ledger(root, "activated_tactics: [alpha]\n");
    let context = OfferingContext::resolve(root).expect("offering context");
    let service = CanonicalOfferingService::new(context);
    assert_eq!(
        service
            .ids_for("activated_tactics")
            .expect("tactic mapping"),
        vec![text("alpha-tactic")]
    );

    ledger(
        root,
        "activated_tactics: [alpha]\ndeactivated_tactics: [alpha-tactic]\n",
    );
    let context = OfferingContext::resolve(root).expect("offering context with deny");
    let service = CanonicalOfferingService::new(context);
    assert!(
        service
            .ids_for("activated_tactics")
            .expect("deny-netted tactic mapping")
            .is_empty(),
        "a deny wins over the allow set"
    );
}

/// The allow-filter identity phase is the same abort phase as the reference's
/// `_gated`; it never returns a healthy unresolved token.
#[test]
fn identity_abort_in_the_allow_filter_escapes() {
    let directory = root();
    let root = directory.path();
    let org = org_pack(root, "");
    write(
        &org.join("tactics/bad.tactic.yaml"),
        "id: !!timestamp 2021-13-45\n",
    );
    ledger(root, "activated_tactics: [alpha]\n");
    let gate = gate(root, &org).expect("no pending deny, so construction reads no identity");
    match gate.filter(ArtifactKind::Tactic, &items(&[text("alpha")])) {
        Err(sk_doctrine::GateFailure::Constructor(abort)) => {
            assert_eq!(abort.python, "ValueError");
            assert!(
                abort.message.contains("month must be in 1..12"),
                "the real cause is retained: {}",
                abort.message
            );
        }
        Err(other) => panic!("a non-constructor failure hides the identity abort: {other}"),
        Ok(ids) => panic!("a healthy {ids:?} result hides the identity abort"),
    }
}
