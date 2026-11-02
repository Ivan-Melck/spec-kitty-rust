//! Surface vocabulary and the total surface→filesystem translation.
//!
//! Upstream splits placement into two layers: an artifact *home* (a declared
//! surface) and a materialization-aware *resolved surface* (a real directory
//! plus the stamp naming which tree it is). This module owns both, plus the
//! one total translation from a surface name to a path.
//!
//! `TopologySurface` deliberately has five members: `Primary`, `Coord`,
//! `Lane`, `Consolidated`, and `Temp`. Every member maps to a real location,
//! so none is a phantom: a caller that cannot supply a location for the
//! requested member gets [`WorkspaceError::SurfaceUnresolved`] rather than a
//! guessed path.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sk_mission::MissionTopology;

use crate::artifact::{MissionArtifactKind, declared_read_surface};
use crate::error::WorkspaceError;
use crate::lifecycle::LifecyclePhase;

/// A physical tree a mission artifact resolves to.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum TopologySurface {
    /// The primary mission directory on the target branch.
    Primary,
    /// The mission's coordination worktree.
    Coord,
    /// A lane worktree.
    Lane,
    /// The integrated tree after consolidation.
    Consolidated,
    /// A caller-owned temporary directory.
    Temp,
}

impl TopologySurface {
    /// Every surface, in declaration order.
    pub const ALL: [Self; 5] = [
        Self::Primary,
        Self::Coord,
        Self::Lane,
        Self::Consolidated,
        Self::Temp,
    ];

    /// The canonical wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Coord => "coord",
            Self::Lane => "lane",
            Self::Consolidated => "consolidated",
            Self::Temp => "temp",
        }
    }

    /// The [`SurfaceLocations`] field this surface translates through.
    ///
    /// This is the one surface→field map. Because the match is exhaustive,
    /// adding an enum member without an entry is a compile error, which is a
    /// stronger guarantee than the upstream runtime totality assertion.
    pub fn location_field(self) -> SurfaceLocationField {
        match self {
            Self::Primary => SurfaceLocationField::Primary,
            Self::Coord => SurfaceLocationField::Coord,
            Self::Lane => SurfaceLocationField::Lane,
            Self::Consolidated => SurfaceLocationField::Consolidated,
            Self::Temp => SurfaceLocationField::Temp,
        }
    }
}

impl std::fmt::Display for TopologySurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The name of the [`SurfaceLocations`] slot a surface reads.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SurfaceLocationField {
    /// `SurfaceLocations::primary`
    Primary,
    /// `SurfaceLocations::coord`
    Coord,
    /// `SurfaceLocations::lane`
    Lane,
    /// `SurfaceLocations::consolidated`
    Consolidated,
    /// `SurfaceLocations::temp`
    Temp,
}

/// A totality violation in the surface vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SurfaceTotalityViolation {
    /// An enum member has no translation entry.
    #[error("phantom TopologySurface member(s) with no translation: {0:?}")]
    Missing(Vec<TopologySurface>),
    /// A translation entry names no enum member.
    #[error("translation entry for non-member surface(s): {0:?}")]
    Surplus(Vec<String>),
    /// The enum repeats a member.
    #[error("duplicate TopologySurface member(s): {0:?}")]
    Duplicate(Vec<TopologySurface>),
}

/// Assert the surface vocabulary is exactly the translated set.
///
/// Returns a typed error instead of panicking. Every production translation
/// path is exhaustive by construction, so this is an additional guard for
/// serialized input and for future code that enumerates the enum dynamically.
pub fn assert_surface_totality() -> Result<(), SurfaceTotalityViolation> {
    let mut seen = BTreeSet::new();
    let mut duplicates = Vec::new();
    for surface in TopologySurface::ALL {
        if !seen.insert(surface) {
            duplicates.push(surface);
        }
    }
    if !duplicates.is_empty() {
        return Err(SurfaceTotalityViolation::Duplicate(duplicates));
    }
    let translated: BTreeSet<SurfaceLocationField> = TopologySurface::ALL
        .iter()
        .map(|surface| surface.location_field())
        .collect();
    let all_fields = [
        SurfaceLocationField::Primary,
        SurfaceLocationField::Coord,
        SurfaceLocationField::Lane,
        SurfaceLocationField::Consolidated,
        SurfaceLocationField::Temp,
    ];
    if translated.len() != all_fields.len() {
        let missing = TopologySurface::ALL
            .into_iter()
            .filter(|surface| !translated.contains(&surface.location_field()))
            .collect();
        return Err(SurfaceTotalityViolation::Missing(missing));
    }
    Ok(())
}

