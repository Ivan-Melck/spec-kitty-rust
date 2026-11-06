//! Configured retrospective policy — Source `specify_cli/retrospective/policy.py`.
//!
//! This is the resolver the post-merge retrospective capture calls, on the
//! Source path, to turn three durable inputs into one effective policy plus the
//! per-field **source map** the record carries:
//!
//! 1. `.kittify/charter/charter.yaml` `governance.retrospective:` — the
//!    deterministic AUTHORITY (wins for every key it authors);
//! 2. `.kittify/charter/charter.md` YAML frontmatter `retrospective:` — a
//!    readable but OVERRIDDEN secondary, still resolving for keys the authority
//!    does not claim (and the only charter source on legacy md-only projects);
//! 3. `.kittify/config.yaml` `retrospective:` — applied only where no charter
//!    layer claimed the field, unless a charter layer sets
//!    `precedence: config` (then config fills first and charter still wins its
//!    own explicit fields);
//! 4. the built-in defaults, which are the ONLY source of `<default>` in the map.
//!
//! The deprecated `SPEC_KITTY_RETROSPECTIVE` / `SPEC_KITTY_MODE` environment
//! variables are OBSERVED, never authoritative: when the corresponding field
//! came only from `<default>`, the map records `<env:VAR>`. (Source additionally
//! emits a one-per-process Rich stderr notice; that is a display concern of the
//! Python runtime and is returned here as [`ResolvedRetrospectivePolicy::warnings`]
//! text instead of rendered output.)
//!
//! Error shape matches Source's `PolicyResolutionError`: a `source` (the
//! repo-relative file), a machine-readable `reason` and a human `detail`.
//! Source's malformed-input *classification* is ported field for field; the
//! pydantic/ruamel message wording inside `detail` is not, because it is a
//! parser's prose, not a spec-kitty fact.
//!
//! **One deliberate structural difference, stated:** this resolver reads YAML
//! through `serde_yaml_ng` (the crate's existing YAML codec), not ruamel. The
//! mapping/typing rules below are Source's; a YAML document that ruamel accepts
//! and the codec here rejects (or the reverse) is a codec difference, handled as
//! the same `invalid_yaml` classification with a different `detail` string.

use std::collections::BTreeMap;
use std::path::Path;

use serde_yaml_ng::Value;

/// The readable charter companion (Source `charter.bundle.CHARTER_MD`).
pub const CHARTER_MD: &str = ".kittify/charter/charter.md";
/// The charter authority document (Source `charter.bundle.CHARTER_YAML`).
pub const CHARTER_YAML: &str = ".kittify/charter/charter.yaml";
/// The project configuration file.
pub const CONFIG_REL: &str = ".kittify/config.yaml";
/// The authority block path inside `charter.yaml`.
pub const YAML_BLOCK_PATH: &str = "governance.retrospective";

const VALID_TIMING: [&str; 2] = ["post_completion", "before_completion"];
const VALID_FAILURE_POLICY: [&str; 2] = ["warn", "block"];
const VALID_APPLY_PROPOSALS: [&str; 2] = ["require_human", "low_risk_auto"];
const VALID_PRECEDENCE: [&str; 2] = ["charter", "config"];
const VALID_GENERATOR: [&str; 1] = ["python"];

/// Every top-level key a `retrospective:` block may carry, in Source order.
pub const KNOWN_KEYS: [&str; 10] = [
    "enabled",
    "timing",
    "failure_policy",
    "write_record",
    "generate_proposals",
    "apply_proposals",
    "permissions",
    "precedence",
    "generator",
    "strict_keys",
];

/// Every known permission sub-key, in Source order.
pub const KNOWN_PERMISSION_KEYS: [&str; 7] = [
    "write_record",
    "inspect_mission_artifacts",
    "propose_glossary_changes",
    "propose_drg_changes",
    "propose_doctrine_changes",
    "apply_low_risk_changes",
    "apply_structural_changes",
];

