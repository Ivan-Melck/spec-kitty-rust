//! Canonical mission, coordination, lane, and worktree naming.
//!
//! One grammar, one derivation point. `mid8` is derived from
//! [`MissionId::mid8`] and never re-sliced from an arbitrary string. The
//! historical heuristic "an 8-character Crockford tail on a slug is a mid8" is
//! kept only as a documented fallback for legacy reconstruction, never for a
//! correctness path.
//!
//! This module is pure: it composes names and paths, and performs no
//! filesystem or Git I/O.

use std::path::{Path, PathBuf};

use sk_kernel::MissionSlug;
use sk_mission::{MID8_LENGTH, Mission, MissionId};

use crate::error::WorkspaceError;

/// Prefix of every mission branch.
pub const MISSION_BRANCH_PREFIX: &str = "kitty/mission-";

/// Root directory (relative to the repository root) holding lane worktrees.
pub const WORKTREES_DIRNAME: &str = ".worktrees";

/// Suffix of a coordination worktree directory.
pub const COORD_DIR_SUFFIX: &str = "-coord";

/// The canonical planning-artifact lane id.
///
/// A planning lane resolves to the repository-root checkout and never to a
/// `.worktrees/` directory.
pub const PLANNING_LANE_ID: &str = "lane-planning";

/// Crockford base32, exactly 8 characters (the ULID `mid8` shape).
const MID8_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Canonical mission identity: a slug plus a declared ULID.
///
/// The `mid8` is derived exactly once, at construction, from the declared
/// identity. An invalid or truncated identity is refused; it is never treated
/// as absent and never replaced by a slug tail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissionIdentity {
    slug: MissionSlug,
    mission_id: MissionId,
    mid8: String,
}

impl MissionIdentity {
    /// Derive the identity from a declared mission id.
    ///
    /// The `mid8` is `mission_id[..8]` by construction, so it cannot drift.
    pub fn derive(slug: MissionSlug, mission_id: MissionId) -> Self {
        let mid8 = mission_id.mid8().to_owned();
        Self {
            slug,
            mission_id,
            mid8,
        }
    }

    /// Derive the identity from a loaded mission.
    pub fn from_mission(mission: &Mission) -> Self {
        Self::derive(mission.slug().clone(), mission.id().clone())
    }

    /// The mission slug.
    pub fn slug(&self) -> &MissionSlug {
        &self.slug
    }

    /// The declared ULID.
    pub fn mission_id(&self) -> &MissionId {
        &self.mission_id
    }

    /// The canonical 8-character disambiguator.
    pub fn mid8(&self) -> &str {
        &self.mid8
    }

    /// The canonical mission directory name (`<human-slug>-<mid8>`).
    pub fn mission_dir_name(&self) -> String {
        mission_dir_name(self.slug.as_str(), &self.mid8)
    }

    /// The coordination worktree directory name (`<slug>-<mid8>-coord`).
    pub fn coord_dir_name(&self) -> String {
        coord_dir_name(self.slug.as_str(), &self.mid8)
    }

    /// The canonical mission integration branch.
    pub fn mission_branch_name(&self) -> String {
        mission_branch_name(self.slug.as_str(), Some(&self.mission_id))
    }

    /// The canonical coordination branch.
    pub fn coord_branch_name(&self) -> String {
        coord_branch_name(self.slug.as_str(), Some(&self.mission_id))
    }
}

/// Strip exactly `NNN-` from the start of a slug.
///
/// Two-digit, four-digit, and empty remainders are left untouched. This is the
/// canonical directory/branch grammar; coordination *reconstruction* uses
/// [`coord_mission_dir_name`], which deliberately does not strip.
pub fn strip_numeric_prefix(slug: &str) -> &str {
    let bytes = slug.as_bytes();
    if bytes.len() > 4 && bytes[..3].iter().all(u8::is_ascii_digit) && bytes[3] == b'-' {
        &slug[4..]
    } else {
        slug
    }
}

