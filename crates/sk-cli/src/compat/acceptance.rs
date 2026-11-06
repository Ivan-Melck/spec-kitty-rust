//! The Source-compatible `accept` adapter.
//!
//! Source's `accept` is not a single gate: it is a pipeline whose order is part
//! of the contract (`cli/commands/accept.py`):
//!
//! 1. resolve the Mission,
//! 2. validate `--attest-no-implementation` at the CLI boundary (exit 2, nothing
//!    recorded, in a read-only mode or without a reason),
//! 3. verify `--merge-commit` (read-only, BEFORE the remote is contacted),
//! 4. the origin-freshness gate (fetches; writes only tracking refs),
//! 5. record the operator attestations the request validated,
//! 6. refuse an approved work package with no implementation evidence or no
//!    independent review — BEFORE anything is collected or written,
//! 7. collect the feature summary — including the acceptance-matrix step, which
//!    populates review-evidence rows, enforces negative invariants, persists the
//!    matrix under the Mission lock and judges the FRESH matrix,
//! 8. judge the summary, and
//! 9. only then record acceptance: `Accept <slug>` → `Record acceptance commit
//!    for <slug>` → `Finalize acceptance artifacts for <slug>`.
//!
//! This module composes the canonical owners for each step instead of carrying a
//! second acceptance policy:
//!
//! * facts — [`sk_workspace::acceptance_source`] (the one collector),
//! * judgement — [`sk_status::acceptance::source::evaluate_source_acceptance`]
//!   (the one Source-compatible policy; the leaner Native
//!   `evaluate_acceptance_with` is NOT used here and is not an alternate success
//!   path for this command),
//! * evidence and review independence — [`evidence`] (the port of
//!   `consolidation.wp_evidence`),
//! * the operator attestation — [`attest`] (over
//!   `consolidation.no_implementation_attestation`),
//! * matrix mutation/persistence — [`sk_workspace::acceptance_execution`],
//! * metadata/commits — `sk_mission`'s acceptance recorders and the Git safe-commit
//!   seam already used by the other lifecycle adapters.
//!
//! # Ordering that is easy to get wrong
//!
//! * The work-package state (`status.events.jsonl` rows, reduced lanes, review
//!   evidence) is read LAZILY: only once `lanes.json` exists, which is exactly
//!   when Source's `_evidence_lanes_manifest` lets the evidence axis run and when
//!   the matrix step exists at all. A Mission whose `lanes.json` is absent — the
//!   artifact-only (`plan`) family and every Mission before `finalize-tasks` —
//!   never touches the work-package status surface here, so a corrupt WP-only
//!   artifact cannot fail a run Source accepts.
//! * The matrix is only executed once the policy says the lane gates passed
//!   (lanes manifest readable, `meta.json` target agreement, branch allowed).
//!   The first collection therefore runs with [`MatrixInput::NotRequired`] and is
//!   discarded when the gates pass; the authoritative summary is the one
//!   produced after the matrix step. A refused acceptance must never write or
//!   mutate the matrix for a gate that blocked before it.
//! * `--diagnose` performs exactly one read-only collection: the matrix is judged
//!   as it stands, negative invariants are NOT executed, and nothing is written.
//!   An evidence refusal in that mode is reported as a blocked check beside the
//!   rest of the readiness report and exits 1, as Source's
//!   `_report_evidence_diagnosis` does.
//! * Only an accepting run publishes the matrix and the acceptance metadata.
//!   `--no-commit` still resolves the matrix (Source: otherwise the verdict stays
//!   `pending` and the gate can never pass) but creates no commit.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};
use sk_mission::DefinitionProviders;
use sk_mission::MissionTopology;
use sk_status::acceptance::ReviewEvidence;
use sk_status::acceptance::source::{
    AcceptanceDiagnostic, SourceAcceptanceOptions, SourceAcceptanceSummary,
    evaluate_source_acceptance,
};
use sk_workspace::acceptance_execution::{AcceptanceMatrixRequest, execute_acceptance_matrix};
use sk_workspace::acceptance_source::{
    AcceptanceCollectionOptions, InstalledMissionProviders, MatrixInput, RequiredArtifactsOrigin,
    collect_source_acceptance_facts_with_origin,
};
use sk_workspace::workspace::ProjectWorkspace;

use super::lifecycle::{self, AcceptArgs};
use super::support::{self, CompatError, emit_json, path_string};

mod attest;
mod evidence;
mod providers;

use evidence::{EVIDENCE_WINDOW_UNCOMPUTABLE, EvidenceInputs, EvidenceReport, evaluate_evidence};
use providers::{CompositionMissionProviders, RunStoreRuntimeFacts};

