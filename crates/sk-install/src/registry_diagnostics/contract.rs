//! The Contract Registry (`docs/contracts/contract-registry.yaml`).
//!
//! A Contract Record models one shared contract — a `fallback_name` re-export
//! anchored on a dotted symbol, or a `retired_literal` anchored on fixed
//! strings — together with its **declared consumer set**
//! (`consumers.scan_roots`, `consumers.exemptions` and the optional
//! `test_shards` / `call_sites`), its retirement window and its enforcement
//! class.
//!
//! # What is enforcing (v1) and what is not
//!
//! Structural well-formedness is the only enforcing gate: a malformed record,
//! or one that reintroduces positional `file:line` anchoring, fails validation
//! and the command exits 2. The registry's own absence-sweep over
//! `scan_roots` is **advisory** in v1 and is not run here; the counts
//! (`advisory_count` / `enforcing_count`) report the declared enforcement
//! class, not a sweep verdict.
//!
//! A malformed registry never reports advisory health: validation errors are
//! collected first and raised once, and the command exits 2 with every message.
//!
//! # Anchoring (NFR-003 / DIR-041)
//!
//! Anchors are content, never positions. Two shapes are rejected **anywhere**
//! in a record:
//!
//! * a forbidden field name (`file`, `line`, `lineno`, `line_no`, `file_line`,
//!   `fileline`);
//! * a string shaped like `path/to/file.py:42`.
//!
//! A `retired_literal` may store its value as `value:` or as a `fragments:`
//! list (the self-flag defence: a record for a forbidden term must be able to
//! name that term without the manifest tripping the sweep it models). The
//! **joined** value is checked too, so `["src/x.py", ":42"]` cannot smuggle a
//! positional anchor past the per-fragment guard.
//!
//! # Read-only
//!
//! Reads only. No file is created, rewritten or locked.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sk_kernel::PythonString;
use sk_kernel::python_yaml::{GraphDocument, NodeRef};

use super::{
    FORBIDDEN_POSITIONAL_FIELDS, RegistryDiagnosticsError, RegistryKind, is_dotted_identifier,
    is_file_line_anchor, is_release_field, is_tracker_reference, load_document, message_text,
    parse_version, path_text, scalar_text,
};

/// The registry's canonical home inside a project.
pub const CONTRACT_REGISTRY_REL: &str = "docs/contracts/contract-registry.yaml";

/// The two record kinds Source models.
pub const ALLOWED_KINDS: [&str; 2] = ["fallback_name", "retired_literal"];
/// The three lifecycle statuses Source models.
pub const ALLOWED_STATUSES: [&str; 3] = ["active", "deprecated", "retired"];
/// The two enforcement classes Source models.
pub const ALLOWED_ENFORCEMENT: [&str; 2] = ["advisory", "enforcing"];

/// Source's required top-level record keys.
pub const REQUIRED_KEYS: [&str; 9] = [
    "id",
    "kind",
    "anchor",
    "status",
    "owner",
    "replaced_by",
    "retirement",
    "consumers",
    "verification",
];

/// Source's optional top-level record keys.
pub const OPTIONAL_KEYS: [&str; 1] = ["notes"];

/// Source's required `retirement` keys.
pub const RETIREMENT_REQUIRED: [&str; 3] = ["introduced_in", "removal_target", "tracker_issue"];
/// Source's required `consumers` keys.
pub const CONSUMERS_REQUIRED: [&str; 2] = ["scan_roots", "exemptions"];
/// Source's optional `consumers` keys.
pub const CONSUMERS_OPTIONAL: [&str; 2] = ["test_shards", "call_sites"];

const CONTRACT_RELEASE_REASON: &str = "must be a semver string like '3.4.0'";

/// The canonical registry path (returned whether or not it exists).
pub fn resolve_contract_registry_path(root: &Path) -> PathBuf {
    root.join(CONTRACT_REGISTRY_REL)
}

