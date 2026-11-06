//! Source-first proof of the ONE canonical YAML object graph.
//!
//! Protocol (the wave's paired-execution discipline, same as
//! `python_yaml_source.rs`):
//!
//! 1. The parent writes each case's **original bytes** into a fresh temporary
//!    root and runs the pinned Source child (`oracle/python_yaml_graph.py`,
//!    CPython 3.11.15 / PyYAML 6.0.3 / ruamel 0.19.1) in an asserted
//!    CWD/HOME/all-seven-XDG/TMPDIR environment with `PYTHONHASHSEED=0`.
//! 2. It asserts the intended **cause** on the Source receipt (a real cycle with
//!    `a is a[0]`, two distinct `!!float nan` keys, one `.nan` key / a refusal,
//!    `hash(None) != hash(loader.nan_value)`, the pinned date/datetime hash
//!    facts) *before* the Native leg exists.
//! 3. It re-executes this test binary as a separate process with `env_clear()`
//!    and the same asserted roots; that leg decodes the same bytes with
//!    [`sk_kernel::python_yaml::decode_graph`] and compares structure, identity,
//!    `repr()` text and the pinned seed-0 hashes.
//!
//! This is bounded evidence for object identity/cycles/hash provenance. It is
//! not whole-decoder parity, and the tree projection those typed consumers still
//! read keeps its own declared open cases.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_kernel::PythonString;
use sk_kernel::python_value::PythonValue;
use sk_kernel::python_yaml::{Dialect, ErrorClass, Object, ObjectGraph, decode_graph};

#[path = "support/oracle.rs"]
mod oracle;

struct Case {
    label: &'static str,
    dialect: Dialect,
    text: &'static str,
}

const fn case(label: &'static str, dialect: Dialect, text: &'static str) -> Case {
    Case {
        label,
        dialect,
        text,
    }
}

fn corpus() -> Vec<Case> {
    use Dialect::{PyYamlSafe11 as Py, RuamelSafe12 as Ru};
    vec![
        case("cycle-list", Py, "a: &x [*x]\n"),
        case("cycle-list-12", Ru, "a: &x [*x]\n"),
        case("cycle-map", Py, "a: &x {k: *x}\n"),
        case("cycle-map-12", Ru, "a: &x {k: *x}\n"),
        case("alias-shared", Py, "a: &x [1, 2]\nb: *x\n"),
        case("alias-shared-12", Ru, "a: &x [1, 2]\nb: *x\n"),
        case("nan-fresh-2", Py, "{!!float nan: a, !!float nan: b}\n"),
        case("nan-fresh-2-12", Ru, "{!!float nan: a, !!float nan: b}\n"),
        case("nan-dot", Py, "{.nan: a, .nan: b}\n"),
        case("nan-dot-12", Ru, "{.nan: a, .nan: b}\n"),
        case("mixed-nan", Py, "{.nan: a, !!float nan: b}\n"),
        case("mixed-nan-12", Ru, "{.nan: a, !!float nan: b}\n"),
        case(
            "hash-facts",
            Py,
            "none: null\nnan: .nan\ndate: 2026-11-05\ndatetime: 2026-11-05T00:00:00\naware: 2026-11-05T00:00:00+00:00\ntext: a\nbytes: !!binary YQ==\nint: 1\nfloat: 1.0\ntuple: !!pairs [{a: b}]\n",
        ),
        case(
            "merge-shared",
            Py,
            "base: &b {x: 1}\nv:\n  <<: *b\n  y: 2\n",
        ),
        case(
            "merge-shared-12",
            Ru,
            "base: &b {x: 1}\nv:\n  <<: *b\n  y: 2\n",
        ),
    ]
}

fn dialect_name(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::PyYamlSafe11 => "pyyaml",
        Dialect::RuamelSafe12 => "ruamel",
    }
}

fn roots() -> [(&'static str, &'static str); 11] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "xdg_config_home"),
        ("XDG_CACHE_HOME", "xdg_cache_home"),
        ("XDG_DATA_HOME", "xdg_data_home"),
        ("XDG_STATE_HOME", "xdg_state_home"),
        ("XDG_RUNTIME_DIR", "xdg_runtime_dir"),
        ("XDG_CONFIG_DIRS", "xdg_config_dirs"),
        ("XDG_DATA_DIRS", "xdg_data_dirs"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}

fn child_environment(command: &mut Command, root: &Path, repo: &Path) {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        // Set iteration order and the string/tuple hash facts are properties of
        // the reference *process*; both children are pinned to seed 0 so they
        // have one observable answer.
        .env("PYTHONHASHSEED", "0")
        .env("PROBE_ROOT", root)
        .env("PROBE_REPO", repo);
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
}

