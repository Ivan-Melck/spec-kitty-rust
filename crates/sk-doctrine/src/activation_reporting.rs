//! Source `charter.activation.activation_ledger` warning/report side effects.
//!
//! The pinned reference prints two plain-stderr lines while it reads activation
//! state. Returning the same metadata is not that side effect: the bytes are the
//! capability, and their stage matters.
//!
//! * `report_stray_legacy_keys` (FR-020) — legacy `activated_<kind>` keys that
//!   sit beside a ledger are ignored because the ledger wins.
//! * `report_legacy_deny_keys` (hole 6) — a hand-written legacy
//!   `deactivated_<kind>` key is reported and never applied.
//!
//! Measured Source shape reproduced here:
//!
//! * ONE module-level latch for BOTH reporters, keyed on the whole report tuple
//!   of `(Path, (key, ...))` entries, so a second read in the same process prints
//!   nothing and a changed tuple/path/key order prints again. Both reporters
//!   share it, so the same `(path, keys)` report suppresses the other function
//!   too;
//! * the report is recorded BEFORE the write, so a failed write still consumes
//!   the latch;
//! * the message is `Warning: <text>` on stderr — never a Python `UserWarning`
//!   (whose rendering names an internal source path and which `--json` modes
//!   silence) and never on stdout, so JSON output stays parseable;
//! * the stray list preserves the resolver's file/key order (`; ` between files,
//!   `, ` between keys); the legacy deny tuple is the sorted string keys only,
//!   so a non-string key is never stringified into it;
//! * the path is Source's `str(Path)`: pathlib construction spelling over the
//!   captured filesystem codec (no resolution, canonicalisation or re-encoding),
//!   and the latch compares Source's path COMPONENTS, so `a//b/./c.yaml` and
//!   `a/b/c.yaml` coalesce while `/x/a.yaml` and `//x/a.yaml` stay distinct.
//!
//! Delivery is a typed sink. The default sink is the reference's own stream: the
//! shared Kernel stdio composition with the measured stderr policy (the same
//! startup encoding resolution as stdout, always `backslashreplace`, no BOM).
//! The sink is called with the latch released — Source's `set.add`/`print`
//! sequence holds no lock, so a slow or failing stream cannot deadlock another
//! report.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use sk_kernel::python_os_encoding::{OsDecodeError, OsEncodingContext};
use sk_kernel::python_stream::{StreamError, write_stderr_line};
use sk_kernel::{PythonString, source_path_component_key, source_path_text};

use crate::ledger::{ACTIVATION_LEDGER_REL, DENY_PREFIX};

/// One file of a report: the latch identity, the display spelling, its keys.
///
/// `identity` is Source's `Path` component tuple and `keys` its ordered key
/// tuple; `display` is `str(Path)` for the message. Keeping both means the latch
/// compares Source's shape (`a//b/./c.yaml` coalesced, `//x` distinct) while the
/// message is spelled once by the canonical path authority.
#[derive(Clone, Debug)]
pub struct ReportedFile {
    identity: Vec<PythonString>,
    display: PythonString,
    keys: Vec<PythonString>,
}

/// The latch identity is Source's `(Path, (key, ...))`: the path COMPONENT tuple
/// and the ordered keys. The display spelling is a function of the components, so
/// it never takes part in equality or ordering (that is what Python's `Path`
/// hash/eq does).
impl PartialEq for ReportedFile {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity && self.keys == other.keys
    }
}

impl Eq for ReportedFile {}

impl PartialOrd for ReportedFile {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ReportedFile {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.identity, &self.keys).cmp(&(&other.identity, &other.keys))
    }
}

/// One report: the ordered files, exactly as Source's tuple of tuples.
#[derive(Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct StrayReport {
    files: Vec<ReportedFile>,
}

