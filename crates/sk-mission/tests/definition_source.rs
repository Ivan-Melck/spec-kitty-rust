//! Source-first, exact proof of the canonical Mission definition model.
//!
//! Protocol (the wave's paired-execution discipline, not L2's direct-in-Cargo
//! pattern):
//!
//! 1. The parent process creates a fresh temporary root with one `mission.yaml`
//!    per case, then runs the **pinned Source** (`verify_oracle.py`-verified
//!    CPython 3.11.15) in a fully asserted temporary CWD/HOME/XDG/TMPDIR
//!    environment. Source emits, per case, the PyYAML-decoded document (a
//!    declared transport encoding of the *Python* values), its accept/reject
//!    verdict, its exact rendered message, its warnings, and its config dump.
//! 2. The parent then re-executes this test binary as a **separate process** with
//!    the same root, and that Native leg asserts its own CWD/HOME/XDG/TMPDIR/
//!    PYTHONPATH/packs before comparing the Native model against those receipts.
//!
//! What this proves: the **Native decoder** turns the ORIGINAL authored
//! `mission.yaml` bytes into the same document Source's PyYAML `safe_load`
//! produces, and the validation semantics (accepted vs refused, field domains,
//! defaults, diagnostics, warnings, decoded config) are Source's. No
//! Source-decoded value tree is injected into the model any more: the Native leg
//! reads the file itself, through `PyYamlSafe`.
//!
//! Two extra checks keep that honest:
//!
//! * the Native-decoded document is compared node-for-node with Source's own
//!   decoded document (for the cases whose construction has one canonical
//!   representation — a `!!set`/`!!omap`/`!!pairs` order is process- or
//!   dialect-owned, so those are excluded and counted);
//! * the YAML 1.2-core adapter ([`CoreSchemaYaml`]) is still asserted to diverge,
//!   as a cross-check that the Source dialect, not a 1.2 core read, is doing the
//!   work — the three documents Source refuses are now genuinely refused by the
//!   Source-dialect decoder.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_kernel::PythonString;
use sk_mission::definition::{
    CoreSchemaYaml, DefinitionError, DialectFidelity, MissionDefinition, MissionDomain, PyYamlSafe,
    SafeDocumentDecoder, SourceValue, load_mission_definition,
};

/// The environment roots every child must have, all inside the temp root.
fn roots() -> [(&'static str, &'static str); 9] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
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