/// The `precedence` member of every leaf source-map entry.
pub const POLICY_KEYS: [&str; 15] = [
    "enabled",
    "timing",
    "failure_policy",
    "write_record",
    "generate_proposals",
    "apply_proposals",
    "permissions.write_record",
    "permissions.inspect_mission_artifacts",
    "permissions.propose_glossary_changes",
    "permissions.propose_drg_changes",
    "permissions.propose_doctrine_changes",
    "permissions.apply_low_risk_changes",
    "permissions.apply_structural_changes",
    "precedence",
    "generator",
];

/// Granular retrospective capability flags (Source `RetrospectivePermissions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrospectivePermissions {
    /// Write the record.
    pub write_record: bool,
    /// Inspect mission artifacts.
    pub inspect_mission_artifacts: bool,
    /// Propose glossary changes.
    pub propose_glossary_changes: bool,
    /// Propose DRG changes.
    pub propose_drg_changes: bool,
    /// Propose doctrine changes.
    pub propose_doctrine_changes: bool,
    /// Apply low-risk changes.
    pub apply_low_risk_changes: bool,
    /// Apply structural changes (never `true` in the built-in default).
    pub apply_structural_changes: bool,
}

impl Default for RetrospectivePermissions {
    fn default() -> Self {
        Self {
            write_record: true,
            inspect_mission_artifacts: true,
            propose_glossary_changes: true,
            propose_drg_changes: true,
            propose_doctrine_changes: true,
            apply_low_risk_changes: false,
            apply_structural_changes: false,
        }
    }
}

impl RetrospectivePermissions {
    fn field_mut(&mut self, key: &str) -> Option<&mut bool> {
        Some(match key {
            "write_record" => &mut self.write_record,
            "inspect_mission_artifacts" => &mut self.inspect_mission_artifacts,
            "propose_glossary_changes" => &mut self.propose_glossary_changes,
            "propose_drg_changes" => &mut self.propose_drg_changes,
            "propose_doctrine_changes" => &mut self.propose_doctrine_changes,
            "apply_low_risk_changes" => &mut self.apply_low_risk_changes,
            "apply_structural_changes" => &mut self.apply_structural_changes,
            _ => return None,
        })
    }
}

/// The resolved, runtime-effective policy (Source `RetrospectivePolicy`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrospectivePolicy {
    /// Whether a retrospective is captured at all.
    pub enabled: bool,
    /// `post_completion` | `before_completion`.
    pub timing: String,
    /// `warn` | `block`.
    pub failure_policy: String,
    /// Whether the record is written.
    pub write_record: bool,
    /// Whether proposals are generated (Source's generator consults this).
    pub generate_proposals: bool,
    /// `require_human` | `low_risk_auto`.
    pub apply_proposals: String,
    /// The capability flags.
    pub permissions: RetrospectivePermissions,
    /// `charter` | `config` | `None` (not authored).
    pub precedence: Option<String>,
    /// `python`.
    pub generator: String,
}

impl Default for RetrospectivePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            timing: "post_completion".to_owned(),
            failure_policy: "warn".to_owned(),
            write_record: true,
            generate_proposals: true,
            apply_proposals: "require_human".to_owned(),
            permissions: RetrospectivePermissions::default(),
            precedence: None,
            generator: "python".to_owned(),
        }
    }
}

/// Source `PolicyResolutionError`: where the malformed input was, its
/// machine-readable reason, and a human detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyResolutionError {
    /// The repo-relative file the malformed input came from.
    pub source: String,
    /// The Source reason code (`invalid_yaml`, `invalid_enum`, `invalid_type`,
    /// `invalid_type_for_retrospective_block`, `unknown_key`).
    pub reason: &'static str,
    /// Human-readable elaboration.
    pub detail: String,
}

impl std::fmt::Display for PolicyResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "[{}] in {}: {}",
            self.reason, self.source, self.detail
        )
    }
}

impl std::error::Error for PolicyResolutionError {}

