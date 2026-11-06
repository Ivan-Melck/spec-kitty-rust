//! `doctor shim-registry` and `doctor contracts` — the two registry diagnostics.
//!
//! Both leaves were named refusals before this adapter existed. They now call
//! the read-only diagnostics in [`sk_install::registry_diagnostics`], which own
//! the registry models, the schema rules, the classification and the JSON
//! documents; this module only resolves the project root, renders, and applies
//! the reference's exit contract.
//!
//! # Exit contract (the reference's, preserved)
//!
//! | command | clean / findings | config or schema error | not in a project |
//! |---|---|---|---|
//! | `shim-registry` | 0 / **1** (overdue) | 2 | 2 |
//! | `contracts` | 0 / — (structural validity is the only gate) | 2 | 2 |
//!
//! `shim-registry` is the only one of the two with a finding exit code: an
//! overdue shim means a removal release shipped while the shim file is still on
//! disk, so exit 1 is the gate. `contracts` has no advisory health success on a
//! malformed registry — a schema violation is exit 2 with every message, never
//! exit 0 with a warning.
//!
//! # Human output is informational
//!
//! The reference prints Rich tables. The rows, the summary counts and the
//! overdue remediation block are reproduced here; the box drawing, colour and
//! padding are not byte-pinned (the same disclosure every native `doctor`
//! adapter carries).
//!
//! # Not implemented here
//!
//! The parent owns root dispatch, the `IMPLEMENTED_LEAVES`/`REFUSED_LEAVES`
//! arrays, the module-doc table row and the capability matrix. This module
//! exports the two runners for that wiring and does not touch them. The
//! Contract Registry's advisory absence-sweep over `consumers.scan_roots` is
//! not run anywhere: it is advisory in v1 and stays unimplemented.

use std::path::Path;

use sk_install::registry_diagnostics::contract::{ContractRegistryReport, check_contract_registry};
use sk_install::registry_diagnostics::shim::{ShimRegistryReport, ShimStatus, check_shim_registry};
use sk_install::registry_diagnostics::{RegistryDiagnosticsError, shim};
use sk_kernel::PythonString;

use super::tool_surfaces::NOT_IN_PROJECT_MESSAGE;
use crate::compat::support::{CompatError, emit_json_pretty, project_root};

/// The reference's configuration-error prefix (`doctor shim-registry`).
pub const CONFIGURATION_ERROR_PREFIX: &str = "Configuration error:";
/// The reference's schema-error prefix (`doctor shim-registry`).
pub const SHIM_SCHEMA_ERROR_PREFIX: &str = "Registry schema error:";
/// The reference's schema-error prefix (`doctor contracts`).
pub const CONTRACT_SCHEMA_ERROR_PREFIX: &str = "Contract registry schema error:";

/// Run `doctor shim-registry` (0 clean / 1 overdue / 2 configuration).
pub fn run_shim_registry(json_output: bool) -> Result<u8, CompatError> {
    let root = match project_root() {
        Ok(root) => root,
        Err(_) => return Err(not_in_project(2)),
    };
    run_shim_registry_in(&root, json_output)
}

/// Run `doctor shim-registry` against an explicit project root.
pub fn run_shim_registry_in(root: &Path, json_output: bool) -> Result<u8, CompatError> {
    let report = match check_shim_registry(root) {
        Ok(report) => report,
        Err(error) => return Err(shim_refusal(error)),
    };
    if json_output {
        let document = report.to_json().map_err(shim_refusal)?;
        emit_json_pretty(&document)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
        return Ok(report.recommended_exit_code());
    }
    render_shim_registry(&report);
    Ok(report.recommended_exit_code())
}

/// Run `doctor contracts` (0 well-formed / 2 configuration or schema).
pub fn run_contracts(json_output: bool) -> Result<u8, CompatError> {
    let root = match project_root() {
        Ok(root) => root,
        Err(_) => return Err(not_in_project(2)),
    };
    run_contracts_in(&root, json_output)
}