/// The required-artifact names Source resolves for `software-dev` when the
/// packaged `expected-artifacts.yaml` is unavailable (an installation defect).
pub const PINNED_REQUIRED_ARTIFACTS: [&str; 3] = ["spec.md", "plan.md", "tasks.md"];

/// The `--diagnose` blocked-check name of the evidence refusal (Source
/// `_EVIDENCE_CHECK`).
const EVIDENCE_CHECK: &str = "implementation_evidence";

/// What a matrix step contributed beyond the policy's own judgement.
struct MatrixContribution {
    /// Where the matrix lives, when the gate resolved a surface.
    directory: Option<String>,
    /// Checks the matrix step itself recorded (cannot-evaluate, lock timeout).
    activity_issues: Vec<String>,
}

impl MatrixContribution {
    fn none() -> Self {
        Self {
            directory: None,
            activity_issues: Vec::new(),
        }
    }
}

/// The work-package-only reads the evidence axis and the matrix step need.
///
/// Read at most once, and only when `lanes.json` exists — the same condition
/// under which Source's evidence preflight and lane gates consult work-package
/// state at all.
struct WpState {
    rows: Vec<Value>,
    lanes: Vec<(String, String)>,
    lanes_by_wp: BTreeMap<String, String>,
    review_evidence: Vec<ReviewEvidence>,
    canceled: BTreeSet<String>,
    is_single_branch: bool,
}