/// The fixture corpus. Each entry is one authored `mission.yaml`.
fn corpus() -> Vec<(String, String)> {
    let base = "name: \"N\"\ndescription: \"d\"\nversion: \"1.0.0\"\ndomain: \"software\"\nworkflow:\n  phases:\n    - name: p\n      description: d\nartifacts: {}\n".to_string();
    let cases: Vec<(&str, String)> = vec![
        (
            "valid_full",
            "name: \"Software Dev Kitty\"\ndescription: \"d\"\nversion: \"1.0.0\"\ndomain: \"software\"\nworkflow:\n  phases:\n    - name: research\n      description: r\n    - name: design\n      description: d2\nartifacts:\n  required: [spec.md, plan.md]\n  optional: [data-model.md, contracts/, checklists/]\npaths:\n  workspace: \"src/\"\n  tests: \"tests/\"\n  deliverables: \"contracts/\"\n  documentation: \"docs/\"\nvalidation:\n  checks: [git_clean]\n  custom_validators: true\nmcp_tools:\n  required: [filesystem]\n  recommended: [git]\n  optional: [github]\nagent_context: |\n  line1\n  line2\ntask_metadata:\n  required: [task_id]\n  optional: [assignee]\ncommands:\n  plan:\n    prompt: design\n  tasks:\n    prompt: break down\ntask_types:\n  build:\n    agent_role: rusty\n    description: compile\n".to_string(),
        ),
        ("valid_min", base.clone()),
        (
            "compat_keys",
            format!(
                "{base}mission: x\ninitial: s0\nstates: {{}}\ntransitions: []\nguards: {{}}\ninputs: []\noutputs: []\n"
            ),
        ),
        ("dup_root_keys", format!("{base}paths:\n  workspace: 'lib/'\n")),
        ("name_yes", base.replace("name: \"N\"", "name: yes")),
        ("name_int", base.replace("name: \"N\"", "name: 12")),
        ("name_null", base.replace("name: \"N\"", "name: null")),
        ("name_empty", base.replace("name: \"N\"", "name: \"\"")),
        (
            "name_surrogate",
            base.replace("name: \"N\"", "name: \"\\uD800\""),
        ),
        (
            "name_nonascii",
            base.replace("name: \"N\"", "name: \"héllo ☃\""),
        ),
        (
            "version_float",
            base.replace("version: \"1.0.0\"", "version: 1.0"),
        ),
        (
            "version_short",
            base.replace("version: \"1.0.0\"", "version: \"1.0\""),
        ),
        (
            "version_lead0",
            base.replace("version: \"1.0.0\"", "version: \"01.0.0\""),
        ),
        (
            "version_suffix",
            base.replace("version: \"1.0.0\"", "version: \"1.0.0-alpha\""),
        ),
        (
            "domain_caps",
            base.replace("domain: \"software\"", "domain: \"Software\""),
        ),
        (
            "domain_unknown",
            base.replace("domain: \"software\"", "domain: \"finance\""),
        ),
        (
            "bool_int_one",
            format!("{base}validation:\n  custom_validators: 1\n"),
        ),
        (
            "bool_int_two",
            format!("{base}validation:\n  custom_validators: 2\n"),
        ),
        (
            "bool_str_false",
            format!("{base}validation:\n  custom_validators: 'false'\n"),
        ),
        (
            "bool_str_maybe",
            format!("{base}validation:\n  custom_validators: 'maybe'\n"),
        ),
        (
            "bool_yaml_no",
            format!("{base}validation:\n  custom_validators: no\n"),
        ),
        (
            "paths_unknown_key",
            base.replace(
                "artifacts: {}\n",
                "artifacts: {}\npaths:\n  workspace: \"src/\"\n  zeta: \"z/\"\n  alpha: \"a/\"\n",
            ),
        ),
        (
            "paths_int_value",
            format!("{base}paths:\n  workspace: 12\n"),
        ),
        (
            "paths_bool_value",
            format!("{base}paths:\n  workspace: yes\n"),
        ),
        (
            "paths_null",
            format!("{base}paths: null\n"),
        ),
        (
            "paths_nonstr_key",
            format!("{base}paths:\n  5: 'x/'\n"),
        ),
        (
            "artifacts_absent_block",
            base.replace("artifacts: {}\n", "artifacts: {}\npaths:\n  workspace: \"src\"\n"),
        ),
        (
            "artifacts_missing",
            base.replace("artifacts: {}\n", ""),
        ),
        (
            "artifacts_required_str",
            base.replace("artifacts: {}\n", "artifacts:\n  required: \"spec.md\"\n"),
        ),
        (
            "artifacts_item_int",
            base.replace("artifacts: {}\n", "artifacts:\n  required: [1, 2]\n"),
        ),
        (
            "artifacts_item_bytes",
            base.replace(
                "artifacts: {}\n",
                "artifacts:\n  required: [!!binary c3BlYy5tZA==]\n",
            ),
        ),
        (
            "phase_missing_desc",
            base.replace("      description: d\n", ""),
        ),
        ("phases_empty", base.replace("phases:\n    - name: p\n      description: d\n", "phases: []\n")),
        (
            "phase_extra",
            base.replace("      description: d\n", "      description: d\n      bogus: 1\n"),
        ),
        ("extra_root", format!("{base}bogus_root: 1\n")),
        ("root_list", "- a\n- b\n".to_string()),
        ("root_scalar", "hello\n".to_string()),
        ("empty_file", String::new()),
        ("only_comments", "# nothing\n".to_string()),
        ("workflow_null", base.replace("workflow:\n  phases:\n    - name: p\n      description: d\n", "workflow: null\n")),
        (
            "validation_null",
            format!("{base}validation: null\n"),
        ),
        (
            "validation_extra",
            format!("{base}validation:\n  bogus: 1\n"),
        ),
        (
            "mcp_null",
            format!("{base}mcp_tools: null\n"),
        ),
        (
            "task_metadata_extra",
            format!("{base}task_metadata:\n  required: [a]\n  bogus: 1\n"),
        ),
        (
            "commands_prompt_absent",
            format!("{base}commands:\n  plan: {{}}\n"),
        ),
        (
            "commands_prompt_int",
            format!("{base}commands:\n  plan:\n    prompt: 5\n"),
        ),
        (
            "commands_null",
            format!("{base}commands: null\n"),
        ),
        (
            "task_types_partial",
            format!("{base}task_types:\n  build:\n    agent_role: rusty\n"),
        ),
        (
            "task_types_null",
            format!("{base}task_types: null\n"),
        ),
        (
            "agent_context_list",
            format!("{base}agent_context: [a, b]\n"),
        ),
        (
            "agent_context_null",
            format!("{base}agent_context: null\n"),
        ),
        (
            "timestamp_paths_value",
            format!("{base}paths:\n  workspace: 2026-11-05\n"),
        ),
        (
            "octal_version",
            base.replace("version: \"1.0.0\"", "version: 017"),
        ),
        (
            "sexagesimal_version",
            base.replace("version: \"1.0.0\"", "version: 1:30"),
        ),
        (
            "merge_key_paths",
            format!(
                "defaults: &d\n  workspace: \"src/\"\n{base}paths:\n  <<: *d\n"
            ),
        ),
        (
            "set_value",
            format!("{base}paths:\n  workspace: !!set {{a, b}}\n"),
        ),
        (
            "merge_key_applied",
            base.replace(
                "artifacts: {}\n",
                "artifacts:\n  <<: &shared {required: [spec.md]}\n  optional: [plan.md]\n",
            ),
        ),
        (
            "alias_shared_artifacts",
            base.replace(
                "artifacts: {}\n",
                "artifacts: &shared {required: [spec.md]}\nvalidation:\n  checks: [git_clean]\n",
            ),
        ),
        ("yaml_tab_indent", "a:\n\t- 1\n".to_string()),
    ];
    cases
        .into_iter()
        .map(|(label, yaml)| (label.to_owned(), yaml))
        .collect()
}

