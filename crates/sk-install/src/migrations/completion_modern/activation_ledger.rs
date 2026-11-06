//! `4.0.0rc8_activation_ledger`: move legacy activation keys into the ledger.
//!
//! `.kittify/charter/activation.yaml` is the one activation authority. A project
//! an earlier CLI wrote still carries `activated_<kind>`, `activated_kinds` and
//! `mission_type_activations` on the legacy surface (the `charter.yaml` the
//! `config.yaml` `charter:` pointer names, else `config.yaml`, sometimes with a
//! leftover `config.yaml` copy). This migration plans the move, refuses when
//! the surfaces disagree (writing nothing), strips the legacy keys through the
//! round-trip CST editor and writes the ledger through the doctrine writer.
//!
//! The plan/refusal semantics, the project-intent stem convergence, the
//! prospective `bundle_content_hash` restamp and the "nothing was written on a
//! refusal" contract are ports of
//! `charter/activation/activation_ledger.py::plan_ledger_migration` /
//! `apply_ledger_migration`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_doctrine::{apply_ledger_write, ledger_keys, prepare_ledger_write};

use crate::error::InstallError;
use crate::upgrade::MigrationOutcome;
use crate::yaml_edit::CstDocument;

const CONFIG_REL: &str = ".kittify/config.yaml";
const LEDGER_REL: &str = ".kittify/charter/activation.yaml";
const MANIFEST_REL: &str = ".kittify/charter/synthesis-manifest.yaml";
const CHARTER_YAML_REL: &str = ".kittify/charter/charter.yaml";
const LEGACY_DENY_PREFIX: &str = "deactivated_";
const LEGACY_SLUG: &str = "mission-type-scope-directive";
const NEW_SLUG: &str = "project-intent-directive";
const RERUN: &str = "`spec-kitty migrate activation-ledger` or `spec-kitty upgrade`";

/// One `(key, from-file, to-file)` move.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerMove {
    /// The activation key.
    pub key: String,
    /// The legacy file it leaves.
    pub source: PathBuf,
    /// The ledger it enters.
    pub target: PathBuf,
}

/// Why the move refuses: the key, the disagreeing files, and the kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerRefusal {
    /// The activation key.
    pub key: String,
    /// The files that disagree (or hold a legacy deny key).
    pub files: Vec<PathBuf>,
    /// `true` for a legacy deny-list key, `false` for divergence.
    pub legacy_deny: bool,
}

impl LedgerRefusal {
    /// The operator-facing explanation, ending with the remedy.
    pub fn message(&self) -> String {
        let listed = self
            .files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let what = if self.legacy_deny {
            format!(
                "{} in {listed} is a legacy deny-list key; it was never applied and is not moved. Remove it by hand",
                self.key
            )
        } else {
            format!(
                "{} differs between {listed}. Make the files agree, or remove the stale copy",
                self.key
            )
        };
        format!(
            "Refused to move the activation keys to {LEDGER_REL}: {what}, then run {RERUN} again. Nothing was written."
        )
    }
}

/// The pure plan of the move.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LedgerMigrationPlan {
    /// The allow keys written into a not-yet-existing ledger, verbatim.
    pub ledger_values: BTreeMap<String, Option<Vec<String>>>,
    /// `(file, keys)` activation keys removed from each legacy file.
    pub strips: Vec<(PathBuf, Vec<String>)>,
    /// The refusal, when the surfaces disagree.
    pub refusal: Option<LedgerRefusal>,
}

impl LedgerMigrationPlan {
    /// Whether there is nothing to move or strip, and no refusal.
    pub fn is_noop(&self) -> bool {
        self.refusal.is_none() && self.strips.is_empty() && self.ledger_values.is_empty()
    }

    /// Every `(key, from, to)` move the apply performs (empty for a refusal).
    pub fn moves(&self, repo_root: &Path) -> Vec<LedgerMove> {
        let target = repo_root.join(LEDGER_REL);
        self.strips
            .iter()
            .flat_map(|(path, keys)| {
                keys.iter().map(|key| LedgerMove {
                    key: key.clone(),
                    source: path.clone(),
                    target: target.clone(),
                })
            })
            .collect()
    }
}

struct Surface {
    path: PathBuf,
    data: BTreeMap<String, Option<Vec<String>>>,
}

fn allow_keys() -> Vec<String> {
    ledger_keys()
        .into_iter()
        .filter(|key| !key.starts_with(LEGACY_DENY_PREFIX))
        .collect()
}

fn deny_keys() -> Vec<String> {
    ledger_keys()
        .into_iter()
        .filter(|key| key.starts_with(LEGACY_DENY_PREFIX))
        .collect()
}

