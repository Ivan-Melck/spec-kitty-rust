//! CF91 — identity declared-id eager `str()` phases, catches and caches.
//!
//! One bounded plan, run by two genuinely independent children: the pinned Source
//! oracle and this test binary. Each child asserts its own fresh CWD/HOME/seven-XDG
//! (including `XDG_CONFIG_DIRS`/`XDG_DATA_DIRS`/`XDG_RUNTIME_DIR`)/TMPDIR/TMP/TEMP
//! trees, its own absolute expected Source, `PYTHONPATH`, packs root, module origin,
//! hash seed, the real filesystem/stdout/stderr codecs and the pinned CPython
//! 3.11.15 / exact-53 oracle proof **before** touching business data.
//!
//! Source rows are explicit hexadecimal integers, which the pinned loaders construct
//! without the decimal conversion ceiling; `identity_index._parse_id` then runs the
//! eager `str(data.get(id_field, ""))` OUTSIDE its read/load `try`, so a decimal
//! spelling above the configured ceiling raises `ValueError` out of the scan and is
//! never cached. The same relative fixture paths are used by both children, so every
//! returned length/head/message is compared raw — no root substitution, no output
//! rewriting, no shared directory.
//!
//! The plan covers the 4,300/4,301 default boundary, a nested list integer, the
//! configured `640` and `0`/unlimited policies, a quoted text id, the conservative
//! fast plain-id recognizer beside an unrelated huge integer, the caught `TypeError`
//! construction phase (`_parse_id` returns `None`), the same-index fail -> repair
//! retry with the completed caught-skip/fast boundaries, and the actual allow-filter
//! consumer (`Source IdentityIndex.activation_id`, the exact `_gated` token loop,
//! versus the Native `ActivationGate`). A Native-only child proves an invalid
//! `PYTHONINTMAXSTRDIGITS` is a typed start-up refusal, not a Source exception.

mod support;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sk_charter::ArtifactKind;
use sk_doctrine::{ActivationGate, IdentityIndex};
use sk_kernel::PythonString;

const KIND: ArtifactKind = ArtifactKind::Tactic;
const ROW_PREFIX: &str = "cf91-";

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

fn hex_pow2(zeros: usize) -> String {
    format!("0x1{}", "0".repeat(zeros))
}

/// `0x2` followed by 3,571 zeros: 4,301 decimal digits, one over the default.
fn hex_4301() -> String {
    format!("0x2{}", "0".repeat(3571))
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("fixture root");
    for (_, name) in ROOTS {
        std::fs::create_dir_all(root.path().join(name)).expect("isolated root");
    }
    root
}

