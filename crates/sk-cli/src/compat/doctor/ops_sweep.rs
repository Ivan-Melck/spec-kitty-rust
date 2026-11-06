//! `doctor ops --close-stale` — the sweep adapter (Source `_run_ops_sweep`).
//!
//! The sweep itself is the canonical Invocation domain
//! ([`sk_invocation::ops_sweep`]): this module only supplies the clock, prints
//! Source's JSON / human surface and returns Source's exit code
//! (`contracts/doctor-ops-close-stale.md`).
//!
//! The `DoctorCommand::Ops { close_stale: true, .. }` arm calls [`run_sweep`]
//! after root resolution, without falling through to the read-only listing.
//! This wiring is reachability, not whole-command fidelity approval; the CF22
//! report retains the propagation, race, error-text and platform gaps.

use std::path::Path;

use sk_invocation::ops_sweep::{self, DEFAULT_SWEEP_THRESHOLD_HOURS, OpsSweepError};

use super::super::support::CompatError;

/// Run `doctor ops --close-stale [--threshold HOURS] [--json]`.
///
/// Source resolves the project root (`exit 1` when it is not a repository)
/// **before** the sweep, so the caller passes the already-resolved root; the
/// `--threshold`-without-`--close-stale` usage error stays in the caller too,
/// where Source raises `typer.BadParameter`.
///
/// Returns Source's exit code: `1` when a per-Op failure was recorded or an
/// open-but-fresh Op remains, else `0`. A guard-refused spine commit is *not* a
/// failure; it is reported per entry and the closure stays durable on disk.
pub fn run_sweep(
    repo_root: &Path,
    threshold: Option<f64>,
    json_output: bool,
) -> Result<u8, CompatError> {
    let threshold_hours = threshold.unwrap_or(DEFAULT_SWEEP_THRESHOLD_HOURS);
    let report =
        ops_sweep::close_stale_ops(repo_root, threshold_hours, sk_kernel::clock::now_utc())
            .map_err(map_error)?;
    if json_output {
        // Source prints `json.dumps(report.to_dict(), indent=2)`; the Kernel
        // encoder reproduces that text, including `NaN`/`Infinity`.
        println!("{}", report.json_text());
    } else {
        for line in report.render_lines() {
            println!("{line}");
        }
    }
    Ok(report.exit_code())
}

/// A whole-sweep refusal (unreadable Op store or closure spine) as a typed
/// operator error. Per-Op failures are report entries, never errors.
fn map_error(error: OpsSweepError) -> CompatError {
    CompatError::coded(error.code(), error.to_string())
}
