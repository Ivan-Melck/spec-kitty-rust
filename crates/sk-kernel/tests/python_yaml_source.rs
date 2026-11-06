//! Source-first proof of the shared Python-safe YAML reader, both dialects.
//!
//! Protocol (the wave's paired-execution discipline):
//!
//! 1. The parent writes one fixture per case into a fresh temporary root, runs the
//!    **pinned Source** (`verify_oracle.py`-verified CPython 3.11.15) in a fully
//!    asserted temporary CWD/HOME/all-XDG/TMPDIR environment with
//!    `PYTHONHASHSEED=0`, and keeps its decoded documents / error classes.
//! 2. It then re-executes this test binary as a **separate process** with
//!    `env_clear()` and the same asserted roots; that Native leg decodes the same
//!    ORIGINAL bytes through the shared Kernel reader and compares.
//!
//! What is compared, per case and per dialect:
//!
//! * whether the loader returned a value (`ok`) — the acceptance direction;
//! * the decoded Python document, node for node, including exact code points,
//!   arbitrary-precision integers, `bytes`, `date`/`datetime` `str()` text, set
//!   members in **CPython's iteration order** (both sides seed 0), lists, dicts
//!   and their order;
//! * for a refused document, the exception **class family** (`ConstructorError`
//!   vs `DuplicateKeyError`/`YAMLError`/scanner vs `KeyError`/`ValueError`/
//!   `TypeError`, which decide skip-vs-abort at Source's catch boundary) and, for
//!   the bare Python exceptions, the exact message.
//!
//! Not compared, and declared instead (see `OPEN_CASES`): the libyaml caret mark
//! block and recursive aliases (both loaders build cycles; the semantic reader
//! cannot represent one). A declared divergence assertion is NOT a parity green.
//! L5 closed ruamel's ordinary list-key conversion; its old comment claiming
//! that case remained unsupported was stale, not an additional passing proof.

#[path = "support/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_kernel::PythonString;
use sk_kernel::python_value::PythonValue;
use sk_kernel::python_yaml::{Dialect, ErrorClass, YamlError, decode_bytes, decode_text};

/// One authored case: label, dialect, YAML text, and whether its Source success
/// is a *declared open* case this reader does not reproduce.
struct Case {
    label: &'static str,
    dialect: Dialect,
    yaml: &'static str,
    open_case: Option<&'static str>,
}

const fn closed(label: &'static str, dialect: Dialect, yaml: &'static str) -> Case {
    Case {
        label,
        dialect,
        yaml,
        open_case: None,
    }
}

const fn open(label: &'static str, dialect: Dialect, yaml: &'static str, id: &'static str) -> Case {
    Case {
        label,
        dialect,
        yaml,
        open_case: Some(id),
    }
}

