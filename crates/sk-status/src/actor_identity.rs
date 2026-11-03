//! Canonical actor-identity projection and the reviewer-independence predicate.
//!
//! Native counterparts of the pinned baseline's
//! `status.models.actor_full_identity` / `actor_identities_independent`,
//! `status.work_package_lifecycle._actor_key` / `GENERIC_IMPLEMENTATION_ACTORS`,
//! and `status.review_roles.latest_implementer_actor` at
//! `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
//!
//! These are read-only decisions over recorded wire values: nothing here reads
//! or writes a log, and nothing invents an identity. A `{role, profile, tool,
//! model}` resolved binding and its compact `tool:model:profile:role` string
//! reduce to the same three-part identity, because the role is lane-stamped and
//! would otherwise make an agent unequal to itself (#5340).
//!
//! The identity has three parts — `tool:model:profile` — and the independence
//! rule is the operator ruling R1 one: two identities are provably independent
//! only when their tools differ, or a segment recorded on **both** sides
//! differs. A segment recorded on one side only never proves independence, and
//! an unknown identity is independent of nothing.

use serde_json::Value;

use crate::guard::{MIGRATION_ACTOR_PREFIX, is_migration_actor};
use crate::mutation_emit::ActorValue;

/// The placeholder actors that name no person (#5939 / `GENERIC_IMPLEMENTATION_ACTORS`).
pub const GENERIC_IMPLEMENTATION_ACTORS: [&str; 3] = ["implement-command", "unknown", "user"];

/// The placeholder a refusal names when no actor is on record.
pub const UNKNOWN_ACTOR: &str = "unknown";

/// The `profile_source` marker that means a profile was planned, never proven.
///
/// A frontmatter-filled profile never asserts identity (operator decision
/// #6400, FR-013), so [`actor_full_identity`] leaves it out.
pub const PROFILE_SOURCE_FIELD: &str = "profile_source";

fn non_blank(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Parse a compact `tool:model:profile:role` agent boundary string.
///
/// Faithful to `status.models.parse_agent_boundary_string`: trailing segments
/// default to `None`, interior whitespace is stripped, an empty tool or more
/// than four segments is a refusal.
pub fn parse_boundary(raw: &str) -> Result<Vec<Option<String>>, String> {
    let segments: Vec<&str> = raw.split(':').collect();
    if segments.len() > 4 {
        return Err(format!(
            "Too many segments in --agent value: {raw:?} (expected at most 4, got {})",
            segments.len()
        ));
    }
    let mut padded: Vec<Option<String>> = vec![None; 4];
    for (index, segment) in segments.iter().enumerate() {
        padded[index] = non_blank(Some(segment));
    }
    if padded[0].is_none() {
        return Err(format!("Empty agent tool in --agent value: {raw:?}"));
    }
    Ok(padded)
}

fn join_identity(parts: [Option<String>; 3]) -> Option<String> {
    let [tool, model, profile] = parts;
    let identity = format!(
        "{}:{}:{}",
        tool.unwrap_or_default().trim(),
        model.unwrap_or_default().trim(),
        profile.unwrap_or_default().trim()
    );
    if identity.trim_matches(':').is_empty() {
        None
    } else {
        Some(identity)
    }
}

/// The full identity of a recorded wire actor: `tool:model:profile`, or `None`.
///
/// Accepts a structured four-key object and a compact string; the role is never
/// part of the identity, and a `profile_source: "wp_frontmatter"` profile is
/// dropped (it was planned, not proven).
pub fn actor_full_identity(actor: &Value) -> Option<String> {
    match actor {
        Value::Object(object) => {
            let planned_only = object
                .get(PROFILE_SOURCE_FIELD)
                .and_then(Value::as_str)
                .is_some_and(|source| source == "wp_frontmatter");
            let field = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
            let profile = if planned_only { None } else { field("profile") };
            join_identity([field("tool"), field("model"), profile])
        }
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            match parse_boundary(text) {
                Ok(parts) => join_identity([parts[0].clone(), parts[1].clone(), parts[2].clone()]),
                Err(_) => Some(text.to_owned()),
            }
        }
        _ => None,
    }
}

/// The full identity of a typed [`ActorValue`].
pub fn actor_value_full_identity(actor: &ActorValue) -> Option<String> {
    actor_full_identity(&actor.to_json_value())
}

