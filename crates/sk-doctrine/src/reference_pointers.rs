//! Source reference-pointer selection: filter → distribution → offering lookup.
//!
//! This is the executable reference pipeline
//! `charter/activation/context_renderers/reference_pointers.py`, not a review of
//! it:
//!
//! ```python
//! filtered = _filter_references_for_action(references, action)
//! distributed = _distribute_references_across_kinds(filtered, action)
//! index = _reference_source_index(offering_root)      # process-wide cache
//! for ref in distributed:
//!     resolved = _resolve_reference_source(ref, index)
//!     if resolved is None: continue                   # not a limit consumer
//!     selected.append((ref, resolved))
//!     if len(selected) >= limit: break                # AFTER appending
//! ```
//!
//! Ownership (ONE authority per concern):
//!
//! * [`crate::reference_catalog::ReferenceEntry`] stays the authored catalogue
//!   entry and its `catalog.references` length stays `references_count`; this
//!   module never re-counts and never re-reads `charter.yaml`;
//! * [`SelectedReference`] is the ONE resolved-pointer model — the entry plus
//!   its opening path and the prepared `str(Path)` text — so the renderer needs
//!   no second schema and no scalar re-projection;
//! * the offering index is cached per **supplied** root `Path` (no resolve or
//!   canonicalisation is invented), frozen after the first scan, and independent
//!   roots stay independent;
//! * Source's entry cap belongs here (default [`REFERENCE_POINTER_LIMIT`]), and
//!   is applied after each successful append, so `limit = 0` still yields one
//!   resolved pointer.
//!
//! Source policies reproduced exactly:
//!
//! * action filter: non-`local_support` entries always pass; a `local_support`
//!   entry passes when its summary has no `\(action:\s*(\w+)\)` match (global)
//!   or the first match's group, stripped and lowered, equals the lowered
//!   action. `\s`/`\w`/`lower()` are the pinned Kernel CPython 3.11.15/UCD14
//!   authorities, not Rust Unicode;
//! * distribution: group by first-seen kind, rotate each group by
//!   `Σ (position+1)·ord(char) mod len`, then round-robin one member per kind
//!   per round; no sorted map, dedupe, head slice or total_cmp;
//! * index: every `*.yaml` candidate under the seven offering kind directories,
//!   sorted by path **component** tuples, contributing its filename before the
//!   first dot and, for a numeric prefix, a `DIRECTIVE_<digits>` alias, both
//!   first-wins (`.setdefault`);
//! * lookup: the id after the first `:`, then the local-path `Path.stem` minus
//!   one literal `<kind>-` prefix. Both phases are exact Python text.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use sk_charter::ArtifactKind;
use sk_kernel::python_os_encoding::{OsDecodeError, OsEncodingContext};
use sk_kernel::python_text;
use sk_kernel::source_glob::{SourceGlobError, source_rglob_yaml};
use sk_kernel::{
    PythonString, source_name_before_first_dot, source_path_component_key, source_path_is_dir,
    source_path_text, source_pure_path_stem,
};

use crate::reference_catalog::ReferenceEntry;

/// Source `_REFERENCE_POINTER_LIMIT`.
pub const REFERENCE_POINTER_LIMIT: usize = 12;

/// The seven reference kinds with an offering source directory, in Source's
/// curated order (`_REFERENCE_KIND_DIRS`), derived from the ONE kind authority.
pub const REFERENCE_KINDS: [ArtifactKind; 7] = [
    ArtifactKind::Tactic,
    ArtifactKind::Directive,
    ArtifactKind::Paradigm,
    ArtifactKind::Procedure,
    ArtifactKind::Styleguide,
    ArtifactKind::Toolguide,
    ArtifactKind::AgentProfile,
];

/// One resolved pointer: the authored entry, the opening path, its exact text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedReference {
    /// The authored catalogue entry (no re-typed authored fields).
    pub entry: ReferenceEntry,
    /// The offering-source path the entry OPENED.
    pub source: PathBuf,
    /// `str(Path)` of [`Self::source`] under the captured codec.
    pub source_text: PythonString,
}

