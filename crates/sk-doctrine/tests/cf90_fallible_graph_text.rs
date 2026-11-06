//! CF90 — fallible canonical graph `str()`/`repr()` for eager Source integer errors.
//!
//! One bounded plan, run by two genuinely independent children on three configured
//! integer-conversion policies (interpreter default, `640`, `0`/unlimited): the pinned
//! Source oracle and this test binary. Each child asserts its own fresh
//! CWD/HOME/seven-XDG/TMP trees, its own absolute expected Source, PYTHONPATH, packs
//! root, module origin and hash seed, the real filesystem/stdout/stderr codecs, and the
//! pinned CPython 3.11.15 / exact-53 oracle proof **before** touching business data.
//!
//! The Source rows are shallow hexadecimal integers, which the pinned loaders construct
//! with `int(text, 16)` and therefore *without* the decimal conversion ceiling: the YAML
//! load succeeds and the eager `str(data.get("id", "?"))` in the pre-validation hook is
//! the phase that raises `ValueError` for a decimal spelling above the configured
//! ceiling. The same relative fixture paths are used by both children, so every returned
//! path/message is compared raw — no root substitution, no output rewriting, no shared
//! directory.
//!
//! Rows also cover list/dict nesting (including a mapping key), the exact `4300`/`4301`
//! digit boundary, a negative spelling, leading-zero hex canonicalisation, a cycle
//! control, a non-`id` field Source never renders, a string of digits (not an int), the
//! two eager Procedure renders, the unchanged `InlineReferenceRejectedError` control and
//! a healthy artifact. Native-only controls (not Source pairs) prove the fail->repair
//! retry, the warm-success boundary, and the typed start-up-policy refusal for an invalid
//! `PYTHONINTMAXSTRDIGITS`.

mod support;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sk_charter::ArtifactKind;
use sk_doctrine::{CanonicalOfferingService, OfferingContext, OfferingRoots};

const ROOTS: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "configdirs"),
    ("XDG_DATA_DIRS", "datadirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

/// `<kind token>`, `<plural directory>`, `<filename suffix>`.
const KINDS: [(&str, &str, &str); 3] = [
    ("directive", "directives", "directive"),
    ("tactic", "tactics", "tactic"),
    ("procedure", "procedures", "procedure"),
];

/// `0x1` followed by `zeros` further hex zero digits: `2 ** (4 * zeros)`.
fn hex_pow2(zeros: usize) -> String {
    format!("0x1{}", "0".repeat(zeros))
}

/// `2 ** 14284` (4,300 decimal digits) and `2 ** 14285` (4,301 decimal digits).
fn hex_4300() -> String {
    hex_pow2(3571)
}

fn hex_4301() -> String {
    hex_pow2(3571).replacen("0x1", "0x2", 1)
}

fn healthy_directive() -> &'static str {
    "schema_version: '1.0'\nid: DIRECTIVE_900\ntitle: Healthy\nintent: Prove a healthy artifact still loads.\nenforcement: required\n"
}

