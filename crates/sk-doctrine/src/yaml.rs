//! Lossless YAML documents and rollback-safe, locked writes.
//!
//! User-authored `config.yaml`, `charter.yaml` and the activation ledger keep
//! their comments, key order and formatting: mutations go through the
//! `yaml-edit` lossless syntax tree, never through a wholesale re-serialise.
//!
//! Every write is:
//!
//! 1. prepared from the observed bytes and mode (no directory or file is
//!    created by planning);
//! 2. revalidated under an exclusive sidecar lock;
//! 3. applied with the kernel's atomic replacement primitive;
//! 4. read back, and restored from the observed bytes if verification fails.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_yaml_ng::Value as SerdeValue;
use yaml_edit::{Document, YamlValue};

use crate::error::DoctrineError;
use crate::lock::{FileLock, acquire_synthesis_lock, lock_path_for};

/// The default bounded wait for a mutation lock.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// A parsed YAML document that preserves comments and formatting on mutation.
pub struct YamlDocument {
    path: PathBuf,
    original: String,
    document: Document,
    value: YamlValue,
}

impl YamlDocument {
    /// Parse `text` as one YAML document.
    pub fn parse(text: &str, path: &Path) -> Result<Self, DoctrineError> {
        let semantic: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(text).map_err(|error| DoctrineError::Yaml {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        let source = match semantic {
            serde_yaml_ng::Value::Mapping(_) => text.to_owned(),
            serde_yaml_ng::Value::Null => {
                // A null/empty ledger is an empty mapping, never a scalar we
                // can silently fail to update. Preserve its comment text.
                let mut source = String::new();
                for line in text.lines() {
                    if line.trim_start().starts_with('#') {
                        source.push_str(line);
                        source.push('\n');
                    } else if let Some((_, comment)) = line.split_once('#') {
                        source.push('#');
                        source.push_str(comment);
                        source.push('\n');
                    }
                }
                source.push_str("{}\n");
                source
            }
            _ => {
                return Err(DoctrineError::YamlRootNotMapping {
                    path: path.to_path_buf(),
                });
            }
        };
        let document: Document =
            source
                .parse()
                .map_err(|error: yaml_edit::YamlError| DoctrineError::Yaml {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                })?;
        let value = YamlValue::from_document(&document).map_err(|error| DoctrineError::Yaml {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if !matches!(value, YamlValue::Mapping(_)) || document.as_mapping().is_none() {
            return Err(DoctrineError::YamlRootNotMapping {
                path: path.to_path_buf(),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            original: text.to_owned(),
            document,
            value,
        })
    }

    /// An empty mapping document for a file that does not exist yet.
    pub fn empty(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            original: String::new(),
            document: Document::new_mapping(),
            value: YamlValue::Mapping(Default::default()),
        }
    }

    /// Read and parse a file. An absent file is an empty mapping document.
    pub fn open(path: &Path) -> Result<Self, DoctrineError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::empty(path)),
            Err(source) => Err(DoctrineError::io("read YAML", path, source)),
        }
    }

    /// The document path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The parsed semantic value.
    pub fn value(&self) -> &YamlValue {
        &self.value
    }

    /// The top-level mapping, when the document is a mapping.
    pub fn mapping(&self) -> Option<&std::collections::BTreeMap<String, YamlValue>> {
        self.value.as_mapping()
    }

    /// A top-level semantic value.
    pub fn get(&self, key: &str) -> Option<&YamlValue> {
        self.mapping().and_then(|mapping| mapping.get(key))
    }

    /// A top-level string value.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(YamlValue::as_str)
    }

