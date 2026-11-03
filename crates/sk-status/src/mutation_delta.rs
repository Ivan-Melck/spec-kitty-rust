//! Typed off-axis runtime-state deltas (`WPInnerStateDelta`) and their
//! Python-compatible scalar coercions.
//!
//! Native counterpart of `specify_cli.status.models.WPInnerStateDelta` +
//! `ReviewOverride` at baseline `9aed3a8012cb5eb22c0128b14d525039f95fea7a`.
//! The baseline type is a frozen dataclass, **not** a free `dict[str, Any]`
//! (constraint C-002): every slot is optional, a present slot is folded by the
//! reducer into the corresponding reduced-snapshot runtime field, and an absent
//! slot leaves it untouched.
//!
//! Semantics preserved from the baseline source (not guessed):
//!
//! - **Write-boundary blank normalisation.** `__post_init__` rewrites `""` to
//!   `None` for every pure `str | None` scalar slot, so the append-only log
//!   never records a blanking delta (`agent: ""` can no longer clobber a real
//!   recorded value). The reducer's read-side guard is the belt-and-braces for
//!   logs written before the normalisation existed.
//! - **Explicit claim release.** `release_runtime_claim` is a first-class
//!   `bool` (not a `str | None` scalar): `True` alone makes a delta non-empty
//!   and asks the reducer to clear `agent`/`shell_pid`/`shell_pid_created_at`.
//!   It is therefore deliberately *not* blank-normalised.
//! - **`shell_pid` integer coercion.** Python does `int(shell_pid_raw)`; this
//!   module reproduces Python's `int()`/`bool()` conversions rather than
//!   refusing shapes the baseline accepts.
//! - **Subtask value vocabulary.** A value is either a canonical [`Lane`] or
//!   the subtask-only `skipped`; the input alias `not_applicable` folds to
//!   `skipped` and is never persisted.
//! - **Review override shape.** Exactly the four pinned keys
//!   (`at`, `actor`, `wp_id`, `reason`) with Python `str()` coercion, the
//!   `complete` predicate and the narrow all-empty release sentinel.
//! - **Presence-only encoding.** `to_dict()` emits only populated slots, so the
//!   reducer's "absent leaves the slot untouched" rule is unambiguous on the
//!   wire. Scalar slot order follows the baseline `_SCALAR_FIELDS` tuple.
//!
//! Deliberate fail-closed strengthening (recorded in
//! `CONTINUATION-REPORT.md`): a *non-string* value in a `str | None` scalar
//! slot and an unknown delta key are refused instead of being persisted
//! verbatim. The baseline dataclass performs no runtime type check there, so a
//! caller could write `{"agent": 5}` into the log; the writer refuses it.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::lane::Lane;
use crate::mutation::MutationError;

/// The subtask-only terminal state; it is never a work-package lane.
pub const SUBTASK_SKIPPED: &str = "skipped";

/// Input alias of `skipped` on the read/write boundary; never persisted.
pub const SUBTASK_NOT_APPLICABLE: &str = "not_applicable";

/// One subtask's recorded value: a lane, or the subtask-only `skipped`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubtaskValue {
    /// A canonical lane (typically `done` or `planned`, but every lane parses).
    Lane(Lane),
    /// A deliberate operator decision not to do the subtask.
    Skipped,
}

impl SubtaskValue {
    /// Parse one recorded subtask value exactly as `WPInnerStateDelta.from_dict`
    /// does: the `skipped` / `not_applicable` aliases, else a canonical lane.
    pub fn parse(value: &str) -> Result<Self, String> {
        if value == SUBTASK_SKIPPED || value == SUBTASK_NOT_APPLICABLE {
            return Ok(SubtaskValue::Skipped);
        }
        Lane::parse_canonical(value)
            .map(SubtaskValue::Lane)
            .map_err(|error| format!("{value:?} is not a lane or subtask status: {error}"))
    }

