//! The actual lazy legacy `mission_slug` -> `mission_id` metadata resolver.
//!
//! The reference's store (`specify_cli.status.store._SlugResolver`) resolves a
//! legacy slug by *executing* the sibling mission's `meta.json` read, in this
//! exact order:
//!
//! | Phase | The reference's own call | Failure outcome |
//! |---|---|---|
//! | cache | `mission_slug in self._cache` (a dict lookup, so an unhashable value raises `TypeError`) | cached `None` returned |
//! | grammar | `assert_safe_path_segment(slug)` (`value.strip() if value else value`) | `ValueError` -> warning, `None` |
//! | containment | `ensure_within_any(meta_path, roots=[root])` (`resolve(strict=False)` on both sides) | candidate `OSError` -> `ValueError` -> warning, `None`; a **root** resolution failure is outside that wrapper |
//! | exists | `meta_path.exists()` | `_ignore_error` errnos -> `False`; any other `OSError` **escapes** |
//! | read | `meta_path.read_text(encoding="utf-8")` (unbounded, symlink-following, strict UTF-8, no BOM tolerance) | `OSError`/`UnicodeDecodeError` -> `MissionMetaReadError` -> warning, `None` |
//! | decode | `decode_meta(text, on_malformed="raise")` (`json.loads` rules, object-only) | `JSONDecodeError`/`UnicodeDecodeError`/non-object -> `MissionMetaReadError` -> warning, `None`; a digit-ceiling `ValueError` reaches `load_meta_fail_closed`'s `except ValueError` -> `None`; a `RecursionError` **escapes** |
//! | field | `data.get("mission_id") or None` | absent/falsy -> `None` |
//! | cache | `self._cache[mission_slug] = mission_id` | - |
//!
//! That is an execution policy, not an inspection policy: the read is unbounded
//! and follows symlinks (containment is the traversal guard), the decode is
//! Python's `json.loads`, and the *caught* cases become "no `mission_id`" rather
//! than a refusal. The bounded, `O_NOFOLLOW`, regular-file-only inspection
//! capability elsewhere in the workspace is a deliberately different contract
//! and is NOT used here.
//!
//! # Caught versus escaped (CF62)
//!
//! The absorbed `None` above is the reference's *own* catch policy and stays
//! `None`. Anything the reference does **not** catch is reported as
//! [`SourceResolverFailure`] instead of being collapsed into absence — an
//! `exists()` permission failure, a deep-nesting `RecursionError`, or (from the
//! J1 reader) a native operational limit that has no Source counterpart. A
//! consumer therefore can never read an escaped failure as "no `mission_id`".
//!
//! Two implementation notes that keep this the single authority:
//!
//! * the segment grammar and the containment/exists/read steps reuse the
//!   canonical Kernel path helpers (`is_safe_path_segment`,
//!   `resolve_path_non_strict`, `source_path_exists`, `read_source_bytes`) and
//!   the canonical J1 decoder — no second table, reader or path policy;
//! * the cache key is the exact [`PythonString`]; only strings can reach the
//!   cache write (a non-string slug either raises before it or is refused by the
//!   grammar), which is why the key type is exact rather than a lossy copy.
//!
//! Laziness: this resolver is consulted for **lane rows only**. An annotation row
//! never touches `meta.json`, so an annotation-only log beside a corrupt
//! `meta.json` still admits and folds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sk_kernel::PythonString;
use sk_kernel::python_json::{
    PythonJsonDict, PythonJsonErrorKind, PythonJsonValue, python_json_hashable, python_json_str,
    python_json_truthy, python_json_type_name,
};

use crate::source_event::{normalize_object, row_mission_slug};
use crate::source_execution::{
    SourceExecutionPhase, SourceOperational, SourcePythonClass, SourceRaised,
};

const SPECS_DIR: &str = "kitty-specs";
const META_FILENAME: &str = "meta.json";

/// Why the reference's slug resolver failed to answer.
///
/// The absorbed cases (`meta.json` absent, unreadable, non-UTF-8, malformed,
/// non-object, a rejected slug, a containment refusal, an absent `mission_id`)
/// are **not** failures here: the reference catches them and returns `None`, so
/// this type only carries what the reference lets out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceResolverFailure {
    /// A Python exception raised out of the resolver.
    Raised(SourceRaised),
    /// A native decoder limit with no Source counterpart (never a Python class).
    Operational(SourceOperational),
}

impl SourceResolverFailure {
    /// The execution phase.
    pub fn phase(&self) -> SourceExecutionPhase {
        match self {
            Self::Raised(raised) => raised.phase(),
            Self::Operational(operational) => operational.phase(),
        }
    }
}

impl std::fmt::Display for SourceResolverFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Raised(raised) => raised.fmt(formatter),
            Self::Operational(operational) => operational.fmt(formatter),
        }
    }
}

fn raised(
    class: SourcePythonClass,
    phase: SourceExecutionPhase,
    message: impl Into<String>,
) -> SourceResolverFailure {
    SourceResolverFailure::Raised(SourceRaised::new(class, phase, message))
}

