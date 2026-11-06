//! The doctrine-derived vocabulary of languages that have specialist guidance.
//!
//! Mirrors `src/charter/activation/language_vocabulary.py` (operator decision
//! 2026-09-29, #5284): Spec Kitty doctrine is tech-agnostic, so a language is
//! *known* only when some doctrine artifact is scoped to it through
//! `applies_to_languages`. The vocabulary is read from the packs (built-in, the
//! project overlay and the configured org chain) instead of being hardcoded, so
//! the recogniser stays in step with whatever doctrine ships or a project/org
//! extends (C-001, FR-017).
//!
//! This provider is deliberately a *lightweight YAML scan*: it does not build a
//! DRG graph, does not construct an `ActiveCharterService` (the sole-door
//! activation gate) and does not consult the charter/profile/activation state.
//! It also adds no refusal of its own: a malformed, unreadable or non-UTF-8
//! artifact is skipped exactly as the reference skips it, and an artifact whose
//! scope is a phrase, number, `null` or a `C#`-style token contributes nothing.
//!
//! The memoised provider is [`LanguageVocabularyCache`], an explicit,
//! caller-owned LRU bounded to [`DEFAULT_CACHE_CAPACITY`] roots — the native
//! equivalent of the reference's module-level
//! `functools.lru_cache(maxsize=8)`. Keeping it explicit (rather than a hidden
//! process-global) is what lets a test or a long-lived consumer clear it
//! deterministically; no performance guarantee is claimed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_yaml_ng::Value;
use sk_charter::ArtifactKind;

use crate::error::DoctrineError;

/// The artifact field whose values scope an artifact to languages.
pub const SCOPE_FIELD: &str = "applies_to_languages";

/// The five artifact kinds whose models carry `applies_to_languages`.
///
/// The reference list is exactly `TACTIC`, `STYLEGUIDE`, `TOOLGUIDE`,
/// `PROCEDURE`, `AGENT_PROFILE`; directives, paradigms, glossary packs, skills
/// and assets are not scanned.
pub const LANGUAGE_SCOPED_KINDS: [ArtifactKind; 5] = [
    ArtifactKind::Tactic,
    ArtifactKind::Styleguide,
    ArtifactKind::Toolguide,
    ArtifactKind::Procedure,
    ArtifactKind::AgentProfile,
];

/// The bounded number of roots the scoped-vocabulary memo keeps.
pub const DEFAULT_CACHE_CAPACITY: usize = 8;

/// The project-tier pack root (`resolve_project_root`'s first candidate).
const PROJECT_PACK_DIR: &str = ".kittify/charter-packs";

/// `resolve_project_root`'s code-local built-in-layer fallback candidate.
const LEGACY_PROJECT_LAYER: &str = "src/charter/offering";

/// Whether `item` is a scope string that is a single word.
///
/// The reference is `isinstance(item, str) and re.fullmatch(r"\w+", item.strip())`:
/// numbers/`null`/booleans are not strings, phrases and `C#`/`C++` are not
/// words, and the empty string is not a word.
fn language_word(item: &Value) -> Option<String> {
    let text = item.as_str()?.trim();
    if text.is_empty() || !text.chars().all(is_word_char) {
        return None;
    }
    Some(text.to_owned())
}

/// Python `\w` for `str` patterns: alphanumerics plus underscore.
///
/// Declared edge: `char::is_alphanumeric()` is `Alphabetic | Numeric`, which
/// agrees with `str.isalnum()` for the letter/digit categories that matter here;
/// a combining mark (Mn) is not a word character on either side.
fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The files under `directory` whose name matches `*<suffix>`, recursively.
///
/// The reference glob is `directory.rglob(kind.glob_pattern)`; every shipped
/// pattern is a single `*` plus a literal suffix, and `**` does not descend
/// through directory symlinks. Symlinked *files* are still read, and dot-prefixed
/// names are matched (`pathlib` glob does not hide them).
fn files_with_suffix(directory: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_files(directory, suffix, &mut found);
    found.sort();
    found
}

fn collect_files(directory: &Path, suffix: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            // `pathlib`'s `**` never descends into a directory symlink.
            if !file_type.is_symlink() {
                collect_files(&path, suffix, found);
            }
            continue;
        }
        let matches = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.ends_with(suffix));
        if matches {
            found.push(path);
        }
    }
}