    /// The persisted spelling (`not_applicable` is never written).
    pub fn as_str(self) -> &'static str {
        match self {
            SubtaskValue::Lane(lane) => lane.as_str(),
            SubtaskValue::Skipped => SUBTASK_SKIPPED,
        }
    }
}

/// The pinned review-cycle override slot carried by an annotation delta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewOverride {
    pub at: String,
    pub actor: String,
    pub wp_id: String,
    pub reason: String,
}

impl ReviewOverride {
    /// The four pinned keys, in baseline declaration order.
    pub const KEYS: [&'static str; 4] = ["at", "actor", "wp_id", "reason"];

    /// True only when all four fields are non-empty.
    pub fn complete(&self) -> bool {
        !(self.at.is_empty()
            || self.actor.is_empty()
            || self.wp_id.is_empty()
            || self.reason.is_empty())
    }

    /// True only when **all** four fields are empty (`is_release_sentinel`).
    ///
    /// Deliberately narrower than `!complete()`: a partially filled override
    /// must still be persisted, so its non-completeness keeps blocking the
    /// merge gate instead of being discarded as if no attempt was recorded.
    pub fn is_release_sentinel(&self) -> bool {
        self.at.is_empty()
            && self.actor.is_empty()
            && self.wp_id.is_empty()
            && self.reason.is_empty()
    }

    /// The pinned wire object (all four keys, always present).
    pub fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("at".to_owned(), Value::String(self.at.clone()));
        map.insert("actor".to_owned(), Value::String(self.actor.clone()));
        map.insert("wp_id".to_owned(), Value::String(self.wp_id.clone()));
        map.insert("reason".to_owned(), Value::String(self.reason.clone()));
        Value::Object(map)
    }

    /// Decode the pinned four-key object with the baseline `str()` coercion.
    pub fn from_json(value: &Value) -> Result<Self, MutationError> {
        let Value::Object(object) = value else {
            return Err(MutationError::InvalidRow {
                reason: "an annotation `review` override must be a JSON object".to_owned(),
            });
        };
        let mut fields = [String::new(), String::new(), String::new(), String::new()];
        for (index, key) in Self::KEYS.iter().enumerate() {
            let item = object.get(*key).ok_or_else(|| MutationError::InvalidRow {
                reason: format!("an annotation `review` override requires the {key:?} key"),
            })?;
            fields[index] = python_str(item).ok_or_else(|| MutationError::InvalidRow {
                reason: format!("annotation `review.{key}` must be a scalar, got {item}"),
            })?;
        }
        if let Some(extra) = object
            .keys()
            .find(|key| !Self::KEYS.contains(&key.as_str()))
        {
            return Err(MutationError::InvalidRow {
                reason: format!(
                    "annotation `review` override has an unknown key {extra:?}; expected exactly [\"at\", \"actor\", \"reason\", \"wp_id\"]"
                ),
            });
        }
        let [at, actor, wp_id, reason] = fields;
        Ok(ReviewOverride {
            at,
            actor,
            wp_id,
            reason,
        })
    }
}

/// The pure `str | None` scalar slots, in the baseline `_SCALAR_FIELDS` order.
pub const SCALAR_FIELDS: [&str; 8] = [
    "shell_pid_created_at",
    "agent",
    "assignee",
    "role",
    "agent_profile",
    "agent_profile_version",
    "model",
    "provider",
];

/// A typed partial runtime-state payload for an `InnerStateChanged` event.
///
/// Every field is optional; `None` leaves the reduced-snapshot slot untouched.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InnerStateDelta {
    pub shell_pid: Option<i64>,
    pub shell_pid_created_at: Option<String>,
    pub subtasks: Option<BTreeMap<String, SubtaskValue>>,
    pub note: Option<String>,
    pub tracker_refs: Option<Vec<String>>,
    pub tracker_refs_replace: Option<Vec<String>>,
    pub agent: Option<String>,
    pub assignee: Option<String>,
    pub review: Option<ReviewOverride>,
    pub role: Option<String>,
    pub agent_profile: Option<String>,
    pub agent_profile_version: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Explicit claim release (`release_runtime_claim`), a first-class bool.
    pub release_runtime_claim: bool,
}