/// Typed causes for the index/lookup/preparation phases.
#[derive(Debug, thiserror::Error)]
pub enum ReferencePointerError {
    /// `Path.is_dir()` on a kind directory raised an unignored OSError,
    /// such as EACCES. Source absorbs ValueError separately as false.
    #[error("could not inspect offering source directory {path}: {source}")]
    Inspect {
        /// The kind directory.
        path: PathBuf,
        /// The actual filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The offering scan failed outside Source's ignored cases.
    #[error(transparent)]
    Glob(#[from] SourceGlobError),
    /// A path cannot be decoded as Source text under the captured codec.
    #[error("could not decode offering source path {path}: {source}")]
    Decode {
        /// The undecodable path.
        path: PathBuf,
        /// The captured codec's cause.
        #[source]
        source: OsDecodeError,
    },
    /// The process-wide index cache is poisoned.
    #[error("reference source index cache is unavailable")]
    CacheUnavailable,
}

type KindIndex = HashMap<Vec<u32>, PathBuf>;

/// The `kind -> {lookup key -> source path}` index for one supplied root.
#[derive(Clone, Debug, Default)]
pub struct ReferenceSourceIndex {
    /// One entry per [`REFERENCE_KINDS`] member, in that order.
    pub kinds: Vec<(ArtifactKind, KindIndex)>,
}

impl ReferenceSourceIndex {
    /// The lookup map for one singular kind spelling (`None` when the kind has
    /// no offering source directory, e.g. project-generated `user_profile`).
    #[must_use]
    pub fn kind_index(&self, kind: &PythonString) -> Option<&KindIndex> {
        self.kinds
            .iter()
            .find(|(candidate, _)| *kind == PythonString::from_scalars(candidate.canonical()))
            .map(|(_, index)| index)
    }
}

/// Source's process-wide cache keyed by supplied Path component identity,
/// not Rust Path equality (which would coalesce POSIX double-root spellings).
type SourcePathKey = Vec<PythonString>;
static REFERENCE_SOURCE_INDEX_CACHE: OnceLock<
    Mutex<BTreeMap<SourcePathKey, Arc<ReferenceSourceIndex>>>,
> = OnceLock::new();

/// Source `_filter_references_for_action`.
#[must_use]
pub fn filter_references_for_action<'a>(
    references: &'a [ReferenceEntry],
    action: &PythonString,
) -> Vec<&'a ReferenceEntry> {
    let lowered_action =
        PythonString::from_codepoints(&python_text::lower_codepoints(action.codepoints()))
            .expect("lowered codepoints are a validated PythonString");
    let local_support = PythonString::from_scalars("local_support");
    let mut filtered = Vec::new();
    for reference in references {
        if reference.kind != local_support {
            filtered.push(reference);
            continue;
        }
        match scoped_action(&reference.summary) {
            Some(scoped) => {
                let lowered = PythonString::from_codepoints(&python_text::lower_codepoints(
                    scoped.codepoints(),
                ))
                .expect("lowered codepoints are a validated PythonString");
                if lowered == lowered_action {
                    filtered.push(reference);
                }
            }
            // No action scope in the summary → included globally.
            None => filtered.push(reference),
        }
    }
    filtered
}

