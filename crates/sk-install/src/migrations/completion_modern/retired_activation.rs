//! The shared consumer-retirement engine for activatable doctrine ids.
//!
//! Ports `specify_cli/upgrade/migrations/_retired_activation.py`: one
//! data-driven engine, parameterised by [`Retirement`] records, that removes a
//! retired `activated_<kind>` stem (and, where declared, activates its
//! successor) across every project surface that can still carry it:
//!
//! * `.kittify/config.yaml` — the `activated_<kind>` member list;
//! * `.kittify/charter/charter.yaml` — the same list, a `catalog` block list
//!   matched by `id`, and the nested `governance.charter.selected_<kind>` list;
//! * `.kittify/charter/references.yaml` — a `references` block list;
//! * `.kittify/charter/interview/answers.yaml` — the `selected_<kind>` list.
//!
//! Every edit goes through the one round-trip CST editor
//! ([`crate::yaml_edit::CstDocument`]), so authored comments, quotes, key order
//! and layout survive. A file that exists but cannot be parsed as a mapping is
//! skipped and reported once as a warning — never fatal, exactly like the
//! Source engine, because the fail-closed charter compiler would otherwise trip
//! over the retired id it still carries.
//!
//! A retirement that still resolves through a configured org pack is skipped
//! entirely: the project can legitimately keep using the id.

use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;
use crate::yaml_edit::{CstDocument, CstMapping, CstSequence, CstValue};

/// One activatable doctrine id being retired from consumer projects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Retirement {
    /// The literal `activated_<kind>` key the stem lives under.
    pub kind_key: &'static str,
    /// The bare config-id stem.
    pub stem: &'static str,
    /// The catalog/reference id prefix (`TACTIC`, `STYLEGUIDE`, …).
    pub reference_prefix: &'static str,
    /// `(activated_<kind>, stem)` successors to activate in its place.
    pub successors: &'static [(&'static str, &'static str)],
}

impl Retirement {
    /// The full catalog/reference id (`TACTIC:locality-of-change`).
    pub fn reference_id(&self) -> String {
        format!("{}:{}", self.reference_prefix, self.stem)
    }
}

/// One project surface the engine may touch.
struct Surface {
    relative_path: &'static str,
    member_key: Option<String>,
    block_key: Option<&'static str>,
    successor_eligible: bool,
    successor_key_prefix: Option<&'static str>,
}

const CONFIG_REL: &str = ".kittify/config.yaml";
const CHARTER_REL: &str = ".kittify/charter/charter.yaml";
const REFERENCES_REL: &str = ".kittify/charter/references.yaml";
const ANSWERS_REL: &str = ".kittify/charter/interview/answers.yaml";
const GOVERNANCE_PREFIX: &str = "governance.charter.";

fn selected_key(kind_key: &str) -> String {
    format!(
        "selected_{}",
        kind_key.strip_prefix("activated_").unwrap_or(kind_key)
    )
}

fn surfaces_for(retirement: &Retirement, include_answers: bool) -> Vec<Surface> {
    let governance_member = format!("{GOVERNANCE_PREFIX}{}", selected_key(retirement.kind_key));
    let mut surfaces = vec![
        Surface {
            relative_path: CONFIG_REL,
            member_key: Some(retirement.kind_key.to_string()),
            block_key: None,
            successor_eligible: true,
            successor_key_prefix: None,
        },
        Surface {
            relative_path: CHARTER_REL,
            member_key: Some(retirement.kind_key.to_string()),
            block_key: Some("catalog"),
            successor_eligible: true,
            successor_key_prefix: None,
        },
        Surface {
            relative_path: CHARTER_REL,
            member_key: Some(governance_member),
            block_key: None,
            successor_eligible: true,
            successor_key_prefix: Some(GOVERNANCE_PREFIX),
        },
        Surface {
            relative_path: REFERENCES_REL,
            member_key: None,
            block_key: Some("references"),
            successor_eligible: false,
            successor_key_prefix: None,
        },
    ];
    if include_answers {
        surfaces.push(Surface {
            relative_path: ANSWERS_REL,
            member_key: Some(selected_key(retirement.kind_key)),
            block_key: None,
            successor_eligible: false,
            successor_key_prefix: None,
        });
    }
    surfaces
}

