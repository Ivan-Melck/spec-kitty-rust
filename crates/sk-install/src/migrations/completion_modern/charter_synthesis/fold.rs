//! `consolidate_charter_bundle_fold`: fold the legacy charter bundle and the
//! `config.yaml`-embedded activation into the single git-tracked `charter.yaml`.
//!
//! Ports `m_unify_charter_activation_finalize.py`. It folds the four retired
//! bundle files (`governance.yaml`, `directives.yaml`, `metadata.yaml`,
//! `references.yaml`) plus the `activated_*` keys that still live in
//! `config.yaml` into `.kittify/charter/charter.yaml`, retires the four files, and
//! mints the `.kittify/config.yaml` `charter:` pointer.
//!
//! Contract points that the port preserves:
//!
//! * **Deterministic, VERBATIM activation copy.** Only keys ACTUALLY PRESENT in
//!   `config.yaml` are transplanted, so an absent key stays absent (never
//!   invented as `[]`, which would flip "all built-ins active" to "none active")
//!   and an explicit `[]` stays `[]`.
//! * **Idempotent.** Once `charter.yaml` exists, the four legacy files are gone
//!   and `config.yaml` carries no activation key, both `detect` and `apply`
//!   report a clean zero-change no-op.
//! * **Refuses a legacy `governance.doctrine`.** The charter-pack cutover (which
//!   runs first) rewrites it to `charter`; composing `charter.yaml` from the
//!   un-rewritten file would silently drop the selection.
//! * **Retirement is guarded.** A legacy file whose bytes are not provably
//!   captured by the new `charter.yaml` is archived verbatim under `.kittify/`
//!   before removal, never silently deleted.
//!
//! Everything is written through the one round-trip CST editor, so comments,
//! quotes and key order survive; `charter.yaml` is only ever replaced wholesale
//! when it does not exist yet (there is no prior authored content to destroy).

use std::path::{Path, PathBuf};

use serde_yaml_ng::{Mapping, Value};
use sk_charter::{canonical_yaml, ordered_yaml};

use crate::error::InstallError;
use crate::migrations::completion_legacy::paths::{read_text, write_bytes, write_text};
use crate::preserve::{Prover, RemovalVerdict, guard_destructive_removal};
use crate::upgrade::{MigrationOutcome, NativeMigration};
use crate::yaml_edit::CstDocument;

const KITTIFY_DIRNAME: &str = ".kittify";
const CHARTER_DIRNAME: &str = "charter";
const CONFIG_FILENAME: &str = "config.yaml";
const CHARTER_POINTER_KEY: &str = "charter";

const GOVERNANCE_YAML: &str = "governance.yaml";
const DIRECTIVES_YAML: &str = "directives.yaml";
const METADATA_YAML: &str = "metadata.yaml";
const REFERENCES_YAML: &str = "references.yaml";
const CHARTER_YAML: &str = "charter.yaml";

/// The pre-rename governance selection key of a standalone `governance.yaml`.
const LEGACY_GOVERNANCE_SELECTION_KEY: &str = "doctrine";

/// The four legacy bundle files this migration folds and retires.
pub const LEGACY_BUNDLE_FILENAMES: [&str; 4] = [
    GOVERNANCE_YAML,
    DIRECTIVES_YAML,
    METADATA_YAML,
    REFERENCES_YAML,
];

