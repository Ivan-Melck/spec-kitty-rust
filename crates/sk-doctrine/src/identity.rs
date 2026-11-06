//! Layer-aware artifact identity resolution.
//!
//! Mirrors `src/charter/activation/identity_index.py`: an activation token may
//! spell an artifact as its file stem (`001-arch`), its declared id
//! (`ARCH_001`) or a `<kind>:<id>` URN. Lookups are stem-first across **all**
//! layers (project, then org packs last-to-first, then built-in), then declared
//! id, then — for directives — the canonical id normaliser.
//!
//! **Identity is exact.** The reference keys its index on Python `str`: a
//! declared id such as `"author\ud800"` is one code-point sequence and matches
//! only the identical sequence. A Rust `String` cannot hold that id, so the
//! index keys declared ids on [`sk_kernel::PythonString`] and reads each
//! artifact's `id:`/`profile-id:` through the same lossless transport and
//! construction phase the profile loader uses ([`crate::profile_text`] +
//! [`crate::profile_construct`]) — never through a second YAML reader or a lossy
//! projection. Every exact lookup has an exact accessor (`declared_id_exact`,
//! `stem_exact`, `stored_spelling_exact`, `knows_exact`, …); the `&str`
//! accessors remain as the explicit **scalar capability view** for callers whose
//! token is Rust text (which is always a sequence of scalars), and they return
//! `None` rather than a substituted id when the indexed declaration has no
//! scalar projection.
//!
//! Precedence, not ambiguity, decides a stem two layers share: the reference
//! index takes the first match in layer order and never fails a valid project
//! activation over a shadowed built-in file. The only hard failure is a
//! *declared id* that cannot be represented by any activation stem (two layers
//! bind the same file name to different ids), which is a pack-content defect
//! reported by [`IdentityIndex::stored_spelling`].

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sk_charter::ArtifactKind;
use sk_kernel::PythonString;
use sk_kernel::python_int::IntDecimalError;
use sk_kernel::python_utf8::{PythonUtf8DecodeError, decode_utf8};
use sk_kernel::text_decode::normalize_newlines;

use crate::error::DoctrineError;
use crate::offering::{ConstructorAbort, GateFailure, NativeOfferingFailure, SuccessOnce};
use crate::org::{normalize_directive_id, normalize_directive_id_exact};

/// The identity read's failure: the SAME two-arm runtime authority the activation
/// gate already uses.
///
/// Source's `_parse_id` can raise a real Python exception the loader does not
/// catch (a `ValueError` from the eager `str()` of a large integer, for example),
/// while an invalid `PYTHONINTMAXSTRDIGITS` kills CPython before any business code
/// runs and has no Source exception class. One existing authority carries both:
/// [`GateFailure::Constructor`] is the real Source abort and
/// [`GateFailure::Native`] is the typed start-up/capability refusal. This is a
/// same-item alias, NOT a second `IdentityFailure` enum.
pub type IdentityFailure = GateFailure;

/// The number of artifact kinds: the fixed width of the per-kind table array.
const KIND_COUNT: usize = ArtifactKind::ALL.len();

/// The array position of `kind` (the canonical enum order).
fn kind_slot(kind: ArtifactKind) -> usize {
    ArtifactKind::ALL
        .iter()
        .position(|candidate| *candidate == kind)
        .unwrap_or(0)
}

/// The empty table the total (infallible) lookups read for an aborted kind.
fn empty_table() -> &'static KindTable {
    static EMPTY: OnceLock<KindTable> = OnceLock::new();
    EMPTY.get_or_init(KindTable::default)
}

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
    /// The id the file declares, when it declares a non-empty one.
    declared: Option<PythonString>,
    /// The identity key: the declared id, or the file stem when none is declared.
    declared_id: PythonString,
    layer: usize,
    path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct KindTable {
    by_stem: BTreeMap<String, Entry>,
    by_id: BTreeMap<PythonString, Entry>,
    ids_by_stem: BTreeMap<String, BTreeSet<PythonString>>,
    entries: Vec<Entry>,
}

/// One kind's published table.
///
/// Only a **completed** scan is published: the reference assigns
/// `self._tables[kind] = _KindTable.from_entries(self._scan(kind))`, so the
/// assignment happens after `_scan` returned. A scan that raises therefore
/// leaves the slot empty and the next query retries it, instead of memoising the
/// failure for the instance's whole life.
type KindSlot = SuccessOnce<KindTable>;

/// The reference `ResolutionPass`'s per-file memo key: the artifact file and the
/// YAML field that carries its declared id.
type IdKey = (PathBuf, String);

/// One artifact file's completed identity read.
///
/// This is the reference's per-file memo value (`ResolutionPass.ids`): the
/// declared id the reader produced, or `None` when the read raised one of the
/// exceptions the reference catches — a *completed* answer that contributes no
/// entry and is never re-read in the same pass.
#[derive(Clone, Debug, Eq, PartialEq)]
enum FileRead {
    Declared(Option<PythonString>),
    Skipped,
}

/// The partial caches one identity index retains across a failed scan.
///
/// The reference keeps these in its `ResolutionPass` (`scans`, `ids`) and its
/// private `_fast_ids`: a scan that raises leaves the completed layer scan and
/// every completed per-file read in place, and does **not** memoise the file that
/// raised. A later query on the same index therefore retries only the unread
/// part: the memoised file list is the pass's directory-snapshot boundary, and
/// the completed ids keep their values and iteration order. A new index is a new
/// pass and re-reads the tree.
#[derive(Clone, Debug, Default)]
struct ScanPass {
    /// `_iter_artifact_paths`: the layer file list one kind reads for this index's roots.
    files: BTreeMap<ArtifactKind, Vec<(usize, PathBuf)>>,
    /// `ResolutionPass.ids`: one completed read per (file, id field).
    ids: BTreeMap<IdKey, FileRead>,
}

/// How deep one layer directory is scanned (the reference's `recursive` flag).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScanDepth {
    /// `directory.glob(pattern)` — one level, no subdirectories.
    Flat,
    /// `directory.rglob(pattern)`.
    Recursive,
    /// `directory.rglob(pattern)` minus the reserved **immediate** `built-in/`
    /// child.
    ///
    /// The reference scans that child as its own later entry, so the flat
    /// layout keeps precedence over the legacy package-shaped one
    /// (`kind_vocabulary._under_reserved_builtin_subdir`, FR-001 /
    /// PR-BOUNDARY-002). Excluding it here is what makes the *ordering* of
    /// [`IdentityIndex::scan_layers`] mean what it says.
    RecursiveExcludingLegacy,
}

// The reference's conservative fast plain-ID probe (`identity_index._fast_plain_id`).
//
// This is a business recognizer for ONE top-level `<id field>:` line, not a second
// YAML parser: every shape it cannot prove falls back to the canonical construction,
// and a file with an unrelated malformed body keeps the reference's accepted
// divergence (the artifact repository still rejects that body).

/// One line of `text` starting at `start`, excluding its terminating `\n`, and the
/// offset of that `\n` (or the end of the text, like Python `re` `$`).
fn source_line(text: &str, start: usize) -> (&str, usize) {
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset);
    (&text[start..end], end)
}

/// The start offset of every line, including the position after a final `\n`.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

/// Drop at most one leading `"` or `'` (Python's independent optional quote).
fn strip_one_quote(text: &str) -> &str {
    text.strip_prefix('"')
        .or_else(|| text.strip_prefix('\''))
        .unwrap_or(text)
}