impl StrayReport {
    /// Whether the report carries nothing (Source prints nothing for `()`).
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The number of reported files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Build a report from the resolver's stray-key data, in its order.
    ///
    /// Source's `stray_legacy_keys` is already the `(Path, tuple(keys))` tuple;
    /// this keeps that order (it never sorts the stray list) and spells each path
    /// with [`source_path_text`].
    ///
    /// # Errors
    ///
    /// [`OsDecodeError`] when the captured filesystem codec cannot spell a path.
    pub fn from_resolver(
        stray: &[(PathBuf, Vec<String>)],
        codec: &OsEncodingContext,
    ) -> Result<Self, OsDecodeError> {
        let mut files = Vec::with_capacity(stray.len());
        for (path, keys) in stray {
            files.push(reported_file(
                path,
                keys.iter()
                    .map(|key| PythonString::from_scalars(key))
                    .collect(),
                codec,
            )?);
        }
        Ok(Self { files })
    }

    /// Build the legacy deny report for one legacy path, or `None` when it holds
    /// no `deactivated_<kind>` key (Source's `if keys:` guard).
    ///
    /// # Errors
    ///
    /// [`OsDecodeError`] when the captured filesystem codec cannot spell the path.
    pub fn legacy_deny(
        path: &Path,
        keys: Vec<PythonString>,
        codec: &OsEncodingContext,
    ) -> Result<Option<Self>, OsDecodeError> {
        if keys.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            files: vec![reported_file(path, keys, codec)?],
        }))
    }

    /// The ONE Source message body for a stray-keys report.
    pub fn stray_message(&self) -> PythonString {
        let mut listed = PythonString::default();
        for (index, file) in self.files.iter().enumerate() {
            if index > 0 {
                listed.push_scalars("; ");
            }
            push_joined(&mut listed, &file.keys, ", ");
            listed.push_scalars(" in ");
            listed.append(&file.display);
        }
        let mut message = PythonString::from_scalars("Legacy activation keys beside ");
        message.push_scalars(ACTIVATION_LEDGER_REL);
        message.push_scalars(" are ignored; ");
        message.push_scalars(&activation_ledger_name());
        message.push_scalars(" wins: ");
        message.append(&listed);
        message.push_scalars(". Remove them, or run `spec-kitty doctor charter-packs`.");
        message
    }

    /// The ONE Source message body for a legacy deny report.
    pub fn legacy_deny_message(&self) -> PythonString {
        let file = &self.files[0];
        let mut message = PythonString::default();
        push_joined(&mut message, &file.keys, ", ");
        message.push_scalars(" in ");
        message.append(&file.display);
        message.push_scalars(" is not applied: a deactivation is recorded only in ");
        message.push_scalars(ACTIVATION_LEDGER_REL);
        message.push_scalars(
            ". Remove the key by hand, then run `spec-kitty upgrade` (or `spec-kitty migrate \
             activation-ledger`).",
        );
        message
    }
}

/// How an operator-facing message names the activation ledger (never bare "the
/// ledger": the Migration Ledger exists too).
fn activation_ledger_name() -> String {
    let name = ACTIVATION_LEDGER_REL
        .rsplit('/')
        .next()
        .unwrap_or(ACTIVATION_LEDGER_REL);
    format!("the activation ledger ({name})")
}

fn push_joined(out: &mut PythonString, parts: &[PythonString], separator: &str) {
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.push_scalars(separator);
        }
        out.append(part);
    }
}

fn reported_file(
    path: &Path,
    keys: Vec<PythonString>,
    codec: &OsEncodingContext,
) -> Result<ReportedFile, OsDecodeError> {
    Ok(ReportedFile {
        identity: source_path_component_key(path, codec)?,
        display: source_path_text(path, codec)?,
        keys,
    })
}

/// The sorted `deactivated_<kind>` string keys of one legacy mapping.
///
/// Source's rule is `isinstance(key, str) and key.startswith("deactivated_")`,
/// `sorted(...)`: a non-string key (for example the YAML integer `1`) is never
/// stringified into the deny tuple, and order is Python's `str` order.
pub fn legacy_deny_keys(keys: impl IntoIterator<Item = PythonString>) -> Vec<PythonString> {
    let mut deny: Vec<PythonString> = keys
        .into_iter()
        .filter(|key| starts_with_scalars(key, DENY_PREFIX))
        .collect();
    deny.sort();
    deny
}