/// The dialect policy matrix, one row per probed Source fact.
fn corpus() -> Vec<Case> {
    use Dialect::{PyYamlSafe11 as Py, RuamelSafe12 as Ru};
    vec![
        closed("bool-yes", Py, "name: yes\n"),
        closed("bool-no", Ru, "name: no\n"),
        closed("bool-on", Py, "name: on\n"),
        closed("bool-quoted", Py, "name: 'yes'\n"),
        closed("octal-leading-zero", Py, "v: 017\n"),
        closed("octal-leading-zero-12", Ru, "v: 017\n"),
        closed("octal-0o", Py, "v: 0o17\n"),
        closed("octal-0o-12", Ru, "v: 0o17\n"),
        closed("binary-0b", Py, "v: 0b1010\n"),
        closed("hex", Py, "v: 0x1f\n"),
        closed("underscores", Py, "v: 1_0\n"),
        closed("sexagesimal-int", Py, "v: 1:30\n"),
        closed("sexagesimal-int-12", Ru, "v: 1:30\n"),
        closed("sexagesimal-int-3", Py, "v: 1:2:3\n"),
        closed("sexagesimal-float", Py, "v: 1:30.5\n"),
        closed("date", Py, "v: 2026-11-05\n"),
        closed("date-12", Ru, "v: 2026-11-05\n"),
        closed(
            "parent-date-set",
            Py,
            "v: !!set {2026-11-05: null, 2026-11-04: null, 2000-02-29: null, x: null}\n",
        ),
        closed(
            "parent-date-set-12",
            Ru,
            "v: !!set {2026-11-05: null, 2026-11-04: null, 2000-02-29: null, x: null}\n",
        ),
        closed(
            "parent-naive-set",
            Py,
            "v: !!set {2026-11-05T00:00:00: null, 2000-02-29T23:59:59.123456: null, 0001-01-01T00:00:00: null}\n",
        ),
        closed(
            "parent-naive-set-12",
            Ru,
            "v: !!set {2026-11-05T00:00:00: null, 2000-02-29T23:59:59.123456: null, 0001-01-01T00:00:00: null}\n",
        ),
        closed(
            "parent-aware-set",
            Py,
            "v: !!set {2026-11-05T00:00:00+00:00: null, 2000-02-29T23:59:59.123456+14:00: null, 0001-01-01T00:00:00+23:59: null}\n",
        ),
        closed(
            "parent-aware-set-12",
            Ru,
            "v: !!set {2026-11-05T00:00:00+00:00: null, 2000-02-29T23:59:59.123456+14:00: null, 0001-01-01T00:00:00+23:59: null}\n",
        ),
        closed("datetime-z", Py, "v: 2026-11-05T01:02:03Z\n"),
        closed("datetime-space", Py, "v: 2001-12-15 2:59:43.1\n"),
        closed("datetime-offset", Py, "v: 2001-12-15T02:59:43.10-05:00\n"),
        closed("datetime-micro", Py, "v: 2001-12-15T02:59:43.123456\n"),
        closed("date-invalid-month", Py, "v: 2026-13-05\n"),
        closed("date-invalid-day", Py, "v: 2026-02-30\n"),
        closed("float-dot", Py, "v: 1.0\n"),
        closed("float-trailing-dot", Py, "v: 1.\n"),
        closed("float-leading-dot", Py, "v: .5\n"),
        closed("float-exp-11", Py, "v: 1e5\n"),
        closed("float-exp-12", Ru, "v: 1e5\n"),
        closed("float-exp-plus", Py, "v: 1.0e+5\n"),
        closed("float-inf", Py, "v: .inf\n"),
        closed("float-neg-inf", Py, "v: -.Inf\n"),
        closed("float-nan", Py, "v: .NaN\n"),
        closed("float-plus", Py, "v: +1\n"),
        closed("null-tilde", Py, "v: ~\n"),
        closed("null-word", Py, "v: NULL\n"),
        closed("null-empty", Py, "v: \n"),
        closed("int-plain", Py, "v: 42\n"),
        closed("int-negzero", Py, "v: -0\n"),
        closed("int-big", Py, "v: 123456789012345678901234567890\n"),
        closed("int-u64", Py, "v: 18446744073709551615\n"),
        closed("binary-tag", Py, "v: !!binary aGk=\n"),
        closed("binary-tag-bad", Py, "v: !!binary '!!!'\n"),
        closed("binary-nonascii", Py, "v: !!binary \"\\u00e9\"\n"),
        closed("set-tag", Py, "v: !!set {a: null, b: null}\n"),
        closed("set-abc", Py, "v: !!set {a: null, b: null, c: null}\n"),
        closed(
            "set-xyzw",
            Py,
            "v: !!set {x: null, y: null, z: null, w: null}\n",
        ),
        closed(
            "set-int-keys",
            Py,
            "v: !!set {1: null, 2: null, 10: null}\n",
        ),
        closed("set-unhashable-list", Py, "v: !!set {? [a], b: null}\n"),
        // L5: ruamel's `construct_mapping` converts a **list key** with
        // `tuple(key)`, so the same documents construct a real tuple instead.
        closed("set-list-key-12", Ru, "v: !!set {? [a], b: null}\n"),
        closed("set-list-key-two-12", Ru, "v: !!set {? [a, c], b: null}\n"),
        // A nested list inside the converted tuple stays a list: hashing it is
        // the bare `TypeError` the reference does not catch.
        closed(
            "set-nested-list-key-12",
            Ru,
            "v: !!set {? [[a]], b: null}\n",
        ),
        closed("map-list-key-12", Ru, "? [a, b]\n: 1\n"),
        closed("map-empty-list-key-12", Ru, "? []\n: 1\n"),
        closed("map-list-of-dict-12", Ru, "? [a, {b: c}]\n: 1\n"),
        // A `dict` key is unhashable in both loaders (ruamel's ConstructorError).
        closed("map-dict-key", Py, "? {a: b}\n: 1\n"),
        closed("map-dict-key-12", Ru, "? {a: b}\n: 1\n"),
        // `!!omap` never goes through `construct_mapping`, so ruamel's list key
        // is NOT converted: `assert key not in omap` hashes it instead.
        closed("omap-list-key", Py, "!!omap [{? [a]: 1}]\n"),
        closed("omap-list-key-12", Ru, "!!omap [{? [a]: 1}]\n"),
        closed("pairs-list-key", Py, "!!pairs [{? [a]: 1}]\n"),
        closed("pairs-list-key-12", Ru, "!!pairs [{? [a]: 1}]\n"),
        closed("parent-pairs-as-key", Py, "? !!pairs [{a: b}]\n: 1\n"),
        closed("parent-pairs-as-key-12", Ru, "? !!pairs [{a: b}]\n: 1\n"),
        closed(
            "parent-pairs-set-key-12",
            Ru,
            "!!set {? !!pairs [{a: b}]: null, x: null}\n",
        ),
        closed(
            "parent-pairs-unhashable-value-12",
            Ru,
            "? !!pairs [{a: [b]}]\n: 1\n",
        ),
        // PyYAML's `.nan` is one `nan_value` singleton, so two `.nan` keys are
        // the same object and the dict keeps one entry.
        closed("dup-nan", Py, "v: {.nan: a, .nan: b}\n"),
        // `!!float nan` (the bare spelling) constructs a FRESH float, so two such
        // keys are two distinct Python keys in both dialects (probed).
        closed("nan-fresh-2", Py, "v: {!!float nan: a, !!float nan: b}\n"),
        closed(
            "nan-fresh-2-12",
            Ru,
            "v: {!!float nan: a, !!float nan: b}\n",
        ),
        // The singleton and a fresh object are different keys too.
        closed("nan-mixed", Py, "v: {.nan: a, !!float nan: b}\n"),
        closed("nan-mixed-12", Ru, "v: {.nan: a, !!float nan: b}\n"),
        // `date` and `datetime` objects are never equal, and two aware datetimes
        // at the same instant are equal (PyYAML merges, ruamel refuses).
        closed(
            "dup-date-datetime",
            Py,
            "v: {2026-11-05: a, 2026-11-05T00:00:00: b}\n",
        ),
        closed(
            "dup-date-datetime-12",
            Ru,
            "v: {2026-11-05: a, 2026-11-05T00:00:00: b}\n",
        ),
        closed(
            "dup-tz-same-instant",
            Py,
            "v: {2026-11-05T00:00:00+00:00: a, 2026-11-05T01:00:00+01:00: b}\n",
        ),
        closed(
            "dup-tz-same-instant-12",
            Ru,
            "v: {2026-11-05T00:00:00+00:00: a, 2026-11-05T01:00:00+01:00: b}\n",
        ),
        // `b'a'` and `'a'` share a hash but are different keys in both loaders.
        closed("dup-bytes-str", Py, "v: {!!binary YQ==: a, a: b}\n"),
        closed("dup-bytes-str-12", Ru, "v: {!!binary YQ==: a, a: b}\n"),
        closed("dup-int-float", Py, "v: {1: a, 1.0: b}\n"),
        closed("set-dup-keys", Py, "v: !!set {a: null, a: null}\n"),
        closed("omap-tag", Py, "v: !!omap [{a: b}]\n"),
        closed("omap-tag-12", Ru, "v: !!omap [{a: b}]\n"),
        closed("pairs-tag", Py, "v: !!pairs [{a: b}]\n"),
        closed("str-tag", Py, "v: !!str 7\n"),
        closed("bool-tag-bad", Py, "v: !!bool notabool\n"),
        closed("bool-tag-y", Py, "v: !!bool y\n"),
        closed("bool-tag-y-12", Ru, "v: !!bool y\n"),
        closed("int-tag-bad", Py, "v: !!int abc\n"),
        closed("int-tag-11", Py, "v: !!int 017\n"),
        closed("int-tag-11-12", Ru, "v: !!int 017\n"),
        closed("int-tag-sexagesimal-12", Ru, "v: !!int 1:30\n"),
        closed("timestamp-tag-bad", Py, "v: !!timestamp 2020-13-01\n"),
        closed("null-tag-empty", Py, "v: !!null ''\n"),
        closed("unknown-tag", Py, "v: !custom hello\n"),
        closed("dup-block", Py, "a: 1\nb: 2\na: 3\n"),
        closed("dup-block-12", Ru, "a: 1\nb: 2\na: 3\n"),
        closed("dup-flow", Py, "v: {a: 1, a: 2}\n"),
        closed("dup-int-bool", Py, "v: {1: a, true: b}\n"),
        closed("dup-int-bool-12", Ru, "v: {1: a, true: b}\n"),
        closed(
            "merge-key",
            Py,
            "base: &b {x: 1, y: 2}\nv:\n  <<: *b\n  y: 3\n",
        ),
        closed(
            "merge-key-12",
            Ru,
            "base: &b {x: 1, y: 2}\nv:\n  <<: *b\n  y: 3\n",
        ),
        closed("merge-quoted", Py, "v:\n  '<<': 1\n"),
        closed(
            "merge-seq",
            Py,
            "a: &a {x: 1}\nb: &b {y: 2}\nv: {<<: [*a, *b]}\n",
        ),
        closed(
            "merge-dup-keys",
            Py,
            "a: &a {x: 1}\nb: &b {y: 2}\nv:\n  <<: *a\n  <<: *b\n",
        ),
        closed(
            "merge-dup-keys-12",
            Ru,
            "a: &a {x: 1}\nb: &b {y: 2}\nv:\n  <<: *a\n  <<: *b\n",
        ),
        closed("merge-plain-value", Py, "v: <<\n"),
        closed("value-plain", Py, "v: =\n"),
        closed("alias-shared", Py, "a: &x [1, 2]\nb: *x\n"),
        // Source builds a genuine cycle in BOTH dialects; this reader still
        // refuses it, so the real counterexample is carried as a declared open
        // case instead of being silently absent from the corpus.
        open("recursive-alias", Py, "a: &x [*x]\n", "recursive-alias"),
        open("recursive-alias-12", Ru, "a: &x [*x]\n", "recursive-alias"),
        closed("empty-document", Py, ""),
        closed("comment-only", Py, "# nothing\n"),
        closed("root-list", Py, "- 1\n- 2\n"),
        closed("root-scalar-timestamp", Py, "2026-11-05\n"),
        closed("tab-indent", Py, "v:\n\t- 1\n"),
        closed("bom", Py, "\u{feff}a: 1\n"),
        closed("crlf", Py, "a: 1\r\nb: 2\r\n"),
        closed("nul-in-quoted", Py, "v: \"a\\u0000b\"\n"),
        closed("surrogate-in-quoted", Py, "v: \"a\\ud800b\"\n"),
        closed("marker-shaped", Py, "v: \"\u{fdd0}a\u{fdef}\"\n"),
        closed("nested-flow-implicit-null", Py, "v: {x, y}\n"),
        closed("block-scalar", Py, "v: |\n  one\n  two\n"),
        closed("folded-scalar", Py, "v: >\n  one\n  two\n"),
        closed("yes-as-key", Py, "yes: 1\n"),
        closed("octal-as-key", Py, "017: 1\n"),
        closed("date-as-key", Py, "2026-11-05: 1\n"),
        closed("multiline-flow-seq", Py, "v: [1,\n  2]\n"),
        closed("quoted-key-typed", Py, "'017': 1\n"),
        closed("deep-nesting", Py, "a: {b: {c: [{d: 1}]}}\n"),
    ]
}

