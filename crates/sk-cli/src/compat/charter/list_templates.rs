//! `charter list --all` template availability: the list-SPECIFIC tier roots, the
//! canonical catalog scan and the Source-shaped row composition.
//!
//! Mirrors `specify_cli.cli.commands.charter.list_cmd._template_tier_roots`,
//! `_render_templates` and the `--all` template branch of `list_cmd`. The discovery
//! itself is **not** here: it is the ONE CF30 canonical catalog
//! ([`sk_charter::template_catalog::discover_templates`]). This module only composes the
//! roots the *listing* supplies and renders the rows the listing publishes.
//!
//! # The list tier roots are not the include bases
//!
//! The reference keeps two different root policies and so does the Native port:
//!
//! * `--include template:<id>` resolves against the project/legacy/override **layer**
//!   selection (`resolve_source_project_layer`, which also probes the code-local
//!   `src/charter/offering` directory) — that is CF32's serving policy.
//! * `charter list --all` enumerates, in this order, (1) the project pack's `missions`
//!   directory under `.kittify/charter-packs/` (no code-local fallback, no
//!   `src/charter/offering` probe), (2) **every** configured existing org pack's flat
//!   `<pack>/missions` directory in declaration order, and (3) the package-default
//!   missions root. There are no `legacy`/`global_mission`/`global` tiers here, because
//!   the reference's listing does not enumerate them.
//!
//! Declaration order is the executable fact: [`sk_charter::PackRoots`] preserves the
//! configured order and [`discover_templates`] keeps the **first** supplied root for a
//! duplicated `<mission>/<name>`. The reference's own comment on the org loop claims
//! "last-declared wins"; that comment is wrong and this module follows the behaviour,
//! not the comment.
//!
//! # Errors
//!
//! The reference's `list` command catches `ValueError` (an unreadable project config, an
//! unexpandable org `local_path`) and reports it, and its packaged-root failure is not
//! caught. Both become a [`CompatError`] here; the manifest-read failure is not softened
//! into "no org packs" because the reference raises it too.

use std::path::Path;

use serde_json::{Value, json};

use sk_charter::template_catalog::{TemplateRef, TemplateTier, TierRoot, discover_templates};
use sk_charter::{Limits, PackRoots, PathExpansion, load_project_config};

use crate::compat::support::CompatError;

/// The reference's pseudo-kind label for the template row.
///
/// Templates are not a charter activation kind (there is no `activated_templates` list);
/// they are an availability-only row appended to the `--all` listing.
pub const TEMPLATE_KIND: &str = "template";

/// The reference's Activated cell for the template row, verbatim (em dash included).
pub const TEMPLATE_ACTIVATED_CELL: &str = "(mission-scoped — not separately activated)";

/// The reference's empty table cell.
const EMPTY_CELL: &str = "—";

/// The per-mission tree the project and org pack layers carry.
///
/// Package-default missions come from the installed `packs/built-in/missions` root, whose
/// own children are the same per-mission directories.
const MISSIONS_SUBDIR: &str = "missions";

/// Build the template-discovery tier roots the **listing** supplies, in precedence order.
///
/// Roots are: the project pack's `missions` (override tier, only when it is a directory),
/// one org root per configured existing org pack's `missions` (in declaration order), and
/// the package-default missions root (always appended; the reference's
/// `default_missions_root` fails closed when the installed leaf is missing, which
/// [`sk_kernel::get_package_asset_root`] reproduces).
///
/// # Errors
///
/// [`CompatError`] for an unreadable `.kittify/config.yaml`, an unexpandable declared
/// `local_path` (the reference's `ValueError` family) and a missing installed missions
/// root.
pub fn list_tier_roots(repo_root: &Path) -> Result<Vec<TierRoot>, CompatError> {
    let config = load_project_config(repo_root, &Limits::default())
        .map_err(|error| CompatError::domain(error.to_string()))?;
    let expansion = PathExpansion::from_env();
    let resolved = PackRoots::resolve_existing_source(repo_root, Some(&config), &expansion, None)
        .map_err(|error| CompatError::domain(error.to_string()))?;

    let mut tier_roots: Vec<TierRoot> = Vec::new();
    if let Some(project) = resolved.project.as_deref() {
        let missions = project.join(MISSIONS_SUBDIR);
        if missions.is_dir() {
            tier_roots
                .push(TierRoot::new(TemplateTier::Override, missions).with_project_dir(repo_root));
        }
    }
    for pack in &resolved.organizations {
        let missions = pack.root.join(MISSIONS_SUBDIR);
        if missions.is_dir() {
            tier_roots.push(TierRoot::new(TemplateTier::Org, missions));
        }
    }
    let package = sk_kernel::get_package_asset_root()
        .map_err(|error| CompatError::domain(error.to_string()))?;
    tier_roots.push(TierRoot::new(TemplateTier::PackageDefault, package));
    Ok(tier_roots)
}

