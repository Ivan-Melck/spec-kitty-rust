//! Read-only charter, pack and asset inspection.

use std::path::Path;

use serde_json::json;
use sk_charter::{BuiltInSearch, InspectionOptions, inspect_project};

use crate::commands::project::resolve_root;
use crate::output::{CliError, CommandData};

/// Inspect the project charter surface.
pub fn inspect(explicit: Option<&Path>, built_in: bool) -> Result<CommandData, CliError> {
    let root = resolve_root(explicit)?;
    let mut options = InspectionOptions::new();
    if built_in {
        options = options.with_built_in(BuiltInSearch::from_env(&root));
    }

    let inspection = inspect_project(&root, &options).map_err(|error| {
        CliError::domain(format!(
            "could not inspect the charter surface at {}: {error}",
            root.display()
        ))
    })?;
    let value = serde_json::to_value(&inspection).map_err(|error| {
        CliError::internal(format!(
            "could not serialize the charter inspection: {error}"
        ))
    })?;

    Ok(CommandData::success(json!({
        "built_in_searched": built_in,
        "read_only": true,
        "reserialized": false,
        "inspection": value,
    })))
}
