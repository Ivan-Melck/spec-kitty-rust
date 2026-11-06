//! Compare-and-swap ref reads and writes.
//!
//! This is the only module that writes a ref. Every write is a
//! compare-and-swap expressed as one `git update-ref --stdin` transaction:
//!
//! * a set carries the value the caller observed (`update <ref> <new> <old>`);
//! * a create carries the zero id as its old value, so it fails if the ref
//!   already exists;
//! * a delete carries the tip the caller approved (`delete <ref> <old>`).
//!
//! The ref operands go on stdin, not on the option-parsed command line, and are
//! validated [`RefName`]s, so a name cannot become an option. A transaction is
//! atomic in git: if any command is rejected, none is applied. The plan records
//! the pre-state, and execution re-reads it before writing, so a concurrent
//! move is reported as [`GitError::PlanStale`] with the observed values rather
//! than being overwritten.

use std::collections::BTreeSet;
use std::ffi::OsString;

use crate::error::GitError;
use crate::oid::Oid;
use crate::refname::{RefName, StartPoint};
use crate::repository::GitRepository;

/// The observed value of one ref at plan time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefState {
    reference: RefName,
    oid: Option<Oid>,
}

impl RefState {
    pub(crate) fn new(reference: RefName, oid: Option<Oid>) -> Self {
        Self { reference, oid }
    }

    /// The ref this state describes.
    pub fn reference(&self) -> &RefName {
        &self.reference
    }

    /// Its value, or `None` when the ref does not exist.
    pub fn oid(&self) -> Option<&Oid> {
        self.oid.as_ref()
    }

    /// Whether the ref exists.
    pub fn exists(&self) -> bool {
        self.oid.is_some()
    }
}

/// One ref enumerated under a namespace prefix by a single `for-each-ref` read.
///
/// This is the general ref enumeration the crate previously lacked; it reuses
/// the same `for-each-ref` + [`Oid`] + [`RefName`] authorities as
/// [`GitRepository::read_ref`], so callers (for example a remote-tracking
/// roster) never build their own ref-listing argv or parse a second ref model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefListingEntry {
    reference: RefName,
    oid: Option<Oid>,
    symbolic_target: Option<RefName>,
}

impl RefListingEntry {
    /// The fully-qualified ref name.
    pub fn reference(&self) -> &RefName {
        &self.reference
    }

    /// The object id `for-each-ref` reported, or `None` when Git printed none
    /// (for example a broken loose ref).
    pub fn oid(&self) -> Option<&Oid> {
        self.oid.as_ref()
    }

    /// The symbolic target when this ref is symbolic (`refs/remotes/origin/HEAD`
    /// → `refs/remotes/origin/main`); `None` for an ordinary ref.
    pub fn symbolic_target(&self) -> Option<&RefName> {
        self.symbolic_target.as_ref()
    }

    /// Whether this ref is symbolic.
    pub fn is_symbolic(&self) -> bool {
        self.symbolic_target.is_some()
    }
}

/// One compare-and-swap ref operation.
///
/// Construct with [`RefChange::set`] or [`RefChange::delete`]; the fields are
/// private so an expected value is always present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefChange {
    /// Set a ref to `new`. `expected_old` is `None` only when the ref must not
    /// exist yet (the compare-and-swap create form).
    Set {
        /// The ref to move.
        reference: RefName,
        /// The new commit.
        new: Oid,
        /// The value the caller observed, or `None` for "must be absent".
        expected_old: Option<Oid>,
    },
    /// Delete a ref only while it still points at `expected_old`.
    Delete {
        /// The ref to delete.
        reference: RefName,
        /// The tip the caller approved for deletion.
        expected_old: Oid,
    },
}

impl RefChange {
    /// A compare-and-swap set.
    pub fn set(reference: RefName, new: Oid, expected_old: Option<Oid>) -> Self {
        Self::Set {
            reference,
            new,
            expected_old,
        }
    }

    /// A compare-and-swap create: the ref must not exist.
    pub fn create(reference: RefName, new: Oid) -> Self {
        Self::Set {
            reference,
            new,
            expected_old: None,
        }
    }

    /// A compare-and-swap delete.
    pub fn delete(reference: RefName, expected_old: Oid) -> Self {
        Self::Delete {
            reference,
            expected_old,
        }
    }