fn main_leg() {
    // The Native leg is this binary re-executed with the same asserted roots; the
    // selector is an environment variable so libtest still owns the argv.
    let root = PathBuf::from(std::env::var_os("SK_L6_GRAPH_NATIVE").expect("native root"));
    let _repo =
        PathBuf::from(std::env::var_os("PYTHONPATH").expect("the asserted absolute source root"))
            .parent()
            .expect("the repository root")
            .to_path_buf();
    let rows: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("source.json")).unwrap()).unwrap();
    let rows = rows["rows"].as_array().unwrap().clone();
    let cases = corpus();
    assert_eq!(rows.len(), cases.len(), "one Source receipt per case");

    for (row, case) in rows.iter().zip(cases.iter()) {
        let bytes = fs::read(root.join("cases").join(format!("{}.yaml", case.label))).unwrap();
        let graph = decode_graph(
            std::str::from_utf8(&bytes).expect("the fixtures are UTF-8"),
            case.dialect,
        );
        assert_eq!(row["label"].as_str().unwrap(), case.label, "receipt order");
        let source_ok = row["outcome"].as_str().unwrap() == "ok";
        match (source_ok, graph) {
            (true, Ok(graph)) => compare(&graph, row, case),
            (true, Err(error)) => panic!(
                "{} ({}): Source accepted, Native refused: {error}",
                case.label,
                dialect_name(case.dialect)
            ),
            (false, Ok(_)) => panic!(
                "{} ({}): Source refused ({}), Native accepted",
                case.label,
                dialect_name(case.dialect),
                row["class"].as_str().unwrap_or("?")
            ),
            (false, Err(error)) => {
                let class = row["class"].as_str().unwrap();
                let accepted = match error.class() {
                    ErrorClass::Yaml => matches!(
                        class,
                        "ConstructorError"
                            | "DuplicateKeyError"
                            | "ScannerError"
                            | "ParserError"
                            | "ComposerError"
                            | "ReaderError"
                            | "YAMLError"
                    ),
                    ErrorClass::Runtime => error.python_class() == class,
                    other => panic!("unexpected Native class {other:?}"),
                };
                assert!(
                    accepted,
                    "{} ({}): Source {} vs Native {error:?}",
                    case.label,
                    dialect_name(case.dialect),
                    class
                );
                println!(
                    "{} ({}): both refused ({class})",
                    case.label,
                    dialect_name(case.dialect)
                );
            }
        }
    }
    println!(
        "object-graph cases compared against actual Source: {}",
        cases.len()
    );
}