    /// A top-level list of string ids.
    ///
    /// `None` means the key is absent (or explicitly null). A present,
    /// non-list value is a typed error: a malformed activation surface must
    /// fail closed, never read as absent.
    pub fn get_string_list(&self, key: &str) -> Result<Option<Vec<String>>, DoctrineError> {
        let Some(value) = self.get(key) else {
            return Ok(None);
        };
        if matches!(value, YamlValue::Scalar(scalar) if scalar.scalar_type() == yaml_edit::ScalarType::Null)
        {
            return Ok(None);
        }
        let Some(items) = value.as_sequence() else {
            return Err(DoctrineError::InvalidLedgerValue {
                path: self.path.clone(),
                key: key.to_owned(),
                message: describe(value),
            });
        };
        let mut ids = Vec::with_capacity(items.len());
        for item in items {
            let Some(id) = item.as_str() else {
                return Err(DoctrineError::InvalidLedgerValue {
                    path: self.path.clone(),
                    key: key.to_owned(),
                    message: format!("item {item:?} is not a string id"),
                });
            };
            ids.push(id.to_owned());
        }
        Ok(Some(ids))
    }

    /// Whether a top-level key is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.mapping()
            .is_some_and(|mapping| mapping.contains_key(key))
    }

    /// The top-level keys in document order.
    pub fn keys(&self) -> Vec<String> {
        match self.mapping() {
            Some(mapping) => mapping.keys().cloned().collect(),
            None => Vec::new(),
        }
    }

    /// Replace a top-level key with a list of strings, preserving every other
    /// byte of the document (comments, quoting, ordering).
    ///
    /// When the change is an append or a suffix removal, the existing sequence
    /// node is mutated item-by-item so comments attached to retained entries
    /// survive. A reorder or a disjoint replacement necessarily replaces the
    /// sequence value, because the YAML syntax tree cannot reorder items while
    /// keeping their surrounding trivia attached.
    pub fn set_string_list(&mut self, key: &str, ids: &[String]) {
        if self.mutate_sequence_in_place(key, ids) {
            if let YamlValue::Mapping(mapping) = &mut self.value {
                mapping.insert(
                    key.to_owned(),
                    YamlValue::Sequence(
                        ids.iter().map(|id| YamlValue::from(id.as_str())).collect(),
                    ),
                );
            }
            return;
        }
        let value =
            YamlValue::Sequence(ids.iter().map(|id| YamlValue::from(id.as_str())).collect());
        self.document.set(key, value.clone());
        if let YamlValue::Mapping(mapping) = &mut self.value {
            mapping.insert(key.to_owned(), value);
        }
    }

    /// Mutate an existing sequence's items in place for an append/suffix removal.
    fn mutate_sequence_in_place(&mut self, key: &str, ids: &[String]) -> bool {
        let node = self.document.get(key);
        let Some(sequence) = node.as_ref().and_then(|node| node.as_sequence()) else {
            return false;
        };
        let old: Vec<String> = sequence
            .values()
            .filter_map(|node| node.as_scalar().map(|scalar| scalar.as_string()))
            .collect();
        if old == ids {
            return true;
        }
        if ids.starts_with(&old) {
            for id in &ids[old.len()..] {
                sequence.push(YamlValue::from(id.as_str()));
            }
            return true;
        }
        if old.starts_with(ids) {
            for _ in ids.len()..old.len() {
                let _ = sequence.remove(ids.len());
            }
            return true;
        }
        // Order-preserving subsequence: remove only the dropped items.
        let mut wanted = ids.iter().peekable();
        let mut remove = Vec::new();
        for (index, item) in old.iter().enumerate() {
            match wanted.peek() {
                Some(next) if *next == item => {
                    wanted.next();
                }
                _ => remove.push(index),
            }
        }
        if wanted.peek().is_some() {
            return false;
        }
        for index in remove.into_iter().rev() {
            let _ = sequence.remove(index);
        }
        true
    }

    /// Remove a top-level key, preserving every other byte.
    pub fn remove(&mut self, key: &str) {
        self.document.remove(key);
        if let YamlValue::Mapping(mapping) = &mut self.value {
            mapping.remove(key);
        }
    }

    /// Render the document.
    pub fn to_text(&self) -> String {
        self.document.to_string()
    }

    /// Whether the document changed since it was parsed.
    pub fn changed(&self) -> bool {
        self.to_text() != self.original
    }

    /// The observed source text.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Top-level keys outside `allowed`.
    pub fn unknown_keys<'a>(&self, allowed: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let allowed: std::collections::BTreeSet<&str> = allowed.into_iter().collect();
        self.keys()
            .into_iter()
            .filter(|key| !allowed.contains(key.as_str()))
            .collect()
    }
}

