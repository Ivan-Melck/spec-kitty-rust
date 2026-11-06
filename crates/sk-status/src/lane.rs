//! The closed work-package lane model.
//!
//! `doing` is deliberately **not** a [`Lane`] variant. It is an input alias
//! resolved by [`Lane::parse_input`]; persisted and internal lane values are
//! always canonical.

use std::fmt;
use std::str::FromStr;

/// A canonical work-package lifecycle lane.
///
/// The eleven variants are exhaustive. `genesis` and `uninitialized` are
/// non-display lanes; `done` and `canceled` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lane {
    Genesis,
    Uninitialized,
    Planned,
    Claimed,
    InProgress,
    ForReview,
    InReview,
    Approved,
    Done,
    Blocked,
    Canceled,
}

impl Lane {
    /// Every lane, in the canonical enum order.
    pub const ALL: [Lane; 11] = [
        Lane::Genesis,
        Lane::Uninitialized,
        Lane::Planned,
        Lane::Claimed,
        Lane::InProgress,
        Lane::ForReview,
        Lane::InReview,
        Lane::Approved,
        Lane::Done,
        Lane::Blocked,
        Lane::Canceled,
    ];

    /// The nine displayed lifecycle lanes (`planned` through `canceled`).
    pub const CANONICAL: [Lane; 9] = [
        Lane::Planned,
        Lane::Claimed,
        Lane::InProgress,
        Lane::ForReview,
        Lane::InReview,
        Lane::Approved,
        Lane::Done,
        Lane::Blocked,
        Lane::Canceled,
    ];

    /// Lanes that never appear on the board or in a display summary.
    pub const NON_DISPLAY: [Lane; 2] = [Lane::Genesis, Lane::Uninitialized];

    /// Lanes from which an ordinary (unforced) transition is impossible.
    pub const TERMINAL: [Lane; 2] = [Lane::Done, Lane::Canceled];

    /// The canonical wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Lane::Genesis => "genesis",
            Lane::Uninitialized => "uninitialized",
            Lane::Planned => "planned",
            Lane::Claimed => "claimed",
            Lane::InProgress => "in_progress",
            Lane::ForReview => "for_review",
            Lane::InReview => "in_review",
            Lane::Approved => "approved",
            Lane::Done => "done",
            Lane::Blocked => "blocked",
            Lane::Canceled => "canceled",
        }
    }

    /// Parse a canonical, persisted lane value. No aliases are accepted.
    pub fn parse_canonical(input: &str) -> Result<Self, LaneParseError> {
        match input {
            "genesis" => Ok(Lane::Genesis),
            "uninitialized" => Ok(Lane::Uninitialized),
            "planned" => Ok(Lane::Planned),
            "claimed" => Ok(Lane::Claimed),
            "in_progress" => Ok(Lane::InProgress),
            "for_review" => Ok(Lane::ForReview),
            "in_review" => Ok(Lane::InReview),
            "approved" => Ok(Lane::Approved),
            "done" => Ok(Lane::Done),
            "blocked" => Ok(Lane::Blocked),
            "canceled" => Ok(Lane::Canceled),
            _ => Err(LaneParseError {
                input: input.to_owned(),
            }),
        }
    }

    /// Parse a caller-supplied lane, resolving the `doing` input alias.
    ///
    /// Input is trimmed and lower-cased before alias resolution, matching the
    /// Python boundary. The error preserves the caller's original text.
    pub fn parse_input(input: &str) -> Result<Self, LaneParseError> {
        match Lane::parse_canonical(&resolve_lane_alias(input)) {
            Ok(lane) => Ok(lane),
            Err(_) => Err(LaneParseError {
                input: input.to_owned(),
            }),
        }
    }

    /// Structural successor edges. This is the single lane-adjacency authority.
    pub const fn allowed_targets(self) -> &'static [Lane] {
        match self {
            Lane::Genesis => &[Lane::Planned, Lane::Canceled],
            Lane::Uninitialized => &[],
            Lane::Planned => &[Lane::Claimed, Lane::Blocked, Lane::Canceled],
            Lane::Claimed => &[Lane::InProgress, Lane::Blocked, Lane::Canceled],
            Lane::InProgress => &[
                Lane::ForReview,
                Lane::Approved,
                Lane::Planned,
                Lane::Blocked,
                Lane::Canceled,
            ],
            Lane::ForReview => &[Lane::InReview, Lane::Blocked, Lane::Canceled],
            Lane::InReview => &[
                Lane::Approved,
                Lane::Done,
                Lane::InProgress,
                Lane::Planned,
                Lane::Blocked,
                Lane::Canceled,
            ],
            Lane::Approved => &[
                Lane::Done,
                Lane::InProgress,
                Lane::Planned,
                Lane::Blocked,
                Lane::Canceled,
            ],
            Lane::Done => &[],
            Lane::Blocked => &[Lane::InProgress, Lane::Canceled],
            Lane::Canceled => &[],
        }
    }

    /// Guard-free structural edge check.
    pub fn may_transition_to(self, target: Lane) -> bool {
        self.allowed_targets().contains(&target)
    }

    /// True for `done` and `canceled`.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Lane::Done | Lane::Canceled)
    }

    /// True only for the blocked lane.
    pub const fn is_blocked(self) -> bool {
        matches!(self, Lane::Blocked)
    }

    /// True for the active lanes `planned` through `approved`.
    pub const fn is_run_affecting(self) -> bool {
        matches!(
            self,
            Lane::Planned
                | Lane::Claimed
                | Lane::InProgress
                | Lane::ForReview
                | Lane::InReview
                | Lane::Approved
        )
    }

    /// True for display lanes (`planned` through `canceled`).
    pub const fn is_display(self) -> bool {
        !matches!(self, Lane::Genesis | Lane::Uninitialized)
    }

    /// The progress bucket: `not_started`, `in_flight`, `review`, `terminal`.
    pub const fn progress_bucket(self) -> &'static str {
        match self {
            Lane::Genesis | Lane::Uninitialized | Lane::Planned => "not_started",
            Lane::Claimed | Lane::InProgress | Lane::Blocked => "in_flight",
            Lane::ForReview | Lane::InReview | Lane::Approved => "review",
            Lane::Done | Lane::Canceled => "terminal",
        }
    }

    /// The kanban column label (non-display lanes group under `Planned`).
    pub const fn display_category(self) -> &'static str {
        match self {
            Lane::Genesis | Lane::Uninitialized | Lane::Planned => "Planned",
            Lane::Claimed | Lane::InProgress | Lane::InReview => "In Progress",
            Lane::ForReview => "Review",
            Lane::Approved => "Approved",
            Lane::Done => "Done",
            Lane::Blocked => "Blocked",
            Lane::Canceled => "Canceled",
        }
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Lane {
    type Err = LaneParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Lane::parse_canonical(input)
    }
}

/// Resolve the `doing` input alias after trimming and lower-casing.
///
/// Returns the normalized lane text, even when it is not a known lane, so the
/// function is a total projection (mirrors the Python `resolve_lane_alias`).
pub fn resolve_lane_alias(lane: &str) -> String {
    let normalized = lane.trim().to_lowercase();
    match normalized.as_str() {
        "doing" => "in_progress".to_owned(),
        _ => normalized,
    }
}

/// The one refusal for a lane value that is not part of the closed model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Unknown lane: {input}")]
pub struct LaneParseError {
    pub input: String,
}