/// The plan shared by both children: `<label>`, `<kind token>`, `<YAML body>`.
fn rows(plan: &str) -> Vec<(&'static str, &'static str, String)> {
    let big_4300 = hex_4300();
    let big_4301 = hex_4301();
    match plan {
        "limit640" => vec![
            (
                "directive.hex-4300-no-field",
                "directive",
                format!("id: {big_4300}\n"),
            ),
            (
                "directive.hex-100-no-field",
                "directive",
                format!("id: {}\n", hex_pow2(83)),
            ),
            (
                "tactic.hex-100-no-field",
                "tactic",
                format!("id: {}\n", hex_pow2(83)),
            ),
        ],
        "unlimited" => vec![
            (
                "directive.hex-4301-no-field",
                "directive",
                format!("id: {big_4301}\n"),
            ),
            (
                "directive.hex-4301-inline-field",
                "directive",
                format!("id: {big_4301}\ntactic_refs: []\n"),
            ),
            (
                "procedure.hex-4301-no-field",
                "procedure",
                format!("id: {big_4301}\n"),
            ),
        ],
        _ => vec![
            (
                "directive.hex-4300-no-field",
                "directive",
                format!("id: {big_4300}\n"),
            ),
            (
                "directive.hex-4301-no-field",
                "directive",
                format!("id: {big_4301}\n"),
            ),
            (
                "directive.hex-4301-inline-field",
                "directive",
                format!("id: {big_4301}\ntactic_refs: []\n"),
            ),
            (
                "directive.hex-4300-inline-field",
                "directive",
                format!("id: {big_4300}\ntactic_refs: []\n"),
            ),
            (
                "directive.hex-list-nested",
                "directive",
                format!("id: [1, {big_4301}]\n"),
            ),
            (
                "directive.hex-dict-nested",
                "directive",
                format!("id: {{k: {big_4301}}}\n"),
            ),
            (
                "directive.hex-dict-int-key",
                "directive",
                // A short implicit key: Source's scanner refuses flow-mapping plain
                // keys longer than 1024 characters as a YAMLError before construction,
                // so a long key is outside the parser domain and cannot exercise str().
                format!("id: {{7: {big_4301}}}\n"),
            ),
            (
                "directive.hex-two-list-order",
                "directive",
                format!("id: [{big_4300}, {big_4301}]\n"),
            ),
            (
                "directive.hex-negative",
                "directive",
                format!("id: -{big_4301}\n"),
            ),
            (
                "directive.hex-leading-zeros-small",
                "directive",
                "id: 0x0000007\n".to_owned(),
            ),
            (
                "directive.hex-leading-zeros-huge",
                "directive",
                format!("id: 0x{}2{}\n", "0".repeat(40), "0".repeat(3571)),
            ),
            ("directive.small-int", "directive", "id: 7\n".to_owned()),
            ("directive.cycle", "directive", "id: &a [*a]\n".to_owned()),
            (
                "directive.huge-in-other-field",
                "directive",
                format!("id: item-1\nnote: {big_4301}\n"),
            ),
            (
                "directive.string-of-digits",
                "directive",
                format!("id: \"{}\"\n", "1".repeat(4301)),
            ),
            (
                "tactic.hex-4301-no-field",
                "tactic",
                format!("id: {big_4301}\n"),
            ),
            (
                "procedure.hex-4301-no-field",
                "procedure",
                format!("id: {big_4301}\n"),
            ),
            (
                "procedure.hex-4301-step-field",
                "procedure",
                format!("id: {big_4301}\nsteps:\n  - title: one\n    tactic_refs: [t]\n"),
            ),
            (
                "procedure.small-step-field",
                "procedure",
                "id: p-1\nsteps:\n  - title: one\n    tactic_refs: [t]\n".to_owned(),
            ),
            (
                "directive.small-inline-field",
                "directive",
                "id: item-1\ntactic_refs: []\n".to_owned(),
            ),
            (
                "directive.healthy",
                "directive",
                healthy_directive().to_owned(),
            ),
        ],
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A private root with the seven XDG trees, the redirected TMP trees and the two
/// layer directories both children write into.
fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (_, name) in ROOTS {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
    }
    for (_, plural, _) in KINDS {
        std::fs::create_dir_all(root.path().join("builtin").join(plural)).unwrap();
        std::fs::create_dir_all(root.path().join("project").join(plural)).unwrap();
    }
    root
}

fn plural(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(token, _, _)| *token == kind)
        .map(|(_, plural, _)| *plural)
        .unwrap()
}

fn suffix(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(token, _, _)| *token == kind)
        .map(|(_, _, suffix)| *suffix)
        .unwrap()
}