/// Resolve a plain or dotted member key to the list it names, never creating
/// anything.
fn resolve_member_list(root: &CstMapping, key: &str) -> Option<CstSequence> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut mapping = root.clone();
    for part in &parts[..parts.len() - 1] {
        mapping = mapping.get_mapping(part)?;
    }
    mapping.get_sequence(parts[parts.len() - 1])
}

/// Resolve a block list in either shape: the flat list, or `{references: [...]}`.
fn resolve_block_list(root: &CstMapping, key: &str) -> Option<CstSequence> {
    if let Some(sequence) = root.get_sequence(key) {
        return Some(sequence);
    }
    root.get_mapping(key)?.get_sequence("references")
}

/// Remove every occurrence of `stem` from the list named by `key`.
fn remove_member(root: &CstMapping, key: &str, stem: &str) -> bool {
    let Some(sequence) = resolve_member_list(root, key) else {
        return false;
    };
    let mut removed = false;
    let mut index = 0usize;
    while index < sequence.len() {
        let matches = match sequence.values().get(index) {
            Some(CstValue::Scalar(value)) => value == stem,
            _ => false,
        };
        if matches {
            sequence.remove(index);
            removed = true;
        } else {
            index += 1;
        }
    }
    removed
}

/// Remove every block whose `id` equals `block_id` from the block list at `key`.
fn remove_block(root: &CstMapping, key: &str, block_id: &str) -> bool {
    let Some(sequence) = resolve_block_list(root, key) else {
        return false;
    };
    sequence.remove_mapping_ids("id", block_id) > 0
}

fn append_successors(
    root: &CstMapping,
    successors: &[(&str, &str)],
    key_prefix: Option<&str>,
) -> Vec<(String, String)> {
    let mut added = Vec::new();
    for (target_key, target_stem) in successors {
        let resolved = match key_prefix {
            Some(prefix) => format!("{prefix}{}", selected_key(target_key)),
            None => (*target_key).to_string(),
        };
        if let Some(sequence) = resolve_member_list(root, &resolved) {
            let present = sequence
                .string_values()
                .iter()
                .any(|value| value == target_stem);
            if !present {
                sequence.push_string(target_stem);
                added.push((resolved, (*target_stem).to_string()));
            }
        }
    }
    added
}

fn load_document(path: &Path, warnings: &mut Vec<String>) -> Option<CstDocument> {
    let text = crate::migrations::completion_legacy::paths::read_text(path)?;
    match CstDocument::parse(path, &text) {
        Ok(document) => Some(document),
        Err(_) => {
            let message = format!(
                "{} could not be parsed as YAML and was skipped; retired ids in it were not migrated",
                path.to_string_lossy()
            );
            if !warnings.contains(&message) {
                warnings.push(message);
            }
            None
        }
    }
}

fn write_document(path: &Path, document: &CstDocument) -> Result<(), InstallError> {
    let text = if document.keys().is_empty() {
        "{}\n".to_string()
    } else {
        document.text()
    };
    if path.is_symlink() {
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                "refusing to write through the symlink {}; replace or remove it first",
                path.display()
            ),
        });
    }
    crate::migrations::completion_legacy::paths::write_text(path, &text).map(|_| ())
}

fn surface_needs_change(root: &CstMapping, retirement: &Retirement, surface: &Surface) -> bool {
    if let Some(key) = &surface.member_key
        && resolve_member_list(root, key).is_some_and(|sequence| {
            sequence
                .string_values()
                .iter()
                .any(|value| value == retirement.stem)
        })
    {
        return true;
    }
    match surface.block_key {
        Some(key) => resolve_block_list(root, key).is_some_and(|sequence| {
            sequence
                .values()
                .iter()
                .any(|value| matches!(value, CstValue::Mapping(mapping, _)
                    if mapping.get_string("id").as_deref() == Some(retirement.reference_id().as_str())))
        }),
        None => false,
    }
}