fn starts_with_scalars(value: &PythonString, prefix: &str) -> bool {
    value
        .codepoints()
        .iter()
        .copied()
        .take(prefix.chars().count())
        .eq(prefix.chars().map(u32::from))
}

/// The typed delivery seam for one report line.
///
/// The message is the complete `Warning: ...` line, without the trailing
/// newline (the sink's stream adds it, as `print` does).
pub trait ActivationReportSink {
    /// Deliver one already-formatted warning line.
    ///
    /// # Errors
    ///
    /// The sink's real failure; the caller must not treat it as a delivered line.
    fn deliver(&mut self, line: &PythonString) -> Result<(), ActivationReportError>;
}

/// Why a warning could not be delivered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivationReportError {
    /// The captured filesystem codec cannot spell a warning path.
    Path(OsDecodeError),
    /// The physical stderr write failed (codec refusal, closed pipe, full device).
    Stream(StreamError),
    /// A caller-supplied sink refused the line.
    Refused(String),
}

impl std::fmt::Display for ActivationReportError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(error) => write!(out, "cannot spell the activation warning path: {error}"),
            Self::Stream(error) => std::fmt::Display::fmt(error, out),
            Self::Refused(detail) => out.write_str(detail),
        }
    }
}

impl std::error::Error for ActivationReportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Stream(error) => Some(error),
            Self::Refused(_) => None,
        }
    }
}

impl From<OsDecodeError> for ActivationReportError {
    fn from(error: OsDecodeError) -> Self {
        Self::Path(error)
    }
}

impl From<StreamError> for ActivationReportError {
    fn from(error: StreamError) -> Self {
        Self::Stream(error)
    }
}

/// The reference's own sink: one `print(..., file=sys.stderr)` line.
#[derive(Clone, Copy, Debug, Default)]
pub struct SourceStderrSink;

impl ActivationReportSink for SourceStderrSink {
    fn deliver(&mut self, line: &PythonString) -> Result<(), ActivationReportError> {
        write_stderr_line(line).map_err(ActivationReportError::Stream)
    }
}

/// The process-wide report latch: Source's `_REPORTED_STRAY` set.
fn reported_stray() -> &'static Mutex<BTreeSet<StrayReport>> {
    static REPORTED: OnceLock<Mutex<BTreeSet<StrayReport>>> = OnceLock::new();
    REPORTED.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// Source `_warn_once`: record the report, then print `Warning: <message>`.
///
/// The latch is keyed on the whole report (in Source's order) and is updated
/// BEFORE the write, so a broken output stream still consumes the report, exactly
/// as the reference's `set.add` precedes `print`. The lock is released before the
/// sink runs, so a blocked stream cannot hold the latch against another report.
///
/// # Errors
///
/// The sink's real failure. The report stays recorded, as in the reference.
pub fn warn_once(
    report: &StrayReport,
    message: &PythonString,
    sink: &mut dyn ActivationReportSink,
) -> Result<(), ActivationReportError> {
    {
        let mut latch = reported_stray()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !latch.insert(report.clone()) {
            return Ok(());
        }
    }
    let mut line = PythonString::from_scalars("Warning: ");
    line.append(message);
    sink.deliver(&line)
}

/// FR-020: warn once per process for each distinct *stray* report.
///
/// # Errors
///
/// The sink's real failure. An empty report is silent (Source's `if stray:`).
pub fn report_stray_legacy_keys(
    report: &StrayReport,
    sink: &mut dyn ActivationReportSink,
) -> Result<(), ActivationReportError> {
    if report.is_empty() {
        return Ok(());
    }
    warn_once(report, &report.stray_message(), sink)
}