/// The environment roots every child must have, all inside the temp root.
fn roots() -> [(&'static str, &'static str); 11] {
    [
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
    ]
}

fn child(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        // A `!!set`'s iteration order is a property of the reference process; both
        // children are pinned to the same seed so it has one observable answer.
        .env("PYTHONHASHSEED", "0");
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
    command
}

#[derive(Debug, PartialEq)]
struct InputFileState {
    bytes: Vec<u8>,
    modified: std::time::SystemTime,
    readonly: bool,
    #[cfg(unix)]
    physical: (u64, u64, u32),
}

fn input_states(directories: &[PathBuf]) -> Vec<InputFileState> {
    directories
        .iter()
        .map(|directory| {
            let path = directory.join("case.yaml");
            let metadata = fs::metadata(&path).unwrap();
            InputFileState {
                bytes: fs::read(&path).unwrap(),
                modified: metadata.modified().unwrap(),
                readonly: metadata.permissions().readonly(),
                #[cfg(unix)]
                physical: {
                    use std::os::unix::fs::MetadataExt;
                    (metadata.dev(), metadata.ino(), metadata.mode())
                },
            }
        })
        .collect()
}

fn dialect_name(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::PyYamlSafe11 => "pyyaml",
        Dialect::RuamelSafe12 => "ruamel",
    }
}

