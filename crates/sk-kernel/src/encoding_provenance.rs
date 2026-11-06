//! Source encoding-read provenance lifecycle (reference
//! `charter.activation._io`, `kernel.event_log_append`, `kernel.clock`).
//!
//! The reference's single charter ingestion chokepoint
//! (`charter.activation._io.load_charter_file`) records **every** successful
//! read in an append-only JSONL provenance log:
//!
//! * `.kittify/encoding-provenance/global.jsonl` for a project-level document,
//! * `kitty-specs/<mission>/.encoding-provenance.jsonl` for a mission document.
//!
//! That record is the operator's evidence for *which encoding a human-authored
//! file was interpreted as*, and it is written only when a read actually
//! succeeded. This module is the Rust home of that capability: one record model,
//! one routing rule, one append policy — no second writer, lock, clock, JSON
//! encoder or path authority.
//!
//! ## Reference policies reproduced here
//!
//! * **Record shape.** `json.dumps(record, sort_keys=True)`: the nine keys
//!   `actor, at, bypass_used, confidence, event_id, file_path, mission_id,
//!   normalization_applied, source_encoding` in sorted order, `", "` / `": "`
//!   separators, `ensure_ascii=True`. Rendered by
//!   [`PythonJsonValue::render_python_with`] — the workspace's one
//!   `json.dumps` authority — never by `serde_json`.
//! * **One record per successful read; no deduplication.** Two reads of the same
//!   file append two records with fresh `event_id`/`at` (Source-probed).
//! * **Phase order.** `_build_provenance_record` (mission metadata, identity,
//!   then timestamp) and `_route_provenance_path` run *outside* the reference's
//!   `try:`; only the append is inside it. See
//!   [`EncodingProvenanceJournal::record_read`].
//! * **Caught write failure.** The append is wrapped in
//!   `try: ... except OSError: logger.warning(...)`: a provenance write failure
//!   must never break the read. [`EncodingProvenanceWarning`] carries the
//!   reference's own message text (`Failed to write encoding provenance to
//!   <path>`) instead of swallowing it, so the caller decides how to surface it.
//!   No rollback, no extra lock and no invented atomicity: the append goes
//!   through the existing [`crate::event_log_append`] protocol (torn-tail
//!   refusal, per-log lock, single-unit write, fsync).
//! * **Path placement.** A mission document routes to the mission's own log and
//!   never to both; anything else routes to the project-level log. The reference
//!   resolves the global path against the process CWD; this module takes the
//!   project root explicitly instead (no implicit CWD authority). A mission
//!   prefix carried by an absolute source path stays absolute, exactly as the
//!   reference derives it from the source path's own components.
//! * **`file_path`.** `str(source_path)` / `os.fsdecode` under the captured
//!   Kernel startup filesystem context: UTF-8 and ASCII use CPython's
//!   `surrogateescape`; actual ISO-8859-1/ISO-8859-15 decode their real byte
//!   mappings; an unimplemented legacy charset or a non-Unix platform is an
//!   explicit typed Native gap, never U+FFFD, lossy text or an empty path.
//!   `os.fsdecode(str)` is identity and never passes through the byte decoder.
//!   Inline bytes with no path still carry `""`.
//!
//! ## Bounded non-claims
//!
//! * `event_id` is a **format-valid, per-instance-counter ULID** (48-bit millisecond
//!   timestamp plus a 32-bit pid and a 48-bit per-instance counter). The
//!   reference draws 80 bits from `ulid.new()`; cryptographic randomness needs
//!   the workspace `ulid` crate, which is a parent-owned manifest addition.
//!   This does not establish process-wide uniqueness across fresh journal
//!   instances or reproduce Source entropy; both remain unapproved gaps.
//! * `mission_id` uses exact Python str() over the canonical J1 value. Only the
//!   reference's own caught syntax/IO cases become absence; other failures
//!   precede identity minting and appending.
//! * The startup context is captured once per journal from the process
//!   environment, matching the interpreter's own startup decision; it is not
//!   resampled per record. Non-Unix wide-character path spelling is an explicit
//!   typed gap, not a `to_string_lossy` parity claim.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::KernelError;
use crate::PythonString;
use crate::clock::{Clock, SystemClock};
use crate::event_log_append::{EventLogAppendOptions, append_event_log_lines};
use crate::python_json::{PythonJsonRenderOptions, PythonJsonValue};
use crate::python_os_encoding::{OsDecodeError, OsEncodingContext};