/// The resolved policy plus everything the caller needs to record it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRetrospectivePolicy {
    /// The effective policy values.
    pub policy: RetrospectivePolicy,
    /// Every leaf field's authoritative source string, in [`POLICY_KEYS`] order.
    pub source_map: BTreeMap<String, String>,
    /// Deprecation notices Source would render (the env-var observations).
    pub warnings: Vec<String>,
}

/// The `<default>` source map: every leaf key unconfigured.
pub fn default_source_map() -> BTreeMap<String, String> {
    POLICY_KEYS
        .iter()
        .map(|key| ((*key).to_owned(), "<default>".to_owned()))
        .collect()
}

fn enum_values(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "timing" => Some(&VALID_TIMING),
        "failure_policy" => Some(&VALID_FAILURE_POLICY),
        "apply_proposals" => Some(&VALID_APPLY_PROPOSALS),
        "precedence" => Some(&VALID_PRECEDENCE),
        "generator" => Some(&VALID_GENERATOR),
        _ => None,
    }
}

/// One parsed input block and its source-attribution prefix.
struct Layer {
    block: BTreeMap<String, Value>,
    prefix: String,
}

fn mapping(value: &Value) -> Option<&serde_yaml_ng::Mapping> {
    value.as_mapping()
}

fn key_text(key: &Value) -> Option<String> {
    match key {
        Value::String(text) => Some(text.clone()),
        other => other.as_str().map(str::to_owned),
    }
}

fn mapping_to_block(mapping: &serde_yaml_ng::Mapping) -> BTreeMap<String, Value> {
    let mut block = BTreeMap::new();
    for (key, value) in mapping {
        if let Some(key) = key_text(key) {
            block.insert(key, value.clone());
        }
    }
    block
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Sequence(_) => "list",
        Value::Mapping(_) => "dict",
        Value::Tagged(_) => "tagged",
    }
}

fn parse_yaml(text: &str) -> Result<Option<Value>, String> {
    let trimmed = text.trim_start_matches('\u{feff}');
    if trimmed.trim().is_empty() {
        return Ok(None);
    }
    serde_yaml_ng::from_str::<Value>(trimmed)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Read one file as UTF-8, mapping every read/decode failure to `invalid_yaml`.
fn read_text(path: &Path, source: &str) -> Result<Option<String>, PolicyResolutionError> {
    if !path.exists() {
        return Ok(None);
    }
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(Some(text)),
            Err(error) => Err(PolicyResolutionError {
                source: source.to_owned(),
                reason: "invalid_yaml",
                detail: error.to_string(),
            }),
        },
        Err(error) => Err(PolicyResolutionError {
            source: source.to_owned(),
            reason: "invalid_yaml",
            detail: error.to_string(),
        }),
    }
}

fn invalid_type_block(source: &str, detail: String) -> PolicyResolutionError {
    PolicyResolutionError {
        source: source.to_owned(),
        reason: "invalid_type_for_retrospective_block",
        detail,
    }
}

/// Source `_load_charter_yaml_retrospective_block`, including the #3163
/// "malformed but provably unrelated" fall-through.
fn load_charter_yaml_block(
    root: &Path,
) -> Result<Option<BTreeMap<String, Value>>, PolicyResolutionError> {
    let path = root.join(CHARTER_YAML);
    let Some(text) = read_text(&path, CHARTER_YAML)? else {
        return Ok(None);
    };
    let document = match parse_yaml(&text) {
        Ok(document) => document,
        Err(detail) => {
            if charter_yaml_mentions_retrospective(&path) {
                return Err(PolicyResolutionError {
                    source: CHARTER_YAML.to_owned(),
                    reason: "invalid_yaml",
                    detail,
                });
            }
            return Ok(None);
        }
    };
    let Some(document) = document else {
        return Ok(None);
    };
    let Some(document) = mapping(&document) else {
        return Err(invalid_type_block(
            CHARTER_YAML,
            format!(
                "charter.yaml root is not a YAML mapping; got {}.",
                type_name(&document)
            ),
        ));
    };
    let Some(governance) = document.get("governance") else {
        return Ok(None);
    };
    if governance.is_null() {
        return Ok(None);
    }
    let Some(governance) = mapping(governance) else {
        return Err(invalid_type_block(
            CHARTER_YAML,
            format!(
                "charter.yaml 'governance:' value must be a mapping; got {}.",
                type_name(governance)
            ),
        ));
    };
    let Some(raw_block) = governance.get("retrospective") else {
        return Ok(None);
    };
    if raw_block.is_null() {
        return Ok(None);
    }
    let Some(raw_block) = mapping(raw_block) else {
        return Err(invalid_type_block(
            CHARTER_YAML,
            format!(
                "charter.yaml '{YAML_BLOCK_PATH}:' value must be a mapping; got {}.",
                type_name(raw_block)
            ),
        ));
    };
    let block = mapping_to_block(raw_block);
    authored_keys_only(&block)
}