/// `spec-kitty accept`.
pub fn accept(root: &Path, args: &AcceptArgs) -> Result<(Value, u8), CompatError> {
    let handle = lifecycle::require_mission(args.mission.as_deref(), "accept")?;
    let mission = support::resolve_mission(root, Some(&handle))?;

    // Source order #2 (`_validated_no_implementation_request`): the CLI-boundary
    // check runs before any read or write, and a read-only mode never records.
    let read_only_flag = if args.diagnose {
        Some("--diagnose")
    } else if args.no_commit {
        Some("--no-commit")
    } else {
        None
    };
    let attest_request = attest::AttestationRequest::from_flags(
        &args.attest_no_implementation,
        args.attest_reason.as_deref(),
        read_only_flag,
    )?;

    // Source's next two steps, exactly: `_verify_merge_commit` (read-only) runs
    // BEFORE `_check_origin_freshness` (which fetches), and both run before any
    // acceptance read or write.
    let pr_merge = lifecycle::verify_accept_merge_commit(root, &mission, args)?;
    let origin_warnings = lifecycle::accept_origin_gate(root, &mission, args)?;
    // #6780: the landing commit this run verified stands in for a pruned lane
    // branch's window. The resolved full SHA, not the operator's string.
    let landed_commit = pr_merge
        .as_ref()
        .map(|evidence| evidence.pr_merge_commit.to_string());

    // Source order #5/#6. `lanes.json` is the one switch: absent means Source's
    // `_evidence_lanes_manifest` returns `None`, the evidence axis never runs, and
    // no work-package state is read here at all.
    let manifest = read_lanes_manifest(&mission)?;
    let mut state = load_wp_state(&mission, manifest.as_ref())?;
    if !attest_request.is_empty() {
        record_attestations(
            root,
            &mission,
            &attest_request,
            state.as_ref(),
            manifest.as_ref(),
            landed_commit.as_deref(),
        )?;
        // The preflight must read the attestations this run just recorded.
        state = load_wp_state(&mission, manifest.as_ref())?;
    }
    let evidence_refusal = refuse_unevidenced_work(
        root,
        &mission,
        state.as_ref(),
        manifest.as_ref(),
        args.diagnose,
        landed_commit.as_deref(),
    )?;

    let workspace = ProjectWorkspace::open(root)
        .and_then(|project| project.mission(mission.slug().as_str()))
        .map_err(|error| {
            CompatError::coded(
                "ACCEPTANCE_WORKSPACE_UNRESOLVED",
                format!(
                    "the Mission workspace for {} could not be resolved: {error}",
                    mission.slug().as_str()
                ),
            )
        })?;
    let options = SourceAcceptanceOptions {
        lenient: args.lenient,
    };
    let (required_artifacts, required_source) = required_artifact_names(root)?;
    // Source's two cross-surface reads the collector cannot own: the recorded
    // Mission runtime (non-WP acceptance) and the activated sparse mission-type
    // registration. Both are resolved through their canonical owners here, at
    // the composition root, and handed to the collector as plain providers.
    let project_root =
        sk_workspace::acceptance_source::mission_definition::project_root_for_feature_dir(
            mission.spec_dir(),
        )
        .ok()
        .flatten();
    let mission_providers =
        CompositionMissionProviders::resolve(project_root.as_deref(), mission.mission_type())?;
    let runtime_facts = RunStoreRuntimeFacts {
        root,
        mission_id: Some(mission.id().as_str()),
    };
    let collection = AcceptanceCollectionOptions {
        acceptance: options,
        providers: Some(&mission_providers),
        decoder: None,
        required_artifacts: Some(&required_artifacts),
        non_wp_runtime: Some(&runtime_facts),
    };

    // ---- Phase A: do the lane gates pass? --------------------------------
    // The matrix must not be touched for a gate that blocks before it, so the
    // first collection carries no matrix evidence at all. Its summary is
    // authoritative only when the gates stopped; otherwise it is discarded.
    let probe = collect_origin_and_judge(
        root,
        &mission,
        MatrixInput::NotRequired,
        collection,
        options,
        required_source.as_deref(),
    )?;

    if args.diagnose || matrix_gate_stopped(&probe.summary) {
        // The matrix step runs ONLY when the gate really judges a matrix. A
        // `--diagnose` run over a Mission whose gate never runs (a planning-only
        // or non-WP Mission) reads no matrix at all — Source treats diagnose and
        // the gate-skip outcome identically there, because `_check_lane_gates`
        // returns the same skip before `mutate_matrix` is consulted.
        let gate_never_ran = matrix_gate_stopped(&probe.summary);
        let (summary, contribution, disclosure) = if args.diagnose && !gate_never_ran {
            let Some(state) = state.as_ref() else {
                return Err(CompatError::coded(
                    "ACCEPTANCE_MATRIX_UNRESOLVED",
                    "the acceptance matrix cannot be judged without the Mission's lanes.json"
                        .to_owned(),
                ));
            };
            diagnose(
                root,
                &mission,
                &workspace,
                &state.lanes,
                &state.review_evidence,
                collection,
                options,
                required_source.as_deref(),
            )?
        } else {
            (
                probe.summary,
                MatrixContribution::none(),
                probe.required_artifacts_source,
            )
        };
        if let Some(report) = &evidence_refusal {
            return report_evidence_diagnosis(
                args,
                summary,
                report,
                &contribution,
                origin_warnings,
                disclosure,
            );
        }
        return finish(
            root,
            &mission,
            args,
            summary,
            contribution,
            None,
            origin_warnings,
            disclosure,
        );
    }

    let Some(state) = state.as_ref() else {
        return Err(CompatError::coded(
            "ACCEPTANCE_MATRIX_UNRESOLVED",
            "the acceptance matrix cannot be judged without the Mission's lanes.json".to_owned(),
        ));
    };

    // ---- Matrix step: populate, enforce, persist, judge fresh ------------
    let run = execute_acceptance_matrix(&AcceptanceMatrixRequest {
        workspace: &workspace,
        lanes: &state.lanes,
        review_evidence: &state.review_evidence,
        diagnose: false,
        lock_timeout: sk_status::mutation_lock::MISSION_WRITE_LOCK_TIMEOUT,
    })
    .map_err(collection_error)?;
    let contribution = MatrixContribution {
        directory: run.directory.as_ref().map(|dir| dir.display().to_string()),
        activity_issues: run.activity_issues.clone(),
    };

    // ---- Phase B: the authoritative summary over the FRESH matrix --------
    let collected = collect_origin_and_judge(
        root,
        &mission,
        MatrixInput::Computed(&run.evidence),
        collection,
        options,
        required_source.as_deref(),
    )?;
    let mut summary = collected.summary;
    absorb(
        &mut summary,
        &run.blocked_checks,
        &run.skipped_checks,
        &contribution,
    );

    finish(
        root,
        &mission,
        args,
        summary,
        contribution,
        pr_merge,
        origin_warnings,
        collected.required_artifacts_source,
    )
}

/// One collection plus its verdict, keeping the required-artifact provenance.
struct CollectedSummary {
    summary: SourceAcceptanceSummary,
    /// The `required_artifacts_source` disclosure for THIS branch of the run.
    required_artifacts_source: String,
}

/// Collect the Source facts and hand them to the one policy.
fn collect_origin_and_judge(
    root: &Path,
    mission: &sk_mission::Mission,
    matrix: MatrixInput<'_>,
    collection: AcceptanceCollectionOptions<'_>,
    options: SourceAcceptanceOptions,
    package_manifest_origin: Option<&str>,
) -> Result<CollectedSummary, CompatError> {
    let collected = collect_source_acceptance_facts_with_origin(
        root,
        mission,
        matrix,
        collection,
        package_manifest_origin,
    )
    .map_err(collection_error)?;
    Ok(CollectedSummary {
        required_artifacts_source: required_artifacts_disclosure(
            &collected.required_artifacts_origin,
            package_manifest_origin,
        ),
        summary: evaluate_source_acceptance(&collected.facts, options),
    })
}