/// The one row path for its label, relative to the child's own CWD.
///
/// Each row gets its own filename because the reference memoises shipped documents on
/// `(path, size, mtime_ns, variant)`; rewriting one path with a same-size body inside
/// one timestamp tick is a different behaviour than parsing fresh inputs.
fn row_path(label: &str, kind: &str) -> PathBuf {
    Path::new("builtin").join(plural(kind)).join(format!(
        "row-{}.{}.yaml",
        label.replace('.', "-"),
        suffix(kind)
    ))
}

fn limit(plan: &str) -> Option<&'static str> {
    match plan {
        "limit640" => Some("640"),
        "unlimited" => Some("0"),
        _ => None,
    }
}

fn environment(command: &mut Command, root: &Path, plan: &str) {
    let expected = repository_root();
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CF90_ROOT", root)
        .env("CF90_SOURCE", &expected)
        .env("CF90_PLAN", plan)
        .env("PYTHONPATH", expected.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", expected.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    if let Some(value) = limit(plan) {
        command.env("PYTHONINTMAXSTRDIGITS", value);
    }
    for (key, name) in ROOTS {
        command.env(key, root.join(name));
    }
}

/// Assert this child's own environment before any business read.
fn native_environment_guards(root: &Path, plan: &str) {
    assert!(root.is_absolute());
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key}"
        );
    }
    let expected = repository_root();
    assert_eq!(
        std::env::var_os("CF90_SOURCE").map(PathBuf::from),
        Some(expected.clone())
    );
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(expected.join("src"))
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(expected.join("packs"))
    );
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
    assert_eq!(std::env::var("CF90_PLAN").unwrap(), plan);
    let expected_limit = if std::env::var_os("CF90_STARTUP").is_some() {
        Some("639".to_owned())
    } else {
        limit(plan).map(str::to_owned)
    };
    assert_eq!(
        std::env::var_os("PYTHONINTMAXSTRDIGITS").map(|value| value.to_str().unwrap().to_owned()),
        expected_limit,
        "the configured integer ceiling is the one the Kernel policy reads"
    );
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    let stdout = sk_kernel::python_stream::StreamPolicy::from_environment().unwrap();
    assert_eq!(
        (stdout.encoding(), stdout.errors()),
        ("utf-8", "surrogateescape")
    );
    let stderr = sk_kernel::python_stream::StreamPolicy::for_stderr().unwrap();
    assert_eq!(
        (stderr.encoding(), stderr.errors()),
        ("utf-8", "backslashreplace")
    );
}

/// The pinned, lock-exact oracle proof, run in a child of its own.
///
/// Only the business plan child calls this: the start-up-policy control is a Native
/// boundary (CPython would not have started), so it stores an intentionally invalid
/// `PYTHONINTMAXSTRDIGITS` that the verifier's own interpreter would refuse.
fn verify_oracle() {
    let expected = repository_root();
    let output = Command::new(expected.join(".venv-rust-oracle/bin/python"))
        .arg(expected.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(&expected)
        .current_dir(&expected)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let proof: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["python"], "3.11.15");
    assert_eq!(proof["checked_runtime_requirements"], 53);
}

fn service() -> CanonicalOfferingService {
    CanonicalOfferingService::new(OfferingContext::new(
        PathBuf::from("."),
        OfferingRoots {
            built_in: Some(PathBuf::from("builtin")),
            org_roots: vec![],
            project: None,
        },
        None,
    ))
}

fn native_record(kind: &str) -> serde_json::Value {
    let kind: ArtifactKind = kind.parse().expect("canonical kind token");
    match service().artifact_ids(kind) {
        Ok(ids) => serde_json::json!({
            "class": serde_json::Value::Null,
            "ids": ids
                .iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>(),
        }),
        Err(error) => match error.abort() {
            Some(abort) => serde_json::json!({
                "class": abort.python,
                "message": abort.message,
            }),
            None => serde_json::json!({
                "class": "<not-an-abort>",
                "message": error.message(),
            }),
        },
    }
}

