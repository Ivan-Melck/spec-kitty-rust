//! CF92 — exact native filename identity discovery and primary stems.
//!
//! Business controls over the pre-existing public identity/deny APIs: a file whose
//! native name has a non-UTF8 prefix is discovered exactly as Source
//! `pathlib`/`os.fsdecode` spells it, its config stem keeps the raw prefix (no
//! strip, no replacement), the declared id/URN association survives the deny
//! algebra, and the checked scalar stored-spelling adapter fails explicitly
//! instead of inventing an empty spelling. Other scalar enumeration callers
//! remain a separate required integration gap.
//!
//! The guarded pair runs the actual pinned Source and Native children in independent
//! business trees, each asserting its own roots before the first business read.

mod support;

use serde_json::{Value, json};
use sk_charter::ArtifactKind;
use sk_doctrine::{
    CanonicalOfferingService, IdentityIndex, OfferingContext, OfferingRoots, resolve_deny_exact,
};
use sk_kernel::PythonString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const KIND: ArtifactKind = ArtifactKind::Tactic;

const ROOTS: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn built_in_root() -> PathBuf {
    repo().join("packs/built-in")
}

fn points(text: &PythonString) -> Vec<u32> {
    text.codepoints().to_vec()
}

fn native_index(root: &Path) -> IdentityIndex {
    IdentityIndex::from_roots(&built_in_root(), &[root.join("org")], None)
}

/// One fixture row: raw native filename bytes, body, and the Source spellings.
struct Row {
    name: &'static [u8],
    body: &'static str,
    /// Source `_config_stem` spelling (`name.split('.', 1)[0]`).
    stem: &'static [u32],
    /// Declared id the body carries, or the parsed-stem fallback when absent.
    declared: &'static [u32],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            name: b"\xff-one.tactic.yaml",
            body: "id: cf92-ff\nschema_version: \"1.0\"\nname: CF92 Raw Filename\nsteps:\n  - title: One\n",
            stem: &[0xDCFF, 0x2D, 0x6F, 0x6E, 0x65],
            declared: &[0x63, 0x66, 0x39, 0x32, 0x2D, 0x66, 0x66],
        },
        Row {
            name: b"\xfe-no-id.tactic.yaml",
            body: "schema_version: \"1.0\"\n",
            stem: &[0xDCFE, 0x2D, 0x6E, 0x6F, 0x2D, 0x69, 0x64],
            declared: &[0xDCFE, 0x2D, 0x6E, 0x6F, 0x2D, 0x69, 0x64],
        },
        Row {
            name: b" pad.tactic.yaml",
            body: "id: cf92-space\nschema_version: \"1.0\"\n",
            stem: &[0x20, 0x70, 0x61, 0x64],
            declared: &[0x63, 0x66, 0x39, 0x32, 0x2D, 0x73, 0x70, 0x61, 0x63, 0x65],
        },
        Row {
            name: "é-unicode.tactic.yaml".as_bytes(),
            body: "id: cf92-unicode\nschema_version: \"1.0\"\n",
            stem: &[0xE9, 0x2D, 0x75, 0x6E, 0x69, 0x63, 0x6F, 0x64, 0x65],
            declared: &[
                0x63, 0x66, 0x39, 0x32, 0x2D, 0x75, 0x6E, 0x69, 0x63, 0x6F, 0x64, 0x65,
            ],
        },
        Row {
            name: b"plain.tactic.yaml",
            body: "id: cf92-plain\nschema_version: \"1.0\"\n",
            stem: &[0x70, 0x6C, 0x61, 0x69, 0x6E],
            declared: &[0x63, 0x66, 0x39, 0x32, 0x2D, 0x70, 0x6C, 0x61, 0x69, 0x6E],
        },
    ]
}

#[cfg(unix)]
fn raw_path(folder: &Path, name: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    folder.join(std::ffi::OsStr::from_bytes(name))
}

