//! Layer-aware artifact identity resolution.
//!
//! Mirrors `src/charter/activation/identity_index.py`: an activation token may
//! spell an artifact as its file stem (`001-arch`), its declared id
//! (`ARCH_001`) or a `<kind>:<id>` URN. Lookups are stem-first across **all**
//! layers (project, then org packs last-to-first, then built-in), then declared
//! id, then — for directives — the canonical id normaliser.
//!
//! Precedence, not ambiguity, decides a stem two layers share: the reference
//! index takes the first match in layer order and never fails a valid project
//! activation over a shadowed built-in file. The only hard failure is a
//! *declared id* that cannot be represented by any activation stem (two layers
//! bind the same file name to different ids), which is a pack-content defect
//! reported by [`IdentityIndex::stored_spelling`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sk_charter::ArtifactKind;

use crate::error::DoctrineError;
use crate::org::normalize_directive_id;

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
        let built_in = crate::org_charter::built_in_search(repo_root);
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

    fn lookup_existing(&self, kind: ArtifactKind, token: &str) -> Option<&Entry> {
        let token = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical())?;
        if let Some(entry) = table.by_stem.get(token).or_else(|| table.by_id.get(token)) {
            return Some(entry);
        }
        if kind == ArtifactKind::Directive {
            let normalised = normalize_directive_id(token);
            if normalised != token {
                return table.by_id.get(&normalised);
            }
        }
        None
    }

    /// Declared id for a token (stem first, then declared id, then the
    /// directive normaliser).
    ///
    /// A shared stem resolves to the highest-precedence layer's declaration,
    /// exactly as the reference index does; it never fails. Use
    /// [`IdentityIndex::declared_id_in_layer`] to ask about one layer.
    pub fn declared_id_checked(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Option<String>, DoctrineError> {
        Ok(self.declared_id_impl(kind, token))
    }

    fn declared_id_impl(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        let bare = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical())?;
        if let Some(entry) = table.by_stem.get(bare).or_else(|| table.by_id.get(bare)) {
            return Some(entry.declared_id.clone());
        }
        if kind == ArtifactKind::Directive {
            let normalised = normalize_directive_id(bare);
            if normalised != bare {
                return table
                    .by_id
                    .get(&normalised)
                    .map(|entry| entry.declared_id.clone());
            }
        }
        None
    }

    /// Declared id of the file named `stem` in exactly one layer (`project`,
    /// `org` or `built-in`), never a higher layer's shadowing declaration.
    pub fn declared_id_in_layer(
        &self,
        kind: ArtifactKind,
        stem: &str,
        layer: IdentityLayer,
    ) -> Option<String> {
        let table = self.tables.get(kind.canonical())?;
        table
            .entries
            .iter()
            .find(|entry| entry.stem == stem && self.layer_name(entry.layer) == layer)
            .map(|entry| entry.declared_id.clone())
    }

    /// The layer a token resolves in, when known.
    pub fn source_layer(&self, kind: ArtifactKind, token: &str) -> Option<IdentityLayer> {
        let bare = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical())?;
        let entry = table.by_stem.get(bare).or_else(|| table.by_id.get(bare))?;
        Some(self.layer_name(entry.layer))
    }

    /// The declared id a *selection* resolves to; directives prefer a declared
    /// id over a coincidentally equal file name.
    pub fn selection_id(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        let bare = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical())?;
        if kind == ArtifactKind::Directive
            && let Some(entry) = table.by_id.get(bare)
        {
            return Some(entry.declared_id.clone());
        }
        self.declared_id_impl(kind, token)
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
    pub fn knows(&self, kind: ArtifactKind, token: &str) -> bool {
        self.lookup_existing(kind, token).is_some()
    }

    /// Whether any layer's file declares `token` as its id.
    pub fn has_declared_id(&self, kind: ArtifactKind, token: &str) -> bool {
        let bare = Self::bare(kind, token);
        let Some(table) = self.tables.get(kind.canonical()) else {
            return false;
        };
        if table.by_id.contains_key(bare) {
            return true;
        }
        kind == ArtifactKind::Directive && table.by_id.contains_key(&normalize_directive_id(bare))
    }

    /// Whether `token` is the file stem of some indexed artifact.
    pub fn names_a_file(&self, kind: ArtifactKind, token: &str) -> bool {
        self.tables
            .get(kind.canonical())
            .is_some_and(|table| table.by_stem.contains_key(Self::bare(kind, token)))
    }

    /// The activation stem for a token; `None` when unknown or unrepresentable.
    ///
    /// Directives are id-first (as selections are); other kinds are stem-first.
    pub fn stem(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        let bare = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical())?;
        if kind == ArtifactKind::Directive && table.by_id.contains_key(bare) {
            return self.stem_for_declared_id(kind, bare);
        }
        if table.by_stem.contains_key(bare) {
            return Some(bare.to_owned());
        }
        self.declared_id_impl(kind, bare)
            .and_then(|declared| self.stem_for_declared_id(kind, &declared))
    }

    /// The stem whose declared id is `declared_id`, or `None`.
    pub fn stem_for_declared_id(&self, kind: ArtifactKind, declared_id: &str) -> Option<String> {
        let bare = Self::bare(kind, declared_id);
        if kind != ArtifactKind::Directive {
            return self.representable_stem(kind, bare);
        }
        if let Some(stem) = self.representable_stem(kind, bare) {
            return Some(stem);
        }
        let normalised = normalize_directive_id(bare);
        if normalised == bare {
            None
        } else {
            self.representable_stem(kind, &normalised)
        }
    }

    fn representable_stem(&self, kind: ArtifactKind, declared: &str) -> Option<String> {
        let table = self.tables.get(kind.canonical())?;
        for entry in &table.entries {
            if entry.declared_id != declared {
                continue;
            }
            if kind != ArtifactKind::Directive {
                return Some(entry.stem.clone());
            }
            let representative = table.by_stem.get(&entry.stem)?;
            let distinct = table.ids_by_stem.get(&entry.stem).map(BTreeSet::len);
            if representative.declared_id == declared && distinct == Some(1) {
                return Some(entry.stem.clone());
            }
        }
        None
    }

    /// The spelling a writer stores for `token`: its activation stem.
    ///
    /// A known token with no representable stem raises
    /// [`DoctrineError::ActivationIdCollision`]; an unknown token is returned
    /// as given (its own validation belongs to the caller).
    pub fn stored_spelling(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<String, DoctrineError> {
        if let Some(stem) = self.stem(kind, token) {
            return Ok(stem);
        }
        if self.has_declared_id(kind, token) {
            return Err(self.collision_error(kind, token));
        }
        Ok(token.to_owned())
    }

    fn collision_error(&self, kind: ArtifactKind, token: &str) -> DoctrineError {
        let bare = Self::bare(kind, token);
        let table = self.tables.get(kind.canonical());
        let declared = table
            .and_then(|table| table.by_id.get(bare))
            .map(|entry| entry.declared_id.clone())
            .unwrap_or_else(|| bare.to_owned());
        let file_name = table
            .and_then(|table| {
                table
                    .entries
                    .iter()
                    .find(|entry| entry.declared_id == declared)
            })
            .map(|entry| entry.stem.clone())
            .unwrap_or_else(|| bare.to_owned());
        let declarations = table
            .map(|table| {
                table
                    .entries
                    .iter()
                    .filter(|entry| entry.stem == file_name)
                    .map(|entry| {
                        format!(
                            "{} file {}",
                            self.layer_name(entry.layer),
                            entry.path.display()
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        DoctrineError::ActivationIdCollision {
            kind: kind.canonical().to_owned(),
            artifact_id: bare.to_owned(),
            file_name,
            declarations,
        }
    }

    /// Key for a token no index knows: the directive id normaliser, else the token.
    pub fn unindexed_key(kind: Option<ArtifactKind>, token: &str) -> String {
        if kind == Some(ArtifactKind::Directive) {
            normalize_directive_id(token)
        } else {
            token.to_owned()
        }
    }

    /// Repository/comparison key of `token` (its declared id when known).
    pub fn lookup_key(&self, kind: Option<ArtifactKind>, token: &str, selection: bool) -> String {
        if let Some(kind) = kind {
            let known = if selection {
                self.selection_id(kind, token)
            } else {
                self.declared_id_impl(kind, token)
            };
            if let Some(known) = known {
                return known;
            }
        }
        Self::unindexed_key(kind, token)
    }

    /// Single route for activation-list tokens: a known token yields its
    /// declared id; an unknown directive falls back to the normaliser and is
    /// reported; `anti_pattern` has no files and passes through silently.
    pub fn activation_id(
        &self,
        kind: ArtifactKind,
        token: &str,
        warnings: &mut Vec<String>,
    ) -> String {
        if let Some(declared) = self.declared_id_impl(kind, token) {
            return declared;
        }
        if kind == ArtifactKind::AntiPattern {
            return token.to_owned();
        }
        let resolved = if kind == ArtifactKind::Directive {
            normalize_directive_id(token)
        } else {
            token.to_owned()
        };
        warnings.push(format!(
            "unresolved {} activation token {token:?}; best-effort resolved to {resolved:?}",
            kind.canonical()
        ));
        resolved
    }

    fn layer_name(&self, layer: usize) -> IdentityLayer {
        if layer == 0 {
            return IdentityLayer::Project;
        }
        if layer <= self.org_roots.len() {
            return IdentityLayer::Org;
        }
        IdentityLayer::BuiltIn
    }
}

/// The layer an identity entry comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityLayer {
    /// `.kittify/charter-packs/`.
    Project,
    /// A configured org pack.
    Org,
    /// The built-in offering.
    BuiltIn,
}

impl IdentityLayer {
    /// The reference label (`project`, `org`, `built-in`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Org => "org",
            Self::BuiltIn => "built-in",
        }
    }
}

impl std::fmt::Display for IdentityLayer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl IdentityLookup for IdentityIndex {
    fn declared_id(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        self.declared_id_impl(kind, token)
    }

    fn stem(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        IdentityIndex::stem(self, kind, token)
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
    fn directive_id_normalisation_resolves_numeric_and_slug_spellings() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("directives/001-arch.directive.yaml"),
            "id: DIRECTIVE_001\n",
        );
        write(
            &built_in.join("directives/use-c4-model-techniques.directive.yaml"),
            "id: USE_C4_MODEL_TECHNIQUES\n",
        );
        let mut index = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut index);
        for token in ["1", "001", "directive:1", "DIRECTIVE_001"] {
            assert_eq!(
                index.declared_id(ArtifactKind::Directive, token),
                Some("DIRECTIVE_001".to_owned()),
                "{token}"
            );
        }
        assert_eq!(
            index.stem(ArtifactKind::Directive, "1"),
            Some("001-arch".to_owned())
        );
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "use-c4-model-techniques"),
            Some("USE_C4_MODEL_TECHNIQUES".to_owned())
        );
        assert_eq!(
            index
                .stored_spelling(ArtifactKind::Directive, "001")
                .unwrap(),
            "001-arch"
        );
        assert_eq!(
            index
                .stored_spelling(ArtifactKind::Directive, "use-c4-model-techniques")
                .unwrap(),
            "use-c4-model-techniques"
        );
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
            index.source_layer(ArtifactKind::Directive, "001-arch"),
            Some(IdentityLayer::Project)
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
    fn shared_stem_resolves_by_layer_precedence_instead_of_failing() {
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
        assert_eq!(
            index
                .declared_id_checked(ArtifactKind::Tactic, "shared")
                .unwrap(),
            Some("ORG_ID".to_owned())
        );
        assert_eq!(
            index.declared_id_in_layer(ArtifactKind::Tactic, "shared", IdentityLayer::BuiltIn),
            Some("BUILTIN_ID".to_owned())
        );
        assert_eq!(
            index.declared_id_in_layer(ArtifactKind::Tactic, "shared", IdentityLayer::Org),
            Some("ORG_ID".to_owned())
        );
        // An explicit declared id still resolves.
        assert_eq!(
            index
                .declared_id_checked(ArtifactKind::Tactic, "BUILTIN_ID")
                .unwrap(),
            Some("BUILTIN_ID".to_owned())
        );
        // A non-directive kind takes the first file in layer precedence order
        // that declares the id, so the shared stem is a valid stored spelling.
        assert_eq!(
            index
                .stored_spelling(ArtifactKind::Tactic, "BUILTIN_ID")
                .unwrap(),
            "shared"
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

    #[test]
    fn directive_stem_collision_raises_the_pack_content_refusal() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        let org = directory.path().join("org");
        write(
            &built_in.join("directives/024-shared.directive.yaml"),
            "id: DIRECTIVE_024\n",
        );
        write(
            &org.join("directives/024-shared.directive.yaml"),
            "id: ACME_024\n",
        );
        let mut index = IdentityIndex::from_roots(&built_in, &[org], None);
        scan_all(&mut index);
        // The declared id resolves for reading…
        assert_eq!(
            index.declared_id(ArtifactKind::Directive, "ACME_024"),
            Some("ACME_024".to_owned())
        );
        // …but a writer cannot store it under a file name another layer uses.
        let error = index
            .stored_spelling(ArtifactKind::Directive, "ACME_024")
            .unwrap_err();
        assert!(matches!(error, DoctrineError::ActivationIdCollision { .. }));
    }

    #[test]
    fn unknown_token_passes_through_the_writers_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        let mut index = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut index);
        assert_eq!(
            index
                .stored_spelling(ArtifactKind::Directive, "not-a-directive")
                .unwrap(),
            "not-a-directive"
        );
        assert_eq!(
            index.lookup_key(Some(ArtifactKind::Directive), "24", false),
            "DIRECTIVE_024"
        );
    }

    #[test]
    fn activation_id_reports_unknown_tokens_but_passes_anti_patterns() {
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        let mut index = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut index);
        let mut warnings = Vec::new();
        assert_eq!(
            index.activation_id(ArtifactKind::Directive, "24", &mut warnings),
            "DIRECTIVE_024"
        );
        assert_eq!(warnings.len(), 1);
        let mut warnings = Vec::new();
        assert_eq!(
            index.activation_id(ArtifactKind::AntiPattern, "SMELL", &mut warnings),
            "SMELL"
        );
        assert!(warnings.is_empty());
    }
}