impl InnerStateDelta {
    /// A delta with every slot absent.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: set the claim triple from an already-coerced shell pid.
    pub fn with_claim(
        mut self,
        agent: impl Into<String>,
        shell_pid: Option<i64>,
        shell_pid_created_at: Option<String>,
    ) -> Self {
        self.agent = Some(agent.into());
        self.shell_pid = shell_pid;
        self.shell_pid_created_at = shell_pid_created_at;
        self
    }

    /// Builder: record one subtask's value.
    pub fn with_subtask(mut self, task_id: impl Into<String>, value: SubtaskValue) -> Self {
        self.subtasks
            .get_or_insert_with(BTreeMap::new)
            .insert(task_id.into(), value);
        self
    }

    /// The `str | None` scalar slot by name (baseline `_SCALAR_FIELDS`).
    fn scalar(&self, name: &str) -> Option<&String> {
        match name {
            "shell_pid_created_at" => self.shell_pid_created_at.as_ref(),
            "agent" => self.agent.as_ref(),
            "assignee" => self.assignee.as_ref(),
            "role" => self.role.as_ref(),
            "agent_profile" => self.agent_profile.as_ref(),
            "agent_profile_version" => self.agent_profile_version.as_ref(),
            "model" => self.model.as_ref(),
            "provider" => self.provider.as_ref(),
            other => unreachable!("unknown scalar slot {other}"),
        }
    }

    fn scalar_mut(&mut self, name: &str) -> &mut Option<String> {
        match name {
            "shell_pid_created_at" => &mut self.shell_pid_created_at,
            "agent" => &mut self.agent,
            "assignee" => &mut self.assignee,
            "role" => &mut self.role,
            "agent_profile" => &mut self.agent_profile,
            "agent_profile_version" => &mut self.agent_profile_version,
            "model" => &mut self.model,
            "provider" => &mut self.provider,
            other => unreachable!("unknown scalar slot {other}"),
        }
    }

    /// The write-boundary normalisation: `""` in a scalar slot becomes absent.
    pub fn normalize_blank_scalars(&mut self) {
        for name in SCALAR_FIELDS {
            if self.scalar(name).is_some_and(|value| value.is_empty()) {
                *self.scalar_mut(name) = None;
            }
        }
    }

    /// True when the delta touches no slot.
    ///
    /// `release_runtime_claim` is checked separately: it is a `bool` whose
    /// default `false` never manufactures a non-empty delta on its own.
    pub fn is_empty(&self) -> bool {
        if self.release_runtime_claim {
            return false;
        }
        self.shell_pid.is_none()
            && self.subtasks.is_none()
            && self.note.is_none()
            && self.tracker_refs.is_none()
            && self.tracker_refs_replace.is_none()
            && self.review.is_none()
            && SCALAR_FIELDS.iter().all(|name| self.scalar(name).is_none())
    }

    /// Encode only populated slots (baseline `to_dict()` order).
    pub fn to_json_value(&self) -> Value {
        let mut map = Map::new();
        if let Some(pid) = self.shell_pid {
            map.insert("shell_pid".to_owned(), Value::from(pid));
        }
        if let Some(subtasks) = &self.subtasks {
            let mut encoded = Map::new();
            for (task_id, value) in subtasks {
                encoded.insert(task_id.clone(), Value::String(value.as_str().to_owned()));
            }
            map.insert("subtasks".to_owned(), Value::Object(encoded));
        }
        if let Some(note) = &self.note {
            map.insert("note".to_owned(), Value::String(note.clone()));
        }
        if let Some(refs) = &self.tracker_refs {
            map.insert(
                "tracker_refs".to_owned(),
                Value::Array(refs.iter().cloned().map(Value::String).collect()),
            );
        }
        if let Some(refs) = &self.tracker_refs_replace {
            map.insert(
                "tracker_refs_replace".to_owned(),
                Value::Array(refs.iter().cloned().map(Value::String).collect()),
            );
        }
        if let Some(review) = &self.review {
            map.insert("review".to_owned(), review.to_json_value());
        }
        for name in SCALAR_FIELDS {
            if let Some(value) = self.scalar(name) {
                map.insert(name.to_owned(), Value::String(value.clone()));
            }
        }
        if self.release_runtime_claim {
            map.insert("release_runtime_claim".to_owned(), Value::Bool(true));
        }
        Value::Object(map)
    }