/// The impl-claim comparison key of a wire actor: its bare `tool`.
///
/// A compact string is parsed down to its tool (so `claude:a:b:reviewer` and
/// `{"tool": "claude"}` compare equal); a string the parser rejects stays whole
/// as recorded. Role-blind by construction.
pub fn actor_key(actor: &Value) -> Option<String> {
    match actor {
        Value::Object(object) => non_blank(object.get("tool").and_then(Value::as_str)),
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            match parse_boundary(text) {
                Ok(parts) => parts[0].clone(),
                Err(_) => Some(text.to_owned()),
            }
        }
        _ => None,
    }
}

/// True when two recorded full identities provably belong to different actors.
///
/// Differing tools prove independence; otherwise a model or profile recorded on
/// both sides must differ. An unknown (blank/`None`) identity is independent of
/// nothing.
pub fn actor_identities_independent(left: Option<&str>, right: Option<&str>) -> bool {
    let clean = |value: Option<&str>| {
        value
            .map(|text| text.trim())
            .filter(|text| !text.trim_matches(':').is_empty())
            .map(str::to_owned)
    };
    let (Some(left), Some(right)) = (clean(left), clean(right)) else {
        return false;
    };
    let segments = |identity: &str| -> [String; 3] {
        let mut pieces = identity.splitn(3, ':');
        let tool = pieces.next().unwrap_or_default().trim().to_owned();
        let model = pieces.next().unwrap_or_default().trim().to_owned();
        let profile = pieces.next().unwrap_or_default().trim().to_owned();
        [tool, model, profile]
    };
    let left = segments(&left);
    let right = segments(&right);
    if left[0] != right[0] {
        return true;
    }
    (1..3).any(|index| {
        !left[index].is_empty() && !right[index].is_empty() && left[index] != right[index]
    })
}

fn value_is_migration_actor(actor: &Value) -> bool {
    match actor {
        Value::String(text) => is_migration_actor(Some(text.as_str())),
        Value::Object(object) => object
            .get("tool")
            .and_then(Value::as_str)
            .is_some_and(|tool| tool.starts_with(MIGRATION_ACTOR_PREFIX)),
        _ => false,
    }
}

/// The reviewer-rework predicate (`review_roles._is_reviewer_rework_verdict`).
fn is_reviewer_rework_verdict(row: &Value) -> bool {
    let from = row.get("from_lane").and_then(Value::as_str).unwrap_or("");
    let has_ref = row
        .get("review_ref")
        .and_then(Value::as_str)
        .is_some_and(|reference| !reference.is_empty());
    has_ref && matches!(from, "for_review" | "in_review" | "approved")
}

/// A qualifying implementing claim: a real, non-generic actor into
/// `claimed`/`in_progress`, never a migration seed and never a reviewer rework.
fn qualifies_as_implementer(row: &Value) -> bool {
    if row.get("kind").is_some_and(|kind| !kind.is_null()) {
        return false;
    }
    let to = row.get("to_lane").and_then(Value::as_str).unwrap_or("");
    if !matches!(to, "claimed" | "in_progress") {
        return false;
    }
    if is_reviewer_rework_verdict(row) {
        return false;
    }
    let Some(actor) = row.get("actor") else {
        return false;
    };
    if value_is_migration_actor(actor) {
        return false;
    }
    let Some(key) = actor_key(actor) else {
        return false;
    };
    !GENERIC_IMPLEMENTATION_ACTORS.contains(&key.as_str())
}

/// `wp_id`'s latest qualifying implementing claim actor as a full identity.
///
/// Scans newest-first in append order; `None` when the WP has no implementer of
/// record (the move guards treat an unknown implementer as "cannot prove
/// independence" and pass, exactly as the baseline).
pub fn latest_implementer_actor(rows: &[Value], wp_id: &str) -> Option<String> {
    rows.iter()
        .rev()
        .filter(|row| row.get("wp_id").and_then(Value::as_str) == Some(wp_id))
        .find(|row| qualifies_as_implementer(row))
        .and_then(|row| row.get("actor"))
        .and_then(actor_full_identity)
}