/// The content-addressed identity of a shared contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractAnchor {
    /// The dotted symbol (`kind == "fallback_name"`).
    pub symbol: Option<PythonString>,
    /// The reconstructed fixed strings (`kind == "retired_literal"`).
    pub literals: Vec<PythonString>,
}

/// The retirement window and its tracker reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractRetirement {
    /// The release the contract was introduced in.
    pub introduced_in: PythonString,
    /// The release it must be gone by.
    pub removal_target: PythonString,
    /// The tracker reference (`#123` or a URL).
    pub tracker_issue: PythonString,
}

/// The declared consumer set — the piece the shim registry never had.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractConsumers {
    /// The roots the absence sweep would walk.
    pub scan_roots: Vec<PythonString>,
    /// Path fragments exempt from the sweep.
    pub exemptions: Vec<PythonString>,
    /// Optional test shards that exercise the contract.
    pub test_shards: Vec<PythonString>,
    /// Optional recorded call sites.
    pub call_sites: Vec<PythonString>,
}

/// The declared enforcement class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractVerification {
    /// `"advisory"` or `"enforcing"`.
    pub enforcement: PythonString,
}

/// One validated Contract Record (Source `ContractRecord`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractRecord {
    /// The stable record id.
    pub id: PythonString,
    /// `fallback_name` or `retired_literal`.
    pub kind: PythonString,
    /// The content anchor.
    pub anchor: ContractAnchor,
    /// `active`, `deprecated` or `retired`.
    pub status: PythonString,
    /// The owning tracker reference.
    pub owner: PythonString,
    /// What replaces the contract.
    pub replaced_by: PythonString,
    /// The retirement window.
    pub retirement: ContractRetirement,
    /// The declared consumer set.
    pub consumers: ContractConsumers,
    /// The declared enforcement class.
    pub verification: ContractVerification,
    /// Optional free-form notes (the provenance of a seeded record lives here).
    pub notes: Option<PythonString>,
}

/// Source `ContractRegistryReport` — a validated registry snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractRegistryReport {
    /// Every record, in authored order.
    pub records: Vec<ContractRecord>,
    /// The registry path that was read.
    pub registry_path: PathBuf,
}