/// Is `value` the canonical 8-character Crockford `mid8` shape?
pub fn is_mid8_shape(value: &str) -> bool {
    value.len() == MID8_LENGTH
        && value
            .as_bytes()
            .iter()
            .all(|byte| MID8_ALPHABET.contains(byte))
}

/// Best-effort heuristic: the 8-character Crockford tail after the last hyphen.
///
/// This is **never authoritative**. It exists only for legacy reconstruction
/// after every declared source is exhausted. Use [`MissionIdentity`] whenever a
/// declared `mission_id` is available.
pub fn mid8_from_slug(slug: &str) -> Option<&str> {
    let (_head, tail) = slug.rsplit_once('-')?;
    is_mid8_shape(tail).then_some(tail)
}

/// The reference's `_idempotent_legacy_body` (#1949).
///
/// A slug that embeds an 8-character Crockford `mid8` tail also carries an
/// identity, so a stale `NNN-` prefix is stripped from it: that composes the
/// same resolvable `<human-slug>-<mid8>` body the `mission_id` path produces.
/// A *pure* legacy `NNN-` slug has no mid8, so its prefix is the identity and
/// is preserved byte-for-byte.
///
/// This is the rule for every name that must be idempotent with the
/// `mission_id` path — the mission branch and each code-lane branch. It must
/// **not** be applied to a reconstruction of an existing on-disk name (see
/// [`coord_mission_dir_name`]).
pub fn legacy_branch_body(slug: &str) -> &str {
    if mid8_from_slug(slug).is_some() {
        strip_numeric_prefix(slug)
    } else {
        slug
    }
}

/// The canonical `<human-slug>-<mid8>` mission directory name.
///
/// Idempotent on an already-suffixed slug and strips a stale `NNN-` prefix when
/// appending.
pub fn mission_dir_name(slug: &str, mid8: &str) -> String {
    let suffix = format!("-{mid8}");
    if slug.ends_with(&suffix) {
        return slug.to_owned();
    }
    format!("{}{suffix}", strip_numeric_prefix(slug))
}

/// The VERBATIM coordination mission directory name `<slug>-<mid8>`.
///
/// The coordination read/transaction path reconstructs an existing on-disk
/// name, so it must not strip a legacy `NNN-` prefix.
pub fn coord_mission_dir_name(slug: &str, mid8: &str) -> String {
    let suffix = format!("-{mid8}");
    if slug.ends_with(&suffix) {
        return slug.to_owned();
    }
    format!("{slug}{suffix}")
}

/// The coordination worktree directory name `<slug>-<mid8>-coord`.
pub fn coord_dir_name(slug: &str, mid8: &str) -> String {
    format!("{}{COORD_DIR_SUFFIX}", coord_mission_dir_name(slug, mid8))
}

/// The canonical mission integration branch for a known identity.
pub fn mission_branch_name(slug: &str, mission_id: Option<&MissionId>) -> String {
    match mission_id {
        Some(mission_id) => {
            let human = strip_numeric_prefix(slug);
            let suffix = format!("-{}", mission_id.mid8());
            let body = human.strip_suffix(&suffix).unwrap_or(human);
            format!("{MISSION_BRANCH_PREFIX}{body}{suffix}")
        }
        None => {
            format!("{MISSION_BRANCH_PREFIX}{}", legacy_branch_body(slug))
        }
    }
}

/// Compose the canonical mission branch, failing closed on a lost identity.
///
/// * declared `mission_id` → mid8-era branch;
/// * no id, legacy `NNN-` slug → legacy branch;
/// * no id, slug embeds an 8-character Crockford tail → legacy compose keeps
///   the embedded disambiguator;
/// * no id, modern slug → [`WorkspaceError::BranchIdentityUnresolved`].
pub fn mission_branch_name_required(
    slug: &str,
    mission_id: Option<&MissionId>,
) -> Result<String, WorkspaceError> {
    if mission_id.is_some() {
        return Ok(mission_branch_name(slug, mission_id));
    }
    let legacy = strip_numeric_prefix(slug) != slug;
    if legacy || mid8_from_slug(slug).is_some() {
        return Ok(mission_branch_name(slug, None));
    }
    Err(WorkspaceError::BranchIdentityUnresolved {
        mission: slug.to_owned(),
        detail: "mission_id is absent and the slug carries no mid8 disambiguator; pass mission_id from meta.json or run 'spec-kitty migrate backfill-identity'".to_owned(),
    })
}