fn read_mapping(path: &Path) -> Result<BTreeMap<String, serde_yaml_ng::Value>, InstallError> {
    let Some(text) = crate::migrations::completion_legacy::paths::read_text(path) else {
        return Ok(BTreeMap::new());
    };
    if text.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&text).map_err(|error| InstallError::UpgradeRefused {
            detail: format!("Invalid YAML in {}: {error}", path.display()),
        })?;
    match value {
        serde_yaml_ng::Value::Null => Ok(BTreeMap::new()),
        serde_yaml_ng::Value::Mapping(mapping) => Ok(mapping
            .into_iter()
            .filter_map(|(key, value)| key.as_str().map(|key| (key.to_string(), value)))
            .collect()),
        _ => Err(InstallError::UpgradeRefused {
            detail: format!("{} root must be a mapping.", path.display()),
        }),
    }
}

/// Three-state list value: absent/null → `None`; a list → its string ids.
fn ids_or_error(
    key: &str,
    value: Option<&serde_yaml_ng::Value>,
    where_: &Path,
) -> Result<Option<Vec<String>>, InstallError> {
    match value {
        None | Some(serde_yaml_ng::Value::Null) => Ok(None),
        Some(serde_yaml_ng::Value::Sequence(items)) => {
            let mut ids = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(id) => ids.push(id.to_string()),
                    None => {
                        return Err(InstallError::UpgradeRefused {
                            detail: format!(
                                "Activation key '{key}' in {} must list string ids, got {item:?}.",
                                where_.display()
                            ),
                        });
                    }
                }
            }
            Ok(Some(ids))
        }
        Some(_) => Err(InstallError::UpgradeRefused {
            detail: format!(
                "Activation key '{key}' in {} must be a list, got a scalar.",
                where_.display()
            ),
        }),
    }
}

fn surface_data(path: &Path) -> Result<BTreeMap<String, Option<Vec<String>>>, InstallError> {
    let mapping = read_mapping(path)?;
    let mut data = BTreeMap::new();
    for key in allow_keys().into_iter().chain(deny_keys()) {
        if let Some(value) = mapping.get(&key) {
            data.insert(key.clone(), ids_or_error(&key, Some(value), path)?);
        }
    }
    // Legacy deny keys outside the known kind set are not ledger keys; Source
    // only refuses the known per-kind deny keys, so they are ignored here too.
    Ok(data)
}

fn converge(token: &str) -> Option<String> {
    let (number, slug) = token.split_once('-')?;
    if slug != LEGACY_SLUG || number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{number}-{NEW_SLUG}"))
}

fn converged(ids: &[String]) -> Vec<String> {
    ids.iter()
        .map(|token| converge(token).unwrap_or_else(|| token.clone()))
        .collect()
}

/// Order-insensitive, three-state equality with the rename convergence rule.
fn same(left: Option<&Vec<String>>, right: Option<&Vec<String>>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
        (Some(left), Some(right)) => {
            let left_converged = converged(left);
            let right_converged = converged(right);
            if left_converged == *left && right_converged == *right {
                let mut left = left.clone();
                let mut right = right.clone();
                left.sort();
                right.sort();
                left == right
            } else {
                let left: std::collections::BTreeSet<String> = left_converged.into_iter().collect();
                let right: std::collections::BTreeSet<String> =
                    right_converged.into_iter().collect();
                left == right
            }
        }
    }
}

fn legacy_pointer(repo_root: &Path) -> Result<Option<PathBuf>, InstallError> {
    let mapping = read_mapping(&repo_root.join(CONFIG_REL))?;
    let Some(value) = mapping.get("charter") else {
        return Ok(None);
    };
    let Some(raw) = value.as_str() else {
        return Ok(None);
    };
    let pointer = PathBuf::from(raw);
    let pointer = if pointer.is_absolute() {
        pointer
    } else {
        repo_root.join(pointer)
    };
    if !pointer.exists() {
        return Err(InstallError::UpgradeRefused {
            detail: format!(
                ".kittify/config.yaml 'charter:' pointer names {}, which does not exist.\nRemediation: fix the 'charter:' pointer in .kittify/config.yaml, or remove it when the activation keys live in .kittify/config.yaml itself.",
                pointer.display()
            ),
        });
    }
    Ok(Some(pointer))
}

fn legacy_surfaces(repo_root: &Path) -> Result<Vec<Surface>, InstallError> {
    let config_path = repo_root.join(CONFIG_REL);
    let Some(pointer) = legacy_pointer(repo_root)? else {
        if !config_path.is_file() {
            return Ok(Vec::new());
        }
        return Ok(vec![Surface {
            path: config_path.clone(),
            data: surface_data(&config_path)?,
        }]);
    };
    Ok(vec![
        Surface {
            path: pointer.clone(),
            data: surface_data(&pointer)?,
        },
        Surface {
            path: config_path.clone(),
            data: surface_data(&config_path)?,
        },
    ])
}