fn write_cases(root: &Path) -> Vec<PathBuf> {
    let cases_dir = root.join("cases");
    fs::create_dir_all(&cases_dir).unwrap();
    let mut written = Vec::new();
    let mut index = Vec::new();
    for (position, case) in corpus().into_iter().enumerate() {
        let relative = format!("cases/{position:02}-{}", case.label);
        let directory = root.join(&relative);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("case.yaml"), case.yaml).unwrap();
        index.push(serde_json::json!({
            "label": case.label,
            "dialect": dialect_name(case.dialect),
            "dir": relative,
        }));
        written.push(directory);
    }
    fs::write(
        cases_dir.join("index.json"),
        serde_json::to_vec_pretty(&serde_json::Value::Array(index)).unwrap(),
    )
    .unwrap();
    written
}

/// Decode a receipt document (the oracle's exact transport) into the value domain.
fn receipt_document(value: &serde_json::Value) -> Option<PythonValue> {
    let tag = value.get("t")?.as_str()?;
    Some(match tag {
        "none" => PythonValue::Null,
        "bool" => PythonValue::Bool(value.get("v")?.as_bool()?),
        "int" => PythonValue::Int(value.get("v")?.as_str()?.to_owned()),
        "float" => PythonValue::Float(parse_float(value.get("v")?.as_str()?)),
        "str" => {
            let units: Vec<u32> = serde_json::from_value(value.get("v")?.clone()).ok()?;
            PythonValue::Str(PythonString::from_codepoints(&units).ok()?)
        }
        "bytes" => PythonValue::Bytes(serde_json::from_value(value.get("v")?.clone()).ok()?),
        "date" => PythonValue::Date(value.get("v")?.as_str()?.to_owned()),
        "datetime" => PythonValue::DateTime(value.get("v")?.as_str()?.to_owned()),
        "set" => PythonValue::Set(
            value
                .get("v")?
                .as_array()?
                .iter()
                .map(receipt_document)
                .collect::<Option<Vec<_>>>()?,
        ),
        "seq" => PythonValue::Seq(
            value
                .get("v")?
                .as_array()?
                .iter()
                .map(receipt_document)
                .collect::<Option<Vec<_>>>()?,
        ),
        "tuple" => PythonValue::Tuple(
            value
                .get("v")?
                .as_array()?
                .iter()
                .map(receipt_document)
                .collect::<Option<Vec<_>>>()?,
        ),
        "pairs" => PythonValue::Pairs(
            value
                .get("v")?
                .as_array()?
                .iter()
                .map(|pair| {
                    let pair = pair.as_array()?;
                    Some((receipt_document(&pair[0])?, receipt_document(&pair[1])?))
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        "map" => {
            let pairs = value
                .get("v")?
                .as_array()?
                .iter()
                .map(|pair| {
                    let pair = pair.as_array()?;
                    Some((receipt_document(&pair[0])?, receipt_document(&pair[1])?))
                })
                .collect::<Option<Vec<_>>>()?;
            match value.get("cls").and_then(|cls| cls.as_str()) {
                Some("ordereddict") => PythonValue::OrderedDict(pairs),
                _ => PythonValue::Map(pairs),
            }
        }
        other => panic!("unknown receipt transport tag {other}"),
    })
}

/// CPython's `repr(float)` text back to an `f64` (nan/inf included).
fn parse_float(text: &str) -> f64 {
    match text {
        "nan" => f64::NAN,
        "inf" => f64::INFINITY,
        "-inf" => f64::NEG_INFINITY,
        other => other.parse().unwrap_or_else(|error| {
            panic!("float receipt {other:?} is not a Python repr: {error}")
        }),
    }
}

/// Python value equality, with NaN equal to itself (CPython's float repr is
/// `nan`, and the two sides observed the same value).
fn python_equal(left: &PythonValue, right: &PythonValue) -> bool {
    match (left, right) {
        (PythonValue::Float(left), PythonValue::Float(right)) => {
            (left.is_nan() && right.is_nan()) || left == right
        }
        (PythonValue::Set(left), PythonValue::Set(right))
        | (PythonValue::Seq(left), PythonValue::Seq(right))
        | (PythonValue::Tuple(left), PythonValue::Tuple(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| python_equal(left, right))
        }
        (PythonValue::Map(left), PythonValue::Map(right))
        | (PythonValue::OrderedDict(left), PythonValue::OrderedDict(right))
        | (PythonValue::Pairs(left), PythonValue::Pairs(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|((lk, lv), (rk, rv))| python_equal(lk, rk) && python_equal(lv, rv))
        }
        _ => left == right,
    }
}

fn receipt_string(value: &serde_json::Value) -> PythonString {
    let units: Vec<u32> = serde_json::from_value(value.clone()).expect("code-point list");
    PythonString::from_codepoints(&units).expect("code points")
}

/// The Source exception class families a Kernel outcome may stand for.
fn accepted_source_classes(error: &YamlError) -> Vec<&'static str> {
    match error.class() {
        ErrorClass::Yaml => match error {
            YamlError::Construct(message) if message.starts_with("found duplicate key") => {
                vec!["DuplicateKeyError"]
            }
            YamlError::Construct(_) | YamlError::Merge(_) => {
                vec!["ConstructorError", "DuplicateKeyError"]
            }
            _ => vec![
                "ScannerError",
                "ParserError",
                "ComposerError",
                "ReaderError",
                "YAMLError",
            ],
        },
        ErrorClass::Runtime => vec![error.python_class()],
        other => panic!("unexpected outcome class for a Source comparison: {other:?}"),
    }
}

fn native_leg(root: &Path, repo: &Path) -> usize {
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in roots() {
        assert_eq!(
            std::env::var_os(key).unwrap(),
            root.join(name),
            "root {key} must be the asserted sandbox directory"
        );
    }
    assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
        repo.join("packs")
    );
    assert_eq!(std::env::var_os("PYTHONHASHSEED").unwrap(), "0");

    let receipts: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("source.json")).unwrap()).unwrap();
    let rows = receipts["rows"].as_array().unwrap();
    let cases = corpus();
    assert_eq!(rows.len(), cases.len(), "one receipt per case");

    let mut compared = 0usize;
    let mut refused = 0usize;
    let mut exact_runtime_messages = 0usize;
    let mut open_divergences = 0usize;
    for (position, (row, case)) in rows.iter().zip(cases.iter()).enumerate() {
        assert_eq!(row["label"].as_str().unwrap(), case.label);
        assert_eq!(row["dialect"].as_str().unwrap(), dialect_name(case.dialect));
        let bytes = fs::read(
            root.join("cases")
                .join(format!("{position:02}-{}", case.label))
                .join("case.yaml"),
        )
        .unwrap();
        let native = decode_bytes(&bytes, case.dialect);
        let source_ok = row["ok"].as_bool().unwrap();
        if let Some(id) = case.open_case {
            assert!(
                source_ok,
                "{case_label}: a declared open case must be one Source *accepts",
                case_label = case.label
            );
            assert!(
                native.is_err(),
                "{case_label}: the declared open case must stay an honest refusal",
                case_label = case.label
            );
            assert!(
                sk_kernel::python_yaml::OPEN_CASES
                    .iter()
                    .any(|open| open.id == id),
                "{case_label}: open case {id} must be declared in OPEN_CASES",
                case_label = case.label
            );
            open_divergences += 1;
            continue;
        }
        assert_eq!(
            native.is_ok(),
            source_ok,
            "{label} ({dialect}): the Source verdict must match (native: {:?})",
            native.as_ref().err().map(|error| error.to_string()),
            label = case.label,
            dialect = dialect_name(case.dialect)
        );
        if source_ok {
            let expected = receipt_document(&row["document"])
                .unwrap_or_else(|| panic!("{}: undecodable receipt", case.label));
            let decoded = native.expect("ok compared above").value;
            assert!(
                python_equal(&decoded, &expected),
                "{} ({}): the decoded Python value must match Source\n  native: {decoded:?}\n  source: {expected:?}",
                case.label,
                dialect_name(case.dialect)
            );
        } else {
            refused += 1;
            let error = native.expect_err("refusal compared above");
            let source_class = row["error_class"].as_str().unwrap();
            assert!(
                accepted_source_classes(&error).contains(&source_class),
                "{} ({}): Source raised {source_class}, native outcome is {error:?}",
                case.label,
                dialect_name(case.dialect)
            );
            if let YamlError::Runtime { message, .. } = &error {
                // A bare Python exception's message is pure Python (no libyaml
                // mark), so it is compared exactly.
                exact_runtime_messages += 1;
                assert_eq!(
                    receipt_string(&row["error"]),
                    PythonString::from_scalars(message),
                    "{} ({}): the Python exception message must match Source",
                    case.label,
                    dialect_name(case.dialect)
                );
            }
        }
        compared += 1;
    }
    assert!(compared >= 90, "compared {compared} cases");
    println!(
        "{compared} YAML cases compared against actual Source ({refused} refusals, \
         {exact_runtime_messages} with an exact Python exception message, \
         {open_divergences} declared open divergences)"
    );
    compared
}