/// The first literal `\(action:\s*(\w+)\)` group in `summary`.
///
/// This is re.search's leftmost match using pinned whitespace/word authorities.
/// A closing parenthesis must follow the nonempty word run; EOF is no match.
fn scoped_action(summary: &PythonString) -> Option<PythonString> {
    let codepoints = summary.codepoints();
    let literal: [u32; 7] = [
        u32::from('a'),
        u32::from('c'),
        u32::from('t'),
        u32::from('i'),
        u32::from('o'),
        u32::from('n'),
        u32::from(':'),
    ];
    for start in 0..codepoints.len() {
        if codepoints[start] != u32::from('(') {
            continue;
        }
        let mut cursor = start + 1;
        if codepoints.get(cursor..cursor + literal.len()) != Some(&literal[..]) {
            continue;
        }
        cursor += literal.len();
        while cursor < codepoints.len() && python_text::is_whitespace(codepoints[cursor]) {
            cursor += 1;
        }
        let run_start = cursor;
        while cursor < codepoints.len() && python_text::is_word(codepoints[cursor]) {
            cursor += 1;
        }
        if cursor == run_start {
            continue;
        }
        // ')' is not a word unit, so only the boundary after this nonempty
        // run can close the match. EOF is a nonmatch, not an indexing panic.
        if codepoints.get(cursor) == Some(&u32::from(')')) {
            return Some(
                PythonString::from_codepoints(&codepoints[run_start..cursor])
                    .expect("substring of a validated PythonString"),
            );
        }
    }
    None
}

/// Source's weighted sum modulo one nonempty group's length. Reduce per term
/// rather than assuming the unbounded Python sum fits u128. Each individual
/// 32/64-bit position times a Python code point fits u128; reduced addition
/// cannot overflow either.
fn action_offset_mod(action: &PythonString, modulus: usize) -> usize {
    debug_assert!(modulus > 0);
    let modulus = modulus as u128;
    let mut offset = 0u128;
    for (position, &code) in action.codepoints().iter().enumerate() {
        let term = ((position as u128 + 1) * u128::from(code)) % modulus;
        offset = (offset + term) % modulus;
    }
    offset as usize
}

/// Source `_distribute_references_across_kinds`.
#[must_use]
pub fn distribute_references_across_kinds<'a>(
    references: Vec<&'a ReferenceEntry>,
    action: &PythonString,
) -> Vec<&'a ReferenceEntry> {
    let mut by_kind: Vec<(Vec<u32>, Vec<&'a ReferenceEntry>)> = Vec::new();
    for reference in references {
        let key = reference.kind.codepoints().to_vec();
        match by_kind.iter_mut().find(|(kind, _)| *kind == key) {
            Some((_, members)) => members.push(reference),
            None => by_kind.push((key, vec![reference])),
        }
    }

    let mut rotated: Vec<Vec<&'a ReferenceEntry>> = Vec::with_capacity(by_kind.len());
    for (_, members) in by_kind {
        let start = if members.is_empty() {
            0
        } else {
            action_offset_mod(action, members.len())
        };
        let mut order = Vec::with_capacity(members.len());
        order.extend_from_slice(&members[start..]);
        order.extend_from_slice(&members[..start]);
        rotated.push(order);
    }

    let mut ordered = Vec::new();
    let mut round = 0usize;
    loop {
        let mut added = false;
        for members in &rotated {
            if let Some(reference) = members.get(round) {
                ordered.push(*reference);
                added = true;
            }
        }
        if !added {
            break;
        }
        round += 1;
    }
    ordered
}

/// Source `_reference_source_index`, cached by the supplied root.
///
/// # Errors
///
/// [`ReferencePointerError::Inspect`], [`ReferencePointerError::Glob`],
/// [`ReferencePointerError::Decode`] or [`ReferencePointerError::CacheUnavailable`].
pub fn reference_source_index(
    offering_root: &Path,
    codec: &OsEncodingContext,
) -> Result<Arc<ReferenceSourceIndex>, ReferencePointerError> {
    // Rust Path equality coalesces '/' and '//', unlike POSIX pathlib. Reuse
    // the canonical Source component key without resolving symlink aliases.
    let key = source_path_component_key(offering_root, codec).map_err(|source| {
        ReferencePointerError::Decode {
            path: offering_root.to_path_buf(),
            source,
        }
    })?;
    let cache = REFERENCE_SOURCE_INDEX_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(cached) = cache
        .lock()
        .map_err(|_| ReferencePointerError::CacheUnavailable)?
        .get(&key)
    {
        return Ok(Arc::clone(cached));
    }
    let built = Arc::new(build_reference_source_index(offering_root, codec)?);
    let mut guard = cache
        .lock()
        .map_err(|_| ReferencePointerError::CacheUnavailable)?;
    Ok(Arc::clone(guard.entry(key).or_insert(built)))
}