/// Project-level provenance log, relative to the project root.
pub const GLOBAL_LOG_RELATIVE: &str = ".kittify/encoding-provenance/global.jsonl";

/// Per-mission provenance log file name inside `kitty-specs/<mission>/`.
pub const MISSION_LOG_FILENAME: &str = ".encoding-provenance.jsonl";

/// Directory component that marks a mission document.
pub const SPECS_DIR_NAME: &str = "kitty-specs";

/// The actor label the reference writes into every record.
pub const ACTOR: &str = "spec-kitty charter";

/// The reference's fail-closed diagnostic for an unprovable encoding.
pub const AMBIGUOUS_CODE: &str = "CHARTER_ENCODING_AMBIGUOUS";

const CROCKFORD: [u8; 32] = *b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ULID_TEXT_LEN: usize = 26;
const TIMESTAMP_TEXT_LEN: usize = 10;
const RANDOM_SUFFIX_MASK: u128 = (1u128 << 80) - 1;
const COUNTER_MASK: u64 = (1u64 << 48) - 1;

/// The encoding facts one successful read contributes to its record.
///
/// These are exactly the fields the reference's `CharterContent` carries into
/// `_build_provenance_record`, so a caller cannot invent a confidence or a
/// normalization flag the decode did not produce.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadEncodingFacts<'a> {
    /// The proven encoding name (`utf-8`, `utf-8-sig`, `utf-16-le`, …).
    pub source_encoding: &'a str,
    /// The detection confidence the decode reported.
    pub confidence: f64,
    /// Whether the bytes were re-encoded relative to plain UTF-8 (BOM stripping
    /// and every non-UTF-8 decode set this).
    pub normalization_applied: bool,
    /// Whether an operator override produced the text.
    pub bypass_used: bool,
}

/// One `EncodingProvenanceRecord`.
///
/// The serialized field set is the reference's. `log_path` is this module's
/// private routing fact and is never part of the JSON line.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodingProvenanceRecord {
    event_id: String,
    at: String,
    file_path: PythonString,
    source_encoding: String,
    confidence: f64,
    normalization_applied: bool,
    bypass_used: bool,
    actor: String,
    mission_id: Option<PythonString>,
    log_path: PathBuf,
}

impl EncodingProvenanceRecord {
    /// The minted ULID identity of this record.
    pub fn event_id(&self) -> &str {
        &self.event_id
    }

    /// The canonical aware-UTC `datetime.isoformat()` stamp of the record.
    pub fn at(&self) -> &str {
        &self.at
    }

    /// `str(source_path)` of the read, or `""` for inline bytes.
    pub fn file_path(&self) -> &PythonString {
        &self.file_path
    }

    /// The proven source encoding name.
    pub fn source_encoding(&self) -> &str {
        &self.source_encoding
    }

    /// The detection confidence the decode reported.
    pub fn confidence(&self) -> f64 {
        self.confidence
    }

    /// Whether the read was normalized away from plain UTF-8.
    pub fn normalization_applied(&self) -> bool {
        self.normalization_applied
    }

    /// Whether an operator override produced the text.
    pub fn bypass_used(&self) -> bool {
        self.bypass_used
    }

    /// The actor label (`spec-kitty charter`).
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Exact Python `str(value)` from mission metadata, including non-scalar text.
    pub fn mission_id(&self) -> Option<&PythonString> {
        self.mission_id.as_ref()
    }

    /// The log this record is appended to (a routing fact, not a wire field).
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// The record as the exact Python `json.dumps` value tree.
    ///
    /// Member order here is the reference's `record` literal order; the
    /// `sort_keys=True` rule is applied by the encoder, never by this builder.
    pub fn to_json_value(&self) -> PythonJsonValue {
        PythonJsonValue::Object(vec![
            (
                "event_id".to_owned(),
                PythonJsonValue::String(PythonString::from_scalars(&self.event_id)),
            ),
            (
                "at".to_owned(),
                PythonJsonValue::String(PythonString::from_scalars(&self.at)),
            ),
            (
                "file_path".to_owned(),
                PythonJsonValue::String(self.file_path.clone()),
            ),
            (
                "source_encoding".to_owned(),
                PythonJsonValue::String(PythonString::from_scalars(&self.source_encoding)),
            ),
            (
                "confidence".to_owned(),
                PythonJsonValue::Float(self.confidence),
            ),
            (
                "normalization_applied".to_owned(),
                PythonJsonValue::Bool(self.normalization_applied),
            ),
            (
                "bypass_used".to_owned(),
                PythonJsonValue::Bool(self.bypass_used),
            ),
            (
                "actor".to_owned(),
                PythonJsonValue::String(PythonString::from_scalars(&self.actor)),
            ),
            (
                "mission_id".to_owned(),
                match &self.mission_id {
                    Some(id) => PythonJsonValue::String(id.clone()),
                    None => PythonJsonValue::Null,
                },
            ),
        ])
    }

