use std::path::Path;

use serde_json::{Map, Value};
use sk_kernel::{KernelError, read_bounded};

use crate::error::MissionError;

/// Directory under a project root that holds Mission specification trees.
pub const SPECS_DIR_NAME: &str = "kitty-specs";

/// File inside each Mission directory that carries machine-facing identity.
pub const META_FILE_NAME: &str = "meta.json";

/// Default byte budget for one `meta.json` read.
///
/// Real `meta.json` files are a few kilobytes; the cap exists so a hostile or
/// accidental huge file is refused instead of loaded.
pub const DEFAULT_META_BYTE_LIMIT: u64 = 256 * 1024;

/// Describe a JSON value's kind for typed error messages.
pub(crate) fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Read `path` within `limit` bytes and decode it as a JSON object.
///
/// Every key present in the file — including fields this crate does not
/// interpret — is preserved in the returned map. The bytes are never
/// reserialized, so the source file keeps its formatting and comments.
pub(crate) fn load_meta_object(
    path: &Path,
    limit: u64,
) -> Result<Map<String, Value>, MissionError> {
    let bytes = read_bounded(path, limit).map_err(|source| match source {
        KernelError::TooLarge { limit, .. } => MissionError::MetadataTooLarge {
            path: path.to_path_buf(),
            limit,
        },
        other => MissionError::kernel(path, other),
    })?;

    let text = std::str::from_utf8(&bytes).map_err(|error| MissionError::MalformedMetadata {
        path: path.to_path_buf(),
        detail: format!("not valid UTF-8: {error}"),
    })?;

    let value: Value =
        serde_json::from_str(text).map_err(|error| MissionError::MalformedMetadata {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;

    match value {
        Value::Object(map) => Ok(map),
        other => Err(MissionError::MetadataNotObject {
            path: path.to_path_buf(),
            kind: json_kind(&other),
        }),
    }
}