/// The concrete location candidates for a mission's surfaces.
///
/// `primary` is always known. The others are populated only when the caller
/// has resolved that surface; a `None` slot is refused by
/// [`translate_surface`] rather than guessed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SurfaceLocations {
    primary: Option<PathBuf>,
    coord: Option<PathBuf>,
    lane: Option<PathBuf>,
    consolidated: Option<PathBuf>,
    temp: Option<PathBuf>,
}

impl SurfaceLocations {
    /// Locations with only the primary mission directory resolved.
    pub fn primary(primary: impl Into<PathBuf>) -> Self {
        Self {
            primary: Some(primary.into()),
            ..Self::default()
        }
    }

    /// Set the coordination directory.
    pub fn with_coord(mut self, coord: impl Into<PathBuf>) -> Self {
        self.coord = Some(coord.into());
        self
    }

    /// Set the lane directory.
    pub fn with_lane(mut self, lane: impl Into<PathBuf>) -> Self {
        self.lane = Some(lane.into());
        self
    }

    /// Set the integrated post-consolidation directory.
    pub fn with_consolidated(mut self, consolidated: impl Into<PathBuf>) -> Self {
        self.consolidated = Some(consolidated.into());
        self
    }

    /// Set the temporary directory.
    pub fn with_temp(mut self, temp: impl Into<PathBuf>) -> Self {
        self.temp = Some(temp.into());
        self
    }

    /// The raw location for `field`, if resolved.
    pub fn get(&self, field: SurfaceLocationField) -> Option<&Path> {
        match field {
            SurfaceLocationField::Primary => self.primary.as_deref(),
            SurfaceLocationField::Coord => self.coord.as_deref(),
            SurfaceLocationField::Lane => self.lane.as_deref(),
            SurfaceLocationField::Consolidated => self.consolidated.as_deref(),
            SurfaceLocationField::Temp => self.temp.as_deref(),
        }
    }

    /// The resolved location for `surface`.
    pub fn location(&self, surface: TopologySurface) -> Option<&Path> {
        self.get(surface.location_field())
    }
}

/// Translate a surface to its filesystem location.
///
/// Total over the surface vocabulary. Refuses with
/// [`WorkspaceError::SurfaceUnresolved`] when the caller did not supply this
/// surface's location; it never fabricates a path.
pub fn translate_surface(
    surface: TopologySurface,
    locations: &SurfaceLocations,
) -> Result<&Path, WorkspaceError> {
    locations
        .location(surface)
        .ok_or(WorkspaceError::SurfaceUnresolved { surface })
}

/// A resolved surface plus the stamp naming which physical tree it is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSurface {
    /// Where the artifact is read.
    pub path: PathBuf,
    /// The declared tree the path belongs to.
    pub surface_kind: TopologySurface,
}

/// The materialization state of a mission's coordination surface.
///
/// This mirrors upstream `CoordState`, including the distinction a naive
/// boolean cannot express: a declared branch that was **deleted** is a
/// data-loss refusal, while a declared branch whose worktree has not been
/// materialised yet is a *materialize-me* refusal. Neither may silently read
/// or write the primary checkout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordState {
    /// The topology declares no coordination branch.
    None,
    /// The branch exists but no worktree is registered.
    Empty,
    /// The branch exists but the worktree was removed from the registry.
    Unmaterialized,
    /// The branch and its worktree are present.
    Materialized,
    /// The declared branch no longer exists in git.
    Deleted,
}

/// Inputs to the materialization-aware surface resolution.
#[derive(Clone, Debug)]
pub struct PlacementInputs<'a> {
    /// The artifact kind being read.
    pub kind: MissionArtifactKind,
    /// Mission slug, used only in diagnostics.
    pub mission_slug: &'a str,
    /// The mission's durable stored topology.
    pub topology: MissionTopology,
    /// The primary mission directory.
    pub primary_dir: &'a Path,
    /// The declared coordination branch, used for refusal diagnostics.
    pub coordination_branch: Option<&'a str>,
    /// The materialization state of the coordination surface.
    pub coord_state: CoordState,
    /// The materialized coordination directory, required when
    /// `coord_state` is [`CoordState::Materialized`].
    pub coord_dir: Option<&'a Path>,
    /// The derived lifecycle phase.
    pub phase: LifecyclePhase,
}