fn case_receipt(index: &IdentityIndex, stem: &PythonString, declared: &PythonString) -> Value {
    let by_stem = index
        .try_declared_id_exact(KIND, stem)
        .map_err(|error| format!("{error}"))
        .expect("stem lookup");
    let by_declared = index
        .try_stem_exact(KIND, declared)
        .map_err(|error| format!("{error}"))
        .expect("declared-id lookup");
    #[cfg(unix)]
    let name_bytes = index.path_for_exact(KIND, stem).map(|path| {
        use std::os::unix::ffi::OsStrExt;
        path.file_name().unwrap().as_bytes().to_vec()
    });
    #[cfg(not(unix))]
    let name_bytes: Option<Vec<u8>> = None;
    json!({
        "by_stem": by_stem.as_ref().map(points),
        "by_declared": by_declared.as_ref().map(points),
        "knows": index.try_knows_exact(KIND, stem).expect("knows"),
        "name_bytes": name_bytes,
    })
}

fn native_business(root: &Path) -> Value {
    let folder = root.join("org/tactics");
    fs::create_dir_all(&folder).unwrap();
    let rows = rows();
    #[cfg(unix)]
    for row in &rows {
        fs::write(raw_path(&folder, row.name), row.body).expect("fixture body");
    }
    #[cfg(unix)]
    {
        fs::write(
            raw_path(&folder, b"same.\xff.tactic.yaml"),
            "id: cf92-order-raw\n",
        )
        .unwrap();
        fs::write(folder.join("same.𐀀.tactic.yaml"), "id: cf92-order-astral\n").unwrap();
    }
    let offering = CanonicalOfferingService::new(OfferingContext::new(
        root.to_path_buf(),
        OfferingRoots {
            built_in: Some(built_in_root()),
            org_roots: vec![root.join("org")],
            project: None,
        },
        None,
    ));
    let admitted_raw = offering
        .artifact_ids(KIND)
        .unwrap()
        .contains(&PythonString::from_scalars("cf92-ff"));
    let index = native_index(root);
    let ordering = index
        .try_declared_id_exact(KIND, &PythonString::from_scalars("same"))
        .unwrap();
    let stored = index
        .stored_spelling_exact(KIND, &PythonString::from_scalars("cf92-ff"))
        .unwrap();
    let mut cases = serde_json::Map::new();
    for (position, row) in rows.iter().enumerate() {
        let stem = PythonString::from_codepoints(row.stem).unwrap();
        let declared = PythonString::from_codepoints(row.declared).unwrap();
        cases.insert(
            format!("row{position}"),
            case_receipt(&index, &stem, &declared),
        );
    }

    // The deny algebra reads the same exact identity: a raw-prefix token spells the
    // declared id and its URN, not a minted unknown token.
    let ff_stem = PythonString::from_codepoints(rows[0].stem).unwrap();
    let deny = resolve_deny_exact(KIND, [ff_stem.clone()], &index).expect("deny spelling");
    let set = |items: std::collections::BTreeSet<PythonString>| -> Vec<Vec<u32>> {
        items.iter().map(points).collect()
    };
    let deny_receipt = json!({
        "stems": set(deny.stems),
        "declared_ids": set(deny.declared_ids),
        "urns": set(deny.urns),
    });

    // Warm/new-index precedence: a completed read survives a rewrite on the same
    // index and a fresh index re-reads the tree, exactly as before this change.
    #[cfg(unix)]
    {
        let ff_path = raw_path(&folder, rows[0].name);
        fs::write(&ff_path, "id: cf92-ff-two\nschema_version: \"1.0\"\n").unwrap();
    }
    let warm = index
        .try_declared_id_exact(KIND, &ff_stem)
        .expect("same-instance read");
    let fresh = native_index(root)
        .try_declared_id_exact(KIND, &ff_stem)
        .expect("fresh-index read");
    let warm_receipt = json!({
        "same_index": warm.as_ref().map(points),
        "fresh_index": fresh.as_ref().map(points),
    });

    json!({"cases": cases, "deny": deny_receipt, "warm": warm_receipt,
        "ordering": ordering.as_ref().map(points), "stored": points(&stored), "admitted_raw": admitted_raw})
}