impl ContractRegistryReport {
    /// The number of records.
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// How many records declare `enforcement: advisory`.
    pub fn advisory_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| message_text(&record.verification.enforcement) == "advisory")
            .count()
    }

    /// How many records declare `enforcement: enforcing`.
    pub fn enforcing_count(&self) -> usize {
        self.records
            .iter()
            .filter(|record| message_text(&record.verification.enforcement) == "enforcing")
            .count()
    }

    /// The `--json` document, with Source's exact key set and values.
    pub fn to_json(&self) -> Result<Value, RegistryDiagnosticsError> {
        let reason = "a registry record is not scalar text";
        let mut records = Vec::with_capacity(self.records.len());
        for record in &self.records {
            let mut row = Map::new();
            row.insert(
                "id".to_owned(),
                Value::String(scalar_text(&record.id, reason)?),
            );
            row.insert(
                "kind".to_owned(),
                Value::String(scalar_text(&record.kind, reason)?),
            );
            row.insert(
                "status".to_owned(),
                Value::String(scalar_text(&record.status, reason)?),
            );
            row.insert(
                "owner".to_owned(),
                Value::String(scalar_text(&record.owner, reason)?),
            );
            row.insert(
                "replaced_by".to_owned(),
                Value::String(scalar_text(&record.replaced_by, reason)?),
            );
            row.insert(
                "enforcement".to_owned(),
                Value::String(scalar_text(&record.verification.enforcement, reason)?),
            );
            row.insert(
                "scan_roots".to_owned(),
                Value::Array(
                    record
                        .consumers
                        .scan_roots
                        .iter()
                        .map(|root| Ok(Value::String(scalar_text(root, reason)?)))
                        .collect::<Result<Vec<_>, RegistryDiagnosticsError>>()?,
                ),
            );
            records.push(Value::Object(row));
        }
        let mut document = Map::new();
        document.insert(
            "registry_path".to_owned(),
            Value::String(path_text(&self.registry_path)?),
        );
        document.insert("record_count".to_owned(), json!(self.record_count()));
        document.insert("advisory_count".to_owned(), json!(self.advisory_count()));
        document.insert("enforcing_count".to_owned(), json!(self.enforcing_count()));
        document.insert("records".to_owned(), Value::Array(records));
        document.insert("exit_code".to_owned(), json!(0));
        Ok(Value::Object(document))
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load, validate and project the registry (Source `load_registry`).
pub fn load_contract_registry(
    root: &Path,
) -> Result<Vec<ContractRecord>, RegistryDiagnosticsError> {
    let path = resolve_contract_registry_path(root);
    let document = load_document(RegistryKind::Contract, &path)?;
    validate_contract_registry(&document)
}

/// Load + validate and return the report (Source `check_contract_registry`).
pub fn check_contract_registry(
    root: &Path,
) -> Result<ContractRegistryReport, RegistryDiagnosticsError> {
    let registry_path = resolve_contract_registry_path(root);
    let records = load_contract_registry(root)?;
    Ok(ContractRegistryReport {
        records,
        registry_path,
    })
}

// ---------------------------------------------------------------------------
// Validation (Source `contracts/registry.py`, order and wording preserved)
// ---------------------------------------------------------------------------

/// Validate the whole document, collecting every error before failing.
pub fn validate_contract_registry(
    document: &GraphDocument,
) -> Result<Vec<ContractRecord>, RegistryDiagnosticsError> {
    let root = document.root();
    if root.as_mapping().is_none() {
        return Err(RegistryKind::Contract.schema_failure(vec![MISSING_CONTAINER.to_owned()]));
    }
    let Some(contracts) = root.lookup("contracts") else {
        return Err(RegistryKind::Contract.schema_failure(vec![MISSING_CONTAINER.to_owned()]));
    };
    let Some(items) = contracts.as_sequence() else {
        return Err(RegistryKind::Contract
            .schema_failure(vec!["top-level.contracts: must be a list".to_owned()]));
    };

    let mut errors = Vec::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    let mut records = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(record) =
            validate_entry(index, contracts.view(*item), &mut seen_ids, &mut errors)
        {
            records.push(record);
        }
    }
    if errors.is_empty() {
        Ok(records)
    } else {
        Err(RegistryKind::Contract.schema_failure(errors))
    }
}

const MISSING_CONTAINER: &str = "top-level: must be a mapping with a 'contracts' key";

fn is_known_key(name: &str) -> bool {
    REQUIRED_KEYS.contains(&name) || OPTIONAL_KEYS.contains(&name)
}

fn string_keys(node: NodeRef<'_>) -> BTreeSet<String> {
    node.as_mapping()
        .unwrap_or_default()
        .iter()
        .filter_map(|(key, _)| node.view(*key).as_str().map(message_text))
        .collect()
}

fn non_string_keys(node: NodeRef<'_>) -> Vec<String> {
    node.as_mapping()
        .unwrap_or_default()
        .iter()
        .filter(|(key, _)| node.view(*key).as_str().is_none())
        .map(|(key, _)| node.view(*key).repr())
        .collect()
}