/// Hole 6: warn once per process that the legacy `deactivated_<kind>` keys are
/// not applied.
///
/// # Errors
///
/// The sink's real failure. An empty report is silent.
pub fn report_legacy_deny_keys(
    report: &StrayReport,
    sink: &mut dyn ActivationReportSink,
) -> Result<(), ActivationReportError> {
    if report.is_empty() {
        return Ok(());
    }
    warn_once(report, &report.legacy_deny_message(), sink)
}

/// Report the resolver's stray keys through the default Source stderr sink.
///
/// # Errors
///
/// A path-spelling refusal or the physical stderr failure.
pub(crate) fn report_stray_legacy_keys_to_stderr(
    stray: &[(PathBuf, Vec<String>)],
    codec: &OsEncodingContext,
) -> Result<(), ActivationReportError> {
    if stray.is_empty() {
        return Ok(());
    }
    let report = StrayReport::from_resolver(stray, codec)?;
    report_stray_legacy_keys(&report, &mut SourceStderrSink)
}

/// Report one legacy surface's deny keys through the default Source stderr sink.
///
/// # Errors
///
/// A path-spelling refusal or the physical stderr failure.
pub(crate) fn report_legacy_deny_keys_to_stderr(
    path: &Path,
    keys: Vec<PythonString>,
    codec: &OsEncodingContext,
) -> Result<(), ActivationReportError> {
    let Some(report) = StrayReport::legacy_deny(path, keys, codec)? else {
        return Ok(());
    };
    report_legacy_deny_keys(&report, &mut SourceStderrSink)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sink that keeps every delivered line, so the formatter and the latch can
    /// be checked without touching the process stderr.
    #[derive(Default)]
    struct MemorySink {
        lines: Vec<String>,
        fail_at: Option<usize>,
    }

    impl ActivationReportSink for MemorySink {
        fn deliver(&mut self, line: &PythonString) -> Result<(), ActivationReportError> {
            if self.fail_at == Some(self.lines.len()) {
                return Err(ActivationReportError::Refused("stream failed".to_owned()));
            }
            self.lines
                .push(line.to_scalar_string().expect("test report is scalar"));
            Ok(())
        }
    }

    fn codec() -> OsEncodingContext {
        OsEncodingContext::from_lookup(|name| (name == "PYTHONUTF8").then(|| "1".into()))
    }

    fn text(value: &str) -> PythonString {
        PythonString::from_scalars(value)
    }

    // The latch is process-wide and the test binary runs these cases in
    // parallel, so no case clears it: every case owns a unique report identity
    // (its own path) and therefore observes only its own inserts.

    fn stray(path: &str, keys: &[&str]) -> StrayReport {
        StrayReport::from_resolver(
            &[(
                PathBuf::from(path),
                keys.iter().map(|key| (*key).to_owned()).collect(),
            )],
            &codec(),
        )
        .expect("scalar paths decode")
    }

    #[test]
    fn the_stray_message_is_the_reference_text() {
        let report = StrayReport::from_resolver(
            &[
                (
                    PathBuf::from("/repo/.kittify/config.yaml"),
                    vec![
                        "activated_directives".to_owned(),
                        "activated_tactics".to_owned(),
                    ],
                ),
                (
                    PathBuf::from("/repo/.kittify/charter/charter.yaml"),
                    vec!["activated_kinds".to_owned()],
                ),
            ],
            &codec(),
        )
        .expect("scalar paths decode");
        assert_eq!(
            report.stray_message().to_scalar_string().unwrap(),
            "Legacy activation keys beside .kittify/charter/activation.yaml are ignored; the \
             activation ledger (activation.yaml) wins: activated_directives, activated_tactics \
             in /repo/.kittify/config.yaml; activated_kinds in \
             /repo/.kittify/charter/charter.yaml. Remove them, or run `spec-kitty doctor \
             charter-packs`."
        );
    }

    #[test]
    fn the_legacy_deny_message_is_the_reference_text() {
        let report = StrayReport::legacy_deny(
            Path::new("/repo/.kittify/config.yaml"),
            vec![text("deactivated_directives"), text("deactivated_tactics")],
            &codec(),
        )
        .expect("scalar path decodes")
        .expect("non-empty");
        assert_eq!(
            report.legacy_deny_message().to_scalar_string().unwrap(),
            "deactivated_directives, deactivated_tactics in /repo/.kittify/config.yaml is not \
             applied: a deactivation is recorded only in .kittify/charter/activation.yaml. \
             Remove the key by hand, then run `spec-kitty upgrade` (or `spec-kitty migrate \
             activation-ledger`)."
        );
    }

    #[test]
    fn the_latch_is_one_per_distinct_report_and_shared_by_both_reporters() {
        let mut sink = MemorySink::default();
        let report = stray("/latch/one/config.yaml", &["activated_directives"]);
        report_stray_legacy_keys(&report, &mut sink).unwrap();
        // Same report: silent.
        report_stray_legacy_keys(&report, &mut sink).unwrap();
        assert_eq!(sink.lines.len(), 1);
        // The legacy deny reporter shares the ONE latch: the same (path,keys)
        // report suppresses its different message too.
        let shared = StrayReport::legacy_deny(
            Path::new("/latch/one/config.yaml"),
            vec![text("activated_directives")],
            &codec(),
        )
        .unwrap()
        .unwrap();
        report_legacy_deny_keys(&shared, &mut sink).unwrap();
        assert_eq!(sink.lines.len(), 1, "the report tuple is the latch key");
        // A changed key order is a different report.
        let reordered = stray("/latch/one/config.yaml", &["activated_tactics"]);
        report_stray_legacy_keys(&reordered, &mut sink).unwrap();
        assert_eq!(sink.lines.len(), 2);
        // An empty report prints nothing and records nothing.
        let empty = StrayReport::from_resolver(&[], &codec()).unwrap();
        report_stray_legacy_keys(&empty, &mut sink).unwrap();
        assert_eq!(sink.lines.len(), 2);
        assert!(empty.is_empty() && empty.len() == 0);
    }

    #[test]
    fn the_latch_compares_source_path_components_not_canonicalised_paths() {
        let mut sink = MemorySink::default();
        // pathlib construction coalesces `.` and duplicate slashes...
        report_stray_legacy_keys(
            &stray("/latch/alias/a//b/./c.yaml", &["activated_directives"]),
            &mut sink,
        )
        .unwrap();
        report_stray_legacy_keys(
            &stray("/latch/alias/a/b/c.yaml", &["activated_directives"]),
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.lines.len(), 1);
        assert!(sink.lines[0].contains(" in /latch/alias/a/b/c.yaml."));
        // ...while a leading `//` is a distinct root spelling, never a canonical
        // absolute path.
        report_stray_legacy_keys(
            &stray("//latch/alias/x/a.yaml", &["activated_directives"]),
            &mut sink,
        )
        .unwrap();
        report_stray_legacy_keys(
            &stray("/latch/alias/x/a.yaml", &["activated_directives"]),
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.lines.len(), 3);
        assert!(sink.lines[1].contains(" in //latch/alias/x/a.yaml."));
        assert!(sink.lines[2].contains(" in /latch/alias/x/a.yaml."));
        // A relative and an absolute spelling stay distinct.
        report_stray_legacy_keys(
            &stray("latch-rel/a.yaml", &["activated_directives"]),
            &mut sink,
        )
        .unwrap();
        report_stray_legacy_keys(
            &stray("/latch-rel/a.yaml", &["activated_directives"]),
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.lines.len(), 5);
        assert!(sink.lines[3].contains(" in latch-rel/a.yaml."));
        assert!(sink.lines[4].contains(" in /latch-rel/a.yaml."));
    }

    #[test]
    fn the_report_is_recorded_before_the_write_so_a_failure_still_consumes_it() {
        let report = stray("/latch/failure/config.yaml", &["activated_directives"]);
        let mut failing = MemorySink {
            lines: Vec::new(),
            fail_at: Some(0),
        };
        assert_eq!(
            report_stray_legacy_keys(&report, &mut failing),
            Err(ActivationReportError::Refused("stream failed".to_owned()))
        );
        let mut recovered = MemorySink::default();
        report_stray_legacy_keys(&report, &mut recovered).unwrap();
        assert!(recovered.lines.is_empty(), "the failed report is consumed");
    }

    #[test]
    fn only_string_deny_keys_are_reported_and_they_are_sorted() {
        let keys = legacy_deny_keys([
            text("deactivated_tactics"),
            text("1"),
            text("activated_directives"),
            text("deactivated_directives"),
        ]);
        let rendered: Vec<String> = keys
            .iter()
            .map(|key| key.to_scalar_string().unwrap())
            .collect();
        assert_eq!(
            rendered,
            vec!["deactivated_directives", "deactivated_tactics"]
        );
    }

    /// The measured stderr bytes for a real warning line: the surrogate path
    /// unit escapes under the fixed `backslashreplace` handler, so the plain
    /// codecs agree byte for byte, and UTF-16/32 carry no BOM.
    #[cfg(unix)]
    #[test]
    fn the_default_sink_writes_the_measured_stderr_bytes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        use sk_kernel::python_stream::{StreamKind, StreamPolicy};

        let path = Path::new(OsStr::from_bytes(b"conf\xff.yaml"));
        let report = StrayReport::legacy_deny(path, vec![text("deactivated_directives")], &codec())
            .expect("the captured codec decodes the raw byte")
            .expect("a deny key is present");
        let mut line = PythonString::from_scalars("Warning: ");
        line.append(&report.legacy_deny_message());
        // The message still holds the exact surrogate code point; the escape is
        // the stderr handler's job, at encode time.
        assert!(line.codepoints().contains(&0xDCFF));

        let utf8 = StreamPolicy::resolve_for(StreamKind::Stderr, &|name: &str| {
            (name == "PYTHONIOENCODING").then(|| "utf-8".into())
        })
        .expect("stderr policy");
        let escaped = utf8.encode(&line).expect("the escape keeps it encodable");
        assert!(escaped.is_ascii(), "the surrogate escapes to ASCII");
        assert!(
            escaped
                .windows(b"conf\\udcff.yaml".len())
                .any(|window| window == b"conf\\udcff.yaml"),
            "{}",
            String::from_utf8_lossy(&escaped)
        );

        for setting in ["utf-8", "ascii", "latin-1", "iso8859-15"] {
            let policy = StreamPolicy::resolve_for(StreamKind::Stderr, &|name: &str| {
                (name == "PYTHONIOENCODING").then(|| setting.into())
            })
            .expect("stderr policy");
            assert_eq!(policy.errors(), "backslashreplace", "{setting}");
            assert_eq!(
                policy.encode(&line).expect("escaped line encodes"),
                escaped,
                "{setting}: every unencodable unit is the same ASCII escape"
            );
        }

        for (setting, width) in [("utf-16", 2usize), ("utf-32", 4usize)] {
            let policy = StreamPolicy::resolve_for(StreamKind::Stderr, &|name: &str| {
                (name == "PYTHONIOENCODING").then(|| setting.into())
            })
            .expect("stderr policy");
            let bytes = policy.encode(&line).expect("escaped line encodes");
            assert_eq!(bytes.len(), escaped.len() * width, "{setting}");
            let expected_first: Vec<u8> = if cfg!(target_endian = "little") {
                (u32::from(b'W'))
                    .to_le_bytes()
                    .into_iter()
                    .take(width)
                    .collect()
            } else {
                (u32::from(b'W'))
                    .to_be_bytes()
                    .into_iter()
                    .take(width)
                    .collect()
            };
            assert_eq!(
                &bytes[..width],
                &expected_first[..],
                "{setting}: the first unit is 'W' and there is no BOM"
            );
        }
    }
}
