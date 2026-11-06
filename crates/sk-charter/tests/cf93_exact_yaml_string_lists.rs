//! CF93 bounded serialization, not the activation CLI or publication engine.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sk_kernel::PythonString;

const ROOTS: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

type Input = Vec<Vec<(String, Vec<Vec<u32>>)>>;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn points(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

fn input() -> Input {
    let field = |values| vec![("activated_tactics".to_owned(), values)];
    let mut raw = points("cf93-");
    raw.push(0xDCFF);
    let mut spaced = points(&"prefix words with spaces ".repeat(8));
    spaced.extend([0xD800, 0xDCFF]);
    spaced.extend(points(&" after 'quoted' words \"double\" ".repeat(8)));
    vec![
        vec![],
        vec![
            ("activated_tactics".to_owned(), vec![]),
            ("activated_skills".to_owned(), vec![]),
        ],
        field(vec![
            raw,
            points("ordinary"),
            points("yes"),
            points("true"),
            points("null"),
            points("0x10"),
        ]),
        field((0xD800..=0xDFFF).map(|point| vec![point]).collect()),
        field(vec![(0xD800..=0xDFFF).collect()]),
        field(vec![
            vec![0xD800, 0xDC00],
            vec![0x10000],
            vec![0xDBFF, 0xDFFF],
            vec![0x10FFFF],
        ]),
        field(
            (0..=0x9F)
                .map(|point| vec![point])
                .chain([vec![0x2028, 0x2029, 0xFEFF, 0xFFFE, 0xFFFF]])
                .collect(),
        ),
        field(vec![
            vec![0xE000, 0xF8FF, 0xFDD0, 0x1FFFE, 0xF0000, 0x10FFFE],
            spaced,
        ]),
        field(vec![
            points(&"long words with single spaces ".repeat(12)),
            points(" lead "),
            points("a:b # c [d] {e}\\f\nnext"),
            points("_"),
            points("__"),
            points("_1"),
            points("+_"),
            points("-_"),
        ]),
    ]
}

fn environment(command: &mut Command, root: &Path) {
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CF93_ROOT", root)
        .env("CF93_SOURCE", repo())
        .env("PYTHONPATH", repo().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo().join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONIOENCODING", "utf-8:surrogateescape")
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("LC_ALL", "C.UTF-8");
    for (key, name) in ROOTS {
        fs::create_dir_all(root.join(name)).unwrap();
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
        ("CF93_SOURCE", repo()),
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
    let expected = repo();
    let proof = Command::new(expected.join(".venv-rust-oracle/bin/python"))
        .arg(expected.join("scripts/rust/verify_oracle.py"))
        .arg("--root")
        .arg(&expected)
        .current_dir(&expected)
        .output()
        .unwrap();
    assert!(proof.status.success(), "{proof:?}");
    let proof: Value = serde_json::from_slice(&proof.stdout).unwrap();
    assert_eq!(proof["ok"], true);
    assert_eq!(proof["python"], "3.11.15");
    assert_eq!(proof["checked_runtime_requirements"], 53);
}

fn native_business(root: &Path) {
    for text in ["_", "__", "_1"] {
        let graph = sk_kernel::python_yaml::decode_graph(
            &format!("v: {text}\n"),
            sk_kernel::python_yaml::Dialect::RuamelSafe12,
        )
        .unwrap();
        assert_eq!(
            graph.root_view().lookup("v").unwrap().as_str().unwrap(),
            &PythonString::from_scalars(text)
        );
    }
    let input: Input = serde_json::from_slice(&fs::read("input.json").unwrap()).unwrap();
    let mut receipts = Vec::new();
    for (case, fields) in input.into_iter().enumerate() {
        let fields: Vec<(String, Vec<PythonString>)> = fields
            .into_iter()
            .map(|(key, values)| {
                (
                    key,
                    values
                        .into_iter()
                        .map(|units| PythonString::from_codepoints(&units).unwrap())
                        .collect(),
                )
            })
            .collect();
        let bytes = sk_charter::ordered_yaml_string_lists(&fields);
        let graph = sk_kernel::python_yaml::decode_graph(
            std::str::from_utf8(&bytes).unwrap(),
            sk_kernel::python_yaml::Dialect::RuamelSafe12,
        )
        .unwrap();
        let mut decoded = Vec::new();
        for (key, values) in &fields {
            let actual = graph.root_view().lookup(key).unwrap();
            let actual: Vec<_> = actual
                .as_sequence()
                .unwrap()
                .iter()
                .map(|id| actual.view(*id).as_str().unwrap().codepoints().to_vec())
                .collect();
            if (2..=5).contains(&case) {
                assert_eq!(
                    actual,
                    values
                        .iter()
                        .map(|value| value.codepoints().to_vec())
                        .collect::<Vec<_>>(),
                    "full surrogate roundtrip case {case}"
                );
            }
            decoded.push((key, actual));
        }
        if fields
            .iter()
            .flat_map(|(_, values)| values)
            .all(|text| text.to_scalar_string().is_ok())
        {
            let mut finite = serde_yaml_ng::Mapping::new();
            for (key, values) in &fields {
                finite.insert(
                    serde_yaml_ng::Value::String(key.clone()),
                    serde_yaml_ng::Value::Sequence(
                        values
                            .iter()
                            .map(|text| {
                                serde_yaml_ng::Value::String(text.to_scalar_string().unwrap())
                            })
                            .collect(),
                    ),
                );
            }
            assert_eq!(
                bytes,
                sk_charter::ordered_yaml(&serde_yaml_ng::Value::Mapping(finite))
            );
        }
        fs::write(root.join(format!("case{case}.yaml")), &bytes).unwrap();
        receipts.push(serde_json::json!({"bytes": bytes, "decoded": decoded}));
    }
    fs::write("receipt.json", serde_json::to_vec(&receipts).unwrap()).unwrap();
}

const SOURCE: &str = r#"
import json, os, sys, subprocess
from pathlib import Path
root = Path(os.environ['CF93_ROOT'])
source = Path(os.environ['CF93_SOURCE'])
assert root.is_absolute() and source.is_absolute() and Path.cwd() == root
for key, name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','config-dirs'),('XDG_DATA_DIRS','data-dirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert Path(os.environ[key]) == root / name
assert Path(os.environ['PYTHONPATH']) == source / 'src'
assert Path(os.environ['SPEC_KITTY_PACKS_ROOT']) == source / 'packs'
assert os.environ['PYTHONHASHSEED'] == '0'
assert sys.version_info[:3] == (3,11,15) and sys.float_info.mant_dig == 53
assert (sys.getfilesystemencoding(),sys.getfilesystemencodeerrors()) == ('utf-8','surrogateescape')
assert (sys.stdout.encoding,sys.stdout.errors) == ('utf-8','surrogateescape')
assert (sys.stderr.encoding,sys.stderr.errors) == ('utf-8','backslashreplace')
proof = json.loads(subprocess.check_output([str(source / '.venv-rust-oracle/bin/python'), str(source / 'scripts/rust/verify_oracle.py'), '--root', str(source)], cwd=source))
assert proof['ok'] and proof['python'] == '3.11.15' and proof['checked_runtime_requirements'] == 53
from charter.activation import charter_yaml_io as owner
assert Path(owner.__file__).resolve() == source / 'src/charter/activation/charter_yaml_io.py'
from charter.offering.pack_paths import built_in_root
assert built_in_root() == source / 'packs/built-in'
from ruamel.yaml import YAML
for text in ['_', '__', '_1']:
    assert YAML().load('v: ' + text + '\n')['v'] == text
cases = json.loads(Path('input.json').read_text())
receipts = []
for case, fields in enumerate(cases):
    data = {key: [''.join(map(chr, units)) for units in values] for key, values in fields}
    try:
        raw = owner.render_yaml_document(None, data, YAML())
    except Exception as error:
        from io import StringIO
        stream = StringIO()
        YAML().dump(data, stream)
        diagnostic = stream.getvalue()
        Path('rejected-library-dump.yaml').write_text(diagnostic)
        decoded = YAML().load(diagnostic)
        Path('rejected-case.json').write_text(json.dumps({'case': case, 'class': type(error).__name__, 'message': str(error), 'decoded': {key: [list(map(ord, value)) for value in values] for key, values in decoded.items()}}))
        raise
    parsed = YAML().load(raw)
    assert owner.yaml_documents_equal(parsed, data)
    if 2 <= case <= 5:
        assert parsed == data
    Path(f'case{case}.yaml').write_bytes(raw)
    receipts.append({'bytes': list(raw), 'decoded': [[key, [list(map(ord, value)) for value in values]] for key, values in parsed.items()]})
Path('receipt.json').write_text(json.dumps(receipts, separators=(',', ':')))
"#;

#[test]
fn exact_source_writer_string_lists() {
    if let Some(root) = std::env::var_os("CF93_ROOT").map(PathBuf::from) {
        native_guard(&root);
        native_business(&root);
        return;
    }
    let source = tempfile::tempdir().unwrap().keep();
    let native = tempfile::tempdir().unwrap().keep();
    eprintln!("CF93 independent fixtures source={source:?} native={native:?}");
    assert_ne!(source, native);
    let fixtures = input();
    // Independent copies, never one shared/reset business fixture.
    fs::write(
        source.join("input.json"),
        serde_json::to_vec(&fixtures).unwrap(),
    )
    .unwrap();
    fs::write(
        native.join("input.json"),
        serde_json::to_vec(&fixtures).unwrap(),
    )
    .unwrap();
    let mut source_cmd = Command::new(repo().join(".venv-rust-oracle/bin/python"));
    environment(&mut source_cmd, &source);
    let source_output = source_cmd.args(["-c", SOURCE]).output().unwrap();
    fs::write(source.join("stdout"), &source_output.stdout).unwrap();
    fs::write(source.join("stderr"), &source_output.stderr).unwrap();
    assert!(source_output.status.success(), "{source_output:?}");
    assert!(source_output.stderr.is_empty(), "{source_output:?}");
    let source_receipt: Vec<Value> =
        serde_json::from_slice(&fs::read(source.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(source_receipt[2]["bytes"], serde_json::json!(b"activated_tactics:\n- \"cf93-\\uDCFF\"\n- ordinary\n- yes\n- 'true'\n- 'null'\n- '0x10'\n".as_slice()));
    let mut native_cmd = Command::new(std::env::current_exe().unwrap());
    environment(&mut native_cmd, &native);
    let native_output = native_cmd
        .args(["--exact", "exact_source_writer_string_lists"])
        .output()
        .unwrap();
    fs::write(native.join("stdout"), &native_output.stdout).unwrap();
    fs::write(native.join("stderr"), &native_output.stderr).unwrap();
    assert!(native_output.status.success(), "{native_output:?}");
    assert!(native_output.stderr.is_empty(), "{native_output:?}");
    let native_receipt: Vec<Value> =
        serde_json::from_slice(&fs::read(native.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(source_receipt.len(), fixtures.len());
    assert_eq!(source_receipt, native_receipt);
    for case in 0..fixtures.len() {
        assert_eq!(
            fs::read(source.join(format!("case{case}.yaml"))).unwrap(),
            fs::read(native.join(format!("case{case}.yaml"))).unwrap(),
            "case {case}"
        );
    }
}