    /// The ref this change targets.
    pub fn reference(&self) -> &RefName {
        match self {
            Self::Set { reference, .. } | Self::Delete { reference, .. } => reference,
        }
    }

    /// The value this change requires the ref to hold now.
    fn expected_old(&self) -> Option<&Oid> {
        match self {
            Self::Set { expected_old, .. } => expected_old.as_ref(),
            Self::Delete { expected_old, .. } => Some(expected_old),
        }
    }
}

/// A validated, preflighted ref transaction.
///
/// Holds the observed pre-state so execution can prove nothing moved between
/// plan and write. It is not a capability to run arbitrary git commands: the
/// only effect [`GitRepository::apply_ref_transaction`] can have is the listed
/// compare-and-swap operations.
#[derive(Debug, Clone)]
pub struct RefTransactionPlan {
    message: String,
    core: RawRefTransactionPlan,
}

impl RefTransactionPlan {
    /// The reflog message that will be recorded.
    ///
    /// An exact-byte reflog message (a non-UTF-8 commit payload) is carried by
    /// the private raw plan instead and is not exposed as a lossy `&str`.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The operations in execution order.
    pub fn changes(&self) -> &[RefChange] {
        &self.core.changes
    }

    /// The ref values observed when the plan was prepared.
    pub fn observed(&self) -> &[RefState] {
        &self.core.observed
    }
}

/// The one ref-transaction value: exact reflog bytes plus the CAS operations.
///
/// [`RefTransactionPlan`] is the public textual view; a commit payload that is
/// not valid UTF-8 uses this private form so the reflog keeps the exact bytes the
/// reference records. Both forms share the same validation, pre-state readback,
/// CAS write, and post-write readback below — there is no second writer.
///
/// `message` is `None` for Source's landing form: `git update-ref` invoked with
/// NO `-m`, which git records as an EMPTY reflog subject. An empty string is not
/// the same request — git refuses `-m ""` outright ("Refusing to perform update
/// with empty message") — so absence is carried as absence, never faked.
#[derive(Debug, Clone)]
pub(crate) struct RawRefTransactionPlan {
    message: Option<Vec<u8>>,
    changes: Vec<RefChange>,
    observed: Vec<RefState>,
}

/// The evidence left by a successful ref transaction.
#[derive(Debug, Clone)]
pub struct RefTransactionOutcome {
    message: String,
    previous: Vec<RefState>,
    applied: Vec<RefState>,
}

impl RefTransactionOutcome {
    /// The reflog message git recorded.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The values that held before the transaction.
    pub fn previous(&self) -> &[RefState] {
        &self.previous
    }

    /// The values that hold after the transaction.
    pub fn applied(&self) -> &[RefState] {
        &self.applied
    }
}

/// A prepared ref transaction that carries NO reflog message.
///
/// This is Source's branch-advance form: `git update-ref <ref> <new> <old>`
/// with no `-m`, which git records with an empty reflog subject. It is a
/// SEPARATE plan from the strict [`RefTransactionPlan`] on purpose: the strict
/// contract (a non-empty, NUL-free message) still applies to every caller that
/// records one, while this one states the absence explicitly instead of
/// inventing an empty string — which git itself refuses.
///
/// Both plans run through the same single CAS executor; there is no second
/// writer, no private git invocation and no ref rewind here.
#[derive(Debug, Clone)]
pub struct SilentRefTransaction {
    changes: Vec<RefChange>,
    observed: Vec<RefState>,
}

impl SilentRefTransaction {
    /// The operations in execution order.
    pub fn changes(&self) -> &[RefChange] {
        &self.changes
    }

    /// The ref values observed when the plan was prepared.
    pub fn observed(&self) -> &[RefState] {
        &self.observed
    }
}

/// The maximum reflog message length accepted by this adapter.
const MAX_MESSAGE_LEN: usize = 2048;

/// Validate a textual reflog/plan message used by ref writes.
pub(crate) fn validate_message(message: &str) -> Result<(), GitError> {
    validate_message_bytes(message.as_bytes(), Some(MAX_MESSAGE_LEN))
}