/// Source `_authored_keys_only`: keep the known, non-null keys the operator
/// actually authored; an empty `permissions:` block claims nothing.
fn authored_keys_only(
    block: &BTreeMap<String, Value>,
) -> Result<Option<BTreeMap<String, Value>>, PolicyResolutionError> {
    let mut authored = BTreeMap::new();
    let mut enum_errors: Vec<String> = Vec::new();
    let mut type_errors: Vec<String> = Vec::new();
    for (key, value) in block {
        // pydantic drops unknown keys (`extra="ignore"`), so the authority block
        // never carries them forward.
        if !KNOWN_KEYS.contains(&key.as_str()) {
            continue;
        }
        if value.is_null() {
            continue;
        }
        if key == "permissions" {
            let Some(permissions) = mapping(value) else {
                type_errors.push(
                    "permissions: Input should be a valid dictionary or instance of \
                     RetrospectiveGovernancePermissions"
                        .to_string(),
                );
                continue;
            };
            let mut authored_permissions = BTreeMap::new();
            for (permission_key, permission_value) in permissions {
                let Some(permission_key) = key_text(permission_key) else {
                    continue;
                };
                if !KNOWN_PERMISSION_KEYS.contains(&permission_key.as_str()) {
                    continue;
                }
                if permission_value.is_null() {
                    continue;
                }
                match permission_value.as_bool() {
                    Some(flag) => {
                        authored_permissions.insert(permission_key, Value::Bool(flag));
                    }
                    None => type_errors.push(format!(
                        "permissions.{permission_key}: Input should be a valid boolean"
                    )),
                }
            }
            if !authored_permissions.is_empty() {
                authored.insert(
                    "permissions".to_owned(),
                    Value::Mapping(
                        authored_permissions
                            .into_iter()
                            .map(|(key, value)| (Value::String(key), value))
                            .collect(),
                    ),
                );
            }
            continue;
        }
        if let Some(values) = enum_values(key) {
            match value.as_str() {
                Some(text) if values.contains(&text) => {
                    authored.insert(key.clone(), value.clone());
                }
                _ => enum_errors.push(format!("{key}: Input should be {}", values.join(" or "))),
            }
            continue;
        }
        // The remaining known keys are booleans (`enabled`, `write_record`,
        // `generate_proposals`, `strict_keys`).
        match value.as_bool() {
            Some(flag) => {
                authored.insert(key.clone(), Value::Bool(flag));
            }
            None => type_errors.push(format!("{key}: Input should be a valid boolean")),
        }
    }
    if !enum_errors.is_empty() {
        return Err(PolicyResolutionError {
            source: CHARTER_YAML.to_owned(),
            reason: "invalid_enum",
            detail: enum_errors.join("; "),
        });
    }
    if !type_errors.is_empty() {
        return Err(PolicyResolutionError {
            source: CHARTER_YAML.to_owned(),
            reason: "invalid_type",
            detail: type_errors.join("; "),
        });
    }
    if authored.is_empty() {
        return Ok(None);
    }
    Ok(Some(authored))
}