/// Resolve the affirmative read surface for an artifact kind.
///
/// This is total over `(kind, topology, coord_state)`:
///
/// * a PRIMARY-partition kind resolves the primary directory, stamped
///   `PRIMARY`, for every topology and coordination state;
/// * a COORD-partition kind on a coord-less topology resolves the primary
///   directory affirmatively (its declared home), stamped `PRIMARY`;
/// * a COORD-partition kind on a coordination-routing topology consumes the
///   four-state classifier: `DELETED` and `UNMATERIALIZED` refuse, `MATERIALIZED`
///   resolves the coordination directory stamped `COORD`, and `EMPTY`/`NONE`
///   resolve the primary directory stamped `PRIMARY` (the declared create-window
///   answer, never a silent substitution for a write).
pub fn resolve_artifact_surface(
    inputs: &PlacementInputs<'_>,
) -> Result<ResolvedSurface, WorkspaceError> {
    let declared = declared_read_surface(inputs.kind, inputs.topology);
    if declared == TopologySurface::Primary {
        return Ok(ResolvedSurface {
            path: inputs.primary_dir.to_path_buf(),
            surface_kind: TopologySurface::Primary,
        });
    }

    match inputs.coord_state {
        CoordState::Deleted => Err(WorkspaceError::CoordinationBranchDeleted {
            mission: inputs.mission_slug.to_owned(),
            branch: inputs
                .coordination_branch
                .map(str::to_owned)
                .unwrap_or_else(|| "<declared>".to_owned()),
        }),
        CoordState::Unmaterialized => Err(WorkspaceError::CoordinationWorktreeUnmaterialized {
            mission: inputs.mission_slug.to_owned(),
            branch: inputs
                .coordination_branch
                .map(str::to_owned)
                .unwrap_or_else(|| "<declared>".to_owned()),
        }),
        CoordState::Materialized => {
            let coord = inputs
                .coord_dir
                .ok_or_else(|| WorkspaceError::InvalidArtifact {
                    artifact: "coord surface",
                    detail:
                        "CoordState::Materialized was supplied without a coordination directory"
                            .to_owned(),
                })?;
            Ok(ResolvedSurface {
                path: coord.to_path_buf(),
                surface_kind: TopologySurface::Coord,
            })
        }
        CoordState::None | CoordState::Empty => Ok(ResolvedSurface {
            path: inputs.primary_dir.to_path_buf(),
            surface_kind: TopologySurface::Primary,
        }),
    }
}

/// The lifecycle-aware consolidated location, if consolidation happened.
///
/// `PRE_CONSOLIDATION` has no consolidated tree, so `translate_surface` keeps
/// refusing `CONSOLIDATED` before any consolidation exists. Once E1/E2 is
/// reached, the integrated tree is the primary mission directory.
pub fn consolidated_location(primary_dir: &Path, phase: LifecyclePhase) -> Option<PathBuf> {
    match phase {
        LifecyclePhase::PreConsolidation => None,
        LifecyclePhase::Consolidated | LifecyclePhase::Published => Some(primary_dir.to_path_buf()),
    }
}

/// The one ref planning artifacts and status events resolve to.
///
/// A ref-only carrier, matching upstream `CommitTarget`. It is validated at
/// construction so a caller cannot smuggle an empty string or a control
/// character into a later Git call.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommitTarget(String);

impl CommitTarget {
    /// Validate and construct a commit target.
    pub fn new(reference: impl Into<String>) -> Result<Self, WorkspaceError> {
        let reference = reference.into();
        let valid = !reference.is_empty()
            && reference.trim() == reference
            && !reference.starts_with('-')
            && !reference.chars().any(|character| character.is_control());
        if !valid {
            return Err(WorkspaceError::InvalidArtifact {
                artifact: "commit target",
                detail: format!("{reference:?} is not a usable ref name"),
            });
        }
        Ok(Self(reference))
    }

    /// The ref name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CommitTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A validation error for [`PlacementRef`].
#[derive(Debug, thiserror::Error)]
pub enum PlacementRefError {
    /// The reference failed [`CommitTarget`] validation.
    #[error(transparent)]
    Invalid(#[from] WorkspaceError),
    /// The reference is not a valid branch name.
    #[error("placement ref {reference:?} is not a valid branch name: {detail}")]
    InvalidBranch {
        /// The supplied ref.
        reference: String,
        /// Git's rejection reason.
        detail: String,
    },
}

/// The single destination ref a placement seam carries.
///
/// Unlike a bare [`CommitTarget`] this is guaranteed to be a syntactically
/// valid local branch name, so it can be handed to `sk-git` without a second
/// check.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlacementRef {
    branch: sk_git::BranchName,
}

impl PlacementRef {
    /// Validate a branch name as a placement ref.
    pub fn parse(reference: &str) -> Result<Self, PlacementRefError> {
        CommitTarget::new(reference)?;
        let branch = sk_git::BranchName::parse(reference).map_err(|error| {
            PlacementRefError::InvalidBranch {
                reference: reference.to_owned(),
                detail: error.to_string(),
            }
        })?;
        Ok(Self { branch })
    }

    /// The validated branch.
    pub fn branch(&self) -> &sk_git::BranchName {
        &self.branch
    }

    /// The short branch name.
    pub fn as_str(&self) -> &str {
        self.branch.as_str()
    }

    /// The equivalent commit target.
    pub fn commit_target(&self) -> CommitTarget {
        CommitTarget(self.branch.as_str().to_owned())
    }
}

impl std::fmt::Display for PlacementRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
