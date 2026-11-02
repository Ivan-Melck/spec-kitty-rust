//! Layer-aware artifact identity resolution.
//!
//! Mirrors `src/charter/activation/identity_index.py`: an activation token may
//! spell an artifact as its file stem (`001-arch`), its declared id
//! (`ARCH_001`) or a `<kind>:<id>` URN. Lookups are stem-first across **all**
//! layers (project, then org packs last-to-first, then built-in), then declared
//! id. Ambiguity is never silently resolved by this crate: callers that need a
//! representable stem use [`IdentityIndex::stem_checked`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;

use crate::error::DoctrineError;

/// The two identity lookups the activation ledger needs.
pub trait IdentityLookup {
    /// The declared id for a stem, declared id or `<kind>:` URN.
    fn declared_id(&self, kind: ArtifactKind, token: &str) -> Option<String>;

    /// The activation stem for a token.
    fn stem(&self, kind: ArtifactKind, token: &str) -> Option<String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    kind: ArtifactKind,
    stem: String,
    declared_id: String,
    layer: usize,
    path: PathBuf,
}

#[derive(Clone, Debug, Default)]
struct KindTable {
    by_stem: BTreeMap<String, Entry>,
    by_id: BTreeMap<String, Entry>,
    ids_by_stem: BTreeMap<String, BTreeSet<String>>,
    entries: Vec<Entry>,
}

/// A per-invocation identity index over the built-in, org and project layers.
#[derive(Clone, Debug, Default)]
pub struct IdentityIndex {
    tables: BTreeMap<&'static str, KindTable>,
    pub project_root: Option<PathBuf>,
    pub org_roots: Vec<PathBuf>,
    pub built_in_root: PathBuf,
}

impl IdentityIndex {
    /// Index exactly these roots.
    ///
    /// `org_roots` are in declaration order; `project_root` is the project pack
    /// root (`.kittify/charter-packs`), when present.
    pub fn from_roots(
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
    ) -> Self {
        let mut index = Self {
            tables: BTreeMap::new(),
            project_root: project_root.map(Path::to_path_buf),
            org_roots: org_roots.to_vec(),
            built_in_root: built_in_root.to_path_buf(),
        };
        scan_all(&mut index);
        index
    }

    /// Build an index from a repository root, resolving the built-in and org
    /// pack tiers through `sk-charter`'s read-only path API.
    pub fn from_project(repo_root: &Path) -> Result<Self, DoctrineError> {
        let config = sk_charter::load_project_config(repo_root, &sk_charter::Limits::default())?;
        let expansion = sk_charter::PathExpansion::from_env();
        let built_in = sk_charter::BuiltInSearch::from_env(repo_root);
        let roots =
            sk_charter::PackRoots::resolve(repo_root, &config, &expansion, Some(&built_in))?;
        let built_in_root = roots
            .built_in
            .ok_or(DoctrineError::PackRootUnavailable { tier: "built-in" })?;
        let org_roots = roots
            .organizations
            .iter()
            .map(|root| root.root.clone())
            .collect::<Vec<_>>();
        let project_root = roots.project.clone();
        Ok(Self::from_roots(
            &built_in_root,
            &org_roots,
            project_root.as_deref(),
        ))
    }

    fn table(&mut self, kind: ArtifactKind) -> &KindTable {
        let key = kind.canonical();
        if !self.tables.contains_key(key) {
            let entries = self.scan(kind);
            let mut table = KindTable::default();
            for entry in entries {
                table
                    .by_stem
                    .entry(entry.stem.clone())
                    .or_insert_with(|| entry.clone());
                table
                    .by_id
                    .entry(entry.declared_id.clone())
                    .or_insert_with(|| entry.clone());
                table
                    .ids_by_stem
                    .entry(entry.stem.clone())
                    .or_default()
                    .insert(entry.declared_id.clone());
                table.entries.push(entry);
            }
            self.tables.insert(key, table);
        }
        self.tables.get(key).expect("table inserted above")
    }