/// Python `^["']?<key>["']?[ \t]*:` at a line start.
fn key_line_prefix(line: &str, id_field: &str) -> bool {
    let Some(rest) = strip_one_quote(line).strip_prefix(id_field) else {
        return false;
    };
    strip_one_quote(rest)
        .trim_start_matches([' ', '\t'])
        .starts_with(':')
}

/// One `_BLANK_OR_COMMENT` line: spaces/tabs, then nothing or a `#` comment.
fn is_blank_or_comment_line(line: &str) -> bool {
    let rest = line.trim_start_matches([' ', '\t']);
    rest.is_empty() || rest.starts_with('#')
}

/// Python `_BLANK_OR_COMMENT.fullmatch(prefix)`.
fn blank_or_comment_prefix(prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    let Some(body) = prefix.strip_suffix('\n') else {
        return false;
    };
    body.split('\n').all(is_blank_or_comment_line)
}

/// The `^(---|...)(space/tab/end-of-line)` lines, in order; `true` marks `---`.
fn document_markers(text: &str) -> Vec<(usize, bool)> {
    let mut markers = Vec::new();
    for start in line_starts(text) {
        let (line, _) = source_line(text, start);
        let dashes = line.starts_with("---");
        if !dashes && !line.starts_with("...") {
            continue;
        }
        if matches!(line.as_bytes().get(3), None | Some(&b' ') | Some(&b'\t')) {
            markers.push((start, dashes));
        }
    }
    markers
}

/// Whether the text has a `...` line or a `---` other than a leading one.
fn has_foreign_document_marker(text: &str) -> bool {
    let markers = document_markers(text);
    let Some(&(first, dashes)) = markers.first() else {
        return false;
    };
    let leading = dashes && blank_or_comment_prefix(&text[..first]);
    markers.len() > 1 || !leading
}

/// Whether the line after the one ending at `line_end` is indented (continuation).
fn next_line_is_continuation(text: &str, line_end: usize) -> bool {
    text.get(line_end + 1..)
        .is_some_and(|following| following.starts_with(' ') || following.starts_with('\t'))
}

/// The Source `^<key>:[ \t]+(\S+)[ \t]*$` match: value, line end and whether the
/// matched line contains a tab (which always falls back to the full parse).
fn find_key_value_line<'a>(text: &'a str, id_field: &str) -> Option<(&'a str, usize, bool)> {
    for start in line_starts(text) {
        let (line, line_end) = source_line(text, start);
        let Some(rest) = line.strip_prefix(id_field) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let value_and_trailing = rest.trim_start_matches([' ', '\t']);
        let separator = &rest[..rest.len() - value_and_trailing.len()];
        if separator.is_empty() {
            continue;
        }
        let value_len = value_and_trailing
            .char_indices()
            .find(|(_, point)| is_python_space(*point as u32))
            .map_or(value_and_trailing.len(), |(index, _)| index);
        if value_len == 0 {
            continue;
        }
        let value = &value_and_trailing[..value_len];
        let trailing = &value_and_trailing[value_len..];
        if !trailing.chars().all(|point| point == ' ' || point == '\t') {
            continue;
        }
        let has_tab = line.contains('\t');
        return Some((value, line_end, has_tab));
    }
    None
}

/// `[A-Za-z][A-Za-z0-9_.-]*` — the conservative ASCII plain-string value shape.
fn is_plain_id_value(value: &str) -> bool {
    let mut points = value.chars();
    let Some(first) = points.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && points.all(|point| point.is_ascii_alphanumeric() || matches!(point, '.' | '_' | '-'))
}

/// `_YAML_SPECIAL_SCALARS`, compared case-insensitively like the reference.
fn is_yaml_special_scalar(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "y" | "n" | "~" | ".nan" | ".inf"
    )
}

/// The reference `_fast_plain_id`: the id when the text declares it once as a
/// conservative unquoted plain string, else `None` (use the canonical parse).
fn fast_plain_id(text: &str, id_field: &str) -> Option<PythonString> {
    if text.contains('\u{feff}') || text.contains('\r') || has_foreign_document_marker(text) {
        return None;
    }
    let key_count = line_starts(text)
        .into_iter()
        .filter(|start| {
            let (line, _) = source_line(text, *start);
            key_line_prefix(line, id_field)
        })
        .count();
    if key_count != 1 {
        return None;
    }
    let (value, line_end, has_tab) = find_key_value_line(text, id_field)?;
    if has_tab || next_line_is_continuation(text, line_end) {
        return None;
    }
    if !is_plain_id_value(value) || is_yaml_special_scalar(value) {
        return None;
    }
    Some(PythonString::from_scalars(value))
}

/// The reference `_read_artifact_text` failure phases.
enum FastReadError {
    /// `OSError`: caught by the reference, which retries the canonical read and
    /// completes the file as `None` when the retry also fails.
    Unreadable,
    /// `UnicodeDecodeError`: the reference's reader does not catch it, so it
    /// escapes the scan and is never memoised.
    Decode(PythonUtf8DecodeError),
}

fn decode_abort(error: PythonUtf8DecodeError) -> IdentityFailure {
    GateFailure::Constructor(ConstructorAbort {
        python: "UnicodeDecodeError",
        message: error.to_string(),
    })
}

/// The reference `_read_artifact_text`: ordinary `Path.read_text(encoding="utf-8")`.
///
/// Strict UTF-8 (no BOM sniffing, no replacement) plus TextIO universal-newline
/// translation; the decode failure is the pinned CPython `UnicodeDecodeError`
/// (class, range and message), kept distinct from the read `OSError` phase.
fn read_artifact_text(path: &Path) -> Result<String, FastReadError> {
    let bytes = std::fs::read(path).map_err(|_| FastReadError::Unreadable)?;
    let text = decode_utf8(&bytes).map_err(FastReadError::Decode)?;
    Ok(normalize_newlines(text))
}

/// The canonical fallback over already-read text (one construction pipeline).
///
/// The eager declared-id render runs only after the document constructed, with
/// the interpreter's configured decimal ceiling. A `TypeError` raised while the
/// YAML load constructs a value is one of the exceptions Source's `_parse_id`
/// catches, so it is the reference's completed no-id read (no stem fallback), not
/// a blanket-fatal abort.
fn canonical_read(
    path: &Path,
    text: &str,
    kind: ArtifactKind,
) -> Result<FileRead, IdentityFailure> {
    match ArtifactDocument::from_text(text) {
        Ok(document) => {
            let digit_limit = integer_digit_limit(path)?;
            Ok(FileRead::Declared(declared_id(
                &document,
                kind,
                digit_limit,
            )?))
        }
        Err(error) => match error.fatal() {
            Some(("TypeError", _)) => Ok(FileRead::Skipped),
            Some((python, message)) => Err(GateFailure::Constructor(ConstructorAbort {
                python,
                message: message.to_owned(),
            })),
            None => Ok(FileRead::Skipped),
        },
    }
}