/// Validate reflog/plan message bytes; the one shared rule set.
fn validate_message_bytes(message: &[u8], max_length: Option<usize>) -> Result<(), GitError> {
    if message.is_empty() || max_length.is_some_and(|limit| message.len() > limit) {
        return Err(GitError::InvalidPlan {
            detail: match max_length {
                Some(limit) => format!("a ref transaction message must be 1..={limit} bytes"),
                None => "a ref transaction message must not be empty".to_string(),
            },
        });
    }
    if message.contains(&0) {
        return Err(GitError::InvalidPlan {
            detail: "a ref transaction message must not contain NUL".to_string(),
        });
    }
    Ok(())
}

/// The `git update-ref -m` argument for an exact reflog payload.
fn reflog_os_string(message: &[u8]) -> Result<OsString, GitError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(message.to_vec()))
    }
    #[cfg(not(unix))]
    {
        match std::str::from_utf8(message) {
            Ok(text) => Ok(OsString::from(text)),
            Err(_) => Err(GitError::NonUtf8 {
                field: "reflog message",
            }),
        }
    }
}

impl GitRepository {
    /// Read the exact value of a fully-qualified ref.
    ///
    /// Enumerates the exact ref with `for-each-ref` and matches the full name,
    /// so a prefix-sharing ref (`refs/heads/foo/bar` for `refs/heads/foo`) can
    /// never answer for the requested ref. A missing ref is an empty listing,
    /// not a failed command.
    pub fn read_ref(&self, reference: &RefName) -> Result<Option<Oid>, GitError> {
        let output = self.process().checked(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)",
            reference.as_str(),
        ])?;
        let text = crate::path::utf8_field(&output.stdout, "ref listing")?;
        for line in text.lines() {
            let Some((name, oid)) = line.split_once('\0') else {
                return Err(GitError::InvalidOutput {
                    detail: format!("malformed for-each-ref line: {line:?}"),
                });
            };
            if name == reference.as_str() {
                return Ok(Some(Oid::parse(oid)?));
            }
        }
        Ok(None)
    }

    /// Test exact ref existence with `rev-parse --verify --quiet`.
    ///
    /// Every completed nonzero exit is false; spawn/timeout/capture failures
    /// remain typed errors. This does not require a commit or enumerate refs.
    pub fn ref_exists(&self, reference: &RefName) -> Result<bool, GitError> {
        let output =
            self.process()
                .run(&["rev-parse", "--verify", "--quiet", reference.as_str()])?;
        Ok(output.status.success())
    }

    /// Enumerate every ref under a namespace prefix in one `for-each-ref` read.
    ///
    /// *prefix* is a ref namespace such as `refs/remotes/`; it must start with
    /// `refs/` and contain no control byte. Git itself decides which refs match
    /// (a symbolic ref whose target is missing is not listed by Git, exactly as
    /// a `for-each-ref` run by the reference is not), so this is Source-shaped
    /// enumeration rather than a private ref model. Each entry carries the
    /// exact [`RefName`], the object id Git printed (when any) and the symbolic
    /// target (when the ref is symbolic).
    ///
    /// # Errors
    ///
    /// A failed `for-each-ref`, a non-UTF-8 ref field, or a field that is not a
    /// structurally valid ref/object id is a typed error; an empty result means
    /// Git succeeded and listed no refs under the prefix.
    pub fn refs_under(&self, prefix: &str) -> Result<Vec<RefListingEntry>, GitError> {
        validate_ref_prefix(prefix)?;
        let output = self.process().checked(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(symref)",
            prefix,
        ])?;
        let mut entries = Vec::new();
        for line in output.stdout.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split(|byte| *byte == 0);
            let reference = fields.next().unwrap_or_default();
            let objectname = fields.next().ok_or_else(|| GitError::InvalidOutput {
                detail: format!(
                    "malformed for-each-ref record {:?}: expected a ref name and object id",
                    String::from_utf8_lossy(line)
                ),
            })?;
            let symref = fields.next().ok_or_else(|| GitError::InvalidOutput {
                detail: format!(
                    "malformed for-each-ref record {:?}: expected a symbolic target field",
                    String::from_utf8_lossy(line)
                ),
            })?;
            if fields.next().is_some() {
                return Err(GitError::InvalidOutput {
                    detail: format!(
                        "malformed for-each-ref record {:?}: unexpected extra field",
                        String::from_utf8_lossy(line)
                    ),
                });
            }
            let name = crate::path::utf8_field(reference, "ref listing name")?;
            entries.push(RefListingEntry {
                reference: RefName::parse(&name)?,
                oid: optional_oid(objectname)?,
                symbolic_target: optional_ref(symref)?,
            });
        }
        Ok(entries)
    }

    /// Read a local branch's tip, or `None` when the branch does not exist.
    pub fn read_branch_tip(
        &self,
        branch: &crate::refname::BranchName,
    ) -> Result<Option<Oid>, GitError> {
        self.read_ref(&branch.as_ref_name())
    }

    /// Resolve a start point to a commit object id.
    ///
    /// The operand is a validated ref, a full object id, or `HEAD`; the
    /// `^{commit}` peel happens inside git and only the resulting full id is
    /// returned.
    pub fn resolve_commit(&self, start: &StartPoint) -> Result<Oid, GitError> {
        let peel = format!("{}^{{commit}}", start.operand());
        let output = self.process().allowing(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &peel,
            ],
            &[1],
        )?;
        if output.status.code() != Some(0) {
            return Err(GitError::RefMissing {
                name: start.operand().to_string(),
            });
        }
        let raw = crate::path::strip_line_feed(&output.stdout);
        if raw.is_empty() {
            return Err(GitError::RefMissing {
                name: start.operand().to_string(),
            });
        }
        let text = crate::path::utf8_field(raw, "commit id")?;
        Oid::parse(&text)
    }

    /// Whether `oid` names a commit object in this repository.
    ///
    /// The object type is checked exactly (`git cat-file -t`), so an annotated
    /// tag object is not a commit here even though it peels to one. A ref write
    /// therefore cannot point a branch at a tag object by mistake.
    pub fn commit_exists(&self, oid: &Oid) -> Result<bool, GitError> {
        let output = self
            .process()
            .allowing(&["cat-file", "-t", oid.as_str()], &[1, 128])?;
        if output.status.code() != Some(0) {
            return Ok(false);
        }
        Ok(crate::path::strip_line_feed(&output.stdout) == b"commit")
    }

    /// Whether `ancestor` is an ancestor of (or equal to) `descendant`.
    ///
    /// Only a real git failure is an error; git's "no" (exit 1) is `Ok(false)`.
    pub fn is_ancestor(&self, ancestor: &Oid, descendant: &Oid) -> Result<bool, GitError> {
        let output = self.process().allowing(
            &[
                "merge-base",
                "--is-ancestor",
                ancestor.as_str(),
                descendant.as_str(),
            ],
            &[1],
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(GitError::InvalidOutput {
                detail: "git merge-base --is-ancestor returned an unexpected status".to_string(),
            }),
        }
    }

    /// Prepare a ref transaction, validating every expected value now.
    ///
    /// The plan refuses an empty transaction, a duplicate ref, a reflog message
    /// with a NUL or longer than 2048 bytes, a set whose new object is not a
    /// commit here, and any ref whose current value differs from the caller's
    /// expected value. Nothing is written.
    pub fn plan_ref_transaction(
        &self,
        message: &str,
        changes: Vec<RefChange>,
    ) -> Result<RefTransactionPlan, GitError> {
        let core = self.plan_ref_transaction_message(
            Some(message.as_bytes()),
            changes,
            Some(MAX_MESSAGE_LEN),
        )?;
        Ok(RefTransactionPlan {
            message: message.to_string(),
            core,
        })
    }

    /// Prepare a ref transaction with an exact-byte reflog message.
    ///
    /// The same syntax validation and compare-and-swap planning as
    /// [`GitRepository::plan_ref_transaction`], without the public adapter's
    /// length budget: Source commit subjects must not be truncated. The message
    /// may be non-UTF-8 and never passes through a lossy decode or escaped
    /// `Debug` rendering. Git owns reflog whitespace normalization.
    pub(crate) fn plan_ref_transaction_bytes(
        &self,
        message: &[u8],
        changes: Vec<RefChange>,
    ) -> Result<RawRefTransactionPlan, GitError> {
        self.plan_ref_transaction_message(Some(message), changes, None)
    }

    /// Prepare a ref transaction whose reflog entry carries NO message.
    ///
    /// Same validation of the operations and the same compare-and-swap
    /// pre-state readback as [`GitRepository::plan_ref_transaction`]; only the
    /// reflog message is absent. Nothing is written.
    pub fn plan_silent_ref_transaction(
        &self,
        changes: Vec<RefChange>,
    ) -> Result<SilentRefTransaction, GitError> {
        let core = self.plan_ref_transaction_message(None, changes, None)?;
        Ok(SilentRefTransaction {
            changes: core.changes,
            observed: core.observed,
        })
    }

    fn plan_ref_transaction_message(
        &self,
        message: Option<&[u8]>,
        changes: Vec<RefChange>,
        max_message_length: Option<usize>,
    ) -> Result<RawRefTransactionPlan, GitError> {
        if let Some(message) = message {
            validate_message_bytes(message, max_message_length)?;
        }
        if changes.is_empty() {
            return Err(GitError::InvalidPlan {
                detail: "a ref transaction must contain at least one change".to_string(),
            });
        }

        let mut seen = BTreeSet::new();
        for change in &changes {
            if !seen.insert(change.reference().clone()) {
                return Err(GitError::InvalidPlan {
                    detail: format!(
                        "ref {} appears twice in one transaction",
                        change.reference()
                    ),
                });
            }
            if let RefChange::Set { new, .. } = change
                && !self.commit_exists(new)?
            {
                return Err(GitError::RefMissing {
                    name: new.to_string(),
                });
            }
        }

        let mut observed = Vec::with_capacity(changes.len());
        for change in &changes {
            let current = self.read_ref(change.reference())?;
            let expected = change.expected_old();
            if current.as_ref() != expected {
                return Err(GitError::PlanStale {
                    what: change.reference().to_string(),
                    detail: format!(
                        "expected {}, observed {}",
                        describe_expected(expected),
                        describe_observed(current.as_ref())
                    ),
                });
            }
            observed.push(RefState {
                reference: change.reference().clone(),
                oid: current,
            });
        }

        Ok(RawRefTransactionPlan {
            message: message.map(<[u8]>::to_vec),
            changes,
            observed,
        })
    }

    /// Execute a prepared ref transaction as one atomic `update-ref --stdin`.
    ///
    /// Re-reads every ref first: if any value moved after the plan was prepared
    /// the call returns [`GitError::PlanStale`] and writes nothing. The write
    /// itself carries the same expected values, so a move racing the write is
    /// refused by git rather than overwritten. There is no retry and no
    /// unconditional fallback.
    pub fn apply_ref_transaction(
        &self,
        plan: &RefTransactionPlan,
    ) -> Result<RefTransactionOutcome, GitError> {
        let applied = self.apply_ref_transaction_message(&plan.core)?;
        Ok(RefTransactionOutcome {
            message: plan.message.clone(),
            previous: plan.core.observed.clone(),
            applied,
        })
    }

    /// Execute a prepared exact-byte ref transaction.
    ///
    /// Same re-read, CAS write, and readback as
    /// [`GitRepository::apply_ref_transaction`]; only the reflog message
    /// argument is carried as exact bytes.
    pub(crate) fn apply_ref_transaction_raw(
        &self,
        plan: &RawRefTransactionPlan,
    ) -> Result<(), GitError> {
        self.apply_ref_transaction_message(plan).map(|_| ())
    }

    /// Execute a prepared message-less ref transaction.
    ///
    /// Same re-read, CAS write and post-write readback as
    /// [`GitRepository::apply_ref_transaction`], with `git update-ref` run
    /// WITHOUT `-m` — Source's landing form. Returns the applied values (there
    /// is no recorded message to report).
    pub fn apply_silent_ref_transaction(
        &self,
        plan: &SilentRefTransaction,
    ) -> Result<Vec<RefState>, GitError> {
        let core = RawRefTransactionPlan {
            message: None,
            changes: plan.changes.clone(),
            observed: plan.observed.clone(),
        };
        self.apply_ref_transaction_message(&core)
    }

    fn apply_ref_transaction_message(
        &self,
        plan: &RawRefTransactionPlan,
    ) -> Result<Vec<RefState>, GitError> {
        for state in &plan.observed {
            let current = self.read_ref(state.reference())?;
            if current != state.oid {
                return Err(GitError::PlanStale {
                    what: state.reference().to_string(),
                    detail: format!(
                        "planned {}, observed {}",
                        describe_observed(state.oid.as_ref()),
                        describe_observed(current.as_ref())
                    ),
                });
            }
        }

        let mut stdin = String::new();
        for change in &plan.changes {
            match change {
                RefChange::Set {
                    reference,
                    new,
                    expected_old,
                } => {
                    let old = expected_old.clone().unwrap_or_else(|| new.zero_like());
                    stdin.push_str(&format!("update {reference} {new} {old}\n"));
                }
                RefChange::Delete {
                    reference,
                    expected_old,
                } => {
                    stdin.push_str(&format!("delete {reference} {expected_old}\n"));
                }
            }
        }

        // One executor, two reflog forms: a recorded message (`-m <bytes>`) or
        // NO message at all. `-m ""` is not the message-less form — git refuses
        // it — so the absent case must omit the option, never pass an empty one.
        let mut args: Vec<OsString> = vec![OsString::from("update-ref")];
        let message = plan.message.as_deref().map(reflog_os_string).transpose()?;
        if let Some(message) = message {
            args.push(OsString::from("-m"));
            args.push(message);
        }
        args.push(OsString::from("--stdin"));
        let borrowed: Vec<&std::ffi::OsStr> = args.iter().map(OsString::as_os_str).collect();
        self.process()
            .checked_with_stdin(&borrowed, stdin.as_bytes())?;

        let mut applied = Vec::with_capacity(plan.changes.len());
        for change in &plan.changes {
            applied.push(RefState {
                reference: change.reference().clone(),
                oid: self.read_ref(change.reference())?,
            });
        }
        Ok(applied)
    }
}