/// The canonical coordination branch (mission-create grammar).
pub fn coord_branch_name(slug: &str, mission_id: Option<&MissionId>) -> String {
    mission_branch_name(slug, mission_id)
}

/// Reconstruct an EXISTING coordination branch VERBATIM (no `NNN-` strip).
pub fn coord_reconstruct_branch(slug: &str, mid8: &str) -> String {
    format!(
        "{MISSION_BRANCH_PREFIX}{}",
        coord_mission_dir_name(slug, mid8)
    )
}

fn validate_lane_id(lane_id: &str) -> Result<(), WorkspaceError> {
    let valid = lane_id
        .strip_prefix("lane-")
        .is_some_and(|body| !body.is_empty() && body.bytes().all(|byte| byte.is_ascii_lowercase()));
    if !valid {
        return Err(WorkspaceError::InvalidArtifact {
            artifact: "lane id",
            detail: format!("{lane_id:?} does not match 'lane-<lowercase letters>'"),
        });
    }
    Ok(())
}

/// The code-lane branch name `kitty/mission-<body>-<lane_id>`.
///
/// Refuses the canonical planning lane: a planning lane has no
/// `kitty/mission-…` branch.
///
/// `<body>` is [`legacy_branch_body`], the reference's
/// `_idempotent_legacy_body` rule: a stale `NNN-` prefix is stripped **only**
/// when the slug embeds its own mid8 tail. A pure legacy `057-slug` keeps its
/// prefix, which is the whole point: the previous unconditional strip composed
/// `kitty/mission-<slug>-<lane>` for a branch Git had created as
/// `kitty/mission-057-<slug>-<lane>`, so every lane-head stamp, allocation and
/// planning path silently addressed a branch that did not exist.
pub fn code_lane_branch_name(slug: &str, lane_id: &str) -> Result<String, WorkspaceError> {
    validate_lane_id(lane_id)?;
    if lane_id == PLANNING_LANE_ID {
        return Err(WorkspaceError::InvalidArtifact {
            artifact: "lane id",
            detail: "the planning lane resolves to the target branch, not a lane branch".to_owned(),
        });
    }
    Ok(format!(
        "{MISSION_BRANCH_PREFIX}{}-{lane_id}",
        legacy_branch_body(slug)
    ))
}

/// The branch a lane executes on.
///
/// The planning lane resolves to `target_branch`; every code lane resolves to
/// its own `kitty/mission-…` branch.
pub fn lane_branch_name(
    slug: &str,
    lane_id: &str,
    target_branch: &str,
) -> Result<String, WorkspaceError> {
    validate_lane_id(lane_id)?;
    if lane_id == PLANNING_LANE_ID {
        return Ok(target_branch.to_owned());
    }
    code_lane_branch_name(slug, lane_id)
}

/// The on-disk lane-worktree directory name `<slug>-<lane_id>`.
pub fn worktree_dir_name(slug: &str, lane_id: &str) -> Result<String, WorkspaceError> {
    validate_lane_id(lane_id)?;
    if lane_id == PLANNING_LANE_ID {
        return Err(WorkspaceError::RepoRootLaneHasNoWorktree {
            lane: lane_id.to_owned(),
        });
    }
    Ok(format!("{slug}-{lane_id}"))
}

/// The absolute lane-worktree path under `<repo_root>/.worktrees/`.
pub fn worktree_path(
    repo_root: &Path,
    slug: &str,
    lane_id: &str,
) -> Result<PathBuf, WorkspaceError> {
    Ok(repo_root
        .join(WORKTREES_DIRNAME)
        .join(worktree_dir_name(slug, lane_id)?))
}

