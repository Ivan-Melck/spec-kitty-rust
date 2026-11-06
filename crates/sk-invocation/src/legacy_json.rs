//! Legacy CPython `json.loads` compatibility adapter over the canonical codec.
//!
//! The executor's best-effort commit reads the **first line** of an Op record
//! with `path.read_text(encoding="utf-8").splitlines()[0]` and then
//! `json.loads` that line (`executor.py::_read_started_event`). That reader is
//! deliberately looser than the strict Op claim/model readers and the wire
//! writer:
//!
//! * `NaN`, `Infinity` and `-Infinity` are accepted as bare constants
//!   (case-sensitive; `+Infinity` and `nan` are not).
//! * Object members keep Python dict order; a duplicate key keeps its **first
//!   insertion position** and takes the **last** value, including nested maps.
//! * Integer tokens are exact (`-0` decodes to `0`); float tokens (with `.`,
//!   `e` or `E`) decode like CPython `float`, so overflow becomes `±inf` and
//!   underflow becomes signed zero.
//!
//! The parser, the value tree, the Python-truthiness walk and the value `repr`
//! walk now live in `sk-kernel` (`sk_kernel::python_json`). This module is the
//! Op-specific **adapter**, not a second implementation: it keeps the legacy
//! names, the object-only `Option` facade and the exact-op semantics
//! (`parse_object` still accepts only a top-level object, `None` otherwise),
//! while the canonical reader accepts any root value and reports typed Source
//! errors with positions.
//!
//! Genuine remaining gaps (not claimed as arbitrary-JSON equivalence): a used
//! *top-level* string keeps its surrogate code points and encoding the composed
//! message through the application-owned filesystem encoder is the caller's one
//! late step, so nothing here replaces or drops a code unit. Integer conversion
//! follows the reference interpreter's default 4300-digit ceiling (or its
//! existing `PYTHONINTMAXSTRDIGITS` startup setting, including zero for
//! unlimited). The canonical reader's native operational budget of 1000 value
//! frames (the reference's default recursion budget) prevents process stack
//! loss on source-refused nesting; measured Source acceptance in the released
//! caller contexts is 989–994, i.e. below that budget. This is NOT a fixed
//! JSON-format limit or exact emulation of the source's caller-dependent
//! acceptance ceiling. Customized interpreter recursion settings, runtime Python
//! integer-limit mutations and a bytes (`json.loads(bytes)`) document are
//! caller-specific phases outside this adapter.

// The canonical Python-string authority lives in `sk-kernel`; re-export the
// EXACT same type and error (not a wrapper or a second nominal type).
pub use sk_kernel::{InvalidCodepoint, PythonString};

// The canonical JSON model and value-text walkers, under the legacy names the
// Op reader/callers already use. These are aliases/re-exports of one model.
pub(crate) use sk_kernel::python_json::{is_falsy, python_repr_value, source_integer_digit_limit};

/// One decoded `json.loads` value (the canonical tree).
pub(crate) type LegacyValue = sk_kernel::python_json::PythonJsonValue;

/// A Python-dict-ordered JSON object with exact code-point keys.
pub(crate) type LegacyObject = sk_kernel::python_json::PythonJsonDict;

/// Python `str(value)` before UTF-8 encoding.
///
/// A top-level string is verbatim (and may still contain surrogate units); any
/// other value is its `repr`, whose escapes make the text valid UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PythonText {
    /// `str()` of a string value: the original code points.
    Verbatim(PythonString),
    /// `repr()` of a non-string value, or the `"unknown"` default.
    Rendered(String),
}

impl PythonText {
    /// Append this field's exact code points to a Python-string composition.
    ///
    /// A verbatim string keeps its code units (including lone surrogates); a
    /// rendered value appends its canonical `repr` (non-printables escaped,
    /// printable scalars verbatim). This is the one place Op message
    /// composition appends a field, so no second formatter or code point
    /// table exists.
    pub(crate) fn append_to(&self, out: &mut PythonString) {
        match self {
            Self::Verbatim(text) => out.append(text),
            Self::Rendered(text) => out.push_scalars(text),
        }
    }