/// The interpreter's decimal integer-conversion ceiling for this scan.
///
/// CPython refuses to start for an invalid `PYTHONINTMAXSTRDIGITS`, so there is no
/// Source business exception here: the canonical Kernel policy error is carried as
/// a typed Native start-up failure, never a fake Source class or an unlimited
/// fallback.
fn integer_digit_limit(path: &Path) -> Result<usize, IdentityFailure> {
    sk_kernel::python_json::environment_integer_digits().map_err(|cause| {
        GateFailure::Native(NativeOfferingFailure::IntegerTextPolicy {
            phase: "identity declared-id read",
            path: path.to_path_buf(),
            cause: Box::new(cause),
        })
    })
}

/// The Source `ValueError` for an eager `str(int)` above the ceiling.
fn decimal_conversion_abort(error: IntDecimalError) -> IdentityFailure {
    match error {
        IntDecimalError::DigitLimit { .. } => GateFailure::Constructor(ConstructorAbort {
            python: "ValueError",
            message: error.to_string(),
        }),
    }
}

/// A per-invocation identity index over the built-in, org and project layers.
///
/// **Construction is lazy and infallible.** The reference's `IdentityIndex`
/// scans one `(kind, layer set)` at most once, when a lookup first asks for it
/// (NFR-003); a kind nothing queries is never read. So a pack file whose
/// construction aborts the reference's loader cannot abort the construction of
/// an index that never looks at that kind. A completed scan publishes its table;
/// a scan that raises publishes nothing, keeps the reference `ResolutionPass`'s
/// partial caches ([`ScanPass`]) and is retried by the next query — so repairing
/// the failing file and asking again on the same index recovers, exactly as it
/// does in the reference.
///
/// The infallible lookups ([`IdentityIndex::declared_id`] and friends) are the
/// crate's **total capability view**: an aborted kind contributes no entries to
/// them. A caller that must reproduce the reference's failure uses the `try_*`
/// lookups (or [`IdentityIndex::try_scan`]), which fail at the exact query phase
/// that needed the kind.
#[derive(Clone, Debug)]
pub struct IdentityIndex {
    tables: [KindSlot; KIND_COUNT],
    /// The reference `ResolutionPass` partial caches, retained across a failed scan.
    pass: RefCell<ScanPass>,
    /// The reference's PRIVATE fast-id cache (`IdentityIndex._fast_ids`).
    ///
    /// Only the conservative plain-`id:` probe writes here, and it is consulted
    /// after the shared pass, so a value the canonical construction did not
    /// produce never reaches another consumer of `pass.ids`. A new index starts
    /// empty, exactly like the reference's per-instance `_fast_ids`.
    fast_ids: RefCell<BTreeMap<IdKey, PythonString>>,
    pub project_root: Option<PathBuf>,
    pub org_roots: Vec<PathBuf>,
    pub built_in_root: PathBuf,
}

impl Default for IdentityIndex {
    fn default() -> Self {
        Self {
            tables: std::array::from_fn(|_| SuccessOnce::new()),
            pass: RefCell::new(ScanPass::default()),
            fast_ids: RefCell::new(BTreeMap::new()),
            project_root: None,
            org_roots: Vec::new(),
            built_in_root: PathBuf::new(),
        }
    }
}

impl IdentityIndex {
    /// Index exactly these roots. **No directory is read here.**
    ///
    /// `org_roots` are in declaration order; `project_root` is the project pack
    /// root (`.kittify/charter-packs`), when present.
    pub fn from_roots(
        built_in_root: &Path,
        org_roots: &[PathBuf],
        project_root: Option<&Path>,
    ) -> Self {
        Self {
            tables: std::array::from_fn(|_| SuccessOnce::new()),
            pass: RefCell::new(ScanPass::default()),
            fast_ids: RefCell::new(BTreeMap::new()),
            project_root: project_root.map(Path::to_path_buf),
            org_roots: org_roots.to_vec(),
            built_in_root: built_in_root.to_path_buf(),
        }
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

    /// Distinct activation stems in layer/scan insertion order.
    ///
    /// Reads the existing index only: a shadowed stem occurs once, at its
    /// highest-precedence occurrence. This enumerates identity, not an
    /// effective-set verdict (which also depends on service and pack policy).
    pub fn stems(&self, kind: ArtifactKind) -> Vec<String> {
        let table = self.table(kind);
        let mut seen = BTreeSet::new();
        table
            .entries
            .iter()
            .filter(|entry| seen.insert(&entry.stem))
            .map(|entry| entry.stem.clone())
            .collect()
    }

    /// Distinct stems of the files this **identity** scan indexed with a declared id.
    ///
    /// This follows the identity layout, which for mission step contracts also
    /// indexes the org and project overlays. It is therefore *not* the
    /// reference's `list_available` answer (that layout is
    /// `crate::offering::availability_stems`); use it only to reason about the
    /// index itself.
    pub fn declared_id_stems(&self, kind: ArtifactKind) -> Vec<String> {
        let table = self.table(kind);
        let mut seen = BTreeSet::new();
        table
            .entries
            .iter()
            .filter(|entry| entry.declared.is_some() && seen.insert(&entry.stem))
            .map(|entry| entry.stem.clone())
            .collect()
    }

    /// The memoised table of `kind`, scanning it on first use.
    ///
    /// An aborted kind contributes nothing here (the total capability view); a
    /// caller that needs the abort asks [`IdentityIndex::try_table`].
    fn table(&self, kind: ArtifactKind) -> &KindTable {
        match self.try_table(kind) {
            Ok(table) => table,
            Err(_) => empty_table(),
        }
    }

    /// The memoised table of `kind`, or the abort the reference's scan raises.
    ///
    /// This is the exact query-phase seam: nothing is read until a caller asks
    /// for the kind, and the failure is the kind's own, never an unused one's.
    ///
    /// # Errors
    ///
    /// The Python exception the reference's artifact loader does not catch
    /// (see [`crate::offering::value`]) raised while scanning one file of
    /// `kind`.
    pub(crate) fn try_table(&self, kind: ArtifactKind) -> Result<&KindTable, IdentityFailure> {
        // Publish only a completed table. A scan that raises keeps the pass's
        // completed file list and per-file reads and leaves the slot empty, so the
        // next query retries the unread remainder instead of a memoised failure.
        self.tables[kind_slot(kind)].get_or_try_init(|| self.build_table(kind))
    }

    /// Scan `kind` now and report whether the reference's scan would abort.
    ///
    /// A caller that wants only the phase — a diagnostic, or a preflight before
    /// a write — asks this instead of one of the `try_*` lookups.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_scan(&self, kind: ArtifactKind) -> Result<(), IdentityFailure> {
        self.try_table(kind).map(|_| ())
    }