const SOURCE: &str = r#"
import json, os, sys, subprocess
from pathlib import Path
root = Path(os.environ['CF92_PAIR_ROOT'])
expected = Path(os.environ['CF92_PAIR_SOURCE'])
assert root.is_absolute() and expected.is_absolute() and Path.cwd() == root
for key, name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','config-dirs'),('XDG_DATA_DIRS','data-dirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert Path(os.environ[key]) == root / name
assert Path(os.environ['PYTHONPATH']) == expected / 'src'
assert Path(os.environ['SPEC_KITTY_PACKS_ROOT']) == expected / 'packs'
assert os.environ['PYTHONHASHSEED'] == '0'
assert sys.version_info[:3] == (3,11,15)
assert (sys.getfilesystemencoding(),sys.getfilesystemencodeerrors()) == ('utf-8','surrogateescape')
assert (sys.stderr.encoding,sys.stderr.errors) == ('utf-8','backslashreplace')
assert (sys.stdout.encoding,sys.stdout.errors) == ('utf-8','surrogateescape')
import charter
assert Path(charter.__file__).resolve().is_relative_to(expected / 'src')
proof = json.loads(subprocess.check_output([str(expected / '.venv-rust-oracle/bin/python'), str(expected / 'scripts/rust/verify_oracle.py'), '--root', str(expected)], cwd=expected, text=True))
assert proof['ok'] and proof['python'] == '3.11.15' and proof['checked_runtime_requirements'] == 53
from charter.activation import identity_index as ii
from charter.activation.activation_ledger import resolve_deny
from charter.offering.artifact_kinds import ArtifactKind
K = ArtifactKind.TACTIC
folder = root / 'org/tactics'
folder.mkdir(parents=True)
ROWS = [
    (b'\xff-one.tactic.yaml', 'id: cf92-ff\nschema_version: "1.0"\nname: CF92 Raw Filename\nsteps:\n  - title: One\n'),
    (b'\xfe-no-id.tactic.yaml', 'schema_version: "1.0"\n'),
    (b' pad.tactic.yaml', 'id: cf92-space\nschema_version: "1.0"\n'),
    ('é-unicode.tactic.yaml'.encode('utf-8'), 'id: cf92-unicode\nschema_version: "1.0"\n'),
    (b'plain.tactic.yaml', 'id: cf92-plain\nschema_version: "1.0"\n'),
]
for raw, body in ROWS:
    (folder / os.fsdecode(raw)).write_text(body, encoding='utf-8')
def index():
    return ii.IdentityIndex.from_roots(offering_root=expected / 'src/charter/offering', org_roots=[root / 'org'], layer_roots=None)
(folder / os.fsdecode(b'same.\xff.tactic.yaml')).write_text('id: cf92-order-raw\n', encoding='utf-8')
(folder / 'same.𐀀.tactic.yaml').write_text('id: cf92-order-astral\n', encoding='utf-8')
from charter.offering.tactics.repository import TacticRepository
admitted_raw = TacticRepository(built_in_dir=expected / 'packs/built-in/tactics', org_dirs=[folder]).get('cf92-ff') is not None
assert admitted_raw
idx = index()
def cp(text):
    return None if text is None else [ord(ch) for ch in text]
cases = {}
for position, (raw, _body) in enumerate(ROWS):
    stem = os.fsdecode(raw).split('.', 1)[0]
    declared = idx.declared_id(K, stem)
    by_stem = idx.declared_id(K, stem)
    by_declared = idx.stem(K, declared) if declared is not None else None
    path = next(entry.path for entry in idx._table(K).entries if entry.stem == stem)
    cases['row%d' % position] = {
        'by_stem': cp(by_stem),
        'by_declared': cp(by_declared),
        'knows': idx.knows(K, stem),
        'name_bytes': list(os.fsencode(path.name)),
    }