    /// Decode and validate a delta object (the sanctioned write boundary).
    ///
    /// Types are checked rather than trusted, unknown keys are refused, and the
    /// result is blank-normalised exactly like the baseline `__post_init__`.
    pub fn from_json(value: &Value) -> Result<Self, MutationError> {
        let Value::Object(object) = value else {
            return Err(MutationError::InvalidRow {
                reason: "an annotation delta must be a JSON object".to_owned(),
            });
        };
        const KNOWN_KEYS: [&str; 15] = [
            "shell_pid",
            "subtasks",
            "note",
            "tracker_refs",
            "tracker_refs_replace",
            "review",
            "release_runtime_claim",
            "shell_pid_created_at",
            "agent",
            "assignee",
            "role",
            "agent_profile",
            "agent_profile_version",
            "model",
            "provider",
        ];
        if let Some(unknown) = object
            .keys()
            .find(|key| !KNOWN_KEYS.contains(&key.as_str()))
        {
            return Err(MutationError::InvalidRow {
                reason: format!(
                    "annotation delta has an unknown key {unknown:?}; the typed WPInnerStateDelta accepts exactly {KNOWN_KEYS:?}"
                ),
            });
        }

        let shell_pid = match object.get("shell_pid") {
            None | Some(Value::Null) => None,
            Some(value) => Some(python_int(value).ok_or_else(|| MutationError::InvalidRow {
                reason: format!("annotation `shell_pid` must be an int, got {value}"),
            })?),
        };

        let subtasks = match object.get("subtasks") {
            None | Some(Value::Null) => None,
            Some(Value::Object(entries)) => {
                let mut parsed = BTreeMap::new();
                for (task_id, value) in entries {
                    let Value::String(text) = value else {
                        return Err(MutationError::InvalidRow {
                            reason: format!(
                                "annotation `subtasks.{task_id}` must be a string, got {value}"
                            ),
                        });
                    };
                    let resolved =
                        SubtaskValue::parse(text).map_err(|reason| MutationError::InvalidRow {
                            reason: format!("annotation `subtasks.{task_id}`: {reason}"),
                        })?;
                    parsed.insert(task_id.clone(), resolved);
                }
                Some(parsed)
            }
            Some(other) => {
                return Err(MutationError::InvalidRow {
                    reason: format!("annotation `subtasks` must be an object, got {other}"),
                });
            }
        };

        let note = optional_string(object, "note")?;
        let tracker_refs = string_array(object, "tracker_refs")?;
        let tracker_refs_replace = string_array(object, "tracker_refs_replace")?;
        let review = match object.get("review") {
            None | Some(Value::Null) => None,
            Some(value) => Some(ReviewOverride::from_json(value)?),
        };

        let mut delta = InnerStateDelta {
            shell_pid,
            subtasks,
            note,
            tracker_refs,
            tracker_refs_replace,
            review,
            release_runtime_claim: object
                .get("release_runtime_claim")
                .map(python_bool)
                .unwrap_or(false),
            ..InnerStateDelta::default()
        };
        for name in SCALAR_FIELDS {
            *delta.scalar_mut(name) = optional_string(object, name)?;
        }
        delta.normalize_blank_scalars();
        Ok(delta)
    }
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, MutationError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(other) => Err(MutationError::InvalidRow {
            reason: format!("annotation `{key}` must be a string, got {other}"),
        }),
    }
}