    fn build_table(&self, kind: ArtifactKind) -> Result<KindTable, IdentityFailure> {
        let mut table = KindTable::default();
        for entry in self.scan(kind)? {
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
        Ok(table)
    }

    /// Scan one kind, reading each layer in the reference's precedence order.
    ///
    /// A file the reference *skips* (unreadable, YAML syntax error, refused
    /// tag) contributes no entry; a file whose construction raises the Python
    /// exception the reference does not catch aborts the scan, which is the
    /// abort this returns rather than a silently smaller table.
    fn scan(&self, kind: ArtifactKind) -> Result<Vec<Entry>, IdentityFailure> {
        let mut entries = Vec::new();
        if kind.glob_pattern().is_empty() {
            return Ok(entries);
        }
        for (layer, path) in self.scan_files(kind) {
            let stem = config_stem(&path);
            if stem.is_empty() {
                continue;
            }
            let declared = match self.file_read(kind, &path)? {
                FileRead::Skipped => continue,
                FileRead::Declared(declared) => declared,
            };
            let identity = declared
                .clone()
                .unwrap_or_else(|| PythonString::from_scalars(stem.trim()));
            if identity.is_empty() {
                continue;
            }
            entries.push(Entry {
                kind,
                stem,
                declared,
                declared_id: identity,
                layer,
                path,
            });
        }
        Ok(entries)
    }

    /// This pass's layer file list for `kind` (the reference `_iter_artifact_paths`).
    ///
    /// The list is memoised for the pass, so a file born after a failed scan stays
    /// outside the retried scan until a new index starts a new pass — the
    /// reference's directory-snapshot boundary, not a fresh walk per query.
    fn scan_files(&self, kind: ArtifactKind) -> Vec<(usize, PathBuf)> {
        if let Some(files) = self.pass.borrow().files.get(&kind) {
            return files.clone();
        }
        let suffix = kind.glob_pattern().trim_start_matches('*').to_owned();
        let mut files = Vec::new();
        for (layer, directory, depth) in self.scan_layers(kind) {
            for path in artifact_files_in(&directory, &suffix, depth) {
                files.push((layer, path));
            }
        }
        self.pass.borrow_mut().files.insert(kind, files.clone());
        files
    }

    /// One file's completed identity read.
    ///
    /// The reference's lookup order is: a completed canonical read first
    /// (`ResolutionPass.ids`, including a caught `None`), then its private
    /// `_fast_ids` cache, then one ordinary read whose text feeds the conservative
    /// plain-id probe and, when that misses, the canonical construction over the
    /// SAME text (or a true re-read after a read `OSError`). A fast hit is stored
    /// only in the private cache; only the canonical fallback seeds the shared pass,
    /// and an uncaught read/construction abort is never stored.
    fn file_read(&self, kind: ArtifactKind, path: &Path) -> Result<FileRead, IdentityFailure> {
        let field = crate::offering::id_field(kind);
        let key = (path.to_path_buf(), field.to_owned());
        if let Some(read) = self.pass.borrow().ids.get(&key).cloned() {
            return Ok(read);
        }
        if let Some(fast) = self.fast_ids.borrow().get(&key).cloned() {
            return Ok(FileRead::Declared(Some(fast)));
        }
        let text = match read_artifact_text(path) {
            Ok(text) => Some(text),
            Err(FastReadError::Unreadable) => None,
            Err(FastReadError::Decode(error)) => return Err(decode_abort(error)),
        };
        if let Some(text) = text.as_deref()
            && let Some(fast) = fast_plain_id(text, field)
        {
            self.fast_ids.borrow_mut().insert(key.clone(), fast.clone());
            return Ok(FileRead::Declared(Some(fast)));
        }
        let read = match text {
            Some(text) => canonical_read(path, &text, kind)?,
            None => match read_artifact_text(path) {
                Ok(text) => canonical_read(path, &text, kind)?,
                Err(FastReadError::Unreadable) => FileRead::Skipped,
                Err(FastReadError::Decode(error)) => return Err(decode_abort(error)),
            },
        };
        self.pass.borrow_mut().ids.insert(key, read.clone());
        Ok(read)
    }

    /// The reference's `_scan_roots` order for `kind`.
    ///
    /// Project first, then **every** org pack's flat `<plural>` directory (last
    /// declaration first), then **every** org pack's legacy `<plural>/built-in`
    /// directory (same order), then the built-in tier. The two org groups are
    /// not interleaved per pack: that global grouping is what keeps a flat org
    /// artifact ahead of a legacy one from an earlier pack
    /// (PR-BOUNDARY-002).
    fn scan_layers(&self, kind: ArtifactKind) -> Vec<(usize, PathBuf, ScanDepth)> {
        let mut layers: Vec<(usize, PathBuf, ScanDepth)> = Vec::new();
        if let Some(project) = &self.project_root {
            layers.push((0, project.join(kind.project_dir()), ScanDepth::Recursive));
        }
        for (position, root) in self.org_roots.iter().rev().enumerate() {
            layers.push((
                1 + position,
                root.join(kind.plural()),
                ScanDepth::RecursiveExcludingLegacy,
            ));
        }
        for (position, root) in self.org_roots.iter().rev().enumerate() {
            layers.push((
                1 + position,
                root.join(kind.plural()).join("built-in"),
                ScanDepth::Recursive,
            ));
        }
        let built_in_layer = 1 + self.org_roots.len();
        layers.push((
            built_in_layer,
            self.built_in_root.join(kind.plural()),
            ScanDepth::Recursive,
        ));
        if kind == ArtifactKind::MissionStepContract {
            layers.push((
                built_in_layer,
                self.built_in_root.join("missions/built_in_step_contracts"),
                ScanDepth::Flat,
            ));
        }
        layers
    }

    /// `token` without its `<kind>:` URN prefix, exactly.
    fn bare_exact(kind: ArtifactKind, token: &PythonString) -> PythonString {
        let mut prefix = PythonString::from_scalars(kind.canonical());
        prefix.push_scalars(":");
        let codepoints = token.codepoints();
        if codepoints.starts_with(prefix.codepoints()) {
            return PythonString::from_codepoints(&codepoints[prefix.codepoints().len()..])
                .expect("a slice of valid code points is valid");
        }
        token.clone()
    }

    /// `token` without its `<kind>:` URN prefix, when it has a scalar spelling.
    fn bare(kind: ArtifactKind, token: &str) -> &str {
        token
            .strip_prefix(kind.canonical())
            .and_then(|rest| rest.strip_prefix(':'))
            .unwrap_or(token)
    }

    fn lookup_existing_exact(&self, kind: ArtifactKind, token: &PythonString) -> Option<&Entry> {
        let bare = Self::bare_exact(kind, token);
        let table = self.table(kind);
        if let Ok(stem) = bare.to_scalar_string()
            && let Some(entry) = table.by_stem.get(&stem)
        {
            return Some(entry);
        }
        if let Some(entry) = table.by_id.get(&bare) {
            return Some(entry);
        }
        if kind == ArtifactKind::Directive {
            let normalised = normalize_directive_id_exact(&bare);
            if normalised != bare {
                return table.by_id.get(&normalised);
            }
        }
        None
    }

    fn lookup_existing(&self, kind: ArtifactKind, token: &str) -> Option<&Entry> {
        self.lookup_existing_exact(kind, &PythonString::from_scalars(token))
    }

    /// Declared id for a token (stem first, then declared id, then the
    /// directive normaliser), exactly.
    ///
    /// A shared stem resolves to the highest-precedence layer's declaration,
    /// exactly as the reference index does; it never fails.
    pub fn declared_id_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Option<PythonString> {
        let bare = Self::bare_exact(kind, token);
        let table = self.table(kind);
        if let Ok(stem) = bare.to_scalar_string()
            && let Some(entry) = table.by_stem.get(&stem)
        {
            return Some(entry.declared_id.clone());
        }
        if let Some(entry) = table.by_id.get(&bare) {
            return Some(entry.declared_id.clone());
        }
        if kind == ArtifactKind::Directive {
            let normalised = normalize_directive_id_exact(&bare);
            if normalised != bare {
                return table
                    .by_id
                    .get(&normalised)
                    .map(|entry| entry.declared_id.clone());
            }
        }
        None
    }

    /// Declared id for a token — the explicit **scalar capability view**.
    ///
    /// `None` when the token is unknown *or* when the indexed declaration has
    /// no scalar projection (a lone surrogate in the declared id). Use
    /// [`IdentityIndex::declared_id_exact`] for the exact answer.
    pub fn declared_id_checked(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Option<String>, DoctrineError> {
        Ok(self.declared_id(kind, token))
    }

    /// Declared id for a scalar token.
    pub fn declared_id(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        self.declared_id_exact(kind, &PythonString::from_scalars(token))
            .and_then(|declared| declared.to_scalar_string().ok())
    }

    /// Declared id of the file named `stem` in exactly one layer (`project`,
    /// `org` or `built-in`), never a higher layer's shadowing declaration.
    pub fn declared_id_in_layer(
        &self,
        kind: ArtifactKind,
        stem: &str,
        layer: IdentityLayer,
    ) -> Option<String> {
        self.declared_id_in_layer_exact(kind, stem, layer)
            .and_then(|declared| declared.to_scalar_string().ok())
    }

    /// [`IdentityIndex::declared_id_in_layer`], exactly.
    pub fn declared_id_in_layer_exact(
        &self,
        kind: ArtifactKind,
        stem: &str,
        layer: IdentityLayer,
    ) -> Option<PythonString> {
        let table = self.table(kind);
        table
            .entries
            .iter()
            .find(|entry| entry.stem == stem && self.layer_name(entry.layer) == layer)
            .map(|entry| entry.declared_id.clone())
    }

    /// The layer a token resolves in, when known.
    pub fn source_layer(&self, kind: ArtifactKind, token: &str) -> Option<IdentityLayer> {
        self.lookup_existing(kind, token)
            .map(|entry| self.layer_name(entry.layer))
    }

    /// The declared id a *selection* resolves to; directives prefer a declared
    /// id over a coincidentally equal file name.
    pub fn selection_id(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        self.selection_id_exact(kind, &PythonString::from_scalars(token))
            .and_then(|declared| declared.to_scalar_string().ok())
    }

    /// [`IdentityIndex::selection_id`], exactly.
    pub fn selection_id_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Option<PythonString> {
        let bare = Self::bare_exact(kind, token);
        let table = self.table(kind);
        if kind == ArtifactKind::Directive
            && let Some(entry) = table.by_id.get(&bare)
        {
            return Some(entry.declared_id.clone());
        }
        self.declared_id_exact(kind, token)
    }

    /// The declaring file for a token, when the index knows it.
    pub fn path_for(&self, kind: ArtifactKind, token: &str) -> Option<&Path> {
        self.lookup_existing(kind, token)
            .map(|entry| entry.path.as_path())
    }

    /// The declaring file for an exact token, when the index knows it.
    pub fn path_for_exact(&self, kind: ArtifactKind, token: &PythonString) -> Option<&Path> {
        self.lookup_existing_exact(kind, token)
            .map(|entry| entry.path.as_path())
    }

    /// Whether a token names a known artifact.
    pub fn knows(&self, kind: ArtifactKind, token: &str) -> bool {
        self.lookup_existing(kind, token).is_some()
    }

    /// Whether an exact token names a known artifact.
    ///
    /// This is the total capability view: an aborted kind contributes no
    /// entries, so the answer is `false`. A caller that must reproduce the
    /// reference's abort asks [`IdentityIndex::try_knows_exact`].
    pub fn knows_exact(&self, kind: ArtifactKind, token: &PythonString) -> bool {
        self.try_knows_exact(kind, token).unwrap_or(false)
    }

    /// Whether any layer's file declares `token` as its id.
    pub fn has_declared_id(&self, kind: ArtifactKind, token: &str) -> bool {
        self.has_declared_id_exact(kind, &PythonString::from_scalars(token))
    }

    /// Whether any layer's file declares an exact token as its id.
    pub fn has_declared_id_exact(&self, kind: ArtifactKind, token: &PythonString) -> bool {
        let bare = Self::bare_exact(kind, token);
        let table = self.table(kind);
        if table.by_id.contains_key(&bare) {
            return true;
        }
        kind == ArtifactKind::Directive
            && table
                .by_id
                .contains_key(&normalize_directive_id_exact(&bare))
    }

    /// Whether `token` is the file stem of some indexed artifact.
    pub fn names_a_file(&self, kind: ArtifactKind, token: &str) -> bool {
        self.table(kind)
            .by_stem
            .contains_key(Self::bare(kind, token))
    }

    /// The activation stem for a token; `None` when unknown or unrepresentable.
    ///
    /// Directives are id-first (as selections are); other kinds are stem-first.
    pub fn stem(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        self.stem_exact(kind, &PythonString::from_scalars(token))
            .and_then(|stem| stem.to_scalar_string().ok())
    }

    /// [`IdentityIndex::stem`], exactly (a stem is always a file name).
    pub fn stem_exact(&self, kind: ArtifactKind, token: &PythonString) -> Option<PythonString> {
        let bare = Self::bare_exact(kind, token);
        let table = self.table(kind);
        if kind == ArtifactKind::Directive && table.by_id.contains_key(&bare) {
            return self.stem_for_declared_id_exact(kind, &bare);
        }
        if let Ok(stem) = bare.to_scalar_string()
            && table.by_stem.contains_key(&stem)
        {
            return Some(PythonString::from_scalars(&stem));
        }
        self.declared_id_exact(kind, &bare)
            .and_then(|declared| self.stem_for_declared_id_exact(kind, &declared))
    }

    /// The stem whose declared id is `declared_id`, or `None`.
    pub fn stem_for_declared_id(&self, kind: ArtifactKind, declared_id: &str) -> Option<String> {
        self.stem_for_declared_id_exact(kind, &PythonString::from_scalars(declared_id))
            .and_then(|stem| stem.to_scalar_string().ok())
    }

    /// [`IdentityIndex::stem_for_declared_id`], exactly.
    pub fn stem_for_declared_id_exact(
        &self,
        kind: ArtifactKind,
        declared_id: &PythonString,
    ) -> Option<PythonString> {
        let bare = Self::bare_exact(kind, declared_id);
        if kind != ArtifactKind::Directive {
            return self.representable_stem(kind, &bare);
        }
        if let Some(stem) = self.representable_stem(kind, &bare) {
            return Some(stem);
        }
        let normalised = normalize_directive_id_exact(&bare);
        if normalised == bare {
            None
        } else {
            self.representable_stem(kind, &normalised)
        }
    }

    fn representable_stem(
        &self,
        kind: ArtifactKind,
        declared: &PythonString,
    ) -> Option<PythonString> {
        let table = self.table(kind);
        for entry in &table.entries {
            if &entry.declared_id != declared {
                continue;
            }
            if kind != ArtifactKind::Directive {
                return Some(PythonString::from_scalars(&entry.stem));
            }
            let representative = table.by_stem.get(&entry.stem)?;
            let distinct = table.ids_by_stem.get(&entry.stem).map(BTreeSet::len);
            if &representative.declared_id == declared && distinct == Some(1) {
                return Some(PythonString::from_scalars(&entry.stem));
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
        self.stored_spelling_exact(kind, &PythonString::from_scalars(token))
            .map(|stem| stem.to_scalar_string().unwrap_or_default())
    }

    /// [`IdentityIndex::stored_spelling`], exactly.
    ///
    /// A stem is a file name, so the result always has a scalar projection; for
    /// an unknown token the exact token is returned unchanged (it may be
    /// non-scalar), which is the reference's "kept as written" behaviour.
    pub fn stored_spelling_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<PythonString, DoctrineError> {
        if let Some(stem) = self.stem_exact(kind, token) {
            return Ok(stem);
        }
        if self.has_declared_id_exact(kind, token) {
            return Err(self.collision_error_exact(kind, token));
        }
        Ok(token.clone())
    }

    fn collision_error_exact(&self, kind: ArtifactKind, token: &PythonString) -> DoctrineError {
        let bare = Self::bare_exact(kind, token);
        let label = |value: &PythonString| match value.to_scalar_string() {
            Ok(text) => text,
            Err(_) => value.repr(),
        };
        let table = self.table(kind);
        let declared = table
            .by_id
            .get(&bare)
            .map(|entry| entry.declared_id.clone())
            .unwrap_or_else(|| bare.clone());
        let file_name = table
            .entries
            .iter()
            .find(|entry| entry.declared_id == declared)
            .map(|entry| entry.stem.clone())
            .unwrap_or_else(|| label(&bare));
        let declarations = table
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
            .collect::<Vec<_>>();
        DoctrineError::ActivationIdCollision {
            kind: kind.canonical().to_owned(),
            artifact_id: label(&bare),
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

    /// [`IdentityIndex::unindexed_key`], exactly.
    ///
    /// The reference's fallback is a Python `str` operation chain, and the
    /// pinned Kernel `str.upper()` covers every code unit a Python `str` may
    /// carry, so a token is never re-spelled lossily: surrogate units are
    /// preserved and their neighbouring scalars are uppercased exactly as the
    /// reference does.
    #[must_use]
    pub fn unindexed_key_exact(kind: Option<ArtifactKind>, token: &PythonString) -> PythonString {
        if kind == Some(ArtifactKind::Directive) {
            normalize_directive_id_exact(token)
        } else {
            token.clone()
        }
    }

    /// [`IdentityIndex::lookup_key`], exactly (declared id first, then the fallback).
    #[must_use]
    pub fn lookup_key_exact(
        &self,
        kind: Option<ArtifactKind>,
        token: &PythonString,
        selection: bool,
    ) -> PythonString {
        if let Some(kind) = kind {
            let known = if selection {
                self.selection_id_exact(kind, token)
            } else {
                self.declared_id_exact(kind, token)
            };
            if let Some(known) = known {
                return known;
            }
        }
        Self::unindexed_key_exact(kind, token)
    }

    /// Repository/comparison key of `token` (its declared id when known).
    pub fn lookup_key(&self, kind: Option<ArtifactKind>, token: &str, selection: bool) -> String {
        if let Some(kind) = kind {
            let known = if selection {
                self.selection_id(kind, token)
            } else {
                self.declared_id(kind, token)
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
    ///
    /// This scalar view is the [`IdentityIndex::try_activation_id_exact`] route
    /// through the same resolution and warning owner; a token that is not Rust
    /// scalar text is the exact view's business, not this one's.
    pub fn activation_id(
        &self,
        kind: ArtifactKind,
        token: &str,
        warnings: &mut Vec<String>,
    ) -> String {
        if let Some(declared) = self.declared_id(kind, token) {
            return declared;
        }
        let exact = PythonString::from_scalars(token);
        let resolved_exact = Self::activation_id_without_index(kind, &exact);
        if let Some(warning) = Self::unresolved_activation_warning(kind, &exact, &resolved_exact) {
            warnings.push(
                warning
                    .to_scalar_string()
                    .expect("a Python repr of a scalar token stays scalar text"),
            );
        }
        resolved_exact
            .to_scalar_string()
            .expect("a scalar token stays scalar text")
    }

    /// The ONE unknown-activation-token diagnostic the reference appends.
    ///
    /// Source:
    /// `f"unresolved {kind.value} activation token {token!r}; best-effort resolved to {resolved!r}"`.
    /// Both interpolations use Python `repr()`, so the one canonical Kernel
    /// formatter supplies them; a Rust `Debug` rendering is a different quoting
    /// and escaping rule and is never substituted. `None` for a known token, for
    /// `anti_pattern` (no files, silent passthrough) and for any other resolution
    /// the reference does not report.
    fn unresolved_activation_warning(
        kind: ArtifactKind,
        token: &PythonString,
        resolved: &PythonString,
    ) -> Option<PythonString> {
        if kind == ArtifactKind::AntiPattern {
            return None;
        }
        Some(PythonString::from_scalars(&format!(
            "unresolved {} activation token {}; best-effort resolved to {}",
            kind.canonical(),
            token.repr(),
            resolved.repr()
        )))
    }

    /// [`IdentityIndex::activation_id`], exactly.
    ///
    /// The reference resolves an activation token to a Python `str` declared id
    /// (`_gated`'s `_activated_id_cache`); a declared id holding a code point a
    /// Rust `String` cannot carry must still match its repository key. A known
    /// token yields its exact declared id; an unknown directive falls back to the
    /// normaliser (`Normalisation` in the reference), `anti_pattern` passes
    /// through, and any other unknown token is kept verbatim.
    ///
    /// This is the total capability view: an aborted kind contributes no
    /// entries. A caller that must reproduce the reference's phase asks
    /// [`IdentityIndex::try_activation_id_exact`].
    pub fn activation_id_exact(&self, kind: ArtifactKind, token: &PythonString) -> PythonString {
        self.try_activation_id_exact(kind, token)
            .unwrap_or_else(|_| Self::activation_id_without_index(kind, token))
    }

    /// The exact activation-list id, or the queried kind's scan abort.
    ///
    /// This is the production filtering phase `_gated` runs: an identity scan
    /// abort must escape instead of becoming a best-effort unresolved token.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_activation_id_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<PythonString, IdentityFailure> {
        self.try_activation_report_exact(kind, token)
            .map(|(resolved, _warning)| resolved)
    }

    /// [`IdentityIndex::try_activation_id_exact`] plus the reference's diagnostic.
    ///
    /// `Ok((id, Some(warning)))` is exactly the pair `_gated` records: the id the
    /// token resolves to and the one message it appends for an unknown token. A
    /// known token, an `anti_pattern` and a service-only token carry `None`. The
    /// warning text comes from the same owner the scalar view uses, so the two
    /// APIs cannot drift.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_activation_report_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<(PythonString, Option<PythonString>), IdentityFailure> {
        self.try_table(kind)?;
        if let Some(declared) = self.declared_id_exact(kind, token) {
            return Ok((declared, None));
        }
        let resolved = Self::activation_id_without_index(kind, token);
        let warning = Self::unresolved_activation_warning(kind, token, &resolved);
        Ok((resolved, warning))
    }

    /// The reference's fallback when the identity table holds no entry for
    /// *token*: anti-patterns and non-directives stay verbatim, and a directive
    /// token takes the canonical normaliser.
    fn activation_id_without_index(kind: ArtifactKind, token: &PythonString) -> PythonString {
        if kind == ArtifactKind::AntiPattern {
            return token.clone();
        }
        if kind == ArtifactKind::Directive {
            return normalize_directive_id_exact(token);
        }
        token.clone()
    }

    // ------------------------------------------------- fallible queries --
    //
    // Each `try_*` lookup is the same question as its infallible sibling, but it
    // fails at this call when scanning the *queried* kind raises the Python
    // exception the reference's loader does not catch. That is the reference's
    // own phase: `IdentityIndex._table` propagates out of the lookup, so a
    // caller that has to reproduce the abort must ask a `try_*` accessor.

    /// Declared id for a token, or the queried kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_declared_id_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<Option<PythonString>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.declared_id_exact(kind, token))
    }

    /// [`IdentityIndex::try_declared_id_exact`], scalar capability view.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_declared_id(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Option<String>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.declared_id(kind, token))
    }

    /// Declared id of `stem` in one layer, or the queried kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_declared_id_in_layer_exact(
        &self,
        kind: ArtifactKind,
        stem: &str,
        layer: IdentityLayer,
    ) -> Result<Option<PythonString>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.declared_id_in_layer_exact(kind, stem, layer))
    }

    /// The activation stem for a token, or the queried kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_stem_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<Option<PythonString>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.stem_exact(kind, token))
    }

    /// [`IdentityIndex::try_stem_exact`], scalar capability view.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_stem(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Option<String>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.stem(kind, token))
    }

