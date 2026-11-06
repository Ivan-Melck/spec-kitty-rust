//! Source-first binding for the one canonical Kernel JSON codec.
//!
//! Every expected value in this file comes from the pinned CPython 3.11.15
//! oracle (`.venv-rust-oracle`, verified by `scripts/rust/verify_oracle.py`)
//! running first under asserted isolated roots; the Native codec is only
//! exercised after all Source execution and assertions have passed. No stored
//! Native golden is used.

#[path = "support/oracle.rs"]
mod oracle_support;

use std::process::Command;

use sha2::Digest;
use sk_kernel::python_json::{
    PythonJsonDefault, PythonJsonErrorKind, PythonJsonRenderOptions, PythonJsonSeparators,
    PythonJsonValue, parse, parse_str_with_integer_digits,
};

/// Asserted isolated-root + pinned-interpreter header for every Source run.
const HEADER: &str = concat!(
    "import hashlib, json, os, sys\n",
    "from pathlib import Path\n",
    "assert sys.version.split()[0] == '3.11.15', sys.version\n",
    "assert sys.implementation.name == 'cpython', sys.implementation.name\n",
    "root = Path(os.environ['SK_J1_ROOT'])\n",
    "assert Path.cwd() == root, Path.cwd()\n",
    "for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME'):\n",
    "    assert Path(os.environ[key]).parent == root, (key, os.environ[key])\n",
    "assert os.environ.get('PYTHONDONTWRITEBYTECODE') == '1'\n",
);