ff_stem = os.fsdecode(ROWS[0][0]).split('.', 1)[0]
deny = resolve_deny(K, [ff_stem], idx)
deny_receipt = {
    'stems': sorted([ord(ch) for ch in item] for item in deny.stems),
    'declared_ids': sorted([ord(ch) for ch in item] for item in deny.declared_ids),
    'urns': sorted([ord(ch) for ch in item] for item in deny.urns),
}
(folder / os.fsdecode(ROWS[0][0])).write_text('id: cf92-ff-two\nschema_version: "1.0"\n', encoding='utf-8')
fresh_idx = index()
warm_receipt = {
    'same_index': cp(idx.declared_id(K, ff_stem)),
    'fresh_index': cp(fresh_idx.declared_id(K, ff_stem)),
}
receipt = {'cases': cases, 'deny': deny_receipt, 'warm': warm_receipt,
    'ordering': cp(idx.declared_id(K, 'same')), 'stored': cp(idx.stored_spelling(K, 'cf92-ff')), 'admitted_raw': admitted_raw}
(root / 'receipt.json').write_text(json.dumps(receipt, sort_keys=True), encoding='utf-8')
"#;

fn command_env(command: &mut Command, root: &Path) {
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CF92_PAIR_ROOT", root)
        .env("CF92_PAIR_SOURCE", repo())
        .env("PYTHONPATH", repo().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo().join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("LC_ALL", "C.UTF-8")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in ROOTS {
        fs::create_dir_all(root.join(name)).expect("env root");
        command.env(key, root.join(name));
    }
}

fn native_guard(root: &Path) {
    assert!(root.is_absolute());
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name))
        );
    }
    for (key, expected) in [
        ("CF92_PAIR_SOURCE", repo()),
        ("PYTHONPATH", repo().join("src")),
        ("SPEC_KITTY_PACKS_ROOT", repo().join("packs")),
    ] {
        assert_eq!(std::env::var_os(key).map(PathBuf::from), Some(expected));
    }
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    let stream = sk_kernel::python_stream::StreamPolicy::for_stderr().unwrap();
    assert_eq!(stream.encoding(), "utf-8");
    assert_eq!(stream.errors(), "backslashreplace");
    let stdout = sk_kernel::python_stream::StreamPolicy::from_environment().unwrap();
    assert_eq!(
        (stdout.encoding(), stdout.errors()),
        ("utf-8", "surrogateescape")
    );
    let expected = repo();
    let output = Command::new(expected.join(".venv-rust-oracle/bin/python"))
        .arg(expected.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(&expected)
        .current_dir(&expected)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let proof: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["python"], "3.11.15");
    assert_eq!(proof["checked_runtime_requirements"], 53);
}

