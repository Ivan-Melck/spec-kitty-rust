//! Operator attestation that an approved work package owes no implementation
//! (#6715) — the port of `specify_cli/consolidation/no_implementation_attestation.py`.
//!
//! `--attest-no-implementation <WP> --attest-reason "<why>"` records *why* an
//! approved work package's evidence window holds no change to its `owned_files`,
//! and the evidence axis then lifts `IMPLEMENTATION_MISSING` for that one work
//! package.
//!
//! **What it never does.** It lifts no other refusal: `REVIEW_NOT_INDEPENDENT`,
//! `IMPLEMENTATION_UNREADABLE` and `EVIDENCE_WINDOW_UNCOMPUTABLE` stand. It is
//! never a review: the independence comparison skips it when it looks for the
//! approving actor, and the recorded event is a forced self-transition, not an
//! approval.
//!
//! **Record shape — no event-schema change, one writer.** A forced operator
//! transition from the work package's CURRENT lane to itself, appended through
//! the canonical status writer ([`sk_status::mutation::emit_status_transition`]),
//! with `reason_source = "operator"` and
//! `policy_metadata["attestation"] = "no_implementation"`. The evidence contract
//! requires evidence for a transition into `approved`/`done`, so the record names
//! the operator and the attestation as its reference.
//!
//! **Atomicity residual (Source's own).** Every request is validated before any
//! is recorded, but the records are separate appends: a later write failure
//! leaves the earlier attestations recorded.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;
use sk_status::mutation::{ActorValue, TransitionRequest, emit_status_transition};

use super::evidence::{
    ATTEST_NO_IMPLEMENTATION_FLAG, ATTEST_REASON_FLAG, ATTESTATION_KEY,
    EVIDENCE_WINDOW_UNCOMPUTABLE, IMPLEMENTATION_MISSING, IMPLEMENTATION_UNREADABLE,
    NO_IMPLEMENTATION,
};
use super::support::CompatError;

/// Prefix of the recorded `reason`, so the status log says what was attested.
const REASON_PREFIX: &str = "operator attests no implementation is owed: ";
/// Evidence `review.reference` prefix: the attestation, not a review, is the reference.
const REFERENCE_PREFIX: &str = "operator-attestation:";
const NOTHING_RECORDED: &str = "Nothing was recorded.";

/// The verdicts an attestation cannot lift: the evidence itself cannot be read.
const UNLIFTABLE_VERDICTS: [&str; 2] = [IMPLEMENTATION_UNREADABLE, EVIDENCE_WINDOW_UNCOMPUTABLE];

/// What the operator asked to attest.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct AttestationRequest {
    /// Work package ids, in request order (duplicates dropped, blanks ignored).
    pub wp_ids: Vec<String>,
    /// The required `--attest-reason`.
    pub reason: Option<String>,
}

impl AttestationRequest {
    /// Read the two CLI flags into a request (Source `_validated_no_implementation_request`).
    ///
    /// Exit 2, nothing recorded, when a work package is named in a read-only mode
    /// (the request would record) or without a non-blank reason.
    pub fn from_flags(
        wp_ids: &[String],
        reason: Option<&str>,
        read_only_flag: Option<&str>,
    ) -> Result<Self, CompatError> {
        let mut requested: Vec<String> = Vec::new();
        for wp_id in wp_ids {
            let trimmed = wp_id.trim();
            if trimmed.is_empty() || requested.iter().any(|known| known == trimmed) {
                continue;
            }
            requested.push(trimmed.to_owned());
        }
        if requested.is_empty() {
            return Ok(Self::default());
        }
        if let Some(flag) = read_only_flag {
            return Err(CompatError::usage(format!(
                "{ATTEST_NO_IMPLEMENTATION_FLAG} would record a no-implementation attestation; {flag} is read-only. {NOTHING_RECORDED}"
            )));
        }
        let Some(reason) = reason.map(str::trim).filter(|text| !text.is_empty()) else {
            return Err(CompatError::usage(format!(
                "{ATTEST_NO_IMPLEMENTATION_FLAG} requires {ATTEST_REASON_FLAG} \"<why no implementation is owed>\". {NOTHING_RECORDED}"
            )));
        };
        Ok(Self {
            wp_ids: requested,
            reason: Some(reason.to_owned()),
        })
    }