/// Read `lanes.json` (PRIMARY partition); a corrupt document is Source's
/// `EVIDENCE_WINDOW_UNCOMPUTABLE` refusal.
fn read_lanes_manifest(
    mission: &sk_mission::Mission,
) -> Result<Option<sk_workspace::LanesManifest>, CompatError> {
    let store = sk_workspace::LanesStore::new(mission.spec_dir());
    match store.read() {
        Ok(manifest) => Ok(manifest),
        Err(error) => Err(CompatError::coded(
            EVIDENCE_WINDOW_UNCOMPUTABLE,
            format!(
                "{EVIDENCE_WINDOW_UNCOMPUTABLE}: the implementation-evidence window cannot be computed ({error})."
            ),
        )),
    }
}

/// The lazy work-package reads: only when `lanes.json` exists.
fn load_wp_state(
    mission: &sk_mission::Mission,
    manifest: Option<&sk_workspace::LanesManifest>,
) -> Result<Option<WpState>, CompatError> {
    if manifest.is_none() {
        // No `lanes.json`: Source's evidence preflight returns before reading the
        // status surface, and the matrix gate reports the missing manifest. A
        // corrupt WP-only artifact must not fail this run.
        return Ok(None);
    }
    let rows = lifecycle::status_rows(mission).map_err(|error| {
        // Source `_refuse_unevidenced_work` catches the store failure and says so
        // explicitly, so an unreadable status surface is not reported as a plain
        // domain refusal.
        CompatError::domain(format!(
            "The Mission could not be checked for implementation evidence ({}); nothing was accepted.",
            error.message()
        ))
    })?;
    let lanes = lifecycle::work_package_lanes(&rows).map_err(|error| {
        CompatError::domain(format!(
            "The Mission could not be checked for implementation evidence ({}); nothing was accepted.",
            error.message()
        ))
    })?;
    let lanes_by_wp: BTreeMap<String, String> = lanes.iter().cloned().collect();
    let review_evidence: Vec<ReviewEvidence> =
        lifecycle::review_evidence(&rows).into_values().collect();
    let canceled: BTreeSet<String> = lanes_by_wp
        .iter()
        .filter(|(_, lane)| lane.as_str() == "canceled")
        .map(|(wp_id, _)| wp_id.clone())
        .collect();
    let is_single_branch = matches!(mission.topology(), Some(MissionTopology::SingleBranch));
    Ok(Some(WpState {
        rows,
        lanes,
        lanes_by_wp,
        review_evidence,
        canceled,
        is_single_branch,
    }))
}

/// Source `_refuse_unevidenced_work`: exit 1 before the summary is collected;
/// with `report_only` (`--diagnose`) the refused report is returned instead.
fn refuse_unevidenced_work(
    root: &Path,
    mission: &sk_mission::Mission,
    state: Option<&WpState>,
    manifest: Option<&sk_workspace::LanesManifest>,
    report_only: bool,
    landed_commit: Option<&str>,
) -> Result<Option<EvidenceReport>, CompatError> {
    let (Some(state), Some(manifest)) = (state, manifest) else {
        // Source: `lanes.json` absent leaves the acceptance readiness check in
        // charge (it refuses on its own).
        return Ok(None);
    };
    let report = evaluate_evidence(&EvidenceInputs {
        repo_root: root,
        tasks_dir: &mission.spec_dir().join("tasks"),
        manifest,
        lanes: &state.lanes_by_wp,
        events: Some(&state.rows),
        is_single_branch: state.is_single_branch,
        canceled: &state.canceled,
        landed_commit,
    });
    let Some(refusal) = report.refusal.clone() else {
        return Ok(None);
    };
    if report_only {
        return Ok(Some(report));
    }
    let codes = report.codes();
    let first = codes
        .first()
        .cloned()
        .unwrap_or_else(|| EVIDENCE_WINDOW_UNCOMPUTABLE.to_owned());
    Err(CompatError::coded(static_code(first.as_str()), refusal)
        .with("error_code", json!(first))
        .with("error_codes", json!(codes)))
}