fn plan_ledger_migration(repo_root: &Path) -> Result<LedgerMigrationPlan, InstallError> {
    let ledger_path = repo_root.join(LEDGER_REL);
    let surfaces = legacy_surfaces(repo_root)?;
    let ledger = if ledger_path.is_file() {
        Some(surface_data(&ledger_path)?)
    } else {
        None
    };
    let deny = deny_keys();
    for surface in &surfaces {
        for key in &deny {
            if surface.data.contains_key(key) {
                return Ok(LedgerMigrationPlan {
                    refusal: Some(LedgerRefusal {
                        key: key.clone(),
                        files: vec![surface.path.clone()],
                        legacy_deny: true,
                    }),
                    ..LedgerMigrationPlan::default()
                });
            }
        }
    }
    for key in allow_keys() {
        if let Some(ledger) = &ledger {
            let target = ledger.get(&key).cloned().flatten();
            for surface in &surfaces {
                if let Some(held) = surface.data.get(&key).cloned().flatten()
                    && !same(Some(&held), target.as_ref())
                {
                    return Ok(LedgerMigrationPlan {
                        refusal: Some(LedgerRefusal {
                            key: key.clone(),
                            files: vec![surface.path.clone(), ledger_path.clone()],
                            legacy_deny: false,
                        }),
                        ..LedgerMigrationPlan::default()
                    });
                }
            }
        } else if surfaces.len() >= 2
            && let Some(copy) = surfaces[1].data.get(&key).cloned().flatten()
            && !same(
                Some(&copy),
                surfaces[0].data.get(&key).cloned().flatten().as_ref(),
            )
        {
            return Ok(LedgerMigrationPlan {
                refusal: Some(LedgerRefusal {
                    key: key.clone(),
                    files: vec![surfaces[0].path.clone(), surfaces[1].path.clone()],
                    legacy_deny: false,
                }),
                ..LedgerMigrationPlan::default()
            });
        }
    }
    let strips: Vec<(PathBuf, Vec<String>)> = surfaces
        .iter()
        .filter_map(|surface| {
            let keys: Vec<String> = allow_keys()
                .into_iter()
                .filter(|key| surface.data.contains_key(key))
                .collect();
            (!keys.is_empty()).then(|| (surface.path.clone(), keys))
        })
        .collect();
    let ledger_values = if ledger.is_none() {
        surfaces.first().map_or_else(BTreeMap::new, |authority| {
            allow_keys()
                .into_iter()
                .filter_map(|key| {
                    authority
                        .data
                        .get(&key)
                        .cloned()
                        .flatten()
                        .map(|ids| (key, Some(ids)))
                })
                .collect()
        })
    } else {
        BTreeMap::new()
    };
    Ok(LedgerMigrationPlan {
        ledger_values,
        strips,
        refusal: None,
    })
}

/// The bundle stamp over the PROSPECTIVE bytes: the charter text the caller
/// supplied (or the authored one) and the activation member when it will exist.
/// This routes through the one canonical bundle recipe
/// (`sk_charter::bundle_content_hash_of`), so a preview cannot drift from the
/// stamp a real write produces.
fn prospective_bundle_hash(repo_root: &Path, charter_text: Option<&str>) -> Option<String> {
    let charter_path = repo_root.join(CHARTER_YAML_REL);
    let charter = match charter_text {
        Some(text) => text.to_string(),
        None => crate::migrations::completion_legacy::paths::read_text(&charter_path)?,
    };
    let ledger_path = repo_root.join(LEDGER_REL);
    let ledger = crate::migrations::completion_legacy::paths::read_text(&ledger_path);
    let optional: Vec<&str> = ledger.iter().map(String::as_str).collect();
    Some(sk_charter::bundle_content_hash_of(
        &[charter.as_str()],
        &optional,
    ))
}

/// Restamp the synthesis manifest's `bundle_content_hash` (no comment loss).
fn stamp_manifest(repo_root: &Path, bundle_hash: &str) -> Result<Option<PathBuf>, InstallError> {
    let manifest_path = repo_root.join(MANIFEST_REL);
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let Some(text) = crate::migrations::completion_legacy::paths::read_text(&manifest_path) else {
        return Ok(None);
    };
    let document = CstDocument::parse(&manifest_path, &text)?;
    let root = document.root();
    let built_in_only = root.get_string("built_in_only").as_deref() == Some("true");
    let stored = root.get_string("bundle_content_hash");
    if built_in_only || stored.is_none() {
        return Ok(None);
    }
    if stored.as_deref() == Some(bundle_hash) {
        return Ok(None);
    }
    // Replace in place with a CST edit; re-use the scalar upsert to keep every
    // other authored byte.
    let editor = crate::yaml_edit::YamlEditor::new(&manifest_path, &text);
    let outcome = editor.upsert_scalars(
        "",
        &[(
            "bundle_content_hash".to_string(),
            crate::yaml_edit::Scalar::Str(bundle_hash.to_string()),
        )],
        crate::yaml_edit::InsertPosition::Bottom,
    )?;
    if !outcome.changed {
        return Ok(None);
    }
    crate::migrations::completion_legacy::paths::write_text(&manifest_path, &outcome.text)?;
    Ok(Some(manifest_path))
}