/// The flat activation-key vocabulary, derived from the one authority
/// (`sk_doctrine::ledger::ledger_keys`): `activated_kinds`, the per-kind allow
/// keys and the two tokenless keys. The deny keys are not activation state a
/// fold relocates.
fn activation_keys() -> Vec<String> {
    // The single vocabulary authority, in Source order: `activated_kinds`, then
    // every per-kind allow key (the artifact-kind taxonomy order), then the two
    // non-kind allow keys the ledger module declares.
    let mut keys: Vec<String> = Vec::new();
    for key in std::iter::once("activated_kinds".to_string())
        .chain(
            sk_doctrine::activatable_kinds()
                .into_iter()
                .filter_map(|kind| sk_doctrine::allow_key(kind).ok()),
        )
        .chain(
            sk_doctrine::non_kind_allow_keys()
                .into_iter()
                .map(str::to_owned),
        )
    {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

fn charter_dir(project_root: &Path) -> PathBuf {
    project_root.join(KITTIFY_DIRNAME).join(CHARTER_DIRNAME)
}

fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(KITTIFY_DIRNAME).join(CONFIG_FILENAME)
}

fn charter_yaml_path(project_root: &Path) -> PathBuf {
    charter_dir(project_root).join(CHARTER_YAML)
}

/// Safe-load a YAML mapping file; absent/empty/non-mapping is an empty mapping.
fn load_mapping(path: &Path) -> Mapping {
    let Some(text) = read_text(path) else {
        return Mapping::new();
    };
    match serde_yaml_ng::from_str::<Value>(&text) {
        Ok(Value::Mapping(mapping)) => mapping,
        _ => Mapping::new(),
    }
}

/// Whether ANY of the four legacy bundle files exist on disk.
pub fn legacy_bundle_present(project_root: &Path) -> bool {
    let directory = charter_dir(project_root);
    LEGACY_BUNDLE_FILENAMES
        .iter()
        .any(|name| directory.join(name).exists())
}

fn config_has_activation(config: &Mapping) -> bool {
    activation_keys()
        .iter()
        .any(|key| config.contains_key(Value::String(key.clone())))
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        other => format!("{other:?}"),
    }
}

/// A typed read of an authored scalar, refusing the shapes the Source models refuse.
fn required(mapping: &Mapping, key: &str, what: &str) -> Result<String, InstallError> {
    match mapping.get(Value::String(key.to_string())) {
        Some(Value::String(text)) => Ok(text.clone()),
        None | Some(Value::Null) => Err(InstallError::UpgradeRefused {
            detail: format!("{what}: field {key:?} is required"),
        }),
        Some(_) => Err(InstallError::UpgradeRefused {
            detail: format!("{what}: field {key:?} must be a string"),
        }),
    }
}

/// The pydantic-v2 coercion rules the fold's models apply, per field.
///
/// Measured against the pin (`charter.activation.schemas`, pydantic 2.13):
/// an INTEGRAL float coerces into an int field (`2.0` -> 2) while a fractional
/// one is REFUSED (`int_from_float`); `"true"`/`"True"`/`"yes"`/`1`/`0` coerce
/// into a bool and `2`/`null` are refused; an int/bool/float is NEVER coerced
/// into a `str`; a scalar is never coerced into `list[str]` and a non-string list
/// item is refused; `null` for a non-optional field is refused. A refusal is
/// collected and turned into the migration's failure (the Source lets the
/// `ValidationError` propagate, so a real run writes nothing either way).
#[derive(Default)]
struct Coercions {
    error: Option<String>,
}

impl Coercions {
    fn fail(&mut self, key: &str, detail: &str) {
        if self.error.is_none() {
            self.error = Some(format!("{key}: {detail}"));
        }
    }

    fn finish(self) -> Result<(), InstallError> {
        match self.error {
            Some(detail) => Err(InstallError::UpgradeRefused {
                detail: format!("charter bundle validation failed ({detail})"),
            }),
            None => Ok(()),
        }
    }

    /// `int` field: absent -> default; integral number/numeric string/bool coerce.
    fn integer(&mut self, mapping: &Mapping, key: &str, default: i64) -> Value {
        let fallback = Value::Number(default.into());
        let Some(value) = mapping.get(Value::String(key.to_string())) else {
            return fallback;
        };
        match value {
            Value::Bool(flag) => Value::Number(i64::from(*flag).into()),
            Value::Number(number) => {
                if let Some(integer) = number.as_i64() {
                    return Value::Number(integer.into());
                }
                match number.as_f64() {
                    Some(float) if float.is_finite() && float.fract() == 0.0 => {
                        Value::Number((float as i64).into())
                    }
                    _ => {
                        self.fail(
                            key,
                            "input should be a valid integer, got a number with a fractional part",
                        );
                        fallback
                    }
                }
            }
            Value::String(text) => match text.trim().parse::<i64>() {
                Ok(integer) => Value::Number(integer.into()),
                Err(_) => {
                    self.fail(key, "input should be a valid integer");
                    fallback
                }
            },
            _ => {
                self.fail(key, "input should be a valid integer");
                fallback
            }
        }
    }