    fn scan(&self, kind: ArtifactKind) -> Vec<Entry> {
        let mut entries = Vec::new();
        let suffix = kind.glob_pattern();
        if suffix.is_empty() {
            return entries;
        }
        let suffix = suffix.trim_start_matches('*');
        let mut layers: Vec<(usize, PathBuf, bool)> = Vec::new();
        if let Some(project) = &self.project_root {
            layers.push((0, project.join(kind.project_dir()), true));
        }
        for (position, root) in self.org_roots.iter().rev().enumerate() {
            layers.push((1 + position, root.join(kind.plural()), true));
        }
        layers.push((
            1 + self.org_roots.len(),
            self.built_in_root.join(kind.plural()),
            true,
        ));
        if kind == ArtifactKind::MissionStepContract {
            layers.push((
                1 + self.org_roots.len(),
                self.built_in_root.join("missions/built_in_step_contracts"),
                false,
            ));
        }

        for (layer, directory, recursive) in layers {
            for path in files_with_suffix(&directory, suffix, recursive) {
                let stem = config_stem(&path);
                if stem.is_empty() {
                    continue;
                }
                let declared = declared_id(&path, kind).unwrap_or_else(|| stem.clone());
                if declared.is_empty() {
                    continue;
                }
                entries.push(Entry {
                    kind,
                    stem,
                    declared_id: declared,
                    layer,
                    path,
                });
            }
        }
        entries
    }

    fn bare(kind: ArtifactKind, token: &str) -> &str {
        token
            .strip_prefix(kind.canonical())
            .and_then(|rest| rest.strip_prefix(':'))
            .unwrap_or(token)
    }

    fn lookup(&mut self, kind: ArtifactKind, token: &str) -> Option<Entry> {
        let token = Self::bare(kind, token);
        let table = self.table(kind);
        table
            .by_stem
            .get(token)
            .or_else(|| table.by_id.get(token))
            .cloned()
    }

    /// Declared id for a token, refusing an ambiguous stem.
    pub fn declared_id_checked(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Option<String>, DoctrineError> {
        let bare = Self::bare(kind, token).to_owned();
        let table = self.tables.get(kind.canonical());
        let Some(table) = table else {
            return Ok(None);
        };
        if let Some(candidates) = table.ids_by_stem.get(&bare)
            && candidates.len() > 1
            && !table.by_id.contains_key(&bare)
        {
            let layer = table
                .by_stem
                .get(&bare)
                .map(|entry| entry.layer)
                .unwrap_or_default();
            return Err(DoctrineError::AmbiguousIdentity {
                kind: kind.canonical().to_owned(),
                token: bare,
                layer: self.layer_path(layer),
                candidates: candidates.iter().cloned().collect(),
            });
        }
        Ok(table
            .by_stem
            .get(&bare)
            .or_else(|| table.by_id.get(&bare))
            .map(|entry| entry.declared_id.clone()))
    }

    /// The declaring file for a token, when the index knows it.
    pub fn path_for(&self, kind: ArtifactKind, token: &str) -> Option<&Path> {
        let bare = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical())?;
        table
            .by_stem
            .get(bare)
            .or_else(|| table.by_id.get(bare))
            .map(|entry| entry.path.as_path())
    }

    /// Whether a token names a known artifact.
    pub fn knows(&mut self, kind: ArtifactKind, token: &str) -> bool {
        self.lookup(kind, token).is_some()
    }

    fn layer_path(&self, layer: usize) -> PathBuf {
        if layer == 0 {
            return self
                .project_root
                .clone()
                .unwrap_or_else(|| PathBuf::from("<project>"));
        }
        if layer <= self.org_roots.len() {
            return self.org_roots[self.org_roots.len() - layer].clone();
        }
        self.built_in_root.clone()
    }
}

impl IdentityLookup for IdentityIndex {
    fn declared_id(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        let bare = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical())?;
        table
            .by_stem
            .get(bare)
            .or_else(|| table.by_id.get(bare))
            .map(|entry| entry.declared_id.clone())
    }

    fn stem(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        let bare = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical())?;
        if let Some(entry) = table.by_stem.get(bare) {
            return Some(entry.stem.clone());
        }
        table.by_id.get(bare).map(|entry| entry.stem.clone())
    }
}