    /// The stem whose declared id is `declared_id`, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_stem_for_declared_id(
        &self,
        kind: ArtifactKind,
        declared_id: &str,
    ) -> Result<Option<String>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.stem_for_declared_id(kind, declared_id))
    }

    /// Whether a token names a known artifact, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_knows(&self, kind: ArtifactKind, token: &str) -> Result<bool, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.knows(kind, token))
    }

    /// Whether an exact token names a known artifact, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_knows_exact(
        &self,
        kind: ArtifactKind,
        token: &PythonString,
    ) -> Result<bool, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.lookup_existing_exact(kind, token).is_some())
    }

    /// Whether any layer declares `token` as an id, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_has_declared_id(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<bool, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.has_declared_id(kind, token))
    }

    /// Whether `token` is a file stem, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_names_a_file(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<bool, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.names_a_file(kind, token))
    }

    /// The layer a token resolves in, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_source_layer(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Option<IdentityLayer>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.source_layer(kind, token))
    }

    /// The declaring file for a token, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_path_for(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Option<PathBuf>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.path_for(kind, token).map(Path::to_path_buf))
    }

    /// The declared id a selection resolves to, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_selection_id(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Option<String>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.selection_id(kind, token))
    }

    /// The writer's stored spelling, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`, never the collision refusal (which stays in
    /// the inner result).
    pub fn try_stored_spelling(
        &self,
        kind: ArtifactKind,
        token: &str,
    ) -> Result<Result<String, DoctrineError>, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.stored_spelling(kind, token))
    }

    /// The activation-list id for a token, or the kind's scan abort.
    ///
    /// # Errors
    ///
    /// The aborting file of `kind`.
    pub fn try_activation_id(
        &self,
        kind: ArtifactKind,
        token: &str,
        warnings: &mut Vec<String>,
    ) -> Result<String, IdentityFailure> {
        self.try_table(kind)?;
        Ok(self.activation_id(kind, token, warnings))
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
        IdentityIndex::declared_id(self, kind, token)
    }

    fn stem(&self, kind: ArtifactKind, token: &str) -> Option<String> {
        IdentityIndex::stem(self, kind, token)
    }
}