fn validate_ref_prefix(prefix: &str) -> Result<(), GitError> {
    if !prefix.starts_with("refs/") {
        return Err(GitError::InvalidPlan {
            detail: format!("a ref namespace prefix must start with `refs/`, got {prefix:?}"),
        });
    }
    if prefix.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return Err(GitError::InvalidPlan {
            detail: "a ref namespace prefix must not contain a control byte".to_string(),
        });
    }
    Ok(())
}

/// Parse an optionally-empty `%(objectname)` field.
fn optional_oid(raw: &[u8]) -> Result<Option<Oid>, GitError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let text = crate::path::utf8_field(raw, "ref object id")?;
    Ok(Some(Oid::parse(&text)?))
}

/// Parse an optionally-empty `%(symref)` field.
fn optional_ref(raw: &[u8]) -> Result<Option<RefName>, GitError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let text = crate::path::utf8_field(raw, "symbolic ref target")?;
    Ok(Some(RefName::parse(&text)?))
}

fn describe_expected(expected: Option<&Oid>) -> String {
    match expected {
        Some(oid) => oid.to_string(),
        None => "absent".to_string(),
    }
}

fn describe_observed(observed: Option<&Oid>) -> String {
    match observed {
        Some(oid) => oid.to_string(),
        None => "absent".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_commit_audit_keeps_full_bytes_without_changing_public_budget() {
        for long in [vec![b'x'; 65_536], vec![0xff; 2_100]] {
            assert!(validate_message_bytes(&long, None).is_ok());
            assert!(validate_message_bytes(&long, Some(MAX_MESSAGE_LEN)).is_err());
        }
        for invalid in [b"".as_slice(), b"subject\0tail"] {
            assert!(validate_message_bytes(invalid, None).is_err());
            assert!(validate_message_bytes(invalid, Some(MAX_MESSAGE_LEN)).is_err());
        }
        assert!(validate_message(&"x".repeat(MAX_MESSAGE_LEN)).is_ok());
        assert!(validate_message(&"x".repeat(MAX_MESSAGE_LEN + 1)).is_err());
    }

    #[test]
    fn a_silent_plan_carries_no_message_instead_of_an_empty_one() {
        // The strict form still refuses an empty message; the silent form does
        // not invent one. The two are different requests, exactly as git treats
        // `update-ref` and `update-ref -m ""`.
        assert!(validate_message("").is_err());
        let silent = RawRefTransactionPlan {
            message: None,
            changes: Vec::new(),
            observed: Vec::new(),
        };
        assert!(silent.message.is_none());
    }

    #[test]
    fn describe_uses_absent_not_an_empty_value() {
        assert_eq!(describe_expected(None), "absent");
        assert_eq!(describe_observed(Some(&Oid::zero_sha1())), "0".repeat(40));
    }
}
