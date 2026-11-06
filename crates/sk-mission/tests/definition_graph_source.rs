//! Source-first proof that the Mission loader reads the ONE canonical YAML object
//! graph — not a whole-root acyclic projection.
//!
//! The binding behaviour is Source's own:
//!
//! * a real cycle in an **ignored compatibility field** (`states`, `transitions`)
//!   is ACCEPTED by Source's model, so it must be accepted here and the authored
//!   cyclic data must still be present in the graph;
//! * a real cycle in a **required field** (`name`) is refused in the MODEL phase
//!   with `Input should be a valid string`, not by a transport/tree limit;
//! * an alias is one object, not a copied tree.
//!
//! Source runs FIRST, in the pinned oracle, asserting the intended cause (real
//! self-cycle, exact `str()`, shared identity) before the Native leg exists. Both
//! legs run in an asserted temporary CWD/HOME/all-seven-XDG/TMPDIR/PYTHONPATH/
//! packs sandbox and record raw stdout/stderr/status.
//!
//! Nothing here claims more than it proves: the decoder's declared `OPEN_CASES`
//! still travel with the verdict.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_kernel::python_yaml::Dialect;
use sk_mission::definition::{
    DefinitionError, PyYamlSafe, SafeDocumentDecoder, load_mission_definition,
};

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
        .env("LC_ALL", "C.UTF-8");
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
    command
}

fn base() -> String {
    "name: \"N\"\ndescription: \"d\"\nversion: \"1.0.0\"\ndomain: \"software\"\nworkflow:\n  phases:\n    - name: p\n      description: d\nartifacts: {}\n".to_string()
}

/// `(label, kind, authored mission.yaml)`.
fn corpus() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "ignored_list_cycle",
            "ignored_list_cycle",
            format!("{}states: &x [*x]\n", base()),
        ),
        (
            "ignored_map_cycle",
            "ignored_map_cycle",
            format!("{}transitions: &x {{k: *x}}\n", base()),
        ),
        (
            "required_name_cycle",
            "required_name_cycle",
            "name: &x [*x]\ndescription: \"d\"\nversion: \"1.0.0\"\ndomain: \"software\"\nworkflow:\n  phases:\n    - name: p\n      description: d\nartifacts: {}\n".to_string(),
        ),
        (
            "alias_shared_string",
            "alias_shared_string",
            base()
                .replace("name: \"N\"", "name: &n \"N\"")
                .replace("description: \"d\"", "description: *n"),
        ),
        (
            "ignored_cycle_missing_required",
            "ignored_cycle_missing_required",
            format!(
                "{}states: &x [*x]\n",
                base().replace("artifacts: {}\n", "")
            ),
        ),
    ]
}

fn write_cases(root: &Path) -> usize {
    fs::create_dir_all(root.join("cases")).unwrap();
    let cases = corpus();
    let mut index = Vec::new();
    for (label, kind, text) in &cases {
        let dir = format!("cases/{label}");
        fs::create_dir_all(root.join(&dir)).unwrap();
        fs::write(root.join(&dir).join("mission.yaml"), text).unwrap();
        index.push(serde_json::json!({"label": label, "dir": dir, "kind": kind}));
    }
    fs::write(
        root.join("cases/index.json"),
        serde_json::to_vec_pretty(&serde_json::Value::Array(index)).unwrap(),
    )
    .unwrap();
    cases.len()
}

#[derive(Debug, PartialEq)]
struct InputState {
    bytes: Vec<u8>,
    permissions: fs::Permissions,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64),
}

fn input_states(root: &Path) -> Vec<InputState> {
    corpus()
        .iter()
        .map(|(label, _, _)| {
            let path = root.join("cases").join(label).join("mission.yaml");
            let metadata = fs::metadata(&path).unwrap();
            InputState {
                bytes: fs::read(&path).unwrap(),
                permissions: metadata.permissions(),
                modified: metadata.modified().unwrap(),
                #[cfg(unix)]
                identity: {
                    use std::os::unix::fs::MetadataExt;
                    (metadata.dev(), metadata.ino())
                },
            }
        })
        .collect()
}

#[test]
fn receipt_retains_all_source_codepoints() {
    let value = serde_json::json!({"u": [0, 0xd800, 0xdcff, 0xe000, 0xffff, 0x10ffff]});
    assert_eq!(
        receipt_string(&value).codepoints(),
        &[0, 0xd800, 0xdcff, 0xe000, 0xffff, 0x10ffff]
    );
}

fn receipt_string(value: &serde_json::Value) -> sk_kernel::PythonString {
    let units: Vec<u32> = serde_json::from_value(value["u"].clone()).unwrap();
    sk_kernel::PythonString::from_codepoints(&units).expect("exact Source code points")
}