/// Reject any positional `file:line` anchor anywhere inside `node`.
fn reject_positional_anchors(prefix: &str, node: NodeRef<'_>, errors: &mut Vec<String>) {
    if let Some(entries) = node.as_mapping() {
        for (key, value) in entries {
            let key = node.view(*key);
            let name = match key.as_str() {
                Some(text) => message_text(text),
                None => key.repr(),
            };
            if key.as_str().is_some_and(|text| {
                FORBIDDEN_POSITIONAL_FIELDS.contains(&message_text(text).as_str())
            }) {
                errors.push(format!(
                    "{prefix}.{name}: positional 'file:line' anchoring is forbidden \
                     (NFR-003/DIR-041) — anchor on a dotted symbol or a fixed literal"
                ));
            }
            reject_positional_anchors(&format!("{prefix}.{name}"), node.view(*value), errors);
        }
        return;
    }
    if let Some(items) = node.as_sequence() {
        for (index, item) in items.iter().enumerate() {
            reject_positional_anchors(&format!("{prefix}[{index}]"), node.view(*item), errors);
        }
        return;
    }
    if let Some(text) = node.as_str()
        && is_file_line_anchor(text)
    {
        errors.push(format!(
            "{prefix}: value {} is a positional 'file:line' anchor, which is forbidden \
             (NFR-003/DIR-041) — anchor on content, never a line number",
            text.repr()
        ));
    }
}

fn validate_literal_entry(prefix: &str, node: NodeRef<'_>, errors: &mut Vec<String>) {
    if node.as_mapping().is_none() {
        errors.push(format!(
            "{prefix}: each literal must be a mapping with 'value' or 'fragments'"
        ));
        return;
    }
    let has_value = node.lookup("value").is_some();
    let has_fragments = node.lookup("fragments").is_some();
    if has_value == has_fragments {
        errors.push(format!(
            "{prefix}: exactly one of 'value' or 'fragments' is required"
        ));
        return;
    }
    if has_value {
        match node.lookup("value").and_then(NodeRef::as_str) {
            Some(text) if !text.is_empty() => {}
            _ => errors.push(format!("{prefix}.value: must be a non-empty string")),
        }
        return;
    }
    let Some(fragments) = node.lookup("fragments") else {
        return;
    };
    let Some(items) = fragments.as_sequence() else {
        errors.push(format!(
            "{prefix}.fragments: must be a non-empty list of strings"
        ));
        return;
    };
    if items.is_empty() {
        errors.push(format!(
            "{prefix}.fragments: must be a non-empty list of strings"
        ));
        return;
    }
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        match fragments.view(*item).as_str() {
            Some(text) => parts.push(text.clone()),
            None => {
                errors.push(format!(
                    "{prefix}.fragments: every fragment must be a string"
                ));
                return;
            }
        }
    }
    let mut joined = PythonString::default();
    for part in &parts {
        joined.append(part);
    }
    if joined.is_empty() {
        errors.push(format!(
            "{prefix}.fragments: joined value must be non-empty"
        ));
    } else if is_file_line_anchor(&joined) {
        // Review gap preserved from Source (WP01-review): `["src/foo.py", ":42"]`
        // reconstructs a positional anchor past the per-fragment guard.
        errors.push(format!(
            "{prefix}.fragments: joined value {} is a positional 'file:line' anchor, which is \
             forbidden (NFR-003/DIR-041) — fragments must not reconstruct a line number",
            joined.repr()
        ));
    }
}

fn validate_anchor(
    index: usize,
    kind: Option<&PythonString>,
    node: Option<NodeRef<'_>>,
    errors: &mut Vec<String>,
) {
    let prefix = format!("contracts[{index}].anchor");
    let Some(node) = node else {
        return;
    };
    if node.as_mapping().is_none() {
        errors.push(format!("{prefix}: must be a mapping"));
        return;
    }
    let kind = kind.map(message_text);
    if kind.as_deref() == Some("fallback_name") {
        match node.lookup("symbol").and_then(NodeRef::as_str) {
            Some(text) if is_dotted_identifier(text) => {}
            _ => errors.push(format!(
                "{prefix}.symbol: must be a dotted identifier for kind 'fallback_name'"
            )),
        }
        if node.lookup("literals").is_some() {
            errors.push(format!(
                "{prefix}: kind 'fallback_name' uses 'symbol', not 'literals'"
            ));
        }
    } else if kind.as_deref() == Some("retired_literal") {
        match node.lookup("literals") {
            Some(literals) => match literals.as_sequence() {
                Some(items) if !items.is_empty() => {
                    for (position, item) in items.iter().enumerate() {
                        validate_literal_entry(
                            &format!("{prefix}.literals[{position}]"),
                            literals.view(*item),
                            errors,
                        );
                    }
                }
                _ => errors.push(format!(
                    "{prefix}.literals: must be a non-empty list for kind 'retired_literal'"
                )),
            },
            None => errors.push(format!(
                "{prefix}.literals: must be a non-empty list for kind 'retired_literal'"
            )),
        }
        if node.lookup("symbol").is_some() {
            errors.push(format!(
                "{prefix}: kind 'retired_literal' uses 'literals', not 'symbol'"
            ));
        }
    }
}