    /// The one log line: `json.dumps(record, sort_keys=True)` without a newline.
    ///
    /// Key order comes from the shared encoder's `sort_keys` rule (never a
    /// hand-sorted literal here) and `ensure_ascii=True` is the reference
    /// default, so a surrogateescaped `file_path` renders as `\udcXX` escapes.
    pub fn to_json_line(&self) -> String {
        let options = PythonJsonRenderOptions::new(None)
            .ensure_ascii(true)
            .sort_keys(true);
        let rendered = self.to_json_value().render_python_with(options);
        // `ensure_ascii=True` never leaves a surrogate, so the scalar projection
        // is total for every record this module can build.
        rendered
            .to_scalar_string()
            .expect("an ensure_ascii JSON rendering is always UTF-8 representable")
    }
}

/// Mints the record's `event_id`.
///
/// Injected so a test can produce a stable identity; production uses
/// [`SystemEventIds`]. The reference calls `ulid.new()` unconditionally.
pub trait EventIdSource {
    /// Mint the next identity for a record stamped at `millis` (Unix epoch ms).
    fn next_event_id(&self, millis: i64) -> Result<String, EncodingProvenanceError>;
}

/// The production identity source: one format-valid ULID per record.
#[derive(Debug, Default)]
pub struct SystemEventIds {
    counter: AtomicU64,
}

