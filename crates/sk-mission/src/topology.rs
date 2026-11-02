use std::fmt;

use serde::{Deserialize, Serialize};

/// The four stored Mission shapes of the coordination × lanes grid.
///
/// `flattened` is provenance, not a shape: a mission whose coordination branch
/// was dropped is `SingleBranch`/`Lanes` plus a separate `flattened` flag. This
/// enum therefore never carries a `Flattened` member.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionTopology {
    /// No coordination branch, no lanes.
    SingleBranch,
    /// No coordination branch, lanes present.
    Lanes,
    /// Coordination branch, no lanes.
    Coord,
    /// Coordination branch and lanes.
    LanesWithCoord,
}

impl MissionTopology {
    /// Every topology member, in declaration order.
    pub const ALL: [Self; 4] = [
        Self::SingleBranch,
        Self::Lanes,
        Self::Coord,
        Self::LanesWithCoord,
    ];

    /// The canonical stored string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleBranch => "single_branch",
            Self::Lanes => "lanes",
            Self::Coord => "coord",
            Self::LanesWithCoord => "lanes_with_coord",
        }
    }

    /// Parse the stored `meta.json` `topology` value.
    ///
    /// A missing, non-string, or unrecognised value degrades to `None`, matching
    /// the upstream reader's tolerance. This never derives a shape from absent
    /// provenance and never treats `flattened` as a topology.
    pub fn from_stored(raw: Option<&str>) -> Option<Self> {
        let raw = raw?;
        Self::ALL
            .into_iter()
            .find(|topology| topology.as_str() == raw)
    }
}

impl fmt::Display for MissionTopology {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Map the two orthogonal mission signals to one topology cell.
///
/// This is the single 2×2 classifier: callers supply the coordination branch
/// (if any) and whether a lane manifest carries lanes. `flattened` is never
/// consulted.
pub fn classify_topology(coordination_branch: Option<&str>, has_lanes: bool) -> MissionTopology {
    match (coordination_branch.is_some(), has_lanes) {
        (true, true) => MissionTopology::LanesWithCoord,
        (true, false) => MissionTopology::Coord,
        (false, true) => MissionTopology::Lanes,
        (false, false) => MissionTopology::SingleBranch,
    }
}

/// Whether a topology routes artifacts and status through a coordination ref.
///
/// This predicate is over the *stored* topology only; it is not a substitute
/// for [`classify_topology`], which names the whole-mission shape.
pub fn routes_through_coordination(topology: MissionTopology) -> bool {
    matches!(
        topology,
        MissionTopology::Coord | MissionTopology::LanesWithCoord
    )
}