    /// Encode for a UTF-8-only consumer, or name the first surrogate unit.
    #[cfg(test)]
    pub(crate) fn to_utf8(&self) -> Result<String, u32> {
        match self {
            Self::Verbatim(text) => units_to_utf8(text.codepoints()),
            Self::Rendered(text) => Ok(text.clone()),
        }
    }
}

/// Test-only strict UTF-8 projection; the Op path uses the filesystem encoder.
#[cfg(test)]
fn units_to_utf8(units: &[u32]) -> Result<String, u32> {
    let mut out = String::with_capacity(units.len());
    for &code in units {
        match char::from_u32(code) {
            Some(character) => out.push(character),
            None => return Err(code),
        }
    }
    Ok(out)
}

/// Decode the first row as a Python `dict`, or `None` for the invalid cases.
///
/// The facade keeps its legacy contract: only a top-level object is accepted,
/// and every Source error (syntax, non-object root, a startup-invalid
/// `PYTHONINTMAXSTRDIGITS` value, the integer digit ceiling, the operational
/// frame budget) is the executor's `Invalid invocation record` failure.
pub(crate) fn parse_object(text: &str) -> Option<LegacyObject> {
    let setting = match std::env::var("PYTHONINTMAXSTRDIGITS") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => return None,
    };
    let integer_digits = source_integer_digit_limit(setting.as_deref()).ok()?;
    match sk_kernel::python_json::parse_str_with_integer_digits(text, integer_digits).ok()? {
        LegacyValue::ExactObject(object) => Some(object),
        _ => None,
    }
}