impl SystemEventIds {
    /// A fresh identity source.
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventIdSource for SystemEventIds {
    fn next_event_id(&self, millis: i64) -> Result<String, EncodingProvenanceError> {
        let millis = u64::try_from(millis).map_err(|_| EncodingProvenanceError::EventId {
            reason: format!("timestamp {millis} ms is before the Unix epoch"),
        })?;
        let counter = self.counter.fetch_add(1, Ordering::Relaxed) & COUNTER_MASK;
        let pid = u128::from(std::process::id()) & 0xFFFF_FFFF;
        Ok(ulid_text(
            millis,
            ((pid << 48) | u128::from(counter)) & RANDOM_SUFFIX_MASK,
        ))
    }
}

/// Crockford-base32 ULID text: 48 timestamp bits then 80 identity bits.
fn ulid_text(millis: u64, suffix: u128) -> String {
    let mut text = [CROCKFORD[0]; ULID_TEXT_LEN];
    let mut timestamp = millis & ((1u64 << 48) - 1);
    for slot in (0..TIMESTAMP_TEXT_LEN).rev() {
        text[slot] = CROCKFORD[(timestamp & 0x1F) as usize];
        timestamp >>= 5;
    }
    let mut identity = suffix & RANDOM_SUFFIX_MASK;
    for slot in (TIMESTAMP_TEXT_LEN..ULID_TEXT_LEN).rev() {
        text[slot] = CROCKFORD[(identity & 0x1F) as usize];
        identity >>= 5;
    }
    // The array is ASCII by construction.
    String::from_utf8(text.to_vec()).expect("the Crockford alphabet is ASCII")
}

/// A provenance append the reference catches instead of propagating.
///
/// `message` is the reference's own log body, so a caller that prints it
/// reproduces the operator-visible text; `source` keeps the typed cause rather
/// than collapsing it to a string.
#[derive(Debug)]
pub struct EncodingProvenanceWarning {
    path: PathBuf,
    message: String,
    source: EncodingProvenanceError,
}

impl EncodingProvenanceWarning {
    /// The log path the append targeted.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The reference's warning body for that path.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The caught append failure.
    pub fn source(&self) -> &EncodingProvenanceError {
        &self.source
    }
}

/// A provenance lifecycle failure the reference would raise.
#[derive(Debug, thiserror::Error)]
pub enum EncodingProvenanceError {
    /// A record could not be minted (outside the reference's `try:` block, so it
    /// escapes there too).
    #[error("cannot mint an encoding-provenance event id: {reason}")]
    EventId {
        /// Why the identity could not be produced.
        reason: String,
    },
    /// Strict UTF-8 metadata decoding escaped the reference's catch tuple.
    #[error("{source}")]
    MissionDecode {
        /// Metadata path.
        path: PathBuf,
        /// Canonical decoding cause.
        #[source]
        source: Box<crate::python_utf8::PythonUtf8DecodeError>,
    },
    /// A non-syntax JSON failure escaped metadata loading (e.g. digit ceiling),
    /// or the Native decoder reached its explicitly labelled operational limit.
    #[error("{source}")]
    MissionJson {
        /// Metadata path.
        path: PathBuf,
        /// Canonical JSON cause; native limits are not Python exceptions.
        #[source]
        source: Box<crate::python_json::PythonJsonError>,
    },
    /// Python membership/subscript raised TypeError on non-object metadata.
    #[error("{message}")]
    MissionType {
        /// Metadata path.
        path: PathBuf,
        /// Actual Source TypeError text.
        message: String,
    },
    /// The append itself failed. The reference catches exactly this class.
    #[error("cannot append encoding provenance to {path}: {source}")]
    Append {
        /// The routed log path.
        path: PathBuf,
        /// The underlying writer refusal or I/O failure.
        #[source]
        source: KernelError,
    },
    /// The native path spelling could not be decoded under the captured
    /// startup filesystem codec. Source would have produced text here, so this
    /// is an explicit Native capability gap, never a fake Source exception or
    /// lossy/empty path.
    #[error("cannot spell filesystem path {path} under the captured startup encoding: {source}")]
    FilePathDecode {
        /// The path whose native spelling was requested.
        path: PathBuf,
        /// The typed decoder/platform/context cause.
        #[source]
        source: OsDecodeError,
    },
}

impl EncodingProvenanceError {
    /// Whether the reference's `except OSError` would have caught this failure.
    pub fn is_caught_by_reference(&self) -> bool {
        matches!(self, Self::Append { .. })
    }
}

/// One completed read's provenance outcome.
#[derive(Debug)]
pub struct RecordedRead {
    record: EncodingProvenanceRecord,
    warning: Option<EncodingProvenanceWarning>,
}

impl RecordedRead {
    /// The appended record.
    pub fn record(&self) -> &EncodingProvenanceRecord {
        &self.record
    }

    /// The log the record was appended to.
    pub fn log_path(&self) -> &Path {
        self.record.log_path()
    }

    /// The caught append failure, when the reference would have logged one.
    pub fn warning(&self) -> Option<&EncodingProvenanceWarning> {
        self.warning.as_ref()
    }

    /// Consume the outcome into its record and optional caught warning.
    pub fn into_parts(self) -> (EncodingProvenanceRecord, Option<EncodingProvenanceWarning>) {
        (self.record, self.warning)
    }
}

/// The one Source encoding-provenance lifecycle.
///
/// Holds the project root (the base for a project-level record) and the injected
/// clock and identity source. Nothing here caches, deduplicates or retries: each
/// call is one record and one append.
pub struct EncodingProvenanceJournal {
    root: PathBuf,
    actor: String,
    clock: Box<dyn Clock>,
    ids: Box<dyn EventIdSource>,
    os_encoding: OsEncodingContext,
}

impl EncodingProvenanceJournal {
    /// A journal for `root` using the canonical clock and the production
    /// identity source.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_sources(root, Box::new(SystemClock), Box::new(SystemEventIds::new()))
    }

    /// A journal with an injected clock and identity source (tests, and a caller
    /// that already owns a frozen clock).
    pub fn with_sources(
        root: impl Into<PathBuf>,
        clock: Box<dyn Clock>,
        ids: Box<dyn EventIdSource>,
    ) -> Self {
        Self {
            root: root.into(),
            actor: ACTOR.to_owned(),
            clock,
            ids,
            os_encoding: OsEncodingContext::from_environment(),
        }
    }

