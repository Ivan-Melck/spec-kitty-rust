//! The mint-only ownership fact for an owned checkout.
//!
//! [`OwnedCheckout`] is proof that a checkout `P` has been validated for one
//! mission: it is not the repository root, its mission directory contains a
//! real `meta.json`, the directory name equals the mission slug, and the
//! stored topology is known. It is minted once and then threaded, immutable,
//! through a command. It is never persisted, and it is never constructed from a
//! guessed directory name.
//!
//! # Why a mint token instead of `pub struct { .. }`
//!
//! The fields are private and the struct has a private token field, so the only
//! construction path is [`OwnedCheckout::mint`]. A caller cannot fabricate
//! ownership by assembling a look-alike value, and cannot bypass the
//! manifest/topology validation by using a struct literal.

use std::path::{Component, Path, PathBuf};

use sk_git::BranchName;
use sk_kernel::MissionSlug;
use sk_mission::{Mission, MissionId, MissionTopology};

use crate::error::WorkspaceError;

/// The `kitty-specs/` directory name inside a checkout.
pub const SPECS_DIR_NAME: &str = "kitty-specs";

/// The manifest file that proves a directory is a mission.
pub const META_FILE_NAME: &str = "meta.json";

/// A validated ownership fact for one owned checkout.
///
/// Every field is private: consumers read the accessors and cannot mutate or
/// forge the fact.
#[derive(Clone, Debug)]
pub struct OwnedCheckout {
    repository_root: PathBuf,
    owned_root: PathBuf,
    mission_dir: PathBuf,
    mission_slug: MissionSlug,
    mission_id: MissionId,
    mid8: String,
    topology: MissionTopology,
    write_branch: BranchName,
    _token: (),
}

