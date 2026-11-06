//! The `meta.json` "defines no contracts" waiver (#5298).
//!
//! The ONE authority Source has for this waiver is a flat `"contracts": "none"`
//! plus a required non-empty string `"contracts_rationale"`, read through the
//! fail-closed `meta.json` reader. Fail-closed means never truthiness-coerced: an
//! unknown or non-string value, an explicit `null`, or a missing/blank/non-string
//! rationale keeps the requirement in force and returns a warning naming the
//! problem. An absent field is simply "no waiver" (silent).
//!
//! A corrupt `meta.json` is a typed read failure, not "absent" — turning it into
//! a silent no-waiver would be exactly the false-acceptance class this authority
//! exists to stop.

use std::path::Path;

use sk_kernel::PythonString;

use crate::definition::error::DefinitionError;

/// The `meta.json` field that carries the waiver.
pub const CONTRACTS_FIELD: &str = "contracts";

/// The only accepted waiver value.
pub const CONTRACTS_WAIVER_VALUE: &str = "none";

/// The rationale field that must accompany a waiver.
pub const CONTRACTS_RATIONALE_FIELD: &str = "contracts_rationale";

/// Source `ContractsWaiver`: `waived` is true only for a well-formed
/// declaration, which then carries its stripped rationale. A present-but
/// malformed declaration is not a waiver and carries a warning instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContractsWaiver {
    /// Whether a well-formed waiver is in force.
    pub waived: bool,
    /// The stripped rationale, when waived.
    pub rationale: Option<PythonString>,
    /// The operator-facing problem, when the declaration was malformed.
    pub warning: Option<PythonString>,
}

fn malformed(problem: &str) -> ContractsWaiver {
    ContractsWaiver {
        waived: false,
        rationale: None,
        warning: Some(PythonString::from_scalars(&format!(
            "meta.json '{CONTRACTS_FIELD}' waiver ignored: {problem}. The contracts/ requirement \
             stays in force; declare \"{CONTRACTS_FIELD}\": \"{CONTRACTS_WAIVER_VALUE}\" with a \
             non-empty '{CONTRACTS_RATIONALE_FIELD}' to waive it."
        ))),
    }
}

/// Read the Mission's waiver from `<primary_meta_dir>/meta.json`.
///
/// `primary_meta_dir` is the Mission's PRIMARY-surface directory. An absent
/// `meta.json` is "no waiver"; a corrupt one is [`DefinitionError::MetaReadFailed`].
pub fn read_contracts_waiver_from_meta(
    primary_meta_dir: &Path,
) -> Result<ContractsWaiver, DefinitionError> {
    let meta_path = primary_meta_dir.join(crate::meta::META_FILE_NAME);
    let exists = match std::fs::symlink_metadata(&meta_path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(DefinitionError::MetaReadFailed {
                path: meta_path,
                detail: error.to_string(),
            });
        }
    };
    if !exists {
        return Ok(ContractsWaiver::default());
    }
    let meta = crate::meta::load_meta_object(&meta_path, crate::meta::DEFAULT_META_BYTE_LIMIT)
        .map_err(|error| DefinitionError::MetaReadFailed {
            path: meta_path.clone(),
            detail: error.to_string(),
        })?;
    let Some(value) = meta.get(CONTRACTS_FIELD) else {
        return Ok(ContractsWaiver::default());
    };
    if value != &serde_json::Value::String(CONTRACTS_WAIVER_VALUE.to_owned()) {
        return Ok(malformed(&format!(
            "value {} is not '{}'",
            python_repr(value),
            CONTRACTS_WAIVER_VALUE
        )));
    }
    match meta.get(CONTRACTS_RATIONALE_FIELD) {
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Ok(ContractsWaiver {
            waived: true,
            rationale: Some(PythonString::from_scalars(text.trim())),
            warning: None,
        }),
        _ => Ok(malformed(&format!(
            "'{CONTRACTS_RATIONALE_FIELD}' must be a non-empty string"
        ))),
    }
}

/// Python `repr()` of a decoded `meta.json` value, for the malformed-waiver
/// message (`{value!r}`).
pub fn python_repr(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_owned(),
        serde_json::Value::Bool(true) => "True".to_owned(),
        serde_json::Value::Bool(false) => "False".to_owned(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => PythonString::from_scalars(text).repr(),
        serde_json::Value::Array(items) => {
            let mut out = String::from("[");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&python_repr(item));
            }
            out.push(']');
            out
        }
        serde_json::Value::Object(entries) => {
            let mut out = String::from("{");
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&PythonString::from_scalars(key).repr());
                out.push_str(": ");
                out.push_str(&python_repr(item));
            }
            out.push('}');
            out
        }
    }
}