/// Run `doctor contracts` against an explicit project root.
pub fn run_contracts_in(root: &Path, json_output: bool) -> Result<u8, CompatError> {
    let report = match check_contract_registry(root) {
        Ok(report) => report,
        Err(error) => return Err(contract_refusal(error)),
    };
    if json_output {
        let document = report.to_json().map_err(contract_refusal)?;
        emit_json_pretty(&document)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
        return Ok(0);
    }
    render_contract_registry(&report);
    Ok(0)
}

fn not_in_project(exit: u8) -> CompatError {
    CompatError::envelope_with_exit(
        exit,
        serde_json::json!({
            "ok": false,
            "error": {"code": "not_in_project", "message": NOT_IN_PROJECT_MESSAGE},
        }),
        NOT_IN_PROJECT_MESSAGE,
    )
}

/// Translate a shim-registry failure into the reference's exit-2 contract.
fn shim_refusal(error: RegistryDiagnosticsError) -> CompatError {
    let message = match &error {
        RegistryDiagnosticsError::ShimSchema { .. } => {
            format!("{SHIM_SCHEMA_ERROR_PREFIX}\n{}", error)
        }
        _ => format!("{CONFIGURATION_ERROR_PREFIX} {error}"),
    };
    diagnostic_error(2, error.code(), message)
}

/// Translate a contract-registry failure into the reference's exit-2 contract.
fn contract_refusal(error: RegistryDiagnosticsError) -> CompatError {
    let message = match &error {
        RegistryDiagnosticsError::ContractSchema { .. } => {
            format!("{CONTRACT_SCHEMA_ERROR_PREFIX}\n{}", error)
        }
        _ => format!("{CONFIGURATION_ERROR_PREFIX} {error}"),
    };
    diagnostic_error(2, error.code(), message)
}

/// One emission path for the family's error envelope.
fn diagnostic_error(exit: u8, code: &str, message: impl Into<String>) -> CompatError {
    let message = message.into();
    CompatError::envelope_with_exit(
        exit,
        serde_json::json!({
            "ok": false,
            "error": {"code": code, "message": message},
        }),
        message,
    )
}