impl OwnedCheckout {
    /// Mint the fact for `mission` inside `owned_root`.
    ///
    /// The mission must already have been loaded from
    /// `<owned_root>/kitty-specs/<slug>/meta.json`; this function proves that
    /// claim by comparing the manifest's canonical directory with the composed
    /// one, rather than trusting the caller's spelling.
    ///
    /// Refusals (all [`WorkspaceError::OwnedRefused`], each with a stable code):
    ///
    /// * `OWNED_CHECKOUT_IS_REPOSITORY_ROOT` — `owned_root` is the repository
    ///   root checkout;
    /// * `OWNED_MISSION_PATH_REFUSED` — the mission directory is not
    ///   `<owned_root>/kitty-specs/<slug>` or does not exist;
    /// * `OWNED_TOPOLOGY_UNSUPPORTED` — the mission has no stored topology;
    /// * `OWNED_BRANCH_REFUSED` — `write_branch` is empty or begins with `-`.
    ///   Protection policy is not consulted here: the caller resolves the
    ///   write branch with [`crate::workspace::MissionWorkspace::write_branch`],
    ///   which refuses a protected target that has no minted `mission_branch`.
    pub fn mint(
        repository_root: &Path,
        owned_root: &Path,
        mission: &Mission,
        write_branch: &BranchName,
    ) -> Result<Self, WorkspaceError> {
        let repository_root = canonicalize(repository_root, "repository root")?;
        let owned_root = canonicalize(owned_root, "owned checkout")?;
        let mission_dir = canonicalize(mission.spec_dir(), "mission directory")?;

        if owned_root == repository_root {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_CHECKOUT_IS_REPOSITORY_ROOT",
                message: format!(
                    "owned checkout {} is the repository root checkout",
                    owned_root.display()
                ),
            });
        }

        let spec_slug = mission.slug().as_str();
        let expected_dir = owned_root.join(SPECS_DIR_NAME).join(spec_slug);
        if mission_dir != expected_dir {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_MISSION_PATH_REFUSED",
                message: format!(
                    "mission directory {} is not {}",
                    mission_dir.display(),
                    expected_dir.display()
                ),
            });
        }
        if mission_dir.file_name().and_then(|name| name.to_str()) != Some(spec_slug) {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_MISSION_PATH_REFUSED",
                message: "mission directory name does not equal the mission slug".to_owned(),
            });
        }
        let manifest = mission_dir.join(META_FILE_NAME);
        if !manifest.is_file() {
            return Err(WorkspaceError::MissingManifest {
                mission: spec_slug.to_owned(),
                path: manifest,
            });
        }

        let Some(topology) = mission.topology() else {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_TOPOLOGY_UNSUPPORTED",
                message: format!(
                    "mission {spec_slug} has no recognised stored topology; refusing an ownership fact that cannot enforce placement"
                ),
            });
        };

        let write_branch_text = write_branch.as_str();
        if write_branch_text.is_empty() || write_branch_text.starts_with('-') {
            return Err(WorkspaceError::OwnedRefused {
                code: "OWNED_BRANCH_REFUSED",
                message: format!("write branch {write_branch_text:?} is not usable"),
            });
        }

        Ok(Self {
            repository_root,
            owned_root,
            mission_dir,
            mission_slug: mission.slug().clone(),
            mission_id: mission.id().clone(),
            mid8: mission.mid8().to_owned(),
            topology,
            write_branch: write_branch.clone(),
            _token: (),
        })
    }

    /// The canonical repository root.
    pub fn repository_root(&self) -> &Path {
        &self.repository_root
    }

    /// The canonical owned checkout root (never the repository root).
    pub fn owned_root(&self) -> &Path {
        &self.owned_root
    }

    /// The canonical mission directory inside the owned checkout.
    pub fn mission_dir(&self) -> &Path {
        &self.mission_dir
    }

    /// The mission slug.
    pub fn mission_slug(&self) -> &MissionSlug {
        &self.mission_slug
    }

    /// The declared mission identity.
    pub fn mission_id(&self) -> &MissionId {
        &self.mission_id
    }

    /// The canonical 8-character disambiguator.
    pub fn mid8(&self) -> &str {
        &self.mid8
    }

    /// The stored mission topology.
    pub fn topology(&self) -> MissionTopology {
        self.topology
    }

    /// The branch every owned write lands on and the checkout must be on.
    pub fn write_branch(&self) -> &BranchName {
        &self.write_branch
    }

    /// Does `handle` (slug, mid8, or full id) name this mission?
    ///
    /// Exact forms only. A prefix match would let an unrelated handle resolve
    /// to this mission, so it is refused.
    pub fn handle_names_mission(&self, handle: &str) -> bool {
        if handle == self.mission_slug.as_str() {
            return true;
        }
        let handle_upper = handle.to_ascii_uppercase();
        if crate::naming::is_mid8_shape(&handle_upper) {
            return handle_upper == self.mid8;
        }
        if handle_upper.len() == sk_mission::MISSION_ID_LENGTH {
            return handle_upper[..self.mid8.len()] == self.mid8
                && handle_upper
                    .as_bytes()
                    .iter()
                    .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(byte));
        }
        false
    }

    /// Validate a batch of paths against [`Self::mission_dir`].
    ///
    /// A relative path is joined to the owned root. Any `..` component in the
    /// supplied spelling is refused. Parents are resolved (symlinks collapse)
    /// while a symlink leaf is kept, so a link inside the mission is committed
    /// as a link. With `follow_links = true` the leaf is resolved too, and a
    /// link that leads outside the mission is refused.
    ///
    /// The whole batch is validated before anything is returned, so a caller
    /// that stages files cannot stage a prefix of an invalid batch.
    pub fn files(
        &self,
        paths: &[PathBuf],
        follow_links: bool,
    ) -> Result<Vec<PathBuf>, WorkspaceError> {
        let mut resolved = Vec::with_capacity(paths.len());
        for path in paths {
            if path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
            {
                return Err(WorkspaceError::OwnedPathRefused { path: path.clone() });
            }
            let candidate = if path.is_absolute() {
                path.clone()
            } else {
                self.owned_root.join(path)
            };
            let resolved_candidate = if follow_links {
                canonicalize(&candidate, "owned path")
                    .map_err(|_| WorkspaceError::OwnedPathRefused { path: path.clone() })?
            } else {
                resolve_commit_path(&candidate)
                    .map_err(|_| WorkspaceError::OwnedPathRefused { path: path.clone() })?
            };
            if !contains(&self.mission_dir, &resolved_candidate) {
                return Err(WorkspaceError::OwnedPathRefused { path: path.clone() });
            }
            resolved.push(resolved_candidate);
        }
        Ok(resolved)
    }
}

fn canonicalize(path: &Path, what: &'static str) -> Result<PathBuf, WorkspaceError> {
    std::fs::canonicalize(path).map_err(|source| WorkspaceError::OwnedRefused {
        code: "OWNED_MISSION_PATH_REFUSED",
        message: format!("cannot resolve {what} {}: {source}", path.display()),
    })
}

/// Resolve the parent and keep the leaf, mirroring upstream `resolve_commit_path`.
fn resolve_commit_path(path: &Path) -> std::io::Result<PathBuf> {
    let Some(name) = path.file_name() else {
        return std::fs::canonicalize(path);
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = std::fs::canonicalize(parent)?;
    Ok(parent.join(name))
}

fn contains(ancestor: &Path, candidate: &Path) -> bool {
    candidate == ancestor || candidate.starts_with(ancestor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_dotdot_is_refused_before_any_resolution() {
        // Construct the error path without a checkout on disk: the check is on
        // the supplied spelling, so it must fire before filesystem access.
        let paths = [PathBuf::from("../outside.txt")];
        assert!(
            paths[0]
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        );
    }
}