/// Cache-backed legacy `mission_slug` -> `mission_id` resolver.
///
/// One instance per store read (the reference builds it inside
/// `_partition_event_stream_from_text`), which is what makes the cache
/// *per call*: a later store read re-reads `meta.json`.
#[derive(Debug, Clone)]
pub struct SourceSlugResolver {
    specs_root: PathBuf,
    cache: BTreeMap<PythonString, Option<PythonJsonValue>>,
}

impl SourceSlugResolver {
    /// Build the resolver for the directory that owns `status.events.jsonl`.
    ///
    /// This is `_find_mission_specs_root`: a **feature-dir-relative** lookup of
    /// the sibling `kitty-specs` directory (pure path arithmetic on the given
    /// feature dir, never a canonical-repo-root resolution), so co-located
    /// `meta.json` files resolve even outside a git repository.
    pub fn new(feature_dir: &Path) -> Self {
        let parent = feature_dir.parent().unwrap_or(feature_dir);
        let parent_is_specs = parent.file_name().and_then(|name| name.to_str()) == Some(SPECS_DIR);
        let grandparent_is_specs = parent
            .parent()
            .and_then(|up| up.file_name())
            .and_then(|name| name.to_str())
            == Some(SPECS_DIR);
        let specs_root = if parent_is_specs {
            parent.to_path_buf()
        } else if grandparent_is_specs {
            parent.parent().unwrap_or(parent).to_path_buf()
        } else {
            parent.to_path_buf()
        };
        Self {
            specs_root,
            cache: BTreeMap::new(),
        }
    }

    /// The resolved `kitty-specs` root that owns this feature dir's siblings.
    pub fn specs_root(&self) -> &Path {
        &self.specs_root
    }

    /// How many slugs this call has answered (the reference's cache size).
    pub fn cached_slugs(&self) -> usize {
        self.cache.len()
    }