/// Populate the index for every artifact kind, best effort.
///
/// Construction no longer needs this: each kind is scanned on its first query.
/// It stays for callers that deliberately want one pass over the whole tree
/// (diagnostics, an operator-facing inventory) and for the infallible
/// capability view: a kind whose scan aborts is left as an empty table rather
/// than failing this call. A caller that must see the abort uses
/// [`IdentityIndex::try_scan`].
pub fn scan_all(index: &mut IdentityIndex) {
    for kind in ArtifactKind::ALL {
        let _ = index.try_table(kind);
    }
}

fn config_stem(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.split('.').next().unwrap_or(name).to_owned())
        .unwrap_or_default()
}

/// The declared id of one artifact file, exactly.
///
/// The reference index reads the artifact with the same `ruamel.yaml` safe
/// loader the profile repository uses, so a `profile-id: "author\ud800"` is the
/// exact Python `str` `author\ud800`. This reads through the one lossless
/// transport and construction phase for the same reason: a second YAML reader
/// could not carry that code point.
fn declared_id(
    document: &ArtifactDocument,
    kind: ArtifactKind,
    digit_limit: usize,
) -> Result<Option<PythonString>, IdentityFailure> {
    let field = crate::offering::id_field(kind);
    let Some(value) = document.root().lookup(field) else {
        return Ok(None);
    };
    // Source `str(data.get(field, ""))` runs OUTSIDE `_parse_id`'s read/load
    // `try`, so its `ValueError` escapes the scan and is never cached.
    let exact = document
        .try_python_str(value, digit_limit)
        .map_err(decimal_conversion_abort)?;
    let stripped = python_strip(&exact);
    Ok((!stripped.is_empty()).then_some(stripped))
}