/// Discover the templates the listing publishes, through the canonical catalog.
///
/// # Errors
///
/// [`list_tier_roots`]' failures plus any catalog scan refusal (an unreadable directory,
/// a mission or template name that is not valid UTF-8).
pub fn list_templates(repo_root: &Path) -> Result<Vec<TemplateRef>, CompatError> {
    let tier_roots = list_tier_roots(repo_root)?;
    discover_templates(&tier_roots).map_err(|error| CompatError::domain(error.to_string()))
}

/// The reference's `payload["templates"]` rows, in catalog order.
///
/// Exactly the four reference keys: `template_id`, `mission`, `name` and `tier` (the
/// Source `ResolutionTier.value` spelling — underscored, never a hyphenated label).
#[must_use]
pub fn template_payload(references: &[TemplateRef]) -> Value {
    Value::Array(
        references
            .iter()
            .map(|reference| {
                json!({
                    "template_id": reference.template_id,
                    "mission": reference.mission,
                    "name": reference.name,
                    "tier": reference.tier.as_str(),
                })
            })
            .collect(),
    )
}

/// Render the human `available` cell of the template row from the published rows.
///
/// The reference renders the table cell and the JSON payload from the same
/// `template_refs` list; this takes that list's JSON rows so the text view and `--json`
/// cannot drift or re-scan the tiers. Each entry is `<template_id> (<tier>)`. The
/// reference wraps the tier in Rich `[dim]…[/dim]` markup; the Native text view is a
/// plain-line renderer, so the markup is not reproduced (it is disclosed, not imitated).
#[must_use]
pub fn template_text(rows: &Value) -> String {
    let Some(rows) = rows.as_array() else {
        return EMPTY_CELL.to_owned();
    };
    if rows.is_empty() {
        return EMPTY_CELL.to_owned();
    }
    rows.iter()
        .map(|row| {
            let template_id = row
                .get("template_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let tier = row.get("tier").and_then(Value::as_str).unwrap_or_default();
            format!("{template_id} ({tier})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use sk_charter::template_catalog::{TemplateTier, discover_templates};

    use super::{list_templates, list_tier_roots, template_payload, template_text};

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent");
        }
        fs::write(path, contents).expect("fixture write");
    }

    /// A project with a project pack, two declared existing org packs and one absent one.
    fn fixture() -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path();
        write(
            &root.join(".kittify/config.yaml"),
            "charter_packs:\n  org:\n    packs:\n\
             \x20     - name: org-first\n        local_path: org-first\n\
             \x20     - name: org-server\n        local_path: org-not-fetched\n\
             \x20     - name: org-second\n        local_path: org-second\n",
        );
        write(
            &root.join(".kittify/charter-packs/missions/alpha/templates/a.md"),
            "project alpha\n",
        );
        write(
            &root.join(".kittify/charter-packs/missions/shared/templates/same.md"),
            "project shared\n",
        );
        write(
            &root.join("org-first/missions/shared/templates/same.md"),
            "org-first shared\n",
        );
        write(
            &root.join("org-second/missions/shared/templates/same.md"),
            "org-second shared\n",
        );
        write(
            &root.join("org-second/missions/beta/command-templates/implement.md"),
            "org command\n",
        );
        write(
            &root.join("org-second/missions/beta/templates/README.md"),
            "not a template\n",
        );
        write(
            &root.join("org-second/missions/beta/templates/nested/deep.md"),
            "nested\n",
        );
        temp
    }

    #[test]
    fn tier_roots_are_project_then_each_existing_org_pack_then_the_package() {
        let temp = fixture();
        let roots = list_tier_roots(temp.path()).expect("roots");
        assert_eq!(roots.len(), 4, "{roots:?}");
        assert_eq!(roots[0].tier, TemplateTier::Override);
        assert_eq!(
            roots[0].missions_root,
            temp.path().join(".kittify/charter-packs").join("missions")
        );
        assert_eq!(roots[0].project_dir.as_deref(), Some(temp.path()));
        // Declaration order, and the declared-but-absent pack is dropped.
        assert_eq!(roots[1].tier, TemplateTier::Org);
        assert_eq!(
            roots[1].missions_root,
            temp.path().join("org-first/missions")
        );
        assert_eq!(roots[2].tier, TemplateTier::Org);
        assert_eq!(
            roots[2].missions_root,
            temp.path().join("org-second/missions")
        );
        assert_eq!(roots[3].tier, TemplateTier::PackageDefault);
        assert!(roots[3].missions_root.is_dir());
    }

    #[test]
    fn the_first_declared_org_pack_wins_a_duplicated_template() {
        let temp = fixture();
        let roots = list_tier_roots(temp.path()).expect("roots");
        let references = discover_templates(&roots).expect("catalog");
        let shared = references
            .iter()
            .find(|reference| reference.template_id == "shared/same.md")
            .expect("duplicate id");
        // The project layer outranks both org packs.
        assert_eq!(shared.tier, TemplateTier::Override);
        assert!(
            shared
                .path
                .starts_with(temp.path().join(".kittify/charter-packs"))
        );

        // With the project layer absent, the FIRST declared org pack wins — not the
        // last, despite the reference's own misleading "last-declared wins" comment.
        let only_org: Vec<_> = roots
            .into_iter()
            .filter(|root| root.tier != TemplateTier::Override)
            .collect();
        let references = discover_templates(&only_org).expect("catalog");
        let shared = references
            .iter()
            .find(|reference| reference.template_id == "shared/same.md")
            .expect("duplicate id");
        assert_eq!(shared.tier, TemplateTier::Org);
        assert!(shared.path.starts_with(temp.path().join("org-first")));
    }

    #[test]
    fn discovery_walks_command_templates_recursively_and_skips_readmes() {
        let temp = fixture();
        let references = list_templates(temp.path()).expect("catalog");
        let ids: Vec<&str> = references
            .iter()
            .map(|reference| reference.template_id.as_str())
            .collect();
        assert!(ids.contains(&"beta/implement.md"), "{ids:?}");
        assert!(ids.contains(&"beta/deep.md"), "{ids:?}");
        assert!(!ids.iter().any(|id| id.ends_with("README.md")), "{ids:?}");
        assert!(!ids.iter().any(|id| id.contains("nested/")), "{ids:?}");
        // Cross-mission same-basename templates stay two distinct references.
        assert!(ids.contains(&"alpha/a.md"), "{ids:?}");
        assert!(ids.contains(&"software-dev/spec-template.md"), "{ids:?}");
        // Sorted by (template_id, tier) like the reference.
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn payload_rows_carry_the_reference_keys_and_value_spellings() {
        let temp = fixture();
        let references = list_templates(temp.path()).expect("catalog");
        let payload = template_payload(&references);
        let rows = payload.as_array().expect("rows");
        assert_eq!(rows.len(), references.len());
        let alpha = rows
            .iter()
            .find(|row| row["template_id"] == "alpha/a.md")
            .expect("alpha");
        assert_eq!(alpha["mission"], "alpha");
        assert_eq!(alpha["name"], "a.md");
        assert_eq!(alpha["tier"], "override");
        let _: PathBuf = temp.path().to_path_buf();
        let text = template_text(&payload);
        assert!(text.contains("alpha/a.md (override)"), "{text}");
        assert!(!text.contains("global"), "{text}");
        assert_eq!(template_text(&serde_json::json!([])), "—");
    }
}
