//! `doctor ops` — read-only listing of open (orphan) Op records.
//!
//! # Source
//!
//! `specify_cli.doctor.ops.list_orphan_ops`: every `kitty-ops/*.jsonl` record
//! that carries no `completed` event and no closure on the append-only
//! `kitty-ops/op-closures.jsonl` spine (#4397). The doctor sweep closes
//! pre-existing records on the spine, never by rewriting the byte-frozen
//! per-record file, so a closed Op is one of those two facts.
//!
//! The named non-record files are excluded exactly as Source excludes them
//! (`ops-index.jsonl`, `lifecycle.jsonl`, `propagation-errors.jsonl` and the
//! closure spine itself).
//!
//! # What this module does not own
//!
//! The `kitty-ops` directory name, the closure-spine filename and the closure
//! ids themselves belong to the Op record/store owner
//! ([`sk_invocation`]): the adapter passes [`OpStoreLayout`] in, so this
//! diagnosis never recomposes a second Op-storage policy and never grows a
//! second closure reader. The `completed`-event scan *is* the doctor's own
//! policy in Source, so it lives here.
//!
//! `--close-stale` (Source's mutating sweep) is refused by the adapter before
//! this module is reached: nothing here writes, locks or closes anything.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sk_kernel::python_json::{PythonJsonValue, parse};
use sk_kernel::python_text::{is_whitespace, splitlines};

use crate::doctor::DoctorError;

/// The Op-store facts this diagnosis reads, supplied by the store owner.
#[derive(Clone, Copy, Debug)]
pub struct OpStoreLayout<'a> {
    /// The absolute Op record directory (`<repo>/kitty-ops`).
    pub ops_dir: &'a Path,
    /// The closure-spine filename inside it (`op-closures.jsonl`).
    pub closures_filename: &'a str,
    /// Record-file suffixes that are never Op records (`ops-index.jsonl`, …).
    pub non_record_files: &'a [&'a str],
    /// Invocation ids already closed on the closure spine.
    pub closed_ids: &'a BTreeSet<String>,
}

/// The open Op records, in sorted path order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpsAudit {
    /// Absolute paths of the open records.
    pub open_ops: Vec<PathBuf>,
}

impl OpsAudit {
    /// Whether no open Op record was found.
    pub fn healthy(&self) -> bool {
        self.open_ops.is_empty()
    }

    /// Source's `[{"path": <repo-relative>}]` payload.
    ///
    /// A path outside `root` keeps its absolute spelling rather than an
    /// invented relative one.
    pub fn to_json(&self, root: &Path) -> Value {
        Value::Array(
            self.open_ops
                .iter()
                .map(|path| {
                    json!({
                        "path": path
                            .strip_prefix(root)
                            .map(|relative| relative.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| path.to_string_lossy().into_owned()),
                    })
                })
                .collect(),
        )
    }
}

/// List the open Op records under `layout.ops_dir`.
///
/// A missing `kitty-ops/` directory is an empty list (Source's rule). Every
/// other listing failure is a typed refusal, and a record that cannot be read
/// as UTF-8 is a refusal too — a corrupt record must not be silently promoted
/// to an orphan or to a closure.
pub fn audit_open_ops(layout: &OpStoreLayout<'_>) -> Result<OpsAudit, DoctorError> {
    let entries = match std::fs::read_dir(layout.ops_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(OpsAudit::default()),
        Err(source) => {
            return Err(DoctorError::Io {
                context: "list the Op record directory",
                path: layout.ops_dir.to_path_buf(),
                source,
            });
        }
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| DoctorError::Io {
            context: "list the Op record directory",
            path: layout.ops_dir.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            // A non-UTF-8 name cannot be one of the named non-record files and
            // cannot equal a ULID closure id, so it is still inspected below.
            candidates.push(entry.path());
            continue;
        };
        if !name.ends_with(".jsonl")
            || layout.non_record_files.contains(&name)
            || name == layout.closures_filename
        {
            continue;
        }
        let stem = match name.strip_suffix(".jsonl") {
            Some(stem) => stem,
            None => continue,
        };
        if layout.closed_ids.contains(stem) {
            continue;
        }
        candidates.push(entry.path());
    }
    // Source `sorted(ops_dir.glob("*.jsonl"))` — byte order of the names.
    candidates.sort();

    let mut open_ops = Vec::new();
    for path in candidates {
        if !has_completed_event(&path)? {
            open_ops.push(path);
        }
    }
    Ok(OpsAudit { open_ops })
}