fn write_cases(root: &Path) -> Vec<(String, PathBuf)> {
    let cases_dir = root.join("cases");
    fs::create_dir_all(&cases_dir).unwrap();
    let mut index = Vec::new();
    for (position, (label, yaml)) in corpus().into_iter().enumerate() {
        let relative = format!("cases/{position:02}-{label}");
        let directory = root.join(&relative);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("mission.yaml"), yaml).unwrap();
        index.push((label, directory));
    }
    let index_json: Vec<serde_json::Value> = index
        .iter()
        .enumerate()
        .map(|(position, (label, _))| {
            serde_json::json!({
                "label": label,
                "dir": format!("cases/{position:02}-{label}"),
            })
        })
        .collect();
    fs::write(
        cases_dir.join("index.json"),
        serde_json::to_vec_pretty(&serde_json::Value::Array(index_json)).unwrap(),
    )
    .unwrap();
    index
}

fn decode_document(value: &serde_json::Value) -> Option<SourceValue> {
    let tag = value.get("t")?.as_str()?;
    Some(match tag {
        "none" => SourceValue::Null,
        "bool" => SourceValue::Bool(value.get("v")?.as_bool()?),
        "int" => SourceValue::Int(value.get("v")?.as_str()?.to_owned()),
        "float" => SourceValue::Float(value.get("v")?.as_f64()?),
        "str" => {
            let units: Vec<u32> = serde_json::from_value(value.get("v")?.clone()).ok()?;
            SourceValue::Str(PythonString::from_codepoints(&units).ok()?)
        }
        "bytes" => SourceValue::Bytes(serde_json::from_value(value.get("v")?.clone()).ok()?),
        "date" => SourceValue::Date(value.get("v")?.as_str()?.to_owned()),
        "datetime" => SourceValue::DateTime(value.get("v")?.as_str()?.to_owned()),
        "set" => SourceValue::Set(
            value
                .get("v")?
                .as_array()?
                .iter()
                .filter_map(decode_document)
                .collect(),
        ),
        "seq" => SourceValue::Seq(
            value
                .get("v")?
                .as_array()?
                .iter()
                .filter_map(decode_document)
                .collect(),
        ),
        "map" => SourceValue::Map(
            value
                .get("v")?
                .as_array()?
                .iter()
                .map(|pair| {
                    let pair = pair.as_array()?;
                    Some((decode_document(&pair[0])?, decode_document(&pair[1])?))
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    })
}

/// The authored text of one fixture, read exactly as the loader reads it.
fn document_text(path: &Path) -> String {
    let bytes = fs::read(path).unwrap();
    sk_kernel::python_yaml::read_document_text(
        &bytes,
        sk_kernel::python_yaml::Dialect::PyYamlSafe11,
    )
    .unwrap_or_else(|error| panic!("{}: {}", path.display(), error))
}

/// Whether a receipt document contains a constructed kind with no canonical
/// ordering (`set`, `pairs`, `ordereddict`).
fn receipt_has_ordered_container(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => {
            if matches!(
                fields.get("t").and_then(|tag| tag.as_str()),
                Some("set") | Some("pairs") | Some("ordereddict")
            ) {
                return true;
            }
            fields.values().any(receipt_has_ordered_container)
        }
        serde_json::Value::Array(items) => items.iter().any(receipt_has_ordered_container),
        _ => false,
    }
}

/// Decode a receipt string (a `{"u": [code points]}` transport node) exactly.
fn receipt_string(value: &serde_json::Value) -> PythonString {
    let units: Vec<u32> = serde_json::from_value(value["u"].clone())
        .unwrap_or_else(|error| panic!("receipt string is not a code-point list: {error}"));
    PythonString::from_codepoints(&units).expect("receipt code points")
}

fn receipt_strings(value: &serde_json::Value) -> Vec<PythonString> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("receipt string list expected"))
        .iter()
        .map(receipt_string)
        .collect()
}