fn render_shim_registry(report: &ShimRegistryReport) {
    if report.entries.is_empty() {
        println!("Shim Registry: registry is empty — no shims to check.");
        return;
    }
    println!(
        "\nShim Registry — {} entry/entries (project version: {})\n",
        report.entries.len(),
        report.project_version
    );
    println!(
        "{:<34} {:<40} {:<10} Status",
        "Legacy Path", "Canonical Import", "Removal"
    );
    for entry in &report.entries {
        println!(
            "{:<34} {:<40} {:<10} {}",
            render_value(&entry.entry.legacy_path),
            entry.entry.canonical_import.joined(),
            render_value(&entry.entry.removal_target_release),
            entry.status.as_str(),
        );
    }
    println!();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for status in [
        ShimStatus::Grandfathered,
        ShimStatus::Overdue,
        ShimStatus::Pending,
        ShimStatus::Removed,
    ] {
        let count = report
            .entries
            .iter()
            .filter(|entry| entry.status == status)
            .count();
        if count > 0 {
            counts.push((status.as_str().to_owned(), count));
        }
    }
    println!(
        "Summary: {}",
        counts
            .into_iter()
            .map(|(status, count)| format!("{count} {status}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if report.has_overdue() {
        render_overdue(report);
    }
    println!();
}

/// The reference's overdue remediation block (Option A / Option B).
fn render_overdue(report: &ShimRegistryReport) {
    println!();
    println!("Overdue shims must be resolved before release:");
    for entry in report
        .entries
        .iter()
        .filter(|entry| entry.status == ShimStatus::Overdue)
    {
        println!("\n  {}", render_value(&entry.entry.legacy_path));
        println!(
            "    Canonical import : {}",
            entry.entry.canonical_import.joined()
        );
        println!(
            "    Removal target   : {}",
            render_value(&entry.entry.removal_target_release)
        );
        println!(
            "    Tracker          : {}",
            render_value(&entry.entry.tracker_issue)
        );
        println!("    Remediation:");
        println!(
            "      Option A: Delete src/specify_cli/{}.py (or __init__.py)",
            render_value(&entry.entry.legacy_path).replace('.', "/")
        );
        println!(
            "      Option B: Extend removal_target_release in docs/migrations/shim-registry.yaml \
             with extension_rationale"
        );
    }
}

fn render_contract_registry(report: &ContractRegistryReport) {
    if report.records.is_empty() {
        println!("Contract Registry: registry is empty — nothing to validate.");
        return;
    }
    println!(
        "\nContract Registry — {} record(s) ({} advisory, {} enforcing)\n",
        report.record_count(),
        report.advisory_count(),
        report.enforcing_count()
    );
    println!(
        "{:<42} {:<16} {:<10} {:<10} Owner",
        "ID", "Kind", "Status", "Enforcement"
    );
    for record in &report.records {
        println!(
            "{:<42} {:<16} {:<10} {:<10} {}",
            render_value(&record.id),
            render_value(&record.kind),
            render_value(&record.status),
            render_value(&record.verification.enforcement),
            render_value(&record.owner),
        );
    }
    println!();
    println!("Contract Registry: all records are well-formed.");
    println!();
}

/// Authored code points as printable text, `repr`-escaped when they are not.
fn render_value(value: &PythonString) -> String {
    value.to_scalar_string().unwrap_or_else(|_| value.repr())
}

/// The two refused leaf names this module replaces, for the parent's wiring.
pub const IMPLEMENTED_LEAVES_ADDED: [&str; 2] = ["shim-registry", "contracts"];

/// The shim registry's canonical relative path (re-exported for callers that
/// only need the location, e.g. a remedy line).
pub const SHIM_REGISTRY_REL: &str = shim::SHIM_REGISTRY_REL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_not_in_project_refusal_keeps_the_family_shape() {
        let error = not_in_project(2);
        assert_eq!(error.exit(), 2);
        assert_eq!(error.message(), NOT_IN_PROJECT_MESSAGE);
        assert_eq!(
            error.to_json()["error"]["code"],
            serde_json::json!("not_in_project")
        );
    }

    #[test]
    fn a_schema_failure_carries_the_reference_prefix_and_exit_two() {
        let error = shim_refusal(RegistryDiagnosticsError::ShimSchema {
            errors: vec!["entry[0].legacy_path: required field is missing".to_owned()],
        });
        assert_eq!(error.exit(), 2);
        assert!(error.message().starts_with(SHIM_SCHEMA_ERROR_PREFIX));
        assert!(error.message().contains("entry[0].legacy_path"));

        let error = contract_refusal(RegistryDiagnosticsError::ContractSchema {
            errors: vec!["contracts[0].anchor: must be a mapping".to_owned()],
        });
        assert_eq!(error.exit(), 2);
        assert!(error.message().starts_with(CONTRACT_SCHEMA_ERROR_PREFIX));
    }

    #[test]
    fn a_missing_registry_is_a_configuration_error_not_a_health_pass() {
        let error = shim_refusal(RegistryDiagnosticsError::NotFound {
            label: "Shim registry",
            path: std::path::PathBuf::from("/tmp/project/docs/migrations/shim-registry.yaml"),
        });
        assert_eq!(error.exit(), 2);
        assert!(error.message().starts_with(CONFIGURATION_ERROR_PREFIX));
        assert!(error.message().contains("shim-registry.yaml"));
        assert_eq!(
            error.to_json()["error"]["code"],
            serde_json::json!("registry_not_found")
        );
    }

    #[test]
    fn the_leaf_names_this_adapter_implements_are_the_reference_ones() {
        assert_eq!(IMPLEMENTED_LEAVES_ADDED, ["shim-registry", "contracts"]);
        assert_eq!(SHIM_REGISTRY_REL, "docs/migrations/shim-registry.yaml");
    }
}