fn build_reference_source_index(
    offering_root: &Path,
    codec: &OsEncodingContext,
) -> Result<ReferenceSourceIndex, ReferencePointerError> {
    let mut kinds = Vec::with_capacity(REFERENCE_KINDS.len());
    for kind in REFERENCE_KINDS {
        let base = offering_root.join(kind.plural());
        let mut kind_index: KindIndex = HashMap::new();
        if source_path_is_dir(&base).map_err(|source| ReferencePointerError::Inspect {
            path: base.clone(),
            source,
        })? {
            let candidates = source_rglob_yaml(&base)?;
            // `sorted(Path)` compares component tuples: decode once, sort, then
            // build the first-wins index in that order.
            let mut keyed = Vec::with_capacity(candidates.len());
            for path in candidates {
                let key = source_path_component_key(&path, codec).map_err(|source| {
                    ReferencePointerError::Decode {
                        path: path.clone(),
                        source,
                    }
                })?;
                keyed.push((key, path));
            }
            keyed.sort_by(|left, right| left.0.cmp(&right.0));
            for (_, path) in keyed {
                let name = codec
                    .decode_os_str(
                        path.file_name()
                            .expect("rglob candidates have a final component"),
                    )
                    .map_err(|source| ReferencePointerError::Decode {
                        path: path.clone(),
                        source,
                    })?;
                let stem = source_name_before_first_dot(&name);
                kind_index
                    .entry(stem.codepoints().to_vec())
                    .or_insert_with(|| path.clone());
                if let Some(alias) = directive_alias(&stem) {
                    kind_index
                        .entry(alias.codepoints().to_vec())
                        .or_insert_with(|| path.clone());
                }
            }
        }
        kinds.push((kind, kind_index));
    }
    Ok(ReferenceSourceIndex { kinds })
}

/// `re.match(r"^(\d+)-", stem)` → `DIRECTIVE_<digits>` (`\d` = Unicode `Nd`).
fn directive_alias(stem: &PythonString) -> Option<PythonString> {
    let codepoints = stem.codepoints();
    let mut end = 0usize;
    while end < codepoints.len() && python_text::is_decimal(codepoints[end]) {
        end += 1;
    }
    if end == 0 || codepoints.get(end) != Some(&u32::from('-')) {
        return None;
    }
    let prefix = PythonString::from_scalars("DIRECTIVE_");
    let mut alias: Vec<u32> = prefix.codepoints().to_vec();
    alias.extend_from_slice(&codepoints[..end]);
    Some(PythonString::from_codepoints(&alias).expect("validated alias text"))
}

/// Source `_resolve_reference_source`.
#[must_use]
pub fn resolve_reference_source(
    reference: &ReferenceEntry,
    index: &ReferenceSourceIndex,
) -> Option<PathBuf> {
    let kind_index = index.kind_index(&reference.kind)?;
    if kind_index.is_empty() {
        return None;
    }

    let id = reference.id.codepoints();
    let artifact_id = match id.iter().position(|&code| code == u32::from(':')) {
        Some(position) => &id[position + 1..],
        None => id,
    };
    if let Some(resolved) = kind_index.get(artifact_id) {
        return Some(resolved.clone());
    }

    let mut slug = source_pure_path_stem(&reference.local_path);
    let mut prefix = reference.kind.clone();
    prefix.push_scalars("-");
    if let Some(stripped) = strip_python_prefix(&slug, &prefix) {
        slug = stripped;
    }
    kind_index.get(slug.codepoints()).cloned()
}

fn strip_python_prefix(text: &PythonString, prefix: &PythonString) -> Option<PythonString> {
    let codepoints = text.codepoints();
    let prefix = prefix.codepoints();
    if codepoints.len() < prefix.len() || &codepoints[..prefix.len()] != prefix {
        return None;
    }
    Some(
        PythonString::from_codepoints(&codepoints[prefix.len()..])
            .expect("substring of a validated PythonString"),
    )
}

