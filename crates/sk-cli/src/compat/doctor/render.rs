//! Human-readable rendering for the implemented `doctor` diagnostics.
//!
//! The layout is informational: it carries the same rows, findings, counts and
//! remedies as the JSON payload, but it is **not** byte-pinned to the
//! reference's Rich rendering. Every path and message is printed verbatim (no
//! wrapping), so an operator can grep it.

use std::collections::{BTreeMap, BTreeSet};

use sk_workspace::doctor::{
    DecisionsAudit, IdentityAudit, IdentityState, MissionTypeAudit, MissionTypeState, OpsAudit,
    PairingAudit, StateRootEnvironment, StateRootsAudit, TopologyAudit, TopologyFinding,
};

const RESIDUE_HEADINGS: [(sk_workspace::doctor::ResidueReason, &str); 3] = [
    (sk_workspace::doctor::ResidueReason::NoMeta, "No meta.json:"),
    (
        sk_workspace::doctor::ResidueReason::UntrackedIdlessMeta,
        "Untracked meta.json without a mission_id:",
    ),
    (
        sk_workspace::doctor::ResidueReason::CorruptMeta,
        "Corrupt meta.json (could not be read):",
    ),
];

/// `doctor identity`.
pub fn identity(
    audit: &IdentityAudit,
    fail_on_states: &BTreeSet<IdentityState>,
    triggered: bool,
    fail_on: Option<&str>,
) {
    let counts = audit.counts();
    let residue = counts.get("residue").copied().unwrap_or(0);
    println!(
        "\nMission Identity Audit — {} Mission(s), {residue} residue folder(s)\n",
        audit.rows.len().saturating_sub(residue)
    );

    println!("{:<10} {:>6}", "State", "Count");
    for state in sk_workspace::doctor::IDENTITY_STATES {
        println!(
            "{:<10} {:>6}",
            state.as_str(),
            counts.get(state.as_str()).copied().unwrap_or(0)
        );
    }
    println!();

    if audit.duplicate_prefixes.is_empty() && audit.ambiguous_selectors.is_empty() {
        println!("No duplicate prefixes or ambiguous selectors.\n");
    } else {
        if !audit.duplicate_prefixes.is_empty() {
            println!("Duplicate Prefixes");
            for (prefix, rows) in &audit.duplicate_prefixes {
                println!("  {prefix} — {} collision(s):", rows.len());
                for row in rows {
                    let id = row
                        .mission_id
                        .as_ref()
                        .map(render_value)
                        .unwrap_or_else(|| "no mission_id".to_owned());
                    println!(
                        "    {}  mission_id={id}  state={}",
                        row.slug,
                        row.state.as_str()
                    );
                }
            }
            println!();
        }
        if !audit.ambiguous_selectors.is_empty() {
            println!("Ambiguous Selectors");
            for (handle, rows) in &audit.ambiguous_selectors {
                println!("  {handle:?} → {} candidate(s):", rows.len());
                for row in rows {
                    println!("    {}", row.slug);
                }
            }
            println!();
        }
    }

    print_paths(
        "Legacy missions (need backfill):",
        audit
            .rows
            .iter()
            .filter(|row| row.state == IdentityState::Legacy)
            .map(|row| row.path.to_string_lossy().into_owned())
            .collect(),
    );
    print_paths(
        "Orphan missions (need triage):",
        audit
            .rows
            .iter()
            .filter(|row| row.state == IdentityState::Orphan)
            .map(|row| row.path.to_string_lossy().into_owned())
            .collect(),
    );

    let reasons = audit.residue_reasons();
    if !reasons.is_empty() {
        println!("Residue folders (not Missions; Mission selection does not list them):");
        for (reason, heading) in RESIDUE_HEADINGS {
            if !reasons.contains(&reason) {
                continue;
            }
            println!("  {heading}");
            for row in audit
                .rows
                .iter()
                .filter(|row| row.residue_reason == Some(reason))
            {
                let detail = row
                    .error
                    .as_ref()
                    .map(|error| format!(" — {error}"))
                    .unwrap_or_default();
                println!("    {}{detail}", row.path.to_string_lossy());
            }
            println!("  Remedy: {}", reason.remedy());
        }
        println!();
    }

    if triggered {
        println!(
            "FAIL: --fail-on {fail_on:?} triggered (one or more missions in: {})",
            fail_on_states
                .iter()
                .map(|state| state.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

fn print_paths(heading: &str, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    println!("{heading}");
    for path in paths {
        println!("  {path}");
    }
    println!();
}

/// `doctor topology`.
pub fn topology(audit: &TopologyAudit) {
    println!(
        "\nMission Topology Audit — {} mission(s)\n",
        audit.rows.len()
    );
    println!(
        "{:<32} {:<18} {:<10} Finding",
        "Mission", "Topology", "Flattened"
    );
    for row in &audit.rows {
        let topology = row
            .topology
            .as_ref()
            .map(render_value)
            .unwrap_or_else(|| "null".to_owned());
        let flattened = row.flattened.as_ref().map(render_value).unwrap_or_default();
        let finding = row.finding.map(TopologyFinding::as_str).unwrap_or_default();
        println!(
            "{:<32} {:<18} {:<10} {finding}",
            row.slug, topology, flattened
        );
        if let Some(error) = &row.error {
            println!("    error: {error}");
        }
    }
    for row in &audit.rows {
        if let Some(remedy) = &row.remedy {
            let code = row.finding.map(TopologyFinding::as_str).unwrap_or_default();
            println!("\nRemedy for {} ({code}): {remedy}", row.slug);
        }
    }
}

/// `doctor mission-type`.
pub fn mission_type(
    audit: &MissionTypeAudit,
    fail_on_states: &BTreeSet<MissionTypeState>,
    triggered: bool,
    fail_on: Option<&str>,
) {
    let counts = audit.counts();
    println!("\nMission Type Audit — {} mission(s)\n", audit.rows.len());
    println!("{:<10} {:>6}", "State", "Count");
    for state in sk_workspace::doctor::MISSION_TYPE_STATES {
        println!(
            "{:<24} {:>6}",
            state.as_str(),
            counts.get(state.as_str()).copied().unwrap_or(0)
        );
    }
    println!();

    if triggered {
        println!(
            "FAIL: --fail-on {fail_on:?} triggered (one or more missions in: {})",
            fail_on_states
                .iter()
                .map(|state| state.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// `doctor state-roots`.
pub fn state_roots(audit: &StateRootsAudit, environment: &StateRootEnvironment) {
    println!("\nState Roots");
    for root in &audit.roots {
        let status = if root.exists { "exists" } else { "absent" };
        println!(
            "  {:<20} {}  {status}",
            root.name,
            root.resolved_path.to_string_lossy()
        );
    }

    let mut by_root: BTreeMap<&'static str, Vec<&sk_workspace::doctor::StateSurfaceCheck>> =
        BTreeMap::new();
    for check in &audit.surfaces {
        by_root
            .entry(check.surface.root.as_str())
            .or_default()
            .push(check);
    }

    for (root, label) in [
        ("project", "Project Surfaces (.kittify/)"),
        ("feature", "Feature Surfaces (kitty-specs/)"),
    ] {
        let Some(checks) = by_root.get(root) else {
            continue;
        };
        println!("\n{label}");
        println!(
            "{:<40} {:<16} {:<24} Present",
            "Name", "Authority", "Git Policy"
        );
        for check in checks {
            println!(
                "{:<40} {:<16} {:<24} {}",
                check.surface.name,
                check.surface.authority.as_str(),
                check.surface.git_class.as_str(),
                if check.present { "Y" } else { "N" }
            );
        }
    }
    for (root, label) in [
        (
            "global_runtime",
            format!(
                "Global Runtime ({})",
                environment.kittify_home.to_string_lossy()
            ),
        ),
        (
            "global_sync",
            format!(
                "Global Sync ({})",
                environment.runtime_base.to_string_lossy()
            ),
        ),
        ("git_internal", "Git-Internal (.git/spec-kitty/)".to_owned()),
    ] {
        let Some(checks) = by_root.get(root) else {
            continue;
        };
        println!("\n{label}");
        for check in checks {
            println!(
                "  {:<40} {:<16} {:<24} {}",
                check.surface.name,
                check.surface.authority.as_str(),
                check.surface.git_class.as_str(),
                if check.present { "Y" } else { "N" }
            );
        }
    }

    println!();
    if audit.warnings.is_empty() {
        println!("No warnings -- all runtime surfaces are properly covered.");
    } else {
        println!("Warnings");
        for warning in &audit.warnings {
            println!("  ! {warning}");
        }
    }
    println!();
}

/// `doctor workspaces`.
pub fn workspaces(report: &sk_workspace::doctor::HuskReport) {
    if report.healthy() {
        println!("No workspace husks found.");
        return;
    }
    if report.husks.is_empty() {
        if let Some(error) = &report.registration_error {
            println!("Error: {error}");
        }
        return;
    }
    println!("Workspace husks under {}", report.worktrees_dir);
    for entry in &report.husks {
        let status = match entry.registered {
            None => "unknown registration state (fix refused until git succeeds)",
            Some(true) => "registered (manual repair needed)",
            Some(false) => "unregistered (safe to --fix)",
        };
        println!("  ! {}  {status}", entry.path);
    }
    println!();
    if let Some(error) = &report.registration_error {
        println!("Error: {error}");
    }
    println!(
        "Remove unregistered husks with: spec-kitty doctor workspaces --fix (not implemented in this slice; see the refusal)"
    );
}

/// `doctor decisions`.
///
/// Source's human shape, without the Rich markup: the clean line, the diverged
/// header and its two id lists, then the malformed-fold / stale-status
/// warnings and the one-line disclosure of the findings this slice cannot
/// evaluate.
pub fn decisions(audit: &DecisionsAudit) {
    if audit.clean() {
        println!(
            "ok: {} -- decisions index matches the event log ({} decision(s)).",
            audit.mission_slug,
            audit.log_decision_ids.len()
        );
    } else {
        println!("diverged: {}", audit.mission_slug);
        if !audit.missing_from_index.is_empty() {
            println!(
                "  missing from index ({}): {}",
                audit.missing_from_index.len(),
                audit.missing_from_index.join(", ")
            );
        }
        if !audit.orphaned_in_index.is_empty() {
            println!(
                "  orphaned in index, no backing event ({}): {}",
                audit.orphaned_in_index.len(),
                audit.orphaned_in_index.join(", ")
            );
        }
    }
    if let Some(remedy) = audit.malformed_fold_remedy() {
        println!(
            "  could not fold ({}) decision(s) from a malformed event-log group -- the \
             DecisionPointOpened/Resolved envelopes for these decision_ids do not satisfy the \
             canonical fold's invariants; {remedy} Affected: {}",
            audit.malformed_folds.len(),
            audit.malformed_folds.join(", ")
        );
    }
    if !audit.status_mismatch.is_empty() {
        let detail = audit
            .status_mismatch
            .iter()
            .map(|item| {
                format!(
                    "{} (index={}, log={})",
                    item.decision_id, item.index_status, item.folded_status
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  stale status ({}) index entry disagrees with the folded event log: {detail}",
            audit.status_mismatch.len()
        );
    }
    println!(
        "  note: the two-surface fork report is not implemented, so {} were not evaluated.",
        DecisionsAudit::UNEVALUATED_FINDINGS.join(" / ")
    );
}

/// `doctor invocation-pairing`.
pub fn invocation_pairing(audit: &PairingAudit) {
    let percent = audit.pairing_percent();
    if audit.healthy() {
        println!(
            "Invocation Pairing: no orphan started records (pairing rate: {percent}%, groups: {}).",
            audit.total_groups
        );
        return;
    }

    println!(
        "\nInvocation Pairing — {} orphan started record(s)\n",
        audit.orphan_count()
    );
    println!(
        "{:<40} {:<12} {:<12} {:<8} Started at",
        "Canonical Action ID", "Agent", "Mission ID", "WP"
    );
    for entry in &audit.orphans {
        println!(
            "{:<40} {:<12} {:<12} {:<8} {}",
            entry.canonical_action_id,
            entry.agent,
            entry.mission_id,
            entry.wp_id.as_deref().unwrap_or("-"),
            entry.started_at
        );
    }
    println!(
        "\nPairing rate: {percent}% across {} group(s).",
        audit.total_groups
    );
    println!();
}

/// `doctor ops`.
pub fn ops(audit: &OpsAudit, root: &std::path::Path) {
    if audit.healthy() {
        println!("Ops: no orphan op records found.");
        return;
    }
    println!("\nOps — {} orphan op record(s)\n", audit.open_ops.len());
    for path in &audit.open_ops {
        println!(
            "  {}",
            path.strip_prefix(root)
                .map(|relative| relative.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned())
        );
    }
    println!(
        "\nThese op records were started but never completed. Run spec-kitty doctor ops --json for machine-readable output."
    );
    println!();
}

/// Render a decoded JSON value the way a Python f-string would for the common
/// scalar cases (strings unquoted, everything else compact JSON).
fn render_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => "None".to_owned(),
        serde_json::Value::Bool(true) => "True".to_owned(),
        serde_json::Value::Bool(false) => "False".to_owned(),
        other => other.to_string(),
    }
}