fn validate_retirement(index: usize, node: Option<NodeRef<'_>>, errors: &mut Vec<String>) {
    let prefix = format!("contracts[{index}].retirement");
    let Some(node) = node else {
        return;
    };
    if node.as_mapping().is_none() {
        errors.push(format!("{prefix}: must be a mapping"));
        return;
    }
    let names = string_keys(node);
    let missing: Vec<&str> = RETIREMENT_REQUIRED
        .into_iter()
        .filter(|key| !names.contains(*key))
        .collect();
    for key in &missing {
        errors.push(format!("{prefix}.{key}: required field is missing"));
    }

    let mut versions: [Option<PythonString>; 2] = [None, None];
    for (slot, key) in ["introduced_in", "removal_target"].into_iter().enumerate() {
        let Some(value) = node.lookup(key) else {
            continue;
        };
        match value.as_str() {
            Some(text) if is_release_field(text) => versions[slot] = Some(text.clone()),
            _ => errors.push(format!("{prefix}.{key}: {CONTRACT_RELEASE_REASON}")),
        }
    }
    if let (Some(introduced), Some(removal)) = (&versions[0], &versions[1]) {
        match (
            parse_version(&message_text(introduced)),
            parse_version(&message_text(removal)),
        ) {
            (Some(introduced), Some(removal)) => {
                if removal < introduced {
                    errors.push(format!("{prefix}.removal_target: must be >= introduced_in"));
                }
            }
            _ => errors.push(format!("{prefix}: version strings are not valid semver")),
        }
    }

    match node.lookup("tracker_issue").and_then(NodeRef::as_str) {
        Some(text) if is_tracker_reference(text) => {}
        _ => errors.push(format!("{prefix}.tracker_issue: must be '#123' or a URL")),
    }
}

fn validate_str_list(
    prefix: &str,
    node: Option<NodeRef<'_>>,
    allow_empty: bool,
    errors: &mut Vec<String>,
) {
    let Some(node) = node else {
        return;
    };
    let Some(items) = node.as_sequence() else {
        errors.push(format!("{prefix}: must be a list of strings"));
        return;
    };
    if items.is_empty() && !allow_empty {
        errors.push(format!("{prefix}: must not be empty"));
    }
    if items.iter().any(|item| node.view(*item).as_str().is_none()) {
        errors.push(format!("{prefix}: every item must be a string"));
    }
}