/// Source `_charter_yaml_mentions_retrospective`: a fail-closed, byte-level
/// relevance check for a malformed `charter.yaml`.
fn charter_yaml_mentions_retrospective(path: &Path) -> bool {
    let Ok(raw) = std::fs::read(path) else {
        return true;
    };
    let Ok(text) = String::from_utf8(raw) else {
        return true;
    };
    text.replace('\0', "")
        .to_lowercase()
        .contains("retrospective")
}

/// Source `_load_charter_retrospective_block`: the `charter.md` frontmatter
/// `retrospective:` mapping, unvalidated (the applier validates it).
fn load_charter_md_block(
    root: &Path,
) -> Result<Option<BTreeMap<String, Value>>, PolicyResolutionError> {
    let path = root.join(CHARTER_MD);
    let Some(raw) = read_text(&path, CHARTER_MD)? else {
        return Ok(None);
    };
    if !raw.starts_with("---") {
        return Ok(None);
    }
    let lines: Vec<&str> = raw.lines().collect();
    let Some(close) = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(index, _)| index)
    else {
        return Err(PolicyResolutionError {
            source: CHARTER_MD.to_owned(),
            reason: "invalid_yaml",
            detail: format!(
                "Charter at {} has an unclosed YAML frontmatter block.",
                path.display()
            ),
        });
    };
    let frontmatter = lines[1..close].join("\n");
    let document = match parse_yaml(&frontmatter) {
        Ok(document) => document,
        Err(detail) => {
            return Err(PolicyResolutionError {
                source: CHARTER_MD.to_owned(),
                reason: "invalid_yaml",
                detail,
            });
        }
    };
    let Some(document) = document else {
        return Ok(None);
    };
    let Some(document) = mapping(&document) else {
        return Err(invalid_type_block(
            CHARTER_MD,
            format!(
                "Charter frontmatter is not a YAML mapping; got {}.",
                type_name(&document)
            ),
        ));
    };
    let Some(block) = document.get("retrospective") else {
        return Ok(None);
    };
    if block.is_null() {
        return Ok(None);
    }
    let Some(block) = mapping(block) else {
        return Err(invalid_type_block(
            CHARTER_MD,
            format!(
                "Charter frontmatter 'retrospective:' value must be a mapping; got {}.",
                type_name(block)
            ),
        ));
    };
    Ok(Some(mapping_to_block(block)))
}

/// Source `_load_config_retrospective_block`.
fn load_config_block(
    root: &Path,
) -> Result<Option<BTreeMap<String, Value>>, PolicyResolutionError> {
    let path = root.join(CONFIG_REL);
    let Some(raw) = read_text(&path, CONFIG_REL)? else {
        return Ok(None);
    };
    let document = match parse_yaml(&raw) {
        Ok(document) => document,
        Err(detail) => {
            return Err(PolicyResolutionError {
                source: CONFIG_REL.to_owned(),
                reason: "invalid_yaml",
                detail,
            });
        }
    };
    let Some(document) = document else {
        return Ok(None);
    };
    let Some(document) = mapping(&document) else {
        return Err(invalid_type_block(
            CONFIG_REL,
            format!(
                "Config root is not a YAML mapping; got {}.",
                type_name(&document)
            ),
        ));
    };
    let Some(block) = document.get("retrospective") else {
        return Ok(None);
    };
    if block.is_null() {
        return Ok(None);
    }
    let Some(block) = mapping(block) else {
        return Err(invalid_type_block(
            CONFIG_REL,
            format!(
                ".kittify/config.yaml 'retrospective:' value must be a mapping; got {}.",
                type_name(block)
            ),
        ));
    };
    Ok(Some(mapping_to_block(block)))
}