/// A `'static` spelling of a refusal code (they are all compile-time strings).
fn static_code(code: &str) -> &'static str {
    match code {
        evidence::IMPLEMENTATION_MISSING => evidence::IMPLEMENTATION_MISSING,
        evidence::REVIEW_NOT_INDEPENDENT => evidence::REVIEW_NOT_INDEPENDENT,
        evidence::IMPLEMENTATION_UNREADABLE => evidence::IMPLEMENTATION_UNREADABLE,
        _ => EVIDENCE_WINDOW_UNCOMPUTABLE,
    }
}

/// Validate every `--attest-no-implementation` request, then record each.
///
/// A refused request (a work package outside the approved claim, or one whose
/// evidence cannot be read) exits 1 with nothing recorded.
fn record_attestations(
    root: &Path,
    mission: &sk_mission::Mission,
    request: &attest::AttestationRequest,
    state: Option<&WpState>,
    manifest: Option<&sk_workspace::LanesManifest>,
    landed_commit: Option<&str>,
) -> Result<(), CompatError> {
    let (Some(state), Some(manifest)) = (state, manifest) else {
        return Err(CompatError::domain(format!(
            "{} needs the Mission's lanes.json (run finalize-tasks first). Nothing was recorded.",
            evidence::ATTEST_NO_IMPLEMENTATION_FLAG
        )));
    };
    let claim_lanes: BTreeMap<String, String> = state
        .lanes_by_wp
        .iter()
        .filter(|(wp_id, lane)| {
            !state.canceled.contains(wp_id.as_str()) && matches!(lane.as_str(), "approved" | "done")
        })
        .map(|(wp_id, lane)| (wp_id.clone(), lane.clone()))
        .collect();
    let report = evaluate_evidence(&EvidenceInputs {
        repo_root: root,
        tasks_dir: &mission.spec_dir().join("tasks"),
        manifest,
        lanes: &state.lanes_by_wp,
        events: Some(&state.rows),
        is_single_branch: state.is_single_branch,
        canceled: &state.canceled,
        landed_commit,
    });
    let plan = attest::plan(request, &claim_lanes, &report.verdicts)?;
    let actor = lifecycle::resolve_acceptance_actor(None);
    for (wp_id, lane) in plan {
        attest::record(
            root,
            mission.spec_dir(),
            mission.slug().as_str(),
            wp_id.as_str(),
            lane.as_str(),
            request.reason.as_deref().unwrap_or_default(),
            actor.as_str(),
        )?;
        eprintln!(
            "Operator attestation recorded for {wp_id} (no implementation owed) by {actor}: {}",
            request.reason.as_deref().unwrap_or_default()
        );
    }
    // Source records the attestation through its TRANSACTIONAL status seam, which
    // commits the status surface itself, and the acceptance dirty-tree gate then
    // sees a clean tree. This adapter appends through the same canonical writer
    // but the coord-transactional committer belongs to another lane, so the
    // accept adapter's own commit seam publishes exactly the surface the writer
    // touched — no new authority, and no silently dirty tree after an attested
    // run.
    commit_attestation_surface(root, mission)
}

/// Publish the status surface an attestation write touched.
fn commit_attestation_surface(
    root: &Path,
    mission: &sk_mission::Mission,
) -> Result<(), CompatError> {
    let mut paths = Vec::new();
    let events = support::events_path(mission);
    if events.exists() {
        paths.push(events);
    }
    let snapshot = mission.spec_dir().join(sk_status::SNAPSHOT_FILENAME);
    if snapshot.exists() {
        paths.push(snapshot);
    }
    if paths.is_empty() {
        return Ok(());
    }
    lifecycle::commit_acceptance_paths(
        root,
        mission,
        &paths,
        &format!(
            "Record operator attestation for {}",
            mission.slug().as_str()
        ),
        true,
    )
}