    /// `bool` field: absent -> default; the Source's lax bool literals coerce.
    fn boolean(&mut self, mapping: &Mapping, key: &str, default: bool) -> Value {
        let fallback = Value::Bool(default);
        let Some(value) = mapping.get(Value::String(key.to_string())) else {
            return fallback;
        };
        match value {
            Value::Bool(flag) => Value::Bool(*flag),
            Value::Number(number) => match number.as_i64() {
                Some(0) => Value::Bool(false),
                Some(1) => Value::Bool(true),
                _ => {
                    self.fail(key, "input should be a valid boolean");
                    fallback
                }
            },
            Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "1" => Value::Bool(true),
                "false" | "no" | "0" => Value::Bool(false),
                _ => {
                    self.fail(key, "input should be a valid boolean");
                    fallback
                }
            },
            _ => {
                self.fail(key, "input should be a valid boolean");
                fallback
            }
        }
    }

    /// `float` field: absent -> default; an int or numeric string coerces.
    fn float(&mut self, mapping: &Mapping, key: &str, default: f64) -> Value {
        let fallback = Value::Number(serde_yaml_ng::Number::from(default));
        let Some(value) = mapping.get(Value::String(key.to_string())) else {
            return fallback;
        };
        match value {
            Value::Bool(flag) => {
                Value::Number(serde_yaml_ng::Number::from(f64::from(u8::from(*flag))))
            }
            Value::Number(number) => match number.as_f64() {
                Some(float) => Value::Number(serde_yaml_ng::Number::from(float)),
                None => {
                    self.fail(key, "input should be a valid number");
                    fallback
                }
            },
            Value::String(text) => match text.trim().parse::<f64>() {
                Ok(float) => Value::Number(serde_yaml_ng::Number::from(float)),
                Err(_) => {
                    self.fail(key, "input should be a valid number");
                    fallback
                }
            },
            _ => {
                self.fail(key, "input should be a valid number");
                fallback
            }
        }
    }

    /// `str | None` field: absent/`null` -> `None`; a non-string is refused.
    fn optional_string(&mut self, mapping: &Mapping, key: &str) -> Option<String> {
        match mapping.get(Value::String(key.to_string())) {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => {
                self.fail(key, "input should be a valid string");
                None
            }
        }
    }

    /// `list[str]` field: absent -> empty; a non-list or non-string item is refused.
    fn string_list(&mut self, mapping: &Mapping, key: &str) -> Vec<Value> {
        match mapping.get(Value::String(key.to_string())) {
            None => Vec::new(),
            Some(Value::Sequence(items)) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        Value::String(text) => out.push(Value::String(text.clone())),
                        _ => {
                            self.fail(key, "input should be a valid string");
                            return Vec::new();
                        }
                    }
                }
                out
            }
            Some(_) => {
                self.fail(key, "input should be a valid list");
                Vec::new()
            }
        }
    }
}

/// `str(...)` over an authored scalar — the CATALOG block's own coercion rule.
fn text_of(mapping: &Mapping, key: &str) -> Option<String> {
    match mapping.get(Value::String(key.to_string())) {
        None | Some(Value::Null) => None,
        Some(value) => Some(scalar_text(value)),
    }
}

fn section(mapping: &Mapping, key: &str) -> Mapping {
    match mapping.get(Value::String(key.to_string())) {
        Some(Value::Mapping(inner)) => inner.clone(),
        _ => Mapping::new(),
    }
}

fn insert(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_string()), value);
}