/// Run a Source script under asserted isolated roots and return its JSON.
fn run_source(script: &str, args: &[&str]) -> serde_json::Value {
    let temp = tempfile::tempdir().expect("isolated Source roots");
    let root = temp.path().canonicalize().expect("canonical root");
    for name in ["home", "config", "cache", "data", "state", "tmp"] {
        std::fs::create_dir(root.join(name)).expect("isolated directory");
    }
    let output = Command::new(oracle_support::python())
        .arg("-c")
        .arg(format!("{HEADER}{script}"))
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("TMPDIR", root.join("tmp"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("SK_J1_ROOT", &root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(&root)
        .output()
        .expect("pinned Source oracle");
    assert!(
        output.status.success(),
        "Source must run and assert first: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Source JSON report")
}

fn codepoints(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

fn rendered(value: &PythonJsonValue, indent: Option<usize>) -> Vec<u32> {
    value.render_python(indent, true).codepoints().to_vec()
}

fn units(row: &serde_json::Value) -> Vec<u32> {
    row.as_array()
        .expect("code point array")
        .iter()
        .map(|unit| unit.as_u64().expect("code point") as u32)
        .collect()
}

#[test]
fn source_messages_positions_and_lines_match_the_canonical_reader() {
    let documents = [
        "",
        " ",
        "\u{feff}{}",
        "x",
        "{} x",
        "{} {}",
        "{}{}",
        "[] []",
        "[1 2]",
        "[1,]",
        r#"{"a": 1,}"#,
        r#"{"a" 1}"#,
        "{a: 1}",
        "{1: 2}",
        r#"{"a": 1 "b": 2}"#,
        "tru",
        "truex",
        "nullx",
        "falsy",
        "-",
        "- ",
        "+1",
        ".5",
        "1.",
        "1e",
        "1e+",
        "01",
        "-01",
        "1..2",
        "\"abc",
        r#""a\qb""#,
        "\"a\nb\"",
        "\"\u{0}\"",
        r#""\uZZZZ""#,
        r#""\u12""#,
        r#""\ud800\uZZZZ""#,
        r#""\ud800\u12""#,
        r#"{"\u12": 1}"#,
        "NaNx",
        "+Infinity",
        "-NaN",
        "nan",
        "inf",
        "Inf",
        "[\n1,\n2,\n3\n]x",
        "\"𝄞\" x",
        "\"\u{e9}\"\t\n  y",
        "[\"😀\", 1,\n 2,\n bad]",
        // Every value is validated even when a caller only reads one key.
        r#"{"a": 1, "a": }"#,
        r#"{"a": 1, "b": {"c": [1, 2, ]}}"#,
    ];
    let source = run_source(
        concat!(
            "documents = json.loads(sys.argv[1])\n",
            "report = []\n",
            "for document in documents:\n",
            "    try:\n",
            "        value = json.loads(document)\n",
            "    except json.JSONDecodeError as error:\n",
            "        report.append(['JSONDecodeError', error.msg, error.pos, error.lineno, error.colno, str(error)])\n",
            "    except ValueError as error:\n",
            "        report.append(['ValueError', str(error)])\n",
            "    else:\n",
            "        report.append(['OK', json.dumps(value)])\n",
            "assert all(row[0] != 'OK' for row in report), [row for row in report if row[0] == 'OK']\n",
            "print(json.dumps(report))\n",
        ),
        &[&serde_json::to_string(&documents[..]).expect("document list")],
    );
    let rows = source.as_array().expect("Source rows");
    assert_eq!(rows.len(), documents.len());
    for (document, row) in documents.iter().zip(rows) {
        let row = row.as_array().expect("row fields");
        match row[0].as_str().expect("kind") {
            "JSONDecodeError" => {
                let error = parse(document).expect_err(document);
                assert_eq!(
                    error.kind().message(),
                    row[1].as_str().expect("message"),
                    "{document:?}"
                );
                assert_eq!(
                    error.pos(),
                    row[2].as_u64().map(|value| value as usize),
                    "{document:?}"
                );
                assert_eq!(
                    error.line(),
                    row[3].as_u64().map(|value| value as usize),
                    "{document:?}"
                );
                assert_eq!(
                    error.column(),
                    row[4].as_u64().map(|value| value as usize),
                    "{document:?}"
                );
                assert_eq!(
                    error.to_string(),
                    row[5].as_str().expect("display"),
                    "{document:?}"
                );
            }
            other => panic!("unexpected Source outcome {other} for {document:?}"),
        }
    }
}

#[test]
fn source_documents_round_trip_byte_for_byte_through_the_canonical_codec() {
    let documents = [
        "null",
        "true",
        "false",
        "[]",
        "{}",
        r#"{"b": 1, "a": 2}"#,
        r#"{"b": 1, "a": 2, "b": 3}"#,
        r#"{"p": {"a": 1, "b": 2, "a": 3}}"#,
        r#"[1, NaN, {"z": 1, "y": 2}, true, null, [], {}]"#,
        r#"  { "a" : [ 1 , 2 ] }  "#,
        r#"{"a": "h\u00e9llo \u2603 \ud834\udd1e"}"#,
        r#"{"\u0000": "\u0000"}"#,
        "{\"a\": \"\u{e9}\u{2603}\u{1d11e}\"}",
        r#"{"a": "a\u00a0b"}"#,
        r#"{"a\u00a0b": 1}"#,
        r#"{"a": -Infinity, "b": Infinity, "c": NaN}"#,
        r#"{"a": 1e400, "b": -1e400, "c": 1e-400, "d": -1e-400}"#,
        r#"{"a": 123456789012345678901234567890}"#,
        r#"{"a": -0, "b": -0.0, "c": 0.0}"#,
        r#"{"a": "\u007f\u0080\u2028\u2029\ufeff"}"#,
        r#"{"a": "back\\slash \"quote\" \/ \b\f\n\r\t"}"#,
        r#"{"empty_object": {}, "empty_array": [], "nested": [[[]]]}"#,
    ];
    let source = run_source(
        concat!(
            "documents = json.loads(sys.argv[1])\n",
            "report = []\n",
            "for document in documents:\n",
            "    value = json.loads(document)\n",
            "    report.append({\n",
            "        'repr': repr(value),\n",
            "        'default': json.dumps(value),\n",
            "        'indent0': json.dumps(value, indent=0),\n",
            "        'indent2': json.dumps(value, indent=2),\n",
            "        'indent4': json.dumps(value, indent=4),\n",
            "        'sorted': json.dumps(value, sort_keys=True),\n",
            "        'sorted2': json.dumps(value, sort_keys=True, indent=2),\n",
            "        'utf8': [ord(c) for c in json.dumps(value, ensure_ascii=False)],\n",
            "    })\n",
            "print(json.dumps(report))\n",
        ),
        &[&serde_json::to_string(&documents).expect("document list")],
    );
    let rows = source.as_array().expect("Source rows");
    for (document, row) in documents.iter().zip(rows) {
        let value = parse(document).unwrap_or_else(|error| panic!("{document:?}: {error}"));
        assert_eq!(
            sk_kernel::python_json::python_repr_value(&value),
            row["repr"].as_str().expect("repr"),
            "{document:?}"
        );
        assert_eq!(
            rendered(&value, None),
            codepoints(row["default"].as_str().expect("dumps")),
            "{document:?}"
        );
        for (indent, key) in [(0usize, "indent0"), (2, "indent2"), (4, "indent4")] {
            assert_eq!(
                rendered(&value, Some(indent)),
                codepoints(row[key].as_str().expect("indented dumps")),
                "{document:?} indent {indent}"
            );
        }
        assert_eq!(
            value
                .render_python_with(PythonJsonRenderOptions::default().sort_keys(true))
                .codepoints()
                .to_vec(),
            codepoints(row["sorted"].as_str().expect("sorted dumps")),
            "{document:?} sort_keys"
        );
        let sorted_indent = PythonJsonRenderOptions::new(Some(2)).sort_keys(true);
        assert_eq!(
            value
                .render_python_with(sorted_indent)
                .codepoints()
                .to_vec(),
            codepoints(row["sorted2"].as_str().expect("sorted dumps")),
            "{document:?} sort_keys indent"
        );
        assert_eq!(
            value.render_python(None, false).codepoints(),
            units(&row["utf8"]),
            "{document:?} ensure_ascii=False"
        );
    }
}

#[test]
fn source_render_options_cover_the_caller_dump_shapes() {
    // The two shapes a cutover caller needs, plus the indent/separator
    // combinations CPython resolves itself:
    //   json.dumps(as_dict(), sort_keys=True)
    //   json.dumps(value, default=str, separators=(',', ':'))
    let documents = [
        r#"{"b": 1, "a": 2}"#,
        r#"{"profile": {"role": "architect", "domains": ["a", "b"]}, "count": 2}"#,
        r#"{"z": [1, {"y": 2, "x": 3}], "a": []}"#,
        r#"{"k": "\u00e9\ud83d\ude00", "n": NaN}"#,
    ];
    let source = run_source(
        concat!(
            "documents = json.loads(sys.argv[1])\n",
            "report = []\n",
            "for document in documents:\n",
            "    value = json.loads(document)\n",
            "    report.append({\n",
            "        'sorted': json.dumps(value, sort_keys=True),\n",
            "        'compact': [ord(c) for c in json.dumps(value, separators=(',', ':'))],\n",
            "        'indent_compact': json.dumps(value, indent=2, separators=(',', ':')),\n",
            "        'indent_default': json.dumps(value, indent=2),\n",
            "    })\n",
            "projected = {'k': b'ab', 'z': 1}\n",
            "projected_wire = json.dumps(projected, default=str, separators=(',', ':'))\n",
            "assert projected_wire == '{\"k\":\"b\\'ab\\'\",\"z\":1}', projected_wire\n",
            "print(json.dumps({\n",
            "    'rows': report,\n",
            "    'projection': [ord(c) for c in projected_wire],\n",
            "    'projection_text': str(b\"ab\"),\n",
            "}))\n",
        ),
        &[&serde_json::to_string(&documents).expect("document list")],
    );
    for (document, row) in documents
        .iter()
        .zip(source["rows"].as_array().expect("rows"))
    {
        let value = parse(document).expect("document parses");
        assert_eq!(
            value
                .render_python_with(PythonJsonRenderOptions::default().sort_keys(true))
                .codepoints()
                .to_vec(),
            codepoints(row["sorted"].as_str().expect("sorted")),
            "{document:?}"
        );
        let compact = PythonJsonRenderOptions::default()
            .separators(Some(PythonJsonSeparators::COMPACT))
            .default_projection(PythonJsonDefault::PythonStr);
        assert_eq!(
            value.render_python_with(compact).codepoints(),
            units(&row["compact"]),
            "{document:?} compact"
        );
        let indent_compact =
            PythonJsonRenderOptions::new(Some(2)).separators(Some(PythonJsonSeparators::COMPACT));
        assert_eq!(
            value
                .render_python_with(indent_compact)
                .codepoints()
                .to_vec(),
            codepoints(row["indent_compact"].as_str().expect("indent compact")),
            "{document:?} indent+compact"
        );
        assert_eq!(
            value
                .render_python_with(PythonJsonRenderOptions::new(Some(2)))
                .codepoints()
                .to_vec(),
            codepoints(row["indent_default"].as_str().expect("indent default")),
            "{document:?} indent default"
        );
    }
    // `default=str` is an upstream projection: the caller stores the one `str()`
    // primitive's text as a JSON string and the codec writes exactly CPython's
    // `json.dumps(..., default=str, separators=(',',':'))`.
    let projection = PythonJsonValue::Object(vec![
        (
            "k".to_owned(),
            PythonJsonValue::scalar(source["projection_text"].as_str().expect("str(b'ab')")),
        ),
        ("z".to_owned(), PythonJsonValue::Integer("1".to_owned())),
    ]);
    let options = PythonJsonRenderOptions::default()
        .separators(Some(PythonJsonSeparators::COMPACT))
        .default_projection(PythonJsonDefault::PythonStr);
    assert_eq!(
        projection.render_python_with(options).codepoints(),
        units(&source["projection"])
    );
    assert_eq!(options.resolved_separators(), PythonJsonSeparators::COMPACT);
    assert_eq!(
        PythonJsonRenderOptions::new(Some(2)).resolved_separators(),
        PythonJsonSeparators::INDENTED
    );
    assert_eq!(
        PythonJsonRenderOptions::default().resolved_separators(),
        PythonJsonSeparators::DEFAULT
    );
}

#[test]
fn source_numbers_keep_python_int_and_float_semantics() {
    let tokens = [
        "0",
        "-0",
        "0.0",
        "-0.0",
        "1",
        "-1",
        "1.5",
        "1e2",
        "1E+2",
        "1e-2",
        "1e400",
        "-1e400",
        "1e-400",
        "-1e-400",
        "1e15",
        "1e16",
        "1234.5678",
        "0.0001",
        "0.00001",
        "18446744073709551616",
        "-18446744073709551617",
        "123456789012345678901234567890123456789012345678901234567890",
        "1.7976931348623157e308",
        "5e-324",
        "1.7976931348623159e308",
        "9007199254740993",
        "9007199254740993.0",
        "NaN",
        "Infinity",
        "-Infinity",
    ];
    let source = run_source(
        concat!(
            "tokens = json.loads(sys.argv[1])\n",
            "report = []\n",
            "for token in tokens:\n",
            "    value = json.loads(token)\n",
            "    report.append([type(value).__name__, repr(value), json.dumps(value)])\n",
            "assert report[0] == ['int', '0', '0'], report[0]\n",
            "assert report[1] == ['int', '0', '0'], report[1]\n",
            "assert report[10][1:] == ['inf', 'Infinity'], report[10]\n",
            "assert report[11][1:] == ['-inf', '-Infinity'], report[11]\n",
            "assert report[12][1:] == ['0.0', '0.0'], report[12]\n",
            "assert report[13][1:] == ['-0.0', '-0.0'], report[13]\n",
            "assert report[-3][1:] == ['nan', 'NaN'], report[-3]\n",
            "assert report[-2][1:] == ['inf', 'Infinity'], report[-2]\n",
            "assert report[-1][1:] == ['-inf', '-Infinity'], report[-1]\n",
            "print(json.dumps(report))\n",
        ),
        &[&serde_json::to_string(&tokens).expect("token list")],
    );
    for (token, row) in tokens.iter().zip(source.as_array().expect("rows")) {
        let value = parse(token).unwrap_or_else(|error| panic!("{token}: {error}"));
        assert_eq!(
            sk_kernel::python_json::python_repr_value(&value),
            row[1].as_str().expect("repr"),
            "{token}"
        );
        assert_eq!(
            rendered(&value, None),
            codepoints(row[2].as_str().expect("dumps")),
            "{token}"
        );
        match row[0].as_str().expect("type") {
            "int" => assert!(matches!(value, PythonJsonValue::Integer(_)), "{token}"),
            "float" => assert!(matches!(value, PythonJsonValue::Float(_)), "{token}"),
            other => panic!("unexpected Source type {other}"),
        }
    }
    // The integer digit ceiling is the interpreter's startup setting.
    let digits = "1".repeat(4300);
    assert!(parse(&digits).is_ok());
    assert!(parse(&format!("-{digits}")).is_ok());
    let error = parse(&format!("{digits}1")).expect_err("4301 digits");
    assert_eq!(error.kind(), PythonJsonErrorKind::IntegerDigitLimit);
    assert!(
        parse_str_with_integer_digits(&format!("{digits}1"), 0).is_ok(),
        "startup limit 0 means unlimited"
    );
}

#[test]
fn source_preserves_every_code_point_as_value_and_object_key() {
    // Values: all 1,114,112 code points in one escaped document. Escaping each
    // code point as `\uXXXX` (surrogate pairs above the BMP) makes the wire pure
    // ASCII, so the comparison itself cannot be confounded by a UTF-8 phase.
    let source = run_source(
        concat!(
            "def escape(cp):\n",
            "    if 0xD800 <= cp <= 0xDFFF:\n",
            "        return '\\\\u%04x' % cp\n",
            "    if cp >= 0x10000:\n",
            "        adjusted = cp - 0x10000\n",
            "        return '\\\\u%04x\\\\u%04x' % (0xD800 + (adjusted >> 10), 0xDC00 + (adjusted & 0x3FF))\n",
            "    return '\\\\u%04x' % cp\n",
            "codes = [cp for cp in range(0x110000) if not (0xD800 <= cp <= 0xDFFF)]\n",
            "surface = ''.join(chr(cp) for cp in codes)\n",
            "wire = json.dumps(surface)\n",
            "assert json.loads(wire) == surface\n",
            "assert all(ord(c) == cp for cp, c in zip(codes, surface))\n",
            "surrogate_wire = '[' + ', '.join('\"%s\"' % escape(cp) for cp in range(0xD800, 0xE000)) + ']'\n",
            "surrogates = json.loads(surrogate_wire)\n",
            "assert [ord(item) for item in surrogates] == list(range(0xD800, 0xE000))\n",
            "assert json.dumps(surrogates) == surrogate_wire\n",
            "keys = list(range(0xD800, 0xE000))\n",
            "key_wire = '{' + ', '.join('\"%s\": %d' % (escape(key), position) for position, key in enumerate(keys)) + '}'\n",
            "keyed = json.loads(key_wire)\n",
            "assert [ord(key) for key in keyed] == keys\n",
            "assert json.dumps(keyed) == key_wire\n",
            "raw_keys = [ord(c) for c in json.dumps(keyed, ensure_ascii=False)]\n",
            "single = json.loads('\"\\\\ud800\\\\u0000\"')\n",
            "assert [ord(c) for c in single] == [0xD800, 0]\n",
            "print(json.dumps({\n",
            "    'length': len(wire),\n",
            "    'sha256': hashlib.sha256(wire.encode('ascii')).hexdigest(),\n",
            "    'key_wire': key_wire,\n",
            "    'raw_keys': raw_keys,\n",
            "    'surrogate_wire': surrogate_wire,\n",
            "    'single_ascii': json.dumps(single),\n",
            "}))\n",
        ),
        &[],
    );

    // Values: all 1,112,064 Unicode-scalar code points (the 2048 surrogate
    // units are covered separately as values and as object keys). The Source
    // renders the raw surface text with `json.dumps`; the Native renderer must
    // reproduce those exact bytes, and the Native reader must decode them back.
    let mut wire = String::new();
    let scalars: Vec<u32> = (0..0x11_0000u32)
        .filter(|code| !(0xd800..=0xdfff).contains(code))
        .collect();
    let surface = PythonJsonValue::String(
        sk_kernel::PythonString::from_codepoints(&scalars).expect("scalar code points"),
    );
    let rendered_wire = surface
        .render_python(None, true)
        .to_scalar_string()
        .expect("ascii escapes are scalar");
    wire.push_str(&rendered_wire);
    assert_eq!(
        wire.len(),
        source["length"].as_u64().expect("length") as usize,
        "Source-measured ascii wire length"
    );
    assert_eq!(
        format!("{:x}", sha2::Sha256::digest(wire.as_bytes())),
        source["sha256"].as_str().expect("sha"),
        "the ascii wire must have the Source-measured bytes"
    );
    let value = parse(&wire).expect("every scalar value decodes");
    let PythonJsonValue::String(text) = &value else {
        panic!("string root");
    };
    assert_eq!(text.codepoints(), scalars, "every scalar value is exact");
    assert_eq!(rendered(&value, None), codepoints(&wire));

    // The published frozen ascii-True wire for all 1,114,112 code points in one
    // string (surrogate units included, escaped individually). The receipt is
    // `/tmp/sk-g-handoff-counters/source-first-current-json-positive.gNkYZ1.log`
    // (12975637 bytes, sha256 7a12e7…); this re-measures it Source-first rather
    // than copying the figure, and binds the canonical encoder to it.
    let frozen = run_source(
        concat!(
            "raw = json.dumps(''.join(chr(c) for c in range(0x110000))).encode('ascii')\n",
            "assert b'\\\\ud800' in raw and b'\\\\udfff' in raw\n",
            "assert raw.startswith(b'\"\\\\u0000')\n",
            "print(json.dumps({'length': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}))\n",
        ),
        &[],
    );
    assert_eq!(frozen["length"].as_u64(), Some(12_975_637));
    assert_eq!(
        frozen["sha256"].as_str(),
        Some("7a12e7fbcb38887f94979b257537384a7581f39b402037f3a597a8f7e9c7d67c")
    );
    let all_units: Vec<u32> = (0..0x11_0000u32).collect();
    let frozen_body = PythonJsonValue::String(
        sk_kernel::PythonString::from_codepoints(&all_units).expect("all code units"),
    )
    .render_python(None, true)
    .to_scalar_string()
    .expect("ascii escapes are scalar");
    assert_eq!(frozen_body.len(), 12_975_637);
    assert_eq!(
        format!("{:x}", sha2::Sha256::digest(frozen_body.as_bytes())),
        "7a12e7fbcb38887f94979b257537384a7581f39b402037f3a597a8f7e9c7d67c",
        "the canonical encoder must reproduce the frozen ascii-True wire"
    );

    // All 2048 surrogate units as values: an array keeps them unpaired, exactly
    // like the reference reader, and the exact renderer writes them back.
    let surrogate_wire = source["surrogate_wire"].as_str().expect("surrogate wire");
    let surrogate_value = parse(surrogate_wire).expect("all units decode");
    let PythonJsonValue::Array(surrogates) = &surrogate_value else {
        panic!("array root");
    };
    assert_eq!(surrogates.len(), 2048);
    for (position, item) in surrogates.iter().enumerate() {
        assert_eq!(
            item,
            &PythonJsonValue::String(
                sk_kernel::PythonString::from_codepoints(&[0xd800 + position as u32])
                    .expect("surrogate unit")
            ),
            "surrogate value {position}"
        );
    }
    assert_eq!(
        surrogate_value.render_python(None, true).codepoints(),
        codepoints(surrogate_wire)
    );

    // Surrogate units are reachable through the scalar view only as escapes; the
    // exact phase is what carries them.
    assert!(surrogate_value.render_with(None, false).is_none());
    assert!(surrogate_value.to_serde().is_none());
    assert!(value.render_with(None, false).is_some(), "scalars encode");

    let key_wire = source["key_wire"].as_str().expect("key wire");
    let keyed = parse(key_wire).expect("every surrogate key decodes");
    let PythonJsonValue::ExactObject(object) = &keyed else {
        panic!("object root");
    };
    assert_eq!(object.len(), 2048);
    for (position, key) in object.keys().enumerate() {
        assert_eq!(
            key.codepoints(),
            &[0xd800 + position as u32],
            "key {position}"
        );
    }
    assert_eq!(rendered(&keyed, None), codepoints(key_wire));
    assert_eq!(
        keyed.render_python(None, false).codepoints(),
        units(&source["raw_keys"])
    );
    assert!(keyed.render_with(None, false).is_none());
    assert!(keyed.to_serde().is_none());
    // NUL stays literal-vs-escaped exact: `\u0000` decodes, a raw NUL does not.
    assert_eq!(
        codepoints(source["single_ascii"].as_str().expect("single")),
        codepoints(r#""\ud800\u0000""#)
    );
    assert!(parse("\"\u{0}\"").is_err());
}

#[test]
fn source_recursion_boundaries_stay_inside_the_operational_budget() {
    let source = run_source(
        concat!(
            "def accepts(depth):\n",
            "    try:\n",
            "        json.loads('[' * depth + ']' * depth)\n",
            "    except RecursionError:\n",
            "        return False\n",
            "    return True\n",
            "def one(depth):\n",
            "    return accepts(depth)\n",
            "def two(depth):\n",
            "    return one(depth)\n",
            "def three(depth):\n",
            "    return two(depth)\n",
            "contexts = {'top': accepts, 'one': one, 'two': two, 'three': three}\n",
            "report = {}\n",
            "assert sys.getrecursionlimit() == 1000, sys.getrecursionlimit()\n",
            "for name, probe in contexts.items():\n",
            "    assert probe(989), name\n",
            "    assert not probe(1000), name\n",
            "    report[name] = [probe(989), probe(994)]\n",
            "print(json.dumps(report))\n",
        ),
        &[],
    );
    for name in ["top", "one", "two", "three"] {
        assert_eq!(
            source[name].as_array().expect("context row")[0],
            true,
            "{name} must accept 989 like Source"
        );
    }
    let deep = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    for depth in [989usize, 994] {
        assert!(
            parse(&deep(depth)).is_ok(),
            "Native must accept every depth Source accepts in these contexts ({depth})"
        );
    }
    assert!(parse(&deep(1000)).is_ok());
    // The 1001st frame is the documented operational guard, not a JSON rule.
    let error = parse(&deep(1001)).expect_err("operational budget");
    assert_eq!(error.kind(), PythonJsonErrorKind::RecursionBudget);
    let deep_value = parse(&deep(994)).expect("deep value");
    assert_eq!(rendered(&deep_value, None), codepoints(&deep(994)));
}