fn describe(value: &YamlValue) -> String {
    match value {
        YamlValue::Scalar(scalar) => format!("a scalar ({scalar:?})"),
        YamlValue::Sequence(_) => "a sequence".to_owned(),
        YamlValue::Mapping(_) => "a mapping".to_owned(),
        other => other.to_yaml_string(0),
    }
}

/// Check that a semantic YAML value is a mapping (or explicit null).
pub fn ensure_mapping(value: &SerdeValue, path: &Path) -> Result<(), DoctrineError> {
    match value {
        SerdeValue::Mapping(_) | SerdeValue::Null => Ok(()),
        _ => Err(DoctrineError::YamlRootNotMapping {
            path: path.to_path_buf(),
        }),
    }
}

/// Which lock family guards a prepared write.
///
/// The activation authority (and every `charter.yaml` write the activation
/// transaction performs) must use [`LockScope::Synthesis`], the canonical
/// cross-language lock Python's `charter activate` holds. A private sidecar
/// there would let a Python process and a native process interleave their
/// read-modify-write of the same file.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LockScope {
    /// A conventional `<target>.lock` sidecar (no cross-language writer).
    #[default]
    Sidecar,
    /// The run-wide charter synthesis lock, keyed on a repository root.
    Synthesis,
}

/// A prepared, rollback-safe YAML write.
#[derive(Clone, Debug)]
pub struct PreparedWrite {
    target: PathBuf,
    before: Option<Vec<u8>>,
    desired: Vec<u8>,
    mode: u32,
    lock_scope: LockScope,
    repo_root: Option<PathBuf>,
}

/// Observe a YAML target's exact bytes without following a final symlink.
///
/// `None` means the file does not exist. This is the plan-time observation an
/// activation transaction carries to commit, so a write that lost a race is
/// refused instead of overwriting the winner.
pub fn observe(target: &Path) -> Result<Option<Vec<u8>>, DoctrineError> {
    match fs::symlink_metadata(target) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(DoctrineError::UnsafeYamlTarget {
                    path: target.to_path_buf(),
                });
            }
            fs::read(target)
                .map(Some)
                .map_err(|source| DoctrineError::io("read YAML observation", target, source))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DoctrineError::io(
            "inspect YAML observation",
            target,
            source,
        )),
    }
}

/// The outcome of applying a prepared write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteOutcome {
    /// The target changed.
    Changed,
    /// The target already held the desired bytes; nothing was touched.
    Unchanged,
}