/// The `governance` block, field order and defaults as the Source model dumps it.
fn governance_document(authored: &Mapping) -> Result<Mapping, InstallError> {
    let mut c = Coercions::default();
    let testing = section(authored, "testing");
    let mut out_testing = Mapping::new();
    insert(
        &mut out_testing,
        "min_coverage",
        c.integer(&testing, "min_coverage", 0),
    );
    insert(
        &mut out_testing,
        "tdd_required",
        c.boolean(&testing, "tdd_required", false),
    );
    insert(
        &mut out_testing,
        "framework",
        Value::String(c.optional_string(&testing, "framework").unwrap_or_default()),
    );
    insert(
        &mut out_testing,
        "type_checking",
        Value::String(
            c.optional_string(&testing, "type_checking")
                .unwrap_or_default(),
        ),
    );

    let quality = section(authored, "quality");
    let mut out_quality = Mapping::new();
    insert(
        &mut out_quality,
        "linting",
        Value::String(c.optional_string(&quality, "linting").unwrap_or_default()),
    );
    insert(
        &mut out_quality,
        "pr_approvals",
        c.integer(&quality, "pr_approvals", 1),
    );
    insert(
        &mut out_quality,
        "pre_commit_hooks",
        c.boolean(&quality, "pre_commit_hooks", false),
    );

    let commits = section(authored, "commits");
    let mut out_commits = Mapping::new();
    if let Some(convention) = c.optional_string(&commits, "convention") {
        insert(&mut out_commits, "convention", Value::String(convention));
    }

    let performance = section(authored, "performance");
    let mut out_performance = Mapping::new();
    insert(
        &mut out_performance,
        "cli_timeout_seconds",
        c.float(&performance, "cli_timeout_seconds", 2.0),
    );
    insert(
        &mut out_performance,
        "dashboard_max_wps",
        c.integer(&performance, "dashboard_max_wps", 100),
    );

    let branch = section(authored, "branch_strategy");
    let mut out_branch = Mapping::new();
    insert(
        &mut out_branch,
        "main_branch",
        Value::String(
            c.optional_string(&branch, "main_branch")
                .unwrap_or_else(|| "main".to_string()),
        ),
    );
    if let Some(dev_branch) = c.optional_string(&branch, "dev_branch") {
        insert(&mut out_branch, "dev_branch", Value::String(dev_branch));
    }
    insert(
        &mut out_branch,
        "rules",
        Value::Sequence(c.string_list(&branch, "rules")),
    );

    let charter = section(authored, "charter");
    let mut out_charter = Mapping::new();
    for key in [
        "selected_paradigms",
        "selected_directives",
        "selected_tactics",
        "selected_styleguides",
        "selected_toolguides",
        "selected_procedures",
        "selected_agent_profiles",
        "selected_mission_step_contracts",
        "selected_glossary_packs",
        "selected_assets",
    ] {
        insert(
            &mut out_charter,
            key,
            Value::Sequence(c.string_list(&charter, key)),
        );
    }
    insert(
        &mut out_charter,
        "available_tools",
        Value::Sequence(c.string_list(&charter, "available_tools")),
    );
    if let Some(template_set) = c.optional_string(&charter, "template_set") {
        insert(
            &mut out_charter,
            "template_set",
            Value::String(template_set),
        );
    }
    insert(
        &mut out_charter,
        "authority_paths",
        Value::Sequence(c.string_list(&charter, "authority_paths")),
    );
    insert(
        &mut out_charter,
        "governance_references",
        Value::Sequence(c.string_list(&charter, "governance_references")),
    );

    c.finish()?;
    let mut out = Mapping::new();
    insert(&mut out, "testing", Value::Mapping(out_testing));
    insert(&mut out, "quality", Value::Mapping(out_quality));
    insert(&mut out, "commits", Value::Mapping(out_commits));
    insert(&mut out, "performance", Value::Mapping(out_performance));
    insert(&mut out, "branch_strategy", Value::Mapping(out_branch));
    insert(&mut out, "charter", Value::Mapping(out_charter));
    insert(
        &mut out,
        "activations",
        Value::Sequence(
            match authored.get(Value::String("activations".to_string())) {
                Some(Value::Sequence(entries)) => entries
                    .iter()
                    .filter_map(|entry| {
                        let entry = entry.as_mapping()?;
                        let mut item = Mapping::new();
                        for key in ["context", "charter_pack_id", "artifact_id", "artifact_kind"] {
                            insert(
                                &mut item,
                                key,
                                Value::String(required(entry, key, "governance.activations").ok()?),
                            );
                        }
                        Some(Value::Mapping(item))
                    })
                    .collect(),
                _ => Vec::new(),
            },
        ),
    );
    if let Some(Value::Mapping(retrospective)) =
        authored.get(Value::String("retrospective".to_string()))
    {
        insert(
            &mut out,
            "retrospective",
            Value::Mapping(retrospective.clone()),
        );
    }
    insert(
        &mut out,
        "enforcement",
        Value::Mapping(section(authored, "enforcement")),
    );
    Ok(out)
}