#[test]
fn both_source_yaml_dialects_match_actual_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();

    if let Some(root) = std::env::var_os("SK_L4_PYTHON_YAML_NATIVE") {
        let root = PathBuf::from(root);
        let compared = native_leg(&root, &repo);
        assert!(compared > 0);
        return;
    }

    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let inputs = write_cases(&root);
    let before = input_states(&inputs);
    eprintln!("python_yaml evidence: {}", root.display());

    let source = child(&root, &repo, oracle::python())
        .arg("-c")
        .arg(include_str!("oracle/python_yaml.py"))
        .arg(&root)
        .output()
        .unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    fs::write(
        root.join("source.status"),
        format!("{:?}\n", source.status.code()),
    )
    .unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    fs::write(root.join("source.json"), &source.stdout).unwrap();
    assert_eq!(
        input_states(&inputs),
        before,
        "Source must not mutate YAML inputs"
    );

    let native = child(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "both_source_yaml_dialects_match_actual_source",
            "--nocapture",
        ])
        .env("SK_L4_PYTHON_YAML_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    fs::write(
        root.join("native.status"),
        format!("{:?}\n", native.status.code()),
    )
    .unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(
        input_states(&inputs),
        before,
        "Native must not mutate YAML inputs"
    );
}

#[test]
fn recursive_aliases_stay_a_declared_open_case() {
    // PyYAML builds a genuinely cyclic object here; the semantic reader cannot
    // represent a cycle, so the divergence is asserted instead of hidden.
    let error = decode_text("a: &x [*x]\n", Dialect::PyYamlSafe11)
        .expect_err("a cyclic alias is an open case, not a silent success");
    assert_eq!(error.class(), ErrorClass::Yaml);
}