/// Whether at least one retirement still has a removable presence.
pub fn detect_retirements(
    project_root: &Path,
    retirements: &[Retirement],
    include_answers: bool,
) -> Result<bool, InstallError> {
    let mut warnings = Vec::new();
    for retirement in retirements {
        if still_resolves_via_org_pack(project_root, retirement) {
            continue;
        }
        for surface in surfaces_for(retirement, include_answers) {
            let path = project_root.join(surface.relative_path);
            let Some(document) = load_document(&path, &mut warnings) else {
                continue;
            };
            if surface_needs_change(&document.root(), retirement, &surface) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Remove every retired id's presence (and activate its successors).
pub fn apply_retirements(
    project_root: &Path,
    retirements: &[Retirement],
    include_answers: bool,
) -> Result<MigrationOutcome, InstallError> {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    for retirement in retirements {
        if still_resolves_via_org_pack(project_root, retirement) {
            continue;
        }
        for surface in surfaces_for(retirement, include_answers) {
            let path = project_root.join(surface.relative_path);
            let Some(document) = load_document(&path, &mut warnings) else {
                continue;
            };
            let root = document.root();
            let mut touched: Vec<String> = Vec::new();
            let member_removed = match &surface.member_key {
                Some(key) if remove_member(&root, key, retirement.stem) => {
                    touched.push(key.clone());
                    true
                }
                _ => false,
            };
            if let Some(key) = surface.block_key
                && remove_block(&root, key, &retirement.reference_id())
            {
                touched.push(key.to_string());
            }
            let successors = if member_removed && surface.successor_eligible {
                append_successors(&root, retirement.successors, surface.successor_key_prefix)
            } else {
                Vec::new()
            };
            if touched.is_empty() && successors.is_empty() {
                continue;
            }
            let relative = surface.relative_path;
            for key in &touched {
                changes.push(format!(
                    "Removed {} from {relative} ({key})",
                    retirement.stem
                ));
            }
            for (key, stem) in &successors {
                changes.push(format!("Activated successor {stem} in {relative} ({key})"));
            }
            write_document(&path, &document)?;
        }
    }
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings,
        ..crate::upgrade::MigrationOutcome::default()
    })
}

/// The org-pack-resolvable skip.
///
/// A project that declares an org pack which still ships
/// `<prefix>s/<stem>.<prefix>.yaml` can legitimately keep activating the id, so
/// the retirement leaves every surface alone. The roots come from
/// `.kittify/config.yaml`'s `charter_packs.org.packs` entries (and the retired
/// flat `organisation_packs` list) exactly as Source reads them.
fn still_resolves_via_org_pack(project_root: &Path, retirement: &Retirement) -> bool {
    let Some(text) =
        crate::migrations::completion_legacy::paths::read_text(&project_root.join(CONFIG_REL))
    else {
        return false;
    };
    let Ok(document) = CstDocument::parse("config.yaml", &text) else {
        return false;
    };
    let root = document.root();
    let kind_dir = format!("{}s", retirement.reference_prefix.to_lowercase());
    let filename = format!(
        "{}.{}.yaml",
        retirement.stem,
        retirement.reference_prefix.to_lowercase()
    );
    for pack_root in org_pack_roots(project_root, &root) {
        if pack_root.join(&kind_dir).join(&filename).is_file() {
            return true;
        }
    }
    false
}

fn org_pack_roots(project_root: &Path, root: &CstMapping) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(packs) = root
        .get_mapping("charter_packs")
        .and_then(|charter_packs| charter_packs.get_mapping("org"))
        .and_then(|org| org.get_sequence("packs"))
    {
        for value in packs.values() {
            if let CstValue::Mapping(mapping, _) = value
                && let Some(local_path) = mapping.get_string("local_path")
            {
                let subdir = mapping.get_string("subdir").unwrap_or_default();
                roots.push(project_root.join(local_path).join(subdir));
            }
        }
    }
    if let Some(packs) = root.get_sequence("organisation_packs") {
        for value in packs.values() {
            if let CstValue::Mapping(mapping, _) = value
                && let Some(path) = mapping.get_string("path")
            {
                roots.push(project_root.join(path));
            }
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_key_strips_the_activation_prefix() {
        assert_eq!(selected_key("activated_tactics"), "selected_tactics");
        // Source derives the nested key by prefix replacement, not idempotently:
        // a value already in `selected_` form gets the prefix again.
        assert_eq!(
            selected_key("selected_tactics"),
            "selected_selected_tactics"
        );
    }

    #[test]
    fn reference_id_is_prefixed() {
        let retirement = Retirement {
            kind_key: "activated_tactics",
            stem: "x",
            reference_prefix: "TACTIC",
            successors: &[],
        };
        assert_eq!(retirement.reference_id(), "TACTIC:x");
    }
}