/// The `directives` block (nested `directives:` list).
fn directives_document(authored: &Mapping) -> Result<Mapping, InstallError> {
    let mut c = Coercions::default();
    let entries = match authored.get(Value::String("directives".to_string())) {
        Some(Value::Sequence(entries)) => entries.clone(),
        None | Some(Value::Null) => Vec::new(),
        Some(_) => {
            return Err(InstallError::UpgradeRefused {
                detail: "directives.yaml: directives must be a list".to_string(),
            });
        }
    };
    let mut items = Vec::new();
    for entry in &entries {
        let Some(entry) = entry.as_mapping() else {
            return Err(InstallError::UpgradeRefused {
                detail: "directives.yaml: each directive must be a mapping".to_string(),
            });
        };
        let mut item = Mapping::new();
        insert(
            &mut item,
            "id",
            Value::String(required(entry, "id", "directives")?),
        );
        insert(
            &mut item,
            "title",
            Value::String(required(entry, "title", "directives")?),
        );
        insert(
            &mut item,
            "description",
            Value::String(c.optional_string(entry, "description").unwrap_or_default()),
        );
        insert(
            &mut item,
            "severity",
            Value::String(
                c.optional_string(entry, "severity")
                    .unwrap_or_else(|| "warn".to_string()),
            ),
        );
        insert(
            &mut item,
            "references",
            Value::Sequence(c.string_list(entry, "references")),
        );
        items.push(Value::Mapping(item));
    }
    c.finish()?;
    let mut out = Mapping::new();
    insert(&mut out, "directives", Value::Sequence(items));
    Ok(out)
}

/// The `catalog` block derived from the retired `references.yaml`.
fn catalog_document(authored: &Mapping) -> Result<Mapping, InstallError> {
    let mut catalog = Mapping::new();
    insert(
        &mut catalog,
        "mission",
        Value::String(text_of(authored, "mission").unwrap_or_default()),
    );
    insert(
        &mut catalog,
        "template_set",
        Value::String(text_of(authored, "template_set").unwrap_or_default()),
    );
    let languages = match authored.get(Value::String("languages".to_string())) {
        None | Some(Value::Null) => None,
        Some(Value::Sequence(items)) => Some(
            items
                .iter()
                .map(|item| Value::String(scalar_text(item)))
                .collect::<Vec<_>>(),
        ),
        Some(_) => {
            return Err(InstallError::UpgradeRefused {
                detail: "references.yaml: languages must be a list".to_string(),
            });
        }
    };
    if let Some(languages) = languages {
        insert(&mut catalog, "languages", Value::Sequence(languages));
    }
    let references = match authored.get(Value::String("references".to_string())) {
        Some(Value::Sequence(items)) => items.clone(),
        None | Some(Value::Null) => Vec::new(),
        Some(_) => {
            return Err(InstallError::UpgradeRefused {
                detail: "references.yaml: references must be a list".to_string(),
            });
        }
    };
    let mut items = Vec::new();
    for entry in &references {
        let Some(entry) = entry.as_mapping() else {
            return Err(InstallError::UpgradeRefused {
                detail: "references.yaml: each reference must be a mapping".to_string(),
            });
        };
        let mut item = Mapping::new();
        for key in [
            "id",
            "kind",
            "title",
            "summary",
            "source_path",
            "local_path",
        ] {
            insert(
                &mut item,
                key,
                Value::String(required(entry, key, "references")?),
            );
        }
        items.push(Value::Mapping(item));
    }
    insert(&mut catalog, "references", Value::Sequence(items));
    Ok(catalog)
}