/// Whether the record carries a `completed` event anywhere in its lines.
///
/// Source iterates the file and stops at the first `{"event": "completed"}`
/// row, skipping non-JSON lines. A path that cannot be opened at all (a
/// directory named `x.jsonl`, a permission failure) is *not* a completed
/// record — Source's `except OSError: return False`. An opened but non-UTF-8
/// file is a typed refusal, because Source would abort there and a guess would
/// either hide or invent a closure.
fn has_completed_event(path: &Path) -> Result<bool, DoctorError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        // Source's `except OSError: return False`: the record is not readable
        // as a completed one. (A directory entry matching `*.jsonl` lands here.)
        Err(_) => return Ok(false),
    };
    let text = std::str::from_utf8(&bytes).map_err(|error| DoctorError::Io {
        context: "read an Op record",
        path: path.to_path_buf(),
        source: std::io::Error::new(
            ErrorKind::InvalidData,
            format!("Op record is not valid UTF-8: {error}"),
        ),
    })?;
    for line in splitlines(text) {
        let line = trim_python_whitespace(line);
        if line.is_empty() {
            continue;
        }
        let Ok(event) = parse(line) else {
            continue;
        };
        let PythonJsonValue::ExactObject(object) = event else {
            continue;
        };
        if matches!(
            object.get("event"),
            Some(PythonJsonValue::String(event)) if *event == python_string("completed")
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// An ASCII literal as an exact code-point Python string.
fn python_string(text: &str) -> sk_kernel::PythonString {
    sk_kernel::PythonString::from_scalars(text)
}

/// Python `str.strip()` — strip code points where `str.isspace()` is true.
fn trim_python_whitespace(text: &str) -> &str {
    let start = text
        .char_indices()
        .find(|(_, character)| !is_whitespace(u32::from(*character)))
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let end = text
        .char_indices()
        .rev()
        .find(|(_, character)| !is_whitespace(u32::from(*character)))
        .map(|(index, character)| index + character.len_utf8())
        .unwrap_or(start);
    &text[start..end]
}

impl<'a> OpStoreLayout<'a> {
    /// Build a layout from the store owner's facts.
    pub fn new(
        ops_dir: &'a Path,
        closures_filename: &'a str,
        non_record_files: &'a [&'a str],
        closed_ids: &'a BTreeSet<String>,
    ) -> Self {
        Self {
            ops_dir,
            closures_filename,
            non_record_files,
            closed_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent");
        }
        std::fs::write(path, contents).expect("fixture write");
    }

    const NON_RECORD: [&str; 3] = [
        "ops-index.jsonl",
        "lifecycle.jsonl",
        "propagation-errors.jsonl",
    ];

    fn audit(root: &Path, closed: &BTreeSet<String>) -> Result<OpsAudit, DoctorError> {
        audit_open_ops(&OpStoreLayout::new(
            &root.join("kitty-ops"),
            "op-closures.jsonl",
            &NON_RECORD,
            closed,
        ))
    }

    #[test]
    fn an_absent_op_directory_is_empty_not_an_error() {
        let root = tempfile::tempdir().expect("tempdir");
        assert!(
            audit(root.path(), &BTreeSet::new())
                .expect("audit")
                .healthy()
        );
    }

    #[test]
    fn an_open_record_is_reported_and_a_completed_one_is_not() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("kitty-ops");
        write(&dir.join("01OPEN.jsonl"), r#"{"event":"started"}"#);
        write(
            &dir.join("01DONE.jsonl"),
            r#"{"event":"started"}
{"event":"completed"}"#,
        );
        let report = audit(root.path(), &BTreeSet::new()).expect("audit");
        assert_eq!(
            report.open_ops,
            vec![dir.join("01OPEN.jsonl")],
            "{report:?}"
        );
    }

    #[test]
    fn the_named_non_record_files_and_the_spine_are_never_records() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("kitty-ops");
        for name in NON_RECORD.iter().copied().chain(["op-closures.jsonl"]) {
            write(&dir.join(name), r#"{"event":"not-a-record"}"#);
        }
        assert!(
            audit(root.path(), &BTreeSet::new())
                .expect("audit")
                .healthy()
        );
    }

    #[test]
    fn a_spine_closure_closes_a_record_without_a_completed_event() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("kitty-ops");
        write(&dir.join("01SWEPT.jsonl"), r#"{"event":"started"}"#);
        let closed: BTreeSet<String> = ["01SWEPT".to_owned()].into_iter().collect();
        assert!(audit(root.path(), &closed).expect("audit").healthy());
    }

    #[test]
    fn malformed_lines_do_not_count_as_a_closure() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("kitty-ops");
        write(&dir.join("01BAD.jsonl"), "{not json\n{\"event\": 7}");
        let report = audit(root.path(), &BTreeSet::new()).expect("audit");
        assert_eq!(report.open_ops, vec![dir.join("01BAD.jsonl")]);
    }

    #[test]
    fn the_json_payload_is_repo_relative() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("kitty-ops");
        write(&dir.join("01OPEN.jsonl"), r#"{"event":"started"}"#);
        let report = audit(root.path(), &BTreeSet::new()).expect("audit");
        assert_eq!(
            report.to_json(root.path()),
            json!([{"path": "kitty-ops/01OPEN.jsonl"}])
        );
    }
}