// The one constructed-YAML read/text authority lives in the offering subtree so
// the identity index and the offering repositories cannot drift: the same
// transport, the same construction phase, the same Python `str()`/`repr()`.
// `pack_manager._declared_id` and `_read_id` are the same reader in the
// reference too.
pub(crate) use crate::offering::value::ArtifactDocument;

/// Whether `point` is Python `str.isspace()` — the pinned CPython/UCD table.
fn is_python_space(point: u32) -> bool {
    sk_kernel::python_text::is_whitespace(point)
}

/// Python `str.strip()` over exact code points.
pub(crate) fn python_strip(text: &PythonString) -> PythonString {
    let points = text.codepoints();
    let start = points
        .iter()
        .position(|point| !is_python_space(*point))
        .unwrap_or(points.len());
    let end = points
        .iter()
        .rposition(|point| !is_python_space(*point))
        .map_or(start, |position| position + 1);
    PythonString::from_codepoints(&points[start..end])
        .expect("a slice of valid code points is valid")
}

/// The artifact files of one layer directory, sorted (the reference's
/// `sorted(glob(...))` per scan directory).
pub(crate) fn artifact_files_in(directory: &Path, suffix: &str, depth: ScanDepth) -> Vec<PathBuf> {
    let mut files = Vec::new();
    match depth {
        ScanDepth::Flat => {
            if let Ok(entries) = std::fs::read_dir(directory) {
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
        }
        ScanDepth::Recursive | ScanDepth::RecursiveExcludingLegacy => {
            collect_recursive(directory, suffix, &mut files, depth);
        }
    }
    files.sort();
    files
}

fn collect_recursive(directory: &Path, suffix: &str, files: &mut Vec<PathBuf>, depth: ScanDepth) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Only the *immediate* `built-in/` child is reserved; a deeper
            // directory that happens to be named `built-in` stays discoverable,
            // exactly as the reference's loader still loads it.
            if depth == ScanDepth::RecursiveExcludingLegacy
                && path.file_name().and_then(|name| name.to_str()) == Some("built-in")
            {
                continue;
            }
            collect_recursive(&path, suffix, files, ScanDepth::Recursive);
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
    fn a_non_scalar_declared_id_resolves_only_exactly() {
        // The reference's index is keyed on Python `str`: `"author\ud800"` is
        // one code-point sequence, and only that sequence matches.
        let directory = tempfile::tempdir().unwrap();
        let built_in = directory.path().join("built-in");
        write(
            &built_in.join("agent_profiles/surrogate-file.agent.yaml"),
            "profile-id: \"author\\ud800\"\nname: Surrogate\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\n",
        );
        let mut index = IdentityIndex::from_roots(&built_in, &[], None);
        scan_all(&mut index);
        let exact =
            PythonString::from_codepoints(&[0x61, 0x75, 0x74, 0x68, 0x6f, 0x72, 0xd800]).unwrap();
        assert!(index.knows_exact(ArtifactKind::AgentProfile, &exact));
        assert_eq!(
            index
                .stored_spelling_exact(ArtifactKind::AgentProfile, &exact)
                .unwrap()
                .to_scalar_string()
                .unwrap(),
            "surrogate-file"
        );
        assert_eq!(
            index
                .stem_exact(ArtifactKind::AgentProfile, &exact)
                .unwrap()
                .to_scalar_string()
                .unwrap(),
            "surrogate-file"
        );
        // The scalar capability view reports the boundary, not a substituted id.
        assert_eq!(
            index.declared_id(ArtifactKind::AgentProfile, "author"),
            None
        );
        assert_eq!(
            index.declared_id_exact(ArtifactKind::AgentProfile, &exact),
            Some(exact.clone())
        );
        assert_eq!(
            index.declared_id_in_layer_exact(
                ArtifactKind::AgentProfile,
                "surrogate-file",
                IdentityLayer::BuiltIn
            ),
            Some(exact)
        );
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

    /// The pinned Source helper matrix (`identity_index._fast_plain_id`), measured in an
    /// independent 3.11.15 child. Every expected value is the reference's own answer.
    #[test]
    fn fast_plain_id_matches_the_source_helper_matrix() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("plain", "id: alpha\n", Some("alpha")),
            ("plain_no_newline", "id: alpha", Some("alpha")),
            ("plain_trailing_spaces", "id: alpha  \n", Some("alpha")),
            ("plain_lead_spaces", "id:   alpha\n", Some("alpha")),
            (
                "plain_malformed_body",
                "id: alpha\nbroken: [unclosed\n",
                Some("alpha"),
            ),
            ("raw_cr", "id: alpha\r\n", None),
            ("raw_lone_cr", "id: alpha\rx: y\r", None),
            ("quoted_value", "id: \"alpha\"\n", None),
            ("single_quoted_value", "id: 'alpha'\n", None),
            ("quoted_key_double", "\"id\": alpha\n", None),
            ("quoted_key_single", "'id': alpha\n", None),
            ("mismatched_quote_key", "id': alpha\n", None),
            ("space_before_colon", "id : alpha\n", None),
            ("no_space_after_colon", "id:alpha\n", None),
            ("tab_separator", "id:\talpha\n", None),
            ("tab_trailing", "id: alpha\t\n", None),
            ("duplicate_key", "id: a\nid: b\n", None),
            ("duplicate_quoted", "\"id\": a\nid: b\n", None),
            ("comment_value", "id: alpha # c\n", None),
            ("continuation_indented", "id: alpha\n  more\n", None),
            (
                "continuation_blank_then_indented",
                "id: alpha\n\n  more\n",
                Some("alpha"),
            ),
            ("continuation_tab", "id: alpha\n\tmore\n", None),
            (
                "continuation_comment",
                "id: alpha\n# c\n  more\n",
                Some("alpha"),
            ),
            ("numeric", "id: 123\n", None),
            ("bool_true", "id: true\n", None),
            ("bool_True", "id: True\n", None),
            ("bool_TRUE", "id: TRUE\n", None),
            ("bool_yes", "id: yes\n", None),
            ("bool_Yes", "id: Yes\n", None),
            ("bool_on", "id: on\n", None),
            ("bool_oN", "id: oN\n", None),
            ("bool_off", "id: off\n", None),
            ("null", "id: null\n", None),
            ("null_NULL", "id: NULL\n", None),
            ("y", "id: y\n", None),
            ("n", "id: n\n", None),
            ("Y_upper", "id: Y\n", None),
            ("N_upper", "id: N\n", None),
            ("tilde", "id: ~\n", None),
            ("nan_dot", "id: .nan\n", None),
            ("nan_dot_mixed", "id: .NaN\n", None),
            ("inf_dot", "id: .inf\n", None),
            ("inf_dot_mixed", "id: .Inf\n", None),
            ("plain_nan_word", "id: nan\n", Some("nan")),
            ("underscore_lead", "id: _alpha\n", None),
            ("dash_lead", "id: -alpha\n", None),
            ("dot_lead", "id: .alpha\n", None),
            ("digit_lead", "id: 9alpha\n", None),
            (
                "accepted_charset",
                "id: alpha.beta-gamma_1\n",
                Some("alpha.beta-gamma_1"),
            ),
            ("uppercase_lead", "id: Alpha\n", Some("Alpha")),
            ("non_ascii", "id: caf\u{e9}\n", None),
            ("nul_in_value", "id: al\u{0}pha\n", None),
            ("bom_start", "\u{feff}id: alpha\n", None),
            ("bom_later", "id: alpha\nx: \u{feff}\n", None),
            ("nbsp_trailing", "id: alpha\u{a0}\n", None),
            ("nbsp_separator", "id:\u{a0}alpha\n", None),
            ("nbsp_before_colon", "id\u{a0}: alpha\n", None),
            ("em_space_trailing", "id: alpha\u{2003}\n", None),
            ("vtab_trailing", "id: alpha\u{b}\n", None),
            ("nel_trailing", "id: alpha\u{85}\n", None),
            ("info_sep_trailing", "id: alpha\u{1c}\n", None),
            ("marker_leading", "---\nid: alpha\n", Some("alpha")),
            ("marker_leading_space", "--- \nid: alpha\n", Some("alpha")),
            (
                "marker_leading_after_comments",
                "\n\n# c\n  \n---\nid: alpha\n",
                Some("alpha"),
            ),
            ("marker_two", "---\nid: alpha\n---\n", None),
            ("marker_closing", "id: alpha\n...\n", None),
            ("marker_late", "id: alpha\n---\nx: y\n", None),
            ("marker_indented", "  ---\nid: alpha\n", Some("alpha")),
            ("marker_dashes_x", "# c\n---x\nid: alpha\n", Some("alpha")),
            ("block_scalar_line", "x: |\nid: alpha\n", Some("alpha")),
        ];
        for (label, text, expected) in cases {
            let found = fast_plain_id(text, "id").map(|value| value.to_scalar_string().unwrap());
            assert_eq!(found.as_deref(), *expected, "{label}");
        }
        for (label, text) in [
            ("marker_two", "---\nid: alpha\n---\n"),
            ("marker_closing", "id: alpha\n...\n"),
            ("marker_late", "id: alpha\n---\nx: y\n"),
        ] {
            assert!(has_foreign_document_marker(text), "{label}");
        }
        assert_eq!(
            fast_plain_id("profile-id: p-one\n", "profile-id").unwrap(),
            PythonString::from_scalars("p-one")
        );
        assert_eq!(
            fast_plain_id("a.b: x\n", "a.b").unwrap(),
            PythonString::from_scalars("x")
        );
        assert_eq!(
            fast_plain_id("id[0]: x\n", "id[0]").unwrap(),
            PythonString::from_scalars("x")
        );
        assert!(
            fast_plain_id("pro\tfile: alpha\n", "pro\tfile").is_none(),
            "Source rejects tabs anywhere in found.group(0), including the literal key"
        );
    }
}