/// `wp_id`'s latest qualifying implementing claim key (bare tool).
pub fn latest_implementer_key(rows: &[Value], wp_id: &str) -> Option<String> {
    rows.iter()
        .rev()
        .filter(|row| row.get("wp_id").and_then(Value::as_str) == Some(wp_id))
        .find(|row| qualifies_as_implementer(row))
        .and_then(|row| row.get("actor"))
        .and_then(actor_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(actor: Value, from: &str, to: &str, wp: &str) -> Value {
        json!({
            "event_id": "01M6GKCHTCGY549MM8NH2R6FYK",
            "wp_id": wp,
            "from_lane": from,
            "to_lane": to,
            "actor": actor,
            "review_ref": Value::Null,
            "reason": Value::Null,
        })
    }

    #[test]
    fn identity_excludes_role_and_matches_both_representations() {
        let dict = json!({"role": "reviewer", "profile": null, "tool": "claude", "model": null});
        assert_eq!(actor_full_identity(&dict).as_deref(), Some("claude::"));
        assert_eq!(
            actor_full_identity(&json!("claude:::reviewer")).as_deref(),
            Some("claude::")
        );
        assert_eq!(
            actor_full_identity(&json!("claude:opus:reviewer-renata:reviewer")).as_deref(),
            Some("claude:opus:reviewer-renata")
        );
        assert_eq!(actor_full_identity(&json!("  ")), None);
    }

    #[test]
    fn a_planned_frontmatter_profile_never_asserts_identity() {
        let actor = json!({
            "role": "implementer",
            "profile": "planner",
            "profile_source": "wp_frontmatter",
            "tool": "claude",
            "model": "opus",
        });
        assert_eq!(actor_full_identity(&actor).as_deref(), Some("claude:opus:"));
        assert!(!actor_identities_independent(
            actor_full_identity(&actor).as_deref(),
            Some("claude:opus:planner")
        ));
    }

    #[test]
    fn independence_needs_a_proven_difference() {
        assert!(actor_identities_independent(
            Some("claude::"),
            Some("codex::")
        ));
        assert!(actor_identities_independent(
            Some("claude:opus:"),
            Some("claude:sonnet:")
        ));
        assert!(actor_identities_independent(
            Some("claude::reviewer-a"),
            Some("claude::reviewer-b")
        ));
        // A segment on one side only proves nothing.
        assert!(!actor_identities_independent(
            Some("claude::"),
            Some("claude:opus:")
        ));
        assert!(!actor_identities_independent(
            Some("claude::"),
            Some("claude::")
        ));
        assert!(!actor_identities_independent(None, Some("codex::")));
    }

    #[test]
    fn actor_key_is_the_bare_tool() {
        assert_eq!(
            actor_key(&json!({"tool": "claude", "role": "reviewer"})).as_deref(),
            Some("claude")
        );
        assert_eq!(
            actor_key(&json!("claude:opus:p:reviewer")).as_deref(),
            Some("claude")
        );
        assert_eq!(actor_key(&json!("")), None);
    }

    #[test]
    fn latest_implementer_skips_generic_seeds_rework_and_annotations() {
        let rows = vec![
            row(json!("claude"), "genesis", "planned", "WP01"),
            row(json!("user"), "planned", "claimed", "WP01"),
            row(json!("claude"), "planned", "claimed", "WP01"),
            row(json!("alice"), "claimed", "in_progress", "WP01"),
            json!({"kind": "annotation", "wp_id": "WP01", "actor": json!("bob"), "delta": {}}),
            json!({
                "wp_id": "WP01",
                "from_lane": "for_review",
                "to_lane": "in_progress",
                "actor": json!("carol"),
                "review_ref": "feedback://x",
            }),
            row(json!("migration:seed"), "planned", "claimed", "WP02"),
        ];
        assert_eq!(
            latest_implementer_actor(&rows, "WP01").as_deref(),
            Some("alice::")
        );
        assert_eq!(
            latest_implementer_key(&rows, "WP01").as_deref(),
            Some("alice")
        );
        assert_eq!(latest_implementer_actor(&rows, "WP02"), None);
    }

    #[test]
    fn the_placeholder_approver_is_recognised() {
        assert!(
            actor_key(&json!("user"))
                .is_some_and(|key| GENERIC_IMPLEMENTATION_ACTORS.contains(&key.as_str()))
        );
    }
}