/// Source `_report_evidence_diagnosis`: the full diagnosis, the refusal as a
/// blocked check, exit 1.
#[allow(clippy::too_many_arguments)]
fn report_evidence_diagnosis(
    args: &AcceptArgs,
    mut summary: SourceAcceptanceSummary,
    report: &EvidenceReport,
    contribution: &MatrixContribution,
    origin_warnings: Vec<String>,
    required_source: String,
) -> Result<(Value, u8), CompatError> {
    let refusal = report.refusal.clone().unwrap_or_default();
    summary
        .blocked_checks
        .push(AcceptanceDiagnostic::new(EVIDENCE_CHECK, refusal.clone()));
    let codes = report.codes();
    let mut payload = summary.to_json();
    if let Some(object) = payload.as_object_mut() {
        object.insert("diagnose".into(), json!(true));
        object.insert("error".into(), json!(refusal));
        object.insert("error_code".into(), json!(codes.first()));
        object.insert("error_codes".into(), json!(codes));
        object.insert(
            "acceptance_matrix_path".into(),
            json!(contribution.directory),
        );
        object.insert("acceptance_ok".into(), json!(false));
        object.insert("mutation_performed".into(), json!(false));
        object.insert("required_artifacts_source".into(), json!(required_source));
        object.insert("evidence".into(), evidence_payload(report));
        let mut advisories = origin_warnings.clone();
        advisories.extend(summary.warnings.iter().cloned());
        if !advisories.is_empty() {
            object.insert("advisories".into(), json!(advisories));
        }
    }
    for warning in &origin_warnings {
        eprintln!("Warning: {warning}");
    }
    if args.json {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else {
        render_summary(&summary);
        println!("Diagnose: blocked");
        println!("{refusal}");
    }
    Ok((payload, 1))
}

/// The additive `evidence` payload block: the verdicts this run recorded.
fn evidence_payload(report: &EvidenceReport) -> Value {
    json!({
        "blocked": report.refusal.is_some(),
        "codes": report.codes(),
        "verdicts": report.verdicts,
        "lifted_work_packages": report.lifted_wp_ids,
    })
}

/// The read-only `--diagnose` collection: one pass, no writes, no lock.
#[allow(clippy::too_many_arguments)]
fn diagnose(
    root: &Path,
    mission: &sk_mission::Mission,
    workspace: &sk_workspace::MissionWorkspace,
    lanes: &[(String, String)],
    review_evidence: &[ReviewEvidence],
    collection: AcceptanceCollectionOptions<'_>,
    options: SourceAcceptanceOptions,
    package_manifest_origin: Option<&str>,
) -> Result<(SourceAcceptanceSummary, MatrixContribution, String), CompatError> {
    // Source mutates the matrix unless `--diagnose`; the diagnose leg judges the
    // matrix exactly as it exists on disk.
    let run = execute_acceptance_matrix(&AcceptanceMatrixRequest {
        workspace,
        lanes,
        review_evidence,
        diagnose: true,
        lock_timeout: sk_status::mutation_lock::MISSION_WRITE_LOCK_TIMEOUT,
    })
    .map_err(collection_error)?;
    let contribution = MatrixContribution {
        directory: run.directory.as_ref().map(|dir| dir.display().to_string()),
        activity_issues: run.activity_issues.clone(),
    };
    let collected = collect_origin_and_judge(
        root,
        mission,
        MatrixInput::Computed(&run.evidence),
        collection,
        options,
        package_manifest_origin,
    )?;
    let mut summary = collected.summary;
    absorb(
        &mut summary,
        &run.blocked_checks,
        &run.skipped_checks,
        &contribution,
    );
    Ok((summary, contribution, collected.required_artifacts_source))
}

/// Did the policy stop before the matrix gate (manifest, branch, planning-only)?
fn matrix_gate_stopped(summary: &SourceAcceptanceSummary) -> bool {
    summary.acceptance_matrix_gate_skip_reason.is_some()
        || summary
            .blocked_checks
            .iter()
            .any(|check| matches!(check.check.as_str(), "lanes_manifest" | "mission_branch"))
}

/// Fold the matrix step's own diagnostics into the policy summary.
///
/// The policy cannot know about a lock timeout or an unmaterialized coordination
/// surface, so those arrive here rather than being guessed at inside the policy.
fn absorb(
    summary: &mut SourceAcceptanceSummary,
    blocked: &[AcceptanceDiagnostic],
    skipped: &[AcceptanceDiagnostic],
    contribution: &MatrixContribution,
) {
    for issue in &contribution.activity_issues {
        if !summary.activity_issues.contains(issue) {
            summary.activity_issues.push(issue.clone());
        }
    }
    for check in blocked {
        if !summary.blocked_checks.contains(check) {
            summary.blocked_checks.push(check.clone());
        }
    }
    for check in skipped {
        if !summary.skipped_checks.contains(check) {
            summary.skipped_checks.push(check.clone());
        }
    }
    if let Some(directory) = &contribution.directory {
        summary.acceptance_matrix_dir = Some(directory.clone());
    }
}

/// The required planning-artifact names and where they were read from.
///
/// Source resolves `input.spec.main` / `output.plan.main` / `output.tasks.list`
/// from the packaged `software-dev` manifest (the acceptance path passes no
/// project root, so the org tier is not consulted). When the packaged tree is
/// absent altogether — an installation defect, not a Mission state — the pinned
/// value is used and disclosed (`None` origin) rather than pretending a manifest
/// was read.
fn required_artifact_names(root: &Path) -> Result<(Vec<String>, Option<String>), CompatError> {
    let providers = InstalledMissionProviders;
    let packaged = providers.packaged_missions_root(root);
    let decoder = sk_mission::PyYamlSafe;
    let resolved = sk_mission::expected_artifacts::acceptance_required_artifact_names_with_origin(
        sk_mission::expected_artifacts::ExpectedArtifactRoots {
            org_roots: &[],
            packaged_missions_root: packaged.as_deref(),
        },
        &decoder,
    )
    .map_err(|error| {
        CompatError::coded(
            "ACCEPTANCE_ARTIFACTS_UNRESOLVED",
            format!("the mission's expected artifacts could not be resolved: {error}"),
        )
    })?;
    match resolved {
        Some((names, origin)) => Ok((names, Some(origin))),
        None => Ok((PINNED_REQUIRED_ARTIFACTS.map(str::to_owned).to_vec(), None)),
    }
}

/// Render a collection failure with the Source-shaped acceptance codes.
fn collection_error(error: sk_workspace::error::WorkspaceError) -> CompatError {
    CompatError::coded(
        "ACCEPTANCE_COLLECTION_FAILED",
        format!("acceptance evidence could not be collected: {error}"),
    )
}

/// The `required_artifacts_source` disclosure for the run that actually happened.
///
/// * The non-WP branch reads the mission type's OWN DECLARED manifest, so that
///   manifest's origin is reported (Source's `required_artifacts_for` over the
///   plan family's `expected-artifacts.yaml`), never the software-dev trio's.
/// * The work-package branch reports the caller-resolved `software-dev`
///   `expected-artifacts.yaml`, or `pinned-default` when the packaged tree is
///   absent altogether (an installation defect, disclosed rather than hidden).
fn required_artifacts_disclosure(
    origin: &RequiredArtifactsOrigin,
    package_origin: Option<&str>,
) -> String {
    if origin.is_mission_type_manifest() {
        return origin
            .manifest_origin()
            .map(str::to_owned)
            .unwrap_or_else(|| "no-declared-manifest".to_owned());
    }
    match package_origin {
        Some(_) => "expected-artifacts.yaml".to_owned(),
        None => "pinned-default".to_owned(),
    }
}

/// Record acceptance, publish the matrix, and render — the finalize half.
#[allow(clippy::too_many_arguments)]
fn finish(
    root: &Path,
    mission: &sk_mission::Mission,
    args: &AcceptArgs,
    summary: SourceAcceptanceSummary,
    contribution: MatrixContribution,
    pr_merge: Option<sk_git::PrMergeEvidence>,
    origin_warnings: Vec<String>,
    required_source: String,
) -> Result<(Value, u8), CompatError> {
    let mode_text = lifecycle::resolved_mode(args, mission)?;
    let actor = lifecycle::resolve_acceptance_actor(args.actor.as_deref());
    let mut accept_commit: Option<String> = None;

    if summary.ok() && !args.diagnose && !args.no_commit {
        if let Some(pr_merge) = &pr_merge {
            sk_mission::record_pr_merge_baseline(
                mission.spec_dir(),
                pr_merge.baseline_merge_commit.as_str(),
                pr_merge.pr_merge_commit.as_str(),
                pr_merge.anchor_evidence,
            )
            .map_err(|error| {
                CompatError::coded(
                    "ACCEPTANCE_PR_MERGE_NOT_RECORDED",
                    format!(
                        "the PR landing commit {} was verified but its baseline could not be recorded: {error}",
                        pr_merge.pr_merge_commit
                    ),
                )
            })?;
        }
        let parent_commit = sk_git::GitRepository::open(root)
            .ok()
            .and_then(|repository| repository.head_commit().ok().flatten());
        let accepted_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false);
        let meta_path = sk_mission::record_acceptance(
            mission.spec_dir(),
            &actor,
            mode_text.as_str(),
            parent_commit.as_deref(),
            None,
            &accepted_at,
        )
        .map_err(|error| {
            CompatError::coded(
                "ACCEPTANCE_NOT_RECORDED",
                format!("the acceptance metadata could not be recorded: {error}"),
            )
        })?;
        lifecycle::commit_acceptance_paths(
            root,
            mission,
            std::slice::from_ref(&meta_path),
            &format!("Accept {}", mission.slug().as_str()),
            false,
        )?;
        let stamped_commit = sk_git::GitRepository::open(root)
            .ok()
            .and_then(|repository| repository.head_commit().ok().flatten());
        if let Some(stamped_commit) = &stamped_commit {
            let stamped = sk_mission::stamp_accept_commit(mission.spec_dir(), stamped_commit)
                .map_err(|error| {
                    CompatError::coded(
                        "ACCEPTANCE_NOT_RECORDED",
                        format!("accept_commit could not be stamped: {error}"),
                    )
                })?;
            if let Some(path) = &stamped {
                lifecycle::commit_acceptance_paths(
                    root,
                    mission,
                    std::slice::from_ref(path),
                    &format!("Record acceptance commit for {}", mission.slug().as_str()),
                    false,
                )?;
            }
        }
        // The matrix was written under the Mission lock by the execution step;
        // Source's finalize commit carries exactly that artifact.
        if let Some(directory) = &contribution.directory {
            let matrix_path =
                sk_status::acceptance::AcceptanceMatrix::path_in(Path::new(directory));
            lifecycle::commit_acceptance_paths(
                root,
                mission,
                std::slice::from_ref(&matrix_path),
                &format!(
                    "Finalize acceptance artifacts for {}",
                    mission.slug().as_str()
                ),
                true,
            )?;
        }
        accept_commit = stamped_commit;
    }

    let mut payload = summary.to_json();
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "acceptance_matrix_path".into(),
            json!(contribution.directory.clone()),
        );
        object.insert("acceptance_ok".into(), json!(summary.ok()));
        object.insert("lenient".into(), json!(args.lenient));
        object.insert("diagnose".into(), json!(args.diagnose));
        object.insert("no_commit".into(), json!(args.no_commit));
        object.insert(
            "mutation_performed".into(),
            json!(summary.ok() && !args.diagnose && !args.no_commit),
        );
        object.insert("required_artifacts_source".into(), json!(required_source));
        object.insert("matrix_verdict".into(), json!(summary.matrix_verdict));
        object.insert(
            "attested_work_packages".into(),
            json!(args.attest_no_implementation),
        );
        let mut advisories = origin_warnings.clone();
        advisories.extend(summary.warnings.iter().cloned());
        if !advisories.is_empty() {
            object.insert("advisories".into(), json!(advisories));
        }
        if let Some(accept_commit) = &accept_commit {
            object.insert("accept_commit".into(), json!(accept_commit));
        }
        if let Some(pr_merge) = &pr_merge {
            object.insert(
                "pr_merge".into(),
                json!({
                    "pr_merge_commit": pr_merge.pr_merge_commit.as_str(),
                    "baseline_merge_commit": pr_merge.baseline_merge_commit.as_str(),
                    "anchor_evidence": pr_merge.anchor_evidence,
                    "recorded": summary.ok() && !args.diagnose && !args.no_commit,
                }),
            );
        }
    }
    for warning in origin_warnings.iter().chain(summary.warnings.iter()) {
        eprintln!("Warning: {warning}");
    }
    if args.json {
        emit_json(&payload)
            .map_err(|error| CompatError::domain(format!("could not write JSON: {error}")))?;
    } else if args.diagnose {
        render_summary(&summary);
        println!(
            "Diagnose: {} (read-only; no metadata or matrix artifact was written)",
            if summary.ok() { "eligible" } else { "blocked" }
        );
    } else {
        render_summary(&summary);
        println!(
            "Acceptance: {} ({})",
            if summary.ok() { "ok" } else { "not ready" },
            summary
                .matrix_verdict
                .as_deref()
                .unwrap_or("no matrix verdict")
        );
        if let Some(directory) = &contribution.directory {
            let matrix_path =
                sk_status::acceptance::AcceptanceMatrix::path_in(Path::new(directory));
            println!("Matrix: {}", path_string(&matrix_path));
        }
    }
    // `--diagnose` reports readiness and exits 0 (Source `_report_diagnosis`);
    // every other mode exits 1 when the summary is not accepting.
    let code = if args.diagnose || summary.ok() { 0 } else { 1 };
    Ok((payload, code))
}

/// Source's text rendering of a summary, in its own section order.
fn render_summary(summary: &SourceAcceptanceSummary) {
    for (lane, wp_ids) in &summary.lanes {
        if wp_ids.is_empty() {
            continue;
        }
        println!("{lane}: {}", wp_ids.join(", "));
    }
    for (bucket, details) in summary.outstanding() {
        for detail in details {
            println!("  {bucket}: {detail}");
        }
    }
    if !summary.optional_missing.is_empty() {
        println!(
            "  optional_missing: {}",
            summary.optional_missing.join(", ")
        );
    }
    for check in &summary.skipped_checks {
        println!("  skipped: {} — {}", check.check, check.detail);
    }
    if !summary.recommended_fix_order.is_empty() {
        println!("Recommended fix order:");
        for (index, fix) in summary.recommended_fix_order.iter().enumerate() {
            println!("  {}. {fix}", index + 1);
        }
    }
}