    /// The project root this journal writes under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The record's `actor` label.
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// The log the reference routes a read of `source_path` to.
    ///
    /// * A `kitty-specs/<mission>/…` source path routes to that mission's log,
    ///   keeping the prefix the source path itself carries (absolute stays
    ///   absolute).
    /// * Everything else — including inline bytes with no path — routes to the
    ///   project-level log.
    ///
    /// A returned relative path is anchored on [`root`](Self::root); the
    /// reference anchors it on the process CWD.
    pub fn route(&self, source_path: Option<&Path>) -> PathBuf {
        match source_path.and_then(mission_directory) {
            Some(mission) => anchored(&mission, Path::new(MISSION_LOG_FILENAME)),
            None => anchored(&self.root, Path::new(GLOBAL_LOG_RELATIVE)),
        }
    }

    /// The `mission_id` the reference reads from `meta.json` beside a mission
    /// document.
    ///
    /// `None` for a non-mission path, an absent/unreadable `meta.json`, a
    /// document without a `mission_id` key. A present value uses the ONE Python
    /// str authority; Unicode/type/non-syntax decoder failures escape before mint.
    pub fn mission_id(
        &self,
        source_path: Option<&Path>,
    ) -> Result<Option<PythonString>, EncodingProvenanceError> {
        let Some(mission) = source_path.and_then(mission_directory) else {
            return Ok(None);
        };
        let meta = anchored(&mission, Path::new("meta.json"));
        let Ok(bytes) = crate::read_source_bytes(&meta) else {
            return Ok(None);
        };
        let text = crate::python_utf8::decode_utf8(&bytes).map_err(|source| {
            EncodingProvenanceError::MissionDecode {
                path: meta.clone(),
                source: Box::new(source),
            }
        })?;
        let value = match crate::python_json::parse(text) {
            Ok(value) => value,
            Err(source) => match source.kind() {
                crate::python_json::PythonJsonErrorKind::IntegerDigitLimit
                | crate::python_json::PythonJsonErrorKind::IntegerDigitLimitSetting
                | crate::python_json::PythonJsonErrorKind::RecursionBudget
                | crate::python_json::PythonJsonErrorKind::NumberConversion => {
                    return Err(EncodingProvenanceError::MissionJson {
                        path: meta,
                        source: Box::new(source),
                    });
                }
                _ => return Ok(None), // Source catches JSONDecodeError, not every ValueError.
            },
        };
        if let Some(id) = value.field("mission_id") {
            return Ok(Some(crate::python_json::python_json_str(id)));
        }
        let message = match &value {
            PythonJsonValue::Object(_) | PythonJsonValue::ExactObject(_) => return Ok(None),
            PythonJsonValue::Array(items) => {
                let key = PythonJsonValue::scalar("mission_id");
                if !items
                    .iter()
                    .any(|item| crate::python_json::python_json_equal(item, &key))
                {
                    return Ok(None);
                }
                "list indices must be integers or slices, not str".to_owned()
            }
            PythonJsonValue::String(text) => {
                let key = PythonString::from_scalars("mission_id");
                if !text
                    .codepoints()
                    .windows(key.codepoints().len())
                    .any(|part| part == key.codepoints())
                {
                    return Ok(None);
                }
                "string indices must be integers, not 'str'".to_owned()
            }
            other => format!(
                "argument of type '{}' is not iterable",
                crate::python_json::python_json_type_name(other)
            ),
        };
        Err(EncodingProvenanceError::MissionType {
            path: meta,
            message,
        })
    }

    /// Mint the record for one successful read (no write).
    pub fn record(
        &self,
        source_path: Option<&Path>,
        facts: ReadEncodingFacts<'_>,
    ) -> Result<EncodingProvenanceRecord, EncodingProvenanceError> {
        // Source constructs path text first, then metadata, id and `at`.
        // A path/metadata failure cannot consume an identity/clock or append.
        let file_path = match source_path {
            Some(path) => self.os_encoding.decode_path(path).map_err(|source| {
                EncodingProvenanceError::FilePathDecode {
                    path: path.to_path_buf(),
                    source,
                }
            })?,
            None => PythonString::from_scalars(""),
        };
        let mission_id = self.mission_id(source_path)?;
        let event_id = self
            .ids
            .next_event_id(self.clock.now().timestamp_millis())?;
        let at = self.clock.now_iso();
        Ok(EncodingProvenanceRecord {
            event_id,
            at,
            file_path,
            source_encoding: facts.source_encoding.to_owned(),
            confidence: facts.confidence,
            normalization_applied: facts.normalization_applied,
            bypass_used: facts.bypass_used,
            actor: self.actor.clone(),
            mission_id,
            log_path: self.route(source_path),
        })
    }