fn validate_enum(key: &str, value: &Value, source: &str) -> Result<(), PolicyResolutionError> {
    let Some(values) = enum_values(key) else {
        return Ok(());
    };
    match value.as_str() {
        Some(text) if values.contains(&text) => Ok(()),
        text => Err(PolicyResolutionError {
            source: source.to_owned(),
            reason: "invalid_enum",
            detail: format!(
                "{key}: got {}, expected one of [{}]",
                match text {
                    Some(text) => format!("'{text}'"),
                    None => "None".to_owned(),
                },
                values
                    .iter()
                    .map(|value| format!("'{value}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

/// Source `_apply_block_to_policy`: apply one block, optionally restricted to
/// the leaf keys a higher-precedence source did not claim.
fn apply_block(
    policy: &mut RetrospectivePolicy,
    source_map: &mut BTreeMap<String, String>,
    block: &BTreeMap<String, Value>,
    prefix: &str,
    strict_keys: bool,
    keys_to_apply: Option<&[String]>,
) -> Result<(), PolicyResolutionError> {
    let mut unknown_keys: Vec<String> = Vec::new();
    for (key, value) in block {
        if key == "strict_keys" {
            continue;
        }
        if !KNOWN_KEYS.contains(&key.as_str()) {
            if strict_keys {
                unknown_keys.push(key.clone());
            }
            continue;
        }
        if let Some(eligible) = keys_to_apply
            && !eligible.iter().any(|candidate| candidate == key)
        {
            continue;
        }
        if key == "permissions" {
            let Some(permissions) = mapping(value) else {
                return Err(invalid_type_block(
                    prefix,
                    format!("'permissions' must be a mapping; got {}.", type_name(value)),
                ));
            };
            apply_permissions_block(
                &mut policy.permissions,
                source_map,
                permissions,
                prefix,
                strict_keys,
                None,
            )?;
            continue;
        }
        validate_enum(key, value, prefix)?;
        set_policy_field(policy, source_map, key, value, prefix);
    }
    if strict_keys && !unknown_keys.is_empty() {
        unknown_keys.sort();
        return Err(PolicyResolutionError {
            source: prefix.to_owned(),
            reason: "unknown_key",
            detail: format!("Unknown keys in retrospective block: {unknown_keys:?}"),
        });
    }
    Ok(())
}

fn apply_permissions_block(
    permissions: &mut RetrospectivePermissions,
    source_map: &mut BTreeMap<String, String>,
    block: &serde_yaml_ng::Mapping,
    prefix: &str,
    strict_keys: bool,
    keys_to_apply: Option<&[String]>,
) -> Result<(), PolicyResolutionError> {
    let mut unknown_keys: Vec<String> = Vec::new();
    for (raw_key, raw_value) in block {
        let Some(key) = key_text(raw_key) else {
            continue;
        };
        if !KNOWN_PERMISSION_KEYS.contains(&key.as_str()) {
            if strict_keys {
                unknown_keys.push(key);
            }
            continue;
        }
        let leaf_key = format!("permissions.{key}");
        if let Some(eligible) = keys_to_apply
            && !eligible.iter().any(|candidate| candidate == &leaf_key)
        {
            continue;
        }
        let Some(flag) = raw_value.as_bool() else {
            return Err(PolicyResolutionError {
                source: prefix.to_owned(),
                reason: "invalid_enum",
                detail: format!(
                    "permissions.{key}: expected boolean, got {}.",
                    type_name(raw_value)
                ),
            });
        };
        if let Some(field) = permissions.field_mut(&key) {
            *field = flag;
        }
        source_map.insert(leaf_key.clone(), format!("{prefix}.{leaf_key}"));
    }
    if strict_keys && !unknown_keys.is_empty() {
        unknown_keys.sort();
        return Err(PolicyResolutionError {
            source: prefix.to_owned(),
            reason: "unknown_key",
            detail: format!("Unknown permission keys in retrospective block: {unknown_keys:?}"),
        });
    }
    Ok(())
}

fn set_policy_field(
    policy: &mut RetrospectivePolicy,
    source_map: &mut BTreeMap<String, String>,
    key: &str,
    value: &Value,
    prefix: &str,
) {
    match key {
        "enabled" => {
            if let Some(flag) = value.as_bool() {
                policy.enabled = flag;
            }
        }
        "write_record" => {
            if let Some(flag) = value.as_bool() {
                policy.write_record = flag;
            }
        }
        "generate_proposals" => {
            if let Some(flag) = value.as_bool() {
                policy.generate_proposals = flag;
            }
        }
        "timing" => {
            if let Some(text) = value.as_str() {
                policy.timing = text.to_owned();
            }
        }
        "failure_policy" => {
            if let Some(text) = value.as_str() {
                policy.failure_policy = text.to_owned();
            }
        }
        "apply_proposals" => {
            if let Some(text) = value.as_str() {
                policy.apply_proposals = text.to_owned();
            }
        }
        "precedence" => {
            if let Some(text) = value.as_str() {
                policy.precedence = Some(text.to_owned());
            }
        }
        "generator" => {
            if let Some(text) = value.as_str() {
                policy.generator = text.to_owned();
            }
        }
        _ => return,
    }
    source_map.insert(key.to_owned(), format!("{prefix}.{key}"));
}

fn top_level_keys_in_block(block: &BTreeMap<String, Value>) -> Vec<String> {
    block
        .keys()
        .filter(|key| key.as_str() != "strict_keys")
        .cloned()
        .collect()
}

fn expand_eligible_to_leaf_keys(top_level: &[String]) -> Vec<String> {
    let mut expanded: Vec<String> = top_level.to_vec();
    if top_level.iter().any(|key| key == "permissions") {
        for key in KNOWN_PERMISSION_KEYS {
            expanded.push(format!("permissions.{key}"));
        }
    }
    expanded
}

fn resolve_strict_keys(layers: &[Layer], config: Option<&BTreeMap<String, Value>>) -> bool {
    // Highest precedence first: charter.yaml authority, charter.md, config.
    let ordered: Vec<&BTreeMap<String, Value>> = layers
        .iter()
        .rev()
        .map(|layer| &layer.block)
        .chain(config)
        .collect();
    for block in ordered {
        if let Some(Value::Bool(flag)) = block.get("strict_keys") {
            return *flag;
        }
    }
    false
}

/// Resolve with an explicit environment (the test seam Source provides with
/// `env=`); production uses [`resolve`].
pub fn resolve_with_env(
    repo_root: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<ResolvedRetrospectivePolicy, PolicyResolutionError> {
    let mut policy = RetrospectivePolicy::default();
    let mut source_map = default_source_map();
    let mut warnings: Vec<String> = Vec::new();

    // 1. Authority: charter.yaml governance.retrospective.
    let yaml_block = load_charter_yaml_block(repo_root)?;
    // 2. Secondary: charter.md frontmatter.
    let md_block = load_charter_md_block(repo_root)?;
    // 3. Config.
    let config_block = load_config_block(repo_root)?;

    let mut layers: Vec<Layer> = Vec::new();
    if let Some(block) = md_block {
        layers.push(Layer {
            block,
            prefix: format!("{CHARTER_MD}:retrospective"),
        });
    }
    if let Some(block) = yaml_block {
        layers.push(Layer {
            block,
            prefix: format!("{CHARTER_YAML}:{YAML_BLOCK_PATH}"),
        });
    }
    let precedence = layers.iter().rev().find_map(|layer| {
        layer
            .block
            .get("precedence")
            .and_then(Value::as_str)
            .filter(|value| VALID_PRECEDENCE.contains(value))
            .map(str::to_owned)
    });
    let strict_keys = resolve_strict_keys(&layers, config_block.as_ref());
    let config_prefix = ".kittify/config.yaml#retrospective";

    let failure = |source_map: &mut BTreeMap<String, String>,
                   error: PolicyResolutionError|
     -> PolicyResolutionError {
        mark_source_map_error(source_map);
        error
    };

    if precedence.as_deref() == Some("config") {
        if let Some(config) = config_block.as_ref()
            && let Err(error) = apply_block(
                &mut policy,
                &mut source_map,
                config,
                config_prefix,
                strict_keys,
                None,
            )
        {
            return Err(failure(&mut source_map, error));
        }
        for layer in &layers {
            if let Err(error) = apply_block(
                &mut policy,
                &mut source_map,
                &layer.block,
                &layer.prefix,
                strict_keys,
                None,
            ) {
                return Err(failure(&mut source_map, error));
            }
        }
    } else {
        for layer in &layers {
            if let Err(error) = apply_block(
                &mut policy,
                &mut source_map,
                &layer.block,
                &layer.prefix,
                strict_keys,
                None,
            ) {
                return Err(failure(&mut source_map, error));
            }
        }
        if let Some(config) = config_block.as_ref() {
            let mut charter_keys: Vec<String> = Vec::new();
            for layer in &layers {
                charter_keys.extend(top_level_keys_in_block(&layer.block));
            }
            let eligible_top: Vec<String> = KNOWN_KEYS
                .iter()
                .filter(|key| **key != "strict_keys")
                .filter(|key| !charter_keys.iter().any(|claimed| claimed == *key))
                .map(|key| (*key).to_owned())
                .collect();
            let eligible = expand_eligible_to_leaf_keys(&eligible_top);
            if let Err(error) = apply_block(
                &mut policy,
                &mut source_map,
                config,
                config_prefix,
                strict_keys,
                Some(&eligible),
            ) {
                return Err(failure(&mut source_map, error));
            }
        }
    }

    // 4. Env observation: recorded, never authoritative.
    let retro_env = environment
        .get("SPEC_KITTY_RETROSPECTIVE")
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    if !retro_env.is_empty() {
        if source_map.get("enabled").map(String::as_str) == Some("<default>") {
            source_map.insert(
                "enabled".to_owned(),
                "<env:SPEC_KITTY_RETROSPECTIVE>".to_owned(),
            );
        }
        warnings.push(env_deprecation_warning(
            "SPEC_KITTY_RETROSPECTIVE",
            "retrospective.enabled",
        ));
    }
    let mode_env = environment
        .get("SPEC_KITTY_MODE")
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    if !mode_env.is_empty() {
        for key in ["timing", "failure_policy"] {
            if source_map.get(key).map(String::as_str) == Some("<default>") {
                source_map.insert(key.to_owned(), "<env:SPEC_KITTY_MODE>".to_owned());
            }
        }
        warnings.push(env_deprecation_warning(
            "SPEC_KITTY_MODE",
            "retrospective.timing + retrospective.failure_policy",
        ));
    }

    Ok(ResolvedRetrospectivePolicy {
        policy,
        source_map,
        warnings,
    })
}

/// Resolve against the process environment.
pub fn resolve(repo_root: &Path) -> Result<ResolvedRetrospectivePolicy, PolicyResolutionError> {
    let environment: BTreeMap<String, String> = std::env::vars().collect();
    resolve_with_env(repo_root, &environment)
}

fn env_deprecation_warning(var_name: &str, replacement_key: &str) -> String {
    format!(
        "{var_name} is a test/dev override only and will be removed. Set durable policy via \
         {replacement_key} in .kittify/config.yaml or charter frontmatter. \
         Docs: docs/guides/use-retrospective-learning.md"
    )
}

/// Source `_mark_source_map_error`: every `<default>` becomes
/// `<resolution_error>` before a malformed input is reported.
pub fn mark_source_map_error(source_map: &mut BTreeMap<String, String>) {
    for value in source_map.values_mut() {
        if value == "<default>" {
            *value = "<resolution_error>".to_owned();
        }
    }
}

/// Source's minimal source map for a resolution failure:
/// only `enabled`/`timing`/`failure_policy`, each `<resolution_error>`.
pub fn resolution_error_source_map() -> BTreeMap<String, String> {
    ["enabled", "timing", "failure_policy"]
        .iter()
        .map(|key| ((*key).to_owned(), "<resolution_error>".to_owned()))
        .collect()
}