/// The composed `charter.yaml` document (MG1 — deterministic, verbatim activation copy).
fn compose_charter_yaml(project_root: &Path, config: &Mapping) -> Result<Mapping, InstallError> {
    let directory = charter_dir(project_root);
    let governance = load_mapping(&directory.join(GOVERNANCE_YAML));
    let directives = load_mapping(&directory.join(DIRECTIVES_YAML));
    let references = load_mapping(&directory.join(REFERENCES_YAML));

    if governance.contains_key(Value::String(LEGACY_GOVERNANCE_SELECTION_KEY.to_string())) {
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                "{} still carries the legacy top-level 'doctrine' key (governance.doctrine); the \
                 charter-pack cutover migration rewrites it to 'charter' and runs first. Re-run \
                 `spec-kitty upgrade` so it can, then retry.",
                directory.join(GOVERNANCE_YAML).display()
            ),
        });
    }

    let mut metadata = Mapping::new();
    insert(
        &mut metadata,
        "generated_at",
        Value::String(sk_install_clock_stamp()),
    );
    insert(
        &mut metadata,
        "bundle_schema_version",
        Value::Number(2.into()),
    );

    let mut document = Mapping::new();
    insert(
        &mut document,
        "schema_version",
        Value::String("2.0.0".to_string()),
    );
    insert(
        &mut document,
        "governance",
        Value::Mapping(governance_document(&governance)?),
    );
    insert(
        &mut document,
        "directives",
        Value::Mapping(directives_document(&directives)?),
    );
    insert(
        &mut document,
        "catalog",
        Value::Mapping(catalog_document(&references)?),
    );
    insert(&mut document, "overrides", Value::Mapping(Mapping::new()));
    insert(&mut document, "metadata", Value::Mapping(metadata));
    for key in activation_keys() {
        if let Some(value) = config.get(Value::String(key.clone())) {
            document.insert(Value::String(key), value.clone());
        }
    }
    Ok(document)
}

/// `kernel.clock.now_utc_stamp()`: UTC, second precision, `Z`.
fn sk_install_clock_stamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The `.kittify/config.yaml` `charter:` pointer value.
fn mint_pointer_value(charter_yaml: &Path, project_root: &Path) -> String {
    match (
        sk_kernel::paths::resolve_path_non_strict(charter_yaml),
        sk_kernel::paths::resolve_path_non_strict(project_root),
    ) {
        (Ok(charter), Ok(root)) => charter
            .strip_prefix(&root)
            .map(sk_kernel::paths::to_posix)
            .unwrap_or_else(|_| sk_kernel::paths::to_posix(&charter)),
        _ => charter_yaml.display().to_string(),
    }
}

fn rewrite_config(project_root: &Path, charter_yaml: &Path) -> Result<bool, InstallError> {
    let path = config_path(project_root);
    let pointer = mint_pointer_value(charter_yaml, project_root);
    // The Source re-dumps the round-trip document with the pointer appended at
    // the end; the one CST editor removes the activation keys in place and
    // appends the pointer, keeping every other authored byte.
    let text = read_text(&path).unwrap_or_default();
    let document = CstDocument::parse(&path, &text)?;
    let root = document.root();
    for key in activation_keys() {
        root.remove(&key);
    }
    let stripped = document.text();
    let editor = crate::yaml_edit::YamlEditor::new(&path, &stripped);
    let outcome = editor.upsert_scalars(
        "",
        &[(
            CHARTER_POINTER_KEY.to_string(),
            crate::yaml_edit::Scalar::Str(pointer),
        )],
        crate::yaml_edit::InsertPosition::Bottom,
    )?;
    // The activation-key removal is already in `stripped`; the pointer upsert may
    // be a no-op when the authored pointer already carries the minted value, and
    // dropping `stripped` there would leave the keys in `config.yaml`.
    let final_text = if outcome.changed {
        outcome.text
    } else {
        stripped
    };
    if final_text == text {
        return Ok(false);
    }
    write_text(&path, &final_text).map(|_| true)
}