    /// True when nothing was requested.
    pub fn is_empty(&self) -> bool {
        self.wp_ids.is_empty()
    }
}

/// Validate the request against the approved claim and the evidence verdict.
///
/// Returns work package id → its current lane, in request order. Raises (nothing
/// recorded) for a work package outside the approved claim, or one whose evidence
/// verdict an attestation cannot lift.
pub(super) fn plan(
    request: &AttestationRequest,
    claim_lanes: &BTreeMap<String, String>,
    verdicts: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, CompatError> {
    if request.is_empty() {
        return Ok(BTreeMap::new());
    }
    let not_approved: Vec<&str> = request
        .wp_ids
        .iter()
        .filter(|wp_id| !claim_lanes.contains_key(wp_id.as_str()))
        .map(String::as_str)
        .collect();
    if !not_approved.is_empty() {
        return Err(CompatError::domain(format!(
            "{ATTEST_NO_IMPLEMENTATION_FLAG} applies only to an approved work package; not approved: {}. {NOTHING_RECORDED}",
            not_approved.join(", ")
        )));
    }
    let blocked: Vec<String> = request
        .wp_ids
        .iter()
        .filter_map(|wp_id| {
            let verdict = verdicts.get(wp_id)?;
            UNLIFTABLE_VERDICTS
                .contains(&verdict.as_str())
                .then(|| format!("{wp_id} ({verdict})"))
        })
        .collect();
    if !blocked.is_empty() {
        return Err(CompatError::domain(format!(
            "{ATTEST_NO_IMPLEMENTATION_FLAG} lifts {IMPLEMENTATION_MISSING} only; the evidence of {} cannot be read, so an attestation would not lift it. Fix that first (see the refusal's recovery line). {NOTHING_RECORDED}",
            blocked.join(", ")
        )));
    }
    let mut plan = BTreeMap::new();
    for wp_id in &request.wp_ids {
        let lane = claim_lanes
            .get(wp_id.as_str())
            .cloned()
            .unwrap_or_else(|| "approved".to_owned());
        plan.insert(wp_id.clone(), lane);
    }
    Ok(plan)
}

/// Append ONE no-implementation attestation through the canonical status writer.
///
/// `feature_dir` is the PRIMARY, meta-bearing Mission dir. `current_lane` is the
/// work package's lane now, so the forced transition is from it to itself.
pub(super) fn record(
    repo_root: &Path,
    feature_dir: &Path,
    mission_slug: &str,
    wp_id: &str,
    current_lane: &str,
    reason: &str,
    actor: &str,
) -> Result<(), CompatError> {
    let mut request = TransitionRequest::new(
        feature_dir,
        mission_slug,
        wp_id,
        current_lane,
        ActorValue::identity(actor),
    );
    request.repo_root = Some(repo_root.to_path_buf());
    request.force = true;
    request.reason = Some(format!("{REASON_PREFIX}{}", reason.trim()));
    request.reason_source = Some(sk_status::OPERATOR_REASON_SOURCE.to_owned());
    request.evidence = Some(json!({
        "review": {
            "reviewer": actor,
            "verdict": "approved",
            "reference": format!("{REFERENCE_PREFIX}{wp_id}"),
        }
    }));
    request.policy_metadata = Some(json!({ ATTESTATION_KEY: NO_IMPLEMENTATION }));
    emit_status_transition(&request)
        .map(|_outcome| ())
        .map_err(|error| {
            CompatError::domain(format!(
                "the {ATTEST_NO_IMPLEMENTATION_FLAG} record for {wp_id} could not be written: {error}"
            ))
        })
}