/// The raw `applies_to_languages` tokens of every artifact under `directory`.
fn scoped_tokens_in_dir(directory: &Path, suffix: &str) -> BTreeSet<String> {
    if !directory.is_dir() {
        return BTreeSet::new();
    }
    let mut tokens: Vec<String> = Vec::new();
    for path in files_with_suffix(directory, suffix) {
        // The reference skips an unreadable or non-UTF-8 file, and a file whose
        // text never mentions the scope field (cheap pre-filter before parsing).
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !text.contains(SCOPE_FIELD) {
            continue;
        }
        let Some(document) = crate::language_yaml::parse_safe(&text) else {
            continue;
        };
        let Some(scope) = document
            .as_mapping()
            .and_then(|mapping| mapping.get(SCOPE_FIELD))
        else {
            continue;
        };
        let Some(items) = scope.as_sequence() else {
            continue;
        };
        tokens.extend(items.iter().filter_map(language_word));
    }
    crate::profile::normalize_languages(&tokens)
        .into_iter()
        .collect()
}

/// The built-in `packs/built-in` root for this read.
///
/// `packs_root` accepts either the documented parent (`<...>/packs`) or the
/// built-in leaf (`<...>/packs/built-in`), normalized through the one shared
/// helper. With no explicit value the process `SPEC_KITTY_PACKS_ROOT` and the
/// ancestor walk of [`crate::org_charter::built_in_search`] are used, anchored
/// on `repo_root` (the current directory for an in-memory, rootless read).
pub(crate) fn built_in_root(
    repo_root: Option<&Path>,
    packs_root: Option<&Path>,
) -> Result<PathBuf, DoctrineError> {
    let anchor = match repo_root {
        Some(root) => root.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let search = match packs_root {
        Some(root) => sk_charter::BuiltInSearch::from_env(&anchor)
            .with_packs_root_env(crate::org_charter::normalize_packs_root(root)),
        None => crate::org_charter::built_in_search(&anchor),
    };
    Ok(search.find_root()?)
}

/// `resolve_project_root`: the first existing project-layer candidate.
///
/// The project pack root wins; the code-local built-in-layer path is the legacy
/// fallback. The retired repo-root `doctrine/` directory is not a candidate.
fn project_root(repo_root: &Path) -> Option<PathBuf> {
    let pack_root = repo_root.join(PROJECT_PACK_DIR);
    if pack_root.is_dir() {
        return Some(pack_root);
    }
    let legacy = repo_root.join(LEGACY_PROJECT_LAYER);
    legacy.is_dir().then_some(legacy)
}

/// The existing configured org pack roots, in declaration order.
///
/// This is the lenient chain the reference uses for this scan
/// (`resolve_pack_chain(..., strict=False)` → `resolve_existing_org_roots`):
/// an unparseable or malformed registry contributes nothing (the reference
/// warns and reads an empty registry), and a declared but not-yet-fetched root
/// is dropped. A `local_path` that cannot be expanded (unset environment
/// variable) is a real failure on both sides and is propagated.
fn existing_org_roots(repo_root: &Path) -> Result<Vec<PathBuf>, DoctrineError> {
    let packs = match sk_charter::load_org_pack_registry(repo_root, &sk_charter::Limits::default())
    {
        Ok(packs) => packs,
        // Lenient registry read: the reference warns and disables the org layer.
        Err(_) => return Ok(Vec::new()),
    };
    let expansion = sk_charter::PathExpansion::from_env();
    let mut roots = Vec::new();
    for pack in &packs {
        let base = pack.local_path_root(repo_root, &expansion)?;
        let root = match &pack.subdir {
            None => base,
            Some(subdir) => match pack.effective_root(repo_root, &expansion) {
                Ok(root) => root,
                // The reference resolves with `strict=False`, so a not-yet
                // materialised pack root is not an error by itself.
                Err(_) if !base.exists() => base.join(subdir),
                Err(error) => return Err(error.into()),
            },
        };
        if root.exists() {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// Built-in, project-overlay and org-pack directories that may hold `kind`
/// artifacts.
fn kind_directories(
    kind: ArtifactKind,
    repo_root: Option<&Path>,
    packs_root: Option<&Path>,
) -> Result<Vec<PathBuf>, DoctrineError> {
    let built_in = built_in_root(repo_root, packs_root)?;
    let mut directories = vec![built_in.join(kind.plural())];
    let Some(repo_root) = repo_root else {
        return Ok(directories);
    };
    if let Some(project) = project_root(repo_root) {
        directories.push(project.join(kind.project_dir()));
    }
    for org_root in existing_org_roots(repo_root)? {
        // Both org spellings: the pack's plural directory and the project-tier
        // directory name (identical for three of the five kinds — deduplicated).
        for name in [kind.plural(), kind.project_dir()] {
            let directory = org_root.join(name);
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    Ok(directories)
}

/// Every language some doctrine artifact is scoped to.
///
/// `repo_root == None` reads the built-in packs only (an in-memory compile).
/// Reserved (`unknown`) and sentinel (`any`/`all`) tokens are never vocabulary.
/// The result is the union over the five language-scoped kinds; the scan is
/// uncached — use [`LanguageVocabularyCache`] for the memoised provider.
pub fn scoped_language_vocabulary(
    repo_root: Option<&Path>,
    packs_root: Option<&Path>,
) -> Result<BTreeSet<String>, DoctrineError> {
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for kind in LANGUAGE_SCOPED_KINDS {
        let suffix = kind.glob_pattern().trim_start_matches('*');
        for directory in kind_directories(kind, repo_root, packs_root)? {
            tokens.extend(scoped_tokens_in_dir(&directory, suffix));
        }
    }
    tokens.retain(|token| !is_reserved_or_sentinel(token));
    Ok(tokens)
}

/// The pack roots the invocation profile catalogue reads.
///
/// This is the **catalogue's** root resolution, and it deliberately reuses the
/// exact same private helpers as the language-vocabulary scan rather than
/// copying a chain:
///
/// * `built_in` is the module's `built_in_root` (the native
///   `SPEC_KITTY_PACKS_ROOT` / ancestor-walk seam);
/// * `org_roots` is the module's `existing_org_roots`, the reference's lenient
///   `resolve_pack_chain(strict=False)` chain (unparseable registry ⇒ empty,
///   not-yet-materialised root ⇒ dropped, unexpandable `local_path` ⇒ real
///   error);
/// * `project` is the `.kittify/charter-packs` overlay when that directory
///   exists — the reference's `ActiveCharterService` project layer;
/// * `legacy_project` is `.kittify/profiles` when it exists: the invocation
///   registry's separate, **ungated and language-unfiltered** overlay. It is
///   reported distinctly because it has different gates, not because it has a
///   different loader.
///
/// The strict `configured_org_roots` / `context_roots` chains stay strict for
/// their own consumers; routing the catalogue through them would invent
/// refusals the reference does not have.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CatalogueRoots {
    /// The built-in pack root (`packs/built-in`).
    pub built_in: PathBuf,
    /// Configured org pack roots, in declaration order.
    pub org_roots: Vec<PathBuf>,
    /// The project charter-pack root (`.kittify/charter-packs`), when present.
    pub project: Option<PathBuf>,
    /// The legacy `.kittify/profiles` invocation overlay, when present.
    pub legacy_project: Option<PathBuf>,
}

/// Resolve the catalogue roots for `repo_root`.
pub fn catalogue_roots(repo_root: &Path) -> Result<CatalogueRoots, DoctrineError> {
    Ok(CatalogueRoots {
        built_in: built_in_root(Some(repo_root), None)?,
        org_roots: existing_org_roots(repo_root)?,
        project: existing_project_pack_root(repo_root),
        legacy_project: existing_legacy_profile_root(repo_root),
    })
}

/// The `.kittify/charter-packs` overlay root, when it exists.
pub(crate) fn existing_project_pack_root(repo_root: &Path) -> Option<PathBuf> {
    let root = repo_root.join(PROJECT_PACK_DIR);
    root.is_dir().then_some(root)
}

/// The legacy `.kittify/profiles` invocation overlay root, when it exists.
fn existing_legacy_profile_root(repo_root: &Path) -> Option<PathBuf> {
    let root = repo_root.join(".kittify").join("profiles");
    root.is_dir().then_some(root)
}

/// Whether `token` is `unknown` (reserved) or `any`/`all` (sentinel).
fn is_reserved_or_sentinel(token: &str) -> bool {
    crate::language_scope::RESERVED_LANGUAGE_TOKENS.contains(&token)
        || crate::language_scope::SENTINEL_LANGUAGE_TOKENS.contains(&token)
}

#[derive(Clone, Debug)]
struct CacheEntry {
    repo_root: Option<PathBuf>,
    packs_root: Option<PathBuf>,
    vocabulary: Arc<BTreeSet<String>>,
}

impl CacheEntry {
    fn matches(&self, repo_root: Option<&Path>, packs_root: Option<&Path>) -> bool {
        self.repo_root.as_deref() == repo_root && self.packs_root.as_deref() == packs_root
    }
}

/// The bounded, caller-owned memo for [`scoped_language_vocabulary`].
///
/// The reference memoises the provider with `functools.lru_cache(maxsize=8)`
/// keyed on `repo_root`. This is the same bound and eviction policy, with two
/// declared differences: the cache belongs to the caller (not to a process
/// global), and the explicit `packs_root` seam is part of the key so two
/// built-in roots can never alias one entry. A failed scan is never cached.
#[derive(Clone, Debug)]
pub struct LanguageVocabularyCache {
    capacity: usize,
    entries: Vec<CacheEntry>,
}

impl Default for LanguageVocabularyCache {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageVocabularyCache {
    /// A cache bounded to [`DEFAULT_CACHE_CAPACITY`] roots.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CACHE_CAPACITY)
    }

    /// A cache bounded to `capacity` roots (at least one).
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Vec::new(),
        }
    }

    /// The maximum number of roots retained.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The number of cached roots.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no root is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop every cached root (the reference's `cache_clear()`).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The cached roots, most-recently-used first.
    pub fn keys(&self) -> impl Iterator<Item = (Option<&Path>, Option<&Path>)> {
        self.entries
            .iter()
            .map(|entry| (entry.repo_root.as_deref(), entry.packs_root.as_deref()))
    }

    /// Whether `(repo_root, packs_root)` is currently cached.
    pub fn contains(&self, repo_root: Option<&Path>, packs_root: Option<&Path>) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.matches(repo_root, packs_root))
    }

    /// The memoised vocabulary for one root pair.
    pub fn vocabulary(
        &mut self,
        repo_root: Option<&Path>,
        packs_root: Option<&Path>,
    ) -> Result<Arc<BTreeSet<String>>, DoctrineError> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.matches(repo_root, packs_root))
        {
            let entry = self.entries.remove(index);
            let vocabulary = Arc::clone(&entry.vocabulary);
            self.entries.insert(0, entry);
            return Ok(vocabulary);
        }
        let vocabulary = Arc::new(scoped_language_vocabulary(repo_root, packs_root)?);
        self.entries.insert(
            0,
            CacheEntry {
                repo_root: repo_root.map(Path::to_path_buf),
                packs_root: packs_root.map(Path::to_path_buf),
                vocabulary: Arc::clone(&vocabulary),
            },
        );
        self.entries.truncate(self.capacity);
        Ok(vocabulary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_is_bounded_to_the_reference_size() {
        let cache = LanguageVocabularyCache::new();
        assert_eq!(cache.capacity(), 8);
        assert!(cache.is_empty());
        assert_eq!(LanguageVocabularyCache::default().capacity(), 8);
    }

    #[test]
    fn word_tokens_exclude_non_words() {
        let strings = |text: &str| Value::String(text.to_owned());
        assert_eq!(language_word(&strings(" zig ")).as_deref(), Some("zig"));
        assert_eq!(language_word(&strings("C#")), None);
        assert_eq!(language_word(&strings("c++")), None);
        assert_eq!(language_word(&strings("objective-c")), None);
        assert_eq!(language_word(&strings("a b")), None);
        assert_eq!(language_word(&strings("")), None);
        assert_eq!(language_word(&Value::Number(3.into())), None);
        assert_eq!(language_word(&Value::Null), None);
        assert_eq!(language_word(&Value::Bool(true)), None);
        // `\w` is Unicode-aware on both sides.
        assert_eq!(language_word(&strings("日本語")).as_deref(), Some("日本語"));
        assert_eq!(language_word(&strings("_")).as_deref(), Some("_"));
    }

    #[test]
    fn reserved_and_sentinel_tokens_are_never_vocabulary() {
        for token in ["unknown", "any", "all"] {
            assert!(is_reserved_or_sentinel(token));
        }
        for token in ["python", "jvm", "unknownish"] {
            assert!(!is_reserved_or_sentinel(token));
        }
    }
}
