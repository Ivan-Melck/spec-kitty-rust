//! `ModeOfWork` and its deterministic derivation, plus the record-read policy.
//!
//! `modes.py` is the enum and the `entry command → mode` table;
//! `executor.classify_mode_of_work` / `mode_permits_evidence` are the pure
//! readers of a *recorded* mode (absent vs known vs undetermined). All three
//! are value-level decisions with no I/O, so they belong with the record
//! models rather than with the (unported) executor.

use serde_json::Value;

use crate::error::UnknownEntryCommand;

/// How the Op was opened (`modes.ModeOfWork`).
///
/// `Advisory` is kept for historical Op records; new standalone dispatch
/// records are written as `TaskExecution`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModeOfWork {
    /// Historical records only.
    Advisory,
    /// Standalone dispatch / operator invocation.
    TaskExecution,
    /// A `next`-driven Mission step.
    MissionStep,
    /// A query command; recorded for enforcement consistency.
    Query,
}

impl ModeOfWork {
    /// Every variant, in the reference's declaration order.
    pub const ALL: [Self; 4] = [
        Self::Advisory,
        Self::TaskExecution,
        Self::MissionStep,
        Self::Query,
    ];

    /// The wire value of this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Advisory => "advisory",
            Self::TaskExecution => "task_execution",
            Self::MissionStep => "mission_step",
            Self::Query => "query",
        }
    }

    /// Parse a wire value. Exact and case-sensitive, like a Pydantic `Literal`.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }

    /// Whether this known mode may carry a Tier 2 evidence artifact.
    ///
    /// `mode_permits_evidence` rejects `advisory` and `query`; absence is
    /// permissive (the documented legacy default is `task_execution`).
    pub fn permits_evidence(self) -> bool {
        !matches!(self, Self::Advisory | Self::Query)
    }
}

/// The `_ENTRY_COMMAND_MODE` table, verbatim and in source order.
pub const ENTRY_COMMAND_MODES: &[(&str, ModeOfWork)] = &[
    ("dispatch", ModeOfWork::TaskExecution),
    ("next.specify", ModeOfWork::MissionStep),
    ("next.plan", ModeOfWork::MissionStep),
    ("next.tasks", ModeOfWork::MissionStep),
    ("next.implement", ModeOfWork::MissionStep),
    ("next.review", ModeOfWork::MissionStep),
    ("next.merge", ModeOfWork::MissionStep),
    ("next.accept", ModeOfWork::MissionStep),
    ("profiles.list", ModeOfWork::Query),
    ("invocations.list", ModeOfWork::Query),
];

/// Derive the mode from a CLI entry command.
///
/// The reference raises `KeyError` for an unregistered token; nothing is
/// defaulted, because a mistyped entry command is a CLI-layer programming
/// error rather than an operator error.
pub fn derive_mode(entry_command: &str) -> Result<ModeOfWork, UnknownEntryCommand> {
    ENTRY_COMMAND_MODES
        .iter()
        .find(|(entry, _)| *entry == entry_command)
        .map(|(_, mode)| *mode)
        .ok_or_else(|| UnknownEntryCommand {
            entry: entry_command.to_owned(),
        })
}

/// A recorded `mode_of_work` kept in its three distinct states.
///
/// `None` in the reference means the record carries **no** mode (a pre-v2
/// record, which keeps the documented `task_execution` default);
/// `Undetermined` means it declares something no reader can map. Collapsing
/// the two would let a mangled string inherit the permissive legacy default.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordedMode {
    /// The field is absent, JSON `null`, or the empty string.
    Absent,
    /// A recognised mode.
    Known(ModeOfWork),
    /// A declared value that maps to no `ModeOfWork`; carried verbatim.
    Undetermined(Value),
}

impl RecordedMode {
    /// Whether this recorded state may carry a Tier 2 evidence artifact.
    ///
    /// Undetermined is **not** permission; absence is.
    pub fn permits_evidence(&self) -> bool {
        match self {
            Self::Undetermined(_) => false,
            Self::Absent => true,
            Self::Known(mode) => mode.permits_evidence(),
        }
    }
}