/// The huge-integer spelling this plan's rows use.
fn huge(plan: &str) -> String {
    match plan {
        "limit640" => hex_pow2(600),
        _ => hex_4301(),
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    std::fs::write(path, text).expect("fixture file");
}

fn write_row(directory: &Path, name: &str, body: &str) -> PathBuf {
    let path = directory.join(name);
    write(&path, body);
    path
}

fn declared(index: &IdentityIndex, token: &str) -> Value {
    match index.try_declared_id(KIND, token) {
        Ok(Some(id)) => json!({
            "declared_len": id.chars().count(),
            "declared_head": id.chars().take(16).collect::<String>(),
        }),
        Ok(None) => json!({"declared": Value::Null}),
        Err(error) => json!({"error": error.to_string()}),
    }
}

fn built_in_root() -> PathBuf {
    support::repository_root().join("packs/built-in")
}

fn index(project: &Path) -> IdentityIndex {
    IdentityIndex::from_roots(&built_in_root(), &[], Some(project))
}

/// The leaf rows shared by both children, as `(label, body)`.
fn rows(plan: &str) -> Vec<(&'static str, String)> {
    let huge = huge(plan);
    let mut rows = vec![
        ("hex", format!("id: {huge}\n")),
        ("nested", format!("id: [{huge}]\n")),
        ("text", "id: \"cf91-alpha\"\n".to_owned()),
        ("fast", format!("id: {ROW_PREFIX}fastid\nx: {huge}\n")),
    ];
    if plan == "default" {
        rows.push(("boundary", format!("id: {}\n", hex_pow2(3571))));
    }
    rows
}

fn native_plan(root: &Path, plan: &str) -> Value {
    let mut observed = serde_json::Map::new();
    for (label, body) in rows(plan) {
        // One project per row: a raising row must not hide a sibling row's result.
        let project = root.join(format!("project-{label}"));
        let tactics = project.join("tactic");
        std::fs::create_dir_all(&tactics).expect("project tactic dir");
        write_row(&tactics, &format!("{ROW_PREFIX}{label}.tactic.yaml"), &body);
        observed.insert(
            label.to_owned(),
            declared(&index(&project), &format!("{ROW_PREFIX}{label}")),
        );
    }

    // Same-instance failure -> repair -> retry, with the completed caught-skip and
    // fast caches observed through the same index.
    let retry_project = root.join("retry");
    let retry_tactics = retry_project.join("tactic");
    std::fs::create_dir_all(&retry_tactics).expect("retry tactic dir");
    write_row(
        &retry_tactics,
        &format!("{ROW_PREFIX}a-broken.tactic.yaml"),
        "id: [unclosed\n",
    );
    write_row(
        &retry_tactics,
        &format!("{ROW_PREFIX}a-fast.tactic.yaml"),
        "id: cf91-fastplain\n",
    );
    write_row(
        &retry_tactics,
        &format!("{ROW_PREFIX}a.tactic.yaml"),
        "id: cf91-alpha\n",
    );
    let retry_path = write_row(
        &retry_tactics,
        &format!("{ROW_PREFIX}b.tactic.yaml"),
        &format!("id: {}\n", hex_4301()),
    );
    let retry = index(&retry_project);
    let mut retry_rows = serde_json::Map::new();
    retry_rows.insert(
        "first".to_owned(),
        declared(&retry, &format!("{ROW_PREFIX}b")),
    );
    write(&retry_path, "id: cf91-beta-fixed\n");
    write_row(
        &retry_tactics,
        &format!("{ROW_PREFIX}a-broken.tactic.yaml"),
        "id: cf91-nu-fixed\n",
    );
    write_row(
        &retry_tactics,
        &format!("{ROW_PREFIX}a-fast.tactic.yaml"),
        "id: cf91-fast-rewritten\n",
    );
    retry_rows.insert("b".to_owned(), declared(&retry, &format!("{ROW_PREFIX}b")));
    retry_rows.insert("a".to_owned(), declared(&retry, &format!("{ROW_PREFIX}a")));
    retry_rows.insert(
        "broken".to_owned(),
        declared(&retry, &format!("{ROW_PREFIX}a-broken")),
    );
    retry_rows.insert(
        "nu".to_owned(),
        declared(&retry, &format!("{ROW_PREFIX}nu-fixed")),
    );
    retry_rows.insert(
        "fast".to_owned(),
        declared(&retry, &format!("{ROW_PREFIX}a-fast")),
    );
    let fresh = index(&retry_project);
    retry_rows.insert(
        "fresh_broken".to_owned(),
        declared(&fresh, &format!("{ROW_PREFIX}a-broken")),
    );
    retry_rows.insert(
        "fresh_nu".to_owned(),
        declared(&fresh, &format!("{ROW_PREFIX}nu-fixed")),
    );
    retry_rows.insert(
        "fresh_fast".to_owned(),
        declared(&fresh, &format!("{ROW_PREFIX}a-fast")),
    );
    observed.insert("retry".to_owned(), Value::Object(retry_rows));

    // `_parse_id` catches `(OSError, YAMLError, TypeError)` around the load and
    // returns `None` (no stem fallback); a ruamel nested-list `!!set` key is the
    // measured TypeError phase.
    let typed_project = root.join("typed");
    let typed_tactics = typed_project.join("tactic");
    std::fs::create_dir_all(&typed_tactics).expect("typed tactic dir");
    write_row(
        &typed_tactics,
        &format!("{ROW_PREFIX}typed.tactic.yaml"),
        "id: !!set {? [[a]], b: null}\n",
    );
    let typed = index(&typed_project);
    let typed_rows = json!({
        "typed": declared(&typed, &format!("{ROW_PREFIX}typed")),
    });
    observed.insert("typeerror".to_owned(), typed_rows);

    if plan == "unlimited" {
        observed.insert("gate".to_owned(), json!({"skipped": true}));
    } else {
        observed.insert("gate".to_owned(), native_gate(root));
    }
    Value::Object(observed)
}

/// The actual allow-filter consumer: Source `_gated`'s identity token loop in
/// Native form. A Source identity abort must stay a non-success here.
fn native_gate(root: &Path) -> Value {
    let gate_root = root.join("gate");
    write(
        &gate_root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(&gate_root.join("packs/acme/org-charter.yaml"), "{}\n");
    write(
        &gate_root.join("packs/acme/tactics/cf91-gate-huge.tactic.yaml"),
        &format!("id: {}\n", hex_4301()),
    );
    write(
        &gate_root.join(".kittify/charter/activation.yaml"),
        "activated_tactics: [cf91-gate-huge]\n",
    );
    let gate = match ActivationGate::try_from_project(
        &gate_root,
        &built_in_root(),
        &[gate_root.join("packs/acme")],
        None,
    ) {
        Ok(gate) => gate,
        Err(error) => return json!({"setup_error": error.to_string()}),
    };
    let items: BTreeMap<PythonString, ()> = [(PythonString::from_scalars("cf91-gate-huge"), ())]
        .into_iter()
        .collect();
    match gate.filter(KIND, &items) {
        Ok(kept) => json!({"kept": kept.len()}),
        Err(error) => json!({"error": error.to_string()}),
    }
}

fn limit(plan: &str) -> Option<&'static str> {
    match plan {
        "limit640" => Some("640"),
        "unlimited" => Some("0"),
        _ => None,
    }
}

fn environment(command: &mut Command, root: &Path, plan: &str) {
    let expected = support::repository_root();
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CF91_ROOT", root)
        .env("CF91_SOURCE", &expected)
        .env("CF91_PLAN", plan)
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
    let expected = support::repository_root();
    assert_eq!(
        std::env::var_os("CF91_SOURCE").map(PathBuf::from),
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
    assert_eq!(std::env::var("CF91_PLAN").unwrap(), plan);
    assert_eq!(
        std::env::var_os("PYTHONINTMAXSTRDIGITS").map(|value| value.to_str().unwrap().to_owned()),
        if std::env::var_os("CF91_STARTUP").is_some() {
            Some("639".to_owned())
        } else {
            limit(plan).map(str::to_owned)
        },
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

fn verify_oracle() {
    let expected = support::repository_root();
    let output = Command::new(expected.join(".venv-rust-oracle/bin/python"))
        .arg(expected.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(&expected)
        .output()
        .expect("run the pinned oracle verifier");
    assert!(
        output.status.success(),
        "oracle is not lock-exact: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let proof: serde_json::Value = serde_json::from_slice(&output.stdout).expect("oracle json");
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["python"], "3.11.15");
    assert_eq!(proof["checked_runtime_requirements"], 53);
}

fn receipt(bytes: &[u8], prefix: &str) -> Value {
    let text = std::str::from_utf8(bytes).unwrap();
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("missing {prefix} in:\n{text}"));
    serde_json::from_str(line).unwrap()
}

const SOURCE: &str = r#"
import json, os, pathlib, subprocess, sys, tempfile
root = pathlib.Path(os.environ['CF91_ROOT'])
expected = pathlib.Path(os.environ['CF91_SOURCE'])
plan = os.environ['CF91_PLAN']
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

from charter.activation import kind_vocabulary
from charter.activation.catalog import resolve_offering_root
from charter.activation.identity_index import IdentityIndex
from charter.offering.artifact_kinds import ArtifactKind

KIND = ArtifactKind.TACTIC
rows = json.load(sys.stdin)
def index(project):
    return IdentityIndex(org_roots=[], project_root=pathlib.Path(project), offering_root=resolve_offering_root())
def declared(idx, token):
    try:
        value = idx.declared_id(KIND, token)
    except BaseException as exc:
        return {'error': f'{type(exc).__name__}: {exc}'}
    if value is None:
        return {'declared': None}
    return {'declared_len': len(value), 'declared_head': value[:16]}
def write(relative, body):
    path = pathlib.Path(relative)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding='utf-8')
    return path

plan_rows = {}
for label, body, relative in rows:
    write(relative, body)
    plan_rows[label] = declared(index('project-' + label), 'cf91-' + label)

write('retry/tactic/cf91-a-broken.tactic.yaml', 'id: [unclosed\n')
write('retry/tactic/cf91-a-fast.tactic.yaml', 'id: cf91-fastplain\n')
write('retry/tactic/cf91-a.tactic.yaml', 'id: cf91-alpha\n')
b_path = write('retry/tactic/cf91-b.tactic.yaml', 'id: ' + ('0x1' + '0' * 600 if plan == 'limit640' else '0x2' + '0' * 3571) + '\n')
retry = index('retry')
retry_rows = {'first': declared(retry, 'cf91-b')}
write('retry/tactic/cf91-b.tactic.yaml', 'id: cf91-beta-fixed\n')
write('retry/tactic/cf91-a-broken.tactic.yaml', 'id: cf91-nu-fixed\n')
write('retry/tactic/cf91-a-fast.tactic.yaml', 'id: cf91-fast-rewritten\n')
retry_rows['b'] = declared(retry, 'cf91-b')
retry_rows['a'] = declared(retry, 'cf91-a')
retry_rows['broken'] = declared(retry, 'cf91-a-broken')
retry_rows['nu'] = declared(retry, 'cf91-nu-fixed')
retry_rows['fast'] = declared(retry, 'cf91-a-fast')
fresh = index('retry')
retry_rows['fresh_broken'] = declared(fresh, 'cf91-a-broken')
retry_rows['fresh_nu'] = declared(fresh, 'cf91-nu-fixed')
retry_rows['fresh_fast'] = declared(fresh, 'cf91-a-fast')
plan_rows['retry'] = retry_rows

write('typed/tactic/cf91-typed.tactic.yaml', 'id: !!set {? [[a]], b: null}\n')
plan_rows['typeerror'] = {'typed': declared(index('typed'), 'cf91-typed')}

write('gate/.kittify/config.yaml', 'charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n')
write('gate/packs/acme/org-charter.yaml', '{}\n')
write('gate/packs/acme/tactics/cf91-gate-huge.tactic.yaml', 'id: 0x2' + '0' * 3571 + '\n')
write('gate/.kittify/charter/activation.yaml', 'activated_tactics: [cf91-gate-huge]\n')
if plan == 'unlimited':
    plan_rows['gate'] = {'skipped': True}
else:
    try:
        gate_idx = IdentityIndex(org_roots=[pathlib.Path('gate/packs/acme')], project_root=None, offering_root=resolve_offering_root())
        warnings = []
        kept = gate_idx.activation_id(KIND, 'cf91-gate-huge', warnings)
    except BaseException as exc:
        plan_rows['gate'] = {'error': f'{type(exc).__name__}: {exc}'}
    else:
        plan_rows['gate'] = {'kept': 1, 'id': kept}
print('CF91_PLAN ' + json.dumps(plan_rows, sort_keys=True))
"#;

#[test]
fn identity_text_phase_plan_matches_the_pinned_source() {
    if let Some(value) = std::env::var_os("CF91_NATIVE") {
        let root = PathBuf::from(value);
        let plan = std::env::var("CF91_PLAN").unwrap_or_else(|_| "default".to_owned());
        native_environment_guards(&root, &plan);
        verify_oracle();
        let observed = native_plan(&root, &plan);
        println!("CF91_PLAN {}", serde_json::to_string(&observed).unwrap());
        return;
    }

    for plan in ["default", "limit640", "unlimited"] {
        let payload: Vec<(String, String, String)> = rows(plan)
            .into_iter()
            .map(|(label, body)| {
                (
                    label.to_owned(),
                    body,
                    format!("project-{label}/tactic/cf91-{label}.tactic.yaml"),
                )
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
        let reference = receipt(&output.stdout, "CF91_PLAN ");
        assert_source_classes(plan, &reference);

        let native_root = fixture();
        assert_ne!(source_root.path(), native_root.path());
        let mut native = Command::new(std::env::current_exe().unwrap());
        // The Native child is re-invoked on this same test; it returns before the
        // parent branch because CF91_NATIVE is set in its environment.
        environment(&mut native, native_root.path(), plan);
        let output = native
            .env("CF91_NATIVE", native_root.path())
            .args([
                "--exact",
                "identity_text_phase_plan_matches_the_pinned_source",
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
        let observed = receipt(&output.stdout, "CF91_PLAN ");
        assert_eq!(
            observed, reference,
            "plan {plan}: identical relative business arguments, raw messages, no rewriting"
        );

        if plan == "default" {
            // Native-only: CPython never starts for an invalid ceiling, so this
            // is a typed start-up refusal, not a Source business exception.
            let startup_root = fixture();
            let mut startup = Command::new(std::env::current_exe().unwrap());
            environment(&mut startup, startup_root.path(), plan);
            let output = startup
                .env("CF91_NATIVE", startup_root.path())
                .env("CF91_STARTUP", "1")
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

/// Native-only: an invalid `PYTHONINTMAXSTRDIGITS` is a typed start-up refusal.
fn native_startup_policy(root: &Path) -> Value {
    let project = root.join("startup");
    let tactics = project.join("tactic");
    std::fs::create_dir_all(&tactics).expect("startup tactic dir");
    write_row(
        &tactics,
        "cf91-startup.tactic.yaml",
        &format!("id: {}\n", hex_4301()),
    );
    match index(&project).try_declared_id(KIND, "cf91-startup") {
        Ok(other) => json!({"class": "ok", "declared": other}),
        Err(error) => json!({
            "abort": error.abort().map(|abort| abort.python),
            "native": error.native().is_some(),
            "mentions_integer_policy": error.to_string().contains("integer conversion"),
        }),
    }
}

#[test]
fn invalid_integer_policy_is_a_typed_native_startup_refusal() {
    if std::env::var_os("CF91_STARTUP").is_none() {
        return;
    }
    let root = PathBuf::from(std::env::var_os("CF91_NATIVE").expect("startup child root"));
    let plan = std::env::var("CF91_PLAN").unwrap_or_else(|_| "default".to_owned());
    native_environment_guards(&root, &plan);
    assert_eq!(std::env::var("PYTHONINTMAXSTRDIGITS").unwrap(), "639");
    let observed = native_startup_policy(&root);
    assert_eq!(observed["class"], Value::Null);
    assert_eq!(observed["native"], true);
    assert_eq!(observed["mentions_integer_policy"], true);
}

fn assert_source_classes(plan: &str, reference: &Value) {
    let digits = if plan == "limit640" { 640 } else { 4300 };
    let message = format!(
        "ValueError: Exceeds the limit ({digits} digits) for integer string conversion; \
         use sys.set_int_max_str_digits() to increase the limit"
    );
    let mut failure_rows = vec!["hex", "nested"];
    if plan == "default" {
        failure_rows.push("boundary");
    }
    if plan == "unlimited" {
        // 0 disables the ceiling; the huge spelling renders.
        assert_eq!(reference["hex"]["declared_len"], 4301, "{plan} hex");
        assert_eq!(reference["nested"]["declared_len"], 4303, "{plan} nested");
        assert_eq!(reference["retry"]["first"]["declared_len"], 4301);
        assert_eq!(
            reference["retry"]["b"]["declared_len"], 4301,
            "a successful scan caches the first result for the same instance"
        );
    } else {
        for row in failure_rows {
            if row == "boundary" {
                assert_eq!(
                    reference["boundary"]["declared_len"], 4300,
                    "4,300 decimal digits is exactly at the default ceiling"
                );
            } else {
                assert_eq!(reference[row]["error"], message, "{plan} {row}");
            }
        }
        if plan == "default" {
            assert_eq!(reference["retry"]["first"]["error"], message);
        }
    }
    assert_eq!(reference["text"]["declared_head"], "cf91-alpha");
    assert_eq!(reference["fast"]["declared_head"], "cf91-fastid");
    if plan != "unlimited" {
        assert_eq!(reference["retry"]["b"]["declared_head"], "cf91-beta-fixed");
    }
    assert_eq!(reference["retry"]["a"]["declared_head"], "cf91-alpha");
    assert_eq!(
        reference["retry"]["broken"]["declared"],
        Value::Null,
        "a caught skip is a completed read"
    );
    assert_eq!(reference["retry"]["nu"]["declared"], Value::Null);
    assert_eq!(
        reference["retry"]["fast"]["declared_head"],
        "cf91-fastplain"
    );
    assert_eq!(
        reference["retry"]["fresh_broken"]["declared_head"],
        "cf91-nu-fixed"
    );
    assert_eq!(
        reference["retry"]["fresh_nu"]["declared_head"],
        "cf91-nu-fixed"
    );
    assert_eq!(
        reference["retry"]["fresh_fast"]["declared_head"],
        "cf91-fast-rewrit"
    );
    assert_eq!(
        reference["typeerror"]["typed"]["declared"],
        Value::Null,
        "Source _parse_id catches TypeError and returns None (no stem fallback)"
    );
    // The actual allow-filter consumer: the Source identity token loop aborts.
    if plan == "unlimited" {
        assert_eq!(reference["gate"]["skipped"], true);
    } else {
        assert_eq!(reference["gate"]["error"], message, "{plan} gate");
    }
}