/// Source `_select_reference_pointers` with the default cap.
///
/// # Errors
///
/// The index/decode causes of [`reference_source_index`] and the exact path
/// preparation.
pub fn select_reference_pointers(
    references: &[ReferenceEntry],
    action: &PythonString,
    offering_root: &Path,
    codec: &OsEncodingContext,
) -> Result<Vec<SelectedReference>, ReferencePointerError> {
    select_reference_pointers_with_limit(
        references,
        action,
        offering_root,
        codec,
        REFERENCE_POINTER_LIMIT,
    )
}

/// [`select_reference_pointers`] with an explicit Source `limit`.
///
/// # Errors
///
/// The index/decode causes of [`reference_source_index`] and the exact path
/// preparation.
pub fn select_reference_pointers_with_limit(
    references: &[ReferenceEntry],
    action: &PythonString,
    offering_root: &Path,
    codec: &OsEncodingContext,
    limit: usize,
) -> Result<Vec<SelectedReference>, ReferencePointerError> {
    let filtered = filter_references_for_action(references, action);
    let distributed = distribute_references_across_kinds(filtered, action);
    let index = reference_source_index(offering_root, codec)?;

    let mut selected = Vec::new();
    for reference in distributed {
        let Some(path) = resolve_reference_source(reference, &index) else {
            continue;
        };
        let source_text =
            source_path_text(&path, codec).map_err(|source| ReferencePointerError::Decode {
                path: path.clone(),
                source,
            })?;
        selected.push(SelectedReference {
            entry: reference.clone(),
            source: path,
            source_text,
        });
        if selected.len() >= limit {
            break;
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, id: &str, local_path: &str, summary: &str) -> ReferenceEntry {
        ReferenceEntry {
            id: PythonString::from_scalars(id),
            title: PythonString::default(),
            local_path: PythonString::from_scalars(local_path),
            kind: PythonString::from_scalars(kind),
            summary: PythonString::from_scalars(summary),
        }
    }

    fn ids(references: &[&ReferenceEntry]) -> Vec<String> {
        references
            .iter()
            .map(|reference| reference.id.to_scalar_string().unwrap())
            .collect()
    }

    #[test]
    fn local_support_action_scope_is_the_reference_regex() {
        let references = vec![
            entry("local_support", "exact", "", "(action: implement)"),
            entry("local_support", "spaces", "", "(action:   IMPLEMENT  )"),
            entry(
                "local_support",
                "unicode_space",
                "",
                "(action:\u{2003}implement)",
            ),
            entry("local_support", "nb_space", "", "(action:\u{a0}review)"),
            entry("local_support", "other_action", "", "(action:review)"),
            entry("local_support", "global", "", "no action scope"),
            entry("local_support", "empty_group", "", "(action:  )"),
            entry(
                "local_support",
                "first_match_wins",
                "",
                "(action:review) (action:implement)",
            ),
            entry(
                "local_support",
                "second_match",
                "",
                "(action:implement) (action:review)",
            ),
            entry(
                "local_support",
                "substring",
                "",
                "pre(action:implement)post",
            ),
            entry(
                "local_support",
                "combining",
                "",
                "(action:impleme\u{301}nt)",
            ),
            entry("directive", "other_kind", "", "(action:review)"),
        ];
        let action = PythonString::from_scalars("implement");
        let filtered = filter_references_for_action(&references, &action);
        assert_eq!(
            ids(&filtered),
            vec![
                "exact",
                "spaces",
                "unicode_space",
                "global",
                "empty_group",
                "second_match",
                "substring",
                "combining",
                "other_kind",
            ]
        );
    }

    #[test]
    fn distribution_rotates_then_round_robins() {
        // The real expectation, from the Source probe (`implement`, offset 4881):
        // directive rotation 4881 % 4 = 1 → d1,d2,d3,d0; tactic 4881 % 3 = 0 →
        // t0,t1,t2; paradigm p0; round-robin interleaves them.
        let references: Vec<ReferenceEntry> = (0..4)
            .map(|index| entry("directive", &format!("d{index}"), "", ""))
            .chain((0..3).map(|index| entry("tactic", &format!("t{index}"), "", "")))
            .chain(std::iter::once(entry("paradigm", "p0", "", "")))
            .collect();
        let borrowed: Vec<&ReferenceEntry> = references.iter().collect();
        let ordered =
            distribute_references_across_kinds(borrowed, &PythonString::from_scalars("implement"));
        assert_eq!(
            ids(&ordered),
            vec!["d1", "t0", "p0", "d2", "t1", "d3", "t2", "d0"]
        );
        assert_eq!(
            distribute_references_across_kinds(
                references.iter().chain(references.iter()).collect(),
                &PythonString::from_scalars("implement"),
            )
            .len(),
            16,
            "no dedupe or drop"
        );
        assert_eq!(
            action_offset_mod(&PythonString::from_scalars("implement"), 10000),
            4881
        );
        assert_eq!(
            action_offset_mod(&PythonString::from_scalars("review"), 10000),
            2309
        );
        assert_eq!(
            action_offset_mod(&PythonString::from_scalars("plan"), 10000),
            1059
        );
    }

    #[test]
    fn resolve_uses_first_colon_and_pathlib_stem() {
        let kind = ArtifactKind::Directive;
        let mut kind_index = KindIndex::new();
        kind_index.insert(
            PythonString::from_scalars("dependency-hygiene")
                .codepoints()
                .to_vec(),
            PathBuf::from("directives/dependency-hygiene.yaml"),
        );
        kind_index.insert(
            PythonString::from_scalars("a").codepoints().to_vec(),
            PathBuf::from("directives/directive-a.yaml"),
        );
        kind_index.insert(
            PythonString::from_scalars("DIRECTIVE_012")
                .codepoints()
                .to_vec(),
            PathBuf::from("directives/012-num.yaml"),
        );
        let index = ReferenceSourceIndex {
            kinds: vec![(kind, kind_index)],
        };
        let resolve = |reference: &ReferenceEntry| resolve_reference_source(reference, &index);
        assert_eq!(
            resolve(&entry("directive", "directive:DIRECTIVE_012", "", "")),
            Some(PathBuf::from("directives/012-num.yaml"))
        );
        assert_eq!(
            resolve(&entry(
                "directive",
                "directive:missing",
                "library/directive-a.md",
                ""
            )),
            Some(PathBuf::from("directives/directive-a.yaml"))
        );
        // Path.stem removes one suffix only: `x.yaml.yaml` → `x.yaml`, not `x`.
        assert_eq!(
            resolve(&entry(
                "directive",
                "directive:missing",
                "a/x.yaml.yaml",
                ""
            )),
            None
        );
        assert_eq!(
            resolve(&entry(
                "directive",
                "directive:missing",
                "tactics/dependency-hygiene.tactic.yaml",
                ""
            )),
            None
        );
        assert_eq!(resolve(&entry("directive", "directive:a:b", "", "")), None);
        assert_eq!(resolve(&entry("nope", "nope:x", "", "")), None);
    }

    #[test]
    fn directive_alias_needs_a_decimal_prefix_and_hyphen() {
        let scalar =
            |value: Option<PythonString>| value.map(|value| value.to_scalar_string().unwrap());
        assert_eq!(
            scalar(directive_alias(&PythonString::from_scalars("001-alpha"))),
            Some("DIRECTIVE_001".to_owned())
        );
        assert_eq!(
            scalar(directive_alias(&PythonString::from_scalars("12-num"))),
            Some("DIRECTIVE_12".to_owned())
        );
        assert_eq!(
            scalar(directive_alias(&PythonString::from_scalars("001alpha"))),
            None
        );
        assert_eq!(
            scalar(directive_alias(&PythonString::from_scalars("alpha"))),
            None
        );
        assert_eq!(
            scalar(directive_alias(&PythonString::from_scalars(
                "\u{660}\u{661}-x"
            ))),
            Some("DIRECTIVE_\u{660}\u{661}".to_owned())
        );
    }
}