/// Classify a recorded `mode_of_work` value (`executor.classify_mode_of_work`).
///
/// `raw` is the JSON value of the field, or `None` when the line has no such
/// key. JSON `null` and `""` are absence; a non-string (including `false`,
/// `0`, arrays and objects) is malformation, never absence.
pub fn classify_mode_of_work(raw: Option<&Value>) -> RecordedMode {
    match raw {
        None | Some(Value::Null) => RecordedMode::Absent,
        Some(Value::String(value)) if value.is_empty() => RecordedMode::Absent,
        Some(Value::String(value)) => match ModeOfWork::parse(value) {
            Some(mode) => RecordedMode::Known(mode),
            None => RecordedMode::Undetermined(Value::String(value.clone())),
        },
        Some(other) => RecordedMode::Undetermined(other.clone()),
    }
}

/// Whether a recorded mode may carry a Tier 2 evidence artifact.
pub fn mode_permits_evidence(mode: &RecordedMode) -> bool {
    mode.permits_evidence()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entry_command_table_matches_the_reference() {
        assert_eq!(derive_mode("dispatch").unwrap(), ModeOfWork::TaskExecution);
        for step in [
            "specify",
            "plan",
            "tasks",
            "implement",
            "review",
            "merge",
            "accept",
        ] {
            assert_eq!(
                derive_mode(&format!("next.{step}")).unwrap(),
                ModeOfWork::MissionStep
            );
        }
        assert_eq!(derive_mode("profiles.list").unwrap(), ModeOfWork::Query);
        assert_eq!(derive_mode("invocations.list").unwrap(), ModeOfWork::Query);
        assert_eq!(
            derive_mode("next.retrospect").unwrap_err(),
            UnknownEntryCommand {
                entry: "next.retrospect".to_owned()
            }
        );
    }

    #[test]
    fn mode_wire_values_are_exact_and_case_sensitive() {
        assert_eq!(ModeOfWork::TaskExecution.as_str(), "task_execution");
        assert_eq!(
            ModeOfWork::parse("task_execution"),
            Some(ModeOfWork::TaskExecution)
        );
        assert_eq!(ModeOfWork::parse("TASK_EXECUTION"), None);
        assert_eq!(ModeOfWork::parse("bogus"), None);
    }

    #[test]
    fn recorded_mode_keeps_absence_and_malformation_apart() {
        assert_eq!(classify_mode_of_work(None), RecordedMode::Absent);
        assert_eq!(
            classify_mode_of_work(Some(&json!(null))),
            RecordedMode::Absent
        );
        assert_eq!(
            classify_mode_of_work(Some(&json!(""))),
            RecordedMode::Absent
        );
        assert_eq!(
            classify_mode_of_work(Some(&json!("mission_step"))),
            RecordedMode::Known(ModeOfWork::MissionStep)
        );
        assert_eq!(
            classify_mode_of_work(Some(&json!(false))),
            RecordedMode::Undetermined(json!(false))
        );
        assert_eq!(
            classify_mode_of_work(Some(&json!("bogus"))),
            RecordedMode::Undetermined(json!("bogus"))
        );
    }

    #[test]
    fn evidence_permission_follows_the_reference_gate() {
        assert!(mode_permits_evidence(&RecordedMode::Absent));
        assert!(mode_permits_evidence(&RecordedMode::Known(
            ModeOfWork::TaskExecution
        )));
        assert!(mode_permits_evidence(&RecordedMode::Known(
            ModeOfWork::MissionStep
        )));
        assert!(!mode_permits_evidence(&RecordedMode::Known(
            ModeOfWork::Advisory
        )));
        assert!(!mode_permits_evidence(&RecordedMode::Known(
            ModeOfWork::Query
        )));
        assert!(!mode_permits_evidence(&RecordedMode::Undetermined(json!(
            "bogus"
        ))));
    }
}