/// Populate the index for every artifact kind.
pub fn scan_all(index: &mut IdentityIndex) {
    for kind in ArtifactKind::ALL {
        let _ = index.table(kind);
    }
}

fn config_stem(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.split('.').next().unwrap_or(name).to_owned())
        .unwrap_or_default()
}

fn declared_id(path: &Path, kind: ArtifactKind) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).ok()?;
    let field = if kind == ArtifactKind::AgentProfile {
        "profile-id"
    } else {
        "id"
    };
    value
        .get(field)
        .and_then(|value| match value {
            serde_yaml_ng::Value::String(text) => Some(text.trim().to_owned()),
            serde_yaml_ng::Value::Number(number) => Some(number.to_string()),
            serde_yaml_ng::Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        })
        .filter(|id| !id.is_empty())
}

fn files_with_suffix(directory: &Path, suffix: &str, recursive: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if recursive {
        collect_recursive(directory, suffix, &mut files);
    } else if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(suffix))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn collect_recursive(directory: &Path, suffix: &str, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_recursive(&path, suffix, files);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(suffix))
        {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn stem_first_then_declared_id_and_urn() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "id: ARCH_001\n",
        );
        let mut index = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut index);
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "001-arch"),
            Some("ARCH_001".to_owned())
        );
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "ARCH_001"),
            Some("ARCH_001".to_owned())
        );
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "directive:ARCH_001"),
            Some("ARCH_001".to_owned())
        );
        assert_eq!(
            index.stem(ArtifactKind::Directive, "ARCH_001"),
            Some("001-arch".to_owned())
        );
        assert!(!index.knows(ArtifactKind::Directive, "missing"));
    }

    #[test]
    fn project_and_last_org_pack_win_lookups() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        let org_one = directory.path().join("org-one");
        let org_two = directory.path().join("org-two");
        let project = directory.path().join("project");
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "id: B\n",
        );
        write(
            &org_one.join("directives/001-arch.directive.yaml"),
            "id: O1\n",
        );
        write(
            &org_two.join("directives/001-arch.directive.yaml"),
            "id: O2\n",
        );
        write(
            &project.join("directive/001-arch.directive.yaml"),
            "id: P\n",
        );
        let mut index = IdentityIndex::from_roots(&built_in, &[org_one, org_two], Some(&project));
        scan_all(&mut index);
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "001-arch"),
            Some("P".to_owned())
        );
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "B"),
            Some("B".to_owned())
        );

        let mut org_only = IdentityIndex::from_roots(
            &built_in,
            &[
                directory.path().join("org-one"),
                directory.path().join("org-two"),
            ],
            None,
        );
        scan_all(&mut org_only);
        assert_eq!(
            org_only.declared_id(ArtifactKind::Directive, "001-arch"),
            Some("O2".to_owned())
        );
    }

    #[test]
    fn ambiguous_stem_is_refused_by_checked_lookup() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        let org = directory.path().join("org");
        write(
            &built_in.join("tactics/shared.tactic.yaml"),
            "id: BUILTIN_ID\n",
        );
        write(&org.join("tactics/shared.tactic.yaml"), "id: ORG_ID\n");
        let mut index = IdentityIndex::from_roots(&built_in, &[org], None);
        scan_all(&mut index);
        let error = index
            .declared_id_checked(ArtifactKind::Tactic, "shared")
            .unwrap_err();
        assert!(matches!(error, DoctrineError::AmbiguousIdentity { .. }));
        // An explicit declared id still resolves.
        assert_eq!(
            index
                .declared_id_checked(ArtifactKind::Tactic, "ORG_ID")
                .unwrap(),
            Some("ORG_ID".to_owned())
        );
    }

    #[test]
    fn missing_id_falls_back_to_the_file_stem() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("directives/007-fallback.directive.yaml"),
            "title: x\n",
        );
        let mut index = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut index);
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "007-fallback"),
            Some("007-fallback".to_owned())
        );
    }
}