fn relocate_activation_onto_existing(
    charter_yaml: &Path,
    config: &Mapping,
) -> Result<(), InstallError> {
    let activation: Vec<(String, Value)> = activation_keys()
        .into_iter()
        .filter_map(|key| {
            config
                .get(Value::String(key.clone()))
                .map(|value| (key, value.clone()))
        })
        .collect();
    if activation.is_empty() {
        return Ok(());
    }
    let Some(text) = read_text(charter_yaml) else {
        return Ok(());
    };
    let document = CstDocument::parse(charter_yaml, &text)?;
    let root = document.root();
    for (key, value) in &activation {
        let raw = String::from_utf8(canonical_yaml(value)).unwrap_or_default();
        root.set_raw(key, raw.trim());
    }
    write_text(charter_yaml, &document.text()).map(|_| ())
}

/// Archive-then-remove a legacy bundle file whose bytes the new `charter.yaml`
/// does not provably capture.
fn retire_legacy_file(project_root: &Path, path: &Path) -> Result<Option<String>, InstallError> {
    let verdict = guard_destructive_removal(
        path,
        project_root,
        &Prover::CanonicalContent,
        false,
        Some(&project_root.join(KITTIFY_DIRNAME)),
        false,
    )?;
    Ok(match verdict {
        RemovalVerdict::Absent => None,
        RemovalVerdict::Owned { .. } => Some(format!("Deleted {}", path.display())),
        RemovalVerdict::Archived { path: backup, .. } => Some(format!(
            "Preserved divergent {} to {}, then retired it",
            path.display(),
            backup.display()
        )),
        RemovalVerdict::Preserved { .. } => Some(format!(
            "Kept {} (not provably captured by charter.yaml)",
            path.display()
        )),
    })
}

/// `consolidate_charter_bundle_fold.detect`.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    let config = load_mapping(&config_path(project_root));
    Ok(legacy_bundle_present(project_root) || config_has_activation(&config))
}

/// `consolidate_charter_bundle_fold.can_apply`.
pub fn can_apply(project_root: &Path) -> Result<Option<String>, InstallError> {
    if detect(project_root)? {
        return Ok(None);
    }
    Ok(Some(
        "charter.yaml already composed; no legacy bundle file or config-embedded activation left to migrate"
            .to_string(),
    ))
}

/// The reference's `LegacyGovernanceSelectionKeyError` message, when a
/// standalone `governance.yaml` still carries the legacy `doctrine` key.
fn legacy_governance_selection_key(project_root: &Path) -> Option<String> {
    let path = charter_dir(project_root).join(GOVERNANCE_YAML);
    let governance = load_mapping(&path);
    governance
        .contains_key(Value::String(LEGACY_GOVERNANCE_SELECTION_KEY.to_string()))
        .then(|| {
            format!(
                "{} still carries the legacy top-level 'doctrine' key (governance.doctrine); the \
                 charter-pack cutover migration rewrites it to 'charter' and runs first. Re-run \
                 `spec-kitty upgrade` so it can, then retry.",
                path.display()
            )
        })
}