/// `detect`: something to move or strip, or a refusal to report.
pub fn detect(project_root: &Path) -> Result<bool, InstallError> {
    Ok(!plan_ledger_migration(project_root)?.is_noop())
}

/// `apply`: move the keys (a refusal is a typed failure and writes nothing).
pub fn apply(project_root: &Path) -> Result<MigrationOutcome, InstallError> {
    let plan = plan_ledger_migration(project_root)?;
    if let Some(refusal) = &plan.refusal {
        return Err(InstallError::UpgradeRefused {
            detail: refusal.message(),
        });
    }
    let moves: Vec<String> = plan
        .moves(project_root)
        .into_iter()
        .map(|movement| {
            format!(
                "{}: {} -> {}",
                movement.key,
                crate::migrations::completion_legacy::paths::relative_display(
                    project_root,
                    &movement.source
                ),
                crate::migrations::completion_legacy::paths::relative_display(
                    project_root,
                    &movement.target
                )
            )
        })
        .collect();
    let mut written: Vec<String> = Vec::new();
    if !plan.ledger_values.is_empty() {
        let prepared = prepare_ledger_write(project_root, &plan.ledger_values, &[], None)
            .map_err(doctrine_error)?;
        apply_ledger_write(prepared).map_err(doctrine_error)?;
        written.push(LEDGER_REL.to_string());
    }
    // Prepare the stripped documents first so the manifest hash describes the
    // bundle the strips produce (Source's F3 ordering).
    let mut stripped: Vec<(PathBuf, String)> = Vec::new();
    for (path, keys) in &plan.strips {
        let Some(text) = crate::migrations::completion_legacy::paths::read_text(path) else {
            continue;
        };
        let document = CstDocument::parse(path, &text)?;
        let root = document.root();
        for key in keys {
            root.remove(key);
        }
        let rendered = if document.keys().is_empty() {
            "{}\n".to_string()
        } else {
            document.text()
        };
        stripped.push((path.clone(), rendered));
    }
    if !plan.is_noop() {
        let charter_path = project_root.join(CHARTER_YAML_REL);
        let charter_text = stripped
            .iter()
            .find(|(path, _)| path == &charter_path)
            .map(|(_, text)| text.as_str());
        if let Some(hash) = prospective_bundle_hash(project_root, charter_text)
            && let Some(path) = stamp_manifest(project_root, &hash)?
        {
            written.push(
                crate::migrations::completion_legacy::paths::relative_display(project_root, &path),
            );
        }
    }
    for (path, text) in stripped {
        crate::migrations::completion_legacy::paths::write_text(&path, &text)?;
        written.push(
            crate::migrations::completion_legacy::paths::relative_display(project_root, &path),
        );
    }
    let mut changes: Vec<String> = moves
        .into_iter()
        .map(|line| format!("Moved {line}"))
        .collect();
    changes.extend(written.into_iter().map(|path| format!("Wrote {path}")));
    Ok(MigrationOutcome {
        changed: !changes.is_empty(),
        changes,
        warnings: Vec::new(),
        ..crate::upgrade::MigrationOutcome::default()
    })
}

fn doctrine_error(error: sk_doctrine::DoctrineError) -> InstallError {
    InstallError::UpgradeRefused {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convergence_rewrites_only_the_legacy_slug() {
        assert_eq!(
            converge("004-mission-type-scope-directive").as_deref(),
            Some("004-project-intent-directive")
        );
        assert_eq!(converge("004-other"), None);
    }

    #[test]
    fn three_state_equality_is_order_insensitive_and_keeps_absent_distinct() {
        let a = vec!["x".to_string(), "y".to_string()];
        let b = vec!["y".to_string(), "x".to_string()];
        assert!(same(Some(&a), Some(&b)));
        assert!(!same(Some(&a), None));
        assert!(same(None, None));
        let legacy = vec!["004-mission-type-scope-directive".to_string()];
        let renamed = vec!["004-project-intent-directive".to_string()];
        assert!(same(Some(&legacy), Some(&renamed)));
    }

    #[test]
    fn the_stamp_uses_the_one_charter_recipe() {
        // The module no longer carries its own BOM/CRLF/whitespace recipe: the
        // canonical chunk digest is `sk_charter::hash_content`.
        assert_eq!(
            sk_charter::hash_content("\u{feff}a\r\nb"),
            sk_charter::hash_content("a\nb")
        );
    }
}