/// Encode an authored string the same way the receipt does.
fn units(value: &PythonString) -> serde_json::Value {
    serde_json::json!({ "u": value.codepoints() })
}

fn dump(definition: &MissionDefinition) -> serde_json::Value {
    serde_json::json!({
        "name": units(&definition.name),
        "description": units(&definition.description),
        "version": units(&definition.version),
        "domain": units(&PythonString::from_scalars(definition.domain.as_str())),
        "workflow": {
            "phases": definition
                .workflow
                .phases
                .iter()
                .map(|phase| serde_json::json!({
                    "name": units(&phase.name),
                    "description": units(&phase.description),
                }))
                .collect::<Vec<_>>(),
        },
        "artifacts": {
            "required": definition.required_artifacts().iter().map(units).collect::<Vec<_>>(),
            "optional": definition.optional_artifacts().iter().map(units).collect::<Vec<_>>(),
        },
        "paths": definition
            .paths()
            .iter()
            .map(|(key, value)| serde_json::json!([units(key), units(value)]))
            .collect::<Vec<_>>(),
        "validation": {
            "checks": definition
                .validation
                .checks
                .iter()
                .map(units)
                .collect::<Vec<_>>(),
            "custom_validators": definition.validation.custom_validators,
        },
        "mcp_tools": definition.mcp_tools.as_ref().map(|tools| serde_json::json!({
            "required": tools.required.iter().map(units).collect::<Vec<_>>(),
            "recommended": tools.recommended.iter().map(units).collect::<Vec<_>>(),
            "optional": tools.optional.iter().map(units).collect::<Vec<_>>(),
        })),
        "agent_context": definition.agent_context.as_ref().map(units),
        "task_metadata": definition.task_metadata.as_ref().map(|metadata| serde_json::json!({
            "required": metadata.required.iter().map(units).collect::<Vec<_>>(),
            "optional": metadata.optional.iter().map(units).collect::<Vec<_>>(),
        })),
        "commands": definition.commands.as_ref().map(|commands| commands
            .iter()
            .map(|(key, command)| {
                serde_json::json!([units(key), {"prompt": units(&command.prompt)}])
            })
            .collect::<Vec<_>>()),
        "task_types": definition.task_types.as_ref().map(|types| types
            .iter()
            .map(|(key, task_type)| {
                serde_json::json!([units(key), {
                    "agent_role": task_type.agent_role.as_ref().map(units),
                    "description": task_type.description.as_ref().map(units),
                }])
            })
            .collect::<Vec<_>>()),
    })
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

    let receipts: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("source.json")).unwrap()).unwrap();
    let rows = receipts["rows"].as_array().unwrap();
    assert_eq!(rows.len(), corpus().len(), "one receipt per fixture case");

    let mut compared = 0usize;
    let mut documents_compared = 0usize;
    let mut excluded = 0usize;
    for row in rows {
        let label = row["label"].as_str().unwrap();
        let parsed_ok = row["parsed_ok"].as_bool().unwrap();
        let case_dir = fs::read_dir(root.join("cases"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&format!("-{label}")))
            })
            .unwrap_or_else(|| panic!("fixture directory for {label}"));
        let config_path = case_dir.join("mission.yaml");

        let native = load_mission_definition(&config_path, &PyYamlSafe);
        if !parsed_ok {
            // Source's loader refused the document (`yaml.YAMLError`); the
            // Source-dialect decoder must refuse it too. Its message is the
            // semantic reason (open case: no libyaml caret block), so the class
            // is compared, not the prose.
            match &native {
                Err(DefinitionError::InvalidYaml { detail, .. }) => {
                    assert!(
                        detail.contains("YAML")
                            || detail.contains("duplicate")
                            || detail.contains("unacceptable")
                            || detail.contains("expected")
                            || detail.contains("found"),
                        "{label}: a refused document must report the loader's reason, got {detail}"
                    );
                }
                other => panic!(
                    "{label}: Native must refuse an undecodable document, got {:?}",
                    other.as_ref().err().map(|error| error.message())
                ),
            }
            continue;
        }

        // The Native decoder must reproduce Source's decoded *document*, not just
        // the verdict. `Set`/`Pairs`/`OrderedDict` have no single canonical
        // ordering (a set's order belongs to the CPython process, and PyYAML's
        // `!!omap` is a list of tuples), so those receipts are excluded here and
        // counted below; their construction is proven by the Kernel cases.
        //
        // The decoder now yields the canonical object graph; the legacy tree is a
        // field-scoped projection used only to compare an acyclic receipt.
        let expected = decode_document(&row["document"])
            .unwrap_or_else(|| panic!("{label}: receipt document is not a decodable value tree"));
        let decoded = PyYamlSafe
            .decode(&config_path, &document_text(&config_path))
            .unwrap_or_else(|detail| panic!("{label}: Native decode failed: {detail}"));
        if receipt_has_ordered_container(&row["document"]) {
            excluded += 1;
        } else {
            assert_eq!(
                decoded
                    .root()
                    .to_python_value()
                    .expect("an acyclic receipt must project"),
                expected,
                "{label}: the Native decoder must produce Source's decoded document"
            );
            documents_compared += 1;
        }
        let native = MissionDefinition::from_document(&config_path, &decoded);
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
                    receipt_strings(&row["warnings"]),
                    "{label}: warnings must match Source"
                );
                assert_eq!(
                    dump(&loaded.definition),
                    row["config"],
                    "{label}: decoded config must match Source model_dump()"
                );
            }
            (Err(error), false) => {
                assert_eq!(
                    error.message_codepoints(),
                    receipt_string(&row["error"]),
                    "{label}: rendered diagnostic must match Source"
                );
                assert!(
                    matches!(
                        error,
                        DefinitionError::InvalidConfig { .. } | DefinitionError::NotAMapping { .. }
                    ),
                    "{label}: a refused document is a typed config failure"
                );
            }
            _ => unreachable!("acceptance compared above"),
        }
        compared += 1;
    }
    assert!(compared >= 40, "compared {compared} cases");
    assert!(
        documents_compared >= 40,
        "compared {documents_compared} decoded documents ({excluded} excluded for a process-owned order)"
    );
    println!(
        "{compared} definition cases and {documents_compared} decoded documents compared against actual Source ({excluded} set/pairs cases excluded from document equality)"
    );

    // Dialect parity, then the 1.2-core cross-check. Each row records the Source
    // verdict (from the receipt) and the ACTUAL verdict of both decoders. The
    // Source-dialect decoder must EQUAL Source on every row — including the three
    // documents a 1.2-core read used to accept by mistake (`name: yes`,
    // `paths: {workspace: yes}`, `paths: {workspace: 2026-11-05}`), which are now
    // genuine Source-equal refusals. The 1.2-core adapter's divergence is kept as
    // a cross-check so the dialect seam stays visible.
    let dialect_cases: [(&str, bool, bool); 8] = [
        ("name_yes", false, true),
        ("paths_bool_value", false, true),
        ("timestamp_paths_value", false, true),
        ("octal_version", false, false),
        ("sexagesimal_version", false, false),
        ("set_value", false, false),
        ("merge_key_paths", false, false),
        ("yaml_tab_indent", false, false),
    ];
    let mut parity = 0usize;
    for (label, source_ok_expected, core_ok_expected) in dialect_cases {
        let row = rows
            .iter()
            .find(|row| row["label"].as_str() == Some(label))
            .unwrap_or_else(|| panic!("receipt row for {label}"));
        assert_eq!(
            row["ok"].as_bool().unwrap_or(false),
            source_ok_expected,
            "{label}: recorded Source verdict changed"
        );
        let config_path = fs::read_dir(root.join("cases"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&format!("-{label}")))
            })
            .unwrap_or_else(|| panic!("fixture directory for {label}"))
            .join("mission.yaml");
        let native = load_mission_definition(&config_path, &PyYamlSafe);
        assert_eq!(
            native.is_ok(),
            source_ok_expected,
            "{label}: the PyYAML-dialect decoder must match Source (native error: {:?})",
            native.err().map(|error| error.message())
        );
        parity += 1;
        let core = load_mission_definition(&config_path, &CoreSchemaYaml);
        assert_eq!(
            core.is_ok(),
            core_ok_expected,
            "{label}: the YAML 1.2-core cross-check verdict changed (native error: {:?})",
            core.err().map(|error| error.message())
        );
    }
    // The Source-dialect decoder must not claim more fidelity than it has: the
    // open cases travel with its verdict.
    match PyYamlSafe.fidelity() {
        DialectFidelity::SourceDialect { open, dialect } => {
            assert_eq!(dialect, sk_kernel::python_yaml::Dialect::PyYamlSafe11);
            assert_eq!(open, sk_kernel::python_yaml::OPEN_CASES);
            assert!(
                !open.is_empty(),
                "an unqualified fidelity claim is not allowed"
            );
        }
        other => panic!("expected the PyYAML Source dialect, got {other:?}"),
    }
    println!("{parity} dialect cases matched Source under PyYamlSafe");
    compared
}

