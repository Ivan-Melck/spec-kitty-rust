//! Canonical Mission-definition resolution for the acceptance collector.
//!
//! Source's acceptance path asks `get_mission_for_feature(feature_dir)` for the
//! Mission's resolved `mission.yaml`:
//!
//! * it finds the project root by walking **up** from the Mission directory to
//!   the nearest ancestor containing `.kittify`, and
//! * it resolves the recorded `mission_type` through the org-aware tier chain
//!   (project override → project legacy → org packs → global home → packaged
//!   built-in), and
//! * it swallows every `MissionError` on this path (`except MissionError:
//!   mission = None`), which is what makes "no mission resolved" a legitimate
//!   source fact rather than a crash.
//!
//! This module reproduces exactly those three steps through the existing owners:
//! [`sk_mission::resolve_mission_definition_for_mission_dir`] for the tier
//! chain, [`sk_kernel`] for the installed resource roots, and
//! [`sk_charter`]'s pack-root chain for the org tier. Nothing here invents a
//! tier, a path list, or a "ready" root: a provider that has nothing to offer
//! returns `None`, and the resolver reports its own typed not-found.
//!
//! # Known gap
//!
//! Source's last tier is not only a packaged `mission.yaml`: an activated
//! sparse `mission_types/<type>.yaml` registration yields a *neutral* identity
//! whose `path_conventions` slot may declare paths. That registration roster
//! belongs to the activation/doctrine surface (`sk-doctrine`), which this crate
//! does not depend on, so [`InstalledMissionProviders::registered_mission_type`]
//! returns `None` and such a Mission resolves exactly like an unregistered type
//! (Source's caught-`MissionError` no-op). The collector never claims the
//! registration's conventions; see the task report's open-functionality list.

use std::path::{Path, PathBuf};

use sk_charter::{Limits, PackRoots, PathExpansion, load_project_config};
use sk_kernel::KernelError;
use sk_kernel::paths::{resolve_path_non_strict, source_path_exists};
use sk_mission::{
    DefinitionError, DefinitionProviders, RegisteredMissionType, ResolvedMissionDefinition,
    SafeDocumentDecoder, resolve_mission_definition_for_mission_dir,
};

use crate::error::WorkspaceError;

/// Find the project root the way Source's `get_mission_for_feature` does.
///
/// Walks up from the resolved Mission directory to the nearest ancestor that
/// contains `.kittify`. Source's loop never inspects the filesystem root itself
/// (`while current != current.parent`), so neither does this one, and an
/// `os.path.exists` failure stays a typed error instead of being read as
/// "no project root".
///
/// # Errors
///
/// A path probe that fails for a reason other than absence (Source's
/// `Path.exists()` raises `OSError` for those) or a path that cannot be
/// resolved on this platform.
pub fn project_root_for_feature_dir(feature_dir: &Path) -> Result<Option<PathBuf>, WorkspaceError> {
    let mut current = resolve_path_non_strict(feature_dir)?;
    loop {
        let Some(parent) = current.parent().map(Path::to_path_buf) else {
            return Ok(None);
        };
        if parent == current {
            // The filesystem root: Source's loop condition stops here without
            // probing `<root>/.kittify`.
            return Ok(None);
        }
        let probe = current.join(".kittify");
        if source_path_exists(&probe).map_err(|source| {
            WorkspaceError::Kernel(KernelError::Io {
                action: "probe for a project root",
                path: probe.clone(),
                source,
            })
        })? {
            return Ok(Some(current));
        }
        current = parent;
    }
}

/// The provider composition Source's installed tiers resolve through.
///
/// * **org packs** — the project's configured `.kittify/config.yaml`
///   `charter_packs` chain, resolved by [`PackRoots::resolve_existing_source`]
///   (Source `resolve_pack_chain(strict=False)`: declared-but-absent roots are
///   dropped, and an unreadable registry contributes no roots instead of
///   raising).
/// * **global home** — [`sk_kernel::get_kittify_home`], i.e.
///   `SPEC_KITTY_HOME`/`~/.kittify` (`Source get_kittify_home()`).
/// * **packaged tree** — [`sk_kernel::get_package_asset_root`], the shipped
///   `packs/built-in/missions` tree (`Source _packaged_missions_dir()`), which
///   fails closed when the pinned pack tree has not been fetched.
/// * **registered mission types** — not wired; see the module documentation.
#[derive(Clone, Copy, Debug, Default)]
pub struct InstalledMissionProviders;

impl DefinitionProviders for InstalledMissionProviders {
    fn org_pack_roots(&self, project_root: &Path) -> Vec<PathBuf> {
        let Ok(config) = load_project_config(project_root, &Limits::default()) else {
            // Source's lenient org chain warns and contributes no roots for an
            // unreadable registry; it never fails the Mission lookup.
            return Vec::new();
        };
        let expansion = PathExpansion::from_env();
        match PackRoots::resolve_existing_source(project_root, Some(&config), &expansion, None) {
            Ok(roots) => roots
                .organizations
                .into_iter()
                .map(|organization| organization.root)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn registered_mission_type(
        &self,
        _project_root: &Path,
        _name: &str,
    ) -> Option<RegisteredMissionType> {
        None
    }

    fn packaged_missions_root(&self, _project_root: &Path) -> Option<PathBuf> {
        sk_kernel::get_package_asset_root()
            .ok()
            .filter(|root| root.is_dir())
    }

    fn kittify_home(&self, _project_root: &Path) -> Option<PathBuf> {
        sk_kernel::get_kittify_home().ok()
    }
}

/// Resolve the Mission definition Source's acceptance path would load.
///
/// Returns `Ok(None)` for every failure Source catches as `MissionError` (a
/// missing project root, an unresolved mission type, an unreadable or invalid
/// `mission.yaml`): on that path Source genuinely continues with `mission =
/// None`, so this is not a swallowed Native error. A read failure that Source
/// would *not* catch — `open()` raising `OSError` — stays a typed error.
///
/// # Errors
///
/// * [`WorkspaceError::InvalidArtifact`] when the resolved `mission.yaml`
///   exists but cannot be read (Source's uncaught `OSError`).
/// * [`WorkspaceError::Kernel`] when the project-root probe itself fails.
pub fn resolve_acceptance_definition(
    feature_dir: &Path,
    providers: &dyn DefinitionProviders,
    decoder: &dyn SafeDocumentDecoder,
) -> Result<Option<ResolvedMissionDefinition>, WorkspaceError> {
    let Some(project_root) = project_root_for_feature_dir(feature_dir)? else {
        return Ok(None);
    };
    match resolve_mission_definition_for_mission_dir(feature_dir, &project_root, providers, decoder)
    {
        Ok(resolved) => Ok(Some(resolved)),
        Err(DefinitionError::ReadFailed { path, detail }) => Err(WorkspaceError::InvalidArtifact {
            artifact: "mission.yaml",
            detail: format!("cannot read {}: {detail}", path.display()),
        }),
        // Source's caught `MissionError` family: "no mission resolved".
        Err(_) => Ok(None),
    }
}