fn native_plan(root: &Path, plan: &str) -> serde_json::Value {
    let mut observed = serde_json::Map::new();
    for (label, kind, body) in rows(plan) {
        let path = row_path(label, kind);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        observed.insert(label.to_owned(), native_record(kind));
        std::fs::remove_file(root.join(&path)).unwrap();
    }
    serde_json::Value::Object(observed)
}

/// Native-only: a failed construction is retried and a successful one stays warm.
fn native_cache(root: &Path) -> serde_json::Value {
    let path = row_path("directive.cache", "directive");
    std::fs::write(&path, format!("id: {}\n", hex_4301())).unwrap();
    let cached = service();
    let aborted = cached
        .artifact_ids(ArtifactKind::Directive)
        .is_err_and(|error| error.abort().is_some());
    std::fs::write(&path, healthy_directive()).unwrap();
    let repaired = cached
        .artifact_ids(ArtifactKind::Directive)
        .map(|ids| {
            ids.iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    std::fs::write(&path, format!("id: {}\n", hex_4301())).unwrap();
    let warm = cached
        .artifact_ids(ArtifactKind::Directive)
        .map(|ids| {
            ids.iter()
                .map(|id| id.to_scalar_string().unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    std::fs::remove_file(root.join(&path)).unwrap();
    serde_json::json!({"aborted": aborted, "repaired": repaired, "warm": warm})
}

/// Native-only: an invalid `PYTHONINTMAXSTRDIGITS` is a typed start-up refusal.
fn native_startup_policy(root: &Path) -> serde_json::Value {
    let path = row_path("directive.startup-policy", "directive");
    std::fs::write(&path, "id: item-1\ntactic_refs: []\n").unwrap();
    let outcome = match service().artifact_ids(ArtifactKind::Directive) {
        Ok(ids) => serde_json::json!({"class": "ok", "ids": ids.len()}),
        Err(error) => serde_json::json!({
            "class": error.abort().map(|abort| abort.python),
            "native": error.native().is_some(),
            "mentions_integer_policy": error.message().contains("integer conversion"),
        }),
    };
    std::fs::remove_file(root.join(&path)).unwrap();
    outcome
}

const SOURCE: &str = r#"
import json, os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['CF90_ROOT'])
expected = pathlib.Path(os.environ['CF90_SOURCE'])
assert root.is_absolute() and pathlib.Path.cwd() == root
assert expected.is_absolute() and expected.is_dir()
for key, name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','configdirs'),('XDG_DATA_DIRS','datadirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert pathlib.Path(os.environ[key]) == root / name, key
assert pathlib.Path(os.environ['PYTHONPATH']) == expected / 'src'
assert pathlib.Path(os.environ['SPEC_KITTY_PACKS_ROOT']) == expected / 'packs'
assert os.environ['PYTHONHASHSEED'] == '0'
assert sys.version_info[:3] == (3,11,15)
assert (sys.getfilesystemencoding(), sys.getfilesystemencodeerrors()) == ('utf-8','surrogateescape')
assert (sys.stdout.encoding, sys.stdout.errors) == ('utf-8','surrogateescape')
assert (sys.stderr.encoding, sys.stderr.errors) == ('utf-8','backslashreplace')
import charter
assert pathlib.Path(charter.__file__).resolve().is_relative_to(expected / 'src')
proof = json.loads(subprocess.check_output([str(expected / '.venv-rust-oracle/bin/python'), str(expected / 'scripts/rust/verify_oracle.py'), '--root', str(expected)], cwd=expected, text=True))
assert proof['ok'] and proof['python'] == '3.11.15' and proof['checked_runtime_requirements'] == 53

from charter.offering.directives.repository import DirectiveRepository
from charter.offering.procedures.repository import ProcedureRepository
from charter.offering.tactics.repository import TacticRepository

REPOS = {
    'directive': (DirectiveRepository, 'directives', 'directive'),
    'tactic': (TacticRepository, 'tactics', 'tactic'),
    'procedure': (ProcedureRepository, 'procedures', 'procedure'),
}

rows = json.load(sys.stdin)
plan = {}
for label, kind, body, relative in rows:
    cls, plural, suffix = REPOS[kind]
    path = pathlib.Path(relative)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding='utf-8')
    try:
        repository = cls(built_in_dir=pathlib.Path('builtin') / plural)
    except Exception as exc:
        plan[label] = {'class': type(exc).__name__, 'message': str(exc)}
    else:
        plan[label] = {'class': None, 'ids': sorted(item.id for item in repository.list_all())}
    path.unlink()
print('CF90_PLAN ' + json.dumps(plan, sort_keys=True))
"#;

fn receipt(bytes: &[u8], prefix: &str) -> serde_json::Value {
    let text = std::str::from_utf8(bytes).unwrap();
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("missing {prefix} in:\n{text}"));
    serde_json::from_str(line).unwrap()
}

fn assert_source_classes(plan: &str, reference: &serde_json::Value) {
    let value_error = "Exceeds the limit (4300 digits) for integer string conversion; \
                       use sys.set_int_max_str_digits() to increase the limit";
    match plan {
        "limit640" => {
            let message = "Exceeds the limit (640 digits) for integer string conversion; \
                           use sys.set_int_max_str_digits() to increase the limit";
            assert_eq!(
                reference["directive.hex-4300-no-field"]["class"], "ValueError",
                "a 4,300-digit decimal spelling is over the configured 640 ceiling"
            );
            assert_eq!(reference["directive.hex-4300-no-field"]["message"], message);
            for row in ["directive.hex-100-no-field", "tactic.hex-100-no-field"] {
                assert!(
                    reference[row]["class"].is_null(),
                    "a 100-digit spelling stays under the configured 640 ceiling: {row}"
                );
            }
        }
        "unlimited" => {
            for row in ["directive.hex-4301-no-field", "procedure.hex-4301-no-field"] {
                assert!(
                    reference[row]["class"].is_null(),
                    "0 means unlimited, so a 4,301-digit spelling renders: {row}"
                );
            }
            assert_eq!(
                reference["directive.hex-4301-inline-field"]["class"],
                "InlineReferenceRejectedError",
                "the eager id render succeeds, then the retired field aborts"
            );
        }
        _ => {
            for row in [
                "directive.hex-4301-no-field",
                "directive.hex-4301-inline-field",
                "directive.hex-list-nested",
                "directive.hex-dict-nested",
                "directive.hex-dict-int-key",
                "directive.hex-two-list-order",
                "directive.hex-negative",
                "directive.hex-leading-zeros-huge",
                "tactic.hex-4301-no-field",
                "procedure.hex-4301-no-field",
                "procedure.hex-4301-step-field",
            ] {
                assert_eq!(
                    reference[row]["class"], "ValueError",
                    "Source eager str(int) raises before any field walk: {row}"
                );
                assert_eq!(reference[row]["message"], value_error, "{row}");
            }
            for row in [
                "directive.hex-4300-no-field",
                "directive.hex-leading-zeros-small",
                "directive.small-int",
                "directive.cycle",
                "directive.huge-in-other-field",
                "directive.string-of-digits",
            ] {
                assert!(
                    reference[row]["class"].is_null(),
                    "no eager int conversion or no ceiling breach: {row}"
                );
            }
            for row in [
                "directive.hex-4300-inline-field",
                "procedure.small-step-field",
                "directive.small-inline-field",
            ] {
                assert_eq!(
                    reference[row]["class"], "InlineReferenceRejectedError",
                    "an in-ceiling id keeps the existing Source abort: {row}"
                );
            }
            assert_eq!(
                reference["directive.healthy"]["ids"],
                serde_json::json!(["DIRECTIVE_900"]),
                "the healthy control still loads"
            );
        }
    }
}

#[test]
fn fallible_graph_text_plan_matches_the_pinned_source() {
    if let Some(value) = std::env::var_os("CF90_NATIVE") {
        let root = PathBuf::from(value);
        let plan = std::env::var("CF90_PLAN").unwrap_or_else(|_| "default".to_owned());
        native_environment_guards(&root, &plan);
        if std::env::var_os("CF90_STARTUP").is_some() {
            println!(
                "CF90_STARTUP {}",
                serde_json::to_string(&native_startup_policy(&root)).unwrap()
            );
            return;
        }
        verify_oracle();
        let observed = native_plan(&root, &plan);
        println!("CF90_PLAN {}", serde_json::to_string(&observed).unwrap());
        if plan == "default" {
            let cache = native_cache(&root);
            println!("CF90_CACHE {}", serde_json::to_string(&cache).unwrap());
        }
        return;
    }

    for plan in ["default", "limit640", "unlimited"] {
        let table = rows(plan);
        let payload: Vec<[String; 4]> = table
            .iter()
            .map(|(label, kind, body)| {
                let path = row_path(label, kind);
                [
                    (*label).to_owned(),
                    (*kind).to_owned(),
                    body.clone(),
                    path.to_str().unwrap().to_owned(),
                ]
            })
            .collect();
        let payload = serde_json::to_vec(&payload).unwrap();

        let source_root = fixture();
        let mut source = Command::new(support::python());
        environment(&mut source, source_root.path(), plan);
        let mut child = source
            .arg("-c")
            .arg(SOURCE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.as_mut().unwrap().write_all(&payload).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "Source (plan {plan}): {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reference = receipt(&output.stdout, "CF90_PLAN ");
        assert_source_classes(plan, &reference);

        let native_root = fixture();
        assert_ne!(source_root.path(), native_root.path());
        let mut native = Command::new(std::env::current_exe().unwrap());
        environment(&mut native, native_root.path(), plan);
        let output = native
            .env("CF90_NATIVE", native_root.path())
            .args([
                "--exact",
                "fallible_graph_text_plan_matches_the_pinned_source",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Native (plan {plan}): {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let observed = receipt(&output.stdout, "CF90_PLAN ");
        assert_eq!(
            observed, reference,
            "plan {plan}: identical relative business arguments, raw messages, no rewriting"
        );

        if plan == "default" {
            let cache = receipt(&output.stdout, "CF90_CACHE ");
            assert_eq!(cache["aborted"], true, "the huge-int failure is not cached");
            assert_eq!(
                cache["repaired"],
                serde_json::json!(["DIRECTIVE_900"]),
                "the same service retries after repair"
            );
            assert_eq!(
                cache["warm"],
                serde_json::json!(["DIRECTIVE_900"]),
                "a completed construction stays warm"
            );

            // Native-only: an invalid PYTHONINTMAXSTRDIGITS is a start-up refusal,
            // not a Source business exception (CPython would not have started).
            let startup_root = fixture();
            let mut startup = Command::new(std::env::current_exe().unwrap());
            environment(&mut startup, startup_root.path(), plan);
            let output = startup
                .env("CF90_NATIVE", startup_root.path())
                .env("CF90_STARTUP", "1")
                .env("PYTHONINTMAXSTRDIGITS", "639")
                .args([
                    "--exact",
                    "invalid_integer_policy_is_a_typed_native_startup_refusal",
                    "--nocapture",
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "startup policy: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn invalid_integer_policy_is_a_typed_native_startup_refusal() {
    if std::env::var_os("CF90_NATIVE").is_none() {
        return;
    }
    let root = PathBuf::from(std::env::var_os("CF90_NATIVE").unwrap());
    let plan = std::env::var("CF90_PLAN").unwrap_or_else(|_| "default".to_owned());
    native_environment_guards(&root, &plan);
    assert_eq!(std::env::var("PYTHONINTMAXSTRDIGITS").unwrap(), "639");
    let observed = native_startup_policy(&root);
    assert_eq!(observed["class"], serde_json::Value::Null);
    assert_eq!(observed["native"], true);
    assert_eq!(observed["mentions_integer_policy"], true);
}