/// The repository-root checkout path for a planning-lane write.
pub fn repo_root_lane_path(repo_root: &Path) -> PathBuf {
    repo_root.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> MissionId {
        value.parse().expect("valid ULID")
    }

    #[test]
    fn strip_only_three_digit_prefix() {
        assert_eq!(strip_numeric_prefix("083-foo"), "foo");
        assert_eq!(strip_numeric_prefix("001-bar-baz"), "bar-baz");
        assert_eq!(strip_numeric_prefix("08-foo"), "08-foo");
        assert_eq!(strip_numeric_prefix("1234-foo"), "1234-foo");
        assert_eq!(strip_numeric_prefix("083-"), "083-");
        assert_eq!(strip_numeric_prefix("foo"), "foo");
    }

    #[test]
    fn mission_branch_uses_declared_mid8() {
        let mission_id = id("01KNXQS9ATWWFXS3K5ZJ9E5008");
        assert_eq!(
            mission_branch_name("083-my-feature", Some(&mission_id)),
            "kitty/mission-my-feature-01KNXQS9"
        );
        assert_eq!(
            mission_branch_name("my-feature-01KNXQS9", Some(&mission_id)),
            "kitty/mission-my-feature-01KNXQS9"
        );
    }

    #[test]
    fn legacy_branch_form_is_preserved() {
        assert_eq!(
            mission_branch_name("057-my-feature", None),
            "kitty/mission-057-my-feature"
        );
        assert!(mission_branch_name_required("my-feature", None).is_err());
        assert!(mission_branch_name_required("057-my-feature", None).is_ok());
    }

    #[test]
    fn code_lane_body_matches_the_branch_grammar() {
        // A pure legacy `NNN-` slug keeps its prefix on the lane branch: the
        // branch Git created is `kitty/mission-057-lanehead-lane-a`, so stripping
        // the prefix addressed a branch that never existed.
        assert_eq!(
            code_lane_branch_name("057-lanehead", "lane-a").unwrap(),
            "kitty/mission-057-lanehead-lane-a"
        );
        // A slug that embeds its mid8 tail dedups a stale prefix, exactly like
        // the mission_id path.
        assert_eq!(
            code_lane_branch_name("057-lanehead-01KV6510", "lane-a").unwrap(),
            "kitty/mission-lanehead-01KV6510-lane-a"
        );
        assert_eq!(
            code_lane_branch_name("lanehead", "lane-a").unwrap(),
            "kitty/mission-lanehead-lane-a"
        );
        assert_eq!(
            legacy_branch_body("057-lanehead"),
            "057-lanehead",
            "a pure legacy prefix is the identity"
        );
    }

    #[test]
    fn dir_grammars_differ_on_purpose() {
        assert_eq!(mission_dir_name("057-foo", "01KV6510"), "foo-01KV6510");
        assert_eq!(
            coord_mission_dir_name("057-foo", "01KV6510"),
            "057-foo-01KV6510"
        );
        assert_eq!(coord_dir_name("foo", "01KV6510"), "foo-01KV6510-coord");
    }

    #[test]
    fn planning_lane_has_no_worktree_or_lane_branch() {
        assert_eq!(
            lane_branch_name("foo", PLANNING_LANE_ID, "main").unwrap(),
            "main"
        );
        assert!(worktree_dir_name("foo", PLANNING_LANE_ID).is_err());
        assert!(code_lane_branch_name("foo", PLANNING_LANE_ID).is_err());
    }

    #[test]
    fn lane_grammar_is_validated() {
        assert!(worktree_dir_name("foo", "lane-a").is_ok());
        assert!(worktree_dir_name("foo", "../escape").is_err());
        assert!(code_lane_branch_name("foo", "Lane-A").is_err());
    }

    #[test]
    fn identity_derives_mid8_once() {
        let mission_id = id("01KNXQS9ATWWFXS3K5ZJ9E5008");
        let identity = MissionIdentity::derive("my-feature".parse().unwrap(), mission_id.clone());
        assert_eq!(identity.mid8(), "01KNXQS9");
        assert_eq!(identity.mission_dir_name(), "my-feature-01KNXQS9");
        assert_eq!(
            identity.mission_branch_name(),
            "kitty/mission-my-feature-01KNXQS9"
        );
    }
}