fn native_leg(root: &Path, repo: &Path) {
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

    let receipts: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("source.json")).unwrap()).unwrap();
    let rows = receipts["rows"].as_array().unwrap();
    assert_eq!(rows.len(), corpus().len());

    // The environment the Source child actually asserted travels into the
    // receipt; the Native leg is in its own asserted copy of the same sandbox.
    assert_eq!(
        receipts["env"]["cpython"],
        serde_json::json!([3, 11, 15]),
        "the pinned oracle must be CPython 3.11.15"
    );
    assert_eq!(receipts["env"]["cwd"], root.to_str().unwrap());
    assert_eq!(
        receipts["env"]["pythonpath"],
        repo.join("src").to_str().unwrap()
    );
    assert_eq!(
        receipts["env"]["packs"],
        repo.join("packs").to_str().unwrap()
    );

    let mut compared = 0usize;
    for row in rows {
        let label = row["label"].as_str().unwrap();
        let kind = row["kind"].as_str().unwrap();
        let config_path = root.join("cases").join(label).join("mission.yaml");
        let native = load_mission_definition(&config_path, &PyYamlSafe);
        let source_ok = row["ok"].as_bool().unwrap();
        assert_eq!(
            native.is_ok(),
            source_ok,
            "{label}: acceptance must match Source (native: {:?})",
            native.as_ref().err().map(|error| error.message())
        );

        match (native, source_ok) {
            (Ok(loaded), true) => {
                assert_eq!(
                    loaded.warnings,
                    row["warnings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(receipt_string)
                        .collect::<Vec<_>>(),
                    "{label}: warnings must match Source"
                );
                assert_eq!(
                    loaded.definition.name.to_scalar_string().unwrap(),
                    "N",
                    "{label}: authored data must survive the graph read"
                );
                if kind == "alias_shared_string" {
                    // The alias is one object: Source proved `is`; the graph must
                    // preserve that identity, not copy the tree.
                    let document = PyYamlSafe
                        .decode(&config_path, &fs::read_to_string(&config_path).unwrap())
                        .unwrap();
                    let root_view = document.root();
                    let name = root_view.lookup("name").unwrap();
                    let description = root_view.lookup("description").unwrap();
                    assert!(
                        name.is_same_object(description),
                        "{label}: an alias must stay one graph object"
                    );
                }
                if kind.starts_with("ignored_") {
                    // The ignored field is still a real cycle in the graph; the
                    // model simply never traverses it.
                    let document = PyYamlSafe
                        .decode(&config_path, &fs::read_to_string(&config_path).unwrap())
                        .unwrap();
                    assert!(
                        document.has_cycle(),
                        "{label}: the authored cycle must be preserved, not dropped"
                    );
                    let field = if kind == "ignored_map_cycle" {
                        "transitions"
                    } else {
                        "states"
                    };
                    let viewed = document.root().lookup(field).unwrap();
                    assert_eq!(
                        sk_kernel::PythonString::from_scalars(&viewed.repr()),
                        receipt_string(&row["observed"]["str"]),
                        "{label}: the graph text authority must match Source's own str()"
                    );
                }
            }
            (Err(error), false) => {
                assert_eq!(
                    error.message_codepoints(),
                    receipt_string(&row["error"]),
                    "{label}: the rendered model diagnostic must match Source"
                );
                assert!(
                    matches!(error, DefinitionError::InvalidConfig { .. }),
                    "{label}: a required-field cycle is a MODEL-phase issue, not a loader refusal"
                );
            }
            _ => unreachable!("acceptance compared above"),
        }
        compared += 1;
    }
    assert_eq!(compared, corpus().len());
    println!("{compared} graph-consumer definition cases compared against actual Source");
}

#[test]
fn mission_loader_reads_the_graph_and_matches_actual_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();

    if let Some(root) = std::env::var_os("SK_L7_DEFINITION_GRAPH_NATIVE") {
        let root = PathBuf::from(root);
        native_leg(&root, &repo);
        return;
    }

    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let cases = write_cases(&root);
    let before = input_states(&root);
    eprintln!("graph definition evidence: {}", root.display());

    // Source FIRST: the pinned oracle asserts the intended causes itself.
    let source = child(&root, &repo, oracle::python())
        .arg("-c")
        .arg(include_str!("oracle/definition_graph.py"))
        .arg(&root)
        .arg(&repo)
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
        input_states(&root),
        before,
        "Source loader changed authored input metadata"
    );

    let native = child(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mission_loader_reads_the_graph_and_matches_actual_source",
            "--nocapture",
        ])
        .env("SK_L7_DEFINITION_GRAPH_NATIVE", &root)
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
        input_states(&root),
        before,
        "Native loader changed authored input metadata"
    );
    assert!(cases >= 5);
    let _ = Dialect::PyYamlSafe11;
}