/// `consolidate_charter_bundle_fold.apply`.
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let directory = charter_dir(project_root);
    let charter_yaml = charter_yaml_path(project_root);
    let config = load_mapping(&config_path(project_root));
    let legacy = legacy_bundle_present(project_root);
    let has_activation = config_has_activation(&config);
    if !legacy && !has_activation {
        return Ok(MigrationOutcome::skipped());
    }

    let mut changes = Vec::new();
    if charter_yaml.exists() {
        if has_activation {
            relocate_activation_onto_existing(&charter_yaml, &config)?;
            changes.push(format!(
                "Relocated activation keys onto existing {}",
                charter_yaml.display()
            ));
        }
    } else {
        // A legacy `governance.doctrine` selection key is the reference's CAUGHT
        // `LegacyGovernanceSelectionKeyError`: `success = false` carrying its text,
        // with nothing written. Every other composition failure (a schema drift the
        // pydantic models refuse) stays a raised error, as in the reference.
        if let Some(detail) = legacy_governance_selection_key(project_root) {
            return Ok(MigrationOutcome {
                changed: false,
                changes: Vec::new(),
                success: false,
                errors: vec![detail],
                ..MigrationOutcome::default()
            });
        }
        let document = compose_charter_yaml(project_root, &config)?;
        write_bytes(&charter_yaml, &ordered_yaml(&Value::Mapping(document)))?;
        changes.push(format!(
            "Composed {} from legacy bundle + config activation",
            charter_yaml.display()
        ));
    }

    for name in LEGACY_BUNDLE_FILENAMES {
        if let Some(line) = retire_legacy_file(project_root, &directory.join(name))? {
            changes.push(line);
        }
    }

    let pointer_present = config.contains_key(Value::String(CHARTER_POINTER_KEY.to_string()));
    if has_activation || !pointer_present {
        rewrite_config(project_root, &charter_yaml)?;
        changes.push(
            "Updated .kittify/config.yaml: removed activated_* keys, added charter: pointer"
                .to_string(),
        );
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// `consolidate_charter_bundle_fold.apply(dry_run=True)`.
///
/// The reference's own `_dry_run_summary`: the same premises `apply` reads
/// (legacy bundle presence, whether `charter.yaml` already exists, which bundle
/// files exist, whether activation is still embedded in `config.yaml`), reported
/// as `dry-run: would ...` lines. Nothing is composed, written, retired or
/// re-pointed, and the `governance.doctrine` refusal premise is not consulted —
/// the reference returns its summary before that check.
pub fn preview(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let directory = charter_dir(project_root);
    let charter_yaml = charter_yaml_path(project_root);
    let config = load_mapping(&config_path(project_root));
    let legacy = legacy_bundle_present(project_root);
    let has_activation = config_has_activation(&config);

    let mut summary: Vec<String> = Vec::new();
    if legacy {
        if charter_yaml.exists() {
            summary.push(format!(
                "dry-run: would delete stale legacy files under {}",
                directory.display()
            ));
        } else {
            summary.push(format!(
                "dry-run: would compose {} from legacy bundle files",
                charter_yaml.display()
            ));
        }
        for name in LEGACY_BUNDLE_FILENAMES {
            let path = directory.join(name);
            if path.exists() {
                summary.push(format!("dry-run: would delete {}", path.display()));
            }
        }
    }
    if has_activation {
        summary.push(
            "dry-run: would relocate activated_* keys from config.yaml onto charter.yaml and add the charter: pointer"
                .to_string(),
        );
    }
    Ok(MigrationOutcome {
        changed: !summary.is_empty(),
        changes: summary,
        warnings: Vec::new(),
        ..MigrationOutcome::default()
    })
}

/// The fold migration this module implements.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[NativeMigration {
    id: "consolidate_charter_bundle_fold",
    detect,
    can_apply,
    apply,
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_present_activation_keys_are_transplanted() {
        let mut config = Mapping::new();
        config.insert(
            Value::String("activated_kinds".to_string()),
            Value::Sequence(Vec::new()),
        );
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(charter_dir(root)).unwrap();
        let document = compose_charter_yaml(root, &config).unwrap();
        assert_eq!(
            document.get(Value::String("activated_kinds".to_string())),
            Some(&Value::Sequence(Vec::new()))
        );
        assert!(!document.contains_key(Value::String("activated_directives".to_string())));
    }

    #[test]
    fn a_legacy_governance_doctrine_key_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(charter_dir(root)).unwrap();
        std::fs::write(
            charter_dir(root).join(GOVERNANCE_YAML),
            "doctrine:\n  selected_directives: []\n",
        )
        .unwrap();
        assert!(compose_charter_yaml(root, &Mapping::new()).is_err());
    }
}