/// Python `str(value)` for one decoded value.
///
/// A top-level string is verbatim (code points preserved, including surrogates);
/// every other value uses its `repr`, because `str(list)`/`str(dict)` delegate
/// to element `repr` in CPython. The repr walk itself is the Kernel's single
/// value-text authority.
pub(crate) fn python_str(value: &LegacyValue) -> PythonText {
    match value {
        LegacyValue::String(text) => PythonText::Verbatim(text.clone()),
        other => PythonText::Rendered(python_repr_value(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str, key: &str) -> String {
        let object = parse_object(text).expect("valid object");
        let value = object.get(key).expect("field is present");
        assert!(!is_falsy(value), "the fixture field is truthy");
        python_text(value)
    }

    fn python_text(value: &LegacyValue) -> String {
        python_str(value)
            .to_utf8()
            .expect("fixture text is representable as UTF-8")
    }

    #[test]
    fn duplicate_keys_keep_the_first_position_and_the_last_value() {
        assert_eq!(
            field(r#"{"p": {"a": 1, "b": 2, "a": 3}}"#, "p"),
            "{'a': 3, 'b': 2}"
        );
        assert_eq!(field(r#"{"p": 1, "q": 2, "p": 3}"#, "p"), "3");
    }

    #[test]
    fn nested_maps_and_arrays_keep_python_insertion_order() {
        assert_eq!(field(r#"{"p": {"b": 1, "a": 2}}"#, "p"), "{'b': 1, 'a': 2}");
        assert_eq!(
            field(
                r#"{"p": [1, NaN, {"z": 1, "y": 2}, true, null, [], {}]}"#,
                "p"
            ),
            "[1, nan, {'z': 1, 'y': 2}, True, None, [], {}]"
        );
    }

    #[test]
    fn nonfinite_constants_and_number_overflow_use_python_float_rendering() {
        assert_eq!(field(r#"{"p": NaN}"#, "p"), "nan");
        assert_eq!(field(r#"{"p": Infinity}"#, "p"), "inf");
        assert_eq!(field(r#"{"p": -Infinity}"#, "p"), "-inf");
        assert_eq!(field(r#"{"p": 1e400}"#, "p"), "inf");
        assert_eq!(field(r#"{"p": -1e400}"#, "p"), "-inf");
        assert_eq!(field(r#"{"p": 1e300}"#, "p"), "1e+300");
        assert!(!is_falsy(&LegacyValue::Float(f64::NAN)));
        assert!(!is_falsy(&LegacyValue::Float(f64::INFINITY)));
    }

    #[test]
    fn underflow_and_signed_zero_are_falsy_but_keep_python_repr() {
        for text in [r#"{"p": 1e-400}"#, r#"{"p": -1e-400}"#, r#"{"p": 0.0}"#] {
            let object = parse_object(text).expect("valid object");
            assert!(is_falsy(object.get("p").unwrap()), "{text}");
        }
        assert_eq!(
            python_text(parse_object(r#"{"p": -1e-400}"#).unwrap().get("p").unwrap()),
            "-0.0"
        );
        assert_eq!(
            python_text(parse_object(r#"{"p": -0.0}"#).unwrap().get("p").unwrap()),
            "-0.0"
        );
    }

    #[test]
    fn exact_integers_are_never_coerced_through_f64() {
        assert_eq!(
            field(r#"{"p": 123456789012345678901234567890}"#, "p"),
            "123456789012345678901234567890"
        );
        let object = parse_object(r#"{"p": -0}"#).unwrap();
        assert_eq!(python_text(object.get("p").unwrap()), "0");
        assert!(is_falsy(object.get("p").unwrap()));
    }

    #[test]
    fn authored_strings_are_verbatim_and_nested_strings_use_python_repr() {
        assert_eq!(field(r#"{"p": "NaN"}"#, "p"), "NaN");
        assert_eq!(field(r#"{"p": "Infinity"}"#, "p"), "Infinity");
        assert_eq!(field(r#"{"p": {"k": "NaN"}}"#, "p"), "{'k': 'NaN'}");
        assert_eq!(
            field(r#"{"p": "line\nit's"}"#, "p"),
            "line\nit's",
            "a top-level string is verbatim"
        );
        assert_eq!(field(r#"{"p": [{"k": "it's"}]}"#, "p"), "[{'k': \"it's\"}]");
    }

    #[test]
    fn unicode_escapes_decode_raw_scalars() {
        assert_eq!(
            field(r#"{"p": "\u00e9 \ud83d\ude00 \u0000"}"#, "p"),
            "\u{e9} \u{1f600} \u{0}"
        );
    }

    #[test]
    fn lone_and_nonpaired_surrogates_are_preserved_as_code_points() {
        let object = parse_object(
            r#"{"p":["\ud800","\udc80","\ud800\u0041","\ud800\ud800\udc00","\ude00\ud83d","\ud83d\ude00"]}"#,
        )
        .expect("valid object");
        assert_eq!(
            python_text(object.get("p").unwrap()),
            "['\\ud800', '\\udc80', '\\ud800A', '\\ud800\u{10000}', '\\ude00\\ud83d', '\u{1f600}']"
        );
        // A used top-level string keeps the same code points; UTF-8 encoding is
        // the caller's fallible step and names the first surrogate unit.
        let object = parse_object(r#"{"p":"\udc80"}"#).expect("valid object");
        assert_eq!(python_str(object.get("p").unwrap()).to_utf8(), Err(0xdc80));
        let object = parse_object(r#"{"p":"\ud800"}"#).expect("valid object");
        assert_eq!(python_str(object.get("p").unwrap()).to_utf8(), Err(0xd800));
        let object = parse_object(r#"{"p":["\ud800"]}"#).expect("valid object");
        assert!(python_str(object.get("p").unwrap()).to_utf8().is_ok());
    }

    #[test]
    fn surrogate_keys_compare_and_deduplicate_by_code_point() {
        assert_eq!(
            field(r#"{"p":{"\ud800":1,"\ud800":2}}"#, "p"),
            r"{'\ud800': 2}"
        );
        assert_eq!(
            field(r#"{"p":{"\ud83d\ude00":1,"😀":2}}"#, "p"),
            "{'\u{1f600}': 2}"
        );
        assert_eq!(
            field(r#"{"p":{"\ud800\u0041":1,"\ud800A":2}}"#, "p"),
            r"{'\ud800A': 2}"
        );
        assert_eq!(
            field(r#"{"p":{"\ud83d\ude00":1,"\ud83d":2}}"#, "p"),
            "{'\u{1f600}': 1, '\\ud83d': 2}"
        );
    }

    #[test]
    fn nested_nonprintable_strings_use_the_verified_python_repr_table() {
        assert_eq!(field(r#"{"p":"a\u00a0b"}"#, "p"), "a\u{a0}b");
        assert_eq!(field(r#"{"p":{"k":"a\u00a0b"}}"#, "p"), r"{'k': 'a\xa0b'}");
        assert_eq!(
            field(r#"{"p":["\u200b","\u2029","\ue000","\u0378"]}"#, "p"),
            r"['\u200b', '\u2029', '\ue000', '\u0378']"
        );
    }

    #[test]
    fn optional_json_whitespace_around_one_object_is_accepted() {
        assert!(parse_object("  {\"p\": 1}  ").is_some());
        assert!(parse_object("\t{\"p\": 1}\t").is_some());
        assert!(parse_object("{}").is_some());
    }

    #[test]
    fn the_facade_stays_object_only_and_option_shaped() {
        for text in ["null", "true", "1", "[]", "\"x\"", "-0.5"] {
            assert!(parse_object(text).is_none(), "non-object {text:?}");
        }
        assert!(parse_object("").is_none());
        assert!(parse_object("[] x").is_none());
    }

    #[test]
    fn bad_syntax_and_non_objects_are_rejected() {
        for text in [
            "",
            " ",
            "NaN",
            "null",
            "[]",
            "[NaN]",
            r#"{"p": 1} x"#,
            r#"{"p": 1}{"q": 2}"#,
            r#"{"p": +Infinity}"#,
            r#"{"p": nan}"#,
            r#"{"p": -NaN}"#,
            r#"{"p": Inf}"#,
            r#"{"p": 1,}"#,
            r#"{"p": 01}"#,
            r#"{"p": .5}"#,
            r#"{"p": 1.}"#,
            r#"{"p": 1e}"#,
            r#"{"p": [1,]}"#,
            r#"{"p": {"a": 1,}}"#,
            r#"{"p" 1}"#,
            r#"{p: 1}"#,
            r#"{1: 2}"#,
            r#"{"p": "unterminated}"#,
            r#"{"p": "bad\qescape"}"#,
            r#"{"p": "raw
control"}"#,
            // Surrogate *escaping* is accepted like CPython, but a malformed
            // second escape is still a decode error.
            r#"{"p": "\ud800\uZZZZ"}"#,
            r#"{"p": "\ud800\u00"}"#,
            r#"{"p": "\ud800\udc0"}"#,
            // A UTF-8 BOM is an explicit `json.loads` error.
            "\u{feff}{\"p\": 1}",
        ] {
            assert!(parse_object(text).is_none(), "must reject {text:?}");
        }
    }
}

#[cfg(test)]
mod source_tests {
    //! Source-first binding for the legacy Op adapter.
    //!
    //! The pinned CPython reference reads the first Op line with
    //! `json.loads`; every expectation below comes from that run first, under
    //! asserted isolated roots. The adapter (`parse_object`) is only exercised
    //! after the Source assertions pass.

    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const HEADER: &str = concat!(
        "import json, os, sys\n",
        "from pathlib import Path\n",
        "assert sys.version.split()[0] == '3.11.15', sys.version\n",
        "root = Path(os.environ['SK_J1_ROOT'])\n",
        "assert Path.cwd() == root, Path.cwd()\n",
        "for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME'):\n",
        "    assert Path(os.environ[key]).parent == root, (key, os.environ[key])\n",
        "assert os.environ.get('PYTHONDONTWRITEBYTECODE') == '1'\n",
    );

    fn python() -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let executable = if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        };
        let python = root.join(".venv-rust-oracle").join(executable);
        assert!(
            python.is_file(),
            "required Python oracle is missing at {}",
            python.display()
        );
        python
    }

    fn run_source(script: &str, args: &[&str]) -> serde_json::Value {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "sk-j1-legacy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).expect("isolated Source root");
        for name in ["home", "config", "cache", "data", "state", "tmp"] {
            std::fs::create_dir_all(root.join(name)).expect("isolated directory");
        }
        let output = Command::new(python())
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
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            output.status.success(),
            "Source must run and assert first: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("Source JSON report")
    }

    fn field_codepoints(object: &LegacyObject, key: &str) -> Vec<u32> {
        match object.get(key) {
            Some(value) if !is_falsy(value) => match python_str(value) {
                PythonText::Verbatim(text) => text.codepoints().to_vec(),
                PythonText::Rendered(text) => text.chars().map(u32::from).collect(),
            },
            _ => "unknown".chars().map(u32::from).collect(),
        }
    }

    fn units(value: &serde_json::Value) -> Vec<u32> {
        value
            .as_array()
            .expect("code point array")
            .iter()
            .map(|unit| unit.as_u64().expect("code point") as u32)
            .collect()
    }

    #[test]
    fn source_first_op_first_line_semantics_match_the_adapter() {
        let cases = [
            (
                r#"{"invocation_id": "01X", "profile_id": "architect", "action": "implement"}"#,
                "profile_id",
            ),
            (r#"{"profile_id": "", "action": "a"}"#, "profile_id"),
            (r#"{"profile_id": null, "action": "a"}"#, "profile_id"),
            (r#"{"a": 1, "a": 3, "profile_id": "dup"}"#, "a"),
            (r#"{"profile_id": {"b": 1, "a": 2, "b": 3}}"#, "profile_id"),
            (
                r#"{"profile_id": ["\ud800", 1, NaN, -Infinity]}"#,
                "profile_id",
            ),
            (r#"{"profile_id": "\udc80"}"#, "profile_id"),
            (r#"{"profile_id": 1e400}"#, "profile_id"),
            (r#"{"profile_id": -0.0}"#, "profile_id"),
            (
                r#"{"profile_id": 123456789012345678901234567890}"#,
                "profile_id",
            ),
            (
                r#"{"profile_id": {"\ud800": 1}, "action": "a"}"#,
                "profile_id",
            ),
            (r#"{"profile_id": true}"#, "profile_id"),
            (r#"{"profile_id": 1, "profile_id": 2}"#, "profile_id"),
            // Rejections: the facade stays `Option`/object-only.
            (r#"{"a": 1,}"#, "a"),
            (r#"{"a": 1} x"#, "a"),
            ("[1]", "a"),
            ("NaN", "a"),
            (r#"{"a": }"#, "a"),
            (r#"{"a": [1, 2, ]}"#, "a"),
        ];
        let documents: Vec<&str> = cases.iter().map(|(document, _)| *document).collect();
        let keys: Vec<&str> = cases.iter().map(|(_, key)| *key).collect();
        let source = run_source(
            concat!(
                "documents = json.loads(sys.argv[1])\n",
                "keys = json.loads(sys.argv[2])\n",
                "report = []\n",
                "for document, key in zip(documents, keys):\n",
                "    try:\n",
                "        value = json.loads(document)\n",
                "    except json.JSONDecodeError:\n",
                "        report.append(['reject'])\n",
                "        continue\n",
                "    if not isinstance(value, dict):\n",
                "        report.append(['reject'])\n",
                "        continue\n",
                "    field = value.get(key)\n",
                "    text = field if field else 'unknown'\n",
                "    report.append([\n",
                "        'accept',\n",
                "        [ord(c) for c in json.dumps(value)],\n",
                "        [[ord(c) for c in name] for name in value],\n",
                "        [ord(c) for c in str(text)],\n",
                "    ])\n",
                "print(json.dumps(report))\n",
            ),
            &[
                &serde_json::to_string(&documents).expect("documents"),
                &serde_json::to_string(&keys).expect("keys"),
            ],
        );
        let rows = source.as_array().expect("Source rows");
        assert_eq!(rows.len(), cases.len());
        for ((document, key), row) in cases.iter().zip(rows) {
            let row = row.as_array().expect("row");
            let object = parse_object(document);
            if row[0] == "reject" {
                assert!(object.is_none(), "the adapter must reject {document:?}");
                continue;
            }
            let object = object.unwrap_or_else(|| panic!("adapter must accept {document:?}"));
            let rendered: Vec<u32> =
                sk_kernel::python_json::PythonJsonValue::ExactObject(object.clone())
                    .render_python(None, true)
                    .codepoints()
                    .to_vec();
            assert_eq!(rendered, units(&row[1]), "{document:?} dumps");
            let order: Vec<Vec<u32>> = row[2].as_array().expect("keys").iter().map(units).collect();
            let actual: Vec<Vec<u32>> = object
                .keys()
                .map(|name| name.codepoints().to_vec())
                .collect();
            assert_eq!(actual, order, "{document:?} key order");
            assert_eq!(
                field_codepoints(&object, key),
                units(&row[3]),
                "{document:?} field"
            );
        }
    }
}