    /// Resolve the row's canonical `mission_id` exactly as `_resolve_mission_id_from_dict`.
    ///
    /// `raw["mission_id"]` (when truthy) wins as `str(mission_id)`; otherwise the
    /// row's own `mission_slug`/`feature_slug` is resolved. The function is
    /// fallible exactly where the reference raises *out* of the resolver (an
    /// unhashable slug, a truthy non-string slug's missing `.strip`, an escaped
    /// `OSError`/`RecursionError` in a metadata phase); every filesystem/decode
    /// problem the reference itself catches yields `Ok(None)`.
    pub fn resolve_for_row(
        &mut self,
        row: &PythonJsonDict,
    ) -> Result<Option<PythonJsonValue>, SourceResolverFailure> {
        if let Some(mission_id) = row
            .get("mission_id")
            .filter(|value| python_json_truthy(value))
        {
            return Ok(Some(PythonJsonValue::String(python_json_str(mission_id))));
        }
        let Some(slug) = row_mission_slug(row) else {
            return Ok(None);
        };
        // `mission_slug in self._cache` runs FIRST: an unhashable slug is a
        // `TypeError` before any path work, and the store's catch tuple catches
        // it (the resolver is called inside the admission `try`).
        python_json_hashable(slug).map_err(|error| {
            raised(
                SourcePythonClass::TypeError,
                SourceExecutionPhase::MissionIdCache,
                error.message(),
            )
        })?;
        match slug {
            PythonJsonValue::String(text) => self.resolve_typed(text),
            other => {
                // `assert_safe_path_segment(value)`: `value.strip() if value else
                // value`. A falsy non-string skips the attribute access and lands
                // in the empty-segment `ValueError` arm (caught -> None); a truthy
                // one raises `AttributeError`, which the store's catch tuple
                // catches (it is inside the admission `try`).
                if python_json_truthy(other) {
                    Err(raised(
                        SourcePythonClass::AttributeError,
                        SourceExecutionPhase::MissionIdGrammar,
                        format!(
                            "'{}' object has no attribute 'strip'",
                            python_json_type_name(other)
                        ),
                    ))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// The absorbing single-slug view (cache-timing and inspection only).
    ///
    /// Every escaped phase becomes `None`. The **store** path uses
    /// [`Self::resolve_for_row`], which preserves the escape; this view exists
    /// for callers that ask only "did the cache answer this slug", and it must
    /// never be used to state a row's `mission_id`.
    pub fn resolve(&mut self, slug: &PythonString) -> Option<PythonJsonValue> {
        self.resolve_typed(slug).ok().flatten()
    }

    /// Resolve one slug, with the reference's cache timing, preserving escapes.
    fn resolve_typed(
        &mut self,
        slug: &PythonString,
    ) -> Result<Option<PythonJsonValue>, SourceResolverFailure> {
        if let Some(cached) = self.cache.get(slug) {
            return Ok(cached.clone());
        }
        let resolved = self.read_meta_mission_id(slug)?;
        self.cache.insert(slug.clone(), resolved.clone());
        Ok(resolved)
    }

    /// The `meta.json` execution phase: grammar, containment, exists, read, decode, field.
    fn read_meta_mission_id(
        &self,
        slug: &PythonString,
    ) -> Result<Option<PythonJsonValue>, SourceResolverFailure> {
        // `assert_safe_path_segment`: any non-ASCII slug (a lone surrogate
        // included) is refused by the segment grammar, which is exactly what the
        // canonical ASCII-only predicate reports. The reference's `strip()` never
        // fails here because the slug is a `str`.
        let Ok(text) = slug.to_scalar_string() else {
            return Ok(None);
        };
        if !crate::replay::is_safe_path_segment(&text) {
            return Ok(None);
        }
        let mission_dir = self.specs_root.join(&text);
        let meta_path = mission_dir.join(META_FILENAME);
        // `_is_contained`: `ensure_within_any(meta_path, roots=[root])`.
        //
        // Phase order is the reference's: the candidate is resolved through
        // `resolve_rejecting_loops`, then the trusted root. `pathlib`'s
        // non-strict resolution *ignores every* `OSError` from its component
        // `lstat` and returns the lexical path (`posixpath._joinrealpath` with
        // `strict=False`), so an unreadable entry must not be a refusal here: it
        // keeps the lexical composition and the reference reaches its real phase
        // (`exists()`, then `read`). A symlink loop is the one case that becomes a
        // `ValueError` in `ensure_within_any` (caught -> `None`); a loop in the
        // *root* is resolved by a plain `Path.resolve` outside that wrapper, so it
        // escapes.
        let policy = sk_kernel::paths::ResolveIoPolicy::IgnoreMetadataErrors;
        let resolved =
            match sk_kernel::paths::resolve_path_non_strict_with_io_policy(&meta_path, policy) {
                Ok(resolved) => resolved,
                // ensure_within_any catches candidate resolution IO/loop failures.
                // Metadata failures are tolerated inside the ONE walker instead.
                Err(_) => return Ok(None),
            };
        let root = match sk_kernel::paths::resolve_path_non_strict_with_io_policy(
            &self.specs_root,
            policy,
        ) {
            Ok(root) => root,
            Err(sk_kernel::KernelError::SymlinkLoop { .. }) => {
                // `pathlib`'s `check_eloop` wraps a loop in `RuntimeError`, and
                // this resolution sits outside `ensure_within_any`'s
                // `OSError` -> `ValueError` wrapper, so it escapes.
                return Err(raised(
                    SourcePythonClass::RuntimeError,
                    SourceExecutionPhase::MissionIdContainment,
                    "symlink loop in the mission-specs root".to_owned(),
                ));
            }
            // Root resolution is outside ensure_within_any's candidate wrapper.
            // Metadata failures already continued inside the walker; a remaining
            // IO failure (e.g. readlink race) escapes here, not as a fake root.
            Err(error) => {
                return Err(raised(
                    SourcePythonClass::OSError,
                    SourceExecutionPhase::MissionIdContainment,
                    error.to_string(),
                ));
            }
        };
        if !resolved.starts_with(&root) {
            return Ok(None);
        }
        // `meta_path.exists()`: a separate probe that follows symlinks. Its
        // `_ignore_error` errnos are `False` (absence); any other `OSError`
        // escapes and must not be reported as absence.
        match sk_kernel::paths::source_path_exists(&meta_path) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(error) => {
                return Err(raised(
                    SourcePythonClass::OSError,
                    SourceExecutionPhase::MissionIdExists,
                    error.to_string(),
                ));
            }
        }
        // `meta_path.read_text(encoding="utf-8")`: unbounded, no BOM tolerance,
        // strict decoding. A read failure or a non-UTF-8 file becomes
        // `MissionMetaReadError` in the reference and is caught -> `None`.
        let Ok(bytes) = sk_kernel::read_source_bytes(&meta_path) else {
            return Ok(None);
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Ok(None);
        };
        let decoded = match sk_kernel::python_json::parse(text) {
            Ok(decoded) => decoded,
            Err(error) => match error.kind() {
                // The digit ceiling is a `ValueError` in `json.loads`, which the
                // reference's `load_meta_fail_closed` catches -> `None`.
                PythonJsonErrorKind::IntegerDigitLimit => return Ok(None),
                // A `RecursionError` escapes the reference's catch tuples; the J1
                // reader's own operational limits are not that class at all.
                PythonJsonErrorKind::RecursionBudget => {
                    return Err(SourceResolverFailure::Operational(SourceOperational::new(
                        SourceExecutionPhase::MissionIdDecode,
                        error.message(),
                    )));
                }
                PythonJsonErrorKind::IntegerDigitLimitSetting
                | PythonJsonErrorKind::NumberConversion => {
                    return Err(SourceResolverFailure::Operational(SourceOperational::new(
                        SourceExecutionPhase::MissionIdDecode,
                        error.to_string(),
                    )));
                }
                // `JSONDecodeError` (and the BOM refusal) -> `MissionMetaReadError`
                // -> caught -> `None`.
                _ => return Ok(None),
            },
        };
        let Some(object) = normalize_object(&decoded) else {
            // A non-object top level is `MetaDecodeError` -> caught -> `None`.
            return Ok(None);
        };
        Ok(object
            .get("mission_id")
            .filter(|value| python_json_truthy(value))
            .cloned())
    }
}