fn compare(graph: &ObjectGraph, row: &serde_json::Value, case: &Case) {
    // The whole-document `repr()` text, derived from the graph's own structure.
    assert_eq!(
        graph.repr(graph.root()),
        row["repr"].as_str().unwrap(),
        "{} ({}): repr text",
        case.label,
        dialect_name(case.dialect)
    );
    if let Some(expected) = row["inner_is_self"].as_bool() {
        assert!(
            expected,
            "{}: Source must build a self reference",
            case.label
        );
        assert!(
            graph.has_cycle(),
            "{}: Native must build a cycle",
            case.label
        );
        let Object::Map(root) = graph.object(graph.root()) else {
            panic!("{}: expected a mapping root", case.label);
        };
        let anchored = root[0].1;
        let inner = match graph.object(anchored) {
            Object::Seq(items) => items[0],
            Object::Map(entries) => entries[0].1,
            other => panic!("{}: unexpected anchored kind {other:?}", case.label),
        };
        assert!(
            graph.is_same_object(inner, anchored),
            "{}: the self link must be the same object",
            case.label
        );
    }
    if let Some(expected) = row["alias_is_same"].as_bool() {
        assert!(expected);
        let Object::Map(root) = graph.object(graph.root()) else {
            panic!("expected a mapping root");
        };
        assert!(
            graph.is_same_object(root[0].1, root[1].1),
            "{}: an alias is one object",
            case.label
        );
    }
    if let Some(count) = row["key_count"].as_u64() {
        let Object::Map(entries) = graph.object(graph.root()) else {
            panic!("{}: expected a mapping root", case.label);
        };
        assert_eq!(
            entries.len() as u64,
            count,
            "{} ({}): key count",
            case.label,
            dialect_name(case.dialect)
        );
        if count > 1 {
            let (first, second) = (entries[0].0, entries[1].0);
            assert!(
                !graph.is_same_object(first, second),
                "{}: distinct NaN objects",
                case.label
            );
            assert!(!graph.equal(first, second), "{}: NaN != NaN", case.label);
            assert_ne!(
                graph.hash(first).unwrap(),
                graph.hash(second).unwrap(),
                "{}: distinct objects hash from distinct addresses",
                case.label
            );
        }
    }
    if row.get("hash-facts").is_some() || case.label == "hash-facts" {
        let Object::Map(entries) = graph.object(graph.root()) else {
            panic!("expected a mapping root");
        };
        let value_of = |name: &str| {
            entries
                .iter()
                .find_map(|(key, value)| match graph.object(*key) {
                    Object::Str(text) if text.to_scalar_string().as_deref() == Ok(name) => {
                        Some(*value)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no entry {name}"))
        };
        // Seed 0 in both processes: the pinned CPython hash facts.
        assert_eq!(
            graph.hash(value_of("date")).unwrap(),
            row["hash_date"].as_i64().unwrap()
        );
        assert_eq!(
            graph.hash(value_of("datetime")).unwrap(),
            row["hash_datetime"].as_i64().unwrap()
        );
        assert_eq!(
            graph.hash(value_of("aware")).unwrap(),
            row["hash_aware"].as_i64().unwrap()
        );
        assert_eq!(
            graph.hash(value_of("text")).unwrap(),
            row["hash_str_a"].as_i64().unwrap()
        );
        assert_eq!(
            graph.hash(value_of("bytes")).unwrap(),
            row["hash_bytes_a"].as_i64().unwrap()
        );
        assert_eq!(
            graph.hash(value_of("int")).unwrap(),
            row["hash_int_one"].as_i64().unwrap()
        );
        assert_eq!(
            graph.hash(value_of("float")).unwrap(),
            row["hash_float_one"].as_i64().unwrap()
        );
        // `hash(None)` and `hash(loader.nan_value)` are two different pointer
        // hashes in Source; the graph hashes the two real objects separately.
        let none = value_of("none");
        let nan = value_of("nan");
        assert_eq!(graph.type_name(none), "NoneType");
        assert_eq!(graph.type_name(nan), "float");
        assert_ne!(
            graph.hash(none).unwrap(),
            graph.hash(nan).unwrap(),
            "None and nan must not share a hash"
        );
        assert_ne!(
            Some(graph.hash(none).unwrap()),
            row["hash_loader_nan"].as_i64(),
            "the None hash is this process's own pointer hash"
        );
        // Projection still works for the acyclic hash-facts document.
        assert!(graph.to_python_value().is_ok());
        // A `!!pairs` value is a Python list of real `(key, value)` tuples.
        assert_eq!(graph.repr(value_of("tuple")), "[('a', 'b')]");
    }
    if case.label.starts_with("merge-shared") {
        let Object::Map(root) = graph.object(graph.root()) else {
            panic!("expected a mapping root");
        };
        let merged = root[1].1;
        assert_eq!(graph.repr(merged), "{'x': 1, 'y': 2}");
    }
}

#[test]
fn paired_object_graph_source_first() {
    // The Native leg: this same test binary, re-executed with a cleared env.
    if std::env::var_os("SK_L6_GRAPH_NATIVE").is_some() {
        main_leg();
        return;
    }

    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo = repo.canonicalize().expect("repository root");
    let root = std::env::temp_dir().join(format!(
        "sk-l6-graph-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(root.join("cases")).unwrap();
    for (key, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
        let _ = key;
    }
    let cases = corpus();
    let mut index = Vec::new();
    for case in &cases {
        fs::write(
            root.join("cases").join(format!("{}.yaml", case.label)),
            case.text,
        )
        .unwrap();
        index.push(serde_json::json!({
            "label": case.label,
            "dialect": dialect_name(case.dialect),
            "text": case.text,
        }));
    }
    fs::write(root.join("cases.json"), serde_json::to_vec(&index).unwrap()).unwrap();

    // 1. Actual Source, with the intended cause asserted below.
    let mut source = Command::new(oracle::python());
    child_environment(&mut source, &root, &repo);
    source
        .arg(repo.join("crates/sk-kernel/tests/oracle/python_yaml_graph.py"))
        .env("PROBE_CASES", root.join("cases.json"))
        .env("PROBE_OUT", root.join("source.json"));
    let output = source.output().expect("run the pinned Source child");
    assert!(
        output.status.success(),
        "Source child failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // 2. Assert the Source cause BEFORE any Native re-execution.
    let receipts: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("source.json")).unwrap()).unwrap();
    let rows = receipts["rows"].as_array().unwrap();
    let by_label = |label: &str| {
        rows.iter()
            .find(|row| row["label"].as_str() == Some(label))
            .unwrap_or_else(|| panic!("no receipt {label}"))
    };
    // Cycles: Source really builds one object that contains itself.
    for label in ["cycle-list", "cycle-list-12", "cycle-map", "cycle-map-12"] {
        let row = by_label(label);
        assert_eq!(row["outcome"], "ok", "{label}");
        assert_eq!(row["inner_is_self"], true, "{label}");
        let version = row["env"]["version"].as_array().expect("version tuple");
        assert_eq!(
            version
                .iter()
                .map(|part| part.as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![3, 11, 15],
            "{label} interpreter"
        );
    }
    assert_eq!(by_label("cycle-list")["repr"], "{'a': [[...]]}");
    assert_eq!(by_label("cycle-map-12")["repr"], "{'a': {'k': {...}}}");
    // Aliases are one object.
    assert_eq!(by_label("alias-shared")["alias_is_same"], true);
    // `!!float nan` is two fresh objects; `.nan` is the loader's singleton.
    for label in ["nan-fresh-2", "nan-fresh-2-12"] {
        assert_eq!(by_label(label)["key_count"], 2, "{label}");
        assert_eq!(by_label(label)["first_is_second"], false, "{label}");
    }
    assert_eq!(by_label("nan-dot")["key_count"], 1);
    assert_eq!(by_label("nan-dot-12")["class"], "DuplicateKeyError");
    assert_eq!(by_label("mixed-nan")["key_count"], 2);
    assert_eq!(by_label("mixed-nan-12")["key_count"], 2);
    // The pointer-hash facts the graph has to keep apart.
    let facts = by_label("hash-facts");
    assert_eq!(facts["none_is_nan"], false);
    assert_ne!(
        facts["hash_none"].as_i64(),
        facts["hash_loader_nan"].as_i64()
    );
    assert_eq!(facts["hash_date"].as_i64(), Some(7_311_123_894_698_728_974));
    assert_eq!(
        facts["hash_datetime"].as_i64(),
        Some(7_281_516_447_428_217_606)
    );
    assert_eq!(
        facts["hash_aware"].as_i64(),
        Some(-2_210_446_007_498_203_512)
    );
    // Every receipt carries the asserted sandbox it ran in.
    for row in rows {
        assert_eq!(row["env"]["seed"], "0");
        assert_eq!(row["env"]["cwd"], root.to_str().unwrap());
        assert_eq!(row["env"]["pythonpath"], repo.join("src").to_str().unwrap());
        assert_eq!(
            row["env"]["packs"],
            repo.join("packs").to_str().unwrap(),
            "packs root"
        );
    }

    // 3. The Native leg: a separate process with a cleared environment.
    let mut native = Command::new(std::env::current_exe().unwrap());
    child_environment(&mut native, &root, &repo);
    native
        .env("SK_L6_GRAPH_NATIVE", &root)
        .arg("--exact")
        .arg("paired_object_graph_source_first")
        .arg("--nocapture");
    let native = native.output().expect("re-execute the Native leg");
    println!("{}", String::from_utf8_lossy(&native.stdout));
    println!("{}", String::from_utf8_lossy(&native.stderr));
    assert!(
        native.status.success(),
        "the Native leg must accept the same original bytes (root {})",
        root.display()
    );
    println!("Source-first graph root retained: {}", root.display());
}

/// A Native-only control: the tree projection has no cycle, and the graph says so.
#[test]
fn graph_is_the_identity_authority_for_this_domain() {
    let graph = decode_graph("a: &x [*x]\n", Dialect::PyYamlSafe11).expect("graph accepts");
    assert!(graph.has_cycle());
    assert!(graph.to_python_value().is_err());
    let shared = decode_graph("a: &x [1]\nb: *x\n", Dialect::PyYamlSafe11).unwrap();
    assert!(!shared.has_cycle());
    let Object::Map(entries) = shared.object(shared.root()) else {
        panic!("expected a mapping");
    };
    assert!(shared.is_same_object(entries[0].1, entries[1].1));
    // The value domain is still one type: the projection of an acyclic document
    // is a real `PythonValue`, not a graph-local DTO.
    let projected = shared.to_python_value().unwrap();
    assert!(matches!(projected, PythonValue::Map(_)));
    let _ = BTreeMap::<String, PythonString>::new();
    let _: &dyn Fn(Dialect) -> &'static str = &dialect_name;
}