impl PreparedWrite {
    /// Observe `target` and prepare to replace it with `desired`.
    ///
    /// Planning creates nothing. A symlink or non-regular target is refused.
    pub fn prepare(target: &Path, desired: Vec<u8>) -> Result<Self, DoctrineError> {
        if let Ok(metadata) = fs::symlink_metadata(target)
            && (metadata.file_type().is_symlink() || !metadata.is_file())
        {
            return Err(DoctrineError::UnsafeYamlTarget {
                path: target.to_path_buf(),
            });
        }
        let (before, mode) = match fs::symlink_metadata(target) {
            Ok(metadata) => {
                let bytes = fs::read(target).map_err(|source| {
                    DoctrineError::io("read YAML before write", target, source)
                })?;
                (Some(bytes), file_mode(&metadata))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (None, 0o644),
            Err(source) => {
                return Err(DoctrineError::io(
                    "inspect YAML before write",
                    target,
                    source,
                ));
            }
        };
        Ok(Self {
            target: target.to_path_buf(),
            before,
            desired,
            mode,
            lock_scope: LockScope::Sidecar,
            repo_root: None,
        })
    }

    /// Guard this write with the run-wide charter synthesis lock of `repo_root`.
    pub fn with_synthesis_lock(mut self, repo_root: &Path) -> Self {
        self.lock_scope = LockScope::Synthesis;
        self.repo_root = Some(repo_root.to_path_buf());
        self
    }

    /// The lock family this write will take.
    pub fn lock_scope(&self) -> LockScope {
        self.lock_scope
    }

    /// The target path.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// The bytes observed when this write was prepared (`None` = absent).
    pub fn before(&self) -> Option<&[u8]> {
        self.before.as_deref()
    }

    /// The exact desired bytes.
    pub fn desired(&self) -> &[u8] {
        &self.desired
    }

    /// Whether applying this write would change the file.
    pub fn changed(&self) -> bool {
        match &self.before {
            None => !self.desired.is_empty(),
            Some(before) => before != &self.desired,
        }
    }

    /// Apply under an exclusive lock, rechecking the inputs observed at
    /// preparation time.
    ///
    /// On a verification failure the previous content (or absence) is restored;
    /// a failed write never leaves a half-updated document.
    pub fn apply(self, timeout: Duration) -> Result<WriteOutcome, DoctrineError> {
        let observed = self.before.clone();
        self.apply_observed(observed.as_deref(), timeout)
    }

    /// Apply under this write's lock, refusing when `observed` (a *plan-time*
    /// observation) is no longer the target's content.
    ///
    /// The check runs inside the lock, so the window between reading the
    /// plan-time observation and writing is one critical section: a concurrent
    /// writer either lands first (this refuses with `precondition_changed`) or
    /// waits for this write.
    pub fn apply_observed(
        self,
        observed: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<WriteOutcome, DoctrineError> {
        let _lock = self.lock(timeout)?;
        let current = observe(&self.target)?;
        if current.as_deref() != observed {
            return Err(DoctrineError::PreconditionChanged {
                path: self.target.clone(),
            });
        }
        self.apply_locked()
    }

    fn lock(&self, timeout: Duration) -> Result<FileLock, DoctrineError> {
        match self.lock_scope {
            LockScope::Sidecar => FileLock::acquire(&lock_path_for(&self.target), timeout),
            LockScope::Synthesis => {
                let repo_root = self
                    .repo_root
                    .as_deref()
                    .ok_or_else(|| DoctrineError::Yaml {
                        path: self.target.clone(),
                        message: "a synthesis-locked write requires its repository root".to_owned(),
                    })?;
                acquire_synthesis_lock(repo_root, timeout)
            }
        }
    }

    fn apply_locked(self) -> Result<WriteOutcome, DoctrineError> {
        let current = match fs::read(&self.target) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(DoctrineError::io("re-read YAML", &self.target, source));
            }
        };
        if current != self.before {
            return Err(DoctrineError::PreconditionChanged {
                path: self.target.clone(),
            });
        }
        if !self.changed() {
            return Ok(WriteOutcome::Unchanged);
        }
        let parent = self
            .target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|source| DoctrineError::io("create YAML directory", parent, source))?;
        }
        if let Err(error) = sk_kernel::atomic_write(&self.target, &self.desired) {
            return Err(DoctrineError::Kernel(error));
        }
        let written = fs::read(&self.target)
            .map_err(|source| DoctrineError::io("verify YAML write", &self.target, source))?;
        if written == self.desired {
            let _ = set_permissions(&self.target, self.mode);
            return Ok(WriteOutcome::Changed);
        }
        restore(&self.target, self.before.as_deref())?;
        Err(DoctrineError::PreconditionChanged {
            path: self.target.clone(),
        })
    }
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0o644
}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_permissions(path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

fn restore(target: &Path, before: Option<&[u8]>) -> Result<(), DoctrineError> {
    match before {
        Some(bytes) => {
            sk_kernel::atomic_write(target, bytes).map_err(DoctrineError::Kernel)?;
        }
        None => match fs::remove_file(target) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(DoctrineError::io("roll back YAML", target, source)),
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_and_comment_ledgers_become_real_mappings_without_losing_the_activation() {
        for original in ["", "null\n", "~ # retained\n", "# note\n"] {
            let mut document = YamlDocument::parse(original, Path::new("activation.yaml")).unwrap();
            document.set_string_list("activated_directives", &["001-arch".to_owned()]);
            let rendered = document.to_text();
            let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&rendered).unwrap();
            assert_eq!(value["activated_directives"][0].as_str(), Some("001-arch"));
            assert_eq!(
                document.get_string_list("activated_directives").unwrap(),
                Some(vec!["001-arch".to_owned()])
            );
            if original.contains('#') {
                assert!(rendered.contains(original.split_once('#').unwrap().1.trim()));
            }
        }
        for original in ["42\n", "hello\n", "[a, b]\n"] {
            assert!(matches!(
                YamlDocument::parse(original, Path::new("activation.yaml")),
                Err(DoctrineError::YamlRootNotMapping { .. })
            ));
        }
    }

    #[test]
    fn comments_and_key_order_survive_a_list_update() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        let original = "# top comment\nactivated_directives:\n  - a  # keep me\nother: value\n";
        fs::write(&path, original).unwrap();
        let mut document = YamlDocument::open(&path).unwrap();
        document.set_string_list("activated_directives", &["a".to_owned(), "b".to_owned()]);
        let text = document.to_text();
        assert!(text.contains("# top comment"));
        assert!(text.contains("# keep me"));
        assert!(text.contains("other: value"));
        assert!(text.contains("- b"));

        let prepared = PreparedWrite::prepare(&path, text.into_bytes()).unwrap();
        assert_eq!(
            prepared.apply(DEFAULT_LOCK_TIMEOUT).unwrap(),
            WriteOutcome::Changed
        );
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("# top comment"));
        assert!(written.contains("# keep me"));
    }

    #[test]
    fn empty_document_becomes_a_mapping_and_absent_file_is_created() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("activation.yaml");
        let mut document = YamlDocument::open(&path).unwrap();
        document.set_string_list("activated_kinds", &["directive".to_owned()]);
        let prepared = PreparedWrite::prepare(&path, document.to_text().into_bytes()).unwrap();
        assert_eq!(
            prepared.apply(DEFAULT_LOCK_TIMEOUT).unwrap(),
            WriteOutcome::Changed
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "activated_kinds:\n- directive\n"
        );
    }

    #[test]
    fn stale_input_is_refused_and_symlinks_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.yaml");
        fs::write(&path, "a: 1\n").unwrap();
        let prepared = PreparedWrite::prepare(&path, b"a: 2\n".to_vec()).unwrap();
        fs::write(&path, "a: 3\n").unwrap();
        assert!(matches!(
            prepared.apply(DEFAULT_LOCK_TIMEOUT),
            Err(DoctrineError::PreconditionChanged { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "a: 3\n");

        #[cfg(unix)]
        {
            let link = directory.path().join("link.yaml");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(matches!(
                PreparedWrite::prepare(&link, b"a: 4\n".to_vec()),
                Err(DoctrineError::UnsafeYamlTarget { .. })
            ));
        }
    }

    #[test]
    fn malformed_lists_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("activation.yaml");
        fs::write(&path, "activated_directives: 7\n").unwrap();
        let document = YamlDocument::open(&path).unwrap();
        assert!(matches!(
            document.get_string_list("activated_directives"),
            Err(DoctrineError::InvalidLedgerValue { .. })
        ));
    }
}