    /// Append one already-minted record through the canonical locked event-log
    /// writer.
    ///
    /// `create_parents` is the reference's `create_parents=True`; every other
    /// option is the shared writer's default (torn-tail refusal, bounded per-log
    /// lock, one write, fsync).
    pub fn write(
        &self,
        record: &EncodingProvenanceRecord,
    ) -> Result<PathBuf, EncodingProvenanceError> {
        let path = record.log_path().to_path_buf();
        let options = EventLogAppendOptions {
            create_parents: true,
            ..EventLogAppendOptions::default()
        };
        append_event_log_lines(&path, &[record.to_json_line()], &options).map_err(|source| {
            EncodingProvenanceError::Append {
                path: path.clone(),
                source,
            }
        })?;
        Ok(path)
    }

    /// Mint and append one record for a successful read, preserving the
    /// reference's phase order.
    ///
    /// A mint failure escapes (the reference builds the record outside its
    /// `try:`); an append failure is *caught* into
    /// [`RecordedRead::warning`], exactly like `except OSError:
    /// logger.warning(...)`. The read's own text is never affected.
    pub fn record_read(
        &self,
        source_path: Option<&Path>,
        facts: ReadEncodingFacts<'_>,
    ) -> Result<RecordedRead, EncodingProvenanceError> {
        let record = self.record(source_path, facts)?;
        let path = record.log_path().to_path_buf();
        let options = EventLogAppendOptions {
            create_parents: true,
            ..EventLogAppendOptions::default()
        };
        let warning = match append_event_log_lines(&path, &[record.to_json_line()], &options) {
            Ok(()) => None,
            Err(source) => Some(EncodingProvenanceWarning {
                message: format!("Failed to write encoding provenance to {}", path.display()),
                path: path.clone(),
                source: EncodingProvenanceError::Append {
                    path: path.clone(),
                    source,
                },
            }),
        };
        Ok(RecordedRead { record, warning })
    }
}

/// Join a possibly-relative route onto `base`.
fn anchored(base: &Path, relative: &Path) -> PathBuf {
    if relative.is_absolute() {
        relative.to_path_buf()
    } else {
        base.join(relative)
    }
}

/// The `kitty-specs/<mission>` prefix of `source_path`, if it has one.
///
/// Mirrors the reference's `Path(*parts[: idx + 2])`: the first `kitty-specs`
/// component plus the component after it, keeping whatever prefix the path
/// carried.
fn mission_directory(source_path: &Path) -> Option<PathBuf> {
    let components: Vec<Component<'_>> = source_path.components().collect();
    let index = components.iter().position(|component| {
        matches!(component, Component::Normal(name) if name.to_str() == Some(SPECS_DIR_NAME))
    })?;
    if index + 1 >= components.len() {
        return None;
    }
    let mut mission = PathBuf::new();
    for component in &components[..=index + 1] {
        mission.push(component.as_os_str());
    }
    Some(mission)
}

#[cfg(test)]
mod cf69_parent_phase_tests {
    use super::*;

    struct NoClock;
    impl Clock for NoClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            panic!("path failure must precede clock access")
        }
    }
    struct NoIds;
    impl EventIdSource for NoIds {
        fn next_event_id(&self, _: i64) -> Result<String, EncodingProvenanceError> {
            panic!("path failure must precede mint")
        }
    }

    #[test]
    fn native_path_boundary_precedes_metadata_mint_and_clock() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("kitty-specs/m/doc.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path.parent().unwrap().join("meta.json"), [0xff]).unwrap();
        let mut journal = EncodingProvenanceJournal::with_sources(
            root.path(),
            Box::new(NoClock),
            Box::new(NoIds),
        );
        journal.os_encoding =
            OsEncodingContext::from_lookup(|key| (key == "PYTHONUTF8").then(|| "invalid".into()));
        let result = journal.record(
            Some(&path),
            ReadEncodingFacts {
                source_encoding: "utf-8",
                confidence: 1.0,
                normalization_applied: false,
                bypass_used: false,
            },
        );
        assert!(matches!(
            result,
            Err(EncodingProvenanceError::FilePathDecode { .. })
        ));
        assert!(!journal.route(Some(&path)).exists());
    }
}