#[test]
fn definition_validation_matches_actual_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();

    if let Some(root) = std::env::var_os("SK_L3_DEFINITION_NATIVE") {
        let root = PathBuf::from(root);
        let compared = native_leg(&root, &repo);
        println!("{compared} definition cases compared against actual Source");
        return;
    }

    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let cases = write_cases(&root);
    assert!(cases.len() >= 50);
    eprintln!("definition evidence: {}", root.display());

    let source = child(&root, &repo, oracle::python())
        .arg("-c")
        .arg(include_str!("oracle/definition_model.py"))
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

    let native = child(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "definition_validation_matches_actual_source",
            "--nocapture",
        ])
        .env("SK_L3_DEFINITION_NATIVE", &root)
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
}

#[test]
fn domain_tokens_are_the_closed_source_set() {
    assert_eq!(
        MissionDomain::parse("software"),
        Some(MissionDomain::Software)
    );
    assert_eq!(MissionDomain::parse("Software"), None);
    assert_eq!(MissionDomain::parse("finance"), None);
    assert!(MissionDomain::parse("research").unwrap().is_research());
}

#[test]
fn the_definition_value_type_is_the_kernel_value_type() {
    // The promoted model is ONE type: the definition model's `SourceValue` and the
    // Kernel floor's `PythonValue` are the same type (same TypeId), so the shared
    // decoder's output flows into the model without a second DTO or a lossy
    // conversion.
    fn same_type(value: sk_kernel::python_value::PythonValue) -> SourceValue {
        value
    }
    let decoded = sk_kernel::python_yaml::decode_text(
        "v: 017\n",
        sk_kernel::python_yaml::Dialect::PyYamlSafe11,
    )
    .unwrap()
    .value;
    let promoted = same_type(decoded);
    assert!(matches!(promoted, SourceValue::Map(_)));
    // And the compatibility names are the promoted type, not wrappers.
    let ordered: sk_mission::definition::OrderedMap<PythonString> =
        sk_mission::definition::OrderedMap::from_pairs([(
            PythonString::from_scalars("k"),
            PythonString::from_scalars("v"),
        )]);
    assert_eq!(ordered.len(), 1);
}