#[test]
fn guarded_source_native_exact_filename_pair() {
    if let Some(root) = std::env::var_os("CF92_PAIR_ROOT").map(PathBuf::from) {
        native_guard(&root);
        fs::write(
            root.join("receipt.json"),
            serde_json::to_vec(&native_business(&root)).unwrap(),
        )
        .unwrap();
        return;
    }
    let source = tempfile::tempdir().unwrap();
    let native = tempfile::tempdir().unwrap();
    assert_ne!(source.path(), native.path());

    let mut source_cmd = Command::new(support::python());
    command_env(&mut source_cmd, source.path());
    let output = source_cmd.args(["-c", SOURCE]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source_receipt: Value =
        serde_json::from_slice(&fs::read(source.path().join("receipt.json")).unwrap()).unwrap();

    assert_eq!(source_receipt["admitted_raw"], true);
    assert_eq!(
        source_receipt["ordering"],
        json!(points(&PythonString::from_scalars("cf92-order-raw")))
    );
    assert_eq!(
        source_receipt["stored"],
        json!([0xdcff, 0x2d, 0x6f, 0x6e, 0x65])
    );
    let mut native_cmd = Command::new(std::env::current_exe().unwrap());
    command_env(&mut native_cmd, native.path());
    let output = native_cmd
        .args([
            "--exact",
            "guarded_source_native_exact_filename_pair",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let native_receipt: Value =
        serde_json::from_slice(&fs::read(native.path().join("receipt.json")).unwrap()).unwrap();

    assert_eq!(source_receipt, native_receipt, "business receipts differ");
    assert_eq!(
        source_receipt["cases"]["row0"]["by_stem"],
        json!([0x63, 0x66, 0x39, 0x32, 0x2D, 0x66, 0x66])
    );
    assert_eq!(
        source_receipt["cases"]["row1"]["by_stem"],
        json!([0xDCFE, 0x2D, 0x6E, 0x6F, 0x2D, 0x69, 0x64])
    );
    assert_eq!(
        source_receipt["cases"]["row2"]["by_stem"],
        json!([0x63, 0x66, 0x39, 0x32, 0x2D, 0x73, 0x70, 0x61, 0x63, 0x65])
    );
    assert_eq!(
        source_receipt["deny"]["declared_ids"],
        json!([[0x63, 0x66, 0x39, 0x32, 0x2D, 0x66, 0x66]])
    );
    assert_eq!(
        source_receipt["warm"]["same_index"],
        json!([0x63, 0x66, 0x39, 0x32, 0x2D, 0x66, 0x66])
    );
    assert_eq!(
        source_receipt["warm"]["fresh_index"],
        json!([
            0x63, 0x66, 0x39, 0x32, 0x2D, 0x66, 0x66, 0x2D, 0x74, 0x77, 0x6F
        ])
    );
}

#[cfg(unix)]
#[test]
fn scalar_stored_spelling_cannot_succeed_with_an_empty_replacement() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("org/tactics");
    fs::create_dir_all(&folder).unwrap();
    fs::write(raw_path(&folder, b"\xff-one.tactic.yaml"), "id: cf92-ff\n").unwrap();
    let index = native_index(root.path());
    let exact = index
        .stored_spelling_exact(KIND, &PythonString::from_scalars("cf92-ff"))
        .unwrap();
    assert_eq!(points(&exact), vec![0xdcff, 0x2d, 0x6f, 0x6e, 0x65]);
    let error = index
        .stored_spelling(KIND, "cf92-ff")
        .expect_err("a checked scalar adapter cannot replace a Source stem with empty text");
    assert!(std::error::Error::source(&error).is_some());
    match error {
        sk_doctrine::DoctrineError::IdentityQuery { source } => {
            match source
                .native()
                .expect("projection remains a typed Native failure")
            {
                sk_doctrine::NativeOfferingFailure::DiagnosticProjection {
                    exact,
                    codepoint,
                    ..
                } => {
                    assert_eq!(*codepoint, 0xdcff);
                    assert_eq!(points(exact), vec![0xdcff, 0x2d, 0x6f, 0x6e, 0x65]);
                }
                other => panic!("wrong Native projection cause: {other:?}"),
            }
        }
        other => panic!("wrong checked scalar error: {other:?}"),
    }
    fs::write(
        folder.join("huge.tactic.yaml"),
        format!("id: !!int 0x2{}\n", "0".repeat(3571)),
    )
    .unwrap();
    let failure = native_index(root.path())
        .stored_spelling_exact(KIND, &PythonString::from_scalars("unknown"))
        .expect_err("an aborted identity scan cannot become a successful unknown spelling");
    match failure {
        sk_doctrine::DoctrineError::IdentityQuery { source } => {
            assert_eq!(
                source.abort().expect("actual Source abort").python,
                "ValueError"
            );
        }
        other => panic!("wrong checked identity failure: {other:?}"),
    }
}