fn validate_consumers(index: usize, node: Option<NodeRef<'_>>, errors: &mut Vec<String>) {
    let prefix = format!("contracts[{index}].consumers");
    let Some(node) = node else {
        return;
    };
    if node.as_mapping().is_none() {
        errors.push(format!("{prefix}: must be a mapping"));
        return;
    }
    let names = string_keys(node);
    let known: BTreeSet<&str> = CONSUMERS_REQUIRED
        .into_iter()
        .chain(CONSUMERS_OPTIONAL)
        .collect();
    for name in names.iter().filter(|name| !known.contains(name.as_str())) {
        errors.push(format!("{prefix}.{name}: unknown field"));
    }
    let missing: Vec<&str> = CONSUMERS_REQUIRED
        .into_iter()
        .filter(|key| !names.contains(*key))
        .collect();
    for key in &missing {
        errors.push(format!("{prefix}.{key}: required field is missing"));
    }
    validate_str_list(
        &format!("{prefix}.scan_roots"),
        node.lookup("scan_roots"),
        false,
        errors,
    );
    validate_str_list(
        &format!("{prefix}.exemptions"),
        node.lookup("exemptions"),
        true,
        errors,
    );
    for optional in CONSUMERS_OPTIONAL {
        validate_str_list(
            &format!("{prefix}.{optional}"),
            node.lookup(optional),
            true,
            errors,
        );
    }
}

fn validate_verification(index: usize, node: Option<NodeRef<'_>>, errors: &mut Vec<String>) {
    let prefix = format!("contracts[{index}].verification");
    let Some(node) = node else {
        return;
    };
    if node.as_mapping().is_none() {
        errors.push(format!("{prefix}: must be a mapping"));
        return;
    }
    match node.lookup("enforcement").and_then(NodeRef::as_str) {
        Some(text) if ALLOWED_ENFORCEMENT.contains(&message_text(text).as_str()) => {}
        _ => errors.push(format!(
            "{prefix}.enforcement: must be one of {}",
            python_list_repr(&ALLOWED_ENFORCEMENT)
        )),
    }
}

/// Source's `sorted(...)` rendering of an allowed-value set inside a message.
fn python_list_repr(values: &[&str]) -> String {
    let items: Vec<String> = values.iter().map(|value| format!("'{value}'")).collect();
    format!("[{}]", items.join(", "))
}

fn validate_scalar_fields(
    index: usize,
    node: NodeRef<'_>,
    seen_ids: &mut BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    let prefix = format!("contracts[{index}]");
    match node.lookup("id").and_then(NodeRef::as_str) {
        Some(text) if !text.is_empty() => {
            if !seen_ids.insert(message_text(text)) {
                errors.push(format!(
                    "{prefix}.id: duplicate value '{}'",
                    message_text(text)
                ));
            }
        }
        _ => errors.push(format!("{prefix}.id: must be a non-empty string")),
    }

    if node
        .lookup("kind")
        .and_then(NodeRef::as_str)
        .is_none_or(|text| !ALLOWED_KINDS.contains(&message_text(text).as_str()))
    {
        errors.push(format!(
            "{prefix}.kind: must be one of {}",
            python_list_repr(&ALLOWED_KINDS)
        ));
    }
    if node
        .lookup("status")
        .and_then(NodeRef::as_str)
        .is_none_or(|text| !ALLOWED_STATUSES.contains(&message_text(text).as_str()))
    {
        errors.push(format!(
            "{prefix}.status: must be one of {}",
            python_list_repr(&ALLOWED_STATUSES)
        ));
    }
    for field in ["owner", "replaced_by"] {
        match node.lookup(field).and_then(NodeRef::as_str) {
            Some(text) if !text.is_empty() => {}
            _ => errors.push(format!("{prefix}.{field}: must be a non-empty string")),
        }
    }
    if let Some(notes) = node.lookup("notes")
        && !notes.is_null()
        && notes.as_str().is_none()
    {
        errors.push(format!("{prefix}.notes: if present, must be a string"));
    }
}