fn string_array(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Vec<String>>, MutationError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                let Value::String(text) = item else {
                    return Err(MutationError::InvalidRow {
                        reason: format!(
                            "annotation `{key}` must be an array of strings, got {item}"
                        ),
                    });
                };
                values.push(text.clone());
            }
            Ok(Some(values))
        }
        Some(other) => Err(MutationError::InvalidRow {
            reason: format!("annotation `{key}` must be an array of strings, got {other}"),
        }),
    }
}

/// Python's `int(value)` for the scalar and PID shapes the baseline coerces.
///
/// `bool` is an `int` subclass, floats truncate toward zero, and a string is
/// parsed after surrounding whitespace with an optional sign (Python also
/// accepts `_` digit separators; they are accepted here too). Fractional or
/// non-numeric strings, non-finite floats, and containers are refusals — exactly
/// the inputs Python's `int()` raises for.
pub fn python_int(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                return Some(integer);
            }
            let float = number.as_f64()?;
            if !float.is_finite() {
                return None;
            }
            let truncated = float.trunc();
            if truncated < i64::MIN as f64 || truncated > i64::MAX as f64 {
                return None;
            }
            Some(truncated as i64)
        }
        Value::String(text) => parse_python_int_str(text),
        _ => None,
    }
}

fn parse_python_int_str(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (sign, digits) = match trimmed.as_bytes()[0] {
        b'+' => (1i64, &trimmed[1..]),
        b'-' => (-1i64, &trimmed[1..]),
        _ => (1i64, trimmed),
    };
    if digits.is_empty() {
        return None;
    }
    // Python's int() accepts `_` separators between digits only; a leading,
    // trailing or doubled separator is a ValueError.
    if digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let cleaned: String = digits
        .chars()
        .filter(|character| *character != '_')
        .collect();
    if cleaned.is_empty() || !cleaned.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let magnitude: i64 = cleaned.parse().ok()?;
    sign.checked_mul(magnitude)
}

/// Python's `bool(value)` truthiness (used by `release_runtime_claim`).
///
/// `None`/`false`/`0`/`0.0`/`""`/`[]`/`{}` are false; everything else —
/// including `NaN` and `"false"` — is true.
pub fn python_bool(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                integer != 0
            } else if let Some(unsigned) = number.as_u64() {
                unsigned != 0
            } else {
                number.as_f64().is_none_or(|float| float != 0.0)
            }
        }
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}