#[test]
fn the_two_dialects_are_not_interchangeable() {
    // The load-bearing distinction: same bytes, different Python values.
    let pyyaml = decode_bytes(b"v: yes\ns: 017\ne: 1e5\n", Dialect::PyYamlSafe11)
        .unwrap()
        .value;
    let ruamel = decode_bytes(b"v: yes\ns: 017\ne: 1e5\n", Dialect::RuamelSafe12)
        .unwrap()
        .value;
    let field = |value: &PythonValue, key: &str| {
        value
            .mapping_entry(key)
            .cloned()
            .unwrap_or_else(|| panic!("missing {key}"))
    };
    assert_eq!(field(&pyyaml, "v"), PythonValue::Bool(true));
    assert_eq!(
        field(&ruamel, "v"),
        PythonValue::Str(PythonString::from_scalars("yes"))
    );
    assert_eq!(field(&pyyaml, "s"), PythonValue::Int("15".to_owned()));
    assert_eq!(field(&ruamel, "s"), PythonValue::Int("17".to_owned()));
    assert_eq!(
        field(&pyyaml, "e"),
        PythonValue::Str(PythonString::from_scalars("1e5"))
    );
    assert_eq!(field(&ruamel, "e"), PythonValue::Float(100_000.0));
}

#[test]
fn open_cases_are_named_not_implied() {
    let mut ids: BTreeMap<&str, ()> = BTreeMap::new();
    for case in sk_kernel::python_yaml::OPEN_CASES {
        assert!(!case.detail.is_empty());
        ids.insert(case.id, ());
    }
    assert!(ids.contains_key("recursive-alias"));
    assert!(ids.contains_key("mark-prose"));
    // `ruamel-set-unhashable-list-key` is CLOSED: the reader now constructs the
    // real tuple and the Source-first suite compares that value (see the
    // `*-list-key-12` rows), so it must not survive as a declared divergence.
    assert!(!ids.contains_key("ruamel-set-unhashable-list-key"));
    assert!(ids.len() >= 4, "declared open cases: {}", ids.len());
}