fn validate_entry(
    index: usize,
    node: NodeRef<'_>,
    seen_ids: &mut BTreeSet<String>,
    errors: &mut Vec<String>,
) -> Option<ContractRecord> {
    if node.as_mapping().is_none() {
        errors.push(format!("contracts[{index}]: must be a mapping"));
        return None;
    }

    for key in non_string_keys(node) {
        errors.push(format!("contracts[{index}].{key}: unknown field"));
    }
    let names = string_keys(node);
    for name in names.iter().filter(|name| !is_known_key(name)) {
        errors.push(format!("contracts[{index}].{name}: unknown field"));
    }
    let missing: Vec<&str> = REQUIRED_KEYS
        .into_iter()
        .filter(|key| !names.contains(*key))
        .collect();
    for key in &missing {
        errors.push(format!(
            "contracts[{index}].{key}: required field is missing"
        ));
    }

    // The DIR-041 guard runs over whatever IS present, regardless of missing keys.
    reject_positional_anchors(&format!("contracts[{index}]"), node, errors);
    if !missing.is_empty() {
        return None;
    }

    validate_scalar_fields(index, node, seen_ids, errors);
    let kind = node.lookup("kind").and_then(NodeRef::as_str);
    validate_anchor(index, kind, node.lookup("anchor"), errors);
    validate_retirement(index, node.lookup("retirement"), errors);
    validate_consumers(index, node.lookup("consumers"), errors);
    validate_verification(index, node.lookup("verification"), errors);

    build_record(node)
}

/// Project a validated record into its typed form.
///
/// Only reached after validation passed; a field that is nevertheless absent
/// yields `None` rather than a panic or an invented default.
fn build_record(node: NodeRef<'_>) -> Option<ContractRecord> {
    let anchor = node.lookup("anchor")?;
    let retirement = node.lookup("retirement")?;
    let consumers = node.lookup("consumers")?;
    let verification = node.lookup("verification")?;

    let literals = match anchor.lookup("literals").and_then(NodeRef::as_sequence) {
        Some(items) => {
            let list = anchor.lookup("literals")?;
            let mut literals = Vec::with_capacity(items.len());
            for item in items {
                literals.push(reconstruct_literal(list.view(*item))?);
            }
            literals
        }
        None => Vec::new(),
    };

    Some(ContractRecord {
        id: node.lookup("id")?.as_str()?.clone(),
        kind: node.lookup("kind")?.as_str()?.clone(),
        anchor: ContractAnchor {
            symbol: anchor.lookup("symbol").and_then(NodeRef::as_str).cloned(),
            literals,
        },
        status: node.lookup("status")?.as_str()?.clone(),
        owner: node.lookup("owner")?.as_str()?.clone(),
        replaced_by: node.lookup("replaced_by")?.as_str()?.clone(),
        retirement: ContractRetirement {
            introduced_in: retirement.lookup("introduced_in")?.as_str()?.clone(),
            removal_target: retirement.lookup("removal_target")?.as_str()?.clone(),
            tracker_issue: retirement.lookup("tracker_issue")?.as_str()?.clone(),
        },
        consumers: ContractConsumers {
            scan_roots: string_list(consumers.lookup("scan_roots")?),
            exemptions: string_list(consumers.lookup("exemptions")?),
            test_shards: consumers
                .lookup("test_shards")
                .map(string_list)
                .unwrap_or_default(),
            call_sites: consumers
                .lookup("call_sites")
                .map(string_list)
                .unwrap_or_default(),
        },
        verification: ContractVerification {
            enforcement: verification.lookup("enforcement")?.as_str()?.clone(),
        },
        notes: node.lookup("notes").and_then(NodeRef::as_str).cloned(),
    })
}

fn string_list(node: NodeRef<'_>) -> Vec<PythonString> {
    node.as_sequence()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| node.view(*item).as_str().cloned())
        .collect()
}

/// Join a validated literal entry (`value:` or `fragments:`) back to its string.
fn reconstruct_literal(node: NodeRef<'_>) -> Option<PythonString> {
    if let Some(value) = node.lookup("value").and_then(NodeRef::as_str) {
        return Some(value.clone());
    }
    let fragments = node.lookup("fragments")?;
    let items = fragments.as_sequence()?;
    let mut joined = PythonString::default();
    for item in items {
        let part = fragments.view(*item).as_str()?;
        joined.append(part);
    }
    Some(joined)
}