/// Python's `str(value)` for scalars (used by `ReviewOverride.from_dict`).
///
/// Scalars render exactly as Python renders them (`True`/`False`, `None`,
/// shortest float repr, decimal ints). Containers are a refusal here rather
/// than Python's `repr()`: a dict/list in a definitionally scalar reviewer slot
/// is a caller bug, and persisting its `repr` would hide it.
pub fn python_str(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("None".to_owned()),
        Value::Bool(true) => Some("True".to_owned()),
        Value::Bool(false) => Some("False".to_owned()),
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(crate::mutation_json::python_number(number)),
        Value::Array(_) | Value::Object(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn python_int_matches_the_baseline_coercions() {
        assert_eq!(python_int(&json!(true)), Some(1));
        assert_eq!(python_int(&json!(false)), Some(0));
        assert_eq!(python_int(&json!(-7)), Some(-7));
        assert_eq!(python_int(&json!(1.9)), Some(1));
        assert_eq!(python_int(&json!(-1.9)), Some(-1));
        assert_eq!(python_int(&json!(" 42 ")), Some(42));
        assert_eq!(python_int(&json!("+7")), Some(7));
        assert_eq!(python_int(&json!("1_000")), Some(1000));
        assert_eq!(python_int(&json!("3.7")), None);
        assert_eq!(python_int(&json!("")), None);
        assert_eq!(python_int(&json!("_1")), None);
        assert_eq!(python_int(&json!("1__0")), None);
        assert_eq!(python_int(&json!([1])), None);
    }

    #[test]
    fn python_bool_matches_truthiness() {
        assert!(!python_bool(&json!(null)));
        assert!(!python_bool(&json!(0)));
        assert!(!python_bool(&json!(0.0)));
        assert!(!python_bool(&json!("")));
        assert!(!python_bool(&json!([])));
        assert!(!python_bool(&json!({})));
        assert!(python_bool(&json!("false")));
        assert!(python_bool(&json!(-1)));
        assert!(python_bool(&json!({"a": 1})));
    }

    #[test]
    fn blank_scalars_normalise_and_release_flag_alone_is_not_empty() {
        let delta = InnerStateDelta::from_json(&json!({"agent": "", "note": ""})).unwrap();
        assert_eq!(delta.agent, None, "a blank scalar normalises to absent");
        assert_eq!(
            delta.note,
            Some(String::new()),
            "`note` is not a scalar slot, so the baseline keeps a blank note"
        );
        assert!(!delta.is_empty());

        let blank_only = InnerStateDelta::from_json(&json!({"agent": "", "assignee": ""})).unwrap();
        assert!(blank_only.is_empty(), "every blank scalar is absent");

        let release = InnerStateDelta::from_json(&json!({"release_runtime_claim": true})).unwrap();
        assert!(!release.is_empty());
        assert_eq!(
            release.to_json_value(),
            json!({"release_runtime_claim": true})
        );
    }

    #[test]
    fn subtask_aliases_fold_and_unknown_values_refuse() {
        let delta = InnerStateDelta::from_json(
            &json!({"subtasks": {"T001": "not_applicable", "T002": "done"}}),
        )
        .unwrap();
        let encoded = delta.to_json_value();
        assert_eq!(encoded["subtasks"]["T001"], json!("skipped"));
        assert_eq!(encoded["subtasks"]["T002"], json!("done"));

        let refused = InnerStateDelta::from_json(&json!({"subtasks": {"T001": "Done"}}));
        assert!(refused.is_err(), "canonical-lane match is case sensitive");
    }

    #[test]
    fn unknown_delta_keys_and_non_string_scalars_refuse() {
        assert!(InnerStateDelta::from_json(&json!({"hostname": "x"})).is_err());
        assert!(InnerStateDelta::from_json(&json!({"agent": 5})).is_err());
        assert!(InnerStateDelta::from_json(&json!({"shell_pid": "abc"})).is_err());
    }

    #[test]
    fn review_override_predicates_match_the_baseline() {
        let empty = ReviewOverride {
            at: String::new(),
            actor: String::new(),
            wp_id: String::new(),
            reason: String::new(),
        };
        assert!(empty.is_release_sentinel());
        assert!(!empty.complete());

        let partial = ReviewOverride {
            at: "2026-11-02T00:00:00Z".to_owned(),
            actor: "alice".to_owned(),
            wp_id: "WP01".to_owned(),
            reason: String::new(),
        };
        assert!(!partial.is_release_sentinel());
        assert!(!partial.complete());

        let parsed = ReviewOverride::from_json(&json!({
            "at": "2026-11-02T00:00:00Z",
            "actor": "alice",
            "wp_id": "WP01",
            "reason": "ok"
        }))
        .unwrap();
        assert!(parsed.complete());
        assert_eq!(parsed.to_json_value()["actor"], json!("alice"));
        assert_eq!(
            ReviewOverride::from_json(
                &json!({"at": 1, "actor": true, "wp_id": null, "reason": 2.5})
            )
            .unwrap(),
            ReviewOverride {
                at: "1".to_owned(),
                actor: "True".to_owned(),
                wp_id: "None".to_owned(),
                reason: "2.5".to_owned(),
            }
        );
        assert!(ReviewOverride::from_json(&json!({"at": "x"})).is_err());
    }
}
